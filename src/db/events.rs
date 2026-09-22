//! Events from other services, for one person — the bell's third kind of
//! entry, beside replies and mentions.
//!
//! What arrives here is somebody else's data, and it is shown on our page
//! with a link that people follow from it. So nothing is stored that
//! [`checked`] has not checked, field by field, and nothing is repaired: an
//! event that does not fit is refused with the reason.

use crate::db::Db;

/// What an event is about. A closed list: a new kind is a decision, with its
/// own wording, not a string somebody else chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    FilmAvailable,
    FilmFailed,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::FilmAvailable => "film_available",
            Kind::FilmFailed => "film_failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "film_available" => Some(Kind::FilmAvailable),
            "film_failed" => Some(Kind::FilmFailed),
            _ => None,
        }
    }
}

/// An event as it arrives: unchecked.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Raw {
    pub handle: String,
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    pub source_key: String,
}

/// An event that may be stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub handle: String,
    pub kind: Kind,
    pub title: String,
    pub link: Option<String>,
    pub reason: Option<String>,
    pub source_key: String,
}

const MAX_TITLE: usize = 200;
const MAX_REASON: usize = 300;

/// Every field, or the reason why not.
pub fn checked(raw: Raw, events: &crate::config::Events) -> Result<Checked, &'static str> {
    let handle = crate::auth::checked_handle(&raw.handle).ok_or("not a handle")?;
    let kind = Kind::parse(&raw.kind).ok_or("unknown kind")?;
    let title = one_line(&raw.title, MAX_TITLE).ok_or("a title is 1 to 200 characters")?;
    let reason = match raw.reason.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(r) => Some(one_line(r, MAX_REASON).ok_or("a reason is at most 300 characters")?),
    };
    let link = match raw.link.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(l) => Some(checked_link(l, &events.link_hosts).ok_or("that link is not allowed")?),
    };
    let key_ok = !raw.source_key.is_empty()
        && raw.source_key.len() <= 64
        && raw
            .source_key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":_-".contains(&b));
    if !key_ok {
        return Err("a source_key is [A-Za-z0-9:_-], 1 to 64");
    }
    Ok(Checked {
        handle,
        kind,
        title,
        link,
        reason,
        source_key: raw.source_key,
    })
}

/// Trimmed, within its length in characters, and without control characters
/// — a line break in a title is somebody else's layout on our page.
fn one_line(s: &str, max: usize) -> Option<String> {
    let t = s.trim();
    let fits = !t.is_empty() && t.chars().count() <= max && !t.chars().any(char::is_control);
    fits.then(|| t.to_string())
}

/// `https://` on an allowed host, no user or password in it, nothing else.
/// Parsed, not matched: `https://jellyfin.example.org@elsewhere.example/`
/// starts with the right words and goes somewhere else.
fn checked_link(l: &str, hosts: &[String]) -> Option<String> {
    let url = url::Url::parse(l).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let fits = url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && hosts.contains(&host);
    fits.then(|| url.to_string())
}

/// Stores a checked event in `space`. `Ok(true)` if it is new, `Ok(false)` if
/// the same `source_key` and kind were already there — a webhook that
/// arrives twice.
pub async fn take(db: &Db, space: &str, event: &Checked) -> anyhow::Result<bool> {
    let inserted = sqlx::query(
        "INSERT INTO events (handle, space, kind, title, link, reason, source_key, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (source_key, kind) DO NOTHING",
    )
    .bind(&event.handle)
    .bind(space)
    .bind(event.kind.as_str())
    .bind(&event.title)
    .bind(&event.link)
    .bind(&event.reason)
    .bind(&event.source_key)
    .bind(crate::db::topics::now())
    .execute(db.pool())
    .await?
    .rows_affected();
    if inserted == 1 {
        db.changed(space);
    }
    Ok(inserted == 1)
}

/// Opening an event: marks it read and returns where it leads — only if it
/// belongs to this account's handle and to this space. `None` for everything
/// else, one answer for "not yours", "not here" and "not there": telling them
/// apart would say something about entries the asker may not see.
///
/// `Some(None)`: it is yours, and it has no link.
pub async fn open(
    db: &Db,
    subject: &str,
    space: &str,
    id: i64,
) -> anyhow::Result<Option<Option<String>>> {
    use sqlx::Row;
    let row = sqlx::query(
        "UPDATE events SET read_at = coalesce(read_at, ?)
          WHERE id = ? AND space = ?
            AND handle = (SELECT handle FROM accounts WHERE subject = ?)
         RETURNING link",
    )
    .bind(crate::db::topics::now())
    .bind(id)
    .bind(space)
    .bind(subject)
    .fetch_optional(db.pool())
    .await?;
    if row.is_some() {
        db.changed(space);
    }
    Ok(row.map(|r| r.get("link")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events() -> crate::config::Events {
        crate::config::Events {
            space: "forum.example.org".into(),
            link_hosts: vec!["jellyfin.example.org".into()],
        }
    }

    fn raw() -> Raw {
        Raw {
            handle: "Konrad".into(),
            kind: "film_available".into(),
            title: "Dune: Part Two".into(),
            link: Some("https://jellyfin.example.org/web/#/details?id=abc".into()),
            reason: None,
            source_key: "seerr:42".into(),
        }
    }

    #[test]
    fn a_good_event_is_taken_as_it_is() {
        let c = checked(raw(), &events()).expect("fine");
        assert_eq!(c.handle, "konrad");
        assert_eq!(c.kind, Kind::FilmAvailable);
        assert_eq!(c.title, "Dune: Part Two");
        assert!(
            c.link
                .as_deref()
                .is_some_and(|l| l.starts_with("https://jellyfin.example.org/"))
        );
    }

    /// EVERY FIELD IS SOMEBODY ELSE'S. Each of these is refused, and none is
    /// repaired into something that would be taken.
    #[test]
    fn what_does_not_fit_is_refused() {
        let e = events();
        let with = |f: fn(&mut Raw)| {
            let mut r = raw();
            f(&mut r);
            checked(r, &e)
        };
        assert!(
            with(|r| r.handle = "Konrad Müller".into()).is_err(),
            "not a handle"
        );
        assert!(
            with(|r| r.kind = "film_approved".into()).is_err(),
            "unknown kind"
        );
        assert!(with(|r| r.title = "  ".into()).is_err(), "empty title");
        assert!(with(|r| r.title = "x".repeat(201)).is_err(), "long title");
        assert!(
            with(|r| r.title = "two\nlines".into()).is_err(),
            "a line break"
        );
        assert!(
            with(|r| r.reason = Some("y".repeat(301))).is_err(),
            "long reason"
        );
        assert!(
            with(|r| r.link = Some("http://jellyfin.example.org/".into())).is_err(),
            "not https"
        );
        assert!(
            with(|r| r.link = Some("https://elsewhere.example/".into())).is_err(),
            "other host"
        );
        assert!(
            with(|r| r.link = Some("https://jellyfin.example.org@elsewhere.example/".into()))
                .is_err(),
            "the right words in front of another host"
        );
        assert!(
            with(|r| r.link = Some("javascript:alert(1)".into())).is_err(),
            "a script"
        );
        assert!(with(|r| r.source_key = "".into()).is_err(), "no key");
        assert!(
            with(|r| r.source_key = "a b".into()).is_err(),
            "a space in the key"
        );
        assert!(
            checked(
                raw(),
                &crate::config::Events {
                    space: "x".into(),
                    link_hosts: vec![]
                }
            )
            .is_err(),
            "no allowed host, no link"
        );
        assert!(with(|r| r.link = None).is_ok(), "a link is optional");
    }

    async fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&dir.path().join("t.db")).await.expect("open");
        (dir, db)
    }

    /// A WEBHOOK ARRIVES TWICE. One entry.
    #[tokio::test]
    async fn the_same_event_twice_is_one_entry() {
        let (_d, db) = db().await;
        let c = checked(raw(), &events()).expect("fine");
        assert!(take(&db, "forum.example.org", &c).await.expect("take"));
        assert!(!take(&db, "forum.example.org", &c).await.expect("again"));
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
            .fetch_one(db.pool())
            .await
            .expect("count");
        assert_eq!(n, 1);
    }
}
