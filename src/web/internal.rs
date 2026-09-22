//! The internal listener: a second door, beside treff's own sign-in, for
//! other services on the same machine (ADR 0006).
//!
//! Two routes, each behind its own token:
//!
//! * `POST /internal/events` — an event for a person, from a service that
//!   knows about it (the first: a film request that became available or
//!   failed). Checked field by field (`db::events::checked`).
//! * `GET /internal/bell` — the bell for a person, for a page outside treff
//!   (the start page). The person is named by the proxy in front, from the
//!   identity provider's answer: `X-Treff-User` (a handle) and
//!   `X-Treff-Groups` (`|`-separated, as Authentik writes them).
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

#[derive(Clone)]
pub struct InternalState {
    config: Arc<Config>,
    db: Db,
    events_token: Option<Arc<Vec<u8>>>,
    bell_token: Option<Arc<Vec<u8>>>,
}

impl InternalState {
    pub fn new(
        config: Arc<Config>,
        db: Db,
        events_token: Option<Vec<u8>>,
        bell_token: Option<Vec<u8>>,
    ) -> Self {
        Self {
            config,
            db,
            events_token: events_token.map(Arc::new),
            bell_token: bell_token.map(Arc::new),
        }
    }
}

/// Where the internal listener lives, and what opens each of its routes.
pub struct Settings {
    pub listen: String,
    pub events_token: Option<Vec<u8>>,
    pub bell_token: Option<Vec<u8>>,
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
    let events = token("TREFF_EVENTS_TOKEN_FILE")?;
    let bell = token("TREFF_BELL_TOKEN_FILE")?;
    match listen {
        Some(listen) => Ok(Some(Settings {
            listen,
            events_token: events,
            bell_token: bell,
        })),
        None if events.is_some() || bell.is_some() => {
            anyhow::bail!(
                "a token file for the internal listener is set, TREFF_INTERNAL_LISTEN is not"
            )
        }
        None => Ok(None),
    }
}

pub fn router(state: InternalState) -> Router {
    let mut router = Router::new();
    if state.events_token.is_some() {
        router = router.route("/internal/events", post(take_event));
    }
    if state.bell_token.is_some() {
        router = router
            .route("/internal/bell", get(bell))
            .route("/internal/bell/stream", get(bell_stream));
    }
    router.with_state(state)
}

/// `Authorization: Bearer <token>`, compared in constant time.
fn presents(headers: &HeaderMap, token: &[u8]) -> bool {
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
    let Some(token) = state.events_token.as_deref() else {
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
    let Some(token) = state.bell_token.as_deref() else {
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
    let Some(token) = state.bell_token.as_deref() else {
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
