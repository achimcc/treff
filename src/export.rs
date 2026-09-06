//! `treff export <file>` — a backup that survives a snapshot.
//!
//! Not a convenience. The data directory is snapshotted by the filesystem, and
//! a snapshot of a SQLite database **in WAL mode** catches a data file plus a
//! write-ahead log in an unknown relationship: usually recoverable, sometimes
//! not. `VACUUM INTO` writes a self-contained file instead, and it does so
//! against a live database — so the maintenance window can call it before the
//! snapshot rather than shutting the service down.

use crate::db::Db;
use std::path::Path;

pub async fn export(db: &Db, target: &Path) -> anyhow::Result<()> {
    // A backup that silently replaces an older backup is the most common way
    // to end up with two copies of the same damage.
    if target.exists() {
        anyhow::bail!("{} already exists", target.display());
    }

    // `VACUUM INTO` takes a literal, not a bound parameter: SQLite allows no
    // placeholder there. So the path is checked here instead of relying on a
    // binding to make it safe.
    let path = target
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("target path is not valid UTF-8"))?;
    if path.contains('\'') || path.contains('\0') {
        anyhow::bail!("target path must not contain a quote or a null byte");
    }

    // sqlx 0.9 refuses a dynamically built statement unless the caller says
    // out loud that they audited it. This is that audit: the only interpolated
    // value is a path that has just been checked for the one character that
    // could end the literal, and it comes from the command line rather than
    // from a request.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("VACUUM INTO '{path}'")))
        .execute(db.pool())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn somebody() -> crate::authz::Identity {
        crate::authz::Identity {
            subject: "s".into(),
            name: "N".into(),
            groups: vec![],
        }
    }

    #[tokio::test]
    async fn the_export_is_a_complete_database_of_its_own() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::Db::open(&dir.path().join("live.db"))
            .await
            .expect("open");
        crate::db::topics::create_topic(&db, "s", "k", "A title", "Some text", &somebody())
            .await
            .expect("topic");

        let target = dir.path().join("backup.db");
        export(&db, &target).await.expect("export");

        // Opened on its own — no WAL file, no source database next to it.
        let copy = crate::db::Db::open(&target).await.expect("open the copy");
        let (topics,): (i64,) = sqlx::query_as("SELECT count(*) FROM topics")
            .fetch_one(copy.pool())
            .await
            .expect("query");
        let (posts,): (i64,) = sqlx::query_as("SELECT count(*) FROM posts")
            .fetch_one(copy.pool())
            .await
            .expect("query");
        assert_eq!((topics, posts), (1, 1));
    }

    #[tokio::test]
    async fn it_refuses_to_overwrite_an_existing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::Db::open(&dir.path().join("live.db"))
            .await
            .expect("open");
        let target = dir.path().join("there.db");
        std::fs::write(&target, b"x").expect("write");

        assert!(export(&db, &target).await.is_err());
        assert_eq!(
            std::fs::read(&target).expect("read"),
            b"x",
            "the refusal overwrote the file anyway"
        );
    }

    #[tokio::test]
    async fn it_refuses_a_path_it_cannot_quote() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::Db::open(&dir.path().join("live.db"))
            .await
            .expect("open");
        assert!(
            export(&db, &dir.path().join("it's here.db")).await.is_err(),
            "a quote in the path reached the statement"
        );
    }

    #[tokio::test]
    async fn writing_while_the_export_runs_still_yields_a_consistent_copy() {
        // The maintenance window calls this on a running service. What must
        // not happen is a copy that holds half of a transaction.
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::Db::open(&dir.path().join("live.db"))
            .await
            .expect("open");
        for i in 0..20 {
            crate::db::topics::create_topic(
                &db,
                "s",
                "k",
                &format!("topic {i}"),
                "text",
                &somebody(),
            )
            .await
            .expect("topic");
        }

        let writer = {
            let db = db.clone();
            tokio::spawn(async move {
                for i in 20..40 {
                    let _ = crate::db::topics::create_topic(
                        &db,
                        "s",
                        "k",
                        &format!("topic {i}"),
                        "text",
                        &somebody(),
                    )
                    .await;
                }
            })
        };

        let target = dir.path().join("busy.db");
        let result = export(&db, &target).await;
        writer.await.expect("writer");
        result.expect("export while writing");

        // Every topic in the copy has its opening post: no half transaction.
        let copy = crate::db::Db::open(&target).await.expect("open the copy");
        let (orphans,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM topics t
             WHERE NOT EXISTS (SELECT 1 FROM posts p WHERE p.topic_id = t.id)",
        )
        .fetch_one(copy.pool())
        .await
        .expect("query");
        assert_eq!(orphans, 0, "the copy holds a topic without its post");
    }
}
