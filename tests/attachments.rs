//! Attachments end to end: the only path on which someone else's bytes enter
//! the server.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

mod common;
use common::{setup_with_db, signed_in};

const BOUNDARY: &str = "----treffboundary";

fn upload(
    host: &str,
    uri: &str,
    cookie: &str,
    filename: &str,
    mime: &str,
    bytes: &[u8],
) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {mime}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());

    Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", host)
        .header("cookie", cookie)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .expect("request")
}

fn get(host: &str, uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("host", host)
        .header("cookie", cookie)
        .body(Body::empty())
        .expect("request")
}

fn png() -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    v.extend_from_slice(&[0u8; 64]);
    v
}

async fn a_topic(db: &treff::db::Db) -> i64 {
    let author = treff::authz::Identity {
        subject: "ada".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: None,
    };
    treff::db::topics::create_topic(db, "forum.example.org", "general", "Pictures", "x", &author)
        .await
        .expect("topic")
}

fn files_in(dir: &std::path::Path) -> Vec<String> {
    let attachments = dir.join("attachments");
    match std::fs::read_dir(&attachments) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[tokio::test]
async fn an_allowed_image_is_stored_and_served_back() {
    let (dir, db, app) = setup_with_db().await;
    let topic = a_topic(&db).await;
    let cookie = signed_in(&db, dir.path(), "ada", &["Household"]).await;

    let response = app
        .clone()
        .oneshot(upload(
            "forum.example.org",
            &format!("/t/{topic}/attach"),
            &cookie,
            "holiday.png",
            "image/png",
            &png(),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let stored = files_in(dir.path());
    assert_eq!(stored.len(), 1, "nothing on disk: {stored:?}");
    assert!(
        stored[0].ends_with(".png") && !stored[0].contains("holiday"),
        "the uploader named the file: {stored:?}"
    );

    // The page shows it, and the link is the one the sanitizer kept.
    let page = axum::body::to_bytes(
        app.clone()
            .oneshot(get("forum.example.org", &format!("/t/{topic}"), &cookie))
            .await
            .expect("response")
            .into_body(),
        1 << 20,
    )
    .await
    .expect("body");
    let page = String::from_utf8(page.to_vec()).expect("utf-8");
    let start = page.find("/a/").expect("no image on the page");
    let id: String = page[start + 3..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();

    let served = app
        .oneshot(get("forum.example.org", &format!("/a/{id}"), &cookie))
        .await
        .expect("response");
    assert_eq!(served.status(), StatusCode::OK);
    assert_eq!(
        served.headers().get("content-type").expect("type"),
        "image/png"
    );
    assert_eq!(
        served
            .headers()
            .get("x-content-type-options")
            .expect("sniff"),
        "nosniff",
        "an attachment without nosniff is a stored XSS waiting for a browser"
    );
}

#[tokio::test]
async fn an_svg_is_refused_and_nothing_reaches_the_disk() {
    // The one that matters: SVG is executable XML, and it is why the list of
    // allowed types is an allow list.
    let (dir, db, app) = setup_with_db().await;
    let topic = a_topic(&db).await;
    let cookie = signed_in(&db, dir.path(), "ada", &["Household"]).await;

    let response = app
        .oneshot(upload(
            "forum.example.org",
            &format!("/t/{topic}/attach"),
            &cookie,
            "innocent.png",
            "image/png",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>",
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(
        files_in(dir.path()).is_empty(),
        "a refused upload was written anyway"
    );
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM attachments")
        .fetch_one(db.pool())
        .await
        .expect("query");
    assert_eq!(n, 0);
}

#[tokio::test]
async fn a_file_over_the_limit_is_refused() {
    let (dir, db, app) = setup_with_db().await;
    let topic = a_topic(&db).await;
    let cookie = signed_in(&db, dir.path(), "ada", &["Household"]).await;

    let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
    big.resize(9 * 1024 * 1024, 0);
    let response = app
        .oneshot(upload(
            "forum.example.org",
            &format!("/t/{topic}/attach"),
            &cookie,
            "big.png",
            "image/png",
            &big,
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(files_in(dir.path()).is_empty());
}

#[tokio::test]
async fn an_attachment_from_another_space_is_not_found() {
    let (dir, db, app) = setup_with_db().await;
    let topic = a_topic(&db).await;
    let cookie = signed_in(&db, dir.path(), "ada", &["Household", "Friends"]).await;

    app.clone()
        .oneshot(upload(
            "forum.example.org",
            &format!("/t/{topic}/attach"),
            &cookie,
            "x.png",
            "image/png",
            &png(),
        ))
        .await
        .expect("response");

    let (id,): (String,) = sqlx::query_as("SELECT id FROM attachments LIMIT 1")
        .fetch_one(db.pool())
        .await
        .expect("query");

    let response = app
        .oneshot(get("blog.example.org", &format!("/a/{id}"), &cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_attachment_needs_the_reading_group() {
    let (dir, db, app) = setup_with_db().await;
    let topic = a_topic(&db).await;
    let member = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    app.clone()
        .oneshot(upload(
            "forum.example.org",
            &format!("/t/{topic}/attach"),
            &member,
            "x.png",
            "image/png",
            &png(),
        ))
        .await
        .expect("response");
    let (id,): (String,) = sqlx::query_as("SELECT id FROM attachments LIMIT 1")
        .fetch_one(db.pool())
        .await
        .expect("query");

    let stranger = signed_in(&db, dir.path(), "stranger", &["Strangers"]).await;
    let response = app
        .oneshot(get("forum.example.org", &format!("/a/{id}"), &stranger))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn uploading_follows_the_reply_right() {
    // In the blog nobody opens a topic, but everybody comments — and an
    // upload is a comment with a picture in it.
    let (dir, db, app) = setup_with_db().await;
    let article_author = treff::authz::Identity {
        subject: "treff:article".into(),
        name: "treff".into(),
        groups: vec![],
        email: None,
    };
    let topic = treff::db::topics::create_topic(
        &db,
        "blog.example.org",
        "notes",
        "An article",
        "x",
        &article_author,
    )
    .await
    .expect("topic");

    let friend = signed_in(&db, dir.path(), "friend", &["Friends"]).await;
    let response = app
        .clone()
        .oneshot(upload(
            "blog.example.org",
            &format!("/t/{topic}/attach"),
            &friend,
            "x.png",
            "image/png",
            &png(),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let stranger = signed_in(&db, dir.path(), "stranger", &["Strangers"]).await;
    let response = app
        .oneshot(upload(
            "blog.example.org",
            &format!("/t/{topic}/attach"),
            &stranger,
            "x.png",
            "image/png",
            &png(),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
