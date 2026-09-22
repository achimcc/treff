//! `@handle`, asked of the ROUTER: who is told, who is not, and what the page
//! shows.
//!
//! Finding a mention in Markdown is unit-tested in `markup`. This file is for
//! the decisions around it, and the one that matters most is the refusal: a
//! mention of somebody who may not read the space must leave no trace — no
//! entry, no highlight, nothing that says the person exists.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

mod common;
use common::{body_of, setup_with_db, signed_in};

const FORUM: &str = "forum.example.org";

fn get(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("host", FORUM)
        .header("cookie", cookie)
        .header("accept-language", "de")
        .body(Body::empty())
        .expect("request")
}

fn post(uri: &str, cookie: &str, form: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", FORUM)
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(form.to_string()))
        .expect("request")
}

async fn send(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(request).await.expect("response")
}

/// Opens a topic through the route and returns its id.
async fn open(app: &axum::Router, cookie: &str, body: &str) -> i64 {
    let form = format!("title=T&body={}", urlencode(body));
    let response = send(app, post("/c/general/new", cookie, &form)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()["location"].to_str().expect("ascii");
    location
        .trim_start_matches("/t/")
        .parse()
        .expect("a topic id")
}

async fn reply(app: &axum::Router, cookie: &str, topic: i64, body: &str) {
    let form = format!("body={}", urlencode(body));
    let response = send(app, post(&format!("/t/{topic}/reply"), cookie, &form)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn entries(db: &treff::db::Db, subject: &str) -> Vec<treff::db::inbox::Entry> {
    treff::db::inbox::entries(db, subject, FORUM, 50)
        .await
        .expect("entries")
}

fn mentions(list: &[treff::db::inbox::Entry]) -> usize {
    list.iter()
        .filter(|e| matches!(e, treff::db::inbox::Entry::Mention { .. }))
        .count()
}

async fn last_post(db: &treff::db::Db) -> i64 {
    sqlx::query_scalar("SELECT max(id) FROM posts")
        .fetch_one(db.pool())
        .await
        .expect("a post")
}

#[tokio::test]
async fn a_mention_reaches_somebody_who_may_read() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let ben = signed_in(&db, dir.path(), "ben", &["Friends"]).await;
    let cem = signed_in(&db, dir.path(), "cem", &["Household"]).await;

    let t = open(&app, &cem, "Holiday plans").await;
    reply(&app, &ada, t, "@Ben what do you think?").await;

    let list = entries(&db, "ben").await;
    assert_eq!(mentions(&list), 1, "{list:?}");
    let page = body_of(send(&app, get("/notifications", &ben)).await).await;
    assert!(page.contains("hat dich erwaehnt in"), "{page}");
}

#[tokio::test]
async fn opening_a_topic_with_a_mention_tells_them_too() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    signed_in(&db, dir.path(), "ben", &["Household"]).await;
    open(&app, &ada, "@ben, film night?").await;
    assert_eq!(mentions(&entries(&db, "ben").await), 1);
}

/// THE REFUSAL. Eve has an account and a handle, and may not read the forum.
/// Mentioning her must change nothing she could ever see, and the page must
/// render her handle exactly like one that belongs to nobody.
#[tokio::test]
async fn a_mention_of_somebody_who_may_not_read_leaves_no_trace() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    signed_in(&db, dir.path(), "eve", &["Neighbours"]).await;

    let t = open(&app, &ada, "opening").await;
    reply(&app, &ada, t, "hello @eve").await;
    reply(&app, &ada, t, "hello @nobody").await;

    assert!(entries(&db, "eve").await.is_empty());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM inbox WHERE subject = 'eve'")
        .fetch_one(db.pool())
        .await
        .expect("count");
    assert_eq!(rows, 0);

    let page = body_of(send(&app, get(&format!("/t/{t}"), &ada)).await).await;
    assert!(!page.contains("class=\"mention\""), "{page}");
    assert!(page.contains("hello @eve"), "{page}");
    assert!(page.contains("hello @nobody"), "{page}");
}

#[tokio::test]
async fn mentioning_yourself_tells_nobody() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    open(&app, &ada, "note to @ada").await;
    assert!(entries(&db, "ada").await.is_empty());
}

/// A follower who is also mentioned has ONE entry, and it is the mention.
#[tokio::test]
async fn a_follower_who_is_mentioned_has_one_entry_not_two() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let t = open(&app, &ben, "ben's topic, so ben follows").await;
    reply(&app, &ada, t, "@ben here you go").await;

    let list = entries(&db, "ben").await;
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(mentions(&list), 1, "{list:?}");
}

#[tokio::test]
async fn an_edit_that_adds_a_mention_tells_them_once() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let t = open(&app, &ada, "opening").await;
    reply(&app, &ada, t, "a thought").await;
    let p = last_post(&db).await;
    assert!(entries(&db, "ben").await.is_empty());

    for body in ["a thought, @ben", "a thought, @ben!"] {
        let form = format!("body={}", urlencode(body));
        let response = send(&app, post(&format!("/p/{p}/edit"), &ada, &form)).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }
    assert_eq!(mentions(&entries(&db, "ben").await), 1);
}

#[tokio::test]
async fn the_page_marks_the_mention_and_names_the_handles() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let t = open(&app, &ada, "ask @ben").await;

    let page = body_of(send(&app, get(&format!("/t/{t}"), &ada)).await).await;
    assert!(
        page.contains(r#"<span class="mention">@ben</span>"#),
        "{page}"
    );
    // The author's own handle next to the name, so it can be copied.
    assert!(
        page.contains(r#"<span class="handle">@ada</span>"#),
        "{page}"
    );
}
