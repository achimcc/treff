-- Who hears about what.
--
-- ONE ROW PER PERSON PER TOPIC, enforced by the primary key rather than by
-- application code: replying twice must not mean two mails, and a check in a
-- handler is a check somebody forgets to write in the second handler.
--
-- `email` is not stored here. It belongs to the session, is refreshed on every
-- sign-in, and copying it would freeze the address a person had on the day
-- they first replied.
CREATE TABLE subscriptions (
    subject    TEXT    NOT NULL,
    topic_id   INTEGER NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (subject, topic_id)
);

-- The index the sender needs: everybody on one topic, in one seek.
CREATE INDEX subscriptions_by_topic ON subscriptions(topic_id);
