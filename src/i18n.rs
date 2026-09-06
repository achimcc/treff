//! Two languages, one set of keys.
//!
//! The interface is German and English; the code, the configuration and the
//! documentation are English (see `AGENTS.md`). The catalogues are embedded at
//! compile time, so a deployment cannot lose them and a missing file cannot
//! turn into a page of empty labels.

use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
}

const DE: &str = include_str!("../i18n/de.toml");
const EN: &str = include_str!("../i18n/en.toml");

fn catalogue(lang: Lang) -> &'static HashMap<String, String> {
    static CATALOGUES: OnceLock<(HashMap<String, String>, HashMap<String, String>)> =
        OnceLock::new();
    let (de, en) = CATALOGUES.get_or_init(|| {
        (
            toml::from_str(DE).expect("the German catalogue is embedded and parses"),
            toml::from_str(EN).expect("the English catalogue is embedded and parses"),
        )
    });
    match lang {
        Lang::De => de,
        Lang::En => en,
    }
}

impl Lang {
    /// Reads `Accept-Language`. Anything that is not German is English —
    /// English is the project's default, not the household's, because a
    /// stranger who finds this software should not land in a language they
    /// cannot read.
    pub fn from_accept_language(header: &str) -> Lang {
        for part in header.split(',') {
            let tag = part.split(';').next().unwrap_or_default().trim();
            let primary = tag.split('-').next().unwrap_or_default();
            match primary.to_ascii_lowercase().as_str() {
                "de" => return Lang::De,
                "en" => return Lang::En,
                _ => continue,
            }
        }
        Lang::En
    }

    /// The text for a key. A missing key yields the key itself: visible in the
    /// page, harmless to a reader, and impossible to miss for whoever put it
    /// there — the test below makes sure it never happens in the first place.
    pub fn t(&self, key: &str) -> &'static str {
        catalogue(*self)
            .get(key)
            .map(|s| s.as_str())
            .unwrap_or_else(|| Box::leak(key.to_string().into_boxed_str()))
    }

    pub fn code(&self) -> &'static str {
        match self {
            Lang::De => "de",
            Lang::En => "en",
        }
    }
}

/// Reading the language never fails: a missing or unreadable header is
/// English, like anything else we do not recognise.
impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Lang {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Lang::from_accept_language(
            parts
                .headers
                .get(axum::http::header::ACCEPT_LANGUAGE)
                .and_then(|h| h.to_str().ok())
                .unwrap_or_default(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_decides_and_the_default_is_english() {
        assert_eq!(Lang::from_accept_language("de-DE,de;q=0.9"), Lang::De);
        assert_eq!(Lang::from_accept_language("en-US,en;q=0.9"), Lang::En);
        assert_eq!(Lang::from_accept_language("DE"), Lang::De);
        assert_eq!(Lang::from_accept_language(""), Lang::En);
        assert_eq!(Lang::from_accept_language("kl-GL"), Lang::En);
        // The first language we know wins, not the first one listed.
        assert_eq!(Lang::from_accept_language("kl-GL,de;q=0.8"), Lang::De);
    }

    #[test]
    fn both_catalogues_carry_exactly_the_same_keys() {
        // This is the actual point of the task. A missing translation
        // otherwise shows up in a browser, in the language nobody on the
        // project reads by default.
        let de: Vec<&String> = {
            let mut k: Vec<_> = catalogue(Lang::De).keys().collect();
            k.sort();
            k
        };
        let en: Vec<&String> = {
            let mut k: Vec<_> = catalogue(Lang::En).keys().collect();
            k.sort();
            k
        };
        assert_eq!(de, en, "the two catalogues have drifted apart");
        assert!(!de.is_empty(), "the catalogues are empty");
    }

    #[test]
    fn no_entry_is_empty_in_either_language() {
        for lang in [Lang::De, Lang::En] {
            for (key, value) in catalogue(lang) {
                assert!(
                    !value.trim().is_empty(),
                    "{key} is empty in {}",
                    lang.code()
                );
            }
        }
    }

    #[test]
    fn a_key_that_exists_reads_differently_in_the_two_languages() {
        // Cheap guard against a catalogue that was copied and never
        // translated.
        assert_ne!(Lang::De.t("sign_out"), Lang::En.t("sign_out"));
    }
}
