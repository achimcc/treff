//! Renders the real pages to files, so the design can be LOOKED AT.
//!
//! Not part of the suite — `#[ignore]`, run it deliberately:
//!
//! ```text
//! cargo test --test preview -- --ignored
//! ```
//!
//! WHY IT EXISTS. On 2026-09-06 this file found, in one pass, four things no
//! assertion would have: the header sat 130px left of the text under it, a
//! two-word button stretched across the whole column, a `>` marker leaked onto
//! a date through a loose selector, and a bare date in a list of sections
//! answered no question anybody had asked. A stylesheet is the one part of a
//! program whose failures are invisible to `cargo test`.
//!
//! The pages come out of the ROUTER, not out of the templates: what lands in
//! the files is exactly what a browser would be sent. `PREVIEW_DIR` says where
//! (default `target/preview`); open the HTML, or point a headless browser at
//! it.
mod common;

use axum::body::Body;
use axum::http::Request;
use common::{setup_with_db, signed_in};
use tower::ServiceExt;

async fn body_of(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf-8")
}

fn get(host: &str, uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("host", host)
        .header("cookie", cookie)
        .body(Body::empty())
        .expect("request")
}

#[tokio::test]
#[ignore = "writes files for a human to look at; run with --ignored"]
async fn render_the_pages_for_a_look() {
    let (dir, db, app) = setup_with_db().await;
    let ada = treff::authz::Identity {
        subject: "s1".into(),
        name: "ada".into(),
        groups: vec!["Household".into()],
        email: None,
    };
    let ben = treff::authz::Identity {
        subject: "s2".into(),
        name: "ben".into(),
        groups: vec!["Household".into()],
        email: None,
    };

    let t1 = treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "Who had the projector last?",
        "It is not in the cupboard any more. Was there not a box?\n\n\
         - looked in the living room\n- looked in the cellar\n\nAny idea?",
        &ada,
    )
    .await
    .expect("topic");
    treff::db::topics::add_reply(
        &db,
        t1,
        "It is at my place. Bringing it back on Friday — \
        sorry, I thought that was agreed.",
        &ben,
    )
    .await
    .expect("reply");
    treff::db::topics::add_reply(
        &db,
        t1,
        "All good. On Friday I will put what else we need \
        into `wishes`:\n\n```\nHDMI cable, 5m\nA tripod\n```\n\n> And popcorn.",
        &ada,
    )
    .await
    .expect("reply2");
    treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "general",
        "Film night on Saturday",
        "Who is coming?",
        &ben,
    )
    .await
    .expect("t2");
    treff::db::topics::create_topic(
        &db,
        "forum.example.org",
        "offtopic",
        "Descaled the coffee machine",
        "It sounds like it used to again.",
        &ada,
    )
    .await
    .expect("t3");

    treff::db::topics::create_topic(
        &db,
        "blog.example.org",
        "notes",
        "The server has a forum now",
        "There is a place to talk now — **Treff**. Four sections, and anyone with an \
         account here can open a topic.\n\nThe entries are the same texts that go out \
         as Monday's mail. Whoever has something to say writes it underneath.",
        &ada,
    )
    .await
    .expect("b1");
    treff::db::topics::create_topic(
        &db,
        "blog.example.org",
        "notes",
        "Subtitles arrive on their own now",
        "Jellyfin fetches them when a film turns up without any.",
        &ben,
    )
    .await
    .expect("b2");

    let cookie = signed_in(&db, dir.path(), "s1", &["Household"]).await;
    let out = std::env::var("PREVIEW_DIR").unwrap_or_else(|_| "target/preview".into());
    std::fs::create_dir_all(&out).expect("dir");
    for (name, host, uri) in [
        ("forum", "forum.example.org", "/"),
        ("blog", "blog.example.org", "/"),
        ("thema", "forum.example.org", "/t/1"),
        ("kategorie", "forum.example.org", "/c/general"),
        // The two pages the topic list shares its markup with, so that a
        // change to one is seen on all three: the question before deleting
        // (post 1 belongs to the reader signed in here), and a search hit,
        // which is a list row with a third line under it.
        ("loeschen", "forum.example.org", "/p/1/delete"),
        ("suche", "forum.example.org", "/search?q=projector"),
    ] {
        let html = body_of(
            app.clone()
                .oneshot(get(host, uri, &cookie))
                .await
                .expect("r"),
        )
        .await;
        let html = html.replace("/assets/style.css", "style.css");
        std::fs::write(format!("{out}/{name}.html"), html).expect("write");
    }
    std::fs::write(format!("{out}/style.css"), treff::web::views::STYLESHEET).expect("css");
    eprintln!("treff: wrote the pages to {out}/");
}
