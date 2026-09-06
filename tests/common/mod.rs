//! The setup every integration test shares.
//!
//! Cargo compiles this module separately into every test binary, so whatever
//! one file does not use is dead code *there*. That is what the allow is for —
//! not for genuinely unused helpers.
#![allow(dead_code)]

use treff::config::Config;

pub const CONFIGURATION: &str = r#"
[[space]]
host  = "blog.example.org"
title = "Notes"
view  = "timeline"
read  = ["Household"]

  [[space.category]]
  slug  = "notes"
  title = "Notes"
  post  = ["Writers"]
  reply = ["Household"]

[[space]]
host  = "forum.example.org"
title = "Forum"
view  = "topics"
read  = ["Household", "Friends"]

  [[space.category]]
  slug  = "general"
  title = "General"
  post  = ["Household", "Friends"]
  reply = ["Household", "Friends"]
"#;

/// A router backed by a fresh database in a temporary directory. The directory
/// is returned along with it: dropping it deletes the database, so it has to
/// outlive the test.
pub async fn setup() -> (tempfile::TempDir, axum::Router) {
    let (dir, _db, app) = setup_with_db().await;
    (dir, app)
}

pub async fn setup_with_db() -> (tempfile::TempDir, treff::db::Db, axum::Router) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = treff::db::Db::open(&dir.path().join("t.db"))
        .await
        .expect("open");
    // No `AppState::for_tests` in the library: a constructor carrying a fixed
    // cookie key would ship inside the binary, and Cargo no longer allows the
    // usual dodge of a `testing` feature enabled through a self dev-dependency.
    // The tests build the state from the public pieces instead. `provider:
    // None` is what makes the suite independent of a reachable identity
    // provider.
    let state = treff::web::AppState::new(
        Config::parse(CONFIGURATION).expect("configuration"),
        db.clone(),
        treff::auth::OidcSettings {
            issuer: "http://127.0.0.1:1/".into(),
            client_id: "treff-test".into(),
            client_secret: "test".into(),
            group_claim: "groups".into(),
        },
        None,
        dir.path(),
    )
    .expect("state");
    let app = treff::web::router(state);
    (dir, db, app)
}

/// Creates a session for someone in `groups` and returns the `Cookie:` header
/// value that carries it.
///
/// The cookie is built with the very function the application uses, and
/// encrypted with the key from the same directory — so a test that passes here
/// means a browser would be let in for the same reason.
pub async fn signed_in(
    db: &treff::db::Db,
    dir: &std::path::Path,
    subject: &str,
    groups: &[&str],
) -> String {
    let identity = treff::authz::Identity {
        subject: subject.into(),
        name: format!("{subject} the tester"),
        groups: groups.iter().map(|g| (*g).to_string()).collect(),
    };
    let sid = treff::auth::Sessions::create(db, &identity)
        .await
        .expect("session");

    let key = treff::web::load_or_create_cookie_key(dir).expect("key");
    let jar = axum_extra::extract::cookie::PrivateCookieJar::new(key)
        .add(treff::web::session_cookie(sid));

    // The jar's own iterator hands back the DECRYPTED value — useful for a
    // handler, useless for a client. What a browser would send is what the
    // Set-Cookie header carries, so take it from there.
    use axum::response::IntoResponse;
    let response = jar.into_response();
    let set_cookie = response
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .expect("Set-Cookie")
        .to_str()
        .expect("ascii");
    set_cookie
        .split(';')
        .next()
        .expect("name=value")
        .to_string()
}
