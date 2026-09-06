//! The frame every later page passes through: which host reaches which space,
//! who is let in at all, and what every answer carries.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

mod common;
use common::setup;

fn get(host: &str, uri: &str) -> Request<Body> {
    let mut builder = Request::builder().uri(uri);
    if !host.is_empty() {
        builder = builder.header("host", host);
    }
    builder.body(Body::empty()).expect("request")
}

#[tokio::test]
async fn an_unknown_host_is_refused() {
    let (_d, app) = setup().await;
    let response = app
        .oneshot(get("evil.example.org", "/"))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_request_without_a_host_header_is_refused() {
    let (_d, app) = setup().await;
    let response = app.oneshot(get("", "/")).await.expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_known_host_without_a_session_is_sent_to_the_login() {
    let (_d, app) = setup().await;
    let response = app
        .oneshot(get("blog.example.org", "/"))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get("location").expect("location"),
        "/auth/login"
    );
}

#[tokio::test]
async fn the_host_header_is_matched_without_case_or_port() {
    let (_d, app) = setup().await;
    let response = app
        .oneshot(get("BLOG.example.org:8443", "/"))
        .await
        .expect("response");
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "a known host in different clothes fell into the refusal branch"
    );
}

#[tokio::test]
async fn every_answer_carries_the_security_headers() {
    // Including the refusals: a 403 is a page too.
    let (_d, app) = setup().await;
    for (host, uri) in [
        ("blog.example.org", "/"),
        ("evil.example.org", "/"),
        ("blog.example.org", "/auth/login"),
    ] {
        let response = app.clone().oneshot(get(host, uri)).await.expect("response");
        let headers = response.headers();
        let csp = headers
            .get("content-security-policy")
            .unwrap_or_else(|| panic!("no CSP on {host}{uri}"))
            .to_str()
            .expect("ascii");
        assert!(csp.contains("default-src 'self'"), "csp: {csp}");
        assert!(!csp.contains("unsafe-inline"), "csp: {csp}");
        assert!(csp.contains("script-src 'none'"), "csp: {csp}");
        assert!(csp.contains("frame-ancestors 'none'"), "csp: {csp}");
        assert_eq!(
            headers.get("x-content-type-options").expect("nosniff"),
            "nosniff"
        );
        assert_eq!(
            headers.get("referrer-policy").expect("referrer"),
            "no-referrer"
        );
        assert_eq!(headers.get("x-frame-options").expect("frame"), "DENY");
    }
}

#[tokio::test]
async fn the_login_path_is_reachable_without_a_session() {
    // Otherwise the redirect points at itself and nobody ever signs in.
    let (_d, app) = setup().await;
    let response = app
        .oneshot(get("blog.example.org", "/auth/login"))
        .await
        .expect("response");
    assert_ne!(response.status(), StatusCode::FORBIDDEN);
    let location = response
        .headers()
        .get("location")
        .and_then(|l| l.to_str().ok())
        .unwrap_or_default();
    assert_ne!(location, "/auth/login", "the login redirects to itself");
}

#[tokio::test]
async fn the_session_cookie_carries_its_defences() {
    // Task 8 promised this and never checked it. HttpOnly keeps a script that
    // should not exist from reading it anyway; Secure keeps it off plain HTTP;
    // SameSite=Lax is what stops a form on another site from posting here in
    // someone else's name, which is the CSRF defence this design relies on.
    let (dir, db, _app) = common::setup_with_db().await;
    let cookie = common::signed_in(&db, dir.path(), "ada", &["Household"]).await;
    assert!(!cookie.is_empty());

    let key = treff::web::load_or_create_cookie_key(dir.path()).expect("key");
    let jar = axum_extra::extract::cookie::PrivateCookieJar::new(key)
        .add(treff::web::session_cookie("whatever".into()));
    use axum::response::IntoResponse;
    let header = jar
        .into_response()
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .expect("Set-Cookie")
        .to_str()
        .expect("ascii")
        .to_string();

    assert!(header.contains("HttpOnly"), "{header}");
    assert!(header.contains("Secure"), "{header}");
    assert!(header.contains("SameSite=Lax"), "{header}");
    assert!(header.contains("Path=/"), "{header}");
}

#[tokio::test]
async fn a_forged_session_cookie_does_not_sign_anyone_in() {
    // The cookie is private (signed and encrypted). A value made up by the
    // client must not even be read, let alone looked up.
    let (_d, app) = setup().await;
    let request = Request::builder()
        .uri("/")
        .header("host", "blog.example.org")
        .header("cookie", "treff_session=deadbeef")
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}
