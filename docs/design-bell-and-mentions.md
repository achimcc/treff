# The bell and @-mentions — design

Stage 3. `design.md` stays the authority for everything else; this file says
what the bell and the mentions are, and why they are shaped this way. The
order of work goes into `plan-stage-3.md` once this is agreed.

## Why

A request from a user, 2026-09-22: *a bell on the forum with notifications
for new posts in threads you have written in — "XY replied" — so that you
have everything at a glance. You get a mail, but this would be one place.*
And from the operator, the same day: *it should also be possible to tag other
users with @, and they should be notified.*

The mail answers "something happened". The bell answers "what have I missed",
and it answers it where the reading happens. Neither replaces the other.

## What it is

- **A bell in the header of every page**, for a signed-in person, with the
  number of unread entries. It is a link to `/notifications`. The site
  ships no script today (and the CSP of design §6 forbids inline script); the
  bell does not change that. The number is rendered on the server and is as
  fresh as the page.
- **`/notifications`**: unread entries first, then the most recent read ones.
  Each entry links to the post it is about.
- **Two kinds of entry:**
  - **Replies, bundled per topic.** "3 new replies in *Topic*, latest from
    Konrad · 2 h ago". The link goes to the *first unread* post of the
    bundle. One bundle per topic among the unread, one among the read.
  - **Mentions, one each.** "Konrad mentioned you in *Topic*". A mention is
    addressed to one person; bundling it with other people's replies would
    hide exactly the one that was meant for them.
- **Read** means: the topic was opened. Opening `/t/{id}` marks every entry
  of that topic as read for that person — replies and mentions alike. There
  is also a "mark all as read" button, a `POST`.
- **One bell everywhere** (0.10.0; until then: per space). The bell on
  `forum.…`, the bell on `blog.…` and the bell on the operator's start page
  show the same entries — replies, mentions, likes and events from every
  space the person may read. The operator asked for it on 2026-09-23, after
  likes on the blog's articles started ringing a bell nobody was looking at.
  A link leads to the host its entry belongs to: relative on that host,
  `https://<host>/…` everywhere else. "Mark all as read" empties the whole
  bell. **The one boundary that stays is reading:** every query takes the
  list of spaces the viewer's groups may read (`authz::may_read`), and an
  entry left in a space somebody may no longer read is shown on no host —
  the same fail-closed rule as for a mention.

### Posts are flat — so it says "replied in", not "replied to you"

`posts` has no `reply_to`. "XY replied to your comment" would be a claim the
data cannot back. The entry says what is true: XY replied **in** a topic you
follow. A quote-and-reply feature would change that; it is not part of this
stage.

## Who gets an entry

### Replies

Exactly the people who get the mail today: everybody in `subscriptions` for
that topic, except the writer. The entry is written in the **same
transaction** as the post and its outbox rows — the same argument as for the
outbox: a post that exists while its notification does not is a silent,
permanent inconsistency. Unfollowing a topic stops new entries; existing
ones stay until read.

### Mentions

`@handle` in a post's Markdown mentions the account with that handle.

- **The handle is the provider's `preferred_username`**, lower-cased, stored
  on `accounts` at every sign-in. Not the display name: names have spaces and
  namesakes (`authz::Identity` already says why the name is not the person).
  A provider that sends no `preferred_username` gives that account no handle
  — it cannot be mentioned, and nothing else changes. Handles are matched
  case-insensitively; `[a-z0-9._-]`, up to 64 characters.
- **Only people who have signed in at least once can be mentioned.** Somebody
  who never came has no account row, no address and no handle. An `@handle`
  that matches nobody is plain text.
- **Shown next to the name on every post** (`Konrad · @konrad`), so a handle
  can be copied. There is no autocomplete: that needs script.
- **Where it counts:** in text, not in code spans, code blocks or links. A
  mention needs a boundary in front of the `@` — `user@example.org` is an
  address, not a mention. Found on the comrak AST, not with a regex over the
  source, for the same reason the renderer is not a regex.
- **Rendered** as a highlighted name (`<span class="mention">@konrad</span>`)
  — only for a handle that *resolves and may read the space* (below); every
  other `@word` stays as written, so the page never confirms that somebody
  exists who may not see it.

#### A mention must not leak the topic — fail closed

Reading is decided per space by groups (`authz::may_read`). A mention of
somebody who may not read the space would hand them the topic title and an
excerpt, by bell and by mail. treff keeps groups only on the **session**
(twelve hours), so it cannot currently answer "may this person read this
space" for somebody who is not signed in.

- **The groups of the last sign-in are stored on `accounts`** (`groups_json`,
  refreshed at every sign-in, like the address).
- **A mention creates an entry only if those groups pass `may_read` for the
  space.** Otherwise nothing happens: no entry, no mail, no highlight — and
  **no message to the writer**, because "@x may not read this" is itself an
  answer about x.
- **Checked again when the mail is composed**, against the account row as it
  is then. A person removed from a group between post and send gets nothing.
- Known limit, stated rather than hidden: the stored groups are as fresh as
  the last sign-in. Somebody removed from a group at the provider keeps
  reaching the check with the old groups until they sign in again — the same
  window the existing reply mail already has, since subscriptions do not know
  about groups at all. The honest fix for both is the same and outside this
  stage (a provider-side lookup); the stage must not make it worse.

#### What a mention does and does not do

- It **notifies** — an entry in the bell and a mail.
- It does **not** subscribe. Following stays something you do by writing.
- **One notification per person and post**, enforced by a primary key. A
  follower who is also mentioned gets the mention and not the reply as well
  — in the bell and in the mail. The mention is the more personal one.
- **Mentioning yourself** does nothing.
- **Editing:** a post that gains a mention on edit notifies the newly
  mentioned person; a mention that was already there does not notify again,
  and removing a mention does not take back what was sent.
- A deleted post takes its entries with it (`ON DELETE CASCADE`).

## The mail for a mention

Through the existing outbox, as a new kind of row (`reason = 'mention'`
beside the existing replies), with its own wording ("Konrad mentioned you in
…"). The reply mail's one-click link unfollows a topic, which means nothing
to somebody who never followed it. The mention mail therefore carries a
one-click link that **turns off mention mails for that account**
(`accounts.mention_mail`, default on) — signed the same way as today's link,
reachable without signing in, and re-enabled from `/notifications`. The bell
is not affected by it.

## Data

- `accounts`: add `handle TEXT` (unique where not null), `groups_json TEXT`,
  `mention_mail INTEGER NOT NULL DEFAULT 1`.
- `inbox`: `subject`, `space`, `topic_id`, `post_id`, `reason`
  (`reply`|`mention`), `created_at`, `read_at`; primary key
  `(subject, post_id)`; `ON DELETE CASCADE` on post and topic; an index on
  `(subject, space, read_at)` for the badge.
- `outbox`: add `reason`, default `reply`, so existing rows stay what they
  are.

The badge counts **unread mentions plus topics with unread replies** — the
same units the page shows.

## Testing

Test the route, not the function (`CLAUDE.md`):

- the badge number and the page, through the router, for replies bundled per
  topic and mentions one by one;
- no entry for your own post; opening a topic marks its entries read;
  "mark all as read";
- a mention of somebody who may not read the space: no entry, no outbox row,
  no highlight — and the rendered page identical to an unknown `@word`;
- a mention in a code span, a code block, a link and an e-mail address:
  nothing;
- a follower who is mentioned: one entry, one mail;
- an edit that adds a mention notifies once, and a second edit does not;
- the mention mail's link, without a session, turns mention mails off;
- `cargo test --test preview -- --ignored` for the bell and the page, read by
  eye.

And one measurement the repository cannot make: that the deployed provider
actually sends `preferred_username` in the ID token (or userinfo). Without it
nobody has a handle and every mention is plain text, with every test green.

## Not in this stage

Autocomplete, quote-and-reply, push to the browser, a live-updating badge,
per-topic mute, mentioning a group.
