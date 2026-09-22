-- What a mention needs to know about somebody who is not signed in.
--
-- `handle` is what others type after an `@`: the provider's
-- `preferred_username`, checked and lower-cased (`auth::checked_handle`).
-- UNIQUE, because two people answering to one handle would hand one of them
-- the other's mentions; the sign-in that would collide writes NULL instead.
--
-- `groups_json` is the groups of the LAST sign-in. A mention reaches people
-- who are not here, and whether they may read the space has to be answerable
-- without their session — which expires after twelve hours. As fresh as that
-- sign-in, and no fresher: see design-bell-and-mentions.md, "Known limit".
ALTER TABLE accounts ADD COLUMN handle TEXT;
ALTER TABLE accounts ADD COLUMN groups_json TEXT NOT NULL DEFAULT '[]';

CREATE UNIQUE INDEX accounts_by_handle ON accounts(handle) WHERE handle IS NOT NULL;
