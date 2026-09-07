//! Notifications, end to end: a reply, a queue, and a server that receives it.
//!
//! The sequencing (retry, back off, give up) is unit-tested in `src/notify`
//! against a transport that refuses everything. THIS file exists for the part
//! that cannot be faked: that a message actually goes over a socket, in a
//! shape an SMTP server accepts.

mod common;
use common::setup_with_db;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tower::ServiceExt as _;

/// A minimal SMTP server: enough of the conversation to accept one message,
/// and no more. It returns what it received, so the test can assert on the
/// bytes rather than on "no error".
///
/// No TLS: this listens on loopback inside one test process. `Settings`
/// refuses that combination as soon as a password is configured, which is the
/// rule that matters outside a test.
async fn one_shot_smtp() -> (std::net::SocketAddr, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");

    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut received = String::new();
        let mut in_data = false;

        write.write_all(b"220 test ESMTP\r\n").await.expect("greet");
        while let Ok(Some(line)) = lines.next_line().await {
            if in_data {
                if line == "." {
                    write.write_all(b"250 Ok\r\n").await.expect("ok");
                    // ONE MESSAGE AND DONE. Waiting for `QUIT` made this test
                    // hang for seven seconds and then fail: without a
                    // connection pool lettre closes the socket, and with one
                    // it keeps it — either way the message is complete here,
                    // and that is what the test is about.
                    break;
                }
                received.push_str(&line);
                received.push('\n');
                continue;
            }
            let upper = line.to_ascii_uppercase();
            if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                // The 8BITMIME line matters: without it lettre would have to
                // encode differently, and the test would be asserting about a
                // conversation nobody has.
                write
                    .write_all(b"250-test\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n")
                    .await
                    .expect("ehlo");
            } else if upper.starts_with("MAIL") || upper.starts_with("RCPT") {
                received.push_str(&line);
                received.push('\n');
                write.write_all(b"250 Ok\r\n").await.expect("ok");
            } else if upper.starts_with("DATA") {
                in_data = true;
                write.write_all(b"354 Go\r\n").await.expect("go");
            } else if upper.starts_with("QUIT") {
                write.write_all(b"221 Bye\r\n").await.expect("bye");
                break;
            } else {
                write.write_all(b"250 Ok\r\n").await.expect("ok");
            }
        }
        received
    });

    (addr, handle)
}

#[tokio::test]
async fn a_reply_reaches_the_mail_server() {
    let (dir, db, _app) = setup_with_db().await;
    let (addr, server) = one_shot_smtp().await;

    let ada = treff::authz::Identity {
        subject: "ada".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: Some("ada@example.org".into()),
    };
    let ben = treff::authz::Identity {
        subject: "ben".into(),
        name: "Ben".into(),
        groups: vec!["Household".into()],
        email: Some("ben@example.org".into()),
    };
    // The account rows are what the sender looks the address up in — the
    // session they arrived with is long gone by the time mail goes out.
    treff::auth::Sessions::create(&db, &ada).await.expect("ada");
    treff::auth::Sessions::create(&db, &ben).await.expect("ben");

    let topic = treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "Who had the projector?",
        "Not in the cupboard.",
        &ada,
    )
    .await
    .expect("topic");
    treff::db::topics::add_reply(&db, topic, "It is at my place.", &ben)
        .await
        .expect("reply");

    let settings = treff::notify::mail::Settings {
        host: addr.ip().to_string(),
        port: addr.port(),
        username: None,
        password: None,
        from: "treff@example.org".into(),
        starttls: false,
    };
    let mailer = treff::notify::mail::Mailer::new(&settings).expect("mailer");

    let config = std::sync::Arc::new(
        treff::config::Config::parse(common::CONFIGURATION).expect("configuration"),
    );
    let key = std::sync::Arc::new(vec![3u8; 64]);
    let composer = |row: &treff::db::outbox::Owed| {
        let db = db.clone();
        let config = config.clone();
        let key = key.clone();
        let row = row.clone();
        Box::pin(async move { treff::notify::compose(&db, &config, &key, &row).await })
            as treff::notify::BoxFuture<anyhow::Result<Option<treff::notify::Message>>>
    };

    let sent = treff::notify::drain_once(&db, &mailer, composer, 10)
        .await
        .expect("drain");
    assert_eq!(sent, 1, "ada is owed one; ben wrote it and is not");

    let received = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("the server answered in time")
        .expect("server task");

    assert!(received.contains("RCPT TO:<ada@example.org>"), "{received}");
    assert!(
        !received.contains("ben@example.org"),
        "the writer is not mailed: {received}"
    );
    assert!(
        received.contains("Who had the projector?"),
        "the subject names the topic: {received}"
    );
    assert!(
        received.contains("It is at my place."),
        "and the body is the reply: {received}"
    );
    assert!(
        received.contains("https://forum.example.org/t/"),
        "with a link back that works from a mail client: {received}"
    );

    assert!(
        received.contains("List-Unsubscribe:"),
        "a mail client can offer the button itself: {received}"
    );
    assert!(
        received.contains("/u/"),
        "and the way out travels with the notification: {received}"
    );

    // Nothing is owed twice — ON THIS CHANNEL. The webhook row is still
    // there, because nothing has drained it: the two do not drain each other,
    // and that is the point of keeping them apart.
    let offen = treff::db::outbox::due(&db, 10).await.expect("due");
    assert!(!offen.iter().any(|o| o.kanal == "mail"), "{offen:?}");
    assert_eq!(offen.iter().filter(|o| o.kanal == "webhook").count(), 1);
    drop(dir);
}

/// The way out, over the router, WITHOUT a session — which is the whole point
/// of it.
#[tokio::test]
async fn the_unsubscribe_link_works_without_signing_in() {
    let (dir, db, app) = setup_with_db().await;

    let ada = treff::authz::Identity {
        subject: "ada".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: Some("ada@example.org".into()),
    };
    let ben = treff::authz::Identity {
        subject: "ben".into(),
        name: "Ben".into(),
        groups: vec!["Household".into()],
        email: Some("ben@example.org".into()),
    };
    let topic =
        treff::db::topics::create_topic(&db, "forum.example.org", "general", "T", "B", &ada)
            .await
            .expect("topic");
    treff::db::topics::add_reply(&db, topic, "A reply", &ben)
        .await
        .expect("reply");

    let row = treff::db::outbox::due(&db, 1).await.expect("due").remove(0);
    let key = treff::notify::unsubscribe::load_or_create_key(dir.path()).expect("key");
    let token = treff::notify::unsubscribe::token(&key, row.id);

    let request = |method: &str, uri: String| {
        axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "forum.example.org")
            .body(axum::body::Body::empty())
            .expect("request")
    };

    // No cookie anywhere in this test — that is the assertion.
    let page = app
        .clone()
        .oneshot(request("GET", format!("/u/{}/{token}", row.id)))
        .await
        .expect("response");
    assert_eq!(page.status(), axum::http::StatusCode::OK);

    assert!(
        treff::db::subscriptions::is_following(&db, "ada", topic)
            .await
            .expect("check")
    );
    let done = app
        .clone()
        .oneshot(request("POST", format!("/u/{}/{token}", row.id)))
        .await
        .expect("response");
    assert_eq!(done.status(), axum::http::StatusCode::OK);
    assert!(
        !treff::db::subscriptions::is_following(&db, "ada", topic)
            .await
            .expect("check")
    );

    // Twice, because a link in a mail gets clicked twice — and because
    // `List-Unsubscribe-Post` means a mail client may send it unasked.
    let again = app
        .clone()
        .oneshot(request("POST", format!("/u/{}/{token}", row.id)))
        .await
        .expect("response");
    assert_eq!(again.status(), axum::http::StatusCode::OK);

    // A tampered token changes nothing and says nothing: the same 404 an
    // unknown row gets, so the answer never confirms that a subscription
    // existed.
    treff::db::subscriptions::follow(&db, "ada", topic)
        .await
        .expect("re-follow");
    for wrong in [
        format!("/u/{}/AAAAAAAAAAAAAAAAAAAAAA", row.id),
        format!("/u/999999/{token}"),
    ] {
        let refused = app
            .clone()
            .oneshot(request("POST", wrong.clone()))
            .await
            .expect("response");
        assert_eq!(
            refused.status(),
            axum::http::StatusCode::NOT_FOUND,
            "{wrong}"
        );
    }
    assert!(
        treff::db::subscriptions::is_following(&db, "ada", topic)
            .await
            .expect("check"),
        "and nothing was cancelled by a wrong token"
    );
}

/// The second exit, against a server that really answers.
///
/// One request per POST — not one per subscriber: whatever is behind a webhook
/// fans out on its own, and one notification per person would be a stack of
/// identical messages on one telephone.
#[tokio::test]
async fn a_reply_reaches_the_webhook_once() {
    let (_dir, db, _app) = setup_with_db().await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let (read, mut write) = stream.into_split();
        let mut reader = BufReader::new(read);
        let mut received = String::new();
        let mut laenge = 0usize;
        loop {
            let mut zeile = String::new();
            if reader.read_line(&mut zeile).await.expect("line") == 0 {
                break;
            }
            if let Some(n) = zeile.to_ascii_lowercase().strip_prefix("content-length:") {
                laenge = n.trim().parse().unwrap_or(0);
            }
            received.push_str(&zeile);
            if zeile == "\r\n" {
                break;
            }
        }
        let mut body = vec![0u8; laenge];
        use tokio::io::AsyncReadExt as _;
        reader.read_exact(&mut body).await.expect("body");
        received.push_str(&String::from_utf8_lossy(&body));
        write
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
            .await
            .expect("answer");
        received
    });

    let ada = treff::authz::Identity {
        subject: "ada".into(),
        name: "Ada".into(),
        groups: vec!["Household".into()],
        email: None,
    };
    let ben = treff::authz::Identity {
        subject: "ben".into(),
        name: "Ben".into(),
        groups: vec!["Household".into()],
        email: None,
    };
    let topic = treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "Who had the projector?",
        "Not in the cupboard.",
        &ada,
    )
    .await
    .expect("topic");
    treff::db::topics::add_reply(&db, topic, "It is at my place.", &ben)
        .await
        .expect("reply");

    let hook = treff::notify::webhook::Hook::new(treff::notify::webhook::Settings {
        url: format!("http://{addr}/"),
        topic: Some("treff".into()),
        token: Some("s3cret".into()),
        timeout: std::time::Duration::from_secs(5),
    })
    .expect("hook");

    let config = std::sync::Arc::new(
        treff::config::Config::parse(common::CONFIGURATION).expect("configuration"),
    );
    let key = std::sync::Arc::new(vec![3u8; 64]);
    let composer = |row: &treff::db::outbox::Owed| {
        let db = db.clone();
        let config = config.clone();
        let key = key.clone();
        let row = row.clone();
        Box::pin(async move { treff::notify::compose(&db, &config, &key, &row).await })
            as treff::notify::BoxFuture<anyhow::Result<Option<treff::notify::Message>>>
    };

    let sent = treff::notify::drain_channel(&db, &hook, composer, 10, "webhook")
        .await
        .expect("drain");
    assert_eq!(sent, 1, "one request per post, however many people follow");

    let received = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("the server answered in time")
        .expect("server task");

    assert!(received.contains("POST / HTTP"), "{received}");
    assert!(
        received.contains("authorization: Bearer s3cret"),
        "the token travels: {received}"
    );
    assert!(received.contains("\"topic\":\"treff\""), "{received}");
    assert!(received.contains("Who had the projector?"), "{received}");
    assert!(received.contains("It is at my place."), "{received}");
    assert!(
        received.contains("https://forum.example.org/t/"),
        "{received}"
    );

    // And the mail rows are untouched: the two channels do not drain each other.
    assert!(
        treff::db::outbox::due(&db, 10)
            .await
            .expect("due")
            .iter()
            .any(|o| o.kanal == "mail"),
        "the mail is still owed"
    );
}
