//! Likes at the route: the heart under a post, `POST /p/{id}/like`, and what
//! the page shows to the liker, the author and everybody else (stage 7,
//! ADR 0008).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

mod common;
use common::{body_of, setup_with_db, signed_in};

const FORUM: &str = "forum.example.org";
const BLOG: &str = "blog.example.org";

fn get(host: &str, uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("host", host)
        .header("cookie", cookie)
        .body(Body::empty())
        .expect("request")
}

fn post(host: &str, uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", host)
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::empty())
        .expect("request")
}

fn post_json(host: &str, uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", host)
        .header("cookie", cookie)
        .header("accept", "application/json")
        .body(Body::empty())
        .expect("request")
}

fn person(subject: &str) -> treff::authz::Identity {
    treff::authz::Identity {
        subject: subject.into(),
        name: format!("{subject} the tester"),
        groups: vec!["Household".into()],
        email: None,
        handle: None,
    }
}

async fn page(app: &axum::Router, host: &str, uri: &str, cookie: &str) -> String {
    let response = app
        .clone()
        .oneshot(get(host, uri, cookie))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    body_of(response).await
}

/// A topic by `by`, and the id of its opening post.
async fn topic(db: &treff::db::Db, space: &str, by: &str) -> (i64, i64) {
    let t =
        treff::db::topics::create_topic(db, space, "general", "Projector", "Where?", &person(by))
            .await
            .expect("topic");
    let (_, posts) = treff::db::topics::load_topic(db, space, t)
        .await
        .expect("load")
        .expect("there");
    (t, posts[0].id)
}

/// The like control of one post, as the page carries it.
fn like_form(html: &str, post: i64) -> Option<&str> {
    let start = html.find(&format!(r#"action="/p/{post}/like""#))?;
    let end = html[start..].find("</form>")? + start;
    Some(&html[start..end])
}

#[tokio::test]
async fn a_like_shows_under_the_post_and_a_second_click_takes_it_back() {
    let (dir, db, app) = setup_with_db().await;
    let (t, p) = topic(&db, FORUM, "ada").await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;

    let before = page(&app, FORUM, &format!("/t/{t}"), &ben).await;
    let form = like_form(&before, p).expect("a heart on somebody else's post");
    assert!(form.contains(r#"aria-pressed="false""#), "{form}");
    assert!(form.contains(r#"aria-label="Like""#), "{form}");
    assert!(
        !form.contains(r#"class="count""#),
        "no number at zero: {form}"
    );

    let response = app
        .clone()
        .oneshot(post(FORUM, &format!("/p/{p}/like"), &ben))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()["location"],
        format!("/t/{t}#p{p}"),
        "back to the post, not to the top of the thread"
    );

    let after = page(&app, FORUM, &format!("/t/{t}"), &ben).await;
    let form = like_form(&after, p).expect("form");
    assert!(form.contains(r#"aria-pressed="true""#), "{form}");
    assert!(form.contains(r#"class="count">1<"#), "{form}");
    // A screen reader hears the number too: `aria-label` replaces the
    // button's content, so the count has to be in it.
    assert!(form.contains(r#"aria-label="Unlike, 1 like""#), "{form}");
    assert!(
        form.contains("ben the tester"),
        "the name in the title: {form}"
    );

    app.clone()
        .oneshot(post(FORUM, &format!("/p/{p}/like"), &ben))
        .await
        .expect("response");
    let back = page(&app, FORUM, &format!("/t/{t}"), &ben).await;
    let form = like_form(&back, p).expect("form");
    assert!(form.contains(r#"aria-pressed="false""#), "{form}");
    assert!(!form.contains(r#"class="count""#), "{form}");
}

/// What `like.js` asks for: the number without a page.
#[tokio::test]
async fn with_accept_json_the_answer_is_the_number() {
    let (dir, db, app) = setup_with_db().await;
    let (_, p) = topic(&db, FORUM, "ada").await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let response = app
        .clone()
        .oneshot(post_json(FORUM, &format!("/p/{p}/like"), &ben))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = body_of(response).await;
    let json: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(json["liked"], true);
    assert_eq!(json["count"], 1);
}

#[tokio::test]
async fn your_own_post_has_no_button_and_the_route_refuses() {
    let (dir, db, app) = setup_with_db().await;
    let (t, p) = topic(&db, FORUM, "ada").await;
    let ada = signed_in(&db, dir.path(), "ada", &["Household"]).await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    app.clone()
        .oneshot(post(FORUM, &format!("/p/{p}/like"), &ben))
        .await
        .expect("response");

    let html = page(&app, FORUM, &format!("/t/{t}"), &ada).await;
    assert!(like_form(&html, p).is_none(), "no form on your own: {html}");
    assert!(
        html.contains(r#"class="like own""#) && html.contains(r#"class="count">1<"#),
        "the number stands, without a button: {html}"
    );

    let response = app
        .clone()
        .oneshot(post(FORUM, &format!("/p/{p}/like"), &ada))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_post_of_the_other_space_is_not_there() {
    let (dir, db, app) = setup_with_db().await;
    let (_, p) = topic(&db, FORUM, "ada").await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let response = app
        .clone()
        .oneshot(post(BLOG, &format!("/p/{p}/like"), &ben))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = app
        .clone()
        .oneshot(post(FORUM, "/p/999/like", &ben))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// The blog's opening posts are articles; their heart stands under the
/// entry on the timeline too, with the number.
#[tokio::test]
async fn the_timeline_carries_the_heart_of_each_entry() {
    let (dir, db, app) = setup_with_db().await;
    let t = treff::db::topics::create_topic(&db, BLOG, "notes", "News", "text", &person("ada"))
        .await
        .expect("topic");
    let (_, posts) = treff::db::topics::load_topic(&db, BLOG, t)
        .await
        .expect("load")
        .expect("there");
    let p = posts[0].id;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    app.clone()
        .oneshot(post(BLOG, &format!("/p/{p}/like"), &ben))
        .await
        .expect("response");
    let html = page(&app, BLOG, "/", &ben).await;
    let form = like_form(&html, p).expect("a heart on the timeline");
    assert!(form.contains(r#"aria-pressed="true""#), "{form}");
    assert!(form.contains(r#"class="count">1<"#), "{form}");
}

/// Reading is the right that is needed (ADR 0008): somebody who may read
/// but not reply still gets the button.
#[tokio::test]
async fn a_reader_who_may_not_reply_may_still_like() {
    let (dir, db, _) = setup_with_db().await;
    let config = r#"
[[space]]
host  = "forum.example.org"
title = "Forum"
view  = "topics"
read  = ["Household", "Guests"]

  [[space.category]]
  slug  = "general"
  title = "General"
  post  = ["Household"]
  reply = ["Household"]
"#;
    let state = treff::web::AppState::new(
        treff::config::Config::parse(config).expect("configuration"),
        db.clone(),
        treff::auth::OidcSettings {
            issuer: "http://127.0.0.1:1/".into(),
            client_id: "t".into(),
            client_secret: "t".into(),
            group_claim: "groups".into(),
        },
        dir.path(),
    )
    .expect("state");
    let app = treff::web::router(state);
    let (t, p) = topic(&db, FORUM, "ada").await;
    let guest = signed_in(&db, dir.path(), "gus", &["Guests"]).await;
    let html = page(&app, FORUM, &format!("/t/{t}"), &guest).await;
    assert!(like_form(&html, p).is_some(), "{html}");
    let response = app
        .clone()
        .oneshot(post(FORUM, &format!("/p/{p}/like"), &guest))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn the_page_loads_the_script_from_its_own_route() {
    let (dir, db, app) = setup_with_db().await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let html = page(&app, FORUM, "/", &ben).await;
    assert!(
        html.contains(r#"<script src="/assets/like.js" defer></script>"#),
        "{html}"
    );
    let response = app
        .clone()
        .oneshot(get(FORUM, "/assets/like.js", &ben))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .expect("type")
            .starts_with("text/javascript"),
        "{:?}",
        response.headers()
    );
}

/// The state of one heart, read-only — what `like.js` asks for when the
/// server said yes but the answer did not arrive whole. Never a toggle.
#[tokio::test]
async fn the_state_of_a_heart_can_be_read_without_changing_it() {
    let (dir, db, app) = setup_with_db().await;
    let (_, p) = topic(&db, FORUM, "ada").await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    treff::db::likes::toggle(&db, FORUM, p, &person("cem"))
        .await
        .expect("like");
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(get(FORUM, &format!("/p/{p}/like"), &ben))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let json: serde_json::Value = serde_json::from_str(&body_of(response).await).expect("json");
        assert_eq!(json["liked"], false);
        assert_eq!(json["count"], 1, "reading changes nothing");
    }
    let response = app
        .clone()
        .oneshot(get(BLOG, &format!("/p/{p}/like"), &ben))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// --- Articles: the owner behind "treff" ------------------------------------
//
// A mirrored article is stored under a name nobody signs in as. `articles_owner`
// names the person who really writes them, so a like on an article reaches
// somebody's bell.

const OWNED_BLOG: &str = r#"
[[space]]
host           = "blog.example.org"
title          = "Notes"
view           = "timeline"
read           = ["Household"]
articles       = "ARTICLES"
articles_owner = "OWNER"

  [[space.category]]
  slug  = "notes"
  title = "Notes"
  post  = []
  reply = ["Household"]
"#;

/// A blog fed by one article, owned by `owner` — and the article's opening
/// post.
async fn blog_owned_by(owner: &str) -> (tempfile::TempDir, treff::db::Db, axum::Router, i64, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let articles = dir.path().join("articles");
    std::fs::create_dir(&articles).expect("mkdir");
    std::fs::write(
        articles.join("2026-09-01-audiobooks.md"),
        "---\ntitle: Audiobooks are here\n---\n\nThere is a new service.\n",
    )
    .expect("write");
    let db = treff::db::Db::open(&dir.path().join("t.db"))
        .await
        .expect("open");
    treff::articles::mirror(&db, BLOG, "notes", &articles, "title", "2026-09-23")
        .await
        .expect("mirror");
    let config = OWNED_BLOG
        .replace("ARTICLES", articles.to_str().expect("utf-8"))
        .replace("OWNER", owner);
    let state = treff::web::AppState::new(
        treff::config::Config::parse(&config).expect("configuration"),
        db.clone(),
        treff::auth::OidcSettings {
            issuer: "http://127.0.0.1:1/".into(),
            client_id: "t".into(),
            client_secret: "t".into(),
            group_claim: "groups".into(),
        },
        dir.path(),
    )
    .expect("state");
    let app = treff::web::router(state);
    let t: i64 = sqlx::query_scalar("SELECT id FROM topics WHERE source_key IS NOT NULL")
        .fetch_one(db.pool())
        .await
        .expect("the article");
    let (_, posts) = treff::db::topics::load_topic(&db, BLOG, t)
        .await
        .expect("load")
        .expect("there");
    (dir, db, app, t, posts[0].id)
}

fn badge(html: &str) -> Option<String> {
    let at = html.find("class=\"unread\">")? + "class=\"unread\">".len();
    Some(html[at..].split('<').next()?.to_string())
}

#[tokio::test]
async fn a_like_on_an_article_rings_the_owners_bell() {
    let (dir, db, app, _, p) = blog_owned_by("achim").await;
    let achim = signed_in(&db, dir.path(), "achim", &["Household"]).await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let response = app
        .clone()
        .oneshot(post(BLOG, &format!("/p/{p}/like"), &ben))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let html = page(&app, BLOG, "/notifications", &achim).await;
    assert_eq!(badge(&html).as_deref(), Some("1"), "{html}");
    assert!(
        html.contains(r#"<span class="name">ben the tester</span> likes your post in <b>Audiobooks are here</b>"#),
        "{html}"
    );
    assert_eq!(
        badge(&page(&app, BLOG, "/", &ben).await),
        None,
        "the liker hears nothing"
    );
    let stray: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inbox WHERE subject = 'treff:article'")
            .fetch_one(db.pool())
            .await
            .expect("count");
    assert_eq!(stray, 0, "nothing is written for the name on the file");
}

/// The article is the owner's in every sense but the name on it.
#[tokio::test]
async fn the_owner_cannot_like_their_own_article() {
    let (dir, db, app, _, p) = blog_owned_by("achim").await;
    let achim = signed_in(&db, dir.path(), "achim", &["Household"]).await;
    let response = app
        .clone()
        .oneshot(post(BLOG, &format!("/p/{p}/like"), &achim))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let html = page(&app, BLOG, "/", &achim).await;
    assert!(
        like_form(&html, p).is_none(),
        "no button on your own article: {html}"
    );
}

/// A comment under an article belongs to whoever wrote it; the owner is not
/// told about likes on other people's words.
#[tokio::test]
async fn a_like_on_a_comment_reaches_its_author_not_the_owner() {
    let (dir, db, app, t, _) = blog_owned_by("achim").await;
    let achim = signed_in(&db, dir.path(), "achim", &["Household"]).await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let cem = signed_in(&db, dir.path(), "cem", &["Household"]).await;
    let c = treff::db::topics::add_reply(&db, t, "nice", &person("ben"))
        .await
        .expect("comment");
    app.clone()
        .oneshot(post(BLOG, &format!("/p/{c}/like"), &cem))
        .await
        .expect("response");
    assert_eq!(
        badge(&page(&app, BLOG, "/", &ben).await).as_deref(),
        Some("1")
    );
    // achim follows the article? No — nobody follows a mirrored article by
    // writing it, so the only thing that could ring is the like, and it
    // must not.
    let html = page(&app, BLOG, "/notifications", &achim).await;
    // The list only: the bell in the header carries the words as data
    // attributes for bell.js on every page.
    let list = html.split("<main").nth(1).expect("a main element");
    assert!(!list.contains("likes your post"), "{list}");
}

/// A handle that matches nobody: the like counts, nobody is told — no
/// guessing, no entry for a name that is not a person here.
#[tokio::test]
async fn an_owner_nobody_answers_to_is_told_nothing() {
    let (dir, db, app, _, p) = blog_owned_by("ghost").await;
    let ben = signed_in(&db, dir.path(), "ben", &["Household"]).await;
    let response = app
        .clone()
        .oneshot(post_json(BLOG, &format!("/p/{p}/like"), &ben))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_of(response).await).expect("json");
    assert_eq!(json["count"], 1);
    let entries: i64 = sqlx::query_scalar("SELECT count(*) FROM inbox")
        .fetch_one(db.pool())
        .await
        .expect("count");
    assert_eq!(entries, 0);
}
