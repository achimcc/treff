//! Articles that are written somewhere else.
//!
//! A directory of `YYYY-MM-DD-<name>.md` files with a `title` in their front
//! matter becomes topics. The direction is one-way and the files win: they
//! decide the title and the body, the database keeps the comments underneath.
//! That asymmetry is the whole design — an article is opened by a commit, a
//! comment by a person, and neither should be able to destroy the other.

use crate::authz::Identity;
use crate::db::Db;
use sqlx::Row;
use std::path::Path;

/// What one run did. Returned rather than logged, so a caller can say
/// something useful and a test can assert on it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Mirrored {
    pub mirrored: usize,
    pub hidden: usize,
    pub skipped: usize,
}

/// The author an article is stored under. It is not a person: nobody signs in
/// as this, and `may_modify` compares subjects, so no account can edit an
/// article through the web — which is exactly right, because the file decides.
fn article_author() -> Identity {
    Identity {
        subject: "treff:article".into(),
        name: "treff".into(),
        groups: Vec::new(),
    }
}

struct Article {
    key: String,
    title: String,
    body: String,
    /// Midnight UTC of the date in the file name.
    published_at: i64,
}

/// `YYYY-MM-DD-<name>.md` → the date part, as a string, and only if it is a
/// date that exists. `2026-13-45` looks like one and is not.
fn date_of(name: &str) -> Option<(String, i64)> {
    let stem = name.strip_suffix(".md")?;
    let (date, rest) = stem.split_at_checked(10)?;
    if !rest.starts_with('-') {
        return None;
    }
    let parsed = time::Date::parse(
        date,
        &time::macros::format_description!("[year]-[month]-[day]"),
    )
    .ok()?;
    let midnight = parsed.midnight().assume_utc().unix_timestamp();
    Some((date.to_string(), midnight))
}

/// Front matter between two `---` lines, then the prose. Only `title` is read
/// here; other keys belong to whatever else consumes these files and are
/// ignored rather than rejected.
fn split_front_matter(text: &str) -> Option<(String, String)> {
    let rest = text.strip_prefix("---")?.trim_start_matches(['\r', '\n']);
    let end = rest.find("\n---")?;
    let (front, body) = rest.split_at(end);

    let title = front
        .lines()
        .find_map(|l| l.strip_prefix("title:"))
        .map(|t| t.trim().trim_matches('"').to_string())
        .filter(|t| !t.is_empty())?;

    let body = body
        .trim_start_matches("\n---")
        .trim_start_matches(['\r', '\n'])
        .trim()
        .to_string();
    Some((title, body))
}

fn read_article(path: &Path, name: &str) -> Option<Article> {
    let (_, published_at) = date_of(name)?;
    let text = std::fs::read_to_string(path).ok()?;
    let (title, body) = split_front_matter(&text)?;
    Some(Article {
        key: name.to_string(),
        title,
        body,
        published_at,
    })
}

/// Mirrors `dir` into `space`/`category`.
///
/// `today` is passed in rather than read from the clock, so the rule "a file
/// dated in the future is a draft" can be tested without waiting for the day.
pub async fn mirror(
    db: &Db,
    space: &str,
    category: &str,
    dir: &Path,
    today: &str,
) -> anyhow::Result<Mirrored> {
    // A missing directory is an error, not an empty blog: a wrong path would
    // otherwise hide in the unit until someone wonders where the articles are.
    let entries = std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("cannot read the article directory {dir:?}: {e}"))?;

    let mut report = Mirrored::default();
    let mut present = Vec::new();

    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".md") {
            continue;
        }
        let Some(article) = read_article(&entry.path(), &name) else {
            eprintln!(
                "treff: skipping {name}: no date in the name, or no title in the front matter"
            );
            report.skipped += 1;
            continue;
        };
        if article.published_at > day_start(today) {
            // Dated in the future: a draft. It is not stored at all, so it
            // cannot be reached by guessing an id either.
            continue;
        }

        present.push(article.key.clone());
        upsert(db, space, category, &article).await?;
        report.mirrored += 1;
    }

    report.hidden = hide_missing(db, space, &present).await?;
    Ok(report)
}

fn day_start(day: &str) -> i64 {
    time::Date::parse(
        day,
        &time::macros::format_description!("[year]-[month]-[day]"),
    )
    .map(|d| d.midnight().assume_utc().unix_timestamp())
    .unwrap_or(i64::MAX)
}

async fn upsert(db: &Db, space: &str, category: &str, article: &Article) -> anyhow::Result<()> {
    let author = article_author();
    let mut tx = db.pool().begin().await?;

    let existing: Option<i64> = sqlx::query("SELECT id FROM topics WHERE source_key = ?")
        .bind(&article.key)
        .fetch_optional(&mut *tx)
        .await?
        .map(|r| r.get("id"));

    let id = match existing {
        Some(id) => {
            // The file wins over the topic — and `hidden` goes back to 0, so a
            // file that returns brings its article back with its comments.
            sqlx::query(
                "UPDATE topics SET title = ?, space = ?, category = ?, created_at = ?,
                 updated_at = max(updated_at, ?), hidden = 0 WHERE id = ?",
            )
            .bind(&article.title)
            .bind(space)
            .bind(category)
            .bind(article.published_at)
            .bind(article.published_at)
            .bind(id)
            .execute(&mut *tx)
            .await?;
            id
        }
        None => sqlx::query(
            "INSERT INTO topics (space, category, title, author_subject, author_name,
             created_at, updated_at, source_key)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(space)
        .bind(category)
        .bind(&article.title)
        .bind(&author.subject)
        .bind(&author.name)
        .bind(article.published_at)
        .bind(article.published_at)
        .bind(&article.key)
        .fetch_one(&mut *tx)
        .await?
        .get(0),
    };

    // The opening post is the article. It is the FIRST post of the topic, and
    // it is rewritten in place — the comments after it keep their order.
    let opening: Option<i64> =
        sqlx::query("SELECT id FROM posts WHERE topic_id = ? ORDER BY id ASC LIMIT 1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .map(|r| r.get("id"));

    match opening {
        Some(post_id) => {
            sqlx::query("UPDATE posts SET body_markdown = ?, updated_at = ? WHERE id = ?")
                .bind(&article.body)
                .bind(article.published_at)
                .bind(post_id)
                .execute(&mut *tx)
                .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO posts (topic_id, body_markdown, author_subject, author_name,
                 created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(&article.body)
            .bind(&author.subject)
            .bind(&author.name)
            .bind(article.published_at)
            .bind(article.published_at)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(())
}

/// Articles whose file is gone are hidden, never deleted: `ON DELETE CASCADE`
/// would take the comments with them, and what people wrote is not ours to
/// remove because a file moved.
async fn hide_missing(db: &Db, space: &str, present: &[String]) -> anyhow::Result<usize> {
    let rows = sqlx::query(
        "SELECT id, source_key FROM topics
         WHERE space = ? AND source_key IS NOT NULL AND hidden = 0",
    )
    .bind(space)
    .fetch_all(db.pool())
    .await?;

    let mut hidden = 0;
    for row in &rows {
        let key: String = row.get("source_key");
        if !present.contains(&key) {
            let id: i64 = row.get("id");
            sqlx::query("UPDATE topics SET hidden = 1 WHERE id = ?")
                .bind(id)
                .execute(db.pool())
                .await?;
            hidden += 1;
        }
    }
    Ok(hidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> (tempfile::TempDir, crate::db::Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::Db::open(&dir.path().join("t.db"))
            .await
            .expect("open");
        (dir, db)
    }

    fn write(dir: &std::path::Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("write");
    }

    fn article(title: &str, prose: &str) -> String {
        format!("---\ntitle: {title}\nkind: note\n---\n\n{prose}\n")
    }

    async fn titles(db: &crate::db::Db) -> Vec<String> {
        crate::db::topics::list_topics(db, "blog.example.org", "notes", 50, 0)
            .await
            .expect("list")
            .into_iter()
            .map(|t| t.title)
            .collect()
    }

    #[tokio::test]
    async fn a_directory_of_files_becomes_topics_newest_first() {
        let (_d, db) = db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "2026-09-01-older.md",
            &article("The older one", "First **thing**."),
        );
        write(
            dir.path(),
            "2026-09-05-newer.md",
            &article("The newer one", "Second thing."),
        );

        let report = mirror(&db, "blog.example.org", "notes", dir.path(), TODAY)
            .await
            .expect("mirror");
        assert_eq!(report.mirrored, 2);
        assert_eq!(report.skipped, 0);

        assert_eq!(
            titles(&db).await,
            vec!["The newer one", "The older one"],
            "articles are ordered by the date in their name"
        );

        // By title, not by id: `read_dir` gives no order, so which file
        // becomes topic 1 is not ours to assume.
        let older = crate::db::topics::list_topics(&db, "blog.example.org", "notes", 50, 0)
            .await
            .expect("list")
            .into_iter()
            .find(|t| t.title == "The older one")
            .expect("the older article");
        let (_, posts) = crate::db::topics::load_topic(&db, "blog.example.org", older.id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(posts.len(), 1, "an article is a topic with one post");
        assert!(posts[0].body_markdown.contains("First **thing**."));
        assert!(
            !posts[0].body_markdown.starts_with("---"),
            "the front matter leaked into the body: {:?}",
            posts[0].body_markdown
        );
    }

    #[tokio::test]
    async fn mirroring_again_changes_nothing() {
        let (_d, db) = db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "2026-09-01-a.md", &article("A", "text"));

        for _ in 0..3 {
            mirror(&db, "blog.example.org", "notes", dir.path(), TODAY)
                .await
                .expect("mirror");
        }
        assert_eq!(titles(&db).await, vec!["A"]);

        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM posts")
            .fetch_one(db.pool())
            .await
            .expect("query");
        assert_eq!(n, 1, "each run added another opening post");
    }

    #[tokio::test]
    async fn an_edited_file_updates_the_article_and_keeps_the_comments() {
        let (_d, db) = db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "2026-09-01-a.md",
            &article("Before", "old text"),
        );
        mirror(&db, "blog.example.org", "notes", dir.path(), TODAY)
            .await
            .expect("mirror");

        let id = crate::db::topics::list_topics(&db, "blog.example.org", "notes", 1, 0)
            .await
            .expect("list")[0]
            .id;
        let reader = crate::authz::Identity {
            subject: "s2".into(),
            name: "Reader".into(),
            groups: vec![],
        };
        crate::db::topics::add_reply(&db, id, "I have a question", &reader)
            .await
            .expect("reply");

        write(dir.path(), "2026-09-01-a.md", &article("After", "new text"));
        mirror(&db, "blog.example.org", "notes", dir.path(), TODAY)
            .await
            .expect("mirror");

        let (topic, posts) = crate::db::topics::load_topic(&db, "blog.example.org", id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(topic.title, "After");
        assert!(posts[0].body_markdown.contains("new text"));
        assert_eq!(
            posts.len(),
            2,
            "the comment did not survive the edit: {posts:?}"
        );
        assert!(posts[1].body_markdown.contains("I have a question"));
    }

    #[tokio::test]
    async fn a_file_that_disappears_takes_the_article_but_not_the_comments() {
        let (_d, db) = db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "2026-09-01-a.md", &article("A", "text"));
        mirror(&db, "blog.example.org", "notes", dir.path(), TODAY)
            .await
            .expect("mirror");

        let id = crate::db::topics::list_topics(&db, "blog.example.org", "notes", 1, 0)
            .await
            .expect("list")[0]
            .id;
        let reader = crate::authz::Identity {
            subject: "s2".into(),
            name: "Reader".into(),
            groups: vec![],
        };
        crate::db::topics::add_reply(&db, id, "still here?", &reader)
            .await
            .expect("reply");

        std::fs::remove_file(dir.path().join("2026-09-01-a.md")).expect("remove");
        let report = mirror(&db, "blog.example.org", "notes", dir.path(), TODAY)
            .await
            .expect("mirror");
        assert_eq!(report.hidden, 1);

        assert!(titles(&db).await.is_empty(), "the article is still listed");
        assert!(
            crate::db::topics::load_topic(&db, "blog.example.org", id)
                .await
                .expect("load")
                .is_none(),
            "a withdrawn article is still reachable by its link"
        );

        let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM posts WHERE topic_id = ?")
            .bind(id)
            .fetch_one(db.pool())
            .await
            .expect("query");
        assert_eq!(n, 2, "deleting a file deleted what people wrote");

        // And when the file comes back, so does everything.
        write(dir.path(), "2026-09-01-a.md", &article("A", "text"));
        mirror(&db, "blog.example.org", "notes", dir.path(), TODAY)
            .await
            .expect("mirror");
        assert_eq!(titles(&db).await, vec!["A"]);
        let (_, posts) = crate::db::topics::load_topic(&db, "blog.example.org", id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(posts.len(), 2, "the comment did not come back with it");
    }

    #[tokio::test]
    async fn a_file_dated_in_the_future_waits_for_its_day() {
        let (_d, db) = db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "2026-09-01-now.md", &article("Now", "text"));
        write(
            dir.path(),
            "2026-09-20-later.md",
            &article("Later", "not yet"),
        );

        mirror(&db, "blog.example.org", "notes", dir.path(), TODAY)
            .await
            .expect("mirror");
        assert_eq!(titles(&db).await, vec!["Now"], "the draft was published");

        // No new deploy, no new file: only the day moved on.
        mirror(&db, "blog.example.org", "notes", dir.path(), "2026-09-20")
            .await
            .expect("mirror");
        assert_eq!(titles(&db).await, vec!["Later", "Now"]);
    }

    #[tokio::test]
    async fn a_broken_file_is_skipped_and_the_others_still_arrive() {
        // One unreadable article must not stop a start.
        let (_d, db) = db().await;
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "2026-09-01-fine.md", &article("Fine", "text"));
        write(dir.path(), "2026-09-02-no-front-matter.md", "just prose\n");
        write(
            dir.path(),
            "2026-09-03-empty-title.md",
            &article("", "text"),
        );
        write(dir.path(), "no-date-at-all.md", &article("Undated", "text"));
        write(dir.path(), "2026-13-45-impossible.md", &article("Bad", "x"));
        write(
            dir.path(),
            "2026-09-04-not-markdown.txt",
            "ignored entirely",
        );

        let report = mirror(&db, "blog.example.org", "notes", dir.path(), TODAY)
            .await
            .expect("mirror");
        assert_eq!(titles(&db).await, vec!["Fine"]);
        assert_eq!(report.mirrored, 1);
        assert_eq!(
            report.skipped, 4,
            "the .txt is not an article and is not a complaint either"
        );
    }

    #[tokio::test]
    async fn a_missing_directory_is_an_error_not_an_empty_blog() {
        // Silently mirroring nothing would hide a wrong path in the unit until
        // someone wonders where the articles went.
        let (_d, db) = db().await;
        assert!(
            mirror(
                &db,
                "blog.example.org",
                "notes",
                std::path::Path::new("/nonexistent/articles"),
                TODAY,
            )
            .await
            .is_err()
        );
    }

    const TODAY: &str = "2026-09-06";
}
