//! The front page of a forum: which categories there are, and what is going on
//! in them.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

mod common;
use common::{setup_with_db, signed_in};

async fn body_of(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf-8")
}

fn get(host: &str, uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("host", host)
        .header("cookie", cookie)
        .body(Body::empty())
        .expect("request")
}

fn author() -> treff::authz::Identity {
    treff::authz::Identity {
        subject: "s1".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: None,
    }
}

#[tokio::test]
async fn the_front_page_of_a_forum_lists_every_category() {
    // Not the first one it happens to find: with five categories there is no
    // sensible "first", and picking one silently is how a category nobody
    // visits comes about.
    let (dir, db, app) = setup_with_db().await;
    treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "A question",
        "x",
        &author(),
    )
    .await
    .expect("topic");

    let cookie = signed_in(&db, dir.path(), "reader", &["Household"]).await;
    let response = app
        .oneshot(get("forum.example.org", "/", &cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let html = body_of(response).await;
    assert!(html.contains("General"), "{html}");
    assert!(
        html.contains("Off topic"),
        "the empty category is hidden: {html}"
    );
    assert!(
        html.contains("/c/general"),
        "no link into the category: {html}"
    );
    assert!(
        html.contains("/c/offtopic"),
        "no link into the category: {html}"
    );
}

#[tokio::test]
async fn the_overview_counts_topics_and_not_posts() {
    // One topic with three replies is one topic. Counting posts would make a
    // busy thread look like a busy category.
    let (dir, db, app) = setup_with_db().await;
    let id = treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "A question",
        "x",
        &author(),
    )
    .await
    .expect("topic");
    for _ in 0..3 {
        treff::db::topics::add_reply(&db, id, "more", &author())
            .await
            .expect("reply");
    }

    let counts =
        treff::db::topics::category_counts(&db, "forum.example.org", &["general".to_string()])
            .await
            .expect("counts");
    assert_eq!(counts.get("general").map(|c| c.topics), Some(1));

    let cookie = signed_in(&db, dir.path(), "reader", &["Household"]).await;
    let html = body_of(
        app.oneshot(get("forum.example.org", "/", &cookie))
            .await
            .expect("response"),
    )
    .await;
    // Exact, not "contains a 1 somewhere": a page is full of ones.
    assert!(html.contains("1 topic"), "wrong or missing count: {html}");
    assert!(
        !html.contains("1 topics"),
        "one topic is not plural: {html}"
    );
}

#[tokio::test]
async fn an_empty_category_is_listed_with_a_zero() {
    let (dir, db, app) = setup_with_db().await;
    let counts = treff::db::topics::category_counts(
        &db,
        "forum.example.org",
        &["general".to_string(), "offtopic".to_string()],
    )
    .await
    .expect("counts");
    assert_eq!(counts.get("offtopic").map(|c| c.topics), Some(0));
    assert_eq!(counts.get("offtopic").and_then(|c| c.last_activity), None);

    let cookie = signed_in(&db, dir.path(), "reader", &["Household"]).await;
    let html = body_of(
        app.oneshot(get("forum.example.org", "/", &cookie))
            .await
            .expect("response"),
    )
    .await;
    assert!(html.contains("Off topic"), "{html}");
    assert!(
        html.contains("0 topics"),
        "the empty category shows no zero: {html}"
    );
    assert!(
        html.contains("no topics yet"),
        "a category without activity should say so: {html}"
    );
}

#[tokio::test]
async fn the_counts_stop_at_the_space_boundary() {
    // The blog has a category called "notes"; a forum category must never
    // count anything from another address.
    let (_dir, db, _app) = setup_with_db().await;
    treff::db::topics::create_topic(&db, "blog.example.org", "notes", "n", "x", &author())
        .await
        .expect("topic");

    let counts =
        treff::db::topics::category_counts(&db, "forum.example.org", &["notes".to_string()])
            .await
            .expect("counts");
    assert_eq!(counts.get("notes").map(|c| c.topics), Some(0));
}

#[tokio::test]
async fn a_timeline_still_goes_straight_to_its_entries() {
    // A blog with one category has nothing to choose from; an overview there
    // would be a page between the reader and the text.
    let (dir, db, app) = setup_with_db().await;
    treff::db::topics::create_topic(
        &db,
        "blog.example.org",
        "notes",
        "First note",
        "Hello **world**",
        &author(),
    )
    .await
    .expect("topic");

    let cookie = signed_in(&db, dir.path(), "reader", &["Household"]).await;
    let html = body_of(
        app.oneshot(get("blog.example.org", "/", &cookie))
            .await
            .expect("response"),
    )
    .await;
    assert!(
        html.contains("<strong>world</strong>"),
        "the blog front page stopped showing its entries: {html}"
    );
}

#[tokio::test]
async fn the_page_speaks_the_language_the_browser_asked_for() {
    let (dir, db, app) = setup_with_db().await;
    treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "Eine Frage",
        "x",
        &author(),
    )
    .await
    .expect("topic");
    let cookie = signed_in(&db, dir.path(), "reader", &["Household"]).await;

    let german = Request::builder()
        .uri("/")
        .header("host", "forum.example.org")
        .header("cookie", &cookie)
        .header("accept-language", "de-DE,de;q=0.9")
        .body(Body::empty())
        .expect("request");
    let html = body_of(app.clone().oneshot(german).await.expect("response")).await;
    assert!(html.contains("lang=\"de\""), "{html}");
    assert!(html.contains("1 Thema"), "{html}");
    assert!(html.contains("Abmelden"), "{html}");

    let english = Request::builder()
        .uri("/")
        .header("host", "forum.example.org")
        .header("cookie", &cookie)
        .header("accept-language", "en-GB,en")
        .body(Body::empty())
        .expect("request");
    let html = body_of(app.oneshot(english).await.expect("response")).await;
    assert!(html.contains("lang=\"en\""), "{html}");
    assert!(html.contains("1 topic"), "{html}");
    assert!(html.contains("Sign out"), "{html}");
}

#[tokio::test]
async fn the_overview_needs_the_reading_group_like_everything_else() {
    let (dir, db, app) = setup_with_db().await;
    let cookie = signed_in(&db, dir.path(), "stranger", &["Strangers"]).await;
    let response = app
        .oneshot(get("forum.example.org", "/", &cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
