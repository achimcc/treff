-- Likes: one heart per person and post (design-likes-and-the-forum-face,
-- ADR 0008).
--
-- The name is copied in as it is on `posts`: a liker who leaves the provider
-- keeps their name on what they liked, and the page that shows the names
-- under every post does not reach into `accounts` for them.
CREATE TABLE likes (
    post_id    INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    subject    TEXT    NOT NULL,
    name       TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (post_id, subject)
);

-- `inbox.reason` gains 'like'. SQLite cannot widen a CHECK, so the table is
-- rebuilt in place; the rows stay what they are. The entry for a like is
-- ONE ROW PER POST, owned by the post's author: the count and the latest
-- name are read from `likes` when the bell is shown, so the line is always
-- as true as the number under the post. The primary key (subject, post_id)
-- is free for it — a reply entry is never written for the writer, and a
-- mention of yourself does nothing.
CREATE TABLE inbox_new (
    subject    TEXT    NOT NULL,
    space      TEXT    NOT NULL,
    topic_id   INTEGER NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    post_id    INTEGER NOT NULL REFERENCES posts(id)  ON DELETE CASCADE,
    reason     TEXT    NOT NULL CHECK (reason IN ('reply', 'mention', 'like')),
    created_at INTEGER NOT NULL,
    read_at    INTEGER,
    PRIMARY KEY (subject, post_id)
);
INSERT INTO inbox_new (subject, space, topic_id, post_id, reason, created_at, read_at)
     SELECT subject, space, topic_id, post_id, reason, created_at, read_at FROM inbox;
DROP TABLE inbox;
ALTER TABLE inbox_new RENAME TO inbox;
CREATE INDEX inbox_unread ON inbox(subject, space, read_at);
