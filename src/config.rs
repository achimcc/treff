//! The configuration: which addresses this instance serves, and who may do
//! what in them. Parsed once at startup; a file that cannot be trusted stops
//! the program instead of being repaired into something plausible.

use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot parse configuration: {0}")]
    Syntax(#[from] toml::de::Error),
    #[error("host {0} is configured twice")]
    DuplicateHost(String),
    #[error("space {0} has no categories")]
    NoCategories(String),
    #[error("space {0} would be readable by nobody")]
    NobodyCanRead(String),
    #[error("space {space} has the category {slug} twice")]
    DuplicateCategory { space: String, slug: String },
    #[error("space {space} has the unusable category slug {slug:?}")]
    InvalidSlug { space: String, slug: String },
    #[error("no time zone is called {0:?}")]
    UnknownTimeZone(String),
}

/// A slug becomes a path segment (`/c/<slug>`), so it is held to the strict
/// set rather than escaped later: lower-case ASCII letters, digits, hyphens,
/// and not empty.
fn slug_is_usable(slug: &str) -> bool {
    !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum View {
    Timeline,
    Topics,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    pub slug: String,
    pub title: String,
    /// Who may open a topic here. An empty list means nobody — never everybody.
    #[serde(default)]
    pub post: Vec<String>,
    /// Who may reply here. Empty means nobody, for the same reason.
    #[serde(default)]
    pub reply: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Space {
    pub host: String,
    pub title: String,
    pub view: View,
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(rename = "category", default)]
    pub categories: Vec<Category>,
    /// A directory of `YYYY-MM-DD-<name>.md` articles to mirror into this
    /// space's **first** category. A space with one category — a blog — is
    /// what this is for; naming several and expecting a choice would be a
    /// second mechanism for no gain.
    #[serde(default)]
    pub articles: Option<String>,
    /// The largest attachment this space accepts, in bytes. The design asks
    /// for it to be configurable; the default is what a photograph from a
    /// phone weighs.
    #[serde(default = "default_attachment_bytes")]
    pub attachment_max_bytes: u64,
    /// The front matter key that holds an article's title. Those files are
    /// written for something else — a newsletter, a static site — and their
    /// keys are in their author's language. Making this configurable is
    /// cheaper than asking every such file to carry a second, English key
    /// saying the same thing.
    #[serde(default = "default_title_key")]
    pub title_key: String,
    /// Where this space came from — a landing page, the other spaces, whatever
    /// the operator wants people to be able to get back to.
    ///
    /// A space is one address among several, and the way back has to be ON the
    /// page: the browser's back button is memory, not navigation, and it is
    /// empty for anyone who arrived by bookmark.
    #[serde(default)]
    pub home: Option<String>,
}

fn default_title_key() -> String {
    "title".to_string()
}

fn default_attachment_bytes() -> u64 {
    8 * 1024 * 1024
}

impl Space {
    pub fn category(&self, slug: &str) -> Option<&Category> {
        self.categories.iter().find(|c| c.slug == slug)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The zone every timestamp in the interface is shown in, as an IANA name
    /// — `Europe/Berlin`, not an offset. Left out, the machine decides (`TZ`,
    /// `/etc/localtime`), which is the right answer whenever the server stands
    /// where the people do.
    ///
    /// Not per space and not per reader: a forum for a closed circle is read
    /// in the circle's own time, and a second mechanism for that would be a
    /// setting nobody asked for.
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(rename = "space", default)]
    pub spaces: Vec<Space>,
}

/// The `Host` header carries case and a possible port; neither means anything
/// for choosing a space. Without this normalisation `BLOG.example.org` would
/// match no space and end up in the 403 branch, though it is the same machine.
fn normalise(host: &str) -> String {
    host.split(':').next().unwrap_or(host).to_ascii_lowercase()
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let mut c: Config = toml::from_str(text)?;

        // Checked HERE and not where the first page is rendered: a name with a
        // typo in it must stop the program while somebody is still watching
        // the logs, not show up as timestamps that are silently two hours out.
        if let Some(name) = c.timezone.as_deref()
            && crate::clock::resolve(Some(name)).is_err()
        {
            return Err(ConfigError::UnknownTimeZone(name.to_string()));
        }

        for s in &mut c.spaces {
            s.host = normalise(&s.host);
        }

        let mut hosts = HashSet::new();
        for s in &c.spaces {
            if !hosts.insert(s.host.clone()) {
                return Err(ConfigError::DuplicateHost(s.host.clone()));
            }
            if s.categories.is_empty() {
                return Err(ConfigError::NoCategories(s.host.clone()));
            }
            if s.read.is_empty() {
                return Err(ConfigError::NobodyCanRead(s.host.clone()));
            }
            let mut slugs = HashSet::new();
            for k in &s.categories {
                if !slug_is_usable(&k.slug) {
                    return Err(ConfigError::InvalidSlug {
                        space: s.host.clone(),
                        slug: k.slug.clone(),
                    });
                }
                if !slugs.insert(k.slug.as_str()) {
                    return Err(ConfigError::DuplicateCategory {
                        space: s.host.clone(),
                        slug: k.slug.clone(),
                    });
                }
            }
        }
        Ok(c)
    }

    pub fn space_for_host(&self, host: &str) -> Option<&Space> {
        let h = normalise(host);
        self.spaces.iter().find(|s| s.host == h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
[[space]]
host  = "blog.example.org"
title = "Notes"
view  = "timeline"
read  = ["Household"]

  [[space.category]]
  slug  = "notes"
  title = "Notes"
  post  = ["Writers"]
  reply = ["Household"]

[[space]]
host  = "forum.example.org"
title = "Forum"
view  = "topics"
read  = ["Household", "Friends"]

  [[space.category]]
  slug  = "general"
  title = "General"
  post  = ["Household", "Friends"]
  reply = ["Household", "Friends"]
"#;

    #[test]
    fn a_configuration_without_a_zone_leaves_the_choice_to_the_machine() {
        let c = Config::parse(EXAMPLE).expect("valid");
        assert_eq!(c.timezone, None);
    }

    #[test]
    fn a_named_zone_is_read_and_kept() {
        let mut text = String::from("timezone = \"Europe/Berlin\"\n");
        text.push_str(EXAMPLE);
        let c = Config::parse(&text).expect("valid");
        assert_eq!(c.timezone.as_deref(), Some("Europe/Berlin"));
    }

    #[test]
    fn a_zone_nobody_has_heard_of_stops_the_program() {
        // Fail closed, like every other unusable value in this file. Falling
        // back to UTC would put every timestamp in the forum an hour or two
        // beside the truth, and nothing would say so.
        let mut text = String::from("timezone = \"Mittelerde/Auenland\"\n");
        text.push_str(EXAMPLE);
        assert!(matches!(
            Config::parse(&text),
            Err(ConfigError::UnknownTimeZone(_))
        ));
    }

    #[test]
    fn reads_two_spaces() {
        let c = Config::parse(EXAMPLE).expect("valid");
        assert_eq!(c.spaces.len(), 2);
        let blog = c.space_for_host("blog.example.org").expect("known host");
        assert_eq!(blog.view, View::Timeline);
        assert_eq!(
            blog.category("notes").expect("category").post,
            vec!["Writers"]
        );
    }

    #[test]
    fn an_unknown_host_is_not_a_space() {
        let c = Config::parse(EXAMPLE).expect("valid");
        assert!(c.space_for_host("evil.example.org").is_none());
    }

    #[test]
    fn the_host_is_matched_without_port_and_case() {
        let c = Config::parse(EXAMPLE).expect("valid");
        assert!(c.space_for_host("BLOG.example.org:443").is_some());
    }

    #[test]
    fn a_duplicate_host_is_refused() {
        let mut twice = String::from(EXAMPLE);
        twice.push_str(
            "\n[[space]]\nhost = \"blog.example.org\"\ntitle = \"x\"\nview = \"topics\"\nread = [\"Household\"]\n\n  [[space.category]]\n  slug = \"a\"\n  title = \"A\"\n  post = []\n  reply = []\n",
        );
        assert!(matches!(
            Config::parse(&twice),
            Err(ConfigError::DuplicateHost(_))
        ));
    }

    #[test]
    fn an_unknown_view_is_refused() {
        let text = EXAMPLE.replace("view  = \"timeline\"", "view  = \"carousel\"");
        assert!(Config::parse(&text).is_err());
    }

    #[test]
    fn a_space_without_categories_is_refused() {
        let text = "[[space]]\nhost = \"a.example.org\"\ntitle = \"A\"\nview = \"topics\"\nread = [\"Household\"]\n";
        assert!(matches!(
            Config::parse(text),
            Err(ConfigError::NoCategories(_))
        ));
    }

    #[test]
    fn a_space_nobody_may_read_is_refused() {
        let text = EXAMPLE.replace("read  = [\"Household\"]", "read  = []");
        assert!(matches!(
            Config::parse(&text),
            Err(ConfigError::NobodyCanRead(_))
        ));
    }

    #[test]
    fn a_category_nobody_may_post_in_is_allowed() {
        // A category nobody may write in is an archive, not a mistake.
        let text = EXAMPLE.replace("post  = [\"Writers\"]", "post  = []");
        assert!(Config::parse(&text).is_ok());
    }

    #[test]
    fn a_duplicate_category_slug_in_one_space_is_refused() {
        let text = EXAMPLE.replace(
            "  [[space.category]]\n  slug  = \"general\"",
            "  [[space.category]]\n  slug  = \"general\"\n  title = \"G\"\n  post  = []\n  reply = []\n\n  [[space.category]]\n  slug  = \"general\"",
        );
        assert!(matches!(
            Config::parse(&text),
            Err(ConfigError::DuplicateCategory { .. })
        ));
    }

    #[test]
    fn a_slug_that_is_not_url_safe_is_refused() {
        // A slug becomes a URL path segment. Anything outside the strict set
        // is either a routing bug or an attempt at one.
        for bad in ["Gene ral", "general/", "../etc", "General", "", "gene_ral"] {
            let text = EXAMPLE.replace("slug  = \"general\"", &format!("slug  = \"{bad}\""));
            assert!(
                matches!(Config::parse(&text), Err(ConfigError::InvalidSlug { .. })),
                "accepted the slug {bad:?}"
            );
        }
    }

    #[test]
    fn a_space_may_name_an_article_directory() {
        let with = EXAMPLE.replace(
            "view  = \"timeline\"",
            "view  = \"timeline\"\narticles = \"/etc/treff/articles\"",
        );
        let c = Config::parse(&with).expect("valid");
        assert_eq!(
            c.space_for_host("blog.example.org")
                .expect("space")
                .articles
                .as_deref(),
            Some("/etc/treff/articles")
        );
        // And a space without one keeps None rather than an empty path.
        let plain = Config::parse(EXAMPLE).expect("valid");
        assert!(
            plain
                .space_for_host("forum.example.org")
                .expect("space")
                .articles
                .is_none()
        );
    }

    #[test]
    fn a_misspelled_key_is_refused() {
        // Without `deny_unknown_fields` this is the worst kind of mistake: the
        // file parses, `post` stays empty, and the category quietly admits
        // nobody. Fail closed is right, but it must also be loud.
        let text = EXAMPLE.replace("post  = [\"Writers\"]", "psot  = [\"Writers\"]");
        assert!(Config::parse(&text).is_err());
    }
}
