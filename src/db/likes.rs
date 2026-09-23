//! Likes: one heart per person and post (ADR 0008).
//!
//! A like is written in the same transaction as the entry that tells the
//! author, for the reason the reply entries are: a like that exists while
//! its entry does not is a like nobody hears about. Nothing here writes to
//! the outbox — "no mail" is a property of this file, not a switch.

use crate::authz::Identity;
use crate::db::Db;
use sqlx::Row;
use std::collections::HashMap;

/// What a click on the heart did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The like is now on (`liked`) or off, and this many people like the
    /// post now. `topic_id` is where the page goes back to.
    Toggled {
        liked: bool,
        count: i64,
        topic_id: i64,
    },
    /// Your own post: the count is the whole point, and counting yourself
    /// would be no count.
    OwnPost,
    /// No such post in this space — the other address's posts are simply
    /// not here.
    NotFound,
}

/// Turns the viewer's like on a post on or off, and keeps the author's
/// entry in the bell true to the count — in one transaction.
pub async fn toggle(db: &Db, space: &str, post_id: i64, who: &Identity) -> anyhow::Result<Outcome> {
    toggle_telling(db, space, post_id, who, None).await
}

/// `toggle`, with the person behind the space's mirrored articles.
///
/// An article's opening post is stored under a name nobody signs in as; its
/// like tells `article_owner` instead — or nobody, when the configured
/// handle answers to no account: the like counts, and no entry is written
/// for a name that is not a person here. Comments under an article are
/// ordinary posts and tell their own authors. The owner MAY like an article
/// (0.10.1; refused until then): the name on it is not theirs and the count
/// is everybody's — they just get no bell for their own click.
pub async fn toggle_telling(
    db: &Db,
    space: &str,
    post_id: i64,
    who: &Identity,
    article_owner: Option<&str>,
) -> anyhow::Result<Outcome> {
    let mut tx = db.pool().begin().await?;
    let Some(row) = sqlx::query(
        "SELECT p.author_subject, p.topic_id,
                (t.source_key IS NOT NULL) AS mirrored,
                p.id = (SELECT min(id) FROM posts WHERE topic_id = p.topic_id) AS opens
           FROM posts p JOIN topics t ON t.id = p.topic_id
          WHERE p.id = ? AND t.space = ?",
    )
    .bind(post_id)
    .bind(space)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(Outcome::NotFound);
    };
    let author: String = row.get("author_subject");
    let topic_id: i64 = row.get("topic_id");
    let is_article = row.get::<i64, _>("mirrored") != 0 && row.get::<i64, _>("opens") != 0;
    if author == who.subject {
        return Ok(Outcome::OwnPost);
    }
    // Whose bell rings: the author's — or, for an article, the owner's, if
    // there is one to ring and it is not the person clicking.
    let tell: Option<&str> = if is_article {
        article_owner.filter(|owner| *owner != who.subject)
    } else {
        Some(author.as_str())
    };

    let removed = sqlx::query("DELETE FROM likes WHERE post_id = ? AND subject = ?")
        .bind(post_id)
        .bind(&who.subject)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let now = crate::db::topics::now();
    let liked = removed == 0;
    if liked {
        sqlx::query("INSERT INTO likes (post_id, subject, name, created_at) VALUES (?, ?, ?, ?)")
            .bind(post_id)
            .bind(&who.subject)
            .bind(&who.name)
            .bind(now)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(tell) = tell
        && liked
    {
        // One row per post for its author; a like on a post whose entry was
        // read makes it news again. `WHERE reason = 'like'` guards the key:
        // an author never holds a reply or mention entry for their own
        // post, but the guard says so rather than assuming it.
        sqlx::query(
            "INSERT INTO inbox (subject, space, topic_id, post_id, reason, created_at)
             VALUES (?, ?, ?, ?, 'like', ?)
             ON CONFLICT (subject, post_id) DO UPDATE
                SET read_at = NULL, created_at = excluded.created_at
              WHERE reason = 'like'",
        )
        .bind(tell)
        .bind(space)
        .bind(topic_id)
        .bind(post_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM likes WHERE post_id = ?")
        .bind(post_id)
        .fetch_one(&mut *tx)
        .await?;
    if count == 0 {
        // Nobody likes it any more: an entry saying somebody does would be
        // a lie the reader can check under the post. By post, not by
        // subject: whoever was told, the entry goes.
        sqlx::query("DELETE FROM inbox WHERE post_id = ? AND reason = 'like'")
            .bind(post_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    db.changed(space);
    Ok(Outcome::Toggled {
        liked,
        count,
        topic_id,
    })
}

/// What a page shows under one post: how many, whether the viewer is among
/// them, and the latest few names for the heart's `title`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Summary {
    pub count: i64,
    pub liked: bool,
    /// Newest first, at most [`NAMES_SHOWN`].
    pub names: Vec<String>,
}

/// How many names the heart's `title` carries. Beyond that the number
/// says it.
pub const NAMES_SHOWN: usize = 5;

/// The summaries of these posts for this viewer, in one query. A post nobody
/// liked is not in the map.
pub async fn summaries(
    db: &Db,
    post_ids: &[i64],
    viewer: &str,
) -> anyhow::Result<HashMap<i64, Summary>> {
    let mut out: HashMap<i64, Summary> = HashMap::new();
    if post_ids.is_empty() {
        return Ok(out);
    }
    // `json_each` rather than a string of `?` placeholders, as
    // `accounts::handles_of` does: one statement, no dynamic SQL.
    let rows = sqlx::query(
        "SELECT post_id, subject, name FROM likes
          WHERE post_id IN (SELECT value FROM json_each(?))
          ORDER BY created_at DESC, rowid DESC",
    )
    .bind(serde_json::to_string(post_ids)?)
    .fetch_all(db.pool())
    .await?;
    for row in rows {
        let entry = out.entry(row.get("post_id")).or_default();
        entry.count += 1;
        if row.get::<String, _>("subject") == viewer {
            entry.liked = true;
        }
        if entry.names.len() < NAMES_SHOWN {
            entry.names.push(row.get("name"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::Identity;
    use crate::db::Db;

    const FORUM: &str = "forum.example.org";
    const BLOG: &str = "blog.example.org";

    fn only(space: &str) -> Vec<String> {
        vec![space.to_string()]
    }

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

    async fn topic(db: &Db, space: &str, by: &str) -> (i64, i64) {
        let t = crate::db::topics::create_topic(db, space, "general", "T", "opening", &who(by))
            .await
            .expect("topic");
        let (_, posts) = crate::db::topics::load_topic(db, space, t)
            .await
            .expect("load")
            .expect("there");
        (t, posts[0].id)
    }

    /// Opening a topic queues its webhook row, so "no mail" is measured as
    /// "no new row", against what was there before the likes.
    async fn outbox_rows(db: &Db) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM outbox")
            .fetch_one(db.pool())
            .await
            .expect("outbox")
    }

    async fn count(db: &Db, post: i64) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM likes WHERE post_id = ?")
            .bind(post)
            .fetch_one(db.pool())
            .await
            .expect("count")
    }

    #[tokio::test]
    async fn a_like_is_counted_and_a_second_click_takes_it_back() {
        let (_d, db) = db().await;
        let (t, p) = topic(&db, FORUM, "ada").await;
        let on = toggle(&db, FORUM, p, &who("ben")).await.expect("like");
        assert_eq!(
            on,
            Outcome::Toggled {
                liked: true,
                count: 1,
                topic_id: t
            }
        );
        assert_eq!(count(&db, p).await, 1);
        let off = toggle(&db, FORUM, p, &who("ben")).await.expect("unlike");
        assert_eq!(
            off,
            Outcome::Toggled {
                liked: false,
                count: 0,
                topic_id: t
            }
        );
        assert_eq!(count(&db, p).await, 0);
    }

    #[tokio::test]
    async fn your_own_post_is_refused_and_the_other_space_is_not_there() {
        let (_d, db) = db().await;
        let (_, p) = topic(&db, FORUM, "ada").await;
        assert_eq!(
            toggle(&db, FORUM, p, &who("ada")).await.expect("own"),
            Outcome::OwnPost
        );
        assert_eq!(
            toggle(&db, BLOG, p, &who("ben")).await.expect("elsewhere"),
            Outcome::NotFound
        );
        assert_eq!(count(&db, p).await, 0);
    }

    /// The names are for the heart's `title`; the viewer's own state for
    /// the pressed button. Newest first, so the latest liker leads.
    #[tokio::test]
    async fn a_summary_says_how_many_who_and_whether_you() {
        let (_d, db) = db().await;
        let (_, p) = topic(&db, FORUM, "ada").await;
        let (_, q) = topic(&db, FORUM, "ada").await;
        toggle(&db, FORUM, p, &who("ben")).await.expect("like");
        toggle(&db, FORUM, p, &who("cem")).await.expect("like");
        let all = summaries(&db, &[p, q], "ben").await.expect("summaries");
        let s = all.get(&p).expect("p");
        assert_eq!(s.count, 2);
        assert!(s.liked);
        assert_eq!(s.names, vec!["cem N.".to_string(), "ben N.".to_string()]);
        assert!(!all.contains_key(&q), "a post nobody liked has no entry");
        let as_cem = summaries(&db, &[p], "dora").await.expect("summaries");
        assert!(!as_cem[&p].liked);
    }

    /// THE AUTHOR HEARS ABOUT IT, THE LIKER DOES NOT, AND NOBODY IS MAILED.
    #[tokio::test]
    async fn the_author_gets_one_entry_per_post_and_no_mail() {
        let (_d, db) = db().await;
        let (t, p) = topic(&db, FORUM, "ada").await;
        let before = outbox_rows(&db).await;
        toggle(&db, FORUM, p, &who("ben")).await.expect("like");
        toggle(&db, FORUM, p, &who("cem")).await.expect("like");
        assert_eq!(
            crate::db::inbox::unread_count(&db, "ada", &only(FORUM))
                .await
                .expect("count"),
            1,
            "two likes on one post are one line"
        );
        assert_eq!(
            crate::db::inbox::unread_count(&db, "ben", &only(FORUM))
                .await
                .expect("count"),
            0
        );
        let list = crate::db::inbox::entries(&db, "ada", &only(FORUM), 50)
            .await
            .expect("list");
        assert_eq!(list.len(), 1, "{list:?}");
        match &list[0] {
            crate::db::inbox::Entry::Likes {
                topic_id,
                topic_title,
                post_id,
                count,
                latest_name,
                unread,
                ..
            } => {
                assert_eq!(*topic_id, t);
                assert_eq!(topic_title, "T");
                assert_eq!(*post_id, p);
                assert_eq!(*count, 2);
                assert_eq!(latest_name, "cem N.");
                assert!(*unread);
            }
            other => panic!("not a likes entry: {other:?}"),
        }
        assert_eq!(outbox_rows(&db).await, before, "a like never writes a mail");
    }

    #[tokio::test]
    async fn reading_the_topic_reads_it_and_the_next_like_reopens_it() {
        let (_d, db) = db().await;
        let (t, p) = topic(&db, FORUM, "ada").await;
        toggle(&db, FORUM, p, &who("ben")).await.expect("like");
        crate::db::inbox::mark_topic_read(&db, "ada", t)
            .await
            .expect("read");
        assert_eq!(
            crate::db::inbox::unread_count(&db, "ada", &only(FORUM))
                .await
                .expect("count"),
            0
        );
        toggle(&db, FORUM, p, &who("cem")).await.expect("like");
        assert_eq!(
            crate::db::inbox::unread_count(&db, "ada", &only(FORUM))
                .await
                .expect("count"),
            1,
            "a new like is news again"
        );
    }

    #[tokio::test]
    async fn the_last_like_taken_back_removes_the_entry() {
        let (_d, db) = db().await;
        let (_, p) = topic(&db, FORUM, "ada").await;
        toggle(&db, FORUM, p, &who("ben")).await.expect("like");
        toggle(&db, FORUM, p, &who("cem")).await.expect("like");
        toggle(&db, FORUM, p, &who("ben")).await.expect("unlike");
        assert_eq!(
            crate::db::inbox::unread_count(&db, "ada", &only(FORUM))
                .await
                .expect("count"),
            1,
            "cem still likes it"
        );
        toggle(&db, FORUM, p, &who("cem")).await.expect("unlike");
        assert_eq!(
            crate::db::inbox::unread_count(&db, "ada", &only(FORUM))
                .await
                .expect("count"),
            0
        );
        assert!(
            crate::db::inbox::entries(&db, "ada", &only(FORUM), 50)
                .await
                .expect("list")
                .is_empty(),
            "read or not, an entry about nobody is gone"
        );
    }

    #[tokio::test]
    async fn a_deleted_post_takes_its_likes_and_the_entry_along() {
        let (_d, db) = db().await;
        let (t, _) = topic(&db, FORUM, "ada").await;
        let p = crate::db::topics::add_reply(&db, t, "answer", &who("ben"))
            .await
            .expect("reply");
        toggle(&db, FORUM, p, &who("ada")).await.expect("like");
        crate::db::topics::delete_post(&db, FORUM, p, &who("ben"))
            .await
            .expect("delete");
        assert_eq!(count(&db, p).await, 0);
        assert_eq!(
            crate::db::inbox::unread_count(&db, "ben", &only(FORUM))
                .await
                .expect("count"),
            0
        );
    }

    /// A withdrawn article keeps its likes under the post — and rings no
    /// bell: nothing calls a reader to a page that is not listed.
    #[tokio::test]
    async fn a_hidden_topic_rings_no_bell() {
        let (_d, db) = db().await;
        let (t, p) = topic(&db, FORUM, "ada").await;
        toggle(&db, FORUM, p, &who("ben")).await.expect("like");
        sqlx::query("UPDATE topics SET hidden = 1 WHERE id = ?")
            .bind(t)
            .execute(db.pool())
            .await
            .expect("hide");
        assert_eq!(count(&db, p).await, 1);
        assert_eq!(
            crate::db::inbox::unread_count(&db, "ada", &only(FORUM))
                .await
                .expect("count"),
            0
        );
    }
}
