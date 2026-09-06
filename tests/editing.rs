//! Editing and deleting: the author, and nobody else.

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

fn who(subject: &str) -> treff::authz::Identity {
    treff::authz::Identity {
        subject: subject.into(),
        name: format!("{subject} the tester"),
        groups: vec!["Household".into()],
        email: None,
    }
}

/// A topic in the forum with an opening post by `ada` and a reply by `bob`.
async fn conversation(db: &treff::db::Db) -> (i64, i64, i64) {
    let topic = treff::db::topics::create_topic(
        db,
        "forum.example.org",
        "general",
        "A question",
        "the opening post",
        &who("ada"),
    )
    .await
    .expect("topic");
    let reply = treff::db::topics::add_reply(db, topic, "bob's reply", &who("bob"))
        .await
        .expect("reply");
    let (_, posts) = treff::db::topics::load_topic(db, "forum.example.org", topic)
        .await
        .expect("load")
        .expect("present");
    (topic, posts[0].id, reply)
}

#[tokio::test]
async fn the_author_may_edit_and_the_post_says_so_afterwards() {
    let (dir, db, app) = setup_with_db().await;
    let (topic, _opening, reply) = conversation(&db).await;

    let cookie = signed_in(&db, dir.path(), "bob", &["Household"]).await;
    let response = app
        .oneshot(post(
            "forum.example.org",
            &format!("/p/{reply}/edit"),
            &cookie,
            "body=on+second+thought",
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let (_, posts) = treff::db::topics::load_topic(&db, "forum.example.org", topic)
        .await
        .expect("load")
        .expect("present");
    assert_eq!(posts[1].body_markdown, "on second thought");
    assert!(posts[1].edited, "an edited post does not say so");
    assert!(!posts[0].edited, "the wrong post was marked as edited");
}

#[tokio::test]
async fn somebody_else_may_not_and_the_text_is_unchanged() {
    let (dir, db, app) = setup_with_db().await;
    let (topic, _opening, reply) = conversation(&db).await;

    let cookie = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let response = app
        .oneshot(post(
            "forum.example.org",
            &format!("/p/{reply}/edit"),
            &cookie,
            "body=let+me+fix+that+for+you",
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let (_, posts) = treff::db::topics::load_topic(&db, "forum.example.org", topic)
        .await
        .expect("load")
        .expect("present");
    assert_eq!(
        posts[1].body_markdown, "bob's reply",
        "the refusal changed the text anyway"
    );
    assert!(!posts[1].edited);
}

#[tokio::test]
async fn the_permission_is_in_the_query_not_only_in_the_handler() {
    // The storage layer is asked directly here, with no HTTP in the way: even
    // a handler that forgot to check must not be able to write.
    let (_dir, db, _app) = setup_with_db().await;
    let (_topic, _opening, reply) = conversation(&db).await;

    let changed = treff::db::topics::update_post(
        &db,
        "forum.example.org",
        reply,
        "not mine to edit",
        &who("ada"),
    )
    .await
    .expect("query");
    assert!(!changed, "the query let a stranger through");

    let deleted =
        treff::db::topics::delete_post(&db, "forum.example.org", reply, &who("ada")).await;
    assert!(matches!(deleted, Ok(treff::db::topics::Deleted::NotYours)));
}

#[tokio::test]
async fn a_post_in_another_space_is_out_of_reach_even_for_its_author() {
    // The space is part of the condition. Otherwise ada could edit her forum
    // post through the blog's address, and the two audiences would share a
    // back door.
    let (_dir, db, _app) = setup_with_db().await;
    let (_topic, _opening, reply) = conversation(&db).await;

    let changed = treff::db::topics::update_post(
        &db,
        "blog.example.org",
        reply,
        "through the wrong door",
        &who("bob"),
    )
    .await
    .expect("query");
    assert!(!changed, "the space was not part of the condition");
}

#[tokio::test]
async fn deleting_a_reply_removes_it_and_leaves_the_topic() {
    let (dir, db, app) = setup_with_db().await;
    let (topic, _opening, reply) = conversation(&db).await;

    let cookie = signed_in(&db, dir.path(), "bob", &["Household"]).await;
    let response = app
        .oneshot(post(
            "forum.example.org",
            &format!("/p/{reply}/delete"),
            &cookie,
            "",
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let (_, posts) = treff::db::topics::load_topic(&db, "forum.example.org", topic)
        .await
        .expect("load")
        .expect("still there");
    assert_eq!(posts.len(), 1);
}

#[tokio::test]
async fn deleting_the_opening_post_takes_the_topic_only_when_nobody_else_wrote() {
    // The decision, made once: the opening post IS the topic, so removing it
    // removes the topic — but never while someone else's words hang under it.
    // Deleting your own must not delete theirs.
    let (dir, db, app) = setup_with_db().await;
    let (topic, opening, _reply) = conversation(&db).await;

    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let response = app
        .clone()
        .oneshot(post(
            "forum.example.org",
            &format!("/p/{opening}/delete"),
            &ada,
            "",
        ))
        .await
        .expect("response");
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "the topic was taken down over someone else's reply"
    );
    assert!(
        treff::db::topics::load_topic(&db, "forum.example.org", topic)
            .await
            .expect("load")
            .is_some()
    );

    // Alone with her own topic, she may take it back.
    let alone = treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "Never mind",
        "posted by mistake",
        &who("ada"),
    )
    .await
    .expect("topic");
    let (_, posts) = treff::db::topics::load_topic(&db, "forum.example.org", alone)
        .await
        .expect("load")
        .expect("present");
    let response = app
        .oneshot(post(
            "forum.example.org",
            &format!("/p/{}/delete", posts[0].id),
            &ada,
            "",
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(
        treff::db::topics::load_topic(&db, "forum.example.org", alone)
            .await
            .expect("load")
            .is_none(),
        "the topic outlived its only post"
    );
}

#[tokio::test]
async fn nobody_can_edit_a_mirrored_article_through_the_web() {
    // Articles belong to the subject `treff:article`, which is nobody. The
    // file decides what they say — that is the whole point of mirroring.
    let (dir, db, app) = setup_with_db().await;
    let article_author = treff::authz::Identity {
        subject: "treff:article".into(),
        name: "treff".into(),
        groups: vec![],
        email: None,
    };
    let topic = treff::db::topics::create_topic(
        &db,
        "blog.example.org",
        "notes",
        "An article",
        "as written in the file",
        &article_author,
    )
    .await
    .expect("topic");
    let (_, posts) = treff::db::topics::load_topic(&db, "blog.example.org", topic)
        .await
        .expect("load")
        .expect("present");

    let cookie = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let response = app
        .oneshot(post(
            "blog.example.org",
            &format!("/p/{}/edit", posts[0].id),
            &cookie,
            "body=my+version",
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_buttons_appear_only_on_your_own_posts() {
    let (dir, db, app) = setup_with_db().await;
    let (topic, opening, reply) = conversation(&db).await;

    let cookie = signed_in(&db, dir.path(), "bob", &["Household"]).await;
    let html = body_of(
        app.oneshot(get("forum.example.org", &format!("/t/{topic}"), &cookie))
            .await
            .expect("response"),
    )
    .await;

    assert!(
        html.contains(&format!("/p/{reply}/edit")),
        "bob cannot edit his own reply: {html}"
    );
    assert!(
        !html.contains(&format!("/p/{opening}/edit")),
        "bob is offered ada's post: {html}"
    );
}

#[tokio::test]
async fn an_edit_obeys_the_same_limits_as_a_new_post() {
    let (dir, db, app) = setup_with_db().await;
    let (_topic, _opening, reply) = conversation(&db).await;
    let cookie = signed_in(&db, dir.path(), "bob", &["Household"]).await;

    for form in ["body=", "body=%20%20"] {
        let response = app
            .clone()
            .oneshot(post(
                "forum.example.org",
                &format!("/p/{reply}/edit"),
                &cookie,
                form,
            ))
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "accepted {form}"
        );
    }
}
