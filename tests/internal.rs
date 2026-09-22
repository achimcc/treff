//! The internal listener: events in, the bell out (ADR 0006).
//!
//! A second door beside treff's own sign-in, so this file is mostly about
//! what it refuses: a wrong token, the other route's token, a route whose
//! token was never configured, input that does not fit, groups that may not
//! read — and the public router, which must not answer `/internal/*` at all.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

mod common;
use common::{body_of, setup_with_db, signed_in};

const FORUM: &str = "forum.example.org";
const EVENTS: &str = "events-token-0123456789";
const BELL: &str = "bell-token-9876543210";

fn internal(db: &treff::db::Db, events: Option<&str>, bell: Option<&str>) -> axum::Router {
    treff::web::internal::router(treff::web::internal::InternalState::new(
        std::sync::Arc::new(
            treff::config::Config::parse(common::CONFIGURATION).expect("configuration"),
        ),
        db.clone(),
        events.map(|t| t.as_bytes().to_vec()),
        bell.map(|t| t.as_bytes().to_vec()),
    ))
}

fn post_event(token: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/internal/events")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

fn bell(token: &str, user: &str, groups: &str) -> Request<Body> {
    Request::builder()
        .uri("/internal/bell")
        .header("authorization", format!("Bearer {token}"))
        .header("x-treff-user", user)
        .header("x-treff-groups", groups)
        .body(Body::empty())
        .expect("request")
}

const DUNE: &str = r#"{"handle": "konrad", "kind": "film_available", "title": "Dune",
    "link": "https://jellyfin.example.org/web/#/details?id=abc", "source_key": "seerr:1"}"#;

async fn rows(db: &treff::db::Db) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(db.pool())
        .await
        .expect("count")
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    serde_json::from_str(&body_of(response).await).expect("json")
}

#[tokio::test]
async fn an_event_is_taken_once_and_answered_accordingly() {
    let (_dir, db, _app) = setup_with_db().await;
    let app = internal(&db, Some(EVENTS), Some(BELL));
    let first = app
        .clone()
        .oneshot(post_event(EVENTS, DUNE))
        .await
        .expect("r");
    assert_eq!(first.status(), StatusCode::CREATED);
    let again = app
        .clone()
        .oneshot(post_event(EVENTS, DUNE))
        .await
        .expect("r");
    assert_eq!(again.status(), StatusCode::OK, "already there");
    assert_eq!(rows(&db).await, 1);
}

/// EACH ROUTE ITS OWN TOKEN. A wrong one, and the other route's one, are
/// both 401 — a leak of one does not open the other.
#[tokio::test]
async fn a_route_opens_to_its_own_token_only() {
    let (_dir, db, _app) = setup_with_db().await;
    let app = internal(&db, Some(EVENTS), Some(BELL));
    for token in ["wrong", BELL, ""] {
        let r = app
            .clone()
            .oneshot(post_event(token, DUNE))
            .await
            .expect("r");
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{token:?}");
    }
    for token in ["wrong", EVENTS] {
        let r = app
            .clone()
            .oneshot(bell(token, "konrad", "Household"))
            .await
            .expect("r");
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{token:?}");
    }
    let no_header = Request::builder()
        .uri("/internal/bell")
        .body(Body::empty())
        .expect("request");
    assert_eq!(
        app.clone().oneshot(no_header).await.expect("r").status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(rows(&db).await, 0);
}

/// A ROUTE WHOSE TOKEN WAS NEVER SET DOES NOT EXIST — never "open, because
/// nothing was configured".
#[tokio::test]
async fn a_route_without_a_token_does_not_exist() {
    let (_dir, db, _app) = setup_with_db().await;
    let app = internal(&db, None, Some(BELL));
    let r = app.clone().oneshot(post_event("", DUNE)).await.expect("r");
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let app = internal(&db, Some(EVENTS), None);
    let r = app
        .oneshot(bell("", "konrad", "Household"))
        .await
        .expect("r");
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn input_that_does_not_fit_leaves_no_row() {
    let (_dir, db, _app) = setup_with_db().await;
    let app = internal(&db, Some(EVENTS), Some(BELL));
    for body in [
        "not json",
        r#"{"handle": "konrad"}"#,
        r#"{"handle": "Konrad Müller", "kind": "film_available", "title": "Dune", "source_key": "k"}"#,
        r#"{"handle": "konrad", "kind": "film_available", "title": "Dune", "source_key": "k",
            "link": "https://elsewhere.example/"}"#,
        r#"{"handle": "konrad", "kind": "film_available", "title": "Dune", "source_key": "k",
            "extra": 1}"#,
    ] {
        let r = app
            .clone()
            .oneshot(post_event(EVENTS, body))
            .await
            .expect("r");
        assert!(r.status().is_client_error(), "{body}: {}", r.status());
    }
    assert_eq!(rows(&db).await, 0);
}

/// THE NUMBER ON THE START PAGE IS THE NUMBER IN THE FORUM. Replies, a
/// mention and an event, for somebody who has been to the forum.
#[tokio::test]
async fn the_bell_counts_what_the_forum_counts() {
    let (dir, db, public) = setup_with_db().await;
    let app = internal(&db, Some(EVENTS), Some(BELL));
    let konrad = signed_in(&db, dir.path(), "konrad", &["Household"]).await;
    let ada = treff::authz::Identity {
        subject: "ada".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: None,
        handle: Some("ada".into()),
    };
    let topic = treff::db::topics::create_topic(
        &db,
        FORUM,
        "general",
        "Film night",
        "who?",
        &treff::authz::Identity {
            subject: "konrad".into(),
            name: "konrad".into(),
            groups: vec!["Household".into()],
            email: None,
            handle: Some("konrad".into()),
        },
    )
    .await
    .expect("topic");
    treff::db::topics::add_reply(&db, topic, "me", &ada)
        .await
        .expect("reply");
    treff::db::topics::add_reply_mentioning(
        &db,
        topic,
        "@konrad popcorn?",
        &ada,
        &["konrad".into()],
    )
    .await
    .expect("mention");
    app.clone()
        .oneshot(post_event(EVENTS, DUNE))
        .await
        .expect("event");

    let answer = app
        .clone()
        .oneshot(bell(BELL, "konrad", "Household|Film"))
        .await
        .expect("r");
    assert_eq!(answer.status(), StatusCode::OK);
    assert_eq!(answer.headers()["cache-control"], "no-store");
    let body = json(answer).await;
    let forum = treff::db::inbox::unread_count(&db, "konrad", FORUM)
        .await
        .expect("count");
    assert_eq!(forum, 3, "a bundle, a mention, an event");
    assert_eq!(body["unread"], forum, "{body}");

    let entries = body["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 3, "{body}");
    let links: Vec<&str> = entries.iter().filter_map(|e| e["link"].as_str()).collect();
    assert!(
        links
            .iter()
            .all(|l| l.starts_with("https://forum.example.org/")),
        "{links:?}"
    );
    let text = body.to_string();
    assert!(
        text.contains("Film night") && text.contains("Dune"),
        "{text}"
    );
    // Nothing that identifies anybody beyond what the forum shows.
    assert!(!text.contains("\"subject\""), "{text}");

    // And the forum's own bell shows the same number.
    let page = body_of(
        public
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("host", FORUM)
                    .header("cookie", &konrad)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("r"),
    )
    .await;
    assert!(page.contains("class=\"unread\">3<"), "{page}");
}

/// Somebody who only asks for films and never came to the forum still has a
/// bell on the start page.
#[tokio::test]
async fn the_bell_works_before_the_first_sign_in() {
    let (_dir, db, _app) = setup_with_db().await;
    let app = internal(&db, Some(EVENTS), Some(BELL));
    app.clone()
        .oneshot(post_event(EVENTS, DUNE))
        .await
        .expect("event");
    let body = json(
        app.oneshot(bell(BELL, "konrad", "Household"))
            .await
            .expect("r"),
    )
    .await;
    assert_eq!(body["unread"], 1, "{body}");
}

/// Groups that may not read the space see nothing, and a user header that is
/// not a handle is nobody.
#[tokio::test]
async fn foreign_groups_and_odd_users_get_an_empty_bell() {
    let (_dir, db, _app) = setup_with_db().await;
    let app = internal(&db, Some(EVENTS), Some(BELL));
    app.clone()
        .oneshot(post_event(EVENTS, DUNE))
        .await
        .expect("event");
    for (user, groups) in [
        ("konrad", "Neighbours"),
        ("konrad", ""),
        ("Konrad Müller", "Household"),
        ("", "Household"),
    ] {
        let body = json(
            app.clone()
                .oneshot(bell(BELL, user, groups))
                .await
                .expect("r"),
        )
        .await;
        assert_eq!(body["unread"], 0, "{user:?} {groups:?}: {body}");
        assert_eq!(
            body["entries"],
            serde_json::json!([]),
            "{user:?} {groups:?}"
        );
    }
}

/// The door is not on the public side. Whatever the public router answers to
/// `/internal/*`, it is not the bell.
#[tokio::test]
async fn the_public_router_has_no_internal_routes() {
    let (_dir, _db, public) = setup_with_db().await;
    let r = public
        .oneshot(
            Request::builder()
                .uri("/internal/bell")
                .header("host", FORUM)
                .header("authorization", format!("Bearer {BELL}"))
                .header("x-treff-user", "konrad")
                .header("x-treff-groups", "Household")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("r");
    assert_ne!(r.status(), StatusCode::OK);
}
