//! The SMTP transport.
//!
//! Everything about *when* to send is in the parent module and tested without
//! a network. What is here is the message and the connection.

use super::{Message, Transport};
use lettre::message::{MultiPart, SinglePart, header};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

/// What it takes to send. Read once, at startup — a mail server that is
/// misconfigured should say so then, not on the evening somebody replies.
#[derive(Debug, Clone)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    /// The envelope sender, and the address a reply to a notification goes to.
    pub from: String,
    /// STARTTLS on the submission port. Off is only for a test server on
    /// localhost, and `from_env` refuses to turn it off together with
    /// credentials — sending a password in the clear is not a configuration,
    /// it is an accident.
    pub starttls: bool,
}

impl Settings {
    /// From the environment, with the password from a **file** — the same rule
    /// as the OIDC secret, for the same reason: a value in the environment is
    /// in `/proc/<pid>/environ` and in every `systemctl show`.
    ///
    /// Returns `None` when no host is configured. Notifications are then not a
    /// broken feature but an absent one, and treff still runs — which is what
    /// makes the mail part optional for whoever does not want it.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Ok(host) = std::env::var("TREFF_SMTP_HOST") else {
            return Ok(None);
        };
        let port: u16 = std::env::var("TREFF_SMTP_PORT")
            .unwrap_or_else(|_| "587".into())
            .parse()
            .map_err(|_| anyhow::anyhow!("TREFF_SMTP_PORT is not a port number"))?;
        let from = std::env::var("TREFF_SMTP_FROM")
            .map_err(|_| anyhow::anyhow!("TREFF_SMTP_FROM is not set; mail needs a sender"))?;
        let username = std::env::var("TREFF_SMTP_USERNAME")
            .ok()
            .filter(|u| !u.is_empty());
        let password = match std::env::var("TREFF_SMTP_PASSWORD_FILE") {
            Ok(path) => {
                let raw = std::fs::read_to_string(&path)
                    .map_err(|e| anyhow::anyhow!("cannot read {path}: {e}"))?;
                let value = raw.trim().to_string();
                if value.is_empty() {
                    anyhow::bail!("{path} is empty; a blank password is not a password");
                }
                Some(value)
            }
            Err(_) => None,
        };
        let starttls = std::env::var("TREFF_SMTP_STARTTLS")
            .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
            .unwrap_or(true);

        if !starttls && password.is_some() {
            anyhow::bail!(
                "TREFF_SMTP_STARTTLS is off and a password is configured; \
                 sending credentials in the clear is not a configuration"
            );
        }
        Ok(Some(Self {
            host,
            port,
            username,
            password,
            from,
            starttls,
        }))
    }
}

pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: String,
}

impl Mailer {
    pub fn new(settings: &Settings) -> anyhow::Result<Self> {
        let mut builder = if settings.starttls {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&settings.host)?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&settings.host)
        }
        .port(settings.port);

        if let (Some(user), Some(password)) = (&settings.username, &settings.password) {
            builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
        }
        Ok(Self {
            transport: builder.build(),
            from: settings.from.clone(),
        })
    }
}

impl Transport for Mailer {
    async fn deliver(&self, message: &Message) -> anyhow::Result<()> {
        let mut mail = lettre::Message::builder()
            .from(self.from.parse()?)
            .to(message.to.parse()?)
            .subject(format!("{}: {}", message.space_title, message.topic_title))
            .multipart(body_parts(message))
            .map_err(|e| anyhow::anyhow!("cannot build the message: {e}"))?;

        // `List-Unsubscribe` lets a mail client offer the button itself, and
        // that is the difference between somebody unsubscribing and somebody
        // reaching for the spam button — the spam button costs the whole
        // domain, not one subscription.
        //
        // Set after the message is built: these two have no typed counterpart
        // in lettre, and a raw header is the honest way to say so.
        if let Some(url) = &message.unsubscribe {
            let headers = mail.headers_mut();
            headers.insert_raw(header::HeaderValue::new(
                header::HeaderName::new_from_ascii_str("List-Unsubscribe"),
                format!("<{url}>"),
            ));
            headers.insert_raw(header::HeaderValue::new(
                header::HeaderName::new_from_ascii_str("List-Unsubscribe-Post"),
                "List-Unsubscribe=One-Click".to_string(),
            ));
        }

        self.transport.send(mail).await?;
        Ok(())
    }
}

/// Plain text and HTML. Plain first, because that is what a reader without
/// HTML sees, and because a notification is three lines and a link.
fn body_parts(message: &Message) -> MultiPart {
    let text = format!(
        "{} wrote in \"{}\":\n\n{}\n\n{}\n",
        message.author, message.topic_title, message.body, message.link
    );
    let html = format!(
        "<p><b>{}</b> wrote in \u{201c}{}\u{201d}:</p><blockquote>{}</blockquote><p><a href=\"{}\">{}</a></p>",
        escape(&message.author),
        escape(&message.topic_title),
        escape(&message.body),
        escape(&message.link),
        escape(&message.link)
    );
    MultiPart::alternative()
        .singlepart(SinglePart::plain(text))
        .singlepart(SinglePart::html(html))
}

/// Escaping by hand, because the body of a post is somebody else's text and
/// this is the one place it leaves the sanitizer's reach. The Markdown is sent
/// as it was typed — a notification is a nudge, not a rendering.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_without_tls_is_refused() {
        temp_env::with_vars(
            [
                ("TREFF_SMTP_HOST", Some("localhost")),
                ("TREFF_SMTP_FROM", Some("treff@example.org")),
                ("TREFF_SMTP_STARTTLS", Some("0")),
                ("TREFF_SMTP_PASSWORD_FILE", Some("/dev/null")),
            ],
            || {
                // `/dev/null` reads as empty, which is refused first — so use a
                // real file to reach the TLS check.
                let dir = tempfile::tempdir().expect("tempdir");
                let path = dir.path().join("secret");
                std::fs::write(&path, "hunter2").expect("write");
                temp_env::with_var("TREFF_SMTP_PASSWORD_FILE", Some(&path), || {
                    let err = Settings::from_env().expect_err("must refuse");
                    assert!(format!("{err}").contains("in the clear"), "{err}");
                });
            },
        );
    }

    #[test]
    fn an_empty_password_file_is_refused_like_a_missing_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secret");
        std::fs::write(&path, "   \n").expect("write");
        temp_env::with_vars(
            [
                ("TREFF_SMTP_HOST", Some("localhost".to_string())),
                ("TREFF_SMTP_FROM", Some("treff@example.org".to_string())),
                ("TREFF_SMTP_PASSWORD_FILE", Some(path.display().to_string())),
            ],
            || {
                let err = Settings::from_env().expect_err("must refuse");
                assert!(format!("{err}").contains("blank password"), "{err}");
            },
        );
    }

    #[test]
    fn no_host_means_no_mail_and_not_an_error() {
        temp_env::with_var("TREFF_SMTP_HOST", None::<&str>, || {
            assert!(Settings::from_env().expect("ok").is_none());
        });
    }

    #[test]
    fn a_body_is_escaped_on_its_way_into_the_html_part() {
        let m = Message {
            to: "a@example.org".into(),
            space_title: "F".into(),
            topic_title: "T".into(),
            author: "<script>".into(),
            body: "a & b".into(),
            link: "https://example.org/t/1".into(),
            unsubscribe: None,
        };
        // `formatted()` and not `Debug`: what matters is the bytes that leave
        // the process, and quoted-printable encoding is part of them.
        let bytes = body_parts(&m).formatted();
        let formatted = String::from_utf8_lossy(&bytes).replace("=\r\n", "");

        // ONLY THE HTML PART. In `text/plain` a `<script>` is four words and a
        // pair of brackets — escaping it there would show people `&lt;` in
        // their mail reader, which is a bug in the other direction.
        let html = formatted
            .split("Content-Type: text/html")
            .nth(1)
            .expect("an HTML part");
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(html.contains("a &amp; b"), "{html}");
        assert!(
            !html.contains("<script>"),
            "somebody else's text is the one thing that must not arrive as markup: {html}"
        );

        // And the plain part carries it as typed, because that is what plain
        // means.
        let plain = formatted
            .split("Content-Type: text/plain")
            .nth(1)
            .expect("a plain part");
        assert!(plain.contains("<script> wrote"), "{plain}");
    }
}
