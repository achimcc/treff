//! The web frame: which host reaches which space, who is let in at all, and
//! what every answer carries. No page is built here — this is the frame each
//! later page passes through, and it is the layer that fails closed.

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
) -> Response {
    if !crate::authz::may_read(&who, &space) {
        return forbidden();
    }

    match space.view {
        crate::config::View::Timeline => {
            let Some(category) = space.categories.first() else {
                return not_found();
            };
            render_space(&app, &space, &who, &category.slug.clone()).await
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
            crate::web::views::category_index(&space, &who, &rows).into_response()
        }
    }
}

async fn space_category(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> Response {
    if space.category(&slug).is_none() {
        return not_found();
    }
    render_space(&app, &space, &who, &slug).await
}

async fn render_space(app: &AppState, space: &Space, who: &Identity, slug: &str) -> Response {
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
    for topic in topics {
        let first = if space.view == crate::config::View::Timeline {
            match crate::db::topics::load_topic(&app.db, &space.host, topic.id).await {
                Ok(Some((_, posts))) => posts.into_iter().next(),
                Ok(None) => None,
                Err(e) => return server_error("cannot load a topic", &e),
            }
        } else {
            None
        };
        rows.push((topic, first));
    }

    crate::web::views::space_page(space, who, &rows).into_response()
}

async fn topic_page(
    State(app): State<AppState>,
    CurrentSpace(space): CurrentSpace,
    CurrentUser(who): CurrentUser,
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
            crate::web::views::topic_page(&space, &who, &topic, &posts).into_response()
        }
        Err(e) => server_error("cannot load a topic", &e),
    }
}

async fn stylesheet() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        crate::web::views::STYLESHEET,
    )
        .into_response()
}

pub fn router(state: AppState) -> Router {
    use tower_http::set_header::SetResponseHeaderLayer;

    let fixed = |name: header::HeaderName, value: &'static str| {
        SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
    };

    Router::new()
        .route("/", get(space_index))
        .route("/c/{slug}", get(space_category))
        .route("/t/{id}", get(topic_page))
        .route("/assets/style.css", get(stylesheet))
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
