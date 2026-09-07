//! Finding something again.
//!
//! **A search is scoped to one space and to the categories that person may
//! read.** That is a security property and not a convenience: a hit list that
//! shows a title from the other audience is a leak, and it is the kind that
//! looks like a feature until somebody notices.

use crate::db::Db;

#[derive(Debug, Clone)]
pub struct Hit {
    pub topic_id: i64,
    pub title: String,
    pub category: String,
    /// The matching text with the match marked, from FTS5's own `snippet`.
    /// Marked with `[` and `]` rather than with markup: what comes back is
    /// somebody else's prose, and it goes through the same escaping as
    /// everything else on its way into a page.
    pub snippet: String,
    pub updated_at: i64,
}

/// FTS5 takes a query language, and a person types words.
///
/// Quoting each word turns `AND OR NEAR("a" "b")` into a search for those
/// words — which is what somebody who typed them meant. Without this a stray
/// quote is a syntax error and a stray `*` is a prefix search nobody asked
/// for; both come back as an error page for a perfectly ordinary question.
fn as_fts_query(typed: &str) -> Option<String> {
    let words: Vec<String> = typed
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-'))
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect();
    if words.is_empty() {
        return None;
    }
    Some(words.join(" AND "))
}

/// Searches bodies and titles, in one space, restricted to `categories`.
///
/// `categories` is what the caller worked out from the person's groups. An
/// empty list finds nothing, which is the same rule as everywhere else here:
/// no permission is not a wildcard.
pub async fn search(
    db: &Db,
    space: &str,
    categories: &[String],
    typed: &str,
    limit: i64,
) -> anyhow::Result<Vec<Hit>> {
    use sqlx::Row;
    let Some(query) = as_fts_query(typed) else {
        return Ok(Vec::new());
    };
    if categories.is_empty() {
        return Ok(Vec::new());
    }

    // TWO QUERIES AND NOT A `UNION`. FTS5's `MATCH` inside a union of joins
    // answers "SQL logic error" — not for a reason worth fighting, and a
    // query that has to be argued with is one nobody will change later.
    // Merging two small result sets in Rust is the same answer, legibly.
    //
    // The category list is bound, not interpolated: it comes from the
    // configuration today, and a query built by formatting is one that
    // becomes an injection the day it is built from something else.
    let placeholders = categories.iter().map(|_| "?").collect::<Vec<_>>().join(",");

    let bodies = format!(
        "SELECT t.id AS topic_id, t.title, t.category, t.updated_at,
                snippet(posts_fts, 0, '[', ']', '…', 12) AS snippet
           FROM posts_fts
           JOIN posts  p ON p.id = posts_fts.rowid
           JOIN topics t ON t.id = p.topic_id
          WHERE posts_fts MATCH ?
            AND t.space = ? AND t.hidden = 0
            AND t.category IN ({placeholders})
          ORDER BY t.updated_at DESC
          LIMIT ?"
    );
    let titles = format!(
        "SELECT t.id AS topic_id, t.title, t.category, t.updated_at, t.title AS snippet
           FROM topics_fts
           JOIN topics t ON t.id = topics_fts.rowid
          WHERE topics_fts MATCH ?
            AND t.space = ? AND t.hidden = 0
            AND t.category IN ({placeholders})
          ORDER BY t.updated_at DESC
          LIMIT ?"
    );

    let mut hits: Vec<Hit> = Vec::new();
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for sql in [bodies, titles] {
        // `AssertSqlSafe`, and here is what is asserted: the only part of this
        // string that varies is a run of `?` produced by COUNTING the
        // categories. Everything a person typed is bound — the query, the
        // space, each category. sqlx makes the assertion loud on purpose, and
        // it is worth re-reading whenever this query changes.
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(&query)
            .bind(space);
        for c in categories {
            q = q.bind(c);
        }
        for row in q.bind(limit).fetch_all(db.pool()).await? {
            let topic_id: i64 = row.get("topic_id");
            // A topic whose title AND body both match is one result, not two.
            // The body's snippet wins because it says more than the title,
            // which is already shown next to it.
            if !seen.insert(topic_id) {
                continue;
            }
            hits.push(Hit {
                topic_id,
                title: row.get("title"),
                category: row.get("category"),
                snippet: row.get("snippet"),
                updated_at: row.get("updated_at"),
            });
        }
    }
    // Newest first, so `sort_by_key` on the negated value rather than a
    // reversed comparator — clippy is right that the intent reads better.
    hits.sort_by_key(|h| std::cmp::Reverse(h.updated_at));
    hits.truncate(limit as usize);
    Ok(hits)
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

    fn who() -> Identity {
        Identity {
            subject: "ada".into(),
            name: "Ada".into(),
            groups: vec!["Household".into()],
            email: None,
        }
    }

    fn all() -> Vec<String> {
        vec!["general".into(), "offtopic".into(), "notes".into()]
    }

    #[tokio::test]
    async fn a_word_is_found_in_a_body_and_in_a_title() {
        let (_d, db) = db().await;
        crate::db::topics::create_topic(
            &db,
            "forum.example.org",
            "general",
            "About the projector",
            "It is in the cellar.",
            &who(),
        )
        .await
        .expect("topic");

        let by_body = search(&db, "forum.example.org", &all(), "cellar", 10)
            .await
            .expect("search");
        assert_eq!(by_body.len(), 1, "{by_body:?}");
        assert!(by_body[0].snippet.contains("[cellar]"), "{:?}", by_body[0]);

        let by_title = search(&db, "forum.example.org", &all(), "projector", 10)
            .await
            .expect("search");
        assert_eq!(by_title.len(), 1, "a title is searched too: {by_title:?}");
    }

    /// THE SECURITY PROPERTY. A hit list that leaks a title from the other
    /// address is a leak, and it looks like a feature until somebody notices.
    #[tokio::test]
    async fn nothing_is_found_across_a_space_or_outside_a_category() {
        let (_d, db) = db().await;
        crate::db::topics::create_topic(
            &db,
            "blog.example.org",
            "notes",
            "Secret plans",
            "The projector is in the cellar.",
            &who(),
        )
        .await
        .expect("topic");

        assert!(
            search(&db, "forum.example.org", &all(), "projector", 10)
                .await
                .expect("search")
                .is_empty(),
            "the other address is not searched"
        );
        assert!(
            search(
                &db,
                "blog.example.org",
                &["general".into()],
                "projector",
                10
            )
            .await
            .expect("search")
            .is_empty(),
            "and neither is a category the person may not read"
        );
        assert!(
            search(&db, "blog.example.org", &[], "projector", 10)
                .await
                .expect("search")
                .is_empty(),
            "an empty list of categories finds nothing, never everything"
        );
    }

    /// Umlauts are why the tokenizer is spelled out in the migration.
    #[tokio::test]
    async fn german_words_are_found_however_they_are_typed() {
        let (_d, db) = db().await;
        crate::db::topics::create_topic(
            &db,
            "forum.example.org",
            "general",
            "Wünsche",
            "Ein Stativ wäre schön.",
            &who(),
        )
        .await
        .expect("topic");

        for typed in ["Wünsche", "wunsche", "WÜNSCHE", "stativ"] {
            assert!(
                !search(&db, "forum.example.org", &all(), typed, 10)
                    .await
                    .expect("search")
                    .is_empty(),
                "{typed:?} found nothing"
            );
        }
    }

    /// An edit changes what is findable, and a deletion removes it. This is
    /// what the triggers are for — the application would forget.
    #[tokio::test]
    async fn the_index_follows_an_edit_and_a_deletion() {
        let (_d, db) = db().await;
        let ada = who();
        let topic = crate::db::topics::create_topic(
            &db,
            "forum.example.org",
            "general",
            "T",
            "aardvark",
            &ada,
        )
        .await
        .expect("topic");
        let post = crate::db::topics::load_topic(&db, "forum.example.org", topic)
            .await
            .expect("load")
            .expect("some")
            .1[0]
            .id;

        crate::db::topics::update_post(&db, "forum.example.org", post, "buffalo", &ada)
            .await
            .expect("edit");
        assert!(
            search(&db, "forum.example.org", &all(), "aardvark", 10)
                .await
                .expect("s")
                .is_empty(),
            "the old text is gone from the index"
        );
        assert_eq!(
            search(&db, "forum.example.org", &all(), "buffalo", 10)
                .await
                .expect("s")
                .len(),
            1
        );

        crate::db::topics::delete_post(&db, "forum.example.org", post, &ada)
            .await
            .expect("delete");
        assert!(
            search(&db, "forum.example.org", &all(), "buffalo", 10)
                .await
                .expect("s")
                .is_empty(),
            "and a deleted post is not findable"
        );
    }

    #[tokio::test]
    async fn a_query_that_is_punctuation_finds_nothing_rather_than_failing() {
        let (_d, db) = db().await;
        for typed in ["", "   ", "\"", "*", "AND", "NEAR(", "^"] {
            let hits = search(&db, "forum.example.org", &all(), typed, 10).await;
            assert!(hits.is_ok(), "{typed:?} produced an error: {hits:?}");
        }
    }
}
