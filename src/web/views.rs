//! The pages. Server-rendered HTML, no script, no third party.
//!
//! Everything user-supplied reaches a page through one of two doors: a title,
//! which Maud escapes on the way in, or a post body, which has already been
//! through the sanitizer in [`crate::markup`]. There is no third door.

use crate::authz::Identity;
use crate::config::{Category, Space, View};
use crate::db::topics::{CategoryCount, Post, Topic};
use maud::{DOCTYPE, Markup, PreEscaped, html};

/// The stylesheet is served as its own route rather than inlined, so that
/// `style-src 'self'` in the CSP stays true.
pub const STYLESHEET: &str = include_str!("style.css");

pub fn layout(space: &Space, who: &Identity, title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " — " (space.title) }
                link rel="stylesheet" href="/assets/style.css";
            }
            body {
                header {
                    a class="home" href="/" { (space.title) }
                    nav {
                        span class="who" { (who.name) }
                        a href="/auth/logout" { "Sign out" }
                    }
                }
                main { (body) }
            }
        }
    }
}

/// The front page of a space with more than one category: what there is, and
/// what has happened lately. Every configured category is listed, including
/// the empty ones — a category that is hidden until someone writes in it is
/// one nobody ever writes in.
pub fn category_index(
    space: &Space,
    who: &Identity,
    rows: &[(&Category, CategoryCount)],
) -> Markup {
    let body = html! {
        ul class="categories" {
            @for (category, count) in rows {
                li {
                    a class="name" href={ "/c/" (category.slug) } { (category.title) }
                    span class="count" { (count.topics) " topics" }
                    span class="byline" {
                        @match count.last_activity {
                            Some(t) => (crate::web::views::day(t)),
                            None => "no topics yet",
                        }
                    }
                }
            }
        }
    };
    layout(space, who, &space.title, body)
}

/// A timestamp as a plain day. No clock: in a forum for a closed circle the
/// hour is noise, and a date needs no time-zone argument. Hand-rolling this
/// would mean hand-rolling leap years, so it goes through `time`, which is in
/// the tree anyway for the cookies.
pub fn day(unix_seconds: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix_seconds)
        .map(|t| t.date().to_string())
        .unwrap_or_else(|_| String::from("unknown"))
}

/// A space, rendered the way it is configured: a timeline shows the posts
/// themselves, a topic list shows titles. Same data, one line of configuration
/// apart.
pub fn space_page(space: &Space, who: &Identity, topics: &[(Topic, Option<Post>)]) -> Markup {
    let body = html! {
        @if topics.is_empty() {
            p class="empty" { "Nothing here yet." }
        }
        @match space.view {
            View::Timeline => {
                @for (topic, first) in topics {
                    article class="entry" {
                        h2 { a href={ "/t/" (topic.id) } { (topic.title) } }
                        p class="byline" { (topic.author_name) }
                        @if let Some(post) = first {
                            div class="body" { (PreEscaped(crate::markup::render(&post.body_markdown))) }
                        }
                    }
                }
            }
            View::Topics => {
                ul class="topics" {
                    @for (topic, _) in topics {
                        li {
                            a href={ "/t/" (topic.id) } { (topic.title) }
                            span class="byline" { (topic.author_name) }
                        }
                    }
                }
            }
        }
    };
    layout(space, who, &space.title, body)
}

pub fn topic_page(space: &Space, who: &Identity, topic: &Topic, posts: &[Post]) -> Markup {
    let body = html! {
        h1 { (topic.title) }
        @for post in posts {
            article class="post" {
                p class="byline" {
                    (post.author_name)
                    @if post.edited { " (edited)" }
                }
                div class="body" { (PreEscaped(crate::markup::render(&post.body_markdown))) }
            }
        }
    };
    layout(space, who, &topic.title, body)
}
