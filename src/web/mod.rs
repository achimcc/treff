//! The web frame: which host reaches which space, who is let in at all, and
//! what every answer carries. No page is built here — this is the frame each
//! later page passes through, and it is the layer that fails closed.

pub mod internal;
pub mod views;

use crate::auth::{OidcSettings, Sessions};
use crate::authz::Identity;
use crate::config::{Config, Space};
use crate::db::Db;
use axum::Router;
use axum::extract::{FromRef, FromRequestParts, Request, State};
use axum::http::{HeaderValue, StatusCode, header, request::Parts};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum_extra::extract::cookie::{Cookie, Key, PrivateCookieJar, SameSite};
use std::path::Path;
use std::sync::Arc;

/// No inline anything, no third party, nothing framed. The stylesheet and the
/// one script (`mention.js`, ADR 0005) are served as their own routes so that
/// `style-src 'self'` and `script-src 'self'` can stay honest.
pub const CSP: &str = "default-src 'self'; img-src 'self'; style-src 'self'; \
                       script-src 'self'; object-src 'none'; frame-ancestors 'none'; \
                       base-uri 'none'; form-action 'self'";

pub const SESSION_COOKIE: &str = "treff_session";

/// Where the provider must send people back to for a given space.
///
/// DERIVED FROM THE HOST, NOT CONFIGURED. One process serves several
/// addresses, and a cookie belongs to exactly one of them — the short-lived
/// cookie carrying `state`, `nonce` and the PKCE verifier is set on the host
/// the sign-in began at, and is never sent to another. A single configured
/// redirect URI therefore worked for exactly one space and left the others in
/// a loop; on 2026-09-06, on the first host to run this, it left them in a
/// **404**, because the route was not mounted at all.
///
/// So each space comes back to itself, and the operator registers one redirect
/// URI per space with the provider. HTTPS is not a choice here: the session
/// cookie is `Secure`, so the site is served over TLS or not at all.
pub fn redirect_uri_for(host: &str) -> String {
    format!("https://{host}/auth/callback")
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Db,
    pub oidc: Arc<OidcSettings>,
    /// The redirect URI the OIDC client is built with — the first space's.
    /// Every request overrides it with the one belonging to ITS space
    /// (`redirect_uri_for`), so this value never reaches a provider on its
    /// own; the client type simply requires one.
    pub redirect_uri: Arc<str>,
    /// The discovered provider, found on first use rather than at startup.
    ///
    /// Discovering at startup made a slow identity provider into a dead
    /// forum: after a power cut the two come up in whatever order they come
    /// up in, and a service that gives up because a neighbour was late is
    /// worse than one that waits. Nothing is opened by this — without a
    /// provider nobody can sign in, and every page needs a session.
    provider: Arc<tokio::sync::OnceCell<Arc<crate::auth::oidc::Provider>>>,
    /// Where the database and the attachment files live. Kept because serving
    /// an attachment means reading a file, and the path must come from here
    /// rather than from anything a request carries.
    pub data_dir: std::path::PathBuf,
    cookie_key: Key,
    /// Signs unsubscribe links. Its own key, never the cookie key — see
    /// `notify::unsubscribe`.
    pub unsubscribe_key: std::sync::Arc<Vec<u8>>,
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.clone()
    }
}

/// The key that signs and encrypts the session cookie.
///
/// It lives in the data directory rather than in memory: a key generated per
/// start would sign everyone out on every restart, even though their sessions
/// are still in the database. Mode 0600, because it is a credential.
pub fn load_or_create_cookie_key(dir: &Path) -> anyhow::Result<Key> {
    let path = dir.join("cookie.key");
    if let Ok(bytes) = std::fs::read(&path)
        && bytes.len() >= 64
    {
        return Ok(Key::from(&bytes));
    }

    let key = Key::generate();
    std::fs::write(&path, key.master())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(key)
}

impl AppState {
    pub fn new(config: Config, db: Db, oidc: OidcSettings, dir: &Path) -> anyhow::Result<Self> {
        // NOT CONFIGURED, DERIVED. Until 0.2.0 the redirect URI was an
        // environment variable — one value for a process that serves several
        // addresses, which could only ever be right for one of them. The
        // client needs *a* URI to be built with; every request replaces it
        // with its own space's.
        let redirect_uri = config
            .spaces
            .first()
            .map(|s| redirect_uri_for(&s.host))
            .ok_or_else(|| anyhow::anyhow!("no spaces are configured"))?;
        Ok(Self {
            config: Arc::new(config),
            db,
            oidc: Arc::new(oidc),
            redirect_uri: Arc::from(redirect_uri.as_str()),
            provider: Arc::new(tokio::sync::OnceCell::new()),
            data_dir: dir.to_path_buf(),
            cookie_key: load_or_create_cookie_key(dir)?,
            unsubscribe_key: std::sync::Arc::new(crate::notify::unsubscribe::load_or_create_key(
                dir,
            )?),
        })
    }

    /// The provider, discovered once and kept. A failure is not remembered:
    /// the next sign-in tries again, which is what makes a provider that was
    /// merely slow recoverable without a restart.
    pub async fn provider(&self) -> anyhow::Result<Arc<crate::auth::oidc::Provider>> {
        if let Some(p) = self.provider.get() {
            return Ok(p.clone());
        }
        let discovered =
            Arc::new(crate::auth::oidc::Provider::discover(&self.oidc, &self.redirect_uri).await?);
        // A race here is harmless: both sides discovered the same provider.
        let _ = self.provider.set(discovered.clone());
        Ok(discovered)
    }
}

/// The space is decided by the `Host` header, and an unknown host is refused —
/// not mapped to the first space. Both addresses land in ONE process; without
/// this rule the separation between two audiences would be a guess.
pub struct CurrentSpace(pub Arc<Space>);

impl<S> FromRequestParts<S> for CurrentSpace
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app = AppState::from_ref(state);
        space_for(&app, parts)
            .map(CurrentSpace)
            .ok_or_else(|| (StatusCode::FORBIDDEN, "unknown host").into_response())
    }
}

fn space_for(app: &AppState, parts: &Parts) -> Option<Arc<Space>> {
    let host = parts
        .headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    app.config.space_for_host(host).map(|s| Arc::new(s.clone()))
}

/// The signed-in person, or nothing. Reading it never fails loudly: an absent,
/// forged or expired cookie is simply "not signed in".
pub struct CurrentUser(pub Identity);

impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app = AppState::from_ref(state);
        match identity_from_cookies(&app, parts).await {
            Some(id) => Ok(CurrentUser(id)),
            None => Err(login_redirect(parts)),
        }
    }
}

async fn identity_from_cookies(app: &AppState, parts: &Parts) -> Option<Identity> {
    let jar = PrivateCookieJar::from_headers(&parts.headers, app.cookie_key.clone());
    let sid = jar.get(SESSION_COOKIE)?;
    Sessions::load(&app.db, sid.value()).await.ok().flatten()
}

/// Everything that is not the sign-in itself or a static asset needs a
/// session. The gate sits in front of the routes, so a page added later cannot
/// forget it.
async fn require_session(State(app): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    let (parts, body) = request.into_parts();

    if space_for(&app, &parts).is_none() {
        return (StatusCode::FORBIDDEN, "unknown host").into_response();
    }

    if !same_origin_enough(&parts) {
        return (StatusCode::FORBIDDEN, "that came from somewhere else").into_response();
    }

    // `/u/` is open BY DESIGN: an unsubscribe link that asks for a sign-in is
    // not an unsubscribe link. It carries its own proof (an HMAC), so being
    // open costs nothing that the token does not already guard.
    let open =
        path.starts_with("/auth/") || path.starts_with("/assets/") || path.starts_with("/u/");
    if !open && identity_from_cookies(&app, &parts).await.is_none() {
        return login_redirect(&parts);
    }

    next.run(Request::from_parts(parts, body)).await
}

/// The second line of defence against CSRF, and the first one that is ours.
///
/// The first is `SameSite=Lax` on the session cookie. It works, and it works
/// in the browser — the server itself asked nothing, so an audit on
/// 2026-09-15 could post a reply from a foreign origin and have it appear.
/// Here the server asks.
///
/// **`Sec-Fetch-Site`, not `Origin` or `Referer`.** This site sends
/// `Referrer-Policy: no-referrer` on purpose, so there is often no referrer
/// to read; the fetch metadata a browser attaches is not something the page
/// doing the asking can set or suppress. And it needs no host comparison:
/// the browser has already done it.
///
/// Three decisions worth writing down:
///
/// * **Only methods that change something are asked.** A GET changes nothing
///   here — since 2026-09-20 that includes `/auth/logout` — and a link in a
///   mail or a chat must keep working.
/// * **`same-site` is refused like `cross-site`.** Every space is its own
///   host with its own groups, so a page served by a neighbouring space is
///   as foreign as any other site.
/// * **No header at all is let through.** CSRF borrows a BROWSER's cookies,
///   and every browser that can be borrowed from sends these headers.
///   Something that does not send them (a script, `curl`, a browser older
///   than the header) has no cookies to borrow, so refusing it would cost
///   reachability and buy nothing.
fn same_origin_enough(parts: &Parts) -> bool {
    if matches!(
        parts.method,
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    ) {
        return true;
    }
    match parts.headers.get("sec-fetch-site") {
        None => true,
        Some(site) => site.as_bytes() == b"same-origin",
    }
}

/// The redirect to the sign-in, carrying the page that asked for it.
///
/// A link to a topic must lead THERE after the sign-in, not to the front
/// page; until 0.3.7 it did the latter, and a link sent to someone without a
/// session was a link to the front page with an extra step. Only a GET is
/// remembered: a form posted after the session ran out cannot be replayed by
/// the GET that follows the callback.
fn login_redirect(parts: &Parts) -> Response {
    let page = (parts.method == axum::http::Method::GET)
        .then(|| parts.uri.path_and_query().map(|pq| pq.as_str()))
        .flatten();
    match page {
        Some(p) if safe_return_path(Some(p)) != "/" => {
            let encoded =
                percent_encoding::utf8_percent_encode(p, percent_encoding::NON_ALPHANUMERIC);
            Redirect::to(&format!("/auth/login?next={encoded}")).into_response()
        }
        _ => Redirect::to("/auth/login").into_response(),
    }
}

/// Where a sign-in returns to. `next` comes from a URL, so anyone can write
/// it: only a path on this site is followed. Anything that would leave the
/// site (a scheme, a protocol-relative `//host`, a backslash a browser reads
/// as one), land on the sign-in again (a loop), or break the cookie it is
/// stored in (a line break) falls back to the front page.
fn safe_return_path(raw: Option<&str>) -> String {
    match raw {
        Some(p)
            if p.starts_with('/')
                && !p.starts_with("//")
                && !p.starts_with("/\\")
                && !p.starts_with("/auth/")
                && !p.chars().any(char::is_control) =>
        {
            p.to_string()
        }
        _ => "/".to_string(),
    }
}

/// What the login accepts from its own redirect.
#[derive(serde::Deserialize)]
struct LoginQuery {
    next: Option<String>,
}

async fn login(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    axum::extract::Query(query): axum::extract::Query<LoginQuery>,
) -> Response {
    let provider = match app.provider().await {
        Ok(p) => p,
        Err(e) => {
            // Saying so is the honest answer; redirecting to ourselves would
            // be a loop, and pretending to be signed in would be worse.
            tracing_error("the identity provider could not be reached", &e);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "the identity provider cannot be reached right now",
            )
                .into_response();
        }
    };

    match provider.begin_login(&redirect_uri_for(&space.host)) {
        Ok(start) => {
            let return_to = safe_return_path(query.next.as_deref());
            let jar = PrivateCookieJar::new(app.cookie_key.clone())
                .add(pending_cookie(&start, &return_to));
            (jar, Redirect::to(&start.url)).into_response()
        }
        Err(e) => {
            tracing_error("could not start a sign-in", &e);
            (
                StatusCode::BAD_GATEWAY,
                "the identity provider did not answer",
            )
                .into_response()
        }
    }
}

fn pending_cookie(start: &crate::auth::oidc::LoginStart, return_to: &str) -> Cookie<'static> {
    // state, nonce, the PKCE verifier and the page to return to travel in one
    // short-lived private cookie. They never appear in a URL — the page is
    // checked by `safe_return_path` BEFORE it goes in, so the callback can
    // follow it without looking twice.
    let value = format!(
        "{}\n{}\n{}\n{}",
        start.pending.state, start.pending.nonce, start.pending.pkce_verifier, return_to
    );
    let mut c = Cookie::new("treff_pending", value);
    c.set_http_only(true);
    c.set_secure(true);
    c.set_same_site(SameSite::Lax);
    c.set_path("/auth");
    c.set_max_age(time::Duration::minutes(10));
    c
}

/// What the provider appends to the redirect URI.
#[derive(serde::Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    /// The provider can answer with a refusal instead of a code. Saying so is
    /// better than a blank page that looks like a bug in treff.
    error: Option<String>,
}

/// The other half of `login`, and the half that was missing until 2026-09-06:
/// implemented, unit-tested against a mock provider, and never mounted. The
/// provider sent people back here and the router answered 404.
async fn callback(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    jar: PrivateCookieJar,
    axum::extract::Query(query): axum::extract::Query<CallbackQuery>,
) -> Response {
    if let Some(error) = query.error {
        // The provider's own words are not repeated back into the page; they
        // are its vocabulary, not ours, and they end up in a log people read.
        eprintln!("treff: the provider refused a sign-in: {error}");
        return (StatusCode::FORBIDDEN, "the provider refused the sign-in").into_response();
    }

    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        return (StatusCode::BAD_REQUEST, "not a callback").into_response();
    };

    // No cookie, nothing to compare `state` against. That is a refusal and not
    // a new sign-in: silently starting one here would make an unsolicited
    // callback indistinguishable from a real one.
    let Some((pending, return_to)) = jar
        .get("treff_pending")
        .and_then(|c| parse_pending(c.value()))
    else {
        return (
            StatusCode::BAD_REQUEST,
            "this sign-in was not started here, or it took too long",
        )
            .into_response();
    };

    let provider = match app.provider().await {
        Ok(p) => p,
        Err(e) => {
            tracing_error("the identity provider could not be reached", &e);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "the identity provider cannot be reached right now",
            )
                .into_response();
        }
    };

    let identity = match provider
        .finish_login(
            pending,
            &code,
            &returned_state,
            &redirect_uri_for(&space.host),
        )
        .await
    {
        Ok(i) => i,
        Err(e) => {
            tracing_error("a sign-in could not be completed", &e);
            return (StatusCode::FORBIDDEN, "the sign-in could not be completed").into_response();
        }
    };

    let session = match Sessions::create(&app.db, &identity).await {
        Ok(s) => s,
        Err(e) => return server_error("cannot store the session", &e),
    };

    // The pending cookie is spent. Removing it needs the same path it was set
    // with, or the browser keeps a second one alongside.
    let mut spent = Cookie::new("treff_pending", "");
    spent.set_path("/auth");
    let jar = jar.remove(spent).add(session_cookie(session));
    (jar, Redirect::to(&return_to)).into_response()
}

/// The cookie's three secrets and the page to return to. A cookie from
/// before 0.3.7 has three lines and no page; a sign-in that was in flight
/// during the upgrade still completes, on the front page.
fn parse_pending(value: &str) -> Option<(crate::auth::oidc::PendingLogin, String)> {
    let mut parts = value.split('\n');
    let state = parts.next()?.to_string();
    let nonce = parts.next()?.to_string();
    let pkce_verifier = parts.next()?.to_string();
    let return_to = safe_return_path(parts.next());
    if parts.next().is_some() || state.is_empty() || nonce.is_empty() || pkce_verifier.is_empty() {
        return None;
    }
    Some((
        crate::auth::oidc::PendingLogin {
            state,
            nonce,
            pkce_verifier,
        },
        return_to,
    ))
}

/// Signing out ends the session IN THE DATABASE, not only in the browser.
/// Dropping the cookie alone would leave a working session behind for anyone
/// who kept a copy of it.
///
/// **A POST, because it changes something.** Until 2026-09-20 a bare `GET`
/// ended the session, so anything that merely FETCHES a link ended it too: a
/// mail client collecting previews, a chat unfurling a pasted address, an
/// `<img src>` on any page in the world. None of that needs a forged form —
/// the link is the attack, and `SameSite=Lax` does not hold a top-level GET
/// back. It is the same reason `/t/{id}/follow` has been a POST since it was
/// written; this was the one route that had been forgotten.
async fn logout(State(app): State<AppState>, jar: PrivateCookieJar) -> Response {
    if let Some(id) = jar.get(SESSION_COOKIE)
        && let Err(e) = Sessions::destroy(&app.db, id.value()).await
    {
        return server_error("cannot end the session", &e);
    }
    let mut spent = Cookie::new(SESSION_COOKIE, "");
    spent.set_path("/");
    (jar.remove(spent), Redirect::to("/")).into_response()
}

/// Follow and unfollow. `POST`, because they change something — a `GET` that
/// changes state is one a link preview can trigger, and a mail client that
/// fetches links would unsubscribe people who never clicked.
///
/// The right is checked, not assumed: a subscription to a topic you may not
/// read would be a mail about a room you cannot enter.
async fn follow(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    change_subscription(&app, &space, &who, id, true).await
}

async fn unfollow(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    change_subscription(&app, &space, &who, id, false).await
}

async fn change_subscription(
    app: &AppState,
    space: &Space,
    who: &Identity,
    id: i64,
    on: bool,
) -> Response {
    if !crate::authz::may_read(who, space) {
        return forbidden();
    }
    // Loaded through the space, so a topic at the other address is not there
    // rather than refused — the same rule as reading it.
    match crate::db::topics::load_topic(&app.db, &space.host, id).await {
        Ok(None) => return not_found(),
        Ok(Some(_)) => {}
        Err(e) => return server_error("cannot load a topic", &e),
    }

    let done = if on {
        crate::db::subscriptions::follow(&app.db, &who.subject, id).await
    } else {
        crate::db::subscriptions::unfollow(&app.db, &who.subject, id).await
    };
    match done {
        Ok(()) => Redirect::to(&format!("/t/{id}")).into_response(),
        Err(e) => server_error("cannot change a subscription", &e),
    }
}

#[derive(serde::Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
}

/// Search, restricted to this space and to what this person may read.
///
/// THE CATEGORY LIST IS THE PERMISSION CHECK, and it is computed here rather
/// than trusted from the request: a hit list that shows a title out of a
/// category somebody may not enter is a leak, and it is the kind that looks
/// like a feature until somebody notices.
async fn search_page(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    lang: crate::i18n::Lang,
    axum::extract::Query(query): axum::extract::Query<SearchQuery>,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }
    // Reading a space is decided per space; the categories then decide what is
    // visible inside it. Someone who may read the space may read its
    // categories — `post` and `reply` are the rights that differ.
    let categories: Vec<String> = space.categories.iter().map(|c| c.slug.clone()).collect();

    match crate::db::search::search(&app.db, &space.host, &categories, &query.q, PAGE_SIZE).await {
        Ok(hits) => {
            let bell = bell(&app, &space, &who).await;
            crate::web::views::search_page(&space, &who, lang, bell, &query.q, &hits)
                .into_response()
        }
        Err(e) => server_error("cannot search", &e),
    }
}

/// The unsubscribe page — reachable WITHOUT SIGNING IN, and that is the whole
/// point. Behind a sign-in this is not an unsubscribe link, it is a sign-in
/// link, and the person who wanted it to stop reaches for the spam button
/// instead. That costs the domain, not the subscription.
///
/// `GET` only shows a button. The `POST` below does the work, so that a link
/// preview or a mail client fetching URLs cannot unsubscribe anybody.
async fn unsubscribe_page(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    lang: crate::i18n::Lang,
    axum::extract::Path((id, token)): axum::extract::Path<(i64, String)>,
) -> Response {
    if !crate::notify::unsubscribe::verify(&app.unsubscribe_key, id, &token) {
        // Same answer as a row that does not exist: whether a link is merely
        // old or was never real is not something to hand out.
        return not_found();
    }
    // The question names what the button will do — stop one topic, or stop
    // mails about mentions. A row that is gone asks the topic question, which
    // is the one this page always asked.
    let mention = match crate::db::outbox::who_and_what(&app.db, id).await {
        Ok(found) => found.is_some_and(|(_, _, reason)| reason == "mention"),
        Err(e) => return server_error("cannot look up a notification", &e),
    };
    crate::web::views::unsubscribe_page(&space, lang, id, &token, mention).into_response()
}

/// Cancels ONE subscription. Never all of them: somebody who is done with one
/// noisy topic has not asked to stop hearing about everything, and a link that
/// did that would be a surprise nobody can undo.
///
/// Idempotent, because a link in a mail gets clicked twice — and because
/// `List-Unsubscribe-Post` means a mail client may send this without anybody
/// clicking at all.
async fn unsubscribe_now(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    lang: crate::i18n::Lang,
    axum::extract::Path((id, token)): axum::extract::Path<(i64, String)>,
) -> Response {
    if !crate::notify::unsubscribe::verify(&app.unsubscribe_key, id, &token) {
        return not_found();
    }
    match crate::db::outbox::who_and_what(&app.db, id).await {
        // A MENTION'S LINK TURNS OFF MENTION MAILS, not a subscription: the
        // person was never following the topic, and unfollowing it would
        // leave them exactly as mailed as before.
        Ok(Some((subject, _, reason))) if reason == "mention" => {
            if let Err(e) = crate::db::accounts::set_mention_mail(&app.db, &subject, false).await {
                return server_error("cannot turn off mention mails", &e);
            }
            return crate::web::views::unsubscribed_page(&space, lang, true).into_response();
        }
        Ok(Some((subject, topic_id, _))) => {
            if let Err(e) = crate::db::subscriptions::unfollow(&app.db, &subject, topic_id).await {
                return server_error("cannot unsubscribe", &e);
            }
        }
        // The row is gone, or the topic is. Nothing to do, and the answer is
        // the same one a successful click gets — the alternative tells a
        // stranger whether a subscription existed.
        Ok(None) => {}
        Err(e) => return server_error("cannot look up a notification", &e),
    }
    crate::web::views::unsubscribed_page(&space, lang, false).into_response()
}

/// The number on the bell for this person in this space.
///
/// A count that cannot be read is logged and shown as no number rather than
/// failing the page: the page is what the person came for, and the bell is a
/// hint about other pages. The list behind it fails loudly on its own.
async fn bell(app: &AppState, space: &Space, who: &Identity) -> crate::web::views::Bell {
    match crate::db::inbox::unread_count(&app.db, &who.subject, &space.host).await {
        Ok(unread) => crate::web::views::Bell { unread },
        Err(e) => {
            tracing_error("cannot count unread notifications", &e);
            crate::web::views::Bell::default()
        }
    }
}

/// How many lines the notifications page shows. Enough for a week away; a
/// list longer than that is not read, it is scrolled past.
const INBOX_PAGE: i64 = 50;

async fn notifications(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    lang: crate::i18n::Lang,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }
    let entries =
        match crate::db::inbox::entries(&app.db, &who.subject, &space.host, INBOX_PAGE).await {
            Ok(e) => e,
            Err(e) => return server_error("cannot list notifications", &e),
        };
    let mention_mail = match crate::db::accounts::wants_mention_mail(&app.db, &who.subject).await {
        Ok(on) => on,
        Err(e) => return server_error("cannot read a setting", &e),
    };
    let bell = bell(&app, &space, &who).await;
    crate::web::views::notifications_page(&space, &who, lang, bell, &entries, mention_mail)
        .into_response()
}

/// The suggestion list for `mention.js` (ADR 0005). Per person and per
/// space, so nothing may keep it.
async fn mentionable(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }
    match crate::mentions::offered(&app.db, &space, &who.subject).await {
        Ok(list) => ([(header::CACHE_CONTROL, "no-store")], axum::Json(list)).into_response(),
        Err(e) => server_error("cannot list who may be mentioned", &e),
    }
}

#[derive(serde::Deserialize)]
pub struct MentionMail {
    on: String,
}

/// The switch on `/notifications`, and the way back after the one-click link
/// in a mention mail turned mention mails off.
async fn set_mention_mail(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    axum::extract::Form(form): axum::extract::Form<MentionMail>,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }
    // Exactly `1` or `0`; anything else is not a guess at what was meant.
    let on = match form.on.as_str() {
        "1" => true,
        "0" => false,
        _ => return bad_request("on is 1 or 0"),
    };
    match crate::db::accounts::set_mention_mail(&app.db, &who.subject, on).await {
        Ok(()) => Redirect::to("/notifications").into_response(),
        Err(e) => server_error("cannot change a setting", &e),
    }
}

/// How many entries the overlay under the bell shows. The rest is one click
/// further, on `/notifications`.
const OVERLAY_ENTRIES: i64 = 12;

/// The overlay's contents without a stream — for a browser whose stream did
/// not come up.
async fn notifications_json(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }
    match crate::live::bell_json(&app.db, &who.subject, &space.host, "", OVERLAY_ENTRIES).await {
        Ok(body) => ([(header::CACHE_CONTROL, "no-store")], axum::Json(body)).into_response(),
        Err(e) => server_error("cannot read the bell", &e),
    }
}

/// The bell in the header, live (`bell.js`). The person is the one this
/// stream was opened by; a session that ends while it is open is noticed at
/// the next reconnect, like everywhere else a page stays open.
async fn notifications_stream(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }
    let db = app.db.clone();
    let host = space.host.clone();
    let subject = who.subject.clone();
    let stream = crate::live::bell_stream(&app.db, space.host.clone(), move || {
        let db = db.clone();
        let host = host.clone();
        let subject = subject.clone();
        async move { crate::live::bell_json(&db, &subject, &host, "", OVERLAY_ENTRIES).await }
    });
    ([(header::CACHE_CONTROL, "no-store")], stream).into_response()
}

/// Following an event: read, then on — to the link that was checked when the
/// event came in (`db::events::checked`), never to one from this request.
async fn open_event(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }
    match crate::db::events::open(&app.db, &who.subject, &space.host, id).await {
        Ok(Some(Some(link))) => Redirect::to(&link).into_response(),
        Ok(Some(None)) => Redirect::to("/notifications").into_response(),
        Ok(None) => not_found(),
        Err(e) => server_error("cannot open an event", &e),
    }
}

async fn read_all(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }
    match crate::db::inbox::mark_all_read(&app.db, &who.subject, &space.host).await {
        Ok(()) => Redirect::to("/notifications").into_response(),
        Err(e) => server_error("cannot mark notifications read", &e),
    }
}

fn tracing_error(what: &str, e: &anyhow::Error) {
    eprintln!("treff: {what}: {e}");
}

/// The cookie that carries a session. Same function in production and in the
/// tests, so a test that gets in proves a browser would.
pub fn session_cookie(session_id: String) -> Cookie<'static> {
    let mut c = Cookie::new(SESSION_COOKIE, session_id);
    c.set_http_only(true);
    c.set_secure(true);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "not for you").into_response()
}

/// A refusal that says as little as possible: through this address the thing
/// does not exist, and the answer must not hint otherwise.
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn server_error(what: &str, e: &anyhow::Error) -> Response {
    tracing_error(what, e);
    (StatusCode::INTERNAL_SERVER_ERROR, "something went wrong").into_response()
}

const PAGE_SIZE: i64 = 50;

/// The point at which a request is refused before it is read into memory. Not
/// the user-facing limit — that one is per space and configurable.
const HARD_UPLOAD_LIMIT: usize = 32 * 1024 * 1024;

/// The front page of a space.
///
/// A timeline goes straight to its entries — a blog with one category has
/// nothing to choose from, and a page in between would only stand between the
/// reader and the text. A forum lists its categories instead of jumping into
/// whichever one the configuration happens to name first.
async fn space_index(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    lang: crate::i18n::Lang,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }

    match space.view {
        crate::config::View::Timeline => {
            let Some(category) = space.categories.first() else {
                return not_found();
            };
            render_space(&app, &space, &who, lang, &category.slug.clone()).await
        }
        crate::config::View::Topics => {
            let slugs: Vec<String> = space.categories.iter().map(|c| c.slug.clone()).collect();
            let counts =
                match crate::db::topics::category_counts(&app.db, &space.host, &slugs).await {
                    Ok(c) => c,
                    Err(e) => return server_error("cannot count categories", &e),
                };
            // In the order of the configuration, not of the query: the person
            // who wrote the file decided what comes first.
            let rows: Vec<_> = space
                .categories
                .iter()
                .map(|c| (c, counts.get(&c.slug).cloned().unwrap_or_default()))
                .collect();
            let bell = bell(&app, &space, &who).await;
            crate::web::views::category_index(&space, &who, lang, bell, &rows).into_response()
        }
    }
}

async fn space_category(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    lang: crate::i18n::Lang,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> Response {
    if space.category(&slug).is_none() {
        return not_found();
    }
    render_space(&app, &space, &who, lang, &slug).await
}

async fn render_space(
    app: &AppState,
    space: &Space,
    who: &Identity,
    lang: crate::i18n::Lang,
    slug: &str,
) -> Response {
    if !crate::authz::may_read(who, space) {
        return forbidden();
    }

    let topics =
        match crate::db::topics::list_topics(&app.db, &space.host, slug, PAGE_SIZE, 0).await {
            Ok(t) => t,
            Err(e) => return server_error("cannot list topics", &e),
        };

    // A timeline shows the bodies, so it needs the opening post of each topic.
    // A topic list does not, and does not ask for them.
    let mut rows = Vec::with_capacity(topics.len());
    for (topic, last) in topics {
        let first = if space.view == crate::config::View::Timeline {
            match crate::db::topics::load_topic(&app.db, &space.host, topic.id).await {
                Ok(Some((_, posts))) => posts.into_iter().next(),
                Ok(None) => None,
                Err(e) => return server_error("cannot load a topic", &e),
            }
        } else {
            None
        };
        rows.push(crate::web::views::TopicRow { topic, last, first });
    }

    let bell = bell(app, space, who).await;
    crate::web::views::space_page(space, who, lang, bell, space.category(slug), &rows)
        .into_response()
}

async fn topic_page(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    lang: crate::i18n::Lang,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }
    match crate::db::topics::load_topic(&app.db, &space.host, id).await {
        // Scoped by space in the query, so a topic from the other address is
        // simply not there — no 403 that would confirm it exists.
        Ok(None) => not_found(),
        Ok(Some((topic, posts))) => {
            let following =
                match crate::db::subscriptions::is_following(&app.db, &who.subject, topic.id).await
                {
                    Ok(f) => f,
                    Err(e) => return server_error("cannot read a subscription", &e),
                };
            // READ FIRST, COUNT SECOND: this page is where the entries of this
            // topic end, and a bell on it that still counted them would be
            // wrong on the one page where the reader can check.
            if let Err(e) = crate::db::inbox::mark_topic_read(&app.db, &who.subject, topic.id).await
            {
                tracing_error("cannot mark a topic read", &e);
            }
            let bell = bell(&app, &space, &who).await;
            // Highlighted by the same rule that decides who is told — see
            // `mentions` for why that has to be one answer and not two.
            let highlighted = match crate::mentions::highlighted(
                &app.db,
                &space,
                posts.iter().map(|p| p.body_markdown.as_str()),
            )
            .await
            {
                Ok(h) => h,
                Err(e) => return server_error("cannot look up mentions", &e),
            };
            let authors: Vec<String> = posts.iter().map(|p| p.author_subject.clone()).collect();
            let handles = match crate::db::accounts::handles_of(&app.db, &authors).await {
                Ok(h) => h,
                Err(e) => return server_error("cannot look up handles", &e),
            };
            let category = space.category(&topic.category);
            let view = crate::web::views::TopicView {
                category,
                topic: &topic,
                posts: &posts,
                following,
                highlighted: &highlighted,
                handles: &handles,
            };
            crate::web::views::topic_page(&space, &who, lang, bell, view).into_response()
        }
        Err(e) => server_error("cannot load a topic", &e),
    }
}

/// The limits on what may be written. They are checked here, in the handler,
/// because they are about a request — the storage layer stores what it is
/// given.
const MAX_TITLE_CHARS: usize = 200;
const MAX_BODY_BYTES: usize = 64 * 1024;

#[derive(serde::Deserialize)]
pub struct NewTopic {
    title: String,
    body: String,
}

#[derive(serde::Deserialize)]
pub struct NewReply {
    body: String,
}

fn bad_request(why: &'static str) -> Response {
    (StatusCode::BAD_REQUEST, why).into_response()
}

/// Trimmed and within its limits, or the reason why not. These two know
/// nothing about HTTP — the handler turns a reason into a status — which is
/// what lets them be tested as plain functions.
///
/// Whitespace counts as empty: a title of three spaces is not a title.
pub fn checked_title(raw: &str) -> Result<String, &'static str> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("a topic needs a title");
    }
    if t.chars().count() > MAX_TITLE_CHARS {
        return Err("that title is too long");
    }
    Ok(t.to_string())
}

pub fn checked_body(raw: &str) -> Result<String, &'static str> {
    let b = raw.trim();
    if b.is_empty() {
        return Err("a post needs a text");
    }
    if b.len() > MAX_BODY_BYTES {
        return Err("that text is too long");
    }
    Ok(b.to_string())
}

async fn open_topic(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    axum::extract::Path(slug): axum::extract::Path<String>,
    axum::extract::Form(form): axum::extract::Form<NewTopic>,
) -> Response {
    let Some(category) = space.category(&slug) else {
        return not_found();
    };
    // Reading first: someone who cannot see the space has no business
    // discovering which categories it has by posting into them.
    if !crate::authz::may_read(&who, &space) || !crate::authz::may_post(&who, category) {
        return forbidden();
    }

    let title = match checked_title(&form.title) {
        Ok(t) => t,
        Err(why) => return bad_request(why),
    };
    let body = match checked_body(&form.body) {
        Ok(b) => b,
        Err(why) => return bad_request(why),
    };

    let told = match crate::mentions::to_tell(&app.db, &space, &who.subject, &body).await {
        Ok(t) => t,
        Err(e) => return server_error("cannot look up a mention", &e),
    };
    match crate::db::topics::create_topic_mentioning(
        &app.db,
        &space.host,
        &slug,
        &title,
        &body,
        &who,
        &told,
    )
    .await
    {
        Ok(id) => Redirect::to(&format!("/t/{id}")).into_response(),
        Err(e) => server_error("cannot open a topic", &e),
    }
}

async fn reply(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    axum::extract::Path(id): axum::extract::Path<i64>,
    axum::extract::Form(form): axum::extract::Form<NewReply>,
) -> Response {
    // Loading is scoped by space, so a topic from the other address is not
    // found rather than refused — the same answer a reader gets.
    let topic = match crate::db::topics::load_topic(&app.db, &space.host, id).await {
        Ok(Some((topic, _))) => topic,
        Ok(None) => return not_found(),
        Err(e) => return server_error("cannot load a topic", &e),
    };

    let Some(category) = space.category(&topic.category) else {
        // The topic sits in a category the configuration no longer has. It can
        // still be read; nothing new goes into it.
        return not_found();
    };
    if !crate::authz::may_read(&who, &space) || !crate::authz::may_reply(&who, category) {
        return forbidden();
    }

    let body = match checked_body(&form.body) {
        Ok(b) => b,
        Err(why) => return bad_request(why),
    };

    let told = match crate::mentions::to_tell(&app.db, &space, &who.subject, &body).await {
        Ok(t) => t,
        Err(e) => return server_error("cannot look up a mention", &e),
    };
    match crate::db::topics::add_reply_mentioning(&app.db, id, &body, &who, &told).await {
        Ok(_) => Redirect::to(&format!("/t/{id}")).into_response(),
        Err(e) => server_error("cannot add a reply", &e),
    }
}

#[derive(serde::Deserialize)]
pub struct EditPost {
    body: String,
}

async fn edit_post(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    axum::extract::Path(id): axum::extract::Path<i64>,
    axum::extract::Form(form): axum::extract::Form<EditPost>,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }
    let body = match checked_body(&form.body) {
        Ok(b) => b,
        Err(why) => return bad_request(why),
    };

    // The permission is enforced by the query. This handler does not look up
    // the author and compare — it asks for a write that only succeeds if the
    // post is this person's, in this space.
    let told = match crate::mentions::to_tell(&app.db, &space, &who.subject, &body).await {
        Ok(t) => t,
        Err(e) => return server_error("cannot look up a mention", &e),
    };
    match crate::db::topics::update_post_mentioning(&app.db, &space.host, id, &body, &who, &told)
        .await
    {
        Ok(true) => match topic_of_post(&app, &space.host, id).await {
            Some(topic) => Redirect::to(&format!("/t/{topic}")).into_response(),
            None => Redirect::to("/").into_response(),
        },
        Ok(false) => forbidden(),
        Err(e) => server_error("cannot edit a post", &e),
    }
}

/// The page that asks before a post goes.
///
/// It refuses whoever may not press the button rather than leaving that to the
/// POST: display follows the right here as everywhere else, and a question
/// that leads to a 403 is a small lie.
async fn delete_question(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    lang: crate::i18n::Lang,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }
    let post = match crate::db::topics::load_post(&app.db, &space.host, id).await {
        // Scoped by space in the query, so a post from the other address is
        // simply not there.
        Ok(None) => return not_found(),
        Ok(Some(post)) => post,
        Err(e) => return server_error("cannot load a post", &e),
    };
    if !crate::authz::may_modify(&who, &post.author_subject) {
        return forbidden();
    }
    let bell = bell(&app, &space, &who).await;
    crate::web::views::delete_question_page(&space, &who, lang, bell, &post).into_response()
}

async fn delete_post(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }

    let topic = topic_of_post(&app, &space.host, id).await;
    match crate::db::topics::delete_post(&app.db, &space.host, id, &who).await {
        Ok(crate::db::topics::Deleted::Post) => match topic {
            Some(topic) => Redirect::to(&format!("/t/{topic}")).into_response(),
            None => Redirect::to("/").into_response(),
        },
        Ok(crate::db::topics::Deleted::Topic) => Redirect::to("/").into_response(),
        Ok(crate::db::topics::Deleted::HasReplies) => (
            StatusCode::CONFLICT,
            "other people have replied here; deleting this would delete their posts too",
        )
            .into_response(),
        Ok(crate::db::topics::Deleted::NotYours) => forbidden(),
        Err(e) => server_error("cannot delete a post", &e),
    }
}

async fn topic_of_post(app: &AppState, space: &str, post_id: i64) -> Option<i64> {
    sqlx::query_as::<_, (i64,)>(
        "SELECT p.topic_id FROM posts p JOIN topics t ON t.id = p.topic_id
         WHERE p.id = ? AND t.space = ?",
    )
    .bind(post_id)
    .bind(space)
    .fetch_optional(app.db.pool())
    .await
    .ok()
    .flatten()
    .map(|(id,)| id)
}

/// An upload becomes a post of its own, whose body is a Markdown image
/// pointing at `/a/<id>`. That is why there is no "attachments per post"
/// limit: one upload is one post, so the count is structurally one, and the
/// picture reaches the page through the same renderer and the same sanitizer
/// as everything else.
async fn attach(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    axum::extract::Path(topic_id): axum::extract::Path<i64>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    let topic = match crate::db::topics::load_topic(&app.db, &space.host, topic_id).await {
        Ok(Some((topic, _))) => topic,
        Ok(None) => return not_found(),
        Err(e) => return server_error("cannot load a topic", &e),
    };
    let Some(category) = space.category(&topic.category) else {
        return not_found();
    };
    // Uploading is replying: it is the same act with a picture in it.
    if !crate::authz::may_read(&who, &space) || !crate::authz::may_reply(&who, category) {
        return forbidden();
    }

    let mut bytes: Option<Vec<u8>> = None;
    let mut caption = String::new();
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => match field.name().unwrap_or_default().to_string().as_str() {
                "file" => match field.bytes().await {
                    Ok(b) => bytes = Some(b.to_vec()),
                    // The hard body limit fires here, before anything is
                    // written. Its own status is passed through, so "too
                    // large" does not arrive as "bad request".
                    Err(e) => return (e.status(), "that file could not be read").into_response(),
                },
                "body" => caption = field.text().await.unwrap_or_default(),
                _ => {}
            },
            Ok(None) => break,
            Err(e) => return (e.status(), "the upload could not be read").into_response(),
        }
    }

    let Some(bytes) = bytes else {
        return bad_request("no file in the upload");
    };
    if bytes.len() as u64 > space.attachment_max_bytes {
        return (StatusCode::PAYLOAD_TOO_LARGE, "that file is too large").into_response();
    }

    // The allow list, from the bytes themselves. Not the file name, not the
    // content type the browser announced — the uploader chooses both.
    let Some(media_type) = crate::media::detect(&bytes) else {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "only JPEG, PNG, GIF and WebP images can be attached",
        )
            .into_response();
    };

    // What a photograph knows besides the photograph — where it was taken,
    // when, with which camera — is removed BEFORE anything is written. Doing
    // it on the way out instead would leave the location on the disk, and
    // from there in every backup.
    let bytes = crate::media::strip_metadata(&bytes, media_type);

    let id = match crate::auth::random_id() {
        Ok(id) => id,
        Err(e) => return server_error("cannot name an attachment", &e),
    };
    let path = crate::db::attachments::path_of(&app.data_dir, &id, media_type);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return server_error("cannot create the attachment directory", &e.into());
    }
    if let Err(e) = std::fs::write(&path, &bytes) {
        return server_error("cannot store an attachment", &e.into());
    }

    let caption = caption.trim();
    let body = if caption.is_empty() {
        format!("![](/a/{id})")
    } else {
        format!("{caption}\n\n![](/a/{id})")
    };

    let post_id = match crate::db::topics::add_reply(&app.db, topic_id, &body, &who).await {
        Ok(id) => id,
        Err(e) => return server_error("cannot attach a picture", &e),
    };
    if let Err(e) =
        crate::db::attachments::record(&app.db, &id, post_id, media_type, bytes.len() as i64).await
    {
        return server_error("cannot record an attachment", &e);
    }

    Redirect::to(&format!("/t/{topic_id}")).into_response()
}

/// Serves an attachment with the type **we** detected, never one from the
/// upload, plus `nosniff` so the browser does not reconsider.
async fn serve_attachment(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }
    let attachment = match crate::db::attachments::load(&app.db, &space.host, &id).await {
        Ok(Some(a)) => a,
        Ok(None) => return not_found(),
        Err(e) => return server_error("cannot load an attachment", &e),
    };

    let path =
        crate::db::attachments::path_of(&app.data_dir, &attachment.id, attachment.media_type);
    match std::fs::read(&path) {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, attachment.media_type.mime()),
                (header::CONTENT_DISPOSITION, "inline"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => server_error("cannot read an attachment", &e.into()),
    }
}

/// A validator for the stylesheet, derived from its own bytes.
///
/// Not a security property, so the hash need not be a cryptographic one — it
/// only has to CHANGE when the file changes, which `DefaultHasher` does even
/// though its output is not stable across Rust versions. An unstable hash is
/// harmless here: a different value means one extra download.
static STYLE_ETAG: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    crate::web::views::STYLESHEET.hash(&mut hasher);
    format!("\"{:016x}\"", hasher.finish())
});

/// `no-cache` means "keep it, but ask before using it" — not "do not keep it".
///
/// WITHOUT A VALIDATOR A REDESIGN CAN BE INVISIBLE. Until 2026-09-06 this
/// route sent neither `ETag` nor `Cache-Control`, which leaves a browser free
/// to reuse the file for as long as it likes. Everything measurable would say
/// the new stylesheet is deployed, running and correctly served, and the
/// person looking at the page would still see the old one — the worst kind of
/// "fixed". With the validator the cost of being current is one conditional
/// request that answers 304.
async fn stylesheet(headers: axum::http::HeaderMap) -> Response {
    let known = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == STYLE_ETAG.as_str());
    if known {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, STYLE_ETAG.as_str())],
        )
            .into_response();
    }
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::ETAG, STYLE_ETAG.as_str()),
        ],
        crate::web::views::STYLESHEET,
    )
        .into_response()
}

/// The validator of `bell.js`, like every asset's.
static BELL_ETAG: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    crate::web::views::BELL_SCRIPT.hash(&mut hasher);
    format!("\"{:016x}\"", hasher.finish())
});

async fn bell_script(headers: axum::http::HeaderMap) -> Response {
    script(&headers, BELL_ETAG.as_str(), crate::web::views::BELL_SCRIPT)
}

/// One script from its own route: `no-cache` and a validator, so a new
/// version is not hidden by a cache.
fn script(headers: &axum::http::HeaderMap, etag: &'static str, body: &'static str) -> Response {
    let known = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == etag);
    if known {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::ETAG, etag),
        ],
        body,
    )
        .into_response()
}

/// The same validator as the stylesheet's, for the same reason.
static SCRIPT_ETAG: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    crate::web::views::MENTION_SCRIPT.hash(&mut hasher);
    format!("\"{:016x}\"", hasher.finish())
});

/// `mention.js` — open like the stylesheet: it is the same file for everyone
/// and carries nothing about anybody. What it asks for (`/mentionable`) is
/// behind the session gate.
async fn mention_script(headers: axum::http::HeaderMap) -> Response {
    script(
        &headers,
        SCRIPT_ETAG.as_str(),
        crate::web::views::MENTION_SCRIPT,
    )
}

pub fn router(state: AppState) -> Router {
    use tower_http::set_header::SetResponseHeaderLayer;

    let fixed = |name: header::HeaderName, value: &'static str| {
        SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
    };

    Router::new()
        .route("/", get(space_index))
        .route("/c/{slug}", get(space_category))
        .route("/search", get(search_page))
        .route("/notifications", get(notifications))
        .route("/notifications/read", axum::routing::post(read_all))
        .route("/notifications/e/{id}", get(open_event))
        .route("/notifications/stream", get(notifications_stream))
        .route("/notifications.json", get(notifications_json))
        .route("/mentionable", get(mentionable))
        .route(
            "/notifications/mention-mail",
            axum::routing::post(set_mention_mail),
        )
        .route("/t/{id}", get(topic_page))
        .route("/c/{slug}/new", axum::routing::post(open_topic))
        .route("/t/{id}/reply", axum::routing::post(reply))
        .route("/t/{id}/follow", axum::routing::post(follow))
        .route("/t/{id}/unfollow", axum::routing::post(unfollow))
        .route("/p/{id}/edit", axum::routing::post(edit_post))
        // One address, two methods: the GET asks, the POST acts.
        .route("/p/{id}/delete", get(delete_question).post(delete_post))
        // Two limits, and both are needed. This one protects memory and is
        // deliberately far above any sensible picture; the configured
        // per-space limit below it gives the answer a person can act on.
        .route(
            "/t/{id}/attach",
            axum::routing::post(attach)
                .layer(axum::extract::DefaultBodyLimit::max(HARD_UPLOAD_LIMIT)),
        )
        .route("/a/{id}", get(serve_attachment))
        .route("/assets/style.css", get(stylesheet))
        .route("/assets/mention.js", get(mention_script))
        .route("/assets/bell.js", get(bell_script))
        .route("/auth/login", get(login))
        .route("/u/{id}/{token}", get(unsubscribe_page))
        .route("/u/{id}/{token}", axum::routing::post(unsubscribe_now))
        .route("/auth/callback", get(callback))
        // POST, nicht GET: s. den Kommentar an `logout`.
        .route("/auth/logout", axum::routing::post(logout))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_session,
        ))
        // The headers are set outermost, so a refusal carries them too.
        .layer(fixed(header::CONTENT_SECURITY_POLICY, CSP))
        .layer(fixed(header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(fixed(header::REFERRER_POLICY, "no-referrer"))
        .layer(fixed(header::X_FRAME_OPTIONS, "DENY"))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_path_on_this_site_is_a_place_to_return_to() {
        assert_eq!(safe_return_path(Some("/t/128")), "/t/128");
        assert_eq!(
            safe_return_path(Some("/c/general?page=2")),
            "/c/general?page=2"
        );
        for wrong in [
            "https://evil.example.org/",
            "//evil.example.org/",
            "/\\evil.example.org/",
            "/auth/login",
            "/auth/callback?code=x",
            "t/128",
            "/t/1\n/x",
            "",
        ] {
            assert_eq!(safe_return_path(Some(wrong)), "/", "{wrong:?} was followed");
        }
        assert_eq!(safe_return_path(None), "/");
    }

    #[test]
    fn a_pending_cookie_from_before_the_page_was_remembered_still_parses() {
        // Three lines, no page: a sign-in begun before the upgrade completes
        // on the front page rather than being refused.
        let (pending, return_to) = parse_pending("s\nn\nv").expect("three lines");
        assert_eq!((pending.state.as_str(), return_to.as_str()), ("s", "/"));
        let (_, return_to) = parse_pending("s\nn\nv\n/t/7").expect("four lines");
        assert_eq!(return_to, "/t/7");
        assert!(parse_pending("s\nn\nv\n/t/7\nextra").is_none());
    }

    #[test]
    fn a_title_of_whitespace_is_no_title() {
        assert!(checked_title("   \t \n ").is_err());
        assert_eq!(checked_title("  Hello  ").as_deref(), Ok("Hello"));
    }

    #[test]
    fn the_limits_count_what_they_say_they_count() {
        // Characters for the title, bytes for the body. An emoji is one
        // character and four bytes, and mixing the two up is how a limit
        // becomes either useless or surprising.
        let two_hundred: String = "\u{e4}".repeat(MAX_TITLE_CHARS);
        assert!(
            checked_title(&two_hundred).is_ok(),
            "200 characters refused"
        );
        assert!(checked_title(&format!("{two_hundred}x")).is_err());

        let body = "b".repeat(MAX_BODY_BYTES);
        assert!(checked_body(&body).is_ok());
        assert!(checked_body(&format!("{body}x")).is_err());
    }
}
