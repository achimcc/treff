//! `@handle`, asked of the ROUTER: who is told, who is not, and what the page
//! shows.
//!
//! Finding a mention in Markdown is unit-tested in `markup`. This file is for
//! the decisions around it, and the one that matters most is the refusal: a
//! mention of somebody who may not read the space must leave no trace — no
//! entry, no highlight, nothing that says the person exists.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

mod common;
use common::{body_of, setup_with_db, signed_in};

const FORUM: &str = "forum.example.org";

fn get(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("host", FORUM)
        .header("cookie", cookie)
        .header("accept-language", "de")
        .body(Body::empty())
        .expect("request")
}

fn post(uri: &str, cookie: &str, form: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", FORUM)
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(form.to_string()))
        .expect("request")
}

async fn send(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(request).await.expect("response")
}

/// Opens a topic through the route and returns its id.
async fn open(app: &axum::Router, cookie: &str, body: &str) -> i64 {
    let form = format!("title=T&body={}", urlencode(body));
    let response = send(app, post("/c/general/new", cookie, &form)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()["location"].to_str().expect("ascii");
    location
        .trim_start_matches("/t/")
        .parse()
        .expect("a topic id")
}

async fn reply(app: &axum::Router, cookie: &str, topic: i64, body: &str) {
    let form = format!("body={}", urlencode(body));
    let response = send(app, post(&format!("/t/{topic}/reply"), cookie, &form)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn entries(db: &treff::db::Db, subject: &str) -> Vec<treff::db::inbox::Entry> {
    treff::db::inbox::entries(db, subject, FORUM, 50)
        .await
        .expect("entries")
}

fn mentions(list: &[treff::db::inbox::Entry]) -> usize {
    list.iter()
        .filter(|e| matches!(e, treff::db::inbox::Entry::Mention { .. }))
        .count()
}

async fn last_post(db: &treff::db::Db) -> i64 {
    sqlx::query_scalar("SELECT max(id) FROM posts")
        .fetch_one(db.pool())
        .await
        .expect("a post")
}

#[tokio::test]
async fn a_mention_reaches_somebody_who_may_read() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let ben = signed_in(&db, dir.path(), "ben", &["Friends"]).await;
    let cem = signed_in(&db, dir.path(), "cem", &["Household"]).await;

    let t = open(&app, &cem, "Holiday plans").await;
    reply(&app, &ada, t, "@Ben what do you think?").await;

    let list = entries(&db, "ben").await;
    assert_eq!(mentions(&list), 1, "{list:?}");
    let page = body_of(send(&app, get("/notifications", &ben)).await).await;
    // In the list, not anywhere on the page: the bell carries the same words
    // as an attribute for bell.js, and a search over the whole page would be
    // green without any mention at all.
    let list = page.split("<main").nth(1).expect("a main element");
    assert!(list.contains("hat dich erwaehnt in"), "{list}");
}

#[tokio::test]
async fn opening_a_topic_with_a_mention_tells_them_too() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    signed_in(&db, dir.path(), "ben", &["Household"]).await;
    open(&app, &ada, "@ben, film night?").await;
    assert_eq!(mentions(&entries(&db, "ben").await), 1);
}

/// THE REFUSAL. Eve has an account and a handle, and may not read the forum.
/// Mentioning her must change nothing she could ever see, and the page must
/// render her handle exactly like one that belongs to nobody.
#[tokio::test]
async fn a_mention_of_somebody_who_may_not_read_leaves_no_trace() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    signed_in(&db, dir.path(), "eve", &["Neighbours"]).await;

    let t = open(&app, &ada, "opening").await;
    reply(&app, &ada, t, "hello @eve").await;
    reply(&app, &ada, t, "hello @nobody").await;

    assert!(entries(&db, "eve").await.is_empty());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM inbox WHERE subject = 'eve'")
        .fetch_one(db.pool())
        .await
        .expect("count");
    assert_eq!(rows, 0);

    let page = body_of(send(&app, get(&format!("/t/{t}"), &ada)).await).await;
    assert!(!page.contains("class=\"mention\""), "{page}");
    assert!(page.contains("hello @eve"), "{page}");
    assert!(page.contains("hello @nobody"), "{page}");
}

#[tokio::test]
async fn mentioning_yourself_tells_nobody() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    open(&app, &ada, "note to @ada").await;
    assert!(entries(&db, "ada").await.is_empty());
}

/// A follower who is also mentioned has ONE entry, and it is the mention.
#[tokio::test]
async fn a_follower_who_is_mentioned_has_one_entry_not_two() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let t = open(&app, &ben, "ben's topic, so ben follows").await;
    reply(&app, &ada, t, "@ben here you go").await;

    let list = entries(&db, "ben").await;
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(mentions(&list), 1, "{list:?}");
}

#[tokio::test]
async fn an_edit_that_adds_a_mention_tells_them_once() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let t = open(&app, &ada, "opening").await;
    reply(&app, &ada, t, "a thought").await;
    let p = last_post(&db).await;
    assert!(entries(&db, "ben").await.is_empty());

    for body in ["a thought, @ben", "a thought, @ben!"] {
        let form = format!("body={}", urlencode(body));
        let response = send(&app, post(&format!("/p/{p}/edit"), &ada, &form)).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }
    assert_eq!(mentions(&entries(&db, "ben").await), 1);
}

#[tokio::test]
async fn the_page_marks_the_mention_and_names_the_handles() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let t = open(&app, &ada, "ask @ben").await;

    let page = body_of(send(&app, get(&format!("/t/{t}"), &ada)).await).await;
    assert!(
        page.contains(r#"<span class="mention">@ben</span>"#),
        "{page}"
    );
    // The author's own handle next to the name, so it can be copied.
    assert!(
        page.contains(r#"<span class="handle">@ada</span>"#),
        "{page}"
    );
}

// --- The mail for a mention (task 5) ---------------------------------------

/// An account with an address and the handle `<subject>`, the way a sign-in
/// leaves it.
async fn account(db: &treff::db::Db, subject: &str, groups: &[&str]) {
    let who = treff::authz::Identity {
        subject: subject.into(),
        name: format!("{subject} the tester"),
        groups: groups.iter().map(|g| (*g).to_string()).collect(),
        email: Some(format!("{subject}@example.org")),
        handle: Some(subject.into()),
    };
    treff::auth::Sessions::create(db, &who)
        .await
        .expect("account");
}

async fn owed_mail(db: &treff::db::Db) -> Vec<treff::db::outbox::Owed> {
    treff::db::outbox::due(db, 100)
        .await
        .expect("due")
        .into_iter()
        .filter(|o| o.kanal == "mail")
        .collect()
}

async fn compose(
    db: &treff::db::Db,
    row: &treff::db::outbox::Owed,
) -> Option<treff::notify::Message> {
    let config = treff::config::Config::parse(common::CONFIGURATION).expect("configuration");
    treff::notify::compose(db, &config, &[3u8; 64], row)
        .await
        .expect("compose")
}

#[tokio::test]
async fn a_mention_is_mailed_and_says_so() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    account(&db, "ben", &["Household"]).await;
    let t = open(&app, &ada, "@ben come and look").await;

    let owed = owed_mail(&db).await;
    assert_eq!(owed.len(), 1, "{owed:?}");
    assert_eq!(owed[0].subject, "ben");
    assert_eq!(owed[0].reason, "mention");

    let message = compose(&db, &owed[0]).await.expect("a message");
    assert_eq!(message.to, "ben@example.org");
    assert!(message.mention, "worded as a mention");
    assert_eq!(message.link, format!("https://{FORUM}/t/{t}"));
}

/// A follower who is mentioned gets ONE mail, and it is the mention.
#[tokio::test]
async fn a_follower_who_is_mentioned_gets_one_mail() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    account(&db, "ben", &["Household"]).await;
    let t = treff::db::topics::create_topic(
        &db,
        FORUM,
        "general",
        "ben's",
        "so ben follows",
        &treff::authz::Identity {
            subject: "ben".into(),
            name: "ben".into(),
            groups: vec!["Household".into()],
            email: None,
            handle: Some("ben".into()),
        },
    )
    .await
    .expect("topic");
    reply(&app, &ada, t, "@ben there").await;

    let for_ben: Vec<_> = owed_mail(&db)
        .await
        .into_iter()
        .filter(|o| o.subject == "ben")
        .collect();
    assert_eq!(for_ben.len(), 1, "{for_ben:?}");
    assert_eq!(for_ben[0].reason, "mention");
}

/// CHECKED AGAIN WHEN THE MAIL IS WRITTEN. Somebody who lost the group
/// between the post and the send is told nothing — not the title, not the
/// text.
#[tokio::test]
async fn a_mention_mail_is_not_sent_to_somebody_who_lost_the_group() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    account(&db, "ben", &["Household"]).await;
    open(&app, &ada, "@ben secret plans").await;
    let owed = owed_mail(&db).await;
    assert_eq!(owed.len(), 1);

    sqlx::query("UPDATE accounts SET groups_json = '[\"Neighbours\"]' WHERE subject = 'ben'")
        .execute(db.pool())
        .await
        .expect("lose the group");
    assert!(compose(&db, &owed[0]).await.is_none());
}

/// The reply mail is what it was: it still has no business checking a
/// mention's rule, and a follower still gets it.
#[tokio::test]
async fn a_reply_mail_is_still_a_reply_mail() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    account(&db, "ben", &["Household"]).await;
    let t = open(&app, &ada, "opening").await;
    let cem = signed_in(&db, dir.path(), "cem", &["Household"]).await;
    reply(&app, &cem, t, "no mention here").await;

    let for_ada: Vec<_> = owed_mail(&db)
        .await
        .into_iter()
        .filter(|o| o.subject == "ada")
        .collect();
    assert_eq!(for_ada.len(), 1);
    assert_eq!(for_ada[0].reason, "reply");
}

/// THE WAY OUT OF A MENTION MAIL turns off mention mails — the reply mail's
/// link would unfollow a topic the person never followed. Without a session,
/// twice, and afterwards the bell still rings but no mail is queued.
#[tokio::test]
async fn the_link_in_a_mention_mail_turns_mention_mails_off() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    account(&db, "ben", &["Household"]).await;
    let t = open(&app, &ada, "@ben one").await;
    let row = owed_mail(&db).await.remove(0);
    let message = compose(&db, &row).await.expect("message");
    let link = message.unsubscribe.expect("a way out travels with it");
    assert!(link.contains(&format!("/u/{}/", row.id)), "{link}");
    // The composer above signed with a test key; the router has its own, in
    // the state directory, and the token is made with that one.
    let key = treff::notify::unsubscribe::load_or_create_key(dir.path()).expect("key");
    let path = format!(
        "/u/{}/{}",
        row.id,
        treff::notify::unsubscribe::token(&key, row.id)
    );

    let bare = |method: &str| {
        Request::builder()
            .method(method)
            .uri(&path)
            .header("host", FORUM)
            .header("accept-language", "de")
            .body(Body::empty())
            .expect("request")
    };
    let question = body_of(send(&app, bare("GET")).await).await;
    assert!(
        question.contains("mit @ erwaehnt"),
        "the page says which kind: {question}"
    );
    for _ in 0..2 {
        let done = send(&app, bare("POST")).await;
        assert_eq!(done.status(), StatusCode::OK);
    }
    let on: i64 = sqlx::query_scalar("SELECT mention_mail FROM accounts WHERE subject = 'ben'")
        .fetch_one(db.pool())
        .await
        .expect("flag");
    assert_eq!(on, 0);

    reply(&app, &ada, t, "@ben two").await;
    assert!(
        owed_mail(&db).await.iter().all(|o| o.id == row.id),
        "no new mail for ben"
    );
    assert_eq!(
        mentions(&entries(&db, "ben").await),
        2,
        "the bell still rings"
    );
}

#[tokio::test]
async fn mention_mails_can_be_switched_back_on() {
    let (dir, db, app) = setup_with_db().await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let set = |on: &str| post("/notifications/mention-mail", &ben, &format!("on={on}"));
    let flag = || async {
        sqlx::query_scalar::<_, i64>("SELECT mention_mail FROM accounts WHERE subject = 'ben'")
            .fetch_one(db.pool())
            .await
            .expect("flag")
    };

    let page = body_of(send(&app, get("/notifications", &ben)).await).await;
    assert!(
        page.contains("action=\"/notifications/mention-mail\""),
        "{page}"
    );

    assert_eq!(send(&app, set("0")).await.status(), StatusCode::SEE_OTHER);
    assert_eq!(flag().await, 0);
    assert_eq!(send(&app, set("1")).await.status(), StatusCode::SEE_OTHER);
    assert_eq!(flag().await, 1);
}

// --- Who may be offered (stage 4, task 1) -----------------------------------

async fn offered(app: &axum::Router, host: &str, cookie: &str) -> axum::response::Response {
    let request = Request::builder()
        .uri("/mentionable")
        .header("host", host)
        .header("cookie", cookie)
        .body(Body::empty())
        .expect("request");
    send(app, request).await
}

/// THE LIST IS THE MENTION'S RULE, NOT THE ACCOUNT TABLE. Showing everybody
/// with an account would say exactly what a mention is careful not to: who
/// exists. So: readers of this space, with a handle — and only name and
/// handle.
#[tokio::test]
async fn the_suggestions_are_the_readers_of_the_space_and_nobody_else() {
    let (dir, db, app) = setup_with_db().await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    signed_in(&db, dir.path(), "Ben", &["Friends"]).await;
    signed_in(&db, dir.path(), "eve", &["Neighbours"]).await;
    // An account without a handle cannot be mentioned, so it is not offered.
    treff::auth::Sessions::create(
        &db,
        &treff::authz::Identity {
            subject: "nohandle".into(),
            name: "No Handle".into(),
            groups: vec!["Household".into()],
            email: Some("secret@example.org".into()),
            handle: None,
        },
    )
    .await
    .expect("account");

    let response = offered(&app, FORUM, &ada).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"].to_str().expect("ascii"),
        "application/json"
    );
    assert_eq!(
        response.headers()["cache-control"].to_str().expect("ascii"),
        "no-store"
    );
    let body = body_of(response).await;
    let list: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(
        list,
        serde_json::json!([
            { "name": "Ben the tester", "handle": "ben" },
        ]),
        "eve and the account without a handle are absent — and so is ada, who asks: \
         mentioning yourself does nothing, and the list offered her only herself \
         on the evening mentions went live"
    );
    assert!(!body.contains("example.org"), "no address: {body}");
}

#[tokio::test]
async fn somebody_who_may_not_read_the_space_gets_no_list() {
    let (dir, db, app) = setup_with_db().await;
    let eve = signed_in(&db, dir.path(), "eve", &["Neighbours"]).await;
    assert_eq!(
        offered(&app, FORUM, &eve).await.status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn without_a_session_there_is_no_list() {
    let (_dir, _db, app) = setup_with_db().await;
    let response = offered(&app, FORUM, "").await;
    assert_ne!(response.status(), StatusCode::OK);
}
