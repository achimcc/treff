//! The setup every integration test shares.

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
