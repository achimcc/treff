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
        // ONE SCRIPT, FROM HERE (ADR 0005): `'self'` and nothing else — no
        // inline, no hash, no other origin.
        let script = csp
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with("script-src"))
            .unwrap_or_else(|| panic!("no script-src: {csp}"));
        assert_eq!(script, "script-src 'self'", "csp: {csp}");
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

/// The stylesheet must be REVALIDATED, not remembered.
///
/// It carried no `ETag` and no `Cache-Control` until 2026-09-06, which leaves
/// a browser free to heuristically cache it — so a redesign that is deployed,
/// running and served correctly can still be invisible to the person looking
/// at it. That is the worst kind of "fixed": every measurement says yes and
/// the screen says no.
#[tokio::test]
async fn the_stylesheet_is_revalidated_rather_than_remembered() {
    let (_dir, app) = common::setup().await;

    let first = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/assets/style.css")
                .header("host", "forum.example.org")
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(first.status(), axum::http::StatusCode::OK);
    let etag = first
        .headers()
        .get(axum::http::header::ETAG)
        .expect("an ETag, or the browser has nothing to ask about")
        .to_str()
        .expect("ascii")
        .to_string();
    assert_eq!(
        first
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-cache"),
        "no-cache means 'keep it, but ask first' — not 'do not keep it'"
    );

    // And the second request, with that ETag, costs a header and no bytes.
    let second = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/assets/style.css")
                .header("host", "forum.example.org")
                .header("if-none-match", &etag)
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(second.status(), axum::http::StatusCode::NOT_MODIFIED);
}

/// A space is one address among several, and the way back to the others has to
/// be ON the page. Without it a forum reached from a landing page is a
/// one-way street: the browser's back button is not navigation, it is memory.
#[tokio::test]
async fn a_configured_home_is_linked_and_an_unconfigured_one_is_not() {
    let (dir, db, app) = common::setup_with_db().await;
    let cookie = common::signed_in(&db, dir.path(), "someone", &["Household"]).await;

    let page = common::body_of(
        app.oneshot(
            Request::builder()
                .uri("/")
                .header("host", "forum.example.org")
                .header("cookie", &cookie)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response"),
    )
    .await;

    // `forum` carries `home` in the test configuration, `blog` does not — so
    // one assertion cannot pass by accident while the other fails.
    assert!(
        page.contains("https://example.org"),
        "the forum links home: {page}"
    );
    assert!(
        page.contains("example.org</a>"),
        "and says where home is, rather than showing a bare arrow: {page}"
    );
}

/// The SECOND line of defence against CSRF.
///
/// Until 2026-09-20 there was only one: `SameSite=Lax` on the session cookie,
/// asserted a few tests above. That line holds in a browser and nowhere else,
/// and only for as long as the browser is the one enforcing it — an audit on
/// 2026-09-15 posted to `/t/1/reply` from a foreign origin and got a 303 with
/// the post created, because the server itself asked nothing.
///
/// `Sec-Fetch-Site` is the header to ask, not `Origin` or `Referer`: this
/// site sends `Referrer-Policy: no-referrer` on purpose, and a browser sends
/// the fetch metadata anyway. A request a browser labels as coming from
/// somewhere else is refused before it reaches a handler.
#[tokio::test]
async fn a_post_that_a_browser_marks_as_cross_site_is_refused() {
    let (dir, db, app) = common::setup_with_db().await;
    let cookie = common::signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let response = app
        .oneshot(post_from("forum.example.org", &cookie, Some("cross-site")))
        .await
        .expect("response");
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a cross-site POST reached a handler"
    );
}

/// `same-site` is not `same-origin`, and here that difference is the point:
/// every space is its own host with its own groups, so a page served by one
/// space is as foreign to another as any other site is.
#[tokio::test]
async fn a_post_from_a_neighbouring_space_is_refused_too() {
    let (dir, db, app) = common::setup_with_db().await;
    let cookie = common::signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let response = app
        .oneshot(post_from("forum.example.org", &cookie, Some("same-site")))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// And the check must not lock out the site's own forms. `same-origin` is
/// what a browser sends for them; the reply below then fails for a reason of
/// its own (there is no topic 1 in an empty database), and that is exactly
/// the point — it got PAST the gate.
#[tokio::test]
async fn a_same_origin_post_passes_the_gate() {
    let (dir, db, app) = common::setup_with_db().await;
    let cookie = common::signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let response = app
        .oneshot(post_from("forum.example.org", &cookie, Some("same-origin")))
        .await
        .expect("response");
    assert_ne!(
        response.status(),
        StatusCode::FORBIDDEN,
        "the site's own form was refused"
    );
}

/// A client that sends no fetch metadata at all is not refused.
///
/// CSRF is an attack that borrows a BROWSER's cookies, and every browser that
/// can be borrowed from sends these headers — the page doing the borrowing
/// cannot suppress them. Anything else (a script, `curl`, a browser older
/// than the header) has no cookies to borrow, so refusing it would cost
/// reachability and buy nothing.
#[tokio::test]
async fn a_request_without_fetch_metadata_is_let_through() {
    let (dir, db, app) = common::setup_with_db().await;
    let cookie = common::signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let response = app
        .oneshot(post_from("forum.example.org", &cookie, None))
        .await
        .expect("response");
    assert_ne!(response.status(), StatusCode::FORBIDDEN);
}

/// Reading stays reachable from anywhere. A link in a mail or a chat arrives
/// as `cross-site` or `none`, and a GET changes nothing — since 2026-09-20
/// that is true of `/auth/logout` too.
#[tokio::test]
async fn a_cross_site_get_still_reaches_the_page() {
    let (dir, db, app) = common::setup_with_db().await;
    let cookie = common::signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let request = Request::builder()
        .uri("/")
        .header("host", "forum.example.org")
        .header("cookie", &cookie)
        .header("sec-fetch-site", "cross-site")
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
}

/// A reply, posted the way a browser would, with the fetch metadata a browser
/// would attach — or none at all.
fn post_from(host: &str, cookie: &str, site: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/t/1/reply")
        .header("host", host)
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded");
    if let Some(site) = site {
        builder = builder.header("sec-fetch-site", site);
    }
    builder.body(Body::from("body=hello")).expect("request")
}

/// The one script: from its own route, open like the stylesheet (it carries
/// nothing about anybody), with a validator so a new version is not hidden by
/// a cache.
#[tokio::test]
async fn the_mention_script_is_served_from_its_own_route() {
    let (_d, app) = setup().await;
    let response = app
        .clone()
        .oneshot(get("forum.example.org", "/assets/mention.js"))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(headers["content-type"], "text/javascript; charset=utf-8");
    assert_eq!(headers["cache-control"], "no-cache");
    let etag = headers["etag"].to_str().expect("ascii").to_string();

    let again = app
        .oneshot(
            Request::builder()
                .uri("/assets/mention.js")
                .header("host", "forum.example.org")
                .header("if-none-match", &etag)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
}

/// The page loads it, and loads nothing else: no inline script, no event
/// attribute. Everything the page does still works without it.
#[tokio::test]
async fn a_page_loads_the_script_and_no_inline_code() {
    let (dir, db, app) = common::setup_with_db().await;
    let cookie = common::signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "forum.example.org")
                .header("cookie", &cookie)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let html = common::body_of(response).await;
    assert!(
        html.contains(r#"<script src="/assets/mention.js" defer></script>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<script src="/assets/bell.js" defer></script>"#),
        "{html}"
    );
    assert_eq!(html.matches("<script").count(), 3, "{html}");
    let lower = html.to_lowercase();
    for attribute in [
        " onclick=",
        " onload=",
        " oninput=",
        " onkeydown=",
        " onerror=",
    ] {
        assert!(!lower.contains(attribute), "{attribute} in {html}");
    }
}

/// The bell's script, from its own route like the other one.
#[tokio::test]
async fn the_bell_script_is_served_from_its_own_route() {
    let (_d, app) = setup().await;
    let response = app
        .oneshot(get("forum.example.org", "/assets/bell.js"))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/javascript; charset=utf-8"
    );
    assert!(response.headers().contains_key("etag"));
}
