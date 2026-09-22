//! The bell, live: one Server-Sent Events stream per open page.
//!
//! A stream sends the bell once on connect, and again whenever the database
//! says something in its space changed (`Db::changed`, announced after every
//! commit that can change somebody's unread) — but only if the answer is
//! different from the last one it sent. A change for somebody else therefore
//! costs one query and sends nothing: not even "still 0", which would tell a
//! watcher that something happened to someone.

use crate::db::Db;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::Stream;
use std::convert::Infallible;
use std::future::Future;

/// Every 25 seconds a comment, so that no proxy between here and the browser
/// takes a quiet connection for a dead one.
const KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(25);

pub fn bell_stream<F, Fut>(
    db: &Db,
    space: String,
    compute: F,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + use<F, Fut>>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<serde_json::Value>> + Send,
{
    struct State<F> {
        changes: tokio::sync::broadcast::Receiver<String>,
        space: String,
        compute: F,
        last: Option<String>,
        first: bool,
    }
    let state = State {
        changes: db.changes(),
        space,
        compute,
        last: None,
        first: true,
    };
    let stream = futures_util::stream::unfold(state, |mut st| async move {
        loop {
            if !st.first {
                use tokio::sync::broadcast::error::RecvError;
                match st.changes.recv().await {
                    Ok(s) if s == st.space || s == crate::db::EVERY_SPACE => {}
                    Ok(_) => continue,
                    // Fell behind: something changed, and what exactly is
                    // lost. Asking again is the whole answer.
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return None,
                }
            }
            st.first = false;
            let value = match (st.compute)().await {
                Ok(v) => v,
                Err(e) => {
                    // One failed read is not the end of the stream; the next
                    // change asks again.
                    eprintln!("treff: cannot read a bell for a stream: {e}");
                    continue;
                }
            };
            let text = value.to_string();
            if st.last.as_deref() == Some(text.as_str()) {
                continue;
            }
            st.last = Some(text.clone());
            return Some((Ok(Event::default().event("bell").data(text)), st));
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
}
