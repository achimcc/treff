//! The bell and `/notifications`, asked of the ROUTER.
//!
//! The bundling and counting are unit-tested in `db::inbox`. This file is for
//! what a person sees: the number in the header, the page behind it, and the
//! moment the number goes away.

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
        .header("accept-language", "de")
        .body(Body::empty())
        .expect("request")
}

fn post(host: &str, uri: &str, cookie: &str, form: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", host)
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(form.to_string()))
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

/// The number on the bell, or `None` when there is none to show.
fn badge(html: &str) -> Option<String> {
    let at = html.find("class=\"unread\">")? + "class=\"unread\">".len();
    Some(html[at..].split('<').next()?.to_string())
}

#[tokio::test]
async fn somebody_elses_reply_puts_a_number_on_the_bell() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let t =
        treff::db::topics::create_topic(&db, FORUM, "general", "Holiday", "Where?", &person("ada"))
            .await
            .expect("topic");

    let before = page(&app, FORUM, "/", &ada).await;
    assert!(
        before.contains("href=\"/notifications\""),
        "the bell is there even when quiet"
    );
    assert_eq!(badge(&before), None, "and has no number yet");

    treff::db::topics::add_reply(&db, t, "The coast", &person("ben"))
        .await
        .expect("reply");
    let after = page(&app, FORUM, "/", &ada).await;
    assert_eq!(badge(&after).as_deref(), Some("1"));
}

#[tokio::test]
async fn your_own_reply_puts_nothing_on_your_bell() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let t = treff::db::topics::create_topic(&db, FORUM, "general", "T", "B", &person("ben"))
        .await
        .expect("topic");
    let response = app
        .clone()
        .oneshot(post(FORUM, &format!("/t/{t}/reply"), &ada, "body=mine"))
        .await
        .expect("reply");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(badge(&page(&app, FORUM, "/", &ada).await), None);
}

/// OPENING THE TOPIC IS READING IT — and the topic page itself already shows
/// the bell without the bundle it just cleared.
#[tokio::test]
async fn opening_the_topic_clears_its_entries() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let t = treff::db::topics::create_topic(&db, FORUM, "general", "T", "B", &person("ada"))
        .await
        .expect("topic");
    treff::db::topics::add_reply(&db, t, "one", &person("ben"))
        .await
        .expect("reply");

    let topic = page(&app, FORUM, &format!("/t/{t}"), &ada).await;
    assert_eq!(
        badge(&topic),
        None,
        "the page that cleared it does not count it"
    );
    assert_eq!(badge(&page(&app, FORUM, "/", &ada).await), None);
}

#[tokio::test]
async fn the_page_lists_a_bundle_and_links_to_its_first_reply() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let t = treff::db::topics::create_topic(&db, FORUM, "general", "Holiday", "B", &person("ada"))
        .await
        .expect("topic");
    let first = treff::db::topics::add_reply(&db, t, "one", &person("ben"))
        .await
        .expect("reply");
    treff::db::topics::add_reply(&db, t, "two", &person("cem"))
        .await
        .expect("reply");
    treff::db::topics::add_reply(&db, t, "three", &person("ben"))
        .await
        .expect("reply");

    let list = page(&app, FORUM, "/notifications", &ada).await;
    assert!(list.contains("Holiday"), "{list}");
    assert!(list.contains("3 neue Antworten"), "{list}");
    assert!(list.contains("ben the tester"), "the latest writer: {list}");
    assert!(
        list.contains(&format!("href=\"/t/{t}#p{first}\"")),
        "the link starts where reading stopped: {list}"
    );
    // The anchor the link points at has to exist.
    let topic = page(&app, FORUM, &format!("/t/{t}"), &ada).await;
    assert!(topic.contains(&format!("id=\"p{first}\"")), "{topic}");
}

#[tokio::test]
async fn one_reply_is_worded_as_one() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let t = treff::db::topics::create_topic(&db, FORUM, "general", "T", "B", &person("ada"))
        .await
        .expect("topic");
    treff::db::topics::add_reply(&db, t, "one", &person("ben"))
        .await
        .expect("reply");
    let list = page(&app, FORUM, "/notifications", &ada).await;
    assert!(list.contains("1 neue Antwort "), "{list}");
}

#[tokio::test]
async fn mark_all_as_read_empties_the_bell() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let t = treff::db::topics::create_topic(&db, FORUM, "general", "T", "B", &person("ada"))
        .await
        .expect("topic");
    treff::db::topics::add_reply(&db, t, "one", &person("ben"))
        .await
        .expect("reply");

    let response = app
        .clone()
        .oneshot(post(FORUM, "/notifications/read", &ada, ""))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok()),
        Some("/notifications")
    );
    assert_eq!(badge(&page(&app, FORUM, "/", &ada).await), None);
}

/// Each host has its own bell. What happened on the blog is not counted on
/// the forum, and the link it would lead to does not exist here.
#[tokio::test]
async fn the_other_space_rings_its_own_bell() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let t = treff::db::topics::create_topic(&db, BLOG, "notes", "Note", "B", &person("ada"))
        .await
        .expect("topic");
    treff::db::topics::add_reply(&db, t, "a comment", &person("ben"))
        .await
        .expect("reply");

    assert_eq!(badge(&page(&app, FORUM, "/", &ada).await), None);
    assert!(
        !page(&app, FORUM, "/notifications", &ada)
            .await
            .contains("Note")
    );
    assert_eq!(
        badge(&page(&app, BLOG, "/", &ada).await).as_deref(),
        Some("1")
    );
}

/// Somebody who may not read the space has no bell to look at.
#[tokio::test]
async fn the_page_is_behind_the_same_door_as_the_space() {
    let (dir, db, app) = setup_with_db().await;
    let stranger = signed_in(&db, dir.path(), "eve", &["Neighbours"]).await;
    let response = app
        .clone()
        .oneshot(get(FORUM, "/notifications", &stranger))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// A bundle that was read is no longer NEW. Found in the preview on
/// 2026-09-22, where the read line still said "1 new reply" below the
/// unread one.
#[tokio::test]
async fn a_read_bundle_does_not_call_itself_new() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let t = treff::db::topics::create_topic(&db, FORUM, "general", "T", "B", &person("ada"))
        .await
        .expect("topic");
    treff::db::topics::add_reply(&db, t, "one", &person("ben"))
        .await
        .expect("reply");
    page(&app, FORUM, &format!("/t/{t}"), &ada).await;

    let list = page(&app, FORUM, "/notifications", &ada).await;
    assert!(list.contains("1 Antwort in"), "{list}");
    assert!(!list.contains("neue Antwort"), "{list}");
}
