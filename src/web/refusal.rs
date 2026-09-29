//! Refusals, told to the log in one fixed shape (homeserver audit 3, B119).
//!
//! Until 0.11.0 a refusal was an answer and nothing else: a wrong token at
//! the internal door, a form from another site, a forged cookie — each got
//! its 401 or 403, and the log said nothing. Somebody guessing the SCIM token
//! from a neighbouring machine would have been heard by nobody.
//!
//! Now every refusal carries a [`Refused`] in its response extensions, and
//! one middleware ([`log_refusals`], on both routers) writes one line for it:
//!
//! ```text
//! treff: refused kind=<kind> status=<code> method=<method> host=<host> path=<path> reason=<reason>
//! ```
//!
//! **Always all six fields, always in this order, never quoted.** A log
//! query can then take the line apart with one pattern rather than guess.
//! `kind` and `reason` come from a fixed vocabulary here; `host` and `path`
//! come from the request and are cut down to a harmless alphabet first
//! ([`field`]), so a request cannot write a second field, a second line or a
//! quote into the log. An absent value is `-`.
//!
//! What never goes in: a token, a cookie, a query string, a session id, a
//! user name. The line says THAT something was refused and where — who was
//! refused is for the proxy's log, which knows the address.

use axum::extract::Request;
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;

/// What every line starts with — the one fixed string a log query filters on.
pub const PREFIX: &str = "treff: refused ";

/// What was refused. The words are part of the log format: renaming one
/// breaks the alert that filters on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A `Host` treff does not serve.
    Host,
    /// A change asked for from another site (`Sec-Fetch-Site`).
    Csrf,
    /// A session cookie that does not decrypt: forged, damaged, or from
    /// before the cookie key changed.
    Cookie,
    /// A cookie that decrypts to a session which is over — expired, signed
    /// out, or its account switched off.
    Session,
    /// A sign-in that did not complete.
    Login,
    /// A wrong or missing bearer token at the internal door.
    Token,
    /// Signed in, and not allowed.
    Forbidden,
    /// One live stream too many for one person.
    Streams,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Host => "host",
            Kind::Csrf => "csrf",
            Kind::Cookie => "cookie",
            Kind::Session => "session",
            Kind::Login => "login",
            Kind::Token => "token",
            Kind::Forbidden => "forbidden",
            Kind::Streams => "streams",
        }
    }
}

/// The mark a refusing response carries. `reason` is a fixed word, never
/// something from the request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused {
    pub kind: Kind,
    pub reason: &'static str,
}

/// Left on a response once its line has been written — so a test can tell
/// "refused and logged" from "refused, and the middleware was never mounted".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Logged;

/// Marks a response as a refusal.
pub fn mark(mut response: Response, kind: Kind, reason: &'static str) -> Response {
    response.extensions_mut().insert(Refused { kind, reason });
    response
}

/// A value from the request, made safe for one logfmt field: at most 128
/// characters out of `[A-Za-z0-9._~:/%+@-]`, everything else as `?`, and
/// `-` for nothing at all.
pub fn field(raw: &str) -> String {
    let safe: String = raw
        .chars()
        .take(128)
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._~:/%+@-".contains(c) {
                c
            } else {
                '?'
            }
        })
        .collect();
    if safe.is_empty() { "-".into() } else { safe }
}

/// The line, without writing it. `status` is `None` where there is no answer
/// to count — a live stream that ends because its session did.
pub fn line(refused: Refused, status: Option<u16>, method: &str, host: &str, path: &str) -> String {
    format!(
        "{PREFIX}kind={} status={} method={} host={} path={} reason={}",
        refused.kind.as_str(),
        status.map_or_else(|| "-".to_string(), |s| s.to_string()),
        field(method),
        field(host),
        field(path),
        field(refused.reason),
    )
}

/// Writes the line to stderr, which is the journal under systemd.
pub fn log(refused: Refused, status: Option<u16>, method: &str, host: &str, path: &str) {
    eprintln!("{}", line(refused, status, method, host, path));
}

/// The middleware: one line for every response that carries a [`Refused`].
pub async fn log_refusals(request: Request, next: Next) -> Response {
    let method = request.method().as_str().to_string();
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();
    // The path only: a query string can carry a code, a state, a token.
    let path = request.uri().path().to_string();
    let mut response = next.run(request).await;
    if let Some(refused) = response.extensions().get::<Refused>().copied() {
        log(
            refused,
            Some(response.status().as_u16()),
            &method,
            &host,
            &path,
        );
        response.extensions_mut().insert(Logged);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE FORMAT IS A CONTRACT with a log query somewhere else. This test is
    /// the one place that spells it out; change it only together with the
    /// query.
    #[test]
    fn the_line_has_six_fields_in_a_fixed_order() {
        let l = line(
            Refused {
                kind: Kind::Token,
                reason: "scim",
            },
            Some(401),
            "GET",
            "",
            "/scim/v2/Users",
        );
        assert_eq!(
            l,
            "treff: refused kind=token status=401 method=GET host=- path=/scim/v2/Users reason=scim"
        );
        let l = line(
            Refused {
                kind: Kind::Session,
                reason: "stream",
            },
            None,
            "GET",
            "forum.example.org",
            "/notifications/stream",
        );
        assert_eq!(
            l,
            "treff: refused kind=session status=- method=GET host=forum.example.org \
             path=/notifications/stream reason=stream"
        );
    }

    /// A request writes the host and the path. It must not be able to write
    /// a field of its own, a second line or a quote.
    #[test]
    fn what_the_request_writes_cannot_break_the_line() {
        let l = line(
            Refused {
                kind: Kind::Host,
                reason: "unknown",
            },
            Some(403),
            "GET",
            "evil.example kind=token\ntreff: refused kind=csrf \"x\"",
            &format!("/{}", "a".repeat(500)),
        );
        assert_eq!(l.lines().count(), 1, "{l}");
        assert_eq!(l.matches("kind=").count(), 1, "{l}");
        assert!(!l.contains('"'), "{l}");
        assert_eq!(l.split(' ').count(), 8, "prefix (2) + six fields: {l}");
        let path = l
            .split(' ')
            .find_map(|f| f.strip_prefix("path="))
            .expect("path");
        assert_eq!(path.len(), 128, "cut to 128");
    }
}
