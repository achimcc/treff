//! The web frame: which host reaches which space, who is let in at all, and
//! what every answer carries. No page is built here — this is the frame each
//! later page passes through, and it is the layer that fails closed.

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

/// No inline anything, no third party, nothing framed. The stylesheet is
/// served as its own route so that `style-src 'self'` can stay honest.
pub const CSP: &str = "default-src 'self'; img-src 'self'; style-src 'self'; \
                       script-src 'none'; object-src 'none'; frame-ancestors 'none'; \
                       base-uri 'none'; form-action 'self'";

pub const SESSION_COOKIE: &str = "treff_session";

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Db,
    pub oidc: Arc<OidcSettings>,
    pub provider: Option<Arc<crate::auth::oidc::Provider>>,
    cookie_key: Key,
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
    pub fn new(
        config: Config,
        db: Db,
        oidc: OidcSettings,
        provider: Option<Arc<crate::auth::oidc::Provider>>,
        dir: &Path,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            config: Arc::new(config),
            db,
            oidc: Arc::new(oidc),
            provider,
            cookie_key: load_or_create_cookie_key(dir)?,
        })
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
            None => Err(Redirect::to("/auth/login").into_response()),
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

    let open = path.starts_with("/auth/") || path.starts_with("/assets/");
    if !open && identity_from_cookies(&app, &parts).await.is_none() {
        return Redirect::to("/auth/login").into_response();
    }

    next.run(Request::from_parts(parts, body)).await
}

async fn login(State(app): State<AppState>) -> Response {
    let Some(provider) = app.provider.clone() else {
        // In this build there is no provider to send anyone to. Saying so is
        // the honest answer; redirecting to ourselves would be a loop.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "the identity provider is not configured",
        )
            .into_response();
    };

    match provider.begin_login() {
        Ok(start) => {
            let jar = PrivateCookieJar::new(app.cookie_key.clone()).add(pending_cookie(&start));
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

fn pending_cookie(start: &crate::auth::oidc::LoginStart) -> Cookie<'static> {
    // state, nonce and the PKCE verifier travel in one short-lived private
    // cookie. They never appear in a URL.
    let value = format!(
        "{}\n{}\n{}",
        start.pending.state, start.pending.nonce, start.pending.pkce_verifier
    );
    let mut c = Cookie::new("treff_pending", value);
    c.set_http_only(true);
    c.set_secure(true);
    c.set_same_site(SameSite::Lax);
    c.set_path("/auth");
    c.set_max_age(time::Duration::minutes(10));
    c
}

fn tracing_error(what: &str, e: &anyhow::Error) {
    eprintln!("treff: {what}: {e}");
}

async fn placeholder() -> &'static str {
    "treff"
}

pub fn router(state: AppState) -> Router {
    use tower_http::set_header::SetResponseHeaderLayer;

    let fixed = |name: header::HeaderName, value: &'static str| {
        SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
    };

    Router::new()
        .route("/", get(placeholder))
        .route("/auth/login", get(login))
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
