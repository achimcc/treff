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

    sqlx::query(
        "INSERT INTO posts (topic_id, body_markdown, author_subject, author_name, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(body)
    .bind(&author.subject)
    .bind(&author.name)
    .bind(t)
    .bind(t)
    .execute(&mut *tx)
    .await?;

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

    tx.commit().await?;
    Ok(id)
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
) -> anyhow::Result<Vec<Topic>> {
    let (limit, offset) = clamp_page(limit, offset);
    let rows = sqlx::query(
        "SELECT * FROM topics WHERE space = ? AND category = ?
         ORDER BY updated_at DESC, id DESC LIMIT ? OFFSET ?",
    )
    .bind(space)
    .bind(category)
    .bind(limit)
    .bind(offset)
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(topic_from).collect())
}

/// `space` is part of the condition, not just of the display: without it every
/// topic would be reachable through every address as soon as someone guessed
/// the number.
pub async fn load_topic(
    db: &Db,
    space: &str,
    id: i64,
) -> anyhow::Result<Option<(Topic, Vec<Post>)>> {
    let Some(row) = sqlx::query("SELECT * FROM topics WHERE id = ? AND space = ?")
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
        Identity {
            subject: sub.into(),
            name: "N".into(),
            groups: vec![],
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
            list.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
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
        assert_eq!(before[0].title, "new");

        add_reply(&db, old, "up you go", &who("s2"))
            .await
            .expect("reply");

        let after = list_topics(&db, "s", "k", 10, 0).await.expect("list");
        assert_eq!(after[0].title, "old", "the reply did not lift its topic");
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
        assert_eq!(page[0].title, "c");
        let second = list_topics(&db, "s", "k", 2, 2).await.expect("list");
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].title, "a");
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
}
