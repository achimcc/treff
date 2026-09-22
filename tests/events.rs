//! Events from elsewhere in the bell, asked of the ROUTER.
//!
//! The intake rules are unit-tested in `db::events`; the listener that takes
//! events in is in `tests/internal.rs`. This file is what a person sees.

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

async fn page(app: &axum::Router, uri: &str, cookie: &str) -> String {
    let response = app
        .clone()
        .oneshot(get(uri, cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    body_of(response).await
}

fn badge(html: &str) -> Option<String> {
    let at = html.find("class=\"unread\">")? + "class=\"unread\">".len();
    Some(html[at..].split('<').next()?.to_string())
}

async fn event(db: &treff::db::Db, handle: &str, kind: &str, title: &str, key: &str) {
    let config = treff::config::Config::parse(common::CONFIGURATION).expect("configuration");
    let raw = treff::db::events::Raw {
        handle: handle.into(),
        kind: kind.into(),
        title: title.into(),
        link: Some("https://jellyfin.example.org/web/#/details?id=abc".into()),
        reason: (kind == "film_failed").then(|| "no release found".to_string()),
        source_key: key.into(),
    };
    let checked =
        treff::db::events::checked(raw, config.events.as_ref().expect("events")).expect("fine");
    treff::db::events::take(db, FORUM, &checked)
        .await
        .expect("take");
}

async fn event_id(db: &treff::db::Db) -> i64 {
    sqlx::query_scalar("SELECT max(id) FROM events")
        .fetch_one(db.pool())
        .await
        .expect("an event")
}

/// AN EVENT BEFORE THE FIRST SIGN-IN is there after it: somebody who only
/// asks for films has never been to the forum.
#[tokio::test]
async fn an_event_waits_for_the_first_sign_in() {
    let (dir, db, app) = setup_with_db().await;
    event(&db, "konrad", "film_available", "Dune", "seerr:1").await;
    let konrad = signed_in(&db, dir.path(), "konrad", &["Household"]).await;

    let front = page(&app, "/", &konrad).await;
    assert_eq!(badge(&front).as_deref(), Some("1"), "{front}");
    let list = page(&app, "/notifications", &konrad).await;
    assert!(list.contains("Dune"), "{list}");
    assert!(list.contains("ist da"), "{list}");
}

#[tokio::test]
async fn a_failed_request_says_why() {
    let (dir, db, app) = setup_with_db().await;
    let konrad = signed_in(&db, dir.path(), "konrad", &["Household"]).await;
    event(&db, "konrad", "film_failed", "Dune", "seerr:1").await;
    let list = page(&app, "/notifications", &konrad).await;
    assert!(list.contains("konnte nicht besorgt werden"), "{list}");
    assert!(list.contains("no release found"), "{list}");
}

/// Somebody else's events are nobody else's business.
#[tokio::test]
async fn an_event_is_only_for_its_handle() {
    let (dir, db, app) = setup_with_db().await;
    event(&db, "konrad", "film_available", "Dune", "seerr:1").await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    assert_eq!(badge(&page(&app, "/", &ada).await), None);
    assert!(!page(&app, "/notifications", &ada).await.contains("Dune"));

    // And its id does not open it for the wrong person.
    let id = event_id(&db).await;
    let response = app
        .clone()
        .oneshot(get(&format!("/notifications/e/{id}"), &ada))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// The link goes through treff, which marks the entry read and sends the
/// person on — to the link that was CHECKED when it came in, never to one
/// from the request.
#[tokio::test]
async fn following_an_event_reads_it_and_leads_on() {
    let (dir, db, app) = setup_with_db().await;
    let konrad = signed_in(&db, dir.path(), "konrad", &["Household"]).await;
    event(&db, "konrad", "film_available", "Dune", "seerr:1").await;
    let id = event_id(&db).await;

    let list = page(&app, "/notifications", &konrad).await;
    assert!(
        list.contains(&format!("href=\"/notifications/e/{id}\"")),
        "{list}"
    );

    let response = app
        .clone()
        .oneshot(get(&format!("/notifications/e/{id}"), &konrad))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()["location"],
        "https://jellyfin.example.org/web/#/details?id=abc"
    );
    assert_eq!(badge(&page(&app, "/", &konrad).await), None);
}

#[tokio::test]
async fn mark_all_as_read_includes_events() {
    let (dir, db, app) = setup_with_db().await;
    let konrad = signed_in(&db, dir.path(), "konrad", &["Household"]).await;
    event(&db, "konrad", "film_available", "Dune", "seerr:1").await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/notifications/read")
                .header("host", FORUM)
                .header("cookie", &konrad)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(badge(&page(&app, "/", &konrad).await), None);
}

/// Events belong to the events space. The blog's bell does not ring for them.
#[tokio::test]
async fn the_blog_does_not_ring_for_the_forums_events() {
    let (dir, db, app) = setup_with_db().await;
    let konrad = signed_in(&db, dir.path(), "konrad", &["Household"]).await;
    event(&db, "konrad", "film_available", "Dune", "seerr:1").await;
    let blog = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "blog.example.org")
                .header("cookie", &konrad)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(badge(&body_of(blog).await), None);
}
