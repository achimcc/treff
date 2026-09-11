//! The setup every integration test shares.
//!
//! Cargo compiles this module separately into every test binary, so whatever
//! one file does not use is dead code *there*. That is what the allow is for —
//! not for genuinely unused helpers.
#![allow(dead_code)]

use treff::config::Config;

/// The address the provider is told to send people back to. One place, so a
/// test can ask "is this a route?" without spelling the path a second time.
pub const REDIRECT_URI: &str = "https://forum.example.org/auth/callback";

pub const CONFIGURATION: &str = r#"
# The blog as the design has it since 2026-09-06: articles come from files,
# so nobody opens one in a browser, and everybody with an account comments.
[[space]]
host  = "blog.example.org"
title = "Notes"
view  = "timeline"
read  = ["Household", "Friends"]

  [[space.category]]
  slug  = "notes"
  title = "Notes"
  post  = []
  reply = ["Household", "Friends"]

[[space]]
host  = "forum.example.org"
title = "Forum"
view  = "topics"
read  = ["Household", "Friends"]
home  = "https://example.org"

  [[space.category]]
  slug  = "general"
  title = "General"
  post  = ["Household", "Friends"]
  reply = ["Household", "Friends"]

  [[space.category]]
  slug  = "offtopic"
  title = "Off topic"
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
    // The tests build the state from the public pieces instead. The issuer
    // points nowhere, which is what keeps the suite free of a reachable
    // identity provider: discovery is attempted on first sign-in and fails
    // there, not here.
    let state = treff::web::AppState::new(
        Config::parse(CONFIGURATION).expect("configuration"),
        db.clone(),
        treff::auth::OidcSettings {
            issuer: "http://127.0.0.1:1/".into(),
            client_id: "treff-test".into(),
            client_secret: "test".into(),
            group_claim: "groups".into(),
        },
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
        email: None,
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

/// The body of a response as a string. Every test file wrote this itself.
pub async fn body_of(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf-8")
}

/// The markup of the `<details>` element whose opening tag is `open_tag`, up
/// to its `</details>`.
///
/// Crude on purpose: no page nests one `<details>` in another, and pulling an
/// HTML parser into the tree for one assertion would be a dependency bought
/// with a test. Panics rather than returning an option — a test that wants
/// this block and does not get it has already failed.
pub fn details_block<'a>(html: &'a str, open_tag: &str) -> &'a str {
    let start = html
        .find(open_tag)
        .unwrap_or_else(|| panic!("no {open_tag} in {html}"));
    let rest = &html[start..];
    let end = rest
        .find("</details>")
        .unwrap_or_else(|| panic!("unclosed {open_tag} in {html}"));
    &rest[..end]
}
