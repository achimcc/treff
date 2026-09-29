//! Every refusal reaches the log (homeserver audit 3, B119).
//!
//! A refusal used to be an answer and nothing else. These tests ask the
//! ROUTERS — public and internal — for each kind of refusal, and check that
//! the answer carries its mark (`Refused`) and that the logging middleware
//! saw it (`Logged`): a mark nobody logs is the old silence with extra steps.
//! The line itself is spelled out once, in `web::refusal`'s own tests.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use treff::web::refusal::{Kind, Logged, Refused};

mod common;
use common::{setup_with_db, signed_in};

const FORUM: &str = "forum.example.org";
const TOKEN: &str = "internal-token-0123456789";

fn refusal(response: &axum::response::Response) -> Option<(Kind, &'static str)> {
    let r = response.extensions().get::<Refused>()?;
    assert!(
        response.extensions().get::<Logged>().is_some(),
        "{:?} is marked and was never logged",
        r.kind
    );
    Some((r.kind, r.reason))
}

async fn public(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(request).await.expect("response")
}

fn get(host: &str, path: &str, cookie: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri(path).header("host", host);
    if let Some(c) = cookie {
        b = b.header("cookie", c);
    }
    b.body(Body::empty()).expect("request")
}

#[tokio::test]
async fn an_unknown_host_is_refused_and_logged() {
    let (_dir, _db, app) = setup_with_db().await;
    let r = public(&app, get("elsewhere.example.org", "/", None)).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert_eq!(refusal(&r), Some((Kind::Host, "unknown")));
}

#[tokio::test]
async fn a_change_from_another_site_is_refused_and_logged() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let r = public(
        &app,
        Request::builder()
            .method("POST")
            .uri("/notifications/read")
            .header("host", FORUM)
            .header("cookie", &ada)
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert_eq!(refusal(&r), Some((Kind::Csrf, "cross-site")));
}

/// A cookie that does not decrypt is somebody making one up — or the key
/// changed. Either is worth a line; no cookie at all is a first visit, and
/// is not.
#[tokio::test]
async fn a_forged_cookie_is_logged_and_no_cookie_is_not() {
    let (_dir, _db, app) = setup_with_db().await;
    let forged = public(
        &app,
        get(FORUM, "/", Some("treff_session=bm90LWEtc2Vzc2lvbg")),
    )
    .await;
    assert_eq!(forged.status(), StatusCode::SEE_OTHER);
    assert_eq!(refusal(&forged), Some((Kind::Cookie, "undecryptable")));

    let first_visit = public(&app, get(FORUM, "/", None)).await;
    assert_eq!(first_visit.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        refusal(&first_visit),
        None,
        "a first visit is not a refusal"
    );
}

#[tokio::test]
async fn a_session_that_is_over_is_logged() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    sqlx::query("DELETE FROM sessions")
        .execute(db.pool())
        .await
        .expect("end it");
    let r = public(&app, get(FORUM, "/", Some(&ada))).await;
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(refusal(&r), Some((Kind::Session, "gone")));
}

#[tokio::test]
async fn somebody_who_may_not_read_is_refused_and_logged() {
    let (dir, db, app) = setup_with_db().await;
    let eve = signed_in(&db, dir.path(), "eve", &["Neighbours"]).await;
    let r = public(&app, get(FORUM, "/", Some(&eve))).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert_eq!(refusal(&r), Some((Kind::Forbidden, "-")));
}

#[tokio::test]
async fn a_sign_in_that_does_not_complete_is_logged() {
    let (_dir, _db, app) = setup_with_db().await;
    let refused = public(&app, get(FORUM, "/auth/callback?error=access_denied", None)).await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refusal(&refused), Some((Kind::Login, "provider")));

    let unsolicited = public(&app, get(FORUM, "/auth/callback?code=c&state=s", None)).await;
    assert_eq!(unsolicited.status(), StatusCode::BAD_REQUEST);
    assert_eq!(refusal(&unsolicited), Some((Kind::Login, "not-started")));
}

/// THE CASE THE AUDIT WAS ABOUT: somebody next door guessing a token. Each
/// door says which one was knocked on; none of them says what was tried.
#[tokio::test]
async fn a_wrong_token_at_every_internal_door_is_logged() {
    let (_dir, db, _app) = setup_with_db().await;
    let internal = treff::web::internal::router(treff::web::internal::InternalState::new(
        std::sync::Arc::new(
            treff::config::Config::parse(common::CONFIGURATION).expect("configuration"),
        ),
        db.clone(),
        treff::web::internal::Tokens {
            events: Some(TOKEN.as_bytes().to_vec()),
            bell: Some(TOKEN.as_bytes().to_vec()),
            scim: Some(TOKEN.as_bytes().to_vec()),
        },
    ));
    for (method, path, door) in [
        ("POST", "/internal/events", "events"),
        ("GET", "/internal/bell", "bell"),
        ("GET", "/internal/bell/stream", "bell"),
        ("GET", "/scim/v2/Users", "scim"),
    ] {
        let r = internal
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", "Bearer guessed")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(refusal(&r), Some((Kind::Token, door)), "{path}");
    }
    // And the right token is no refusal.
    let fine = internal
        .oneshot(
            Request::builder()
                .uri("/internal/bell")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(fine.status(), StatusCode::OK);
    assert_eq!(refusal(&fine), None);
}
