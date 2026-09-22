//! Markdown in, sanitized HTML out. After attachments this is the second
//! place where someone else's bytes decide what a browser does, so the tests
//! next to it describe the attack rather than the function.

use std::collections::HashSet;

/// Two stages, and both are needed.
///
/// comrak turns Markdown into HTML and would pass raw HTML through if
/// `render.r#unsafe` were set — it is not. ammonia then throws away
/// everything that is not on its allowlist. **The sanitizer is the boundary,
/// not the renderer setting:** a later feature that does need raw HTML from
/// comrak must not silently give up the guarantee, and with both stages in
/// place it cannot.
pub fn render(markdown: &str) -> String {
    let raw = comrak::markdown_to_html(markdown, &options());
    sanitize(&raw, false)
}

/// The boundary. `mentions` decides whether the one class treff itself writes
/// may pass; nothing a person types can produce it (see `render_with`).
fn sanitize(raw: &str, mentions: bool) -> String {
    let schemes: HashSet<&str> = HashSet::from(["http", "https", "mailto"]);
    let mut builder = ammonia::Builder::default();
    builder
        .url_schemes(schemes)
        .link_rel(Some("noopener noreferrer nofollow"))
        // An `img` is fetched by the browser without anyone deciding to; a
        // link is followed on purpose. So a picture may only come from this
        // instance — otherwise the operator of some other host learns the
        // address and the moment of every reader, which is precisely what a
        // closed circle is for. The CSP says `img-src 'self'` as well; this is
        // the lock that does not depend on the browser honouring it.
        .attribute_filter(|element, attribute, value| match (element, attribute) {
            ("img", "src") if !value.starts_with('/') => None,
            _ => Some(value.into()),
        });
    if mentions {
        builder.add_allowed_classes("span", &["mention"]);
    }
    builder.clean(raw).to_string()
}

/// One set of options for both renderings, so that `render_with` cannot
/// drift into parsing a post differently from `render`.
fn options() -> comrak::Options<'static> {
    let mut opts = comrak::Options::default();
    opts.extension.strikethrough = true;
    opts.extension.table = true;
    opts.extension.autolink = true;
    opts.render.r#unsafe = false;
    opts
}

/// The handles a post mentions, lower-cased, each once, in order of
/// appearance.
///
/// FOUND ON THE AST, NOT WITH A PATTERN OVER THE SOURCE. What a reader sees
/// as code or as a link is not a mention, and only the parser knows where
/// those are — the same reason the renderer is not a regex. With `autolink`
/// on, an address like `user@example.org` is a link node too, and falls out
/// for that reason before the boundary rule below is even asked.
pub fn mentions(markdown: &str) -> Vec<String> {
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, markdown, &options());
    let mut out: Vec<String> = Vec::new();
    for node in text_nodes(root) {
        if let comrak::nodes::NodeValue::Text(ref text) = node.data.borrow().value {
            for (_, _, handle) in mention_spans(text) {
                if !out.contains(&handle) {
                    out.push(handle);
                }
            }
        }
    }
    out
}

/// [`render`], with every `@handle` in `mentionable` marked up as a mention.
///
/// Anything not in the set stays text — an unknown handle and one that may not
/// be told about this post render identically, so the page never confirms
/// that somebody exists.
///
/// THE SPAN IS A `Raw` NODE, which comrak only ever creates when a program
/// asks it to: raw HTML typed into a post is still omitted by the renderer
/// (`unsafe` stays off), so nobody can write a mention by hand. And the
/// sanitizer still runs over the result, now allowing exactly one class on
/// exactly one element.
pub fn render_with(markdown: &str, mentionable: &HashSet<String>) -> String {
    if mentionable.is_empty() {
        return render(markdown);
    }
    let arena = comrak::Arena::new();
    let opts = options();
    let root = comrak::parse_document(&arena, markdown, &opts);
    for node in text_nodes(root) {
        let text = match node.data.borrow().value {
            comrak::nodes::NodeValue::Text(ref t) => t.to_string(),
            _ => continue,
        };
        let spans: Vec<_> = mention_spans(&text)
            .into_iter()
            .filter(|(_, _, h)| mentionable.contains(h))
            .collect();
        if spans.is_empty() {
            continue;
        }
        // Rebuilt piece by piece in front of the node, which then goes:
        // text, mention, text, mention, …, text.
        let mut at = 0;
        for (start, end, _) in spans {
            if start > at {
                node.insert_before(arena.alloc(
                    comrak::nodes::NodeValue::Text(text[at..start].to_string().into()).into(),
                ));
            }
            // `text[start..end]` is `@` plus `[A-Za-z0-9._-]` and nothing
            // else (see `mention_spans`), so it needs no escaping.
            node.insert_before(
                arena.alloc(
                    comrak::nodes::NodeValue::Raw(format!(
                        r#"<span class="mention">{}</span>"#,
                        &text[start..end]
                    ))
                    .into(),
                ),
            );
            at = end;
        }
        if at < text.len() {
            node.insert_before(
                arena.alloc(comrak::nodes::NodeValue::Text(text[at..].to_string().into()).into()),
            );
        }
        node.detach();
    }
    let mut raw = String::new();
    if comrak::format_html(root, &opts, &mut raw).is_err() {
        // Writing into a String does not fail; if it ever did, the plain
        // rendering is the safe thing to show.
        return render(markdown);
    }
    sanitize(&raw, true)
}

/// The text a reader sees as prose: text nodes that are not inside a link or
/// an image. Code spans and code blocks are nodes of their own and are never
/// text. Adjacent text nodes are merged first, so that a handle the parser
/// happened to split is still one handle.
fn text_nodes<'a>(root: comrak::Node<'a>) -> Vec<comrak::Node<'a>> {
    use comrak::nodes::NodeValue;
    for node in root.descendants().collect::<Vec<_>>() {
        while let Some(next) = node.next_sibling() {
            let merged = {
                let (mut here, there) = (node.data.borrow_mut(), next.data.borrow());
                match (&mut here.value, &there.value) {
                    (NodeValue::Text(a), NodeValue::Text(b)) => {
                        a.to_mut().push_str(b);
                        true
                    }
                    _ => false,
                }
            };
            if !merged {
                break;
            }
            next.detach();
        }
    }
    root.descendants()
        .filter(|n| matches!(n.data.borrow().value, NodeValue::Text(_)))
        .filter(|n| {
            !n.ancestors().any(|a| {
                matches!(
                    a.data.borrow().value,
                    NodeValue::Link(_) | NodeValue::Image(_)
                )
            })
        })
        .collect()
}

/// Where the mentions are in one piece of prose: byte range of `@handle` and
/// the handle, lower-cased.
///
/// An `@` counts only at the start or after a character that could not be part
/// of an address (`user@example.org` is not a mention of `example.org`), and
/// only if what follows is a handle as `auth::checked_handle` accepts it and
/// is not itself followed by another `@`. Trailing `.`, `-` and `_` belong to
/// the sentence, not the handle — "thanks, @ben." mentions `ben`.
fn mention_spans(text: &str) -> Vec<(usize, usize, String)> {
    let bytes = text.as_bytes();
    let part_of_address = |c: char| c.is_alphanumeric() || "._%+-".contains(c);
    let handle_byte = |b: u8| b.is_ascii_alphanumeric() || b"._-".contains(&b);
    let mut out = Vec::new();
    for (i, c) in text.char_indices() {
        if c != '@' {
            continue;
        }
        if text[..i].chars().next_back().is_some_and(part_of_address) {
            continue;
        }
        let mut end = i + 1;
        while end < bytes.len() && handle_byte(bytes[end]) {
            end += 1;
        }
        if bytes.get(end) == Some(&b'@') {
            continue;
        }
        while end > i + 1 && b"._-".contains(&bytes[end - 1]) {
            end -= 1;
        }
        if let Some(handle) = crate::auth::checked_handle(&text[i + 1..end]) {
            out.push((i, end, handle));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `attr="value"` value in the rendered output. Crude on purpose:
    /// the point is to look at what a browser would act on, not to parse HTML.
    fn attribute_values(html: &str) -> Vec<String> {
        html.split("=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next().map(str::to_string))
            .collect()
    }

    #[test]
    fn renders_ordinary_markdown() {
        let html = render("**bold** and a [link](https://example.org)");
        assert!(html.contains("<strong>bold</strong>"), "got: {html}");
        assert!(html.contains("href=\"https://example.org\""), "got: {html}");
    }

    #[test]
    fn keeps_the_everyday_things() {
        let html = render("- one\n- two\n\n`code` and https://example.org\n");
        assert!(html.contains("<li>"), "got: {html}");
        assert!(html.contains("<code>code</code>"), "got: {html}");
        assert!(
            html.contains("href=\"https://example.org\""),
            "autolink: {html}"
        );
    }

    #[test]
    fn links_carry_rel_noopener() {
        // A link to somewhere else must not hand that page a window handle.
        let html = render("[out](https://example.org)");
        assert!(html.contains("rel=\""), "got: {html}");
        assert!(html.contains("noopener"), "got: {html}");
    }

    #[test]
    fn strips_script_tags() {
        let html = render("<script>alert(1)</script>");
        assert!(!html.contains("<script"), "got: {html}");
        assert!(!html.contains("alert(1)"), "got: {html}");
    }

    #[test]
    fn strips_event_handlers() {
        let html = render("<img src=x onerror=alert(1)>");
        assert!(!html.to_lowercase().contains("onerror"), "got: {html}");
    }

    #[test]
    fn strips_event_handlers_on_allowed_tags_too() {
        // The tag survives sanitizing; the attribute must not ride along.
        let html = render("<a href=\"https://example.org\" onclick=\"alert(1)\">x</a>");
        assert!(!html.to_lowercase().contains("onclick"), "got: {html}");
    }

    #[test]
    fn refuses_javascript_urls() {
        let html = render("[click](javascript:alert(1))");
        assert!(!html.to_lowercase().contains("javascript:"), "got: {html}");
    }

    #[test]
    fn refuses_javascript_urls_however_they_are_dressed_up() {
        // Case, whitespace and HTML entities are the three usual disguises.
        //
        // The assertion is about what a browser can *act on*: no `href` and no
        // `src` may survive any of these. Searching the output for the string
        // "javascript:" would be wrong — one of these never becomes a link at
        // all and stays visible as escaped text, which is harmless and would
        // still match.
        for attempt in [
            "[a](JaVaScRiPt:alert(1))",
            "[b](  javascript:alert(1))",
            "[c](java\tscript:alert(1))",
            "[d](&#106;avascript:alert(1))",
            "<a href=\"JAVASCRIPT:alert(1)\">e</a>",
            "<img src=\"javascript:alert(1)\">",
        ] {
            let html = render(attempt).to_lowercase();
            for value in attribute_values(&html) {
                assert!(
                    !value.replace(['\t', '\n', ' '], "").contains("javascript"),
                    "{attempt:?} kept a usable attribute: {html}"
                );
            }
        }
    }

    #[test]
    fn refuses_data_urls_in_images() {
        let html = render("![x](data:text/html;base64,PHNjcmlwdD4=)");
        assert!(!html.contains("data:text/html"), "got: {html}");
    }

    #[test]
    fn keeps_raw_html_out_even_when_it_looks_harmless() {
        // No "just this one tag": whoever makes the first exception makes the
        // second one too.
        let html = render("<iframe src=\"https://example.org\"></iframe>");
        assert!(!html.contains("<iframe"), "got: {html}");
    }

    #[test]
    fn a_script_hidden_inside_markdown_structure_is_still_stripped() {
        let html =
            render("> quoted\n>\n> <script>alert(1)</script>\n\n1. <script>alert(2)</script>\n");
        assert!(!html.contains("<script"), "got: {html}");
        assert!(!html.contains("alert("), "got: {html}");
    }

    #[test]
    fn a_fenced_code_block_shows_its_contents_as_text() {
        // Code about scripts is not a script. It must render visibly and
        // inertly — escaped, not executed and not deleted.
        let html = render("```\n<script>alert(1)</script>\n```\n");
        assert!(html.contains("<code"), "got: {html}");
        assert!(!html.contains("<script>"), "got: {html}");
        assert!(html.contains("&lt;script&gt;"), "got: {html}");
    }
}

#[cfg(test)]
mod mentioning {
    use super::*;

    fn set(handles: &[&str]) -> HashSet<String> {
        handles.iter().map(|h| (*h).to_string()).collect()
    }

    #[test]
    fn a_handle_after_an_at_sign_is_a_mention() {
        assert_eq!(mentions("thanks @konrad!"), vec!["konrad"]);
        assert_eq!(mentions("@ada see this"), vec!["ada"], "start of text");
        assert_eq!(
            mentions("(@ada) and, @ben."),
            vec!["ada", "ben"],
            "after punctuation, before a full stop"
        );
        assert_eq!(mentions("@Konrad"), vec!["konrad"], "lower-cased");
        assert_eq!(mentions("@ada and @ada again"), vec!["ada"], "each once");
        assert_eq!(
            mentions("**@ada** and _@ben_"),
            vec!["ada", "ben"],
            "inside emphasis still counts"
        );
        assert_eq!(mentions("line one\n@ada on line two"), vec!["ada"]);
    }

    /// AN ADDRESS IS NOT A MENTION, and neither is anything a reader sees as
    /// code or as a link — the `@` there is part of something else.
    #[test]
    fn an_at_sign_that_belongs_to_something_else_is_not_a_mention() {
        assert!(
            mentions("write to user@example.org").is_empty(),
            "an address"
        );
        assert!(
            mentions("mail me at ada@home").is_empty(),
            "no boundary before the @"
        );
        assert!(mentions("`@ada`").is_empty(), "a code span");
        assert!(mentions("```\n@ada\n```\n").is_empty(), "a code block");
        assert!(mentions("    @ada\n").is_empty(), "an indented code block");
        assert!(
            mentions("[@ada](https://example.org)").is_empty(),
            "a link text"
        );
        assert!(mentions("https://example.org/@ada").is_empty(), "a url");
        assert!(
            mentions("@ada@example.org").is_empty(),
            "a handle-shaped address"
        );
        assert!(mentions("an @ alone").is_empty());
        assert!(
            mentions(&format!("@{}", "x".repeat(65))).is_empty(),
            "too long to be a handle"
        );
    }

    #[test]
    fn a_mentionable_handle_is_marked_and_nothing_else_is() {
        let html = render_with("hi @Ada and @eve", &set(&["ada"]));
        assert!(
            html.contains(r#"<span class="mention">@Ada</span>"#),
            "got: {html}"
        );
        assert!(
            html.contains("and @eve"),
            "an unknown one stays text: {html}"
        );
        assert!(
            !html.contains(r#"<span class="mention">@eve"#),
            "got: {html}"
        );
    }

    /// THE PAGE MUST NOT TELL WHO EXISTS. With nothing mentionable the output
    /// is exactly what `render` makes — byte for byte.
    #[test]
    fn with_nothing_mentionable_the_output_is_the_plain_rendering() {
        let md = "hi @ada, see `@ben` and [@cem](https://example.org)";
        assert_eq!(render_with(md, &set(&[])), render(md));
    }

    #[test]
    fn a_mention_in_code_is_not_marked_even_if_mentionable() {
        let html = render_with("`@ada`", &set(&["ada"]));
        assert!(!html.contains("mention"), "got: {html}");
    }

    /// Only treff can make the span. Somebody who writes it by hand gets it
    /// escaped by the renderer, and the class never reaches the page.
    #[test]
    fn a_hand_written_mention_span_is_not_a_mention() {
        // `@eve` is not mentionable here, so any `class="mention"` in the
        // output could only have come from the hand-written tag. (With `@ada`
        // the output DOES carry the class — treff's own, because the text left
        // over after the tag is omitted is a real mention of somebody who may
        // be told. Measured while writing this test.)
        let html = render_with(r#"<span class="mention">@eve</span>"#, &set(&["ada"]));
        assert!(!html.contains(r#"class="mention""#), "got: {html}");
        let html = render(r#"<span class="mention">@ada</span>"#);
        assert!(!html.contains(r#"class="mention""#), "got: {html}");
    }
}

#[cfg(test)]
mod attachment_links {
    use super::*;

    #[test]
    fn an_image_from_this_instance_survives() {
        // Attachments are referenced as `/a/<id>`, a RELATIVE url. If the
        // sanitizer dropped those, every uploaded image would render as a
        // broken box — and it would look like a storage bug rather than a
        // sanitizer setting.
        let html = render("![a picture](/a/7)");
        assert!(html.contains("src=\"/a/7\""), "got: {html}");
        assert!(html.contains("alt=\"a picture\""), "got: {html}");
    }

    #[test]
    fn an_image_from_somewhere_else_does_not() {
        // No third party may learn who is reading: the CSP says img-src 'self'
        // and this is the second lock.
        let html = render("![tracker](https://elsewhere.example/pixel.png)");
        assert!(!html.contains("elsewhere.example"), "got: {html}");
    }
}
