//! Identity and session. The OIDC conversation itself lives in [`oidc`]; this
//! module holds what *we* decide: how a set of claims becomes an identity, and
//! how a signed-in person is remembered between requests.

pub mod oidc;

use crate::authz::Identity;
use crate::db::Db;
use rand::TryRng;

pub struct OidcSettings {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub group_claim: String,
}

/// A setting with no sensible default. Empty and unset are the same thing
/// here: both mean the operator has not decided, and treff does not guess.
fn required(name: &str) -> anyhow::Result<String> {
    let value = std::env::var(name).unwrap_or_default();
    if value.trim().is_empty() {
        anyhow::bail!("{name} is not set; treff does not start without it");
    }
    Ok(value)
}

impl OidcSettings {
    pub fn from_env() -> anyhow::Result<Self> {
        // The secret comes from a FILE, not from the environment: an
        // environment variable is readable in /proc/<pid>/environ and shows up
        // in every `systemctl show`.
        let secret_file = required("TREFF_OIDC_CLIENT_SECRET_FILE")?;
        Ok(Self {
            issuer: required("TREFF_OIDC_ISSUER")?,
            client_id: required("TREFF_OIDC_CLIENT_ID")?,
            client_secret: {
                let secret = std::fs::read_to_string(&secret_file)
                    .map_err(|e| anyhow::anyhow!("cannot read {secret_file}: {e}"))?
                    .trim()
                    .to_string();
                // An EMPTY file is the interesting case, not a missing one. A
                // secret that was never filled in — a credential that did not
                // arrive, a template rendered from nothing — otherwise lets
                // the service start and look healthy, and fails only when
                // somebody tries to sign in. Refusing here makes the fault
                // visible where it happened.
                if secret.is_empty() {
                    anyhow::bail!(
                        "{secret_file} is empty; treff will not start without a client secret"
                    );
                }
                secret
            },
            group_claim: std::env::var("TREFF_OIDC_GROUP_CLAIM")
                .unwrap_or_else(|_| "groups".to_string()),
        })
    }
}

/// Claims in, identity out.
///
/// Everything unexpected results in **no** groups: a missing claim, a claim
/// that is a string rather than a list, entries that are not strings. The
/// alternative — guessing — would turn a provider's misconfiguration into an
/// open door.
pub fn claims_to_identity(
    subject: &str,
    name: Option<&str>,
    claims: &serde_json::Value,
    group_claim: &str,
) -> Identity {
    let groups = claims
        .get(group_claim)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|g| g.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // A claim that is not a string is not an address. Same rule as the
    // groups: anything unexpected becomes nothing, never a guess.
    let email = claims
        .get("email")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(String::from);

    Identity {
        subject: subject.to_string(),
        name: name.unwrap_or(subject).to_string(),
        groups,
        email,
    }
}

pub struct Sessions;

const SESSION_SECONDS: i64 = 60 * 60 * 12;

/// 32 bytes from the operating system's CSPRNG, hex encoded. This identifier
/// *is* the credential — a guessable one would sign the guesser in as someone
/// else — so a failure to get randomness is an error, never a fallback.
pub fn random_id() -> anyhow::Result<String> {
    let mut b = [0u8; 32];
    rand::rngs::SysRng.try_fill_bytes(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

impl Sessions {
    pub async fn create(db: &Db, who: &Identity) -> anyhow::Result<String> {
        let id = random_id()?;
        let now = crate::db::topics::now();

        // THE ACCOUNT ROW IS WHY MAIL WORKS ON A THURSDAY. A session expires
        // after twelve hours; a subscription does not. Keeping the address
        // only on the session would mean notifying whoever happens to be
        // logged in, which is the opposite of what a notification is for.
        sqlx::query(
            "INSERT INTO accounts (subject, name, email, seen_at) VALUES (?, ?, ?, ?)
             ON CONFLICT(subject) DO UPDATE SET name = excluded.name,
                                                email = excluded.email,
                                                seen_at = excluded.seen_at",
        )
        .bind(&who.subject)
        .bind(&who.name)
        .bind(&who.email)
        .bind(now)
        .execute(db.pool())
        .await?;

        sqlx::query(
            "INSERT INTO sessions (id, subject, name, groups_json, email, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&who.subject)
        .bind(&who.name)
        .bind(serde_json::to_string(&who.groups)?)
        .bind(&who.email)
        .bind(now)
        .bind(now + SESSION_SECONDS)
        .execute(db.pool())
        .await?;
        Ok(id)
    }

    /// Expiry is part of the query, not of a later check: a session that is
    /// over must be unfindable, not merely unused.
    pub async fn load(db: &Db, id: &str) -> anyhow::Result<Option<Identity>> {
        use sqlx::Row;
        let now = crate::db::topics::now();
        let Some(row) = sqlx::query("SELECT * FROM sessions WHERE id = ? AND expires_at > ?")
            .bind(id)
            .bind(now)
            .fetch_optional(db.pool())
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(Identity {
            subject: row.get("subject"),
            name: row.get("name"),
            // Unreadable groups mean none. Fail closed even against our own
            // storage.
            groups: serde_json::from_str(&row.get::<String, _>("groups_json")).unwrap_or_default(),
            email: row.get("email"),
        }))
    }

    /// The address of somebody who is not here right now.
    ///
    /// `None` covers both "no such account" and "no address on it": the
    /// caller does the same thing either way, and telling them apart would
    /// only invite a branch that treats one as an error.
    pub async fn address_of(db: &Db, subject: &str) -> anyhow::Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT email FROM accounts WHERE subject = ?")
                .bind(subject)
                .fetch_optional(db.pool())
                .await?
                .flatten(),
        )
    }

    pub async fn destroy(db: &Db, id: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(id)
            .execute(db.pool())
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// AN ADDRESS IS OPTIONAL AND IS NOT AN IDENTITY.
    ///
    /// Optional, because a provider that sends none is a provider whose people
    /// get no mail — not one whose people are locked out. And not an identity,
    /// because `sub` is: an address can be edited at many providers, and
    /// matching on one would hand an account to whoever claims it next.
    #[test]
    fn an_address_is_read_when_there_is_one_and_missed_without_complaint() {
        let with = claims_to_identity(
            "sub-1",
            Some("Ada"),
            &json!({ "groups": ["Household"], "email": "ada@example.org" }),
            "groups",
        );
        assert_eq!(with.email.as_deref(), Some("ada@example.org"));

        let without = claims_to_identity("sub-1", Some("Ada"), &json!({}), "groups");
        assert_eq!(without.email, None, "no address is not an error");
        assert_eq!(without.subject, "sub-1", "and changes nothing else");

        // A claim that is not a string is not an address.
        let wrong = claims_to_identity("s", None, &json!({ "email": 42 }), "groups");
        assert_eq!(wrong.email, None);
    }

    #[test]
    fn groups_come_from_the_configured_claim() {
        let claims = json!({ "groups": ["Household", "Writers"] });
        let id = claims_to_identity("sub-1", Some("Ada"), &claims, "groups");
        assert_eq!(id.subject, "sub-1");
        assert_eq!(id.name, "Ada");
        assert_eq!(id.groups, vec!["Household", "Writers"]);
    }

    #[test]
    fn a_different_claim_name_is_honoured() {
        let claims = json!({ "roles": ["Friends"] });
        let id = claims_to_identity("s", None, &claims, "roles");
        assert_eq!(id.groups, vec!["Friends"]);
    }

    #[test]
    fn a_missing_claim_means_no_groups_not_all_groups() {
        let id = claims_to_identity("s", None, &json!({}), "groups");
        assert!(id.groups.is_empty());
    }

    #[test]
    fn a_claim_of_the_wrong_shape_means_no_groups() {
        // A string instead of a list is a misconfiguration at the provider.
        // It must let nobody in.
        let id = claims_to_identity("s", None, &json!({ "groups": "Household" }), "groups");
        assert!(id.groups.is_empty());
    }

    #[test]
    fn non_string_entries_are_dropped_not_stringified() {
        let id = claims_to_identity("s", None, &json!({ "groups": ["A", 7, null] }), "groups");
        assert_eq!(id.groups, vec!["A"]);
    }

    #[test]
    fn without_a_name_the_subject_stands_in() {
        let id = claims_to_identity("sub-9", None, &json!({}), "groups");
        assert_eq!(id.name, "sub-9");
    }

    /// THE CASE THAT MAKES NOTIFICATIONS WORK AT ALL: reaching somebody who
    /// is not signed in.
    #[tokio::test]
    async fn an_address_outlives_the_session_it_arrived_with() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::Db::open(&dir.path().join("t.db"))
            .await
            .expect("open");

        let ada = Identity {
            subject: "s1".into(),
            name: "Ada".into(),
            groups: vec!["Household".into()],
            email: Some("ada@example.org".into()),
        };
        let id = Sessions::create(&db, &ada).await.expect("session");
        Sessions::destroy(&db, &id).await.expect("sign out");

        assert_eq!(
            Sessions::address_of(&db, "s1")
                .await
                .expect("lookup")
                .as_deref(),
            Some("ada@example.org"),
            "signing out is not a reason to stop being reachable"
        );
        assert_eq!(
            Sessions::address_of(&db, "nobody").await.expect("lookup"),
            None
        );
    }

    /// And a changed address at the provider wins, on the next sign-in.
    #[tokio::test]
    async fn the_account_takes_the_newer_address() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::Db::open(&dir.path().join("t.db"))
            .await
            .expect("open");
        let mut ada = Identity {
            subject: "s1".into(),
            name: "Ada".into(),
            groups: vec![],
            email: Some("old@example.org".into()),
        };
        Sessions::create(&db, &ada).await.expect("session");
        ada.email = Some("new@example.org".into());
        ada.name = "Ada B.".into();
        Sessions::create(&db, &ada).await.expect("session");

        assert_eq!(
            Sessions::address_of(&db, "s1")
                .await
                .expect("lookup")
                .as_deref(),
            Some("new@example.org")
        );
    }

    /// Storing an address is not the point — carrying it back out is.
    #[tokio::test]
    async fn a_session_carries_the_address_back_out_and_survives_without_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::Db::open(&dir.path().join("t.db"))
            .await
            .expect("open");

        let with = Identity {
            subject: "s1".into(),
            name: "Ada".into(),
            groups: vec!["Household".into()],
            email: Some("ada@example.org".into()),
        };
        let id = Sessions::create(&db, &with).await.expect("session");
        let back = Sessions::load(&db, &id).await.expect("load").expect("some");
        assert_eq!(back.email.as_deref(), Some("ada@example.org"));

        let without = Identity {
            subject: "s2".into(),
            name: "Ben".into(),
            groups: vec!["Household".into()],
            email: None,
        };
        let id2 = Sessions::create(&db, &without).await.expect("session");
        let back2 = Sessions::load(&db, &id2)
            .await
            .expect("load")
            .expect("some");
        assert_eq!(back2.email, None, "no address is not a reason to refuse");
        assert_eq!(back2.groups, vec!["Household"], "and nothing else changes");
    }

    #[tokio::test]
    async fn a_session_round_trips_and_can_be_destroyed() {
        let (_d, db) = test_db().await;
        let who = Identity {
            subject: "s".into(),
            name: "N".into(),
            groups: vec!["Household".into()],
            email: None,
        };

        let sid = Sessions::create(&db, &who).await.expect("create");
        let back = Sessions::load(&db, &sid)
            .await
            .expect("load")
            .expect("session");
        assert_eq!(back.subject, "s");
        assert_eq!(back.groups, vec!["Household"]);

        Sessions::destroy(&db, &sid).await.expect("destroy");
        assert!(Sessions::load(&db, &sid).await.expect("load").is_none());
    }

    #[tokio::test]
    async fn session_ids_are_long_and_unpredictable() {
        // The identifier is the whole credential: whoever guesses one is
        // signed in as that person.
        let (_d, db) = test_db().await;
        let who = Identity {
            subject: "s".into(),
            name: "N".into(),
            groups: vec![],
            email: None,
        };
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let sid = Sessions::create(&db, &who).await.expect("create");
            assert!(sid.len() >= 32, "session id is short: {sid}");
            assert!(seen.insert(sid), "a session id repeated");
        }
    }

    #[tokio::test]
    async fn an_expired_session_does_not_load() {
        let (_d, db) = test_db().await;
        sqlx::query(
            "INSERT INTO sessions (id, subject, name, groups_json, created_at, expires_at)
             VALUES ('old', 's', 'N', '[]', 0, 1)",
        )
        .execute(db.pool())
        .await
        .expect("insert");
        assert!(Sessions::load(&db, "old").await.expect("load").is_none());
    }

    #[tokio::test]
    async fn an_unknown_session_id_is_not_an_error() {
        let (_d, db) = test_db().await;
        assert!(
            Sessions::load(&db, "no-such-thing")
                .await
                .expect("load")
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_damaged_groups_column_grants_nothing() {
        // Fail closed even against our own storage: unreadable groups must
        // mean none, never all.
        let (_d, db) = test_db().await;
        sqlx::query(
            "INSERT INTO sessions (id, subject, name, groups_json, created_at, expires_at)
             VALUES ('bent', 's', 'N', 'not json', 0, 99999999999)",
        )
        .execute(db.pool())
        .await
        .expect("insert");
        let id = Sessions::load(&db, "bent")
            .await
            .expect("load")
            .expect("session");
        assert!(id.groups.is_empty());
    }

    #[test]
    fn the_settings_refuse_to_be_empty() {
        temp_env::with_vars(
            [
                ("TREFF_OIDC_ISSUER", None::<&str>),
                ("TREFF_OIDC_CLIENT_ID", None),
                ("TREFF_OIDC_CLIENT_SECRET_FILE", None),
            ],
            || assert!(OidcSettings::from_env().is_err()),
        );
    }

    #[test]
    fn the_settings_refuse_a_missing_secret_file() {
        // The path is configured but there is nothing behind it. Starting
        // anyway would mean running with an empty client secret.
        temp_env::with_vars(
            [
                ("TREFF_OIDC_ISSUER", Some("https://id.example.org/")),
                ("TREFF_OIDC_CLIENT_ID", Some("treff")),
                ("TREFF_OIDC_CLIENT_SECRET_FILE", Some("/nonexistent/secret")),
            ],
            || assert!(OidcSettings::from_env().is_err()),
        );
    }

    #[test]
    fn an_empty_secret_file_is_refused_like_a_missing_one() {
        // The credential arrived but carried nothing. Starting anyway would
        // mean a service that looks healthy and cannot sign anyone in.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secret");
        std::fs::write(&path, "   \n").expect("write");
        temp_env::with_vars(
            [
                ("TREFF_OIDC_ISSUER", Some("https://id.example.org/")),
                ("TREFF_OIDC_CLIENT_ID", Some("treff")),
                (
                    "TREFF_OIDC_CLIENT_SECRET_FILE",
                    Some(path.to_str().expect("utf-8")),
                ),
            ],
            || assert!(OidcSettings::from_env().is_err()),
        );
    }

    #[test]
    fn the_settings_read_the_secret_from_the_file_and_default_the_claim() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secret");
        std::fs::write(&path, "  s3cret\n").expect("write");
        temp_env::with_vars(
            [
                ("TREFF_OIDC_ISSUER", Some("https://id.example.org/")),
                ("TREFF_OIDC_CLIENT_ID", Some("treff")),
                (
                    "TREFF_OIDC_CLIENT_SECRET_FILE",
                    Some(path.to_str().expect("utf-8")),
                ),
                ("TREFF_OIDC_GROUP_CLAIM", None),
            ],
            || {
                let s = OidcSettings::from_env().expect("settings");
                assert_eq!(s.client_secret, "s3cret", "the file is read and trimmed");
                assert_eq!(s.group_claim, "groups", "the default claim name");
            },
        );
    }

    async fn test_db() -> (tempfile::TempDir, crate::db::Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::Db::open(&dir.path().join("t.db"))
            .await
            .expect("open");
        (dir, db)
    }
}
