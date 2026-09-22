//! The database: one SQLite file, opened once at startup and handed around as
//! a pool.

pub mod accounts;
pub mod attachments;
pub mod directory;
pub mod events;
pub mod inbox;
pub mod outbox;
pub mod search;
pub mod subscriptions;
pub mod topics;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::path::Path;

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
    /// "Somebody's unread may have changed in this space" — announced after
    /// the commit of every write that can change it, so the bell's streams
    /// can ask again (plan-stage-5, task 3). On the database and not on the
    /// router, because the public and the internal listener share the
    /// database, and a change made through one must reach streams on both.
    changes: tokio::sync::broadcast::Sender<String>,
}

/// What `Db::changed` sends when the space is not known at the call site: every
/// stream asks again. Cheap, and rarer than a precise announcement.
pub const EVERY_SPACE: &str = "*";

impl Db {
    /// Opens (and creates) the database and brings it up to the current
    /// schema.
    ///
    /// `journal_mode` and `foreign_keys` are set through the **connection
    /// options**, not by a one-off `PRAGMA`: the pool opens further
    /// connections later, and a pragma issued on one of them says nothing
    /// about the rest. Getting that wrong is invisible until the day two
    /// requests arrive at once.
    ///
    /// `foreign_keys(true)` restates what sqlx already defaults to — measured,
    /// not assumed: with the line deleted the constraint tests stay green,
    /// and only `foreign_keys(false)` turns them red. It stays because this
    /// is a promise the schema depends on (`ON DELETE CASCADE`), and a
    /// promise that lives in someone else's default is not one we made.
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        // A small buffer is enough: a receiver that falls behind is told so
        // (`Lagged`) and asks again, which is what it would have done anyway.
        let (changes, _) = tokio::sync::broadcast::channel(64);
        Ok(Self { pool, changes })
    }

    /// Announces that unread entries in `space` may have changed. After the
    /// commit, never inside the transaction: a stream that asks before the
    /// commit reads the old state and has nothing left to wake it.
    pub fn changed(&self, space: &str) {
        // No receiver is not an error: nobody is watching.
        let _ = self.changes.send(space.to_string());
    }

    pub fn changes(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.changes.subscribe()
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn temporary() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&dir.path().join("treff.db")).await.expect("open");
        (dir, db)
    }

    #[tokio::test]
    async fn the_migrations_run() {
        let (_dir, db) = temporary().await;
        let (n,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'topics'",
        )
        .fetch_one(db.pool())
        .await
        .expect("query");
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn opening_an_existing_database_again_is_harmless() {
        // A restart must not be a migration adventure.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("treff.db");
        let first = Db::open(&path).await.expect("open");
        sqlx::query(
            "INSERT INTO topics (space, category, title, author_subject, author_name, created_at, updated_at)
             VALUES ('h', 'k', 't', 's', 'n', 0, 0)",
        )
        .execute(first.pool())
        .await
        .expect("insert");
        drop(first);

        let second = Db::open(&path).await.expect("open again");
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM topics")
            .fetch_one(second.pool())
            .await
            .expect("query");
        assert_eq!(n, 1, "the row from before the restart is gone");
    }

    #[tokio::test]
    async fn write_ahead_logging_is_on() {
        // Without WAL a reader blocks every writer. That is not a matter of
        // taste but the difference between "slow" and "database is locked"
        // under two concurrent requests.
        let (_dir, db) = temporary().await;
        let (mode,): (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(db.pool())
            .await
            .expect("query");
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[tokio::test]
    async fn every_pooled_connection_enforces_foreign_keys() {
        // `foreign_keys` is per connection, not per database — so asking one
        // connection proves nothing about the others. Hold several at once
        // and ask each of them.
        let (_dir, db) = temporary().await;
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(db.pool().acquire().await.expect("connection"));
        }
        for (i, conn) in held.iter_mut().enumerate() {
            let (on,): (i64,) = sqlx::query_as("PRAGMA foreign_keys")
                .fetch_one(&mut **conn)
                .await
                .expect("query");
            assert_eq!(on, 1, "connection {i} does not enforce foreign keys");
        }
    }

    #[tokio::test]
    async fn a_post_without_its_topic_is_refused() {
        // SQLite does not check foreign keys by default. Without the pragma,
        // replies hang off deleted topics and surface nowhere.
        let (_dir, db) = temporary().await;
        let refused = sqlx::query(
            "INSERT INTO posts (topic_id, body_markdown, author_subject, author_name, created_at, updated_at)
             VALUES (9999, 'x', 's', 'n', 0, 0)",
        )
        .execute(db.pool())
        .await;
        assert!(
            refused.is_err(),
            "a post without its topic must not be storable"
        );
    }

    #[tokio::test]
    async fn deleting_a_topic_takes_its_posts_with_it() {
        let (_dir, db) = temporary().await;
        sqlx::query(
            "INSERT INTO topics (id, space, category, title, author_subject, author_name, created_at, updated_at)
             VALUES (1, 'h', 'k', 't', 's', 'n', 0, 0)",
        )
        .execute(db.pool())
        .await
        .expect("topic");
        sqlx::query(
            "INSERT INTO posts (topic_id, body_markdown, author_subject, author_name, created_at, updated_at)
             VALUES (1, 'x', 's', 'n', 0, 0)",
        )
        .execute(db.pool())
        .await
        .expect("post");

        sqlx::query("DELETE FROM topics WHERE id = 1")
            .execute(db.pool())
            .await
            .expect("delete");

        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM posts")
            .fetch_one(db.pool())
            .await
            .expect("query");
        assert_eq!(n, 0, "orphaned posts survived their topic");
    }
}
