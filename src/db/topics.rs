//! Topics and posts: the storage layer. No HTTP, no permissions — those are
//! decided in `authz` and applied by the layer above. What this module does
//! guarantee is that a topic can only be found through the space it belongs
//! to.

use crate::authz::Identity;
use crate::db::Db;
use sqlx::Row;

/// Unix seconds. A clock that cannot be read is not a reason to refuse a post,
/// so the failure falls back to the epoch rather than panicking in a request
/// path.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Paging numbers reach this module from a query string. `LIMIT -1` means
/// "everything" in SQLite, so an unchecked negative would hand out the whole
/// table; a huge one would do the same more slowly.
const MAX_PAGE: i64 = 200;

fn clamp_page(limit: i64, offset: i64) -> (i64, i64) {
    (limit.clamp(1, MAX_PAGE), offset.max(0))
}

#[derive(Debug, Clone)]
pub struct Topic {
    pub id: i64,
    pub space: String,
    pub category: String,
    pub title: String,
    pub author_subject: String,
    pub author_name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Who wrote in a topic last, and when — the two things a list shows under a
/// title. Not a whole [`Post`]: reading every body to print two words would be
/// a page of work for a byline.
#[derive(Debug, Clone)]
pub struct LastPost {
    pub author_name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct Post {
    pub id: i64,
    pub topic_id: i64,
    pub body_markdown: String,
    pub author_subject: String,
    pub author_name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub edited: bool,
}

/// The topic and its opening post are written in one transaction: a topic
/// without its first post would render as an empty page nobody can repair.
pub async fn create_topic(
    db: &Db,
    space: &str,
    category: &str,
    title: &str,
    body: &str,
    author: &Identity,
) -> anyhow::Result<i64> {
    let t = now();
    let mut tx = db.pool().begin().await?;
    let id: i64 = sqlx::query(
        "INSERT INTO topics (space, category, title, author_subject, author_name, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(space)
    .bind(category)
    .bind(title)
    .bind(&author.subject)
    .bind(&author.name)
    .bind(t)
    .bind(t)
    .fetch_one(&mut *tx)
    .await?
    .get(0);

    let post_id: i64 = sqlx::query(
        "INSERT INTO posts (topic_id, body_markdown, author_subject, author_name, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(id)
    .bind(body)
    .bind(&author.subject)
    .bind(&author.name)
    .bind(t)
    .bind(t)
    .fetch_one(&mut *tx)
    .await?
    .get(0);

    // WRITING SUBSCRIBES YOU, and it happens in THIS transaction.
    //
    // Opening a topic or replying to one is the clearest statement that you
    // want to know what happens next; asking afterwards is a dialogue nobody
    // wants. In the same transaction because the alternative is a post that
    // exists while its subscription does not — after a crash, forever, and
    // silently.
    crate::db::subscriptions::follow_in(&mut tx, &author.subject, id).await?;

    // EIN NEUES THEMA IST AUCH EIN EREIGNIS. Per Mail geht dabei nichts raus —
    // der einzige Abonnent ist, wer es geschrieben hat —, aber der Betreiber
    // will wissen, dass eines aufgemacht wurde.
    crate::db::outbox::queue_webhook(&mut tx, space, id, post_id, &author.subject).await?;

    tx.commit().await?;
    Ok(id)
}

/// Also in one transaction, for the mirror-image reason: a topic whose
/// `updated_at` moved without a post to show for it would sort to the top of
/// the list and then show nothing new.
pub async fn add_reply(
    db: &Db,
    topic_id: i64,
    body: &str,
    author: &Identity,
) -> anyhow::Result<i64> {
    let t = now();
    let mut tx = db.pool().begin().await?;
    let id: i64 = sqlx::query(
        "INSERT INTO posts (topic_id, body_markdown, author_subject, author_name, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(topic_id)
    .bind(body)
    .bind(&author.subject)
    .bind(&author.name)
    .bind(t)
    .bind(t)
    .fetch_one(&mut *tx)
    .await?
    .get(0);

    sqlx::query("UPDATE topics SET updated_at = ? WHERE id = ?")
        .bind(t)
        .bind(topic_id)
        .execute(&mut *tx)
        .await?;

    // Replying subscribes you too — same transaction, same reason.
    crate::db::subscriptions::follow_in(&mut tx, &author.subject, topic_id).await?;

    // AND WHAT IS OWED IS WRITTEN DOWN HERE, not after the commit. A reply
    // that exists without its notifications is a reply nobody hears about,
    // and nothing later can tell that it happened. `follow_in` runs first, so
    // the writer is already a subscriber and is excluded by name below.
    let space: String = sqlx::query_scalar("SELECT space FROM topics WHERE id = ?")
        .bind(topic_id)
        .fetch_one(&mut *tx)
        .await?;
    crate::db::outbox::queue_for_followers(&mut tx, &space, topic_id, id, &author.subject).await?;

    tx.commit().await?;
    Ok(id)
}

/// What a category overview shows about one category.
#[derive(Debug, Clone, Default)]
pub struct CategoryCount {
    pub topics: i64,
    /// `None` for a category nobody has written in yet — which is listed all
    /// the same, because a hidden category is one nobody ever visits.
    pub last_activity: Option<i64>,
}

/// Counts per category, in one query rather than one per category.
///
/// The space is part of the condition: two spaces may use the same slug, and
/// counting across them would put the blog's activity on the forum's front
/// page.
pub async fn category_counts(
    db: &Db,
    space: &str,
    slugs: &[String],
) -> anyhow::Result<std::collections::HashMap<String, CategoryCount>> {
    // Every configured category appears in the result, including the empty
    // ones; the query only fills in what it finds.
    let mut out: std::collections::HashMap<String, CategoryCount> = slugs
        .iter()
        .map(|s| (s.clone(), CategoryCount::default()))
        .collect();

    let rows = sqlx::query(
        "SELECT category, count(*) AS n, max(updated_at) AS last
         FROM topics WHERE space = ? AND hidden = 0 GROUP BY category",
    )
    .bind(space)
    .fetch_all(db.pool())
    .await?;

    for row in &rows {
        let slug: String = row.get("category");
        if let Some(entry) = out.get_mut(&slug) {
            entry.topics = row.get("n");
            entry.last_activity = row.get("last");
        }
    }
    Ok(out)
}

/// What `delete_post` did, so the layer above can answer accordingly.
#[derive(Debug, PartialEq, Eq)]
pub enum Deleted {
    /// A reply is gone; the topic stands.
    Post,
    /// It was the opening post and nobody else had written: the topic went
    /// with it.
    Topic,
    /// The opening post, but other people have replied under it. Removing it
    /// would remove their words too, and "only your own" means exactly that.
    HasReplies,
    /// Not this person's, not in this space, or not there at all — three
    /// different reasons, one answer, because telling them apart would say
    /// something about posts the asker may not see.
    NotYours,
}

/// Rewrites a post, if it belongs to this person **and** to this space.
///
/// Both conditions live in the WHERE clause, not in the caller. A handler that
/// forgets to check must not be able to write — the query is the place where
/// "only your own" is actually true, and there is a test that asks the
/// storage layer directly with no HTTP in the way.
pub async fn update_post(
    db: &Db,
    space: &str,
    post_id: i64,
    body: &str,
    author: &Identity,
) -> anyhow::Result<bool> {
    let affected = sqlx::query(
        "UPDATE posts SET body_markdown = ?, updated_at = ?, edited = 1
         WHERE id = ? AND author_subject = ?
           AND topic_id IN (SELECT id FROM topics WHERE space = ? AND hidden = 0)",
    )
    .bind(body)
    .bind(now())
    .bind(post_id)
    .bind(&author.subject)
    .bind(space)
    .execute(db.pool())
    .await?
    .rows_affected();
    Ok(affected == 1)
}

pub async fn delete_post(
    db: &Db,
    space: &str,
    post_id: i64,
    author: &Identity,
) -> anyhow::Result<Deleted> {
    let mut tx = db.pool().begin().await?;

    // One query decides ownership, space, and whether this is the opening
    // post — so the checks cannot drift apart.
    let Some(row) = sqlx::query(
        "SELECT p.topic_id AS topic_id,
                (SELECT min(id) FROM posts WHERE topic_id = p.topic_id) AS first_id
         FROM posts p JOIN topics t ON t.id = p.topic_id
         WHERE p.id = ? AND p.author_subject = ? AND t.space = ? AND t.hidden = 0",
    )
    .bind(post_id)
    .bind(&author.subject)
    .bind(space)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(Deleted::NotYours);
    };

    let topic_id: i64 = row.get("topic_id");
    let first_id: i64 = row.get("first_id");

    if first_id == post_id {
        let (others,): (i64,) =
            sqlx::query_as("SELECT count(*) FROM posts WHERE topic_id = ? AND author_subject != ?")
                .bind(topic_id)
                .bind(&author.subject)
                .fetch_one(&mut *tx)
                .await?;
        if others > 0 {
            return Ok(Deleted::HasReplies);
        }
        // The opening post is the topic. With nobody else underneath, taking
        // it back takes the topic — the cascade only reaches this person's own
        // posts.
        sqlx::query("DELETE FROM topics WHERE id = ?")
            .bind(topic_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(Deleted::Topic);
    }

    sqlx::query("DELETE FROM posts WHERE id = ?")
        .bind(post_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE topics SET updated_at = ? WHERE id = ?")
        .bind(now())
        .bind(topic_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Deleted::Post)
}

fn topic_from(row: &sqlx::sqlite::SqliteRow) -> Topic {
    Topic {
        id: row.get("id"),
        space: row.get("space"),
        category: row.get("category"),
        title: row.get("title"),
        author_subject: row.get("author_subject"),
        author_name: row.get("author_name"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

pub async fn list_topics(
    db: &Db,
    space: &str,
    category: &str,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<(Topic, Option<LastPost>)>> {
    let (limit, offset) = clamp_page(limit, offset);
    let rows = sqlx::query(
        // `hidden = 0` keeps withdrawn articles out of the list without
        // deleting them — the comments underneath are not ours to remove.
        //
        // The join answers the question a list is really asked: where is
        // something going on? The highest `id` and not the latest
        // `created_at`, because the order posts were written in is the order
        // they are read in; a backdated article must not jump the queue.
        "SELECT t.*,
                p.author_name AS last_author_name,
                p.created_at  AS last_created_at
           FROM topics t
           LEFT JOIN posts p ON p.id = (SELECT max(id) FROM posts WHERE topic_id = t.id)
          WHERE t.space = ? AND t.category = ? AND t.hidden = 0
          ORDER BY t.updated_at DESC, t.id DESC LIMIT ? OFFSET ?",
    )
    .bind(space)
    .bind(category)
    .bind(limit)
    .bind(offset)
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .iter()
        .map(|row| {
            // `LEFT JOIN` and not `JOIN`: a topic whose posts are all gone
            // would otherwise fall out of the list entirely, and a list that
            // silently drops a row is worse than one with a missing byline.
            let last = row
                .get::<Option<String>, _>("last_author_name")
                .map(|author_name| LastPost {
                    author_name,
                    created_at: row.get("last_created_at"),
                });
            (topic_from(row), last)
        })
        .collect())
}

/// `space` is part of the condition, not just of the display: without it every
/// topic would be reachable through every address as soon as someone guessed
/// the number.
pub async fn load_topic(
    db: &Db,
    space: &str,
    id: i64,
) -> anyhow::Result<Option<(Topic, Vec<Post>)>> {
    let Some(row) = sqlx::query("SELECT * FROM topics WHERE id = ? AND space = ? AND hidden = 0")
        .bind(id)
        .bind(space)
        .fetch_optional(db.pool())
        .await?
    else {
        return Ok(None);
    };
    let topic = topic_from(&row);

    let posts = sqlx::query("SELECT * FROM posts WHERE topic_id = ? ORDER BY id ASC")
        .bind(id)
        .fetch_all(db.pool())
        .await?
        .iter()
        .map(|r| Post {
            id: r.get("id"),
            topic_id: r.get("topic_id"),
            body_markdown: r.get("body_markdown"),
            author_subject: r.get("author_subject"),
            author_name: r.get("author_name"),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
            edited: r.get::<i64, _>("edited") != 0,
        })
        .collect();

    Ok(Some((topic, posts)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::Identity;

    fn who(sub: &str) -> Identity {
        named(sub, "N")
    }

    /// The same, where the test is about the name that ends up on a page.
    fn named(sub: &str, name: &str) -> Identity {
        Identity {
            subject: sub.into(),
            name: name.into(),
            groups: vec![],
            email: None,
        }
    }

    async fn db() -> (tempfile::TempDir, crate::db::Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::Db::open(&dir.path().join("t.db"))
            .await
            .expect("open");
        (dir, db)
    }

    #[tokio::test]
    async fn a_new_topic_carries_its_opening_post() {
        let (_d, db) = db().await;
        let id = create_topic(
            &db,
            "blog.example.org",
            "notes",
            "Hello",
            "**hi**",
            &who("s1"),
        )
        .await
        .expect("create");
        let (topic, posts) = load_topic(&db, "blog.example.org", id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(topic.title, "Hello");
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].body_markdown, "**hi**");
        assert_eq!(posts[0].author_subject, "s1");
        assert!(!posts[0].edited);
    }

    #[tokio::test]
    async fn a_topic_belongs_to_its_space_only() {
        // Two spaces in ONE process: a topic from the blog must not be
        // reachable through the forum's address, not even by guessing its
        // number.
        let (_d, db) = db().await;
        let id = create_topic(&db, "blog.example.org", "notes", "T", "b", &who("s1"))
            .await
            .expect("create");
        assert!(
            load_topic(&db, "forum.example.org", id)
                .await
                .expect("load")
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_list_is_scoped_to_space_and_category() {
        let (_d, db) = db().await;
        create_topic(&db, "a", "k", "here", "x", &who("s1"))
            .await
            .expect("create");
        create_topic(&db, "b", "k", "elsewhere", "x", &who("s1"))
            .await
            .expect("create");
        create_topic(&db, "a", "other", "other category", "x", &who("s1"))
            .await
            .expect("create");

        let list = list_topics(&db, "a", "k", 10, 0).await.expect("list");
        assert_eq!(
            list.iter()
                .map(|(t, _)| t.title.as_str())
                .collect::<Vec<_>>(),
            vec!["here"]
        );
    }

    #[tokio::test]
    async fn replies_come_back_in_order_and_bump_the_topic() {
        let (_d, db) = db().await;
        let id = create_topic(&db, "s", "k", "T", "first", &who("s1"))
            .await
            .expect("create");
        let before = load_topic(&db, "s", id)
            .await
            .expect("load")
            .expect("present")
            .0
            .updated_at;
        add_reply(&db, id, "second", &who("s2"))
            .await
            .expect("reply");
        add_reply(&db, id, "third", &who("s3"))
            .await
            .expect("reply");
        let (topic, posts) = load_topic(&db, "s", id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(
            posts
                .iter()
                .map(|p| p.body_markdown.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );
        assert!(
            topic.updated_at >= before,
            "a reply must lift the topic in the list"
        );
    }

    #[tokio::test]
    async fn a_reply_lifts_an_older_topic_above_a_newer_one() {
        // The assertion above can only ever see `>=`, because both timestamps
        // fall in the same second. This one sets the clock by hand and
        // measures the thing that actually matters: the order of the list.
        let (_d, db) = db().await;
        let old = create_topic(&db, "s", "k", "old", "x", &who("s1"))
            .await
            .expect("create");
        sqlx::query("UPDATE topics SET created_at = 1000, updated_at = 1000 WHERE id = ?")
            .bind(old)
            .execute(db.pool())
            .await
            .expect("age it");
        let new = create_topic(&db, "s", "k", "new", "x", &who("s1"))
            .await
            .expect("create");
        sqlx::query("UPDATE topics SET created_at = 2000, updated_at = 2000 WHERE id = ?")
            .bind(new)
            .execute(db.pool())
            .await
            .expect("age it");

        let before = list_topics(&db, "s", "k", 10, 0).await.expect("list");
        assert_eq!(before[0].0.title, "new");

        add_reply(&db, old, "up you go", &who("s2"))
            .await
            .expect("reply");

        let after = list_topics(&db, "s", "k", 10, 0).await.expect("list");
        assert_eq!(after[0].0.title, "old", "the reply did not lift its topic");
    }

    #[tokio::test]
    async fn a_reply_to_a_missing_topic_fails() {
        let (_d, db) = db().await;
        assert!(add_reply(&db, 4711, "x", &who("s1")).await.is_err());
    }

    #[tokio::test]
    async fn a_refused_reply_stores_no_post() {
        // What this proves is the foreign key, not the transaction: the
        // *first* statement in `add_reply` is the one that fails here, so
        // there is nothing to roll back. The rollback path has no cheap
        // trigger — the second statement is an UPDATE that cannot fail —
        // so it stays untested rather than fake-tested.
        let (_d, db) = db().await;
        let _ = add_reply(&db, 4711, "x", &who("s1")).await;
        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM posts")
            .fetch_one(db.pool())
            .await
            .expect("query");
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn the_list_is_newest_activity_first_and_pages() {
        let (_d, db) = db().await;
        for t in ["a", "b", "c"] {
            create_topic(&db, "s", "k", t, "x", &who("s1"))
                .await
                .expect("create");
        }
        let page = list_topics(&db, "s", "k", 2, 0).await.expect("list");
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].0.title, "c");
        let second = list_topics(&db, "s", "k", 2, 2).await.expect("list");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].0.title, "a");
    }

    #[tokio::test]
    async fn paging_numbers_are_clamped_not_trusted() {
        // These arrive from a query string one day. In SQLite `LIMIT -1` means
        // "no limit", so an unchecked negative would hand out the whole table.
        let (_d, db) = db().await;
        for t in ["a", "b", "c"] {
            create_topic(&db, "s", "k", t, "x", &who("s1"))
                .await
                .expect("create");
        }
        assert_eq!(
            list_topics(&db, "s", "k", -1, 0).await.expect("list").len(),
            1
        );
        assert_eq!(
            list_topics(&db, "s", "k", 10_000, -5)
                .await
                .expect("list")
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn the_list_reports_the_latest_post_of_each_topic() {
        // What a list wants to show is where something is going on: who wrote
        // last, and when. The topic row cannot answer that — its author is
        // whoever opened it, and `updated_at` belongs to no name at all.
        let (_d, db) = db().await;
        let id = create_topic(&db, "a", "k", "here", "x", &named("s1", "Ada"))
            .await
            .expect("create");
        let reply = add_reply(&db, id, "and then", &named("s2", "Bob"))
            .await
            .expect("reply");
        // A date of its own, so that `updated_at` — which is `now()` — cannot
        // pass for the answer by accident.
        sqlx::query("UPDATE posts SET created_at = ? WHERE id = ?")
            .bind(1_700_000_000i64)
            .bind(reply)
            .execute(db.pool())
            .await
            .expect("backdate");

        let list = list_topics(&db, "a", "k", 10, 0).await.expect("list");
        let (_, last) = &list[0];
        let last = last.as_ref().expect("a topic always has a post");
        assert_eq!(last.author_name, "Bob");
        assert_eq!(last.created_at, 1_700_000_000);
    }

    #[tokio::test]
    async fn a_topic_without_replies_reports_its_opening_post() {
        let (_d, db) = db().await;
        create_topic(&db, "a", "k", "here", "x", &named("s1", "Ada"))
            .await
            .expect("create");

        let list = list_topics(&db, "a", "k", 10, 0).await.expect("list");
        let (_, last) = &list[0];
        assert_eq!(last.as_ref().expect("a post").author_name, "Ada");
    }
}
