//! Who is known here, for the two questions a mention asks: which account
//! answers to this handle, and may it read the space?
//!
//! Only accounts that signed in at least once are in this table. Somebody who
//! never came has no row, no handle and no address — a mention of them is
//! plain text, which is also what it looks like.

use crate::db::Db;
use sqlx::Row;
use std::collections::HashMap;

/// An account that answers to a handle, with the groups of its last sign-in.
#[derive(Debug, Clone)]
pub struct Known {
    pub subject: String,
    pub handle: String,
    pub groups: Vec<String>,
}

/// The accounts behind these handles. Handles that nobody holds are simply
/// not in the result.
pub async fn by_handles(db: &Db, handles: &[String]) -> anyhow::Result<Vec<Known>> {
    if handles.is_empty() {
        return Ok(vec![]);
    }
    // `json_each` rather than a string of `?` placeholders: one statement,
    // however many handles a post names, and nothing spliced into SQL.
    let rows = sqlx::query(
        "SELECT subject, handle, groups_json FROM accounts
          WHERE handle IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(handles)?)
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .iter()
        .map(|r| Known {
            subject: r.get("subject"),
            handle: r.get("handle"),
            // Unreadable groups mean none — fail closed, as in `Sessions::load`.
            groups: serde_json::from_str(&r.get::<String, _>("groups_json")).unwrap_or_default(),
        })
        .collect())
}

/// Whether the account may read `space` by the groups of its last sign-in.
/// No account means no.
pub async fn may_read_now(
    db: &Db,
    subject: &str,
    space: &crate::config::Space,
) -> anyhow::Result<bool> {
    let groups: Option<String> =
        sqlx::query_scalar("SELECT groups_json FROM accounts WHERE subject = ?")
            .bind(subject)
            .fetch_optional(db.pool())
            .await?;
    let Some(groups) = groups else {
        return Ok(false);
    };
    let who = crate::authz::Identity {
        subject: subject.to_string(),
        name: String::new(),
        groups: serde_json::from_str(&groups).unwrap_or_default(),
        email: None,
        handle: None,
    };
    Ok(crate::authz::may_read(&who, space))
}

/// Whether this person wants a mail when mentioned. An account that is not
/// there has nobody to mail, which reads as no.
pub async fn wants_mention_mail(db: &Db, subject: &str) -> anyhow::Result<bool> {
    let on: Option<i64> = sqlx::query_scalar("SELECT mention_mail FROM accounts WHERE subject = ?")
        .bind(subject)
        .fetch_optional(db.pool())
        .await?;
    Ok(on == Some(1))
}

/// Idempotent, because the link in a mail gets clicked twice.
pub async fn set_mention_mail(db: &Db, subject: &str, on: bool) -> anyhow::Result<()> {
    sqlx::query("UPDATE accounts SET mention_mail = ? WHERE subject = ?")
        .bind(i64::from(on))
        .bind(subject)
        .execute(db.pool())
        .await?;
    Ok(())
}

/// Subject → handle, for putting the handle next to a name.
pub async fn handles_of(db: &Db, subjects: &[String]) -> anyhow::Result<HashMap<String, String>> {
    if subjects.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        "SELECT subject, handle FROM accounts
          WHERE handle IS NOT NULL AND subject IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(subjects)?)
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .iter()
        .map(|r| (r.get("subject"), r.get("handle")))
        .collect())
}
