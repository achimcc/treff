//! Writing: opening a topic and replying. The first place where a permission
//! decides what happens to the database, not just what a page says.

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

async fn count_topics(db: &treff::db::Db) -> i64 {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM topics")
        .fetch_one(db.pool())
        .await
        .expect("query");
    n
}

async fn count_posts(db: &treff::db::Db) -> i64 {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM posts")
        .fetch_one(db.pool())
        .await
        .expect("query");
    n
}

#[tokio::test]
async fn a_member_opens_a_topic_and_lands_on_it() {
    let (dir, db, app) = setup_with_db().await;
    let cookie = signed_in(&db, dir.path(), "ada", &["Household"]).await;

    let response = app
        .oneshot(post(
            "forum.example.org",
            "/c/general/new",
            &cookie,
            "title=A+question&body=Does+this+work%3F",
        ))
        .await
        .expect("response");

    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "a POST must answer with a redirect, or a reload posts again"
    );
    let location = response
        .headers()
        .get("location")
        .expect("location")
        .to_str()
        .expect("ascii")
        .to_string();
    assert!(location.starts_with("/t/"), "went to {location}");

    assert_eq!(count_topics(&db).await, 1);
    assert_eq!(count_posts(&db).await, 1);
}

#[tokio::test]
async fn without_the_posting_group_nothing_is_written() {
    // The status code is the smaller half of this test. The database is the
    // other one: a refusal that still writes is not a refusal.
    let (dir, db, app) = setup_with_db().await;
    let cookie = signed_in(&db, dir.path(), "friend", &["Friends"]).await;

    // The blog admits nobody as an author — articles are files.
    let response = app
        .oneshot(post(
            "blog.example.org",
            "/c/notes/new",
            &cookie,
            "title=Sneaking+in&body=hello",
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(count_topics(&db).await, 0, "the refusal wrote a topic");
    assert_eq!(count_posts(&db).await, 0, "the refusal wrote a post");
}

#[tokio::test]
async fn replying_follows_reply_and_not_post() {
    // In the blog nobody may open a topic and everybody may comment. That is
    // the whole point of two separate rights.
    let (dir, db, app) = setup_with_db().await;
    let author = treff::authz::Identity {
        subject: "treff:article".into(),
        name: "treff".into(),
        groups: vec![],
        email: None,
    };
    let id = treff::db::topics::create_topic(
        &db,
        "blog.example.org",
        "notes",
        "An article",
        "x",
        &author,
    )
    .await
    .expect("topic");

    let cookie = signed_in(&db, dir.path(), "friend", &["Friends"]).await;
    let response = app
        .oneshot(post(
            "blog.example.org",
            &format!("/t/{id}/reply"),
            &cookie,
            "body=Nice+one",
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(count_posts(&db).await, 2);
}

#[tokio::test]
async fn a_reply_from_someone_who_may_not_read_is_refused() {
    let (dir, db, app) = setup_with_db().await;
    let author = treff::authz::Identity {
        subject: "s1".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: None,
    };
    let id = treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "A question",
        "x",
        &author,
    )
    .await
    .expect("topic");

    let cookie = signed_in(&db, dir.path(), "stranger", &["Strangers"]).await;
    let response = app
        .oneshot(post(
            "forum.example.org",
            &format!("/t/{id}/reply"),
            &cookie,
            "body=hello",
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(count_posts(&db).await, 1, "the refusal wrote a post");
}

#[tokio::test]
async fn a_reply_to_a_topic_in_another_space_is_not_found() {
    let (dir, db, app) = setup_with_db().await;
    let author = treff::authz::Identity {
        subject: "s1".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: None,
    };
    let id = treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "A question",
        "x",
        &author,
    )
    .await
    .expect("topic");

    let cookie = signed_in(&db, dir.path(), "ada", &["Household", "Friends"]).await;
    let response = app
        .oneshot(post(
            "blog.example.org",
            &format!("/t/{id}/reply"),
            &cookie,
            "body=through+the+wrong+door",
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(count_posts(&db).await, 1);
}

#[tokio::test]
async fn empty_and_oversized_input_is_refused_and_stored_nowhere() {
    let (dir, db, app) = setup_with_db().await;
    let cookie = signed_in(&db, dir.path(), "ada", &["Household"]).await;

    let long_title = "t".repeat(201);
    let long_body = "b".repeat(64 * 1024 + 1);
    for form in [
        "title=&body=something".to_string(),
        "title=Something&body=".to_string(),
        "title=%20%20&body=only+spaces+in+the+title".to_string(),
        format!("title={long_title}&body=fine"),
        format!("title=fine&body={long_body}"),
    ] {
        let response = app
            .clone()
            .oneshot(post("forum.example.org", "/c/general/new", &cookie, &form))
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "accepted: {}",
            &form[..form.len().min(60)]
        );
    }
    assert_eq!(count_topics(&db).await, 0);
    assert_eq!(count_posts(&db).await, 0);
}

#[tokio::test]
async fn the_form_is_only_shown_to_someone_who_may_use_it() {
    // Display follows the right. A button that leads to a 403 is a small lie.
    let (dir, db, app) = setup_with_db().await;

    let member = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let html = body_of(
        app.clone()
            .oneshot(get("forum.example.org", "/c/general", &member))
            .await
            .expect("response"),
    )
    .await;
    assert!(
        html.contains("/c/general/new"),
        "no form for a member: {html}"
    );

    // In the blog nobody may open a topic — not even the household.
    let html = body_of(
        app.oneshot(get("blog.example.org", "/c/notes", &member))
            .await
            .expect("response"),
    )
    .await;
    assert!(
        !html.contains("/c/notes/new"),
        "the blog offers a form for articles: {html}"
    );
}

#[tokio::test]
async fn a_post_to_an_unknown_category_is_not_found() {
    let (dir, db, app) = setup_with_db().await;
    let cookie = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let response = app
        .oneshot(post(
            "forum.example.org",
            "/c/no-such-thing/new",
            &cookie,
            "title=t&body=b",
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_post_without_a_session_writes_nothing() {
    let (_dir, db, app) = setup_with_db().await;
    let response = app
        .oneshot(post(
            "forum.example.org",
            "/c/general/new",
            "",
            "title=t&body=b",
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(count_topics(&db).await, 0);
}

/// The button on the topic page, end to end: it changes the subscription and
/// it is a `POST`.
#[tokio::test]
async fn following_and_unfollowing_go_through_the_page() {
    let (dir, db, app) = setup_with_db().await;
    let ada = treff::authz::Identity {
        subject: "ada".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: None,
    };
    let topic = treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "A topic",
        "Body",
        &ada,
    )
    .await
    .expect("topic");

    // Somebody who did not write in it does not follow it.
    let cookie = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    assert!(
        !treff::db::subscriptions::is_following(&db, "ben", topic)
            .await
            .expect("check")
    );

    let post = |uri: &str| {
        Request::builder()
            .method("POST")
            .uri(uri.to_string())
            .header("host", "forum.example.org")
            .header("cookie", &cookie)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::empty())
            .expect("request")
    };

    let r = app
        .clone()
        .oneshot(post(&format!("/t/{topic}/follow")))
        .await
        .expect("r");
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(
        treff::db::subscriptions::is_following(&db, "ben", topic)
            .await
            .expect("check")
    );

    let r = app
        .clone()
        .oneshot(post(&format!("/t/{topic}/unfollow")))
        .await
        .expect("r");
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert!(
        !treff::db::subscriptions::is_following(&db, "ben", topic)
            .await
            .expect("check")
    );

    // And a topic at the other address is not there — not refused, which would
    // confirm that it exists.
    let elsewhere = Request::builder()
        .method("POST")
        .uri(format!("/t/{topic}/follow"))
        .header("host", "blog.example.org")
        .header("cookie", &cookie)
        .body(Body::empty())
        .expect("request");
    let r = app.oneshot(elsewhere).await.expect("r");
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_new_topic_form_waits_behind_a_button() {
    // A list is read far more often than it is written to, and until this
    // test the writing form stood open under every one of them.
    let (dir, db, app) = setup_with_db().await;
    let member = signed_in(&db, dir.path(), "ada", &["Household"]).await;

    let html = body_of(
        app.oneshot(get("forum.example.org", "/c/general", &member))
            .await
            .expect("response"),
    )
    .await;

    assert!(
        !html.contains("<details class=\"write\" open"),
        "the form stands open from the start: {html}"
    );
    let block = common::details_block(&html, "<details class=\"write\">");
    assert!(block.contains("<summary"), "nothing to click on: {block}");
    assert!(
        block.contains("/c/general/new"),
        "the form is not inside the fold: {html}"
    );
}

#[tokio::test]
async fn replying_and_attaching_a_picture_open_together() {
    // Both are the same intention — answering in this thread — so they are
    // behind the same fold. Two buttons would make two decisions out of one.
    let (dir, db, app) = setup_with_db().await;
    let author = treff::authz::Identity {
        subject: "s1".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: None,
    };
    let topic = treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "A question",
        "the opening post",
        &author,
    )
    .await
    .expect("topic");

    let member = signed_in(&db, dir.path(), "bob", &["Household"]).await;
    let html = body_of(
        app.oneshot(get("forum.example.org", &format!("/t/{topic}"), &member))
            .await
            .expect("response"),
    )
    .await;

    assert!(
        !html.contains("<details class=\"write\" open"),
        "the reply box stands open from the start: {html}"
    );
    let block = common::details_block(&html, "<details class=\"write\">");
    assert!(
        block.contains(&format!("/t/{topic}/reply")),
        "the reply form is not behind the button: {html}"
    );
    assert!(
        block.contains(&format!("/t/{topic}/attach")),
        "the picture went behind a second button: {html}"
    );
}
