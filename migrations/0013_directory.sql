-- The directory: who exists, before they ever sign in (plan-stage-6, ADR 0007).
--
-- The identity provider pushes people and groups over SCIM. A person lands in
-- the SAME `accounts` row a sign-in writes — `subject` is the SCIM id, which
-- the operator's mapping makes the user's UUID, which is the OIDC `sub`.
--
-- `scim_user_name` is the user name as SCIM sent it, kept for the one filter
-- the provider uses (`userName eq "…"`); the handle derived from it can differ
-- (lower-cased) or be absent (not a valid handle).
--
-- `active = 0` is somebody who left or was switched off: handle, address and
-- groups are cleared, the row stays — posts carry `author_subject`, and a
-- forum's history is not rewritten because somebody went.
ALTER TABLE accounts ADD COLUMN scim_user_name TEXT;
ALTER TABLE accounts ADD COLUMN active INTEGER NOT NULL DEFAULT 1;

CREATE TABLE scim_groups (
    id   TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

-- Members by subject, not by account row: the provider may name a member
-- before (or without) sending the person, and the membership must be there
-- when the person arrives.
CREATE TABLE scim_members (
    group_id TEXT NOT NULL REFERENCES scim_groups(id) ON DELETE CASCADE,
    subject  TEXT NOT NULL,
    PRIMARY KEY (group_id, subject)
);
CREATE INDEX scim_members_by_subject ON scim_members(subject);
