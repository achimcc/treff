//! What is owed to whom, and what has already gone out.
//!
//! Rows are written in the same transaction as the post they are about, and
//! drained by a background task. Two reasons, and both are the point: a reply
//! must not be lost because SMTP is down, and a request must not wait on the
//! network while somebody watches a spinner.

use crate::db::Db;

/// After this many failures a row is left alone. It stays in the table with
/// its last error, which is where somebody can find out why — a row that
/// vanishes takes the reason with it, and one that retries forever hides it
/// in a log nobody reads.
pub const MAX_ATTEMPTS: i64 = 8;

/// Backoff, in seconds, by attempt number. Bounded on purpose: an hour is long
/// enough that a broken mail server is not hammered, short enough that a fixed
/// one is noticed within the hour.
fn backoff(attempts: i64) -> i64 {
    match attempts {
        0 => 0,
        1 => 60,
        2 => 300,
        3 => 900,
        _ => 3600,
    }
}

#[derive(Debug, Clone)]
pub struct Owed {
    pub id: i64,
    pub subject: String,
    /// `mail` oder `webhook`. Eine Mail geht an jeden Abonnenten, ein Webhook
    /// einmal pro Beitrag — der Empfaenger dahinter verteilt selbst.
    pub kanal: String,
    pub topic_id: i64,
    pub post_id: i64,
    pub space: String,
    pub attempts: i64,
}

/// Queues one notification per follower, inside the caller's transaction.
///
/// `writer` is excluded by the query that finds the followers, not here — see
/// `subscriptions::followers_except`.
pub async fn queue_for_followers(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    space: &str,
    topic_id: i64,
    post_id: i64,
    writer: &str,
) -> anyhow::Result<u64> {
    let now = crate::db::topics::now();
    let result = sqlx::query(
        "INSERT INTO outbox (subject, topic_id, post_id, space, created_at, next_try_at, kanal)
         SELECT subject, ?, ?, ?, ?, ?, 'mail'
           FROM subscriptions
          WHERE topic_id = ? AND subject <> ?",
    )
    .bind(topic_id)
    .bind(post_id)
    .bind(space)
    .bind(now)
    .bind(now)
    .bind(topic_id)
    .bind(writer)
    .execute(&mut **tx)
    .await?;

    // GENAU EINE WEBHOOK-ZEILE, unabhaengig von der Zahl der Abonnenten — und
    // unabhaengig davon, ob ueberhaupt ein Webhook konfiguriert ist. Die
    // Datenbankschicht kennt die Konfiguration nicht, und sie soll sie nicht
    // kennen: Ist keiner eingerichtet, hakt der Sender die Zeile als „nichts
    // zu senden" ab, genau wie bei einem Konto ohne Adresse.
    sqlx::query(
        "INSERT INTO outbox (subject, topic_id, post_id, space, created_at, next_try_at, kanal)
         VALUES (?, ?, ?, ?, ?, ?, 'webhook')",
    )
    .bind(writer)
    .bind(topic_id)
    .bind(post_id)
    .bind(space)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    Ok(result.rows_affected())
}

/// What is owed and due now, oldest first.
pub async fn due(db: &Db, limit: i64) -> anyhow::Result<Vec<Owed>> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT id, subject, topic_id, post_id, space, attempts, kanal
           FROM outbox
          WHERE sent_at IS NULL AND next_try_at <= ? AND attempts < ?
          ORDER BY id
          LIMIT ?",
    )
    .bind(crate::db::topics::now())
    .bind(MAX_ATTEMPTS)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Owed {
            id: r.get("id"),
            subject: r.get("subject"),
            kanal: r.get("kanal"),
            topic_id: r.get("topic_id"),
            post_id: r.get("post_id"),
            space: r.get("space"),
            attempts: r.get("attempts"),
        })
        .collect())
}

pub async fn mark_sent(db: &Db, id: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE outbox SET sent_at = ? WHERE id = ?")
        .bind(crate::db::topics::now())
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok(())
}

/// Records a failure and when to try again.
///
/// The error text is stored TRUNCATED: it comes from another server, it ends
/// up in a table somebody reads, and an SMTP server that answers with a
/// kilobyte of prose should not be able to fill this one.
pub async fn mark_failed(db: &Db, id: i64, attempts: i64, error: &str) -> anyhow::Result<()> {
    let short: String = error.chars().take(500).collect();
    sqlx::query("UPDATE outbox SET attempts = ?, next_try_at = ?, last_error = ? WHERE id = ?")
        .bind(attempts + 1)
        .bind(crate::db::topics::now() + backoff(attempts + 1))
        .bind(short)
        .bind(id)
        .execute(db.pool())
        .await?;
    Ok(())
}

/// Who a notification was for, and what about.
///
/// The unsubscribe link carries only the row id — no subject, no name,
/// nothing that ends up legible in a proxy log. Everything else follows from
/// here.
pub async fn who_and_what(db: &Db, id: i64) -> anyhow::Result<Option<(String, i64)>> {
    use sqlx::Row;
    Ok(
        sqlx::query("SELECT subject, topic_id FROM outbox WHERE id = ?")
            .bind(id)
            .fetch_optional(db.pool())
            .await?
            .map(|r| (r.get("subject"), r.get("topic_id"))),
    )
}

/// Rows nobody will try again — for a status page, and for the test that
/// proves giving up is a state and not a disappearance.
pub async fn given_up(db: &Db) -> anyhow::Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE sent_at IS NULL AND attempts >= ?")
            .bind(MAX_ATTEMPTS)
            .fetch_one(db.pool())
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::Identity;

    async fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&dir.path().join("t.db")).await.expect("open");
        (dir, db)
    }

    fn who(subject: &str) -> Identity {
        Identity {
            subject: subject.into(),
            name: subject.into(),
            groups: vec!["Household".into()],
            email: Some(format!("{subject}@example.org")),
        }
    }

    /// One reply, one row per follower — and none for the person replying.
    #[tokio::test]
    async fn a_reply_owes_one_notification_per_follower() {
        let (_d, db) = db().await;
        let ada = who("ada");
        let ben = who("ben");
        let topic =
            crate::db::topics::create_topic(&db, "forum.example.org", "general", "T", "B", &ada)
                .await
                .expect("topic");
        // ada follows by having opened it; ben follows by replying.
        crate::db::topics::add_reply(&db, topic, "First", &ben)
            .await
            .expect("reply");

        let owed = due(&db, 100).await.expect("due");
        let per_mail: Vec<&str> = owed
            .iter()
            .filter(|o| o.kanal == "mail")
            .map(|o| o.subject.as_str())
            .collect();
        assert_eq!(
            per_mail,
            vec!["ada"],
            "ben wrote it, so only ada is owed a mail: {owed:?}"
        );

        // And exactly ONE webhook, however many people follow: what is behind
        // it fans out on its own, and one notification per subscriber would be
        // a stack of identical messages on one telephone.
        assert_eq!(
            owed.iter().filter(|o| o.kanal == "webhook").count(),
            1,
            "{owed:?}"
        );
    }

    #[tokio::test]
    async fn a_failure_backs_off_and_eventually_gives_up_visibly() {
        let (_d, db) = db().await;
        let ada = who("ada");
        let topic =
            crate::db::topics::create_topic(&db, "forum.example.org", "general", "T", "B", &ada)
                .await
                .expect("topic");
        crate::db::topics::add_reply(&db, topic, "First", &who("ben"))
            .await
            .expect("reply");

        let owed = due(&db, 10).await.expect("due");
        let row = owed
            .iter()
            .find(|o| o.kanal == "mail")
            .expect("a mail row")
            .clone();

        // The first failure pushes it into the future, so it is not due now.
        mark_failed(&db, row.id, row.attempts, "connection refused")
            .await
            .expect("fail");
        assert!(
            !due(&db, 100)
                .await
                .expect("due")
                .iter()
                .any(|o| o.id == row.id),
            "a failed row waits before it is tried again"
        );

        // And after enough failures it is left alone — still there, with a
        // reason.
        for attempt in 1..MAX_ATTEMPTS {
            mark_failed(&db, row.id, attempt, "connection refused")
                .await
                .expect("fail");
        }
        assert_eq!(given_up(&db).await.expect("count"), 1);
    }

    #[tokio::test]
    async fn what_went_out_is_not_owed_again() {
        let (_d, db) = db().await;
        let ada = who("ada");
        let topic =
            crate::db::topics::create_topic(&db, "forum.example.org", "general", "T", "B", &ada)
                .await
                .expect("topic");
        crate::db::topics::add_reply(&db, topic, "First", &who("ben"))
            .await
            .expect("reply");

        // Both rows — the mail and the one webhook — are drained the same way.
        for row in due(&db, 100).await.expect("due") {
            mark_sent(&db, row.id).await.expect("sent");
        }
        assert!(due(&db, 100).await.expect("due").is_empty());
    }
}
