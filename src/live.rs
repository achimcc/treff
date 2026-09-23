//! The bell, live: one Server-Sent Events stream per open page.
//!
//! A stream sends the bell once on connect, and again whenever the database
//! says something changed anywhere (`Db::changed`, announced after every
//! commit that can change somebody's unread; since 0.10.0 one bell shows
//! every space, so every space's change is asked after) — but only if the
//! answer is different from the last one it sent. A change for somebody else therefore
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
    compute: F,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + use<F, Fut>>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<serde_json::Value>> + Send,
{
    struct State<F> {
        changes: tokio::sync::broadcast::Receiver<String>,
        compute: F,
        last: Option<String>,
        first: bool,
    }
    let state = State {
        changes: db.changes(),
        compute,
        last: None,
        first: true,
    };
    let stream = futures_util::stream::unfold(state, |mut st| async move {
        loop {
            if !st.first {
                use tokio::sync::broadcast::error::RecvError;
                match st.changes.recv().await {
                    // Whatever space changed: the bell shows them all, and
                    // the query decides whether this person's answer moved.
                    Ok(_) => {}
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

/// Where an entry's link starts: nothing on the host the entry belongs to,
/// `https://<its host>` everywhere else — the start page (`current_host`
/// empty) gets every link absolute.
pub fn link_base(entry_space: &str, current_host: &str) -> String {
    if entry_space == current_host {
        String::new()
    } else {
        format!("https://{entry_space}")
    }
}

/// One entry as a page outside the list draws it — the start page, and the
/// overlay under the bell. Words are the page's business; this
/// carries the facts and a link that leads through the forum, where reading
/// marks things read. `current_host` is the host the page is shown on.
pub fn entry_json(entry: &crate::db::inbox::Entry, current_host: &str) -> serde_json::Value {
    use crate::db::inbox::Entry;
    let base = link_base(entry.space(), current_host);
    match entry {
        Entry::Replies {
            topic_id,
            topic_title,
            space: _,
            count,
            latest_author,
            latest_at,
            first_post_id,
            unread,
        } => serde_json::json!({
            "kind": "replies",
            "title": topic_title,
            "count": count,
            "author": latest_author,
            "at": latest_at,
            "unread": unread,
            "link": format!("{base}/t/{topic_id}#p{first_post_id}"),
        }),
        Entry::Mention {
            topic_id,
            topic_title,
            post_id,
            author,
            space: _,
            at,
            unread,
        } => serde_json::json!({
            "kind": "mention",
            "title": topic_title,
            "author": author,
            "at": at,
            "unread": unread,
            "link": format!("{base}/t/{topic_id}#p{post_id}"),
        }),
        Entry::Likes {
            topic_id,
            topic_title,
            post_id,
            count,
            space: _,
            latest_name,
            at,
            unread,
        } => serde_json::json!({
            "kind": "likes",
            "title": topic_title,
            "count": count,
            "author": latest_name,
            "at": at,
            "unread": unread,
            "link": format!("{base}/t/{topic_id}#p{post_id}"),
        }),
        Entry::Event {
            id,
            kind,
            title,
            reason,
            at,
            unread,
            space: _,
        } => serde_json::json!({
            "kind": kind.as_str(),
            "title": title,
            "reason": reason,
            "at": at,
            "unread": unread,
            "link": format!("{base}/notifications/e/{id}"),
        }),
    }
}

/// The bell of an account, as JSON: the number and the entries over
/// `spaces` (the ones this person may read), with links relative on
/// `current_host` and absolute everywhere else.
pub async fn bell_json(
    db: &crate::db::Db,
    subject: &str,
    spaces: &[String],
    current_host: &str,
    limit: i64,
) -> anyhow::Result<serde_json::Value> {
    let unread = crate::db::inbox::unread_count(db, subject, spaces).await?;
    let entries = crate::db::inbox::entries(db, subject, spaces, limit).await?;
    let entries: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| entry_json(e, current_host))
        .collect();
    Ok(serde_json::json!({ "unread": unread, "entries": entries }))
}
