-- Full-text search over what people wrote.
--
-- KEPT UP TO DATE BY TRIGGERS, not by the application. The application forgets
-- — there are four places that write a post and one that deletes it, and the
-- fifth one somebody adds next year will forget too. A trigger does not.
--
-- `porter unicode61` and not the default: the default tokenizer does not fold
-- German umlauts, so a search for "Wunsche" would miss "Wünsche" and a search
-- for "WÜNSCHE" would miss both. `remove_diacritics 2` folds them.
-- THE COLUMN IS CALLED `body_markdown` AND NOT `body`, and that is not a
-- matter of taste: with `content = 'posts'` FTS5 reads the original text out
-- of that table by COLUMN NAME. Named `body` it creates fine, indexes fine,
-- matches fine — and every query that touches the content answers
-- "no such column: T.body". Which includes `snippet()`, and `count(*)`.
CREATE VIRTUAL TABLE posts_fts USING fts5(
    body_markdown,
    content = 'posts',
    content_rowid = 'id',
    tokenize = "porter unicode61 remove_diacritics 2"
);

-- Backfill: everything already written stays findable.
INSERT INTO posts_fts (rowid, body_markdown) SELECT id, body_markdown FROM posts;

CREATE TRIGGER posts_fts_insert AFTER INSERT ON posts BEGIN
    INSERT INTO posts_fts (rowid, body_markdown) VALUES (new.id, new.body_markdown);
END;

-- `delete` first, then insert: an external-content FTS5 table needs the old
-- value handed back to it, or the index keeps a row that no longer exists and
-- a search returns a post whose text has changed.
CREATE TRIGGER posts_fts_delete AFTER DELETE ON posts BEGIN
    INSERT INTO posts_fts (posts_fts, rowid, body_markdown) VALUES ('delete', old.id, old.body_markdown);
END;

CREATE TRIGGER posts_fts_update AFTER UPDATE ON posts BEGIN
    INSERT INTO posts_fts (posts_fts, rowid, body_markdown) VALUES ('delete', old.id, old.body_markdown);
    INSERT INTO posts_fts (rowid, body_markdown) VALUES (new.id, new.body_markdown);
END;

-- Titles are searched too, and they live on `topics`.
CREATE VIRTUAL TABLE topics_fts USING fts5(
    title,
    content = 'topics',
    content_rowid = 'id',
    tokenize = "porter unicode61 remove_diacritics 2"
);

INSERT INTO topics_fts (rowid, title) SELECT id, title FROM topics;

CREATE TRIGGER topics_fts_insert AFTER INSERT ON topics BEGIN
    INSERT INTO topics_fts (rowid, title) VALUES (new.id, new.title);
END;

CREATE TRIGGER topics_fts_delete AFTER DELETE ON topics BEGIN
    INSERT INTO topics_fts (topics_fts, rowid, title) VALUES ('delete', old.id, old.title);
END;

CREATE TRIGGER topics_fts_update AFTER UPDATE ON topics BEGIN
    INSERT INTO topics_fts (topics_fts, rowid, title) VALUES ('delete', old.id, old.title);
    INSERT INTO topics_fts (rowid, title) VALUES (new.id, new.title);
END;
