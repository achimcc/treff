//! `@handle`: from the text of a post to the people who may be told.
//!
//! Three layers meet here and nowhere else — the Markdown (`markup`), the
//! accounts (`db::accounts`) and the rule (`authz`) — so that "who is told"
//! and "what is highlighted" are the SAME answer. If they were decided in two
//! places, the page could highlight a name whose owner was never told, or the
//! other way round, and the second is a leak.

use crate::authz::Identity;
use crate::config::Space;
use crate::db::Db;
use std::collections::HashSet;

/// The accounts behind `handles` that may read `space`, by the groups of their
/// last sign-in. Everybody else — unknown, without a handle, without the
/// groups — is left out, and nothing says which of those it was.
async fn readers(
    db: &Db,
    space: &Space,
    handles: &[String],
) -> anyhow::Result<Vec<crate::db::accounts::Known>> {
    Ok(crate::db::accounts::by_handles(db, handles)
        .await?
        .into_iter()
        .filter(|known| may_read(known, space))
        .collect())
}

/// THE rule, once: may this account, as it signed in last, read the space?
fn may_read(known: &crate::db::accounts::Known, space: &Space) -> bool {
    let as_they_were = Identity {
        subject: known.subject.clone(),
        name: String::new(),
        groups: known.groups.clone(),
        email: None,
        handle: Some(known.handle.clone()),
    };
    crate::authz::may_read(&as_they_were, space)
}

/// One line of the suggestion list: what the overlay shows, and what it
/// writes. Nothing else leaves the server — no subject, no address.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Offer {
    pub name: String,
    pub handle: String,
}

/// Everybody who could be mentioned in this space, by name: the same rule as
/// `to_tell` and `highlighted`, asked of every account with a handle. A list
/// that showed more would say who exists, which the mention itself is careful
/// never to do.
///
/// `except` is whoever asks: mentioning yourself does nothing, so offering
/// yourself is noise — and on the evening this went live it was the only name
/// the list had.
pub async fn offered(db: &Db, space: &Space, except: &str) -> anyhow::Result<Vec<Offer>> {
    let mut out: Vec<Offer> = crate::db::accounts::with_handles(db)
        .await?
        .into_iter()
        .filter(|(_, known)| known.subject != except && may_read(known, space))
        .map(|(name, known)| Offer {
            name,
            handle: known.handle,
        })
        .collect();
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.handle.cmp(&b.handle))
    });
    Ok(out)
}

/// The subjects to tell about a post: mentioned, readers of the space, and
/// not the writer.
pub async fn to_tell(
    db: &Db,
    space: &Space,
    writer: &str,
    markdown: &str,
) -> anyhow::Result<Vec<String>> {
    let handles = crate::markup::mentions(markdown);
    Ok(readers(db, space, &handles)
        .await?
        .into_iter()
        .map(|k| k.subject)
        .filter(|s| s != writer)
        .collect())
}

/// The handles on a page that are highlighted: the same rule as `to_tell`,
/// asked once for all posts on it. The writer is NOT removed here — your own
/// name in your own post is still a name that may read the space.
pub async fn highlighted<'a>(
    db: &Db,
    space: &Space,
    bodies: impl IntoIterator<Item = &'a str>,
) -> anyhow::Result<HashSet<String>> {
    let mut handles: Vec<String> = Vec::new();
    for body in bodies {
        for h in crate::markup::mentions(body) {
            if !handles.contains(&h) {
                handles.push(h);
            }
        }
    }
    Ok(readers(db, space, &handles)
        .await?
        .into_iter()
        .map(|k| k.handle)
        .collect())
}
