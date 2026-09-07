//! Getting a notification out of the building.
//!
//! The queue is in `db::outbox`; this module turns one owed row into one
//! message and hands it to a transport. The split is deliberate: **the order
//! of things — retry, back off, give up — is decided here and tested without
//! a network**, and the transport is the only part that needs one.

pub mod mail;
pub mod unsubscribe;
pub mod webhook;

use crate::db::Db;

/// Somewhere a notification can go.
///
/// A trait so the sequencing above it can be tested against something that
/// refuses everything, or accepts everything, without a server in the loop.
/// Failures come back as an error; there is deliberately no "permanent versus
/// temporary" distinction, because an SMTP server that says 550 today because
/// a mailbox is full says 250 tomorrow, and guessing wrong in the permanent
/// direction loses mail silently.
#[allow(async_fn_in_trait)]
pub trait Transport {
    async fn deliver(&self, message: &Message) -> anyhow::Result<()>;
}

/// One notification, ready to be worded by a transport.
#[derive(Debug, Clone)]
pub struct Message {
    pub to: String,
    pub space_title: String,
    pub topic_title: String,
    pub author: String,
    pub body: String,
    /// Absolute, because it is followed from a mail client that has no idea
    /// what host it came from.
    pub link: String,
    /// The one-click way out, and the reason task 4 exists.
    pub unsubscribe: Option<String>,
}

/// Drains what is due, once. Returns how many went out.
///
/// **A failure of one row is not a failure of the run.** One bad address must
/// not hold up everybody else's mail — that is how a queue stops being drained
/// at all.
pub async fn drain_once<T: Transport>(
    db: &Db,
    transport: &T,
    compose: impl Fn(&crate::db::outbox::Owed) -> BoxFuture<anyhow::Result<Option<Message>>>,
    limit: i64,
) -> anyhow::Result<usize> {
    drain_channel(db, transport, compose, limit, "mail").await
}

/// The same, for one channel.
///
/// TWO CHANNELS AND ONE QUEUE, drained separately: a mail server that is down
/// must not hold up the webhook, and a webhook that answers 500 must not hold
/// up the mail. They share the table, the retries and the giving up — and
/// nothing else.
pub async fn drain_channel<T: Transport>(
    db: &Db,
    transport: &T,
    compose: impl Fn(&crate::db::outbox::Owed) -> BoxFuture<anyhow::Result<Option<Message>>>,
    limit: i64,
    kanal: &str,
) -> anyhow::Result<usize> {
    let owed: Vec<_> = crate::db::outbox::due(db, limit)
        .await?
        .into_iter()
        .filter(|o| o.kanal == kanal)
        .collect();
    let mut sent = 0;
    for row in owed {
        match compose(&row).await {
            // Nothing to send to — the person has no address, or the post is
            // gone. That is DONE, not failed: retrying cannot produce an
            // address, and a row that retries forever hides the ones that
            // would succeed.
            Ok(None) => {
                crate::db::outbox::mark_sent(db, row.id).await?;
            }
            Ok(Some(message)) => match transport.deliver(&message).await {
                Ok(()) => {
                    crate::db::outbox::mark_sent(db, row.id).await?;
                    sent += 1;
                }
                Err(e) => {
                    crate::db::outbox::mark_failed(db, row.id, row.attempts, &format!("{e}"))
                        .await?;
                }
            },
            Err(e) => {
                crate::db::outbox::mark_failed(db, row.id, row.attempts, &format!("{e}")).await?;
            }
        }
    }
    Ok(sent)
}

pub type BoxFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

/// Turns one owed row into a message, or into `None` when there is nobody to
/// send it to.
///
/// `None` is a normal answer, not a failure: a person may have no address at
/// the provider, and the post may have been deleted between queueing and
/// sending. Retrying either would be retrying a fact.
pub async fn compose(
    db: &Db,
    config: &crate::config::Config,
    unsubscribe_key: &[u8],
    row: &crate::db::outbox::Owed,
) -> anyhow::Result<Option<Message>> {
    // NUR DIE MAIL BRAUCHT EINE ADRESSE. Ein Webhook geht an einen Ort, nicht
    // an eine Person — die Zeile traegt trotzdem einen `subject`, weil sie an
    // jemandem haengen muss. Ohne diese Unterscheidung wurde jede
    // Webhook-Zeile als „nichts zu senden" abgehakt, sobald das schreibende
    // Konto keine Adresse hatte: der Webhook waere genau dann still, wenn die
    // Mail es auch ist, und aus demselben Grund — was ihn als zweiten Weg
    // wertlos macht.
    let to = if row.kanal == "mail" {
        match crate::auth::Sessions::address_of(db, &row.subject).await? {
            Some(adresse) => adresse,
            None => return Ok(None),
        }
    } else {
        String::new()
    };
    let Some(space) = config.space_for_host(&row.space) else {
        // The space was removed from the configuration while a notification
        // was owed. Nothing to link to and nothing to name it after.
        return Ok(None);
    };
    let Some((topic, posts)) = crate::db::topics::load_topic(db, &row.space, row.topic_id).await?
    else {
        return Ok(None);
    };
    let Some(post) = posts.iter().find(|p| p.id == row.post_id) else {
        return Ok(None);
    };

    Ok(Some(Message {
        to,
        space_title: space.title.clone(),
        topic_title: topic.title.clone(),
        author: post.author_name.clone(),
        body: post.body_markdown.clone(),
        link: format!("https://{}/t/{}", row.space, row.topic_id),
        // The way out travels WITH the notification. A link that has to be
        // looked for is one that is not used.
        unsubscribe: Some(format!(
            "https://{}/u/{}/{}",
            row.space,
            row.id,
            unsubscribe::token(unsubscribe_key, row.id)
        )),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::Identity;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Accepts(AtomicUsize);
    impl Transport for Accepts {
        async fn deliver(&self, _m: &Message) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct Refuses(AtomicUsize);
    impl Transport for Refuses {
        async fn deliver(&self, _m: &Message) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("connection refused")
        }
    }

    fn who(subject: &str) -> Identity {
        Identity {
            subject: subject.into(),
            name: subject.into(),
            groups: vec!["Household".into()],
            email: Some(format!("{subject}@example.org")),
        }
    }

    fn a_message(row: &crate::db::outbox::Owed) -> BoxFuture<anyhow::Result<Option<Message>>> {
        let to = format!("{}@example.org", row.subject);
        Box::pin(async move {
            Ok(Some(Message {
                to,
                space_title: "Forum".into(),
                topic_title: "T".into(),
                author: "ben".into(),
                body: "hello".into(),
                link: "https://forum.example.org/t/1".into(),
                unsubscribe: None,
            }))
        })
    }

    async fn one_owed_row() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&dir.path().join("t.db")).await.expect("open");
        let topic = crate::db::topics::create_topic(
            &db,
            "forum.example.org",
            "general",
            "T",
            "B",
            &who("ada"),
        )
        .await
        .expect("topic");
        crate::db::topics::add_reply(&db, topic, "First", &who("ben"))
            .await
            .expect("reply");
        (dir, db)
    }

    #[tokio::test]
    async fn a_drained_row_is_not_owed_again() {
        let (_d, db) = one_owed_row().await;
        let transport = Accepts(AtomicUsize::new(0));

        assert_eq!(
            drain_once(&db, &transport, a_message, 10)
                .await
                .expect("drain"),
            1
        );
        assert_eq!(transport.0.load(Ordering::SeqCst), 1);

        // The second run has nothing to do, which is what "sent" has to mean.
        assert_eq!(
            drain_once(&db, &transport, a_message, 10)
                .await
                .expect("drain"),
            0
        );
        assert_eq!(
            transport.0.load(Ordering::SeqCst),
            1,
            "and nothing was sent twice"
        );
    }

    /// THE CASE THIS WHOLE TABLE EXISTS FOR: the mail server is down and the
    /// reply is not lost.
    #[tokio::test]
    async fn a_refused_row_is_kept_and_tried_again_later() {
        let (_d, db) = one_owed_row().await;
        let transport = Refuses(AtomicUsize::new(0));

        assert_eq!(
            drain_once(&db, &transport, a_message, 10)
                .await
                .expect("drain"),
            0
        );
        assert_eq!(transport.0.load(Ordering::SeqCst), 1);

        // Still owed — but not right now, or a broken server would be
        // hammered. Only the mail row is asked about: the webhook row is
        // untouched, because nothing drained that channel.
        let offen = crate::db::outbox::due(&db, 10).await.expect("due");
        assert!(!offen.iter().any(|o| o.kanal == "mail"), "{offen:?}");
        assert_eq!(crate::db::outbox::given_up(&db).await.expect("count"), 0);
    }

    /// Somebody without an address is not a failure that blocks the queue.
    #[tokio::test]
    async fn nothing_to_send_to_is_done_rather_than_retried_forever() {
        let (_d, db) = one_owed_row().await;
        let transport = Accepts(AtomicUsize::new(0));
        let nobody = |_: &crate::db::outbox::Owed| -> BoxFuture<anyhow::Result<Option<Message>>> {
            Box::pin(async { Ok(None) })
        };

        assert_eq!(
            drain_once(&db, &transport, nobody, 10)
                .await
                .expect("drain"),
            0
        );
        assert_eq!(transport.0.load(Ordering::SeqCst), 0, "nothing was sent");
        assert!(
            !crate::db::outbox::due(&db, 10)
                .await
                .expect("due")
                .iter()
                .any(|o| o.kanal == "mail"),
            "and it is not owed again — retrying cannot produce an address"
        );
    }
}
