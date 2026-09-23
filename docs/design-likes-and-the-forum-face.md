# Likes, and the face of the forum — design

Stage 7. `design.md` stays the authority for everything else; this file says
what a like is, how it reaches the bell, and what changes on the pages of a
`topics` space so that a reader finds their way. The order of work goes into
`plan-stage-7.md`.

## Why

A request from the operator, 2026-09-23: *likes for forum and blog — every
post shows its number of likes and a like button; whoever is liked gets a
notification, but no mail. And the forum should become clearer: structure,
overview, the way around.*

`design.md` §5 listed reactions under "left out permanently", with the rule
that whoever misses one changes that section before writing code. This stage
is that change, and [ADR 0008](decisions/0008-a-like-is-not-a-reaction.md)
says where the line now runs: **one like**, not a palette. A like says "I read
this and I am glad it is here"; a palette of emoji would turn every post into
a poll about itself.

## What a like is

- **One per person and post**, on any post: the opening post of a topic, a
  reply, a mirrored article, a comment under it. A second click takes it
  back. Nobody likes their own post — the button is not shown on it, and the
  route refuses it.
- **Shown at the foot of every post**: a heart drawn inline in `currentColor`
  like the pencil and the basket, and the number beside it. Pressed, the heart
  fills in the signal colour. On your own post the heart and the number stand
  without a button, and only when the number is not zero.
- **A form, and a script that makes it silent.** `POST /p/{id}/like` toggles
  and answers with a redirect to the post (`/t/{topic}#p{id}`); with
  `Accept: application/json` it answers `{"liked": bool, "count": n}`
  instead. `like.js` — the third script treff ships, on the terms of ADR 0005
  — sends the form with `fetch` and sets the number and the pressed state in
  place. Without it the page reloads at the post. Nothing depends on it.
- **Space and rights as everywhere.** The post is looked up in the space of
  the `Host`; one from the other address is 404, not 403. Liking needs
  `may_read` of the space and nothing more: reading is what a like is a
  reaction to, and somebody who may read but not reply may still say so.
- **Deleting a post deletes its likes** (`ON DELETE CASCADE`), as with
  attachments and entries. Editing changes nothing about them.

## Who is told

- **The author of the post, in the bell, never by mail.** The mail path is
  the outbox, and no like ever writes a row there — "no mail" is a property
  of the code, not a switch somebody can flip.
- **Bundled per post.** One entry per post says "ada likes your post in
  *Topic*", "ada and 2 others like your post in *Topic*": the number and the
  latest name come from the `likes` table at the time the bell is read, so
  the entry is always as true as the count under the post.
- **A new like re-opens a read entry.** The row stays one row; its `read_at`
  goes back to `NULL` and its `created_at` to now. "You have 3 likes, one of
  which you saw" is not a notification; "a new like" is.
- **The last like taken back removes the entry**, read or not. A bell that
  says somebody likes your post when nobody does would be a lie the reader
  can check.
- **Written in the same transaction as the like**, for the reason the reply
  entries are: a like that exists while its entry does not is a like nobody
  hears about, and nothing later can tell that it happened. `db.changed(space)`
  after the commit, so an open page's bell moves.
- **Opening the topic reads it**, like every other entry of that topic;
  "mark all as read" includes it. The badge counts one per post with an
  unread like entry.
- **An article has an owner** (0.9.1). A mirrored article is stored under a
  name nobody signs in as, so a like on it would reach nobody. A space may
  name `articles_owner`, the handle of the person who really writes them:
  the like on an article's opening post then tells that account, on that
  space. Comments under an article keep their own authors. The owner may
  like an article like anybody else (0.10.1; refused until then, which
  took the heart away from the one person who reads every article): the
  name on it is not theirs and the count is everybody's — they just get no
  bell for their own click. A handle no account answers to means the like
  counts and nobody is told — no guessing, no entry for a name that is not
  a person here.

## The face of the forum

Everything below is for a `topics` space; the timeline gets the like control
and, under each entry, the number of comments as a link to them. The look
stays the terminal it is (`style.css`, first comment): what changes is that
the pages answer more of a reader's questions in the same voice.

- **The front page shows where something is going on.** The section cards
  stay. Under them, `[ RECENT ]`: the ten topics that moved last, across all
  sections, each with its section, its replies and likes, and who wrote
  last. A front page that lists sections and nothing else sends the reader
  into every one of them to find out that nothing happened.
- **A section line on every section page.** Above the list, the sections in
  a row, the current one marked (`aria-current="page"`), the way to the front
  page in front of them as `../`. Today a section page has no way sideways
  and no way up but the brand.
- **The topic list answers three more questions.** Beside the subject: how
  many replies, how many likes (of the opening post — the number a reader
  sees when they open it), and a mark on a topic that holds something unread
  for *this* reader, in the same `*` the bell's list uses. On a phone the
  numbers stack under the subject as the last-reply column already does.
- **A topic page says how long it is and where you are.** Under the title, a
  meta line: the section, the number of replies, and the follow control as a
  quiet link-styled button rather than a boxed button pulling at the title.
  Every post carries its number (`#1`, `#2`) as a link to its own anchor, so
  a post can be pointed at. The reply fold is set off from the last post by
  a rule and a little air, and at the foot of a long thread a `top` link
  leads back up.

## Data

- `likes(post_id INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  subject TEXT NOT NULL, name TEXT NOT NULL, created_at INTEGER NOT NULL,
  PRIMARY KEY (post_id, subject))`. The name is copied in as it is on
  `posts`: a liker who leaves the provider keeps their name on what they
  liked, and no join reaches into `accounts` for a number the page shows on
  every post.
- `inbox.reason` gains `like`. SQLite cannot widen a `CHECK`, so migration
  `0014_likes.sql` rebuilds the table in place (create, copy, drop, rename,
  index again) — the rows stay what they are.
- The list queries grow two subqueries per topic (`replies`, `likes`) and,
  for the unread mark, one `EXISTS` against `inbox` for the viewer. One
  query per page, as before.

## Testing

Test the route, not the function (`CLAUDE.md`):

- a like through the router: the count under the post, the pressed state
  for the liker, a second `POST` takes it back; `Accept: application/json`
  answers the number without a page;
- your own post: no button on the page, `403` at the route;
- a post of the other space is `404`; a post that is gone is `404`;
- the author's bell: one entry per post, the count and the latest name, the
  link to the post; the liker's own bell stays empty; **no outbox row**;
- a read entry re-opens on the next like; the last unlike removes the entry;
  opening the topic marks it read; the badge counts it once;
- a deleted post takes its likes and the entry with it;
- `/notifications.json` carries the entry as `kind: "likes"`, and the bell
  script renders it (the JS tests under `tests/js`);
- the pages carry no inline script and no `on…` attribute — the test that
  already holds for `mention.js` and `bell.js` covers `like.js` too;
- the front page lists the recent topics with their sections; the section
  line marks the current section; the topic list shows replies, likes and
  the unread mark for the reader who has one and not for one who has not;
  the topic page numbers its posts;
- `cargo test --test preview -- --ignored`, then the pages in a browser, at
  desktop width and at 390px — before and after, side by side.

## Not in this stage

Emoji reactions, a list of who liked (beyond the names in the heart's
`title`), sorting by likes, likes on topics as opposed to posts, a mail.
