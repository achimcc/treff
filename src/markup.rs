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
    let mut opts = comrak::Options::default();
    opts.extension.strikethrough = true;
    opts.extension.table = true;
    opts.extension.autolink = true;
    opts.render.r#unsafe = false;

    let raw = comrak::markdown_to_html(markdown, &opts);

    let schemes: HashSet<&str> = HashSet::from(["http", "https", "mailto"]);
    ammonia::Builder::default()
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
        })
        .clean(&raw)
        .to_string()
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
