//! The face of the forum (stage 7): where something is going on, how long a
//! thread is, and where you are.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

mod common;
use common::{body_of, setup_with_db, signed_in};

const FORUM: &str = "forum.example.org";
const BLOG: &str = "blog.example.org";

fn get(host: &str, uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("host", host)
        .header("cookie", cookie)
        .body(Body::empty())
        .expect("request")
}

fn person(subject: &str) -> treff::authz::Identity {
    treff::authz::Identity {
        subject: subject.into(),
        name: format!("{subject} the tester"),
        groups: vec!["Household".into()],
        email: None,
        handle: None,
    }
}

async fn page(app: &axum::Router, host: &str, uri: &str, cookie: &str) -> String {
    let response = app
        .clone()
        .oneshot(get(host, uri, cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    body_of(response).await
}

async fn topic(db: &treff::db::Db, space: &str, category: &str, title: &str, by: &str) -> i64 {
    treff::db::topics::create_topic(db, space, category, title, "opening", &person(by))
        .await
        .expect("topic")
}

async fn opening_post(db: &treff::db::Db, space: &str, topic: i64) -> i64 {
    let (_, posts) = treff::db::topics::load_topic(db, space, topic)
        .await
        .expect("load")
        .expect("there");
    posts[0].id
}

/// The table row of one topic, as the page carries it.
fn row_of(html: &str, topic: i64) -> Option<&str> {
    let link = format!(r#"href="/t/{topic}""#);
    let at = html.find(&link)?;
    let start = html[..at].rfind("<tr")?;
    let end = html[at..].find("</tr>")? + at;
    Some(&html[start..end])
}

/// THE FRONT PAGE SHOWS WHERE SOMETHING IS GOING ON: the sections, and under
/// them the topics that moved last, across all sections, each with its
/// section's name.
#[tokio::test]
async fn the_front_page_lists_recent_topics_with_their_sections() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let g = topic(&db, FORUM, "general", "Projector", "ada").await;
    let o = topic(&db, FORUM, "offtopic", "Coffee", "ben").await;
    let html = page(&app, FORUM, "/", &ada).await;
    assert!(
        html.contains(r#"class="categories""#),
        "the cards stay: {html}"
    );
    let recent = html
        .find(r#"class="threads recent""#)
        .expect("a recent list");
    let coffee = row_of(&html, o).expect("coffee");
    assert!(coffee.contains("Off topic"), "its section: {coffee}");
    assert!(html.find(r#"href="/t/2""#).expect("coffee") > recent);
    assert!(row_of(&html, g).is_some(), "general too: {html}");
    // The one that moved last comes first. Both were opened in the same
    // second, so they are moved back a day first — else the tie falls to
    // the higher id.
    for id in [g, o] {
        sqlx::query("UPDATE topics SET updated_at = updated_at - 86400 WHERE id = ?")
            .bind(id)
            .execute(db.pool())
            .await
            .expect("backdate");
    }
    treff::db::topics::add_reply(&db, g, "found it", &person("ben"))
        .await
        .expect("reply");
    let html = page(&app, FORUM, "/", &ada).await;
    assert!(
        html.find(&format!(r#"href="/t/{g}""#)) < html.find(&format!(r#"href="/t/{o}""#)),
        "{html}"
    );
}

/// A topic whose section the configuration no longer has is still listed
/// — with its slug as the label, and a link that still works.
#[tokio::test]
async fn a_topic_of_a_forgotten_section_still_lists_with_its_slug() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let t = topic(&db, FORUM, "archive", "Old news", "ada").await;
    let html = page(&app, FORUM, "/", &ada).await;
    let row = row_of(&html, t).expect("listed");
    assert!(row.contains("archive"), "{row}");
    page(&app, FORUM, &format!("/t/{t}"), &ada).await;
}

/// A SECTION LINE ON EVERY SECTION PAGE: the way up, every section, the
/// current one marked.
#[tokio::test]
async fn a_section_page_carries_the_sections_and_marks_its_own() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let html = page(&app, FORUM, "/c/general", &ada).await;
    let nav_at = html
        .find(r#"<nav class="sections""#)
        .expect("a section line");
    let nav = &html[nav_at..html[nav_at..].find("</nav>").expect("end") + nav_at];
    assert!(nav.contains(r#"href="/""#), "the way up: {nav}");
    assert!(
        nav.contains(r#"href="/c/general" aria-current="page""#),
        "{nav}"
    );
    assert!(nav.contains(r#"href="/c/offtopic""#), "{nav}");
    assert!(!nav.contains(r#"href="/c/offtopic" aria-current"#), "{nav}");
    // A timeline has one section and no line.
    let blog = page(&app, BLOG, "/", &ada).await;
    assert!(!blog.contains(r#"class="sections""#), "{blog}");
}

/// THE TOPIC LIST ANSWERS THREE MORE QUESTIONS: how many replies, how many
/// likes, and whether something in there waits for THIS reader.
#[tokio::test]
async fn the_topic_list_counts_replies_and_likes_and_marks_unread() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let t = topic(&db, FORUM, "general", "Projector", "ada").await;
    let quiet = topic(&db, FORUM, "general", "Nothing", "ada").await;
    treff::db::topics::add_reply(&db, t, "one", &person("ben"))
        .await
        .expect("reply");
    treff::db::topics::add_reply(&db, t, "two", &person("ben"))
        .await
        .expect("reply");
    let p = opening_post(&db, FORUM, t).await;
    treff::db::likes::toggle(&db, FORUM, p, &person("ben"))
        .await
        .expect("like");

    let html = page(&app, FORUM, "/c/general", &ada).await;
    let row = row_of(&html, t).expect("row");
    assert!(
        row.contains(r#"class="n replies" data-label="Replies">2<"#),
        "{row}"
    );
    assert!(
        row.contains(r#"class="n likes" data-label="Likes">1<"#),
        "{row}"
    );
    assert!(
        row.starts_with(r#"<tr class="unread""#),
        "ada has news here: {row}"
    );
    let quiet_row = row_of(&html, quiet).expect("row");
    assert!(
        quiet_row.contains(r#"data-label="Replies">–<"#),
        "zero as a dash: {quiet_row}"
    );
    assert!(!quiet_row.contains("unread"), "{quiet_row}");

    let as_ben = page(&app, FORUM, "/c/general", &ben).await;
    let row = row_of(&as_ben, t).expect("row");
    assert!(!row.contains("unread"), "ben wrote it all: {row}");
    assert!(html.contains(r#"class="n">Replies</th>"#), "{html}");
    assert!(html.contains(r#"class="n">Likes</th>"#), "{html}");
}

/// A TOPIC PAGE SAYS HOW LONG IT IS AND WHERE YOU ARE: the replies in the
/// meta line, a number per post that links to it, a way back up.
#[tokio::test]
async fn a_topic_page_numbers_its_posts_and_says_how_many() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let t = topic(&db, FORUM, "general", "Projector", "ada").await;
    let r = treff::db::topics::add_reply(&db, t, "one", &person("ben"))
        .await
        .expect("reply");
    let html = page(&app, FORUM, &format!("/t/{t}"), &ada).await;
    let meta_at = html.find(r#"<p class="meta">"#).expect("a meta line");
    let meta = &html[meta_at..html[meta_at..].find("</p>").expect("end") + meta_at];
    assert!(meta.contains("1 reply"), "{meta}");
    assert!(meta.contains(r#"href="/c/general""#), "the section: {meta}");
    // The opener follows their own topic, so the control offers to stop.
    assert!(
        meta.contains(r#"action="/t/1/unfollow""#),
        "following lives here now: {meta}"
    );
    assert!(
        html.contains(&format!(r##"<a class="num" href="#p{r}">#2</a>"##)),
        "{html}"
    );
    assert!(html.contains(r##"<a class="top" href="#top">"##), "{html}");
    assert!(html.contains(r#"<main id="top">"#), "{html}");
}

/// A timeline entry says how many comments it has, and leads to them.
#[tokio::test]
async fn a_timeline_entry_counts_its_comments() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let t = topic(&db, BLOG, "notes", "News", "ada").await;
    treff::db::topics::add_reply(&db, t, "nice", &person("ben"))
        .await
        .expect("reply");
    treff::db::topics::add_reply(&db, t, "yes", &person("cem"))
        .await
        .expect("reply");
    let html = page(&app, BLOG, "/", &ada).await;
    assert!(
        html.contains(&format!(
            r#"<a class="comments" href="/t/{t}">2 comments</a>"#
        )),
        "{html}"
    );
}
