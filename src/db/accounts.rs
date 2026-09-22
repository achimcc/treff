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
