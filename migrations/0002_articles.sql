-- Articles that are written somewhere else (design §4).
--
-- `source_key` is the file name an article was mirrored from. It is UNIQUE, so
-- mirroring the same directory twice updates rather than duplicates, and NULL
-- for everything written in a browser.
ALTER TABLE topics ADD COLUMN source_key TEXT;

-- A mirrored article whose file disappeared. Hidden rather than deleted: the
-- comments underneath belong to the people who wrote them, and ON DELETE
-- CASCADE would take them along. If the file comes back, so does everything.
ALTER TABLE topics ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;

CREATE UNIQUE INDEX topics_by_source ON topics(source_key)
    WHERE source_key IS NOT NULL;
