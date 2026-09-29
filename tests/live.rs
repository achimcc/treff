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

    treff::db::inbox::mark_all_read(&db, "ada", &[FORUM.to_string()])
        .await
        .expect("read");
    assert_eq!(next(&mut s).await.expect("after reading")["unread"], 0);
}

/// One bell everywhere: a page open on the forum hears about the blog.
#[tokio::test]
async fn a_forum_stream_follows_the_blog() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let topic =
        treff::db::topics::create_topic(&db, "blog.example.org", "notes", "N", "B", &person("ada"))
            .await
            .expect("topic");
    let mut s = stream(&app, &ada).await;
    assert_eq!(next(&mut s).await.expect("on connect")["unread"], 0);
    treff::db::topics::add_reply(&db, topic, "a comment", &person("ben"))
        .await
        .expect("reply");
    let frame = next(&mut s).await.expect("after a blog reply");
    assert_eq!(frame["unread"], 1);
    assert!(
        frame["entries"][0]["link"]
            .as_str()
            .is_some_and(|l| l.starts_with("https://blog.example.org/t/")),
        "{frame}"
    );
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
        treff::web::internal::Tokens {
            bell: Some(BELL.as_bytes().to_vec()),
            ..Default::default()
        },
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

/// The stream has ended: the body is over, and no frame came first.
async fn ended(frames: &mut Frames) -> bool {
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(3), frames.next()).await,
        Ok(None)
    )
}

async fn raw_stream(app: &axum::Router, cookie: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri("/notifications/stream")
                .header("host", FORUM)
                .header("cookie", cookie)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
}

/// A STREAM ENDS WITH ITS SESSION (homeserver audit 3, B115). The person
/// was fixed when the stream opened, and every later change was answered
/// for them — after a sign-out, an expiry or a switched-off account, an open
/// tab went on showing titles, names and numbers. Now each change asks for
/// the session first, and a session that is over ends the stream instead of
/// feeding it.
#[tokio::test]
async fn a_stream_ends_when_its_session_is_signed_out() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let topic = treff::db::topics::create_topic(&db, FORUM, "general", "T", "B", &person("ada"))
        .await
        .expect("topic");
    let mut s = stream(&app, &ada).await;
    assert_eq!(next(&mut s).await.expect("on connect")["unread"], 0);

    let out = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/logout")
                .header("host", FORUM)
                .header("cookie", &ada)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("logout");
    assert_eq!(out.status(), StatusCode::SEE_OTHER);

    treff::db::topics::add_reply(&db, topic, "after the sign-out", &person("ben"))
        .await
        .expect("reply");
    assert!(ended(&mut s).await, "the stream went on after the sign-out");
}

/// The same for the directory: deactivated, or out of every group that may
/// read — the next change ends the stream (B93 made the SESSION follow the
/// directory; this is the stream that was opened before).
#[tokio::test]
async fn a_stream_ends_when_the_directory_switches_the_account_off() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let mut s = stream(&app, &ada).await;
    assert_eq!(next(&mut s).await.expect("on connect")["unread"], 0);
    treff::db::directory::put_user(
        &db,
        &treff::db::directory::User {
            id: "ada".into(),
            user_name: "ada".into(),
            display_name: "Ada".into(),
            email: None,
            active: false,
        },
    )
    .await
    .expect("deactivate");
    assert!(ended(&mut s).await, "the stream outlived the account");
}

#[tokio::test]
async fn a_stream_ends_when_its_person_may_no_longer_read() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let mut s = stream(&app, &ada).await;
    assert_eq!(next(&mut s).await.expect("on connect")["unread"], 0);
    // Active, and in no group at all: the directory recomputes the groups
    // from its memberships, of which there are none.
    treff::db::directory::put_user(
        &db,
        &treff::db::directory::User {
            id: "ada".into(),
            user_name: "ada".into(),
            display_name: "Ada".into(),
            email: None,
            active: true,
        },
    )
    .await
    .expect("no groups");
    assert!(ended(&mut s).await, "the stream outlived the permission");
}

/// A LIMIT PER PERSON. Every open stream costs a query per change; without
/// a limit one person could open hundreds. Beyond it: 429 — and a stream
/// that closes gives its place back.
#[tokio::test]
async fn one_person_opens_only_so_many_streams() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let mut open = Vec::new();
    for _ in 0..treff::live::STREAMS_PER_PERSON {
        let r = raw_stream(&app, &ada).await;
        assert_eq!(r.status(), StatusCode::OK);
        open.push(r);
    }
    let one_more = raw_stream(&app, &ada).await;
    assert_eq!(one_more.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        raw_stream(&app, &ben).await.status(),
        StatusCode::OK,
        "somebody else is not counted against ada"
    );
    drop(open.pop());
    assert_eq!(
        raw_stream(&app, &ada).await.status(),
        StatusCode::OK,
        "a closed stream gives its place back"
    );
}

/// The start page's door counts too, per person it names.
#[tokio::test]
async fn the_internal_door_opens_only_so_many_streams_per_person() {
    let (_dir, db, _app) = setup_with_db().await;
    let app = internal(&db);
    let mut open = Vec::new();
    for _ in 0..treff::live::STREAMS_PER_PERSON {
        let r = app.clone().oneshot(internal_stream(BELL)).await.expect("r");
        assert_eq!(r.status(), StatusCode::OK);
        open.push(r);
    }
    let r = app.clone().oneshot(internal_stream(BELL)).await.expect("r");
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    drop(open.pop());
    let r = app.oneshot(internal_stream(BELL)).await.expect("r");
    assert_eq!(r.status(), StatusCode::OK);
}
