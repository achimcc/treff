//! The pages. Server-rendered HTML, no script, no third party.
//!
//! Everything user-supplied reaches a page through one of two doors: a title,
//! which Maud escapes on the way in, or a post body, which has already been
//! through the sanitizer in [`crate::markup`]. There is no third door.

use crate::authz::Identity;
use crate::config::{Category, Space, View};
use crate::db::topics::{CategoryCount, LastPost, Post, Topic};
use crate::i18n::Lang;
use maud::{DOCTYPE, Markup, PreEscaped, html};

/// The stylesheet is served as its own route rather than inlined, so that
/// `style-src 'self'` in the CSP stays true.
pub const STYLESHEET: &str = include_str!("style.css");

pub fn layout(space: &Space, who: &Identity, lang: Lang, title: &str, body: Markup) -> Markup {
    layout_with_search(space, who, lang, title, "", body)
}

/// The same layout, with whatever was searched for left standing in the field.
/// A search box that empties itself on the results page is one people retype.
pub fn layout_with_search(
    space: &Space,
    who: &Identity,
    lang: Lang,
    title: &str,
    find: &str,
    body: Markup,
) -> Markup {
    html! {
        (DOCTYPE)
        html lang=(lang.code()) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " — " (space.title) }
                link rel="stylesheet" href="/assets/style.css";
            }
            body {
                header {
                    // The prompt line is shell, not prose: it says who you are
                    // and where you are, in the one notation that needs no
                    // translation. The command follows the view, because a
                    // timeline is read and a topic list is browsed.
                    p class="prompt" {
                        b { (who.name) "@" (space.host) } ":~$ "
                        @match space.view {
                            View::Timeline => "cat *.md",
                            View::Topics => "ls -la",
                        }
                    }
                    div class="headline" {
                        span class="brand" {
                            a class="home" href="/" { (space.title) }
                            span class="cursor" {}
                        }
                        nav {
                            form class="find" method="get" action="/search" {
                                input type="search" name="q" value=(find)
                                      placeholder=(lang.t("search_placeholder"))
                                      aria-label=(lang.t("search_placeholder"));
                            }
                            @if let Some(home) = &space.home {
                                // Beschriftet mit dem Ort, nicht mit einem
                                // Pfeil: Wer hier landet, kommt oft aus einem
                                // Lesezeichen und weiss nicht, wohin „zurueck"
                                // fuehrt. Das `../` davor setzt das Stylesheet.
                                a class="up" href=(home) { (host_of(home)) }
                            }
                            span class="who" { (who.name) }
                            a href="/auth/logout" { (lang.t("sign_out")) }
                        }
                    }
                }
                main { (body) }
                footer {
                    p class="prompt" { "# " (space.host) }
                    p { (lang.t("footer_note")) }
                }
            }
        }
    }
}

/// The host part of a URL, for labelling a link with the place it leads to.
///
/// Deliberately not a URL parser: the value comes from the configuration file
/// of whoever runs the instance, it is only ever printed, and Maud escapes it
/// on the way out. Anything unexpected shows up as itself rather than as a
/// wrong answer.
fn host_of(url: &str) -> &str {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or(url)
}

/// The front page of a space with more than one category: what there is, and
/// what has happened lately. Every configured category is listed, including
/// the empty ones — a category that is hidden until someone writes in it is
/// one nobody ever writes in.
pub fn category_index(
    space: &Space,
    who: &Identity,
    lang: Lang,
    rows: &[(&Category, CategoryCount)],
) -> Markup {
    let body = html! {
        h2 class="section-head" { (lang.t("sections")) }
        ul class="categories" {
            @for (category, count) in rows {
                li {
                    a class="name" href={ "/c/" (category.slug) } { (category.title) }
                    span class="count" {
                        @if count.topics == 1 { (lang.t("topic_one")) }
                        @else { (count.topics) " " (lang.t("topic_many")) }
                    }
                    span class="byline" {
                        // Ein nacktes Datum in einer Liste von Bereichen ist
                        // eine Raetselfrage: Angelegt? Zuletzt gelesen? Das
                        // Wort davor kostet nichts und beantwortet sie.
                        @match count.last_activity {
                            Some(t) => {
                                (lang.t("last_activity")) " "
                                span class="value" { (crate::web::views::day(t)) }
                            },
                            None => (lang.t("no_topics_yet")),
                        }
                    }
                }
            }
        }
    };
    layout(space, who, lang, &space.title, body)
}

/// What a search found. The snippet arrives with `[` and `]` around the
/// match — plain characters, so it goes through Maud's escaping like every
/// other piece of somebody else's text.
pub fn search_page(
    space: &Space,
    who: &Identity,
    lang: Lang,
    find: &str,
    hits: &[crate::db::search::Hit],
) -> Markup {
    let body = html! {
        h2 class="section-head" { (lang.t("search_results")) }
        @if find.trim().is_empty() {
            p class="empty" { (lang.t("search_prompt")) }
        } @else if hits.is_empty() {
            p class="empty" { (lang.t("search_nothing")) }
        } @else {
            ul class="topics" {
                @for hit in hits {
                    li class="hit" {
                        a href={ "/t/" (hit.topic_id) } { (hit.title) }
                        span class="byline" {
                            span class="name" { (hit.category) }
                            span class="sep" { " · " }
                            (day(hit.updated_at))
                        }
                        p class="snippet" { (hit.snippet) }
                    }
                }
            }
        }
    };
    layout_with_search(space, who, lang, lang.t("search_results"), find, body)
}

/// The unsubscribe page. Deliberately plain and deliberately outside the
/// signed-in layout: whoever opens it may not be signed in, and asking them to
/// be would defeat the link.
pub fn unsubscribe_page(space: &Space, lang: Lang, id: i64, token: &str) -> Markup {
    bare(
        space,
        lang,
        lang.t("unsubscribe_title"),
        html! {
            p { (lang.t("unsubscribe_question")) }
            form method="post" action={ "/u/" (id) "/" (token) } {
                button type="submit" { (lang.t("unsubscribe_confirm")) }
            }
        },
    )
}

pub fn unsubscribed_page(space: &Space, lang: Lang) -> Markup {
    bare(
        space,
        lang,
        lang.t("unsubscribed_title"),
        html! { p { (lang.t("unsubscribed_note")) } },
    )
}

/// A page with no header and no navigation, for the two places somebody
/// arrives without a session. Everything the normal layout shows — the name,
/// the sign-out link — would be a lie here.
fn bare(space: &Space, lang: Lang, title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang=(lang.code()) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " — " (space.title) }
                link rel="stylesheet" href="/assets/style.css";
            }
            body {
                main class="bare" {
                    h1 { (title) }
                    (body)
                }
            }
        }
    }
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

/// One line of a space: the topic, whoever wrote in it last, and — in a
/// timeline only — the opening post whose body is shown underneath.
pub struct TopicRow {
    pub topic: Topic,
    pub last: Option<LastPost>,
    pub first: Option<Post>,
}

/// A space, rendered the way it is configured: a timeline shows the posts
/// themselves, a topic list shows titles. Same data, one line of configuration
/// apart.
pub fn space_page(
    space: &Space,
    who: &Identity,
    lang: Lang,
    category: Option<&Category>,
    topics: &[TopicRow],
) -> Markup {
    // Display follows the right: someone who may not open a topic is not shown
    // a button that leads to a refusal.
    let may_post = category.is_some_and(|c| crate::authz::may_post(who, c));
    let body = html! {
        @if topics.is_empty() {
            p class="empty" { (lang.t("nothing_here")) }
        }
        @match space.view {
            View::Timeline => {
                @for TopicRow { topic, first, .. } in topics {
                    article class="entry" {
                        h2 { a href={ "/t/" (topic.id) } { (topic.title) } }
                        // Das Datum steht hier, weil es die ORDNUNG dieser
                        // Ansicht ist: Eine Zeitleiste sortiert danach, und
                        // bei gespiegelten Artikeln entscheidet es sogar, ob
                        // ein Eintrag schon erscheint. Es wegzulassen hiesse,
                        // die einzige sichtbare Begruendung der Reihenfolge zu
                        // verstecken.
                        p class="byline" {
                            span class="name" { (topic.author_name) }
                            span class="sep" { " · " }
                            (day(topic.created_at))
                        }
                        @if let Some(post) = first {
                            div class="body" { (PreEscaped(crate::markup::render(&post.body_markdown))) }
                        }
                    }
                }
            }
            View::Topics => {
                ul class="topics" {
                    @for TopicRow { topic, last, .. } in topics {
                        li {
                            a href={ "/t/" (topic.id) } { (topic.title) }
                            // WHO WROTE LAST, AND WHEN — not the opener next
                            // to the date of somebody else's reply. Those are
                            // two halves of two different events, and the line
                            // said them as one until 2026-09-11.
                            span class="byline" {
                                @match last {
                                    Some(last) => {
                                        span class="name" { (last.author_name) }
                                        span class="sep" { " · " }
                                        (day(last.created_at))
                                    },
                                    // Constructively unreachable — a topic
                                    // always has its opening post. A byline
                                    // that falls back to the topic's own
                                    // beginning is still true; an empty one
                                    // would look like a bug.
                                    None => {
                                        span class="name" { (topic.author_name) }
                                        span class="sep" { " · " }
                                        (day(topic.created_at))
                                    },
                                }
                            }
                        }
                    }
                }
            }
        }
        @if let Some(category) = category {
            @if may_post {
                form class="write" method="post" action={ "/c/" (category.slug) "/new" } {
                    h2 { (lang.t("new_topic")) }
                    label {
                        (lang.t("field_title"))
                        input type="text" name="title" maxlength="200" required;
                    }
                    label {
                        (lang.t("field_text"))
                        textarea name="body" rows="6" required {}
                    }
                    button type="submit" { (lang.t("open_topic")) }
                }
            }
        }
    };
    layout(space, who, lang, &space.title, body)
}

pub fn topic_page(
    space: &Space,
    who: &Identity,
    lang: Lang,
    category: Option<&Category>,
    topic: &Topic,
    posts: &[Post],
    following: bool,
) -> Markup {
    let may_reply = category.is_some_and(|c| crate::authz::may_reply(who, c));
    let body = html! {
        h1 { (topic.title) }
        // A form and not a link: following changes something, and a GET that
        // changes state is one a link preview or a mail client can trigger
        // without anybody clicking.
        form class="follow" method="post"
             action={ "/t/" (topic.id) (if following { "/unfollow" } else { "/follow" }) } {
            button type="submit" {
                (lang.t(if following { "unfollow" } else { "follow" }))
            }
            @if following {
                span class="note" { (lang.t("following_note")) }
            }
        }
        @for post in posts {
            article class="post" {
                p class="byline" {
                    span class="name" { (post.author_name) }
                    span class="sep" { " · " }
                    (day(post.created_at))
                    @if post.edited { " (" (lang.t("edited")) ")" }
                }
                div class="body" { (PreEscaped(crate::markup::render(&post.body_markdown))) }
                // Only on your own — and the display is not the defence: the
                // query refuses the same thing again, on its own.
                @if crate::authz::may_modify(who, &post.author_subject) {
                    div class="own" {
                        form method="post" action={ "/p/" (post.id) "/edit" } {
                            textarea name="body" rows="4" required { (post.body_markdown) }
                            button type="submit" { (lang.t("save")) }
                        }
                        form method="post" action={ "/p/" (post.id) "/delete" } {
                            button type="submit" class="danger" { (lang.t("delete")) }
                        }
                    }
                }
            }
        }
        @if may_reply {
            form class="write" method="post" action={ "/t/" (topic.id) "/reply" } {
                label {
                    (lang.t("reply"))
                    textarea name="body" rows="5" required {}
                }
                button type="submit" { (lang.t("post_reply")) }
            }
            form class="write" method="post" enctype="multipart/form-data"
                 action={ "/t/" (topic.id) "/attach" } {
                label {
                    (lang.t("attach_picture"))
                    input type="file" name="file" accept="image/jpeg,image/png,image/gif,image/webp" required;
                }
                label {
                    (lang.t("caption"))
                    input type="text" name="body";
                }
                button type="submit" { (lang.t("attach")) }
            }
        }
    };
    layout(space, who, lang, &topic.title, body)
}
