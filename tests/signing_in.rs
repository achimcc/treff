//! The round trip of a sign-in, at the level of the router.
//!
//! WHY THIS FILE EXISTS. Until 2026-09-06 `finish_login` was implemented,
//! covered by unit tests against a mock provider — and never mounted. The
//! router had `/auth/login` and nothing else, so the provider sent people back
//! to `/auth/callback` and treff answered **404**. Nobody could sign in, on
//! the first host to run it, and every test was green: they exercised the
//! functions, not the routes.
//!
//! A function that works and is not reachable is not a feature.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

fn get(host: &str, path: &str, cookie: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri(path).header("Host", host);
    if let Some(c) = cookie {
        b = b.header("Cookie", c);
    }
    b.body(Body::empty()).expect("request")
}

/// The address the provider sends people back to must be a route.
///
/// The path is not written out twice: it comes from the same redirect URI the
/// test setup hands to `AppState`, which is what the provider is configured
/// with. A test that spelled `/auth/callback` itself would keep passing after
/// a rename on either side.
#[tokio::test]
async fn the_redirect_uri_is_a_route_and_not_a_404() {
    let (_dir, app) = common::setup().await;

    let path = common::REDIRECT_URI
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|i| &rest[i..]))
        .expect("the redirect URI has a path");

    let response = app
        .oneshot(get(
            "forum.example.org",
            &format!("{path}?code=irrelevant&state=irrelevant"),
            None,
        ))
        .await
        .expect("response");

    assert_ne!(
        response.status(),
        StatusCode::NOT_FOUND,
        "the provider returns people to {path}; a 404 there means nobody can sign in"
    );
}

/// Without the short-lived cookie there is nothing to compare the state
/// against, and the answer is a refusal — not a 404, and not a session.
#[tokio::test]
async fn a_callback_without_a_pending_cookie_is_refused() {
    let (_dir, app) = common::setup().await;

    let response = app
        .oneshot(get(
            "forum.example.org",
            "/auth/callback?code=abc&state=def",
            None,
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// Signing out ends the session in the database, not only in the browser.
/// A cookie that is merely dropped leaves a usable session behind for anyone
/// who kept a copy of it.
#[tokio::test]
async fn signing_out_destroys_the_session() {
    let (dir, db, app) = common::setup_with_db().await;
    let cookie = common::signed_in(&db, dir.path(), "someone", &["Household"]).await;

    // It works before.
    let before = app
        .clone()
        .oneshot(get("forum.example.org", "/", Some(&cookie)))
        .await
        .expect("response");
    assert_eq!(before.status(), StatusCode::OK);

    let out = app
        .clone()
        .oneshot(get("forum.example.org", "/auth/logout", Some(&cookie)))
        .await
        .expect("response");
    assert_ne!(
        out.status(),
        StatusCode::NOT_FOUND,
        "every page links to /auth/logout"
    );

    // And not afterwards — asserted against the database's answer, by using
    // the very same cookie again.
    let after = app
        .oneshot(get("forum.example.org", "/", Some(&cookie)))
        .await
        .expect("response");
    assert_eq!(
        after.status(),
        StatusCode::SEE_OTHER,
        "the session is gone, so the request is sent to sign in again"
    );
}
