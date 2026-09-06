-- Timestamps are Unix seconds as INTEGER. Not ISO text: sorting and
-- comparing stay numeric, and there is no time-zone trap in a WHERE clause.
CREATE TABLE topics (
    id             INTEGER PRIMARY KEY,
    space          TEXT    NOT NULL,
    category       TEXT    NOT NULL,
    title          TEXT    NOT NULL,
    author_subject TEXT    NOT NULL,
    author_name    TEXT    NOT NULL,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

-- The opening post is an ordinary row in `posts`. A second field on the topic
-- would be a second place for the same text.
CREATE TABLE posts (
    id             INTEGER PRIMARY KEY,
    topic_id       INTEGER NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    body_markdown  TEXT    NOT NULL,
    author_subject TEXT    NOT NULL,
    author_name    TEXT    NOT NULL,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,
    edited         INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE attachments (
    id           TEXT    PRIMARY KEY,          -- random, and also the file name
    post_id      INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
    media_type   TEXT    NOT NULL,             -- from the magic bytes, never from the browser
    byte_size    INTEGER NOT NULL,
    created_at   INTEGER NOT NULL
);

-- Sessions live in the database, not in the cookie: the cookie carries only
-- the identifier. A restart must not sign anyone out, and a sign-out must
-- really sign out.
CREATE TABLE sessions (
    id          TEXT    PRIMARY KEY,
    subject     TEXT    NOT NULL,
    name        TEXT    NOT NULL,
    groups_json TEXT    NOT NULL,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL
);

CREATE INDEX posts_by_topic     ON posts(topic_id, id);
CREATE INDEX topics_by_space    ON topics(space, category, updated_at DESC);
CREATE INDEX sessions_by_expiry ON sessions(expires_at);
