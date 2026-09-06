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
    /// Where the provider sends people back to. Needed to build the client,
    /// and only known to whoever runs the instance.
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
        redirect_uri: &str,
        dir: &Path,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            config: Arc::new(config),
            db,
            oidc: Arc::new(oidc),
            redirect_uri: Arc::from(redirect_uri),
            provider: Arc::new(tokio::sync::OnceCell::new()),
            data_dir: dir.to_path_buf(),
            cookie_key: load_or_create_cookie_key(dir)?,
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
            crate::web::views::category_index(&space, &who, lang, &rows).into_response()
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

    crate::web::views::space_page(space, who, lang, space.category(slug), &rows).into_response()
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
            let category = space.category(&topic.category);
            crate::web::views::topic_page(&space, &who, lang, category, &topic, &posts)
                .into_response()
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

    match crate::db::topics::create_topic(&app.db, &space.host, &slug, &title, &body, &who).await {
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

    match crate::db::topics::add_reply(&app.db, id, &body, &who).await {
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
    match crate::db::topics::update_post(&app.db, &space.host, id, &body, &who).await {
        Ok(true) => match topic_of_post(&app, &space.host, id).await {
            Some(topic) => Redirect::to(&format!("/t/{topic}")).into_response(),
            None => Redirect::to("/").into_response(),
        },
        Ok(false) => forbidden(),
        Err(e) => server_error("cannot edit a post", &e),
    }
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
        .route("/c/{slug}/new", axum::routing::post(open_topic))
        .route("/t/{id}/reply", axum::routing::post(reply))
        .route("/p/{id}/edit", axum::routing::post(edit_post))
        .route("/p/{id}/delete", axum::routing::post(delete_post))
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

#[cfg(test)]
mod tests {
    use super::*;

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
