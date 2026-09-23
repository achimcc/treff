//! The pages. Server-rendered HTML, no third party — and one script of our
//! own, `mention.js`, which completes `@handle` and which nothing depends on
//! (ADR 0005): every page works the same without it.
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

/// The scripts, served as their own routes like the stylesheet, so that
/// `script-src 'self'` is the whole of what the CSP allows. Nothing depends on
/// either (ADR 0005).
pub const MENTION_SCRIPT: &str = include_str!("mention.js");
pub const BELL_SCRIPT: &str = include_str!("bell.js");
pub const LIKE_SCRIPT: &str = include_str!("like.js");

/// What the header needs to know about the person looking, beyond who they
/// are: how many things wait for them. Counted by the handler AFTER it has
/// changed anything — a topic page reads its own entries first, so the bell
/// on it does not count the bundle it just cleared.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bell {
    pub unread: i64,
}

pub fn layout(
    space: &Space,
    who: &Identity,
    lang: Lang,
    bell: Bell,
    title: &str,
    body: Markup,
) -> Markup {
    layout_with_search(space, who, lang, bell, title, "", body)
}

/// The same layout, with whatever was searched for left standing in the field.
/// A search box that empties itself on the results page is one people retype.
pub fn layout_with_search(
    space: &Space,
    who: &Identity,
    lang: Lang,
    bell: Bell,
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
                // `defer`, so it runs after the page is there and never holds
                // it up. Only in the signed-in layout: the bare pages have no
                // text field to complete, and no session to ask with.
                script src="/assets/mention.js" defer {}
                // The bell's overlay and its live number (ADR 0005, amended).
                script src="/assets/bell.js" defer {}
                // The heart without a reload (ADR 0008); without it the like
                // is a form and the page comes back at the post.
                script src="/assets/like.js" defer {}
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
                            // A LINK, and with `bell.js` an overlay: the script
                            // opens the list in place and keeps the number
                            // live; without it this is the way to the page.
                            // Everything the script needs is on the element —
                            // where to listen, where to ask, and its words in
                            // this page's language — so the same file can
                            // serve a page that is not treff's (the start
                            // page), and it carries no translation of its own.
                            // The number only when there is one — a bell that
                            // always says 0 is one people stop looking at.
                            a class="bell" href="/notifications"
                              data-stream="/notifications/stream"
                              data-json="/notifications.json"
                              data-t-title=(lang.t("notifications"))
                              data-t-all=(lang.t("all_notifications"))
                              data-t-nothing=(lang.t("nothing_new_short"))
                              data-t-new-reply-in=(lang.t("new_reply_in"))
                              data-t-new-replies-in=(lang.t("new_replies_in"))
                              data-t-reply-in=(lang.t("reply_in"))
                              data-t-replies-in=(lang.t("replies_in"))
                              data-t-latest-from=(lang.t("latest_from"))
                              data-t-mentioned-you-in=(lang.t("mentioned_you_in"))
                              data-t-film-available=(lang.t("film_available"))
                              data-t-film-failed=(lang.t("film_failed"))
                              data-t-likes-your-post-in=(lang.t("likes_your_post_in"))
                              data-t-and=(lang.t("and"))
                              data-t-others-like-your-post-in=(lang.t("others_like_your_post_in"))
                              data-t-unread=(lang.t("unread"))
                              aria-label=(bell_label(lang, bell)) title=(bell_label(lang, bell)) {
                                (icon_bell())
                                @if bell.unread > 0 {
                                    span class="unread" { (bell.unread) }
                                }
                            }
                            span class="who" { (who.name) }
                            // Ein Formular und kein Link: Abmelden AENDERT
                            // etwas, und was ein Link tut, tut auch jeder,
                            // der ihn nur abruft — ein Mailprogramm, das
                            // Vorschauen holt, ein Chat, der eine eingefuegte
                            // Adresse aufloest, ein `<img src>` auf einer
                            // fremden Seite. Derselbe Grund, aus dem
                            // `/t/{id}/follow` seit jeher ein POST ist.
                            form class="leave" method="post" action="/auth/logout" {
                                button type="submit" { (lang.t("sign_out")) }
                            }
                        }
                    }
                }
                // `id="top"`: where the `top` link at the foot of a long
                // thread leads.
                main id="top" { (body) }
                footer {
                    p class="prompt" { "# " (space.host) }
                    p { (lang.t("footer_note")) }
                }
            }
        }
    }
}

/// What the bell says to somebody who cannot see it.
fn bell_label(lang: Lang, bell: Bell) -> String {
    if bell.unread > 0 {
        format!(
            "{}: {} {}",
            lang.t("notifications"),
            bell.unread,
            lang.t("unread")
        )
    } else {
        lang.t("notifications").to_string()
    }
}

/// The bell, drawn like the pencil and the basket: an inline SVG in
/// `currentColor`, because a glyph would be whatever the reader's font makes
/// of it.
fn icon_bell() -> Markup {
    html! {
        svg class="icon" viewBox="0 0 16 16" width="19" height="19"
            aria-hidden="true" focusable="false" {
            path d="M4 11.5V7a4 4 0 0 1 8 0v4.5l1.3 1.3H2.7zM6.6 13.8a1.5 1.5 0 0 0 2.8 0"
                 fill="none" stroke="currentColor" stroke-width="1.3"
                 stroke-linecap="round" stroke-linejoin="round" {}
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
    bell: Bell,
    rows: &[(&Category, CategoryCount)],
    recent: &[TopicRow],
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
                                span class="value" { (crate::web::views::moment(t)) }
                            },
                            None => (lang.t("no_topics_yet")),
                        }
                    }
                }
            }
        }
        // WHERE SOMETHING IS GOING ON. A front page that lists sections and
        // nothing else sends the reader into every one of them to find out
        // that nothing happened.
        @if !recent.is_empty() {
            h2 class="section-head recent-head" { (lang.t("recent")) }
            (threads_table(space, lang, recent, true))
        }
    };
    layout(space, who, lang, bell, &space.title, body)
}

/// What a search found. The snippet arrives with `[` and `]` around the
/// match — plain characters, so it goes through Maud's escaping like every
/// other piece of somebody else's text.
pub fn search_page(
    space: &Space,
    who: &Identity,
    lang: Lang,
    bell: Bell,
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
                            (moment(hit.updated_at))
                        }
                        p class="snippet" { (hit.snippet) }
                    }
                }
            }
        }
    };
    layout_with_search(space, who, lang, bell, lang.t("search_results"), find, body)
}

/// The unsubscribe page. Deliberately plain and deliberately outside the
/// signed-in layout: whoever opens it may not be signed in, and asking them to
/// be would defeat the link.
/// `mention` says which link this is: a reply's (stop this topic) or a
/// mention's (stop mails about mentions). The button does what the question
/// says, and the question says what the button does.
pub fn unsubscribe_page(space: &Space, lang: Lang, id: i64, token: &str, mention: bool) -> Markup {
    bare(
        space,
        lang,
        lang.t("unsubscribe_title"),
        html! {
            p { (lang.t(if mention { "unsubscribe_mentions_question" } else { "unsubscribe_question" })) }
            form method="post" action={ "/u/" (id) "/" (token) } {
                button type="submit" { (lang.t("unsubscribe_confirm")) }
            }
        },
    )
}

pub fn unsubscribed_page(space: &Space, lang: Lang, mention: bool) -> Markup {
    bare(
        space,
        lang,
        lang.t("unsubscribed_title"),
        html! { p { (lang.t(if mention { "unsubscribed_mentions_note" } else { "unsubscribed_note" })) } },
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

/// A timestamp as the day and the hour.
///
/// It used to be the day alone, on the argument that in a forum for a closed
/// circle the hour is noise. It is not: a thread that moved this morning and
/// one that moved a week ago last Tuesday both read as a bare date, and two
/// posts written in the same afternoon lose the order they were written in —
/// which is the one day the order is worth anything. What the bare date really
/// avoided was the time-zone question, and [`crate::clock`] answers that now
/// instead of dodging it.
pub fn moment(unix_seconds: i64) -> String {
    crate::clock::stamp(unix_seconds, crate::clock::zone())
}

/// The day alone, for a topic dated by its day (`Topic::dated_by_day`). An
/// article from a file has no hour; printing midnight for it read as `02:00`
/// on every article until 0.3.8, and an hour nobody chose is worse than none.
pub fn opened(topic: &Topic) -> String {
    if topic.dated_by_day {
        crate::clock::day(topic.created_at, crate::clock::zone())
    } else {
        moment(topic.created_at)
    }
}

/// One line of a space: the topic, whoever wrote in it last, and — in a
/// timeline only — the opening post whose body is shown underneath.
pub struct TopicRow {
    pub topic: Topic,
    pub last: Option<LastPost>,
    pub first: Option<Post>,
    /// The likes of the opening post, for the heart under a timeline entry.
    pub likes: Option<crate::db::likes::Summary>,
    /// Replies, likes, and whether something here waits for the viewer.
    pub counts: crate::db::topics::Counts,
}

/// The name of a section for a label — its configured title, or the slug
/// when the configuration no longer knows it: a topic whose section went is
/// still listed, and "archive" says more than nothing.
fn section_label<'a>(space: &'a Space, slug: &'a str) -> &'a str {
    space.category(slug).map_or(slug, |c| c.title.as_str())
}

/// The section line of a `topics` space: the way up, and every section
/// with the current one marked. `aria-current` is the mark a screen reader
/// hears; the stylesheet draws the `>` from it.
fn section_nav(space: &Space, lang: Lang, current: Option<&str>) -> Markup {
    html! {
        nav class="sections" aria-label=(lang.t("sections")) {
            a class="up" href="/" { (lang.t("sections")) }
            @for category in &space.categories {
                a href={ "/c/" (category.slug) }
                  aria-current=[(current == Some(category.slug.as_str())).then_some("page")] {
                    (category.title)
                }
            }
        }
    }
}

/// A number in a table: `0` as a dash, so the eye finds the rows where
/// something happened instead of reading zeros.
fn tally(n: i64) -> String {
    if n == 0 {
        "–".to_string()
    } else {
        n.to_string()
    }
}

/// The topic list, as a TABLE — see `space_page` for why. Shared by the
/// section page and the front page's recent list; the latter names the
/// section under each subject, because its rows come from all of them.
fn threads_table(space: &Space, lang: Lang, rows: &[TopicRow], with_section: bool) -> Markup {
    html! {
        table class=(if with_section { "threads recent" } else { "threads" }) {
            thead {
                tr {
                    th scope="col" { (lang.t("col_topic")) }
                    th scope="col" class="n" { (lang.t("col_replies")) }
                    th scope="col" class="n" { (lang.t("col_likes")) }
                    th scope="col" { (lang.t("col_last_reply")) }
                }
            }
            tbody {
                @for TopicRow { topic, last, counts, .. } in rows {
                    // `opens_the_topic` and not "is there a post": every
                    // topic has one. What this column promises is a REPLY,
                    // and the opening post is not one — printing it here
                    // would credit the opener with an answer they never
                    // wrote and make every silent thread look like a
                    // conversation.
                    @let answer = last.as_ref().filter(|last| !last.opens_the_topic);
                    // `unread` on the row, not on a cell: the mark stands in
                    // front of the subject and the whole row is what it is
                    // about.
                    tr class=[counts.unread.then_some("unread")] {
                        td class="subject" {
                            a href={ "/t/" (topic.id) } { (topic.title) }
                            p class="byline" {
                                @if with_section {
                                    span class="section" { (section_label(space, &topic.category)) }
                                    span class="sep" { " · " }
                                }
                                span class="name" { (topic.author_name) }
                                span class="sep" { " · " }
                                (opened(topic))
                            }
                        }
                        // The numbers carry their heading on a phone, where
                        // the heading row is hidden — from the catalogue,
                        // like the last-reply cell.
                        // `zero` so a phone, which prints the label after
                        // the number, can leave "– replies" out entirely.
                        td class={ "n replies" (if counts.replies == 0 { " zero" } else { "" }) }
                           data-label=(lang.t("col_replies")) { (tally(counts.replies)) }
                        td class={ "n likes" (if counts.likes == 0 { " zero" } else { "" }) }
                           data-label=(lang.t("col_likes")) { (tally(counts.likes)) }
                        // The heading is repeated on the cell because a
                        // phone cannot show the columns side by side:
                        // stacked, the stylesheet hides the heading row and
                        // prints this instead.
                        //
                        // And only where there IS an answer: "last reply: no
                        // replies yet" says the same thing twice.
                        td class="latest"
                           data-label=[answer.map(|_| lang.t("col_last_reply"))] {
                            @match answer {
                                Some(last) => {
                                    span class="name" { (last.author_name) }
                                    span class="when" { (moment(last.created_at)) }
                                },
                                None => span class="none" { (lang.t("no_replies_yet")) },
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The pencil that opens the edit box, drawn here rather than loaded.
///
/// An inline SVG and not a character: `✎` and `🗑` are rendered by whatever
/// font the reader happens to have, from a hairline glyph to a coloured emoji,
/// and an icon font would be the remote dependency this project does not take.
/// `currentColor` makes it follow the text it sits next to, in both themes.
fn icon_pencil() -> Markup {
    html! {
        svg class="icon" viewBox="0 0 16 16" width="14" height="14"
            aria-hidden="true" focusable="false" {
            path d="M11.2 1.8 14.2 4.8 5.6 13.4 1.8 14.2 2.6 10.4z"
                 fill="none" stroke="currentColor" stroke-width="1.3"
                 stroke-linejoin="round" {}
        }
    }
}

/// The waste basket that leads to the question before deleting.
fn icon_bin() -> Markup {
    html! {
        svg class="icon" viewBox="0 0 16 16" width="14" height="14"
            aria-hidden="true" focusable="false" {
            path d="M2.5 4h11M6 4V2.5h4V4M3.8 4l.7 9.5h7l.7-9.5M6.5 6.5v5M9.5 6.5v5"
                 fill="none" stroke="currentColor" stroke-width="1.3"
                 stroke-linecap="round" stroke-linejoin="round" {}
        }
    }
}

/// The heart. Pressed, the stylesheet fills it in the signal colour; at rest
/// it is an outline in the second voice, like the pencil and the basket.
fn icon_heart() -> Markup {
    html! {
        svg class="icon heart" viewBox="0 0 16 16" width="14" height="14"
            aria-hidden="true" focusable="false" {
            path d="M8 13.6 2.9 8.6a3 3 0 0 1 4.2-4.3L8 5.2l.9-.9a3 3 0 0 1 4.2 4.3z"
                 fill="none" stroke="currentColor" stroke-width="1.3"
                 stroke-linejoin="round" {}
        }
    }
}

/// The like control under a post (ADR 0008): a form with the heart and the
/// number for everybody else's post; on your own, the heart and the number
/// without a button, and nothing at all while the number is zero.
///
/// The names go in the `title`, newest first, so the number can be asked
/// "who?" without another page. `aria-pressed` is the state a screen reader
/// hears; `like.js` keeps both in step without a reload.
fn like_control(
    lang: Lang,
    post_id: i64,
    own: bool,
    summary: Option<&crate::db::likes::Summary>,
) -> Markup {
    let count = summary.map_or(0, |s| s.count);
    let liked = summary.is_some_and(|s| s.liked);
    let names = summary.map(|s| s.names.join(", ")).unwrap_or_default();
    let who = (!names.is_empty()).then_some(names.as_str());
    // `aria-label` REPLACES the button's content for a screen reader, so the
    // number has to be in it: "Unlike, 2 likes". `like.js` rebuilds it from
    // the same words, which travel on the button.
    let label = like_label(lang, liked, count);
    html! {
        @if own {
            @if count > 0 {
                span class="like own" title=[who] {
                    (icon_heart())
                    span class="count" { (count) }
                }
            }
        } @else {
            form class="like" method="post" action={ "/p/" (post_id) "/like" } {
                button type="submit" aria-pressed=(if liked { "true" } else { "false" })
                       title=[who]
                       aria-label=(label)
                       data-t-like=(lang.t("like")) data-t-unlike=(lang.t("unlike"))
                       data-t-likes-one=(lang.t("likes_one")) data-t-likes-many=(lang.t("likes_many")) {
                    (icon_heart())
                    @if count > 0 { span class="count" { (count) } }
                }
            }
        }
    }
}

/// What the heart says to somebody who cannot see it: the action of the
/// next click, and the number — "Like", "Unlike, 1 like", "Like, 3 likes".
fn like_label(lang: Lang, liked: bool, count: i64) -> String {
    let action = lang.t(if liked { "unlike" } else { "like" });
    match count {
        0 => action.to_string(),
        1 => format!("{action}, {}", lang.t("likes_one")),
        n => format!("{action}, {n} {}", lang.t("likes_many")),
    }
}

/// The question asked before a post goes, and the only place the button that
/// removes it lives.
///
/// It shows the post itself: "delete this?" about something the reader cannot
/// see is a question nobody can answer. The way back is a link to the topic,
/// so that leaving is as easy as arriving — the browser's back button is not
/// a design.
pub fn delete_question_page(
    space: &Space,
    who: &Identity,
    lang: Lang,
    bell: Bell,
    post: &Post,
) -> Markup {
    let body = html! {
        h1 { (lang.t("delete_title")) }
        p class="warn" { (lang.t("delete_question")) }
        article class="post" {
            p class="byline" {
                span class="name" { (post.author_name) }
                span class="sep" { " · " }
                (moment(post.created_at))
            }
            div class="body" { (PreEscaped(crate::markup::render(&post.body_markdown))) }
        }
        div class="decide" {
            form method="post" action={ "/p/" (post.id) "/delete" } {
                button type="submit" class="danger" { (lang.t("delete")) }
            }
            a class="back" href={ "/t/" (post.topic_id) } { (lang.t("cancel")) }
        }
    };
    layout(space, who, lang, bell, lang.t("delete_title"), body)
}

/// A space, rendered the way it is configured: a timeline shows the posts
/// themselves, a topic list shows titles. Same data, one line of configuration
/// apart.
pub fn space_page(
    space: &Space,
    who: &Identity,
    lang: Lang,
    bell: Bell,
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
                @for TopicRow { topic, first, likes, counts, .. } in topics {
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
                            (opened(topic))
                        }
                        @if let Some(post) = first {
                            div class="body" { (PreEscaped(crate::markup::render(&post.body_markdown))) }
                            // The heart, and how many people wrote
                            // underneath — the way to them, on an entry
                            // that shows the text but not the comments.
                            div class="foot" {
                                @let own = post.author_subject == who.subject;
                                (like_control(lang, post.id, own, likes.as_ref()))
                                a class="comments" href={ "/t/" (topic.id) } {
                                    @match counts.replies {
                                        0 => (lang.t("no_comments_yet")),
                                        1 => (lang.t("comment_count_one")),
                                        n => { (n) " " (lang.t("comment_count_many")) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            View::Topics => {
                // The section line first: on a page that lists one section,
                // the way to the others and up is the first thing a reader
                // who came from a bookmark or a mail needs.
                (section_nav(space, lang, category.map(|c| c.slug.as_str())))
                // A TABLE, and not a list with a heading pinned over it.
                //
                // Every row answers what happened here — opened by whom,
                // how many replies and likes, answered last by whom — each
                // under a heading that says which is which. That is a table
                // by every definition the word has, and writing it as a
                // `<ul>` would mean holding the columns in line by hand and
                // telling a screen reader nothing about what they mean.
                (threads_table(space, lang, topics, false))
            }
        }
        @if let Some(category) = category {
            @if may_post {
                // BEHIND A FOLD, because a list is read far more often than it
                // is written to. `<details>` and not a script: nothing here may
                // depend on JavaScript (ADR 0005), and the browser has had this element for
                // a decade.
                details class="write" {
                    summary { (lang.t("new_topic")) }
                    form method="post" action={ "/c/" (category.slug) "/new" } {
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
        }
    };
    layout(space, who, lang, bell, &space.title, body)
}

/// What a topic page shows, as opposed to who is looking at it.
pub struct TopicView<'a> {
    pub category: Option<&'a Category>,
    pub topic: &'a Topic,
    pub posts: &'a [Post],
    pub following: bool,
    /// The handles that are marked up as mentions (`mentions::highlighted`).
    pub highlighted: &'a std::collections::HashSet<String>,
    /// Author subject → handle, shown next to the name so it can be copied:
    /// completing `@` needs `mention.js`, and a page must work without it.
    pub handles: &'a std::collections::HashMap<String, String>,
    /// Post id → who likes it, for the heart under each post.
    pub likes: &'a std::collections::HashMap<i64, crate::db::likes::Summary>,
}

pub fn topic_page(
    space: &Space,
    who: &Identity,
    lang: Lang,
    bell: Bell,
    view: TopicView,
) -> Markup {
    let TopicView {
        category,
        topic,
        posts,
        following,
        highlighted,
        handles,
        likes,
    } = view;
    let may_reply = category.is_some_and(|c| crate::authz::may_reply(who, c));
    let body = html! {
        // THE WAY BACK. A topic is reached from a mail, from a search or from
        // a bookmark as often as from the list it belongs to, and without this
        // line those arrivals are a dead end: the browser's back button is not
        // a design, and the front page is a level too far up. Labelled with
        // the place it leads to rather than with an arrow, like the `up` link
        // in the header — the `../` in front of it comes from the stylesheet.
        h1 { (topic.title) }
        // THE META LINE: where this is, how long it is, and whether you
        // are following it — one quiet line under the title.
        //
        // The section link is THE WAY BACK. A topic is reached from a mail,
        // from a search or from a bookmark as often as from the list it
        // belongs to, and without it those arrivals are a dead end: the
        // browser's back button is not a design. Labelled with the place it
        // leads to, like the `up` link in the header — the `../` in front
        // of it comes from the stylesheet.
        //
        // Following is a FORM and not a link: it changes something, and a
        // GET that changes state is one a link preview or a mail client can
        // trigger without anybody clicking.
        // A `div`, not a `p`: a form may not stand inside a paragraph, and
        // a browser closes the paragraph in front of it — the control then
        // stands outside the line it belongs to (seen in the preview).
        @let replies = posts.len().saturating_sub(1);
        div class="meta" {
            @if let Some(category) = category {
                a class="up" href={ "/c/" (category.slug) } { (category.title) }
                span class="sep" { " · " }
            }
            span class="replies" {
                @match replies {
                    0 => (lang.t("no_replies_yet")),
                    1 => (lang.t("reply_count_one")),
                    n => { (n) " " (lang.t("reply_count_many")) }
                }
            }
            span class="sep" { " · " }
            form class="follow" method="post"
                 action={ "/t/" (topic.id) (if following { "/unfollow" } else { "/follow" }) } {
                button type="submit" title=[following.then_some(lang.t("following_note"))] {
                    (lang.t(if following { "unfollow" } else { "follow" }))
                }
            }
        }
        @for (i, post) in posts.iter().enumerate() {
            // An anchor per post, because the bell links to the first post
            // somebody has not read — not to the top of a long thread.
            article class="post" id={ "p" (post.id) } {
                p class="byline" {
                    span class="name" { (post.author_name) }
                    @if let Some(handle) = handles.get(&post.author_subject) {
                        " " span class="handle" { "@" (handle) }
                    }
                    span class="sep" { " · " }
                    // The opening post of an article IS the article, dated
                    // by its day; the comments under it were written at a
                    // moment and keep their hour.
                    @if i == 0 && topic.dated_by_day { (crate::clock::day(post.created_at, crate::clock::zone())) }
                    @else { (moment(post.created_at)) }
                    @if post.edited { " (" (lang.t("edited")) ")" }
                    // The post's number, and a link to its own anchor, so
                    // a post can be pointed at: "see #3".
                    a class="num" href={ "#p" (post.id) } { "#" (i + 1) }
                }
                div class="body" { (PreEscaped(crate::markup::render_with(&post.body_markdown, highlighted))) }
                // The foot of a post: the heart on the left, and on your own
                // the pencil and the basket on the right. One row, so a post
                // ends in a line barely taller than its own text.
                @let own = post.author_subject == who.subject;
                div class="foot" {
                    (like_control(lang, post.id, own, likes.get(&post.id)))
                // Only on your own — and the display is not the defence: the
                // query refuses the same thing again, on its own.
                @if crate::authz::may_modify(who, &post.author_subject) {
                    div class="own" {
                        // The text of your own post used to stand here a
                        // second time, in an open box: a thread you had
                        // written in read twice as long as anybody else's.
                        details class="edit" {
                            summary title=(lang.t("edit")) aria-label=(lang.t("edit")) {
                                (icon_pencil())
                            }
                            form method="post" action={ "/p/" (post.id) "/edit" } {
                                textarea name="body" rows="4" required { (post.body_markdown) }
                                button type="submit" { (lang.t("save")) }
                            }
                        }
                        // A LINK TO THE QUESTION, not a button that acts. An
                        // icon that deletes on the first click is one
                        // mis-click wide, and a page that must work without
                        // JavaScript has
                        // no `confirm()` to catch it. The GET changes
                        // nothing; the button on the page it leads to does.
                        a class="danger" href={ "/p/" (post.id) "/delete" }
                          title=(lang.t("delete")) aria-label=(lang.t("delete")) {
                            (icon_bin())
                        }
                    }
                }
                }
            }
        }
        // The way back up, at the foot of the thread — a long one is read
        // to the end, and the reply box waits there.
        @if posts.len() > 1 {
            a class="top" href="#top" { (lang.t("top")) }
        }
        @if may_reply {
            // ONE FOLD FOR BOTH FORMS. Writing a few lines and adding a
            // picture are the same intention — answering here — and asking
            // which of the two you want before you may write either would
            // make two decisions out of one. They stay two `<form>` elements
            // because one of them must be `multipart/form-data` and the other
            // must not; the fold is what the reader sees.
            details class="write" {
                summary { (lang.t("reply")) }
                form method="post" action={ "/t/" (topic.id) "/reply" } {
                    textarea name="body" rows="5" required aria-label=(lang.t("reply")) {}
                    button type="submit" { (lang.t("post_reply")) }
                }
                form method="post" enctype="multipart/form-data"
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
        }
    };
    layout(space, who, lang, bell, &topic.title, body)
}

/// `/notifications`: what is new first, then what was already seen.
pub fn notifications_page(
    space: &Space,
    who: &Identity,
    lang: Lang,
    bell: Bell,
    entries: &[crate::db::inbox::Entry],
    mention_mail: bool,
) -> Markup {
    use crate::db::inbox::Entry;
    let body = html! {
        h1 { (lang.t("notifications")) }
        @if entries.is_empty() {
            p class="empty" { (lang.t("nothing_new")) }
        } @else {
            @if bell.unread > 0 {
                // A form, like following: it changes something, and a GET
                // that did would be triggered by whatever prefetches links.
                form class="follow read-all" method="post" action="/notifications/read" {
                    button type="submit" { (lang.t("mark_all_read")) }
                }
            }
            ul class="topics inbox" {
                @for entry in entries {
                    // Where the link starts: nothing on this host, the
                    // entry's own host everywhere else (one bell, 0.10.0).
                    @let base = crate::live::link_base(entry.space(), &space.host);
                    li class=(if entry.unread() { "new" } else { "seen" }) {
                        @match entry {
                            Entry::Replies { topic_id, topic_title, count, latest_author, latest_at, first_post_id, unread, .. } => {
                                // "new" only while it is: a bundle that was
                                // read says how many there were, not that they
                                // are waiting.
                                @let key = match (*unread, *count == 1) {
                                    (true, true) => "new_reply_in",
                                    (true, false) => "new_replies_in",
                                    (false, true) => "reply_in",
                                    (false, false) => "replies_in",
                                };
                                a href={ (base) "/t/" (topic_id) "#p" (first_post_id) } {
                                    (count) " " (lang.t(key))
                                    " " b { (topic_title) }
                                }
                                span class="byline" {
                                    (lang.t("latest_from")) " "
                                    span class="name" { (latest_author) }
                                    span class="sep" { " · " }
                                    (moment(*latest_at))
                                }
                            }
                            Entry::Event { id, kind, title, reason, at, .. } => {
                                // Through treff, which marks it read and then
                                // leads on to the link it checked on arrival.
                                a href={ (base) "/notifications/e/" (id) } {
                                    @match kind {
                                        crate::db::events::Kind::FilmAvailable => {
                                            "🎬 " b { (title) } " " (lang.t("film_available"))
                                        }
                                        crate::db::events::Kind::FilmFailed => {
                                            b { (title) } " " (lang.t("film_failed"))
                                        }
                                    }
                                }
                                span class="byline" {
                                    @if let Some(reason) = reason {
                                        (reason)
                                        span class="sep" { " · " }
                                    }
                                    (moment(*at))
                                }
                            }
                            Entry::Mention { topic_id, topic_title, post_id, author, at, .. } => {
                                a href={ (base) "/t/" (topic_id) "#p" (post_id) } {
                                    span class="name" { (author) } " "
                                    (lang.t("mentioned_you_in")) " " b { (topic_title) }
                                }
                                span class="byline" { (moment(*at)) }
                            }
                            Entry::Likes { topic_id, topic_title, post_id, count, latest_name, at, .. } => {
                                // The latest name leads, the rest is a
                                // number: "cem and 2 others like your post".
                                a href={ (base) "/t/" (topic_id) "#p" (post_id) } {
                                    span class="name" { (latest_name) } " "
                                    @if *count == 1 { (lang.t("likes_your_post_in")) }
                                    @else { (lang.t("and")) " " (count - 1) " " (lang.t("others_like_your_post_in")) }
                                    " " b { (topic_title) }
                                }
                                span class="byline" { (moment(*at)) }
                            }
                        }
                    }
                }
            }
        }
    };
    let body = html! {
        (body)
        // The switch sits under the list, not above it: it is visited once,
        // the list every day.
        form class="follow mention-mail" method="post" action="/notifications/mention-mail" {
            input type="hidden" name="on" value=(if mention_mail { "0" } else { "1" });
            button type="submit" {
                (lang.t(if mention_mail { "mention_mail_off" } else { "mention_mail_on" }))
            }
            span class="note" {
                (lang.t(if mention_mail { "mention_mail_is_on" } else { "mention_mail_is_off" }))
            }
        }
    };
    layout(space, who, lang, bell, lang.t("notifications"), body)
}
