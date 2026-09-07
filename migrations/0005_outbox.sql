-- What has to go out, and what has already gone.
--
-- WHY A TABLE AND NOT A CHANNEL IN MEMORY: a reply must not be lost because
-- SMTP is down, and a request must not wait on the network while somebody
-- watches a spinner. Rows are written in the same transaction as the post;
-- a background task drains them.
--
-- The address is COPIED IN at send time, not stored here — see `sessions`.
-- What is stored is who it is for, so the sender can look up the current one.
CREATE TABLE outbox (
    id           INTEGER PRIMARY KEY,
    subject      TEXT    NOT NULL,          -- who it is for
    topic_id     INTEGER NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    post_id      INTEGER NOT NULL REFERENCES posts(id)  ON DELETE CASCADE,
    space        TEXT    NOT NULL,          -- decides the link and the sender name
    created_at   INTEGER NOT NULL,
    -- Attempts and the earliest next try. A row that keeps failing backs off
    -- instead of hammering, and stops being tried at all after `MAX_ATTEMPTS`
    -- — it stays in the table, visible, rather than looping or vanishing.
    attempts     INTEGER NOT NULL DEFAULT 0,
    next_try_at  INTEGER NOT NULL,
    -- NULL while it is still owed. Set when it went out, so a restart in the
    -- middle of a run repeats at most one send and loses none.
    sent_at      INTEGER,
    last_error   TEXT
);

-- The sender's query: what is owed, and due.
CREATE INDEX outbox_due ON outbox(sent_at, next_try_at);
