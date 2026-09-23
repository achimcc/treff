//! What a person has not seen yet — the bell.
//!
//! Rows are written in the same transaction as the post they are about, for
//! the reason the outbox is: a reply that exists while its entry does not is a
//! reply nobody is shown, and nothing later can tell that it happened.
//!
//! **Replies are bundled per topic, mentions stand alone.** Three answers in
//! one lively thread are one line ("3 new replies in …"), because a list that
//! grows by one line per answer stops being "everything at a glance". A
//! mention is addressed to one person; bundling it with other people's replies
//! would hide exactly the line that was meant for them.

use crate::db::Db;
use sqlx::Row;

/// One line of `/notifications`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// New posts in a topic this person follows.
    Replies {
        topic_id: i64,
        topic_title: String,
        count: i64,
        latest_author: String,
        latest_at: i64,
        /// Where the link goes: the first post of the bundle, so reading
        /// starts where the person left off rather than at the end.
        first_post_id: i64,
        unread: bool,
    },
    /// Something happened elsewhere for this person — a film request came
    /// through or failed (`db::events`).
    Event {
        id: i64,
        kind: crate::db::events::Kind,
        title: String,
        reason: Option<String>,
        at: i64,
        unread: bool,
    },
    /// Somebody wrote `@handle` for this person.
    Mention {
        topic_id: i64,
        topic_title: String,
        post_id: i64,
        author: String,
        at: i64,
        unread: bool,
    },
    /// People like this person's post — one line per post, the count and
    /// the latest name read from `likes` when the bell is shown (ADR 0008).
    Likes {
        topic_id: i64,
        topic_title: String,
        post_id: i64,
        count: i64,
        latest_name: String,
        at: i64,
        unread: bool,
    },
}

impl Entry {
    pub fn unread(&self) -> bool {
        match self {
            Entry::Replies { unread, .. }
            | Entry::Mention { unread, .. }
            | Entry::Likes { unread, .. }
            | Entry::Event { unread, .. } => *unread,
        }
    }

    pub fn at(&self) -> i64 {
        match self {
            Entry::Replies { latest_at, .. } => *latest_at,
            Entry::Mention { at, .. } | Entry::Likes { at, .. } | Entry::Event { at, .. } => *at,
        }
    }
}

/// One `reply` entry per follower of the topic except the writer, inside the
/// caller's transaction.
///
/// `ON CONFLICT DO NOTHING`: a follower who was mentioned in the same post
/// already has an entry for it, and that one stays.
pub async fn note_replies_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    space: &str,
    topic_id: i64,
    post_id: i64,
    writer: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO inbox (subject, space, topic_id, post_id, reason, created_at)
         SELECT subject, ?, ?, ?, 'reply', ?
           FROM subscriptions
          WHERE topic_id = ? AND subject <> ?
         ON CONFLICT DO NOTHING",
    )
    .bind(space)
    .bind(topic_id)
    .bind(post_id)
    .bind(crate::db::topics::now())
    .bind(topic_id)
    .bind(writer)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// One `mention` entry per subject, inside the caller's transaction, and the
/// subjects for whom one was actually written.
///
/// The return value is how an edit tells a NEW mention from an old one: a
/// person who already has an entry for this post gets no second one, and is
/// not in the list — so the caller queues a mail only for the people this
/// call has just told.
///
/// Call it BEFORE `note_replies_in` in the same transaction: the primary key
/// then keeps the mention and turns the follower's reply entry into a no-op.
pub async fn note_mentions_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    space: &str,
    topic_id: i64,
    post_id: i64,
    subjects: &[String],
) -> anyhow::Result<Vec<String>> {
    let now = crate::db::topics::now();
    let mut told = Vec::new();
    for subject in subjects {
        let inserted = sqlx::query(
            "INSERT INTO inbox (subject, space, topic_id, post_id, reason, created_at)
             VALUES (?, ?, ?, ?, 'mention', ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(subject)
        .bind(space)
        .bind(topic_id)
        .bind(post_id)
        .bind(now)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if inserted == 1 {
            told.push(subject.clone());
        }
    }
    Ok(told)
}

/// What the bell shows: unread mentions, posts with unread likes, and
/// topics with unread replies — the same units the page lists, so the
/// number and the list agree.
pub async fn unread_count(db: &Db, subject: &str, space: &str) -> anyhow::Result<i64> {
    // `hidden = 0` like every list: a withdrawn article is not something to
    // be called back to.
    //
    // Events are counted by the HANDLE of this account: they were stored for
    // a handle, possibly before the account existed (`db::events`).
    Ok(sqlx::query_scalar(
        "SELECT (SELECT count(DISTINCT CASE WHEN i.reason = 'reply' THEN i.topic_id END)
                      + count(CASE WHEN i.reason = 'mention' THEN 1 END)
                      + count(CASE WHEN i.reason = 'like' THEN 1 END)
                   FROM inbox i JOIN topics t ON t.id = i.topic_id
                  WHERE i.subject = ?1 AND i.space = ?2 AND i.read_at IS NULL AND t.hidden = 0)
              + (SELECT count(*) FROM events e
                  WHERE e.handle = (SELECT handle FROM accounts WHERE subject = ?1)
                    AND e.space = ?2 AND e.read_at IS NULL)",
    )
    .bind(subject)
    .bind(space)
    .fetch_one(db.pool())
    .await?)
}

/// The page: unread first, then newest first, at most `limit` lines.
pub async fn entries(
    db: &Db,
    subject: &str,
    space: &str,
    limit: i64,
) -> anyhow::Result<Vec<Entry>> {
    let limit = limit.clamp(1, 200);

    // A bundle is a topic AND a read state. Read and unread replies of the
    // same topic are two lines: after reading, "1 new reply" is the truth,
    // and "2 replies, one of which you saw" is not a notification.
    let bundles = sqlx::query(
        "WITH b AS (
             SELECT i.topic_id, (i.read_at IS NULL) AS unread, count(*) AS n,
                    min(i.post_id) AS first_post, max(i.post_id) AS last_post,
                    max(i.created_at) AS at
               FROM inbox i JOIN topics t ON t.id = i.topic_id
              WHERE i.subject = ? AND i.space = ? AND i.reason = 'reply' AND t.hidden = 0
              GROUP BY i.topic_id, (i.read_at IS NULL)
         )
         SELECT b.*, t.title AS title, p.author_name AS author
           FROM b JOIN topics t ON t.id = b.topic_id
                  JOIN posts  p ON p.id = b.last_post
          ORDER BY b.unread DESC, b.at DESC
          LIMIT ?",
    )
    .bind(subject)
    .bind(space)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;

    let mentions = sqlx::query(
        "SELECT i.topic_id, i.post_id, i.created_at AS at, (i.read_at IS NULL) AS unread,
                t.title AS title, p.author_name AS author
           FROM inbox i JOIN topics t ON t.id = i.topic_id
                        JOIN posts  p ON p.id = i.post_id
          WHERE i.subject = ? AND i.space = ? AND i.reason = 'mention' AND t.hidden = 0
          ORDER BY unread DESC, i.created_at DESC
          LIMIT ?",
    )
    .bind(subject)
    .bind(space)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;

    // The count and the latest name come from `likes` NOW, not from the
    // entry: the line is then always as true as the number under the post.
    // `n = 0` cannot follow a `toggle` (the entry goes with the last like)
    // but can follow a restore; such a line is left out, not shown as
    // "nobody likes your post".
    let likes = sqlx::query(
        "SELECT i.topic_id, i.post_id, i.created_at AS at, (i.read_at IS NULL) AS unread,
                t.title AS title,
                (SELECT count(*) FROM likes l WHERE l.post_id = i.post_id) AS n,
                (SELECT name FROM likes l WHERE l.post_id = i.post_id
                  ORDER BY l.created_at DESC, l.rowid DESC LIMIT 1) AS latest
           FROM inbox i JOIN topics t ON t.id = i.topic_id
          WHERE i.subject = ? AND i.space = ? AND i.reason = 'like' AND t.hidden = 0
          ORDER BY unread DESC, i.created_at DESC
          LIMIT ?",
    )
    .bind(subject)
    .bind(space)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;

    let events = sqlx::query(
        "SELECT id, kind, title, reason, created_at, (read_at IS NULL) AS unread
           FROM events
          WHERE handle = (SELECT handle FROM accounts WHERE subject = ?)
            AND space = ?
          ORDER BY unread DESC, created_at DESC
          LIMIT ?",
    )
    .bind(subject)
    .bind(space)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;

    let mut out: Vec<Entry> = bundles
        .iter()
        .map(|r| Entry::Replies {
            topic_id: r.get("topic_id"),
            topic_title: r.get("title"),
            count: r.get("n"),
            latest_author: r.get("author"),
            latest_at: r.get("at"),
            first_post_id: r.get("first_post"),
            unread: r.get::<i64, _>("unread") != 0,
        })
        .chain(mentions.iter().map(|r| Entry::Mention {
            topic_id: r.get("topic_id"),
            topic_title: r.get("title"),
            post_id: r.get("post_id"),
            author: r.get("author"),
            at: r.get("at"),
            unread: r.get::<i64, _>("unread") != 0,
        }))
        .chain(likes.iter().filter_map(|r| {
            let count: i64 = r.get("n");
            let latest: Option<String> = r.get("latest");
            Some(Entry::Likes {
                topic_id: r.get("topic_id"),
                topic_title: r.get("title"),
                post_id: r.get("post_id"),
                count: (count > 0).then_some(count)?,
                latest_name: latest?,
                at: r.get("at"),
                unread: r.get::<i64, _>("unread") != 0,
            })
        }))
        .chain(events.iter().filter_map(|r| {
            // A kind this version does not know is left out rather than
            // shown as something it is not (the CHECK in the table makes it
            // impossible today; a downgrade would not).
            let kind = crate::db::events::Kind::parse(r.get("kind"))?;
            Some(Entry::Event {
                id: r.get("id"),
                kind,
                title: r.get("title"),
                reason: r.get("reason"),
                at: r.get("created_at"),
                unread: r.get::<i64, _>("unread") != 0,
            })
        }))
        .collect();
    out.sort_by(|a, b| b.unread().cmp(&a.unread()).then(b.at().cmp(&a.at())));
    out.truncate(limit as usize);
    Ok(out)
}

/// The bell for a HANDLE, as the start page asks for it: number and entries.
///
/// With an account behind the handle it is exactly what that account's bell
/// in treff shows. Without one — somebody who only asks for films and never
/// came to the forum — it is their events alone, which is also all there is.
pub async fn for_handle(
    db: &Db,
    handle: &str,
    space: &str,
    limit: i64,
) -> anyhow::Result<(i64, Vec<Entry>)> {
    if let Some(subject) = crate::db::accounts::subject_of_handle(db, handle).await? {
        let unread = unread_count(db, &subject, space).await?;
        return Ok((unread, entries(db, &subject, space, limit).await?));
    }
    let unread: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE handle = ? AND space = ? AND read_at IS NULL",
    )
    .bind(handle)
    .bind(space)
    .fetch_one(db.pool())
    .await?;
    let rows = sqlx::query(
        "SELECT id, kind, title, reason, created_at, (read_at IS NULL) AS unread
           FROM events WHERE handle = ? AND space = ?
          ORDER BY unread DESC, created_at DESC LIMIT ?",
    )
    .bind(handle)
    .bind(space)
    .bind(limit.clamp(1, 200))
    .fetch_all(db.pool())
    .await?;
    let list = rows
        .iter()
        .filter_map(|r| {
            Some(Entry::Event {
                id: r.get("id"),
                kind: crate::db::events::Kind::parse(r.get("kind"))?,
                title: r.get("title"),
                reason: r.get("reason"),
                at: r.get("created_at"),
                unread: r.get::<i64, _>("unread") != 0,
            })
        })
        .collect();
    Ok((unread, list))
}

/// Opening a topic reads everything in it — replies and mentions alike.
pub async fn mark_topic_read(db: &Db, subject: &str, topic_id: i64) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE inbox SET read_at = ? WHERE subject = ? AND topic_id = ? AND read_at IS NULL",
    )
    .bind(crate::db::topics::now())
    .bind(subject)
    .bind(topic_id)
    .execute(db.pool())
    .await?;
    // The topic's space is not at hand here; every stream asks again.
    db.changed(crate::db::EVERY_SPACE);
    Ok(())
}

pub async fn mark_all_read(db: &Db, subject: &str, space: &str) -> anyhow::Result<()> {
    let now = crate::db::topics::now();
    sqlx::query("UPDATE inbox SET read_at = ? WHERE subject = ? AND space = ? AND read_at IS NULL")
        .bind(now)
        .bind(subject)
        .bind(space)
        .execute(db.pool())
        .await?;
    sqlx::query(
        "UPDATE events SET read_at = ?
          WHERE handle = (SELECT handle FROM accounts WHERE subject = ?)
            AND space = ? AND read_at IS NULL",
    )
    .bind(now)
    .bind(subject)
    .bind(space)
    .execute(db.pool())
    .await?;
    db.changed(space);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::Identity;

    const FORUM: &str = "forum.example.org";

    async fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&dir.path().join("t.db")).await.expect("open");
        (dir, db)
    }

    fn who(subject: &str) -> Identity {
        Identity {
            subject: subject.into(),
            name: format!("{subject} N."),
            groups: vec!["Household".into()],
            email: None,
            handle: None,
        }
    }

    async fn topic(db: &Db, space: &str, title: &str, by: &str) -> i64 {
        crate::db::topics::create_topic(db, space, "general", title, "opening", &who(by))
            .await
            .expect("topic")
    }

    async fn reply(db: &Db, topic: i64, by: &str) -> i64 {
        crate::db::topics::add_reply(db, topic, "an answer", &who(by))
            .await
            .expect("reply")
    }

    /// Nobody is shown their own words — the same rule as the mail.
    #[tokio::test]
    async fn the_writer_gets_no_entry() {
        let (_d, db) = db().await;
        let t = topic(&db, FORUM, "T", "ada").await;
        reply(&db, t, "ada").await;
        assert_eq!(unread_count(&db, "ada", FORUM).await.expect("count"), 0);
        assert!(
            entries(&db, "ada", FORUM, 50)
                .await
                .expect("list")
                .is_empty()
        );
    }

    /// THREE ANSWERS IN ONE TOPIC ARE ONE LINE, counted as one on the bell,
    /// and the link goes to the first of them.
    #[tokio::test]
    async fn replies_in_one_topic_are_one_bundle() {
        let (_d, db) = db().await;
        let t = topic(&db, FORUM, "Holiday", "ada").await;
        let first = reply(&db, t, "ben").await;
        reply(&db, t, "cem").await;
        reply(&db, t, "ben").await;

        assert_eq!(unread_count(&db, "ada", FORUM).await.expect("count"), 1);
        let list = entries(&db, "ada", FORUM, 50).await.expect("list");
        assert_eq!(list.len(), 1, "{list:?}");
        match &list[0] {
            Entry::Replies {
                topic_id,
                topic_title,
                count,
                latest_author,
                first_post_id,
                unread,
                ..
            } => {
                assert_eq!(*topic_id, t);
                assert_eq!(topic_title, "Holiday");
                assert_eq!(*count, 3);
                assert_eq!(latest_author, "ben N.");
                assert_eq!(*first_post_id, first);
                assert!(*unread);
            }
            other => panic!("not a bundle: {other:?}"),
        }
    }

    #[tokio::test]
    async fn two_topics_are_two_bundles_and_reading_one_leaves_the_other() {
        let (_d, db) = db().await;
        let a = topic(&db, FORUM, "A", "ada").await;
        let b = topic(&db, FORUM, "B", "ada").await;
        reply(&db, a, "ben").await;
        reply(&db, b, "ben").await;
        assert_eq!(unread_count(&db, "ada", FORUM).await.expect("count"), 2);

        mark_topic_read(&db, "ada", a).await.expect("read");
        assert_eq!(unread_count(&db, "ada", FORUM).await.expect("count"), 1);
        let list = entries(&db, "ada", FORUM, 50).await.expect("list");
        assert_eq!(list.len(), 2, "the read one stays, below: {list:?}");
        assert!(list[0].unread(), "unread first: {list:?}");
        assert!(!list[1].unread());

        // A reply after reading starts a NEW bundle rather than reviving the
        // read one: "1 new reply", not "2 replies, one of which you saw".
        reply(&db, a, "cem").await;
        let list = entries(&db, "ada", FORUM, 50).await.expect("list");
        let unread_a: Vec<_> = list
            .iter()
            .filter(|e| {
                e.unread() && matches!(e, Entry::Replies { topic_id, .. } if *topic_id == a)
            })
            .collect();
        assert_eq!(unread_a.len(), 1);
        assert!(matches!(unread_a[0], Entry::Replies { count: 1, .. }));
    }

    #[tokio::test]
    async fn mark_all_read_empties_the_bell_of_this_space_only() {
        let (_d, db) = db().await;
        let f = topic(&db, FORUM, "F", "ada").await;
        let b = topic(&db, "blog.example.org", "B", "ada").await;
        reply(&db, f, "ben").await;
        reply(&db, b, "ben").await;

        mark_all_read(&db, "ada", FORUM).await.expect("read");
        assert_eq!(unread_count(&db, "ada", FORUM).await.expect("count"), 0);
        assert_eq!(
            unread_count(&db, "ada", "blog.example.org")
                .await
                .expect("count"),
            1,
            "the other space is not this button's business"
        );
    }

    /// THE SPACE IS PART OF EVERY QUERY. The bell on one host must not count
    /// or list what happened on the other.
    #[tokio::test]
    async fn another_space_is_neither_counted_nor_listed() {
        let (_d, db) = db().await;
        let b = topic(&db, "blog.example.org", "B", "ada").await;
        reply(&db, b, "ben").await;
        assert_eq!(unread_count(&db, "ada", FORUM).await.expect("count"), 0);
        assert!(
            entries(&db, "ada", FORUM, 50)
                .await
                .expect("list")
                .is_empty()
        );
    }

    /// A deleted reply takes its entry with it — the bell must not point at
    /// something that is gone.
    #[tokio::test]
    async fn a_deleted_reply_leaves_no_entry() {
        let (_d, db) = db().await;
        let t = topic(&db, FORUM, "T", "ada").await;
        let p = reply(&db, t, "ben").await;
        crate::db::topics::delete_post(&db, FORUM, p, &who("ben"))
            .await
            .expect("delete");
        assert_eq!(unread_count(&db, "ada", FORUM).await.expect("count"), 0);
    }
}
