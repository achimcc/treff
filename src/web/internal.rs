//! The internal listener: a second door, beside treff's own sign-in, for
//! other services on the same machine (ADR 0006).
//!
//! Three routes, each behind its own token:
//!
//! * `POST /internal/events` — an event for a person, from a service that
//!   knows about it (the first: a film request that became available or
//!   failed). Checked field by field (`db::events::checked`).
//! * `GET /internal/bell` — the bell for a person, for a page outside treff
//!   (the start page). The person is named by the proxy in front, from the
//!   identity provider's answer: `X-Treff-User` (a handle) and
//!   `X-Treff-Groups` (`|`-separated, as Authentik writes them).
//! * `/scim/v2/…` — the identity provider pushing its people and groups in
//!   (`web::scim`, ADR 0007). The only one of the three that WRITES who
//!   exists, which is why it has a token of its own rather than the bell's.
//!
//! **It is its own router on its own listener.** No sessions, no `Host`
//! routing, no page — and the public router has none of these routes, so
//! nothing that reaches treff through a public virtual host reaches this.
//!
//! **A route whose token is not configured does not exist.** Never "open,
//! because nothing was set".

use crate::config::Config;
use crate::db::Db;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use std::sync::Arc;

/// What opens each route. NAMED, not three positional `Option<Vec<u8>>` in a
/// row — the whole point of the separation is that they cannot be confused,
/// and an argument list is the one place where they easily could be.
#[derive(Default)]
pub struct Tokens {
    pub events: Option<Vec<u8>>,
    pub bell: Option<Vec<u8>>,
    pub scim: Option<Vec<u8>>,
}

#[derive(Clone)]
pub struct InternalState {
    pub(crate) config: Arc<Config>,
    pub(crate) db: Db,
    pub(crate) tokens: Arc<Tokens>,
}

impl InternalState {
    pub fn new(config: Arc<Config>, db: Db, tokens: Tokens) -> Self {
        Self {
            config,
            db,
            tokens: Arc::new(tokens),
        }
    }
}

/// Where the internal listener lives, and what opens each of its routes.
pub struct Settings {
    pub listen: String,
    pub tokens: Tokens,
}

/// Where the listener lives and what opens it, from the environment.
///
/// `None` when `TREFF_INTERNAL_LISTEN` is not set. A token file that is set
/// but cannot be read, or is empty, stops treff — and so does a token file
/// without a listener: somebody meant to open a door and it would silently
/// stay shut.
pub fn from_env() -> anyhow::Result<Option<Settings>> {
    let listen = std::env::var("TREFF_INTERNAL_LISTEN").ok();
    let token = |name: &str| -> anyhow::Result<Option<Vec<u8>>> {
        let Ok(path) = std::env::var(name) else {
            return Ok(None);
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("{name} ({path}) cannot be read: {e}"))?;
        let t = text.trim();
        if t.is_empty() {
            anyhow::bail!("{name} ({path}) is empty");
        }
        Ok(Some(t.as_bytes().to_vec()))
    };
    let tokens = Tokens {
        events: token("TREFF_EVENTS_TOKEN_FILE")?,
        bell: token("TREFF_BELL_TOKEN_FILE")?,
        scim: token("TREFF_SCIM_TOKEN_FILE")?,
    };
    let any = tokens.events.is_some() || tokens.bell.is_some() || tokens.scim.is_some();
    match listen {
        Some(listen) => Ok(Some(Settings { listen, tokens })),
        None if any => {
            anyhow::bail!(
                "a token file for the internal listener is set, TREFF_INTERNAL_LISTEN is not"
            )
        }
        None => Ok(None),
    }
}

pub fn router(state: InternalState) -> Router {
    let mut router = Router::new();
    if state.tokens.events.is_some() {
        router = router.route("/internal/events", post(take_event));
    }
    if state.tokens.bell.is_some() {
        router = router
            .route("/internal/bell", get(bell))
            .route("/internal/bell/stream", get(bell_stream));
    }
    if state.tokens.scim.is_some() {
        router = router.merge(crate::web::scim::routes(&state));
    }
    router.with_state(state)
}

/// `Authorization: Bearer <token>`, compared in constant time.
pub(crate) fn presents(headers: &HeaderMap, token: &[u8]) -> bool {
    let Some(given) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    let given = given.as_bytes();
    if given.len() != token.len() {
        return false;
    }
    given
        .iter()
        .zip(token)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

async fn take_event(
    State(state): State<InternalState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(token) = state.tokens.events.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !presents(&headers, token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(events) = state.config.events.as_ref() else {
        // Configured to take events and not told where they go: a mistake of
        // the operator, said as such rather than stored somewhere.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no [events] space is configured",
        )
            .into_response();
    };
    let raw: crate::db::events::Raw = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return (StatusCode::BAD_REQUEST, "not an event").into_response(),
    };
    let checked = match crate::db::events::checked(raw, events) {
        Ok(c) => c,
        Err(why) => return (StatusCode::BAD_REQUEST, why).into_response(),
    };
    match crate::db::events::take(&state.db, &events.space, &checked).await {
        Ok(true) => StatusCode::CREATED.into_response(),
        Ok(false) => StatusCode::OK.into_response(),
        Err(e) => {
            eprintln!("treff: cannot store an event: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// How many entries the start page shows at most. A list, not an archive.
const BELL_ENTRIES: i64 = 20;

async fn bell(State(state): State<InternalState>, headers: HeaderMap) -> Response {
    let Some(token) = state.tokens.bell.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !presents(&headers, token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match bell_for(&state, &headers).await {
        Ok(body) => ([(header::CACHE_CONTROL, "no-store")], axum::Json(body)).into_response(),
        Err(e) => {
            eprintln!("treff: cannot read a bell: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The same answer, live, for the start page (`live::bell_stream`). The
/// person is who the proxy named when the stream was opened.
async fn bell_stream(State(state): State<InternalState>, headers: HeaderMap) -> Response {
    let Some(token) = state.tokens.bell.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !presents(&headers, token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let space = state
        .config
        .events
        .as_ref()
        .map(|e| e.space.clone())
        .unwrap_or_default();
    let db = state.db.clone();
    let stream = crate::live::bell_stream(&db, space, move || {
        let state = state.clone();
        let headers = headers.clone();
        async move { bell_for(&state, &headers).await }
    });
    ([(header::CACHE_CONTROL, "no-store")], stream).into_response()
}

/// The answer for whoever the proxy named — or an empty bell, for everybody
/// it cannot be given to. One shape for "nobody", "not allowed" and "nothing
/// new", so the door says nothing about who exists.
pub async fn bell_for(
    state: &InternalState,
    headers: &HeaderMap,
) -> anyhow::Result<serde_json::Value> {
    let empty = serde_json::json!({ "unread": 0, "entries": [] });
    let Some(space) = state
        .config
        .events
        .as_ref()
        .and_then(|e| state.config.space_for_host(&e.space))
    else {
        return Ok(empty);
    };
    let header_text = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let Some(handle) = crate::auth::checked_handle(&header_text("x-treff-user")) else {
        return Ok(empty);
    };
    let groups: Vec<String> = header_text("x-treff-groups")
        .split('|')
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(String::from)
        .collect();
    let who = crate::authz::Identity {
        subject: String::new(),
        name: String::new(),
        groups,
        email: None,
        handle: Some(handle.clone()),
    };
    if !crate::authz::may_read(&who, space) {
        return Ok(empty);
    }
    let (unread, entries) =
        crate::db::inbox::for_handle(&state.db, &handle, &space.host, BELL_ENTRIES).await?;
    let base = format!("https://{}", space.host);
    let entries: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| crate::live::entry_json(e, &base))
        .collect();
    Ok(serde_json::json!({ "unread": unread, "entries": entries }))
}
