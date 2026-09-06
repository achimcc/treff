//! The article path end to end: a file on disk, a page in a browser.
//!
//! The unit tests next to `mirror` prove the mirroring. This one proves the
//! part that only shows up when everything is wired together — that a mirrored
//! article is a page like any other, rendered through the same sanitizer and
//! behind the same reading rules.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

mod common;
use common::signed_in;

async fn body_of(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf-8")
}

const CONFIGURATION: &str = r#"
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
"#;

#[tokio::test]
async fn a_mirrored_article_is_an_ordinary_page() {
    let dir = tempfile::tempdir().expect("tempdir");
    let articles = dir.path().join("articles");
    std::fs::create_dir(&articles).expect("mkdir");
    std::fs::write(
        articles.join("2026-09-01-audiobooks.md"),
        "---\ntitle: Audiobooks are here\nkind: service\n---\n\nThere is a **new** service.\n\
         And a script: <script>alert(1)</script>\n",
    )
    .expect("write");

    let db = treff::db::Db::open(&dir.path().join("t.db"))
        .await
        .expect("open");
    treff::articles::mirror(&db, "blog.example.org", "notes", &articles, "2026-09-06")
        .await
        .expect("mirror");

    let state = treff::web::AppState::new(
        treff::config::Config::parse(CONFIGURATION).expect("configuration"),
        db.clone(),
        treff::auth::OidcSettings {
            issuer: "http://127.0.0.1:1/".into(),
            client_id: "t".into(),
            client_secret: "t".into(),
            group_claim: "groups".into(),
        },
        None,
        dir.path(),
    )
    .expect("state");
    let app = treff::web::router(state);

    let cookie = signed_in(&db, dir.path(), "friend", &["Friends"]).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "blog.example.org")
                .header("cookie", &cookie)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let html = body_of(response).await;
    assert!(html.contains("Audiobooks are here"), "{html}");
    assert!(
        html.contains("<strong>new</strong>"),
        "the article body is not rendered: {html}"
    );
    assert!(
        !html.contains("<script"),
        "an article is not more trusted for coming from a file: {html}"
    );
    assert!(
        !html.contains("kind: service"),
        "the front matter is on the page: {html}"
    );
}
