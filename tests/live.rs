//! The bell, live: Server-Sent Events that follow every change at once.
//!
//! "At once" is the promise, so every test here waits a short, fixed time for
//! the next frame and fails if it does not come — or, for somebody else's
//! change, fails if one DOES come.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::StreamExt;
use tower::ServiceExt;

mod common;
use common::{setup_with_db, signed_in};

const FORUM: &str = "forum.example.org";
const BELL: &str = "bell-token-9876543210";

type Frames = std::pin::Pin<Box<dyn futures_util::Stream<Item = String> + Send>>;

/// The `data:` lines of a response, one item per event.
fn frames(response: axum::response::Response) -> Frames {
    let mut buffer = String::new();
    Box::pin(
        response
            .into_body()
            .into_data_stream()
            .map(move |chunk| {
                buffer.push_str(&String::from_utf8_lossy(&chunk.expect("chunk")));
                let mut out = Vec::new();
                while let Some(end) = buffer.find("\n\n") {
                    let event: String = buffer.drain(..end + 2).collect();
                    for line in event.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            out.push(data.to_string());
                        }
                    }
                }
                futures_util::stream::iter(out)
            })
            .flatten(),
    )
}

async fn next(frames: &mut Frames) -> Option<serde_json::Value> {
    tokio::time::timeout(std::time::Duration::from_secs(3), frames.next())
        .await
        .ok()
        .flatten()
        .map(|d| serde_json::from_str(&d).expect("json"))
}

async fn nothing_for_a_while(frames: &mut Frames) -> bool {
    tokio::time::timeout(std::time::Duration::from_millis(700), frames.next())
        .await
        .is_err()
}

fn person(subject: &str) -> treff::authz::Identity {
    treff::authz::Identity {
        subject: subject.into(),
        name: subject.into(),
        groups: vec!["Household".into()],
        email: None,
        handle: Some(subject.into()),
    }
}

async fn stream(app: &axum::Router, cookie: &str) -> Frames {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/notifications/stream")
                .header("host", FORUM)
                .header("cookie", cookie)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    frames(response)
}

#[tokio::test]
async fn the_stream_follows_a_reply_a_mention_an_event_and_a_read() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let topic = treff::db::topics::create_topic(&db, FORUM, "general", "T", "B", &person("ada"))
        .await
        .expect("topic");
    let mut s = stream(&app, &ada).await;
    assert_eq!(next(&mut s).await.expect("on connect")["unread"], 0);

    treff::db::topics::add_reply(&db, topic, "one", &person("ben"))
        .await
        .expect("reply");
    assert_eq!(next(&mut s).await.expect("after a reply")["unread"], 1);

    let other = treff::db::topics::create_topic(&db, FORUM, "general", "U", "B", &person("cem"))
        .await
        .expect("topic");
    treff::db::topics::add_reply_mentioning(&db, other, "@ada", &person("cem"), &["ada".into()])
        .await
        .expect("mention");
    assert_eq!(next(&mut s).await.expect("after a mention")["unread"], 2);

    let config = treff::config::Config::parse(common::CONFIGURATION).expect("configuration");
    let event = treff::db::events::checked(
        treff::db::events::Raw {
            handle: "ada".into(),
            kind: "film_available".into(),
            title: "Dune".into(),
            link: None,
            reason: None,
            source_key: "seerr:1".into(),
        },
        config.events.as_ref().expect("events"),
    )
    .expect("fine");
    treff::db::events::take(&db, FORUM, &event)
        .await
        .expect("take");
    assert_eq!(next(&mut s).await.expect("after an event")["unread"], 3);

    treff::db::inbox::mark_all_read(&db, "ada", FORUM)
        .await
        .expect("read");
    assert_eq!(next(&mut s).await.expect("after reading")["unread"], 0);
}

/// Somebody else's news is not sent to you — not even as "still 0".
#[tokio::test]
async fn a_stream_says_nothing_about_somebody_elses_news() {
    let (dir, db, app) = setup_with_db().await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let topic = treff::db::topics::create_topic(&db, FORUM, "general", "T", "B", &person("ada"))
        .await
        .expect("topic");
    let mut s = stream(&app, &ben).await;
    assert_eq!(next(&mut s).await.expect("on connect")["unread"], 0);
    treff::db::topics::add_reply(&db, topic, "for ada", &person("cem"))
        .await
        .expect("reply");
    assert!(
        nothing_for_a_while(&mut s).await,
        "ben heard about ada's reply"
    );
}

fn internal(db: &treff::db::Db) -> axum::Router {
    treff::web::internal::router(treff::web::internal::InternalState::new(
        std::sync::Arc::new(
            treff::config::Config::parse(common::CONFIGURATION).expect("configuration"),
        ),
        db.clone(),
        None,
        Some(BELL.as_bytes().to_vec()),
    ))
}

fn internal_stream(token: &str) -> Request<Body> {
    Request::builder()
        .uri("/internal/bell/stream")
        .header("authorization", format!("Bearer {token}"))
        .header("x-treff-user", "ada")
        .header("x-treff-groups", "Household")
        .body(Body::empty())
        .expect("request")
}

/// The start page's stream: behind its token, and it follows the same
/// changes, with the entries.
#[tokio::test]
async fn the_internal_stream_needs_its_token_and_follows_too() {
    let (dir, db, _app) = setup_with_db().await;
    // Ada has been to the forum: without an account the start page shows her
    // events only (`inbox::for_handle`), and this test is about replies.
    signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let app = internal(&db);
    let refused = app
        .clone()
        .oneshot(internal_stream("wrong"))
        .await
        .expect("r");
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);

    let response = app.oneshot(internal_stream(BELL)).await.expect("r");
    assert_eq!(response.status(), StatusCode::OK);
    let mut s = frames(response);
    let first = next(&mut s).await.expect("on connect");
    assert_eq!(first["unread"], 0);
    assert_eq!(first["entries"], serde_json::json!([]));

    let topic =
        treff::db::topics::create_topic(&db, FORUM, "general", "Film night", "B", &person("ada"))
            .await
            .expect("topic");
    treff::db::topics::add_reply(&db, topic, "me", &person("ben"))
        .await
        .expect("reply");
    let after = next(&mut s).await.expect("after a reply");
    assert_eq!(after["unread"], 1);
    assert_eq!(after["entries"][0]["title"], "Film night");
}
