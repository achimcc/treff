//! Attachments: the row next to the file.
//!
//! The bytes live on disk under a name **we** generate; the database holds
//! what we detected about them. Nothing the uploader chose — not the file
//! name, not the announced content type — is stored or trusted.

use crate::db::Db;
use crate::media::MediaType;
use sqlx::Row;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Attachment {
    pub id: String,
    pub post_id: i64,
    pub media_type: MediaType,
    pub byte_size: i64,
}

/// Where an attachment's bytes are, under the data directory.
///
/// The id is generated and hex-only, and the extension comes from detection,
/// so this path cannot be steered from outside — no `..`, no absolute path, no
/// second dot.
pub fn path_of(data_dir: &Path, id: &str, media_type: MediaType) -> PathBuf {
    data_dir
        .join("attachments")
        .join(format!("{id}.{}", media_type.extension()))
}

pub async fn record(
    db: &Db,
    id: &str,
    post_id: i64,
    media_type: MediaType,
    byte_size: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO attachments (id, post_id, media_type, byte_size, created_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(post_id)
    .bind(media_type.mime())
    .bind(byte_size)
    .bind(crate::db::topics::now())
    .execute(db.pool())
    .await?;
    Ok(())
}

/// An attachment, but only if it hangs under a post in **this** space and its
/// topic is not withdrawn. The space is in the query for the same reason as
/// everywhere else: an id says nothing about who may see it.
pub async fn load(db: &Db, space: &str, id: &str) -> anyhow::Result<Option<Attachment>> {
    let Some(row) = sqlx::query(
        "SELECT a.id AS id, a.post_id AS post_id, a.media_type AS media_type,
                a.byte_size AS byte_size
         FROM attachments a
         JOIN posts p ON p.id = a.post_id
         JOIN topics t ON t.id = p.topic_id
         WHERE a.id = ? AND t.space = ? AND t.hidden = 0",
    )
    .bind(id)
    .bind(space)
    .fetch_optional(db.pool())
    .await?
    else {
        return Ok(None);
    };

    let mime: String = row.get("media_type");
    let Some(media_type) = MediaType::from_mime(&mime) else {
        // A type we no longer serve. Refusing is right: the allow list is the
        // allow list at serving time too, not only at upload time.
        return Ok(None);
    };

    Ok(Some(Attachment {
        id: row.get("id"),
        post_id: row.get("post_id"),
        media_type,
        byte_size: row.get("byte_size"),
    }))
}
