-- One row per person, and the reason it exists is timing.
--
-- The address arrives with a sign-in and was, until this migration, kept on
-- the SESSION — which expires after twelve hours. Somebody who last signed in
-- on Tuesday would then get no mail on Thursday, and the queue would quietly
-- mark those rows as "nothing to send to". A notification service whose
-- delivery depends on the recipient being logged in has it exactly backwards.
--
-- Refreshed on every sign-in, so a changed address at the provider is right
-- again the next time somebody comes by. `subject` stays the identity; the
-- address is only how to reach it.
CREATE TABLE accounts (
    subject   TEXT PRIMARY KEY,
    name      TEXT NOT NULL,
    email     TEXT,
    seen_at   INTEGER NOT NULL
);
