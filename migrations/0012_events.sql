-- Events from other services, for one person: a film request that became
-- available, or failed (plan-stage-5, ADR 0006).
--
-- KEYED BY HANDLE, NOT BY SUBJECT. The person may never have signed in to
-- treff — somebody who only asks for films has no account row here. The
-- handle is what the other service knows them by (the provider's username),
-- and the entries are waiting when they first come.
--
-- `source_key` + `kind` is unique: a webhook that arrives twice is one entry.
CREATE TABLE events (
    id         INTEGER PRIMARY KEY,
    handle     TEXT    NOT NULL,
    space      TEXT    NOT NULL,
    kind       TEXT    NOT NULL CHECK (kind IN ('film_available', 'film_failed')),
    title      TEXT    NOT NULL,
    link       TEXT,
    reason     TEXT,
    source_key TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    read_at    INTEGER,
    UNIQUE (source_key, kind)
);

CREATE INDEX events_unread ON events(handle, space, read_at);
