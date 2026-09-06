//! Who hears about what.
//!
//! A subscription is a person and a topic. It is created by **writing** — see
//! `follow_by_writing` — and removed by asking, either in the forum or through
//! the link in a notification.

use crate::db::Db;

/// Follows a topic. Idempotent: the primary key makes a second call a no-op
/// rather than a second mail, and it is called on every reply.
pub async fn follow(db: &Db, subject: &str, topic_id: i64) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO subscriptions (subject, topic_id, created_at)
         VALUES (?, ?, ?) ON CONFLICT DO NOTHING",
    )
    .bind(subject)
    .bind(topic_id)
    .bind(crate::db::topics::now())
    .execute(db.pool())
    .await?;
    Ok(())
}

/// The same thing inside a caller's transaction.
///
/// It exists as its own function because `follow` takes the pool and a
/// transaction is not a pool — and because the transaction is the point: a
/// post that exists while its subscription does not is a silent, permanent
/// inconsistency, and the crash that causes it happens once a year.
pub async fn follow_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    subject: &str,
    topic_id: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO subscriptions (subject, topic_id, created_at)
         VALUES (?, ?, ?) ON CONFLICT DO NOTHING",
    )
    .bind(subject)
    .bind(topic_id)
    .bind(crate::db::topics::now())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Stops following. Also idempotent — the link in a mail may be clicked twice,
/// and the second click must not be an error page.
pub async fn unfollow(db: &Db, subject: &str, topic_id: i64) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM subscriptions WHERE subject = ? AND topic_id = ?")
        .bind(subject)
        .bind(topic_id)
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn is_following(db: &Db, subject: &str, topic_id: i64) -> anyhow::Result<bool> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM subscriptions WHERE subject = ? AND topic_id = ?")
            .bind(subject)
            .bind(topic_id)
            .fetch_optional(db.pool())
            .await?;
    Ok(found.is_some())
}

/// Everybody following a topic **except** `writer`.
///
/// The exception is not a nicety. A forum that mails you your own words
/// teaches people to filter it away, and then it teaches them nothing else.
pub async fn followers_except(db: &Db, topic_id: i64, writer: &str) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT subject FROM subscriptions WHERE topic_id = ? AND subject <> ? ORDER BY subject",
    )
    .bind(topic_id)
    .bind(writer)
    .fetch_all(db.pool())
    .await?)
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

    async fn a_topic(db: &Db, author: &Identity) -> i64 {
        crate::db::topics::create_topic(db, "forum.example.org", "general", "T", "B", author)
            .await
            .expect("topic")
    }

    #[tokio::test]
    async fn following_twice_is_following_once() {
        let (_d, db) = db().await;
        let topic = a_topic(&db, &who("ada")).await;

        follow(&db, "ben", topic).await.expect("follow");
        follow(&db, "ben", topic).await.expect("again");

        // `ada` opened the topic and is subscribed by that act, so the list is
        // taken from ada's point of view — which is also the only point of
        // view the sender ever asks from.
        assert_eq!(
            followers_except(&db, topic, "ada").await.expect("list"),
            vec!["ben"]
        );
    }

    #[tokio::test]
    async fn the_writer_is_not_told_about_their_own_post() {
        let (_d, db) = db().await;
        let topic = a_topic(&db, &who("ada")).await;
        follow(&db, "ada", topic).await.expect("follow");
        follow(&db, "ben", topic).await.expect("follow");

        let told = followers_except(&db, topic, "ada").await.expect("list");
        assert_eq!(
            told,
            vec!["ben"],
            "ada wrote it, so ada is not told about it"
        );
    }

    #[tokio::test]
    async fn unfollowing_twice_is_not_an_error() {
        let (_d, db) = db().await;
        let topic = a_topic(&db, &who("ada")).await;
        follow(&db, "ben", topic).await.expect("follow");

        unfollow(&db, "ben", topic).await.expect("once");
        unfollow(&db, "ben", topic)
            .await
            .expect("twice — a link in a mail gets clicked twice");

        assert!(!is_following(&db, "ben", topic).await.expect("check"));
        assert!(
            followers_except(&db, topic, "ada")
                .await
                .expect("list")
                .is_empty(),
            "ada still follows what ada opened; ben does not"
        );
    }

    /// The point of the whole task: nobody has to remember to subscribe.
    #[tokio::test]
    async fn writing_subscribes_you_and_replying_does_too() {
        let (_d, db) = db().await;
        let ada = who("ada");
        let ben = who("ben");

        let topic = a_topic(&db, &ada).await;
        assert!(
            is_following(&db, "ada", topic).await.expect("check"),
            "opening a topic follows it"
        );

        crate::db::topics::add_reply(&db, topic, "Yes", &ben)
            .await
            .expect("reply");
        assert!(
            is_following(&db, "ben", topic).await.expect("check"),
            "replying follows it"
        );

        // And a second reply is still one subscription.
        crate::db::topics::add_reply(&db, topic, "Also this", &ben)
            .await
            .expect("reply");
        assert_eq!(
            followers_except(&db, topic, "nobody").await.expect("list"),
            vec!["ada", "ben"]
        );
    }

    /// A subscription to a topic that no longer exists would be a row nobody
    /// can reach and a mail nobody can act on.
    #[tokio::test]
    async fn deleting_a_topic_takes_its_subscriptions() {
        let (_d, db) = db().await;
        let ada = who("ada");
        let topic = a_topic(&db, &ada).await;
        follow(&db, "ben", topic).await.expect("follow");

        // Deleting the opening post takes the topic with it.
        let posts = crate::db::topics::load_topic(&db, "forum.example.org", topic)
            .await
            .expect("load")
            .expect("some")
            .1;
        crate::db::topics::delete_post(&db, "forum.example.org", posts[0].id, &ada)
            .await
            .expect("delete");

        assert!(
            followers_except(&db, topic, "nobody")
                .await
                .expect("list")
                .is_empty(),
            "both of them — the foreign key carries subscriptions out, not a \
             handler that remembers to"
        );
    }
}
