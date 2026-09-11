//! Reading: the timeline, the topic list, and a single topic.

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

/// Two moments far enough apart to tell one from the other, and fixed rather
/// than "now", so that what a page prints can be asserted rather than guessed.
const OPENED: i64 = 1_700_000_000;
const ANSWERED: i64 = 1_700_086_400;

async fn backdate_topic(db: &treff::db::Db, topic: i64, at: i64) {
    sqlx::query("UPDATE topics SET created_at = ? WHERE id = ?")
        .bind(at)
        .bind(topic)
        .execute(db.pool())
        .await
        .expect("backdate the topic");
}

async fn backdate_post(db: &treff::db::Db, post: i64, at: i64) {
    sqlx::query("UPDATE posts SET created_at = ? WHERE id = ?")
        .bind(at)
        .bind(post)
        .execute(db.pool())
        .await
        .expect("backdate the post");
}

async fn backdate_post_of_topic(db: &treff::db::Db, topic: i64, at: i64) {
    sqlx::query("UPDATE posts SET created_at = ? WHERE topic_id = ?")
        .bind(at)
        .bind(topic)
        .execute(db.pool())
        .await
        .expect("backdate the posts");
}

#[tokio::test]
async fn a_timeline_shows_the_posts_themselves() {
    let (dir, db, app) = setup_with_db().await;
    let author = treff::authz::Identity {
        subject: "s1".into(),
        name: "Ada".into(),
        groups: vec!["Writers".into()],
        email: None,
    };
    treff::db::topics::create_topic(
        &db,
        "blog.example.org",
        "notes",
        "First note",
        "Hello **world**",
        &author,
    )
    .await
    .expect("topic");

    let cookie = signed_in(&db, dir.path(), "reader", &["Household"]).await;
    let response = app
        .oneshot(get("blog.example.org", "/", &cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let html = body_of(response).await;
    assert!(html.contains("First note"), "{html}");
    assert!(
        html.contains("<strong>world</strong>"),
        "a timeline shows the body, rendered: {html}"
    );
}

#[tokio::test]
async fn a_topic_list_shows_titles_and_not_bodies() {
    let (dir, db, app) = setup_with_db().await;
    let author = treff::authz::Identity {
        subject: "s1".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: None,
    };
    treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "A question",
        "the body text nobody should see in a list",
        &author,
    )
    .await
    .expect("topic");

    let cookie = signed_in(&db, dir.path(), "reader", &["Friends"]).await;
    // The topic list lives under its category; the front page of a forum
    // lists the categories (task 9b).
    let response = app
        .oneshot(get("forum.example.org", "/c/general", &cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let html = body_of(response).await;
    assert!(html.contains("A question"), "{html}");
    assert!(
        !html.contains("nobody should see in a list"),
        "the list leaked a body: {html}"
    );
}

#[tokio::test]
async fn without_the_reading_group_the_space_is_forbidden() {
    let (dir, db, app) = setup_with_db().await;
    let cookie = signed_in(&db, dir.path(), "stranger", &["Strangers"]).await;
    let response = app
        .oneshot(get("blog.example.org", "/", &cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_topic_from_another_space_is_not_found() {
    // Not "forbidden": through this address that topic does not exist.
    let (dir, db, app) = setup_with_db().await;
    let author = treff::authz::Identity {
        subject: "s1".into(),
        name: "Ada".into(),
        groups: vec!["Writers".into()],
        email: None,
    };
    let id = treff::db::topics::create_topic(
        &db,
        "blog.example.org",
        "notes",
        "Private note",
        "x",
        &author,
    )
    .await
    .expect("topic");

    let cookie = signed_in(&db, dir.path(), "reader", &["Household", "Friends"]).await;
    let response = app
        .oneshot(get("forum.example.org", &format!("/t/{id}"), &cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let html = body_of(response).await;
    assert!(
        !html.contains("Private note"),
        "the refusal leaked the title: {html}"
    );
}

#[tokio::test]
async fn a_topic_page_shows_its_posts_in_order() {
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
        "first",
        &author,
    )
    .await
    .expect("topic");
    treff::db::topics::add_reply(&db, id, "second", &author)
        .await
        .expect("reply");

    let cookie = signed_in(&db, dir.path(), "reader", &["Household"]).await;
    let response = app
        .oneshot(get("forum.example.org", &format!("/t/{id}"), &cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let html = body_of(response).await;
    let first = html.find("first").expect("first post");
    let second = html.find("second").expect("second post");
    assert!(first < second, "the replies came back out of order");
}

#[tokio::test]
async fn an_unknown_category_is_not_found() {
    let (dir, db, app) = setup_with_db().await;
    let cookie = signed_in(&db, dir.path(), "reader", &["Household"]).await;
    let response = app
        .oneshot(get("blog.example.org", "/c/no-such-category", &cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn no_page_carries_a_script_element() {
    // The CSP forbids scripts anyway. This is the second lock: a page that
    // needs one would be noticed here rather than in a browser console.
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
        "<script>alert(1)</script>",
        &author,
    )
    .await
    .expect("topic");

    let cookie = signed_in(&db, dir.path(), "reader", &["Household"]).await;
    for uri in ["/", &format!("/t/{id}"), "/c/general"] {
        let response = app
            .clone()
            .oneshot(get("forum.example.org", uri, &cookie))
            .await
            .expect("response");
        let html = body_of(response).await;
        assert!(!html.contains("<script"), "{uri} carried a script: {html}");
    }
}

#[tokio::test]
async fn the_stylesheet_is_served_from_this_origin() {
    // `style-src 'self'` only means something if the stylesheet is ours.
    let (dir, db, app) = setup_with_db().await;
    let cookie = signed_in(&db, dir.path(), "reader", &["Household"]).await;
    let response = app
        .oneshot(get("blog.example.org", "/assets/style.css", &cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .expect("content type")
            .to_str()
            .expect("ascii"),
        "text/css; charset=utf-8"
    );
}

/// Search, over the router — including the part that is a security property.
#[tokio::test]
async fn a_search_finds_only_what_this_address_holds() {
    let (dir, db, app) = setup_with_db().await;
    let ada = treff::authz::Identity {
        subject: "ada".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: None,
    };
    treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "The projector",
        "It is in the cellar.",
        &ada,
    )
    .await
    .expect("forum topic");
    treff::db::topics::create_topic(
        &db,
        "blog.example.org",
        "notes",
        "Something else entirely",
        "A word only the blog knows: chiffon.",
        &ada,
    )
    .await
    .expect("blog topic");

    let cookie = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let found = body_of(
        app.clone()
            .oneshot(get("forum.example.org", "/search?q=cellar", &cookie))
            .await
            .expect("response"),
    )
    .await;
    assert!(found.contains("The projector"), "{found}");
    assert!(found.contains("[cellar]"), "the match is marked: {found}");

    let across = body_of(
        app.oneshot(get("forum.example.org", "/search?q=chiffon", &cookie))
            .await
            .expect("response"),
    )
    .await;
    assert!(
        !across.contains("Something else entirely"),
        "a search must not reach across addresses: {across}"
    );
}

#[tokio::test]
async fn a_topic_list_shows_the_opener_and_the_last_reply_side_by_side() {
    // One line used to carry the name of whoever wrote LAST and nothing about
    // who started the thing — the opposite half of the mistake it replaced in
    // September 2026, when it carried the opener's name next to somebody
    // else's date. Both are events, both are asked after, and the answer is
    // two columns rather than a choice between them.
    let (dir, db, app) = setup_with_db().await;
    let opener = treff::authz::Identity {
        subject: "s1".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: None,
    };
    let answerer = treff::authz::Identity {
        subject: "s2".into(),
        name: "Bob".into(),
        groups: vec!["Household".into()],
        email: None,
    };
    let topic = treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "A question",
        "the opening post",
        &opener,
    )
    .await
    .expect("topic");
    let reply = treff::db::topics::add_reply(&db, topic, "an answer", &answerer)
        .await
        .expect("reply");
    backdate_topic(&db, topic, OPENED).await;
    backdate_post(&db, reply, ANSWERED).await;

    let cookie = signed_in(&db, dir.path(), "reader", &["Friends"]).await;
    let html = body_of(
        app.oneshot(get("forum.example.org", "/c/general", &cookie))
            .await
            .expect("response"),
    )
    .await;

    assert!(
        html.contains("Ada"),
        "the list does not say who opened the topic: {html}"
    );
    assert!(
        html.contains("Bob"),
        "the list does not say who answered last: {html}"
    );
    // The two stamps are built with the same function the page uses, on
    // purpose: what is at stake HERE is that the route prints a whole moment
    // for each of the two events. That the conversion itself is right is
    // settled in `clock`, against a named zone rather than against whatever
    // zone this machine believes in.
    let zone = treff::clock::zone();
    assert!(
        html.contains(&treff::clock::stamp(OPENED, zone)),
        "the opening of the topic has no time on it: {html}"
    );
    assert!(
        html.contains(&treff::clock::stamp(ANSWERED, zone)),
        "the last reply has no time on it: {html}"
    );
}

#[tokio::test]
async fn a_topic_nobody_answered_has_nothing_in_the_reply_column() {
    // The opening post is not a reply. Repeating it under "last reply" would
    // credit the opener with an answer they never wrote, and make every silent
    // thread look like a conversation.
    let (dir, db, app) = setup_with_db().await;
    let opener = treff::authz::Identity {
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
        &opener,
    )
    .await
    .expect("topic");
    backdate_topic(&db, topic, OPENED).await;

    let cookie = signed_in(&db, dir.path(), "reader", &["Friends"]).await;
    let html = body_of(
        app.oneshot(get("forum.example.org", "/c/general", &cookie))
            .await
            .expect("response"),
    )
    .await;

    assert!(html.contains("Ada"), "{html}");
    assert!(
        html.contains("no replies yet"),
        "a thread nobody answered should say so: {html}"
    );
    assert_eq!(
        html.matches("Ada").count(),
        1,
        "the opener is repeated as the last answerer: {html}"
    );
}

#[tokio::test]
async fn a_post_carries_the_hour_and_not_only_the_day() {
    // Two posts on the same day used to be indistinguishable in time, which
    // is exactly when the order matters most.
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
    backdate_post_of_topic(&db, topic, OPENED).await;

    let cookie = signed_in(&db, dir.path(), "reader", &["Friends"]).await;
    let html = body_of(
        app.oneshot(get("forum.example.org", &format!("/t/{topic}"), &cookie))
            .await
            .expect("response"),
    )
    .await;

    assert!(
        html.contains(&treff::clock::stamp(OPENED, treff::clock::zone())),
        "the byline of a post still carries a bare date: {html}"
    );
}

#[tokio::test]
async fn a_topic_page_leads_back_to_its_category() {
    // Arriving at a topic from a mail, a search or a bookmark used to be a
    // dead end: the only way onwards was the browser's back button, which is
    // not a design, or the front page, which is a level too far up.
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

    let cookie = signed_in(&db, dir.path(), "reader", &["Friends"]).await;
    let html = body_of(
        app.oneshot(get("forum.example.org", &format!("/t/{topic}"), &cookie))
            .await
            .expect("response"),
    )
    .await;

    assert!(
        html.contains("href=\"/c/general\""),
        "no way back to the category: {html}"
    );
    assert!(
        html.contains("General"),
        "the link does not say where it leads: {html}"
    );
}
