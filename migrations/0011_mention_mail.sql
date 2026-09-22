-- Why a mail is owed, and whether a person wants mail for a mention at all.
--
-- `reason` decides the wording ("replied" or "mentioned you"), the check at
-- send time (a mention is checked against the space's groups again; a reply
-- is what it always was) and what the one-click link in the mail does: a
-- reply's link unfollows the topic, a mention's link turns off mention mails —
-- unfollowing a topic means nothing to somebody who never followed it.
ALTER TABLE outbox ADD COLUMN reason TEXT NOT NULL DEFAULT 'reply'
    CHECK (reason IN ('reply', 'mention'));

-- On unless somebody said otherwise. The bell is not affected by it: this is
-- about mail, and the bell is where one looks on purpose.
ALTER TABLE accounts ADD COLUMN mention_mail INTEGER NOT NULL DEFAULT 1;
