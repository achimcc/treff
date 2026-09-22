-- What a person has not seen yet: the bell.
--
-- ONE ROW PER PERSON PER POST, and the primary key is the rule: a follower who
-- is also mentioned in the same post gets one entry, not two. Whoever writes
-- first inside the transaction wins — mentions are written before replies, so
-- the more personal reason is the one that stays (design, "What a mention does
-- and does not do").
--
-- `space` is copied in rather than joined from `topics` because every query
-- here is asked per space — the bell on one host shows that host's entries —
-- and the badge is asked on every page.
CREATE TABLE inbox (
    subject    TEXT    NOT NULL,
    space      TEXT    NOT NULL,
    topic_id   INTEGER NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    post_id    INTEGER NOT NULL REFERENCES posts(id)  ON DELETE CASCADE,
    reason     TEXT    NOT NULL CHECK (reason IN ('reply', 'mention')),
    created_at INTEGER NOT NULL,
    read_at    INTEGER,
    PRIMARY KEY (subject, post_id)
);

-- The badge's query: this person, this space, what is unread.
CREATE INDEX inbox_unread ON inbox(subject, space, read_at);
