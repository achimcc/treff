# Stage 3 — the implementation plan

Five tasks, worked one at a time and test first. `design-bell-and-mentions.md`
says *what* and *why*; this file says *in which order* and *what counts as
done*. The working rules are in `../CLAUDE.md` and `../AGENTS.md`: test first
and see it red, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --check`, a targeted `git add`, and `nix flake check` before the
stage is called done.

## Status

| | |
|---|---|
| Open | **Task 1** — a handle and the groups on the account |
| Open | **Task 2** — the inbox, filled by replies |
| Open | **Task 3** — the bell and `/notifications` |
| Open | **Task 4** — `@handle`: found, checked, noted, highlighted |
| Open | **Task 5** — the mention mail, and its way out |

---

## Task 1 · A handle and the groups on the account

**Files:** `migrations/0009_inbox.sql` (this task's half), `src/authz.rs`,
`src/auth/mod.rs`, `src/auth/oidc.rs`, every `Identity { … }` literal,
`tests/common/mod.rs`

- [ ] `Identity` gains `handle: Option<String>`.
- [ ] `claims_to_identity` reads `preferred_username`: trimmed,
      lower-cased, kept only if it matches `^[a-z0-9._-]{1,64}$`. Anything
      else — missing, not a string, too long, a space in it — is `None`, never
      a repaired guess. Unit tests for each of those.
- [ ] `finish_login` hands the verified payload's `preferred_username` through
      that function (it already reads the claim for the name fallback; the
      payload `extra` carries it).
- [ ] Migration: `accounts` gains `handle TEXT`, `groups_json TEXT NOT NULL
      DEFAULT '[]'`, `mention_mail INTEGER NOT NULL DEFAULT 1`, and a unique
      index on `handle` where it is not null.
- [ ] `Sessions::create` writes handle and groups on every sign-in. A handle
      that another account already holds is written as `NULL` for the
      newcomer rather than failing the sign-in — a sign-in must not break over
      a nickname, and two people answering to one handle would hand one of
      them the other's mentions. Test: two subjects, one handle.
- [ ] `tests/common::signed_in` gives every test person the handle
      `<subject>`, so later tests can mention them by it.

**Done when** a sign-in stores handle and groups on the account row, and the
unit tests for the claim are red-then-green.

## Task 2 · The inbox, filled by replies

**Files:** `migrations/0009_inbox.sql`, `src/db/inbox.rs` (new),
`src/db/mod.rs`, `src/db/topics.rs`

- [ ] Table `inbox (subject, space, topic_id, post_id, reason, created_at,
      read_at)`, primary key `(subject, post_id)`, cascades on topic and post,
      index `(subject, space, read_at)`.
- [ ] `add_reply` writes one `reply` row per follower except the writer, in
      its transaction, `ON CONFLICT DO NOTHING`.
- [ ] `db::inbox`:
  - `unread_count(db, subject, space) -> i64` — unread mentions plus topics
    with unread replies;
  - `entries(db, subject, space, limit) -> Vec<Entry>` — unread first, newest
    first; replies bundled per `(topic, read state)` with count, latest
    author, latest time and the first unread post; mentions one by one;
  - `mark_topic_read(db, subject, topic_id)`, `mark_all_read(db, subject,
    space)`.
- [ ] Tests in the module: no row for the writer; three replies in one topic
      are one bundle of three; two topics are two bundles; marking a topic
      read empties its bundle and not the other; the space is part of every
      query.

## Task 3 · The bell and `/notifications`

**Files:** `src/web/mod.rs`, `src/web/views.rs`, `src/web/style.css`,
`i18n/de.toml`, `i18n/en.toml`, `tests/inbox.rs` (new), `tests/preview.rs`

- [ ] `layout` takes the unread count and draws the bell in the header — an
      inline SVG like the pencil, a link to `/notifications`, the number only
      when it is not zero, and an `aria-label` that says it in words. Every
      handler that renders a page asks for the count **after** it has changed
      anything (a topic page marks itself read first, so its own bundle is not
      counted on the page that just cleared it).
- [ ] `GET /notifications` lists the entries; `POST /notifications/read`
      marks all read and redirects back.
- [ ] `GET /t/{id}` marks that topic's entries read for the viewer.
- [ ] Route tests: the badge after somebody else replies, no badge after your
      own reply, the badge gone after opening the topic, the bundle wording,
      "mark all as read", and the page answering only inside its own space.
- [ ] Preview pages for the bell with and without a number, and for the list.
      Opened and read, not only generated.

## Task 4 · `@handle`: found, checked, noted, highlighted

**Files:** `src/markup.rs`, `src/db/inbox.rs`, `src/db/accounts.rs` (new),
`src/db/topics.rs`, `src/web/mod.rs`, `src/web/views.rs`, `src/web/style.css`,
`tests/mentions.rs` (new)

- [ ] `markup::mentions(markdown) -> Vec<String>`, on the comrak AST: text
      nodes only (not code, not inside links), an `@` at the start or after a
      character that is not `[A-Za-z0-9._%+-]`, the handle pattern of task 1,
      lower-cased, deduplicated. Unit tests: plain, start of text, after
      punctuation, `user@example.org`, a code span, a code block, a link text,
      a URL, uppercase.
- [ ] `db::accounts::mentionable(db, handles) -> Vec<(subject, handle, groups)>`
      and, in the handler, `authz::may_read` against the space on those
      groups. What survives is the list of subjects to notify; the writer is
      removed from it.
- [ ] `create_topic`, `add_reply` and `update_post` get `…_mentioning`
      variants taking that list; the old functions call them with an empty
      list, so the existing callers stay as they are. Inside the transaction,
      **mentions first**: one `mention` row per subject, `ON CONFLICT DO
      NOTHING`; only a row that was actually inserted queues its mail (task
      5). The reply rows come after and therefore lose the conflict for a
      follower who was mentioned — one entry, and the follower's reply mail is
      left out for exactly those subjects.
- [ ] Rendering: `markup::render_with(markdown, &mentionable_handles)` wraps a
      mentionable `@handle` in `<span class="mention">`; the set comes from the
      same check as above, computed once per topic page. Everything else stays
      text — an unknown handle and a forbidden one render identically.
- [ ] The handle next to the author's name on each post, from `accounts`.
- [ ] Route tests: mention of a reader → one entry; of somebody without the
      space's groups → nothing and no highlight, page byte-identical to an
      unknown handle; of yourself → nothing; follower mentioned → one entry;
      edit that adds a mention → one entry, a second edit → still one.

## Task 5 · The mention mail, and its way out

**Files:** `migrations/0009_inbox.sql` (`outbox.reason`), `src/db/outbox.rs`,
`src/notify/mod.rs`, `src/notify/mail.rs`, `src/web/mod.rs`,
`src/web/views.rs`, `i18n/*.toml`, `tests/notifying.rs`, `tests/mentions.rs`

- [ ] `outbox.reason TEXT NOT NULL DEFAULT 'reply'`; a mention queues
      `reason = 'mention'` for the mentioned subject, but only if
      `accounts.mention_mail = 1`.
- [ ] `compose` checks a mention again at send time: the account's stored
      groups must still pass `may_read` for the space, or the row is done with
      nothing to send. `Message` gains the reason, and the mail's subject and
      first line say "mentioned you" instead of "replied".
- [ ] The existing `/u/{id}/{token}` link looks at the row's reason: for a
      reply it unfollows (as now), for a mention it sets `mention_mail = 0`.
      The page's question and answer say which.
- [ ] A switch on `/notifications` to turn mention mails back on (and off),
      a `POST`.
- [ ] Tests: the mail over the one-shot SMTP server says "mentioned"; a row
      whose recipient lost the group sends nothing; the link turns mention
      mails off without a session and is idempotent; after that a mention
      still reaches the bell but queues no mail.

## After the stage

- [ ] `cargo test --test preview -- --ignored`, pages opened.
- [ ] `nix flake check`.
- [ ] Version `0.5.0`, CHANGELOG, README section on notifications.
- [ ] Deployment (homeserver repository): signed tag, `flake.nix` **and**
      `nix flake update treff`, build, deploy, newsletter entry.
- [ ] **Measured at the deployment:** that the provider sends
      `preferred_username` — the `handle` column of a real account after a
      real sign-in is not `NULL`. Without that, every mention is plain text
      and every test is green.
