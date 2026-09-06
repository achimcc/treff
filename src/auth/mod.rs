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

    Identity {
        subject: subject.to_string(),
        name: name.unwrap_or(subject).to_string(),
        groups,
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
        sqlx::query(
            "INSERT INTO sessions (id, subject, name, groups_json, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&who.subject)
        .bind(&who.name)
        .bind(serde_json::to_string(&who.groups)?)
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
        }))
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

    #[tokio::test]
    async fn a_session_round_trips_and_can_be_destroyed() {
        let (_d, db) = test_db().await;
        let who = Identity {
            subject: "s".into(),
            name: "N".into(),
            groups: vec!["Household".into()],
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
