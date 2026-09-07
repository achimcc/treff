//! The second exit: one `POST` per post, to wherever the operator points it.
//!
//! Generic on purpose — ntfy, Gotify, a Matrix bridge, a shell script behind a
//! reverse proxy. What goes out is JSON with four fields, and whatever ignores
//! `topic` simply ignores it.

use super::{Message, Transport};

// REQWEST KOMMT UEBER `openidconnect`, und das ist eine Entscheidung gegen
// eine zweite Abhaengigkeit: Die Bibliothek ist ohnehin im Baum, mit einem
// TLS-Provider, der hier schon funktioniert. Ein eigenes `reqwest` mit
// `rustls` zoege einen ZWEITEN Provider herein (aws-lc-rs neben lettres
// `ring`) — mehr Angriffsflaeche und mehr Bauzeit fuer denselben HTTP-Aufruf.
use openidconnect::reqwest;

/// Where notifications go, besides mail.
#[derive(Debug, Clone)]
pub struct Settings {
    /// **From the configuration file and nowhere else.** A webhook whose
    /// target could be steered by something in a post would be an SSRF with a
    /// friendly name.
    pub url: String,
    /// ntfy wants the topic in the body when the body is JSON. Anything else
    /// ignores the field.
    pub topic: Option<String>,
    /// A bearer token, from a **file** — the same rule as every other secret
    /// here, for the same reason.
    pub token: Option<String>,
    pub timeout: std::time::Duration,
}

impl Settings {
    /// `None` when no URL is configured: no webhook is an absent feature, not
    /// a broken one.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Ok(url) = std::env::var("TREFF_WEBHOOK_URL") else {
            return Ok(None);
        };
        if !url.starts_with("https://") && !url.starts_with("http://") {
            anyhow::bail!("TREFF_WEBHOOK_URL is not an http(s) URL");
        }
        let topic = std::env::var("TREFF_WEBHOOK_TOPIC")
            .ok()
            .filter(|t| !t.is_empty());
        let token = match std::env::var("TREFF_WEBHOOK_TOKEN_FILE") {
            Ok(path) => {
                let raw = std::fs::read_to_string(&path)
                    .map_err(|e| anyhow::anyhow!("cannot read {path}: {e}"))?;
                let value = raw.trim().to_string();
                if value.is_empty() {
                    anyhow::bail!("{path} is empty; a blank token is not a token");
                }
                Some(value)
            }
            Err(_) => None,
        };
        Ok(Some(Self {
            url,
            topic,
            token,
            timeout: std::time::Duration::from_secs(10),
        }))
    }
}

pub struct Hook {
    client: reqwest::Client,
    settings: Settings,
}

impl Hook {
    pub fn new(settings: Settings) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            // NO REDIRECTS. A 302 from the configured host to somewhere else
            // would carry the bearer token along, and the operator never
            // agreed to that second address.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(settings.timeout)
            .build()?;
        Ok(Self { client, settings })
    }
}

impl Transport for Hook {
    async fn deliver(&self, message: &Message) -> anyhow::Result<()> {
        // DER BEITRAGSTEXT GEHT NICHT MIT, und das ist keine Sparsamkeit.
        //
        // Der Webhook ist der Kanal dessen, der die Anlage betreibt — und der
        // hat hier ausdruecklich KEINE Sonderrechte: „Jeder darf nur sein
        // eigenes bearbeiten und loeschen, auch die Verwaltung nicht fremdes"
        // (design.md §5). Ein Push, der jeden fremden Beitrag im Wortlaut aufs
        // Telefon legt, hebelt das praktisch aus: Man liest mit, ohne das
        // Forum zu oeffnen, und niemand sieht es.
        //
        // Was bleibt, ist das Signal: WER hat WO geschrieben, und ein Link
        // dorthin. Der Titel eines Themas ist fuer den Kreis ohnehin sichtbar;
        // der Text bleibt, wo alle ihn unter denselben Bedingungen lesen.
        let mut body = serde_json::json!({
            "title": message.space_title,
            "message": format!("{} hat in \"{}\" geschrieben", message.author, message.topic_title),
            "click": message.link,
        });
        if let Some(topic) = &self.settings.topic {
            body["topic"] = serde_json::Value::String(topic.clone());
        }

        // `.json()` haette das gleichnamige Feature gebraucht, das
        // `openidconnect` nicht aktiviert. Der Rumpf ist drei Felder — ihn
        // selbst zu serialisieren ist billiger als ein Feature-Flag, das
        // irgendwann jemand anders anfasst.
        let mut request = self
            .client
            .post(&self.settings.url)
            .header("content-type", "application/json")
            .body(serde_json::to_string(&body)?);
        if let Some(token) = &self.settings.token {
            request = request.header("authorization", format!("Bearer {token}"));
        }

        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            // The body is somebody else's server talking. Truncated, because
            // it ends up in a table a person reads.
            let text: String = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            anyhow::bail!("the webhook answered {status}: {text}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_url_means_no_webhook_and_not_an_error() {
        temp_env::with_var("TREFF_WEBHOOK_URL", None::<&str>, || {
            assert!(Settings::from_env().expect("ok").is_none());
        });
    }

    /// A URL that is not a URL is a configuration mistake, and it is caught at
    /// startup rather than at the first notification — which is hours later
    /// and in a journal nobody is reading.
    #[test]
    fn something_that_is_not_a_url_is_refused() {
        temp_env::with_var("TREFF_WEBHOOK_URL", Some("ntfy.example.org/treff"), || {
            let err = Settings::from_env().expect_err("must refuse");
            assert!(format!("{err}").contains("http(s)"), "{err}");
        });
    }

    #[test]
    fn an_empty_token_file_is_refused_like_a_missing_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("token");
        std::fs::write(&path, "\n\n").expect("write");
        temp_env::with_vars(
            [
                (
                    "TREFF_WEBHOOK_URL",
                    Some("https://ntfy.example.org/".to_string()),
                ),
                ("TREFF_WEBHOOK_TOKEN_FILE", Some(path.display().to_string())),
            ],
            || {
                let err = Settings::from_env().expect_err("must refuse");
                assert!(format!("{err}").contains("blank token"), "{err}");
            },
        );
    }
}
