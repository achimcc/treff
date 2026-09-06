# Stage 1 — the implementation plan

Sixteen tasks, plus two added on 2026-09-06, worked one at a time and test
first. Each task ends in something that runs and can be checked on its own. `design.md` says *what* and *why*;
this file says *in which order* and *what counts as done*.

**How to work a task:** write the test, **see it fail** with the expected
message, implement the smallest thing that passes, see it green
(`cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt
--check`), commit with a targeted `git add`. The working rules are in
`../AGENTS.md`.

## Status

| | |
|---|---|
| Done | **Task 1** — crate, AGPL-3.0, flake, smoke tests (`cd9f1f2`, `068d5d6`) |
| Done | **Task 2** — configuration: spaces, categories, and the rules that refuse a broken file |
| Done | **Task 3** — permissions as pure functions |
| Done | **Task 4** — database: schema, WAL, enforced foreign keys, migrations |
| Done | **Task 5** — topics and posts: storage, ordering, space isolation |
| Done | **Task 6** — Markdown rendered and sanitized |
| Done | **Task 7** — sign-in: settings, claims, sessions, and the OIDC flow |
| Done | **Task 8** — the web frame: host to space, sign-in gate, security headers |
| Done | **Task 9** — reading: timeline, topic list, topic page |
| Done | **Task 9b** — the category overview |
| Done | **Task 9c** — articles mirrored from a directory |
| Done | **Task 10** — writing: open a topic, reply |
| Done | **Task 11** — editing and deleting, only your own |
| Done | **Task 12** — attachments: magic bytes, storage, serving |
| Done | **Task 13** — languages: German and English |
| Done | **Task 14** — `treff export`: a backup that survives the snapshot |
| Done | **Task 15** — the NixOS module and the VM test |
| Next | **Task 16** — release v0.1.0 (see "Waiting for a decision") |
| Public | not yet; the repository goes public with task 16, so the first
impression is a finished thing and not three commits without a README |

Evidence recorded for task 1: crates.io answered 404 for the name (it is
free); the smoke tests were seen red (`0 passed; 2 failed`) before they were
seen green (`2 passed`); `nix build` produced a store path and
`treff --version` printed `treff 0.1.0`; `nix flake check` passed.

Three deliberate departures from the original plan, so nobody mistakes them
for mistakes: **edition 2024** (what `cargo init` picks, and the toolchain is
new enough); the AGPL text came from the GitHub licence API rather than a
`curl` to gnu.org, and was checked for completeness (662 lines, §13 Remote
Network Interaction present); **no `repository` field** in `Cargo.toml` until
the repository has an address (task 16).

---

## Task 1 · Repository, licence, flake ✅

**Files:** `Cargo.toml`, `src/main.rs`, `LICENSE`, `README.md`, `flake.nix`,
`.gitignore`, `tests/smoke.rs`
**Produces:** the `treff` binary; `nix build` yields `result/bin/treff`;
`nix flake check` runs.

**Done when:** `treff --version` prints the crate version, and `treff` with no
configuration **fails** to start (fail closed — a build that started
unconfigured would serve a forum with no idea who anyone is).

## Task 2 · Configuration: spaces, categories, groups ✅

**Files:** add `src/config.rs`; `src/lib.rs` gains `pub mod config;`
**Produces:** `Config`, `Space`, `Category`, `View`, `ConfigError`;
`Config::parse(&str) -> Result<Config, ConfigError>`;
`Config::space_for_host(&self, host: &str) -> Option<&Space>`;
`Space::category(&self, slug: &str) -> Option<&Category>`.

**Assertions to write first:**

- The example from `design.md` §4 parses, and both spaces come out with their
  categories.
- An unknown host yields `None` — **not** the first space.
- Two spaces with the same `host` are a parse **error**, not a silent
  last-one-wins.
- An unknown `view` value is an error; the two known ones map to `View`.
- An empty `read`/`post`/`reply` list is accepted and grants nobody (it is
  never read as "everyone").
- A category slug that is not URL-safe is an error.

**Two departures, both deliberate:**

1. **`deny_unknown_fields` on every struct**, with a test for it. Without it a
   typo like `psot = ["Writers"]` parses happily, leaves `post` empty, and the
   category quietly admits nobody. Failing closed is right; failing *silently*
   is not. The test was checked in both directions — with the attribute
   removed it fails, so it is a test and not decoration.
2. **The crate gained `src/lib.rs`.** A binary-only crate makes
   `clippy --all-targets -- -D warnings` unreachable — everything the tests
   use is dead code from the binary's point of view — and the integration
   tests from task 9 onwards need something to call anyway. `src/main.rs`
   stays a thin front end.

The slug rule is strict on purpose: lower-case ASCII letters, digits and
hyphens, non-empty. A slug ends up in a URL path, so anything else is either a
routing bug or an attempt at one.

## Task 3 · Permissions as pure functions ✅

**Files:** add `src/authz.rs`; `src/lib.rs` gains `pub mod authz;`
**Consumes:** `Space`, `Category`.
**Produces:** `Identity { subject, name, groups }`,
`Identity::in_any(&self, &[String]) -> bool`,
`may_read(&Identity, &Space) -> bool`, `may_post(&Identity, &Category) -> bool`,
`may_reply(&Identity, &Category) -> bool`,
`may_modify(&Identity, author_subject: &str) -> bool`.

This file knows **no HTTP and no database**. That is exactly why the rules can
be tested exhaustively here, and why every later layer asks these functions
instead of re-deciding.

**Assertions to write first:** an identity with no groups may nothing; group
matching is exact (no prefix, no case folding); `may_modify` compares the
`subject`, never the display name — a renamed account keeps its posts, and two
people with the same name are two people.

## Task 4 · Database: schema, pragmas, migrations ✅

**Files:** add `src/db/mod.rs`, `migrations/0001_initial.sql`; `mod db;`
**Produces:** `Db` (holds the `SqlitePool`), `Db::open(path: &Path)`,
`Db::pool(&self) -> &SqlitePool`.

**Assertions to write first:** a fresh file gets the schema; opening it again
is idempotent; **`journal_mode` reads back as `wal` and `foreign_keys` as
`1`** on a connection taken from the pool (not on the one that ran the
migration); a delete of a topic takes its posts with it.

**Measured while doing it:** `foreign_keys(true)` restates a sqlx default.
Deleting the line leaves the three constraint tests **green** — only
`foreign_keys(false)` turns them red, and `journal_mode(Delete)` turns the WAL
test red. Both directions were checked rather than assumed, and the line stays:
`ON DELETE CASCADE` in the schema depends on it, and a promise that lives in
someone else's default is not one we made. `foreign_keys` is **per
connection**, so the test holds four pooled connections at once and asks each
of them — asking one proves nothing about the rest.

Also: `sqlx` needs the **`macros`** feature for `sqlx::migrate!`, not just
`migrate` — the plan's `cargo add` line was one feature short.

## Task 5 · Topics and posts: the storage layer ✅

**Files:** add `src/db/topics.rs`; `src/db/mod.rs` gains `pub mod topics;`
**Produces:** `Topic { id, space, category, title, author_subject, author_name,
created_at, updated_at }`, `Post { id, topic_id, body_markdown,
author_subject, author_name, created_at, updated_at, edited }`,
`create_topic`, `add_reply`, `list_topics(space, category, limit, offset)`,
`load_topic(space, id)`.

**Assertions to write first:** a created topic comes back with its first post;
`list_topics` orders by last reply for `topics` and by creation for
`timeline`; `load_topic` **scopes by space** — a topic from another space is
`None`, not a permission error later on; paging is stable.

**Added while doing it:** `list_topics` **clamps** limit and offset instead of
trusting them — these numbers arrive from a query string one day, and
`LIMIT -1` means "everything" in SQLite. And the ordering test sets the
timestamps by hand: two topics created in the same second have the same
`updated_at`, so "a reply lifts its topic" is not observable at second
resolution otherwise.

**Honest about one test:** `a_refused_reply_stores_no_post` proves the foreign
key, not the transaction — the first statement is the one that fails, so there
is nothing to roll back. The rollback path has no cheap trigger (the second
statement is an UPDATE that cannot fail), so it is left untested rather than
fake-tested.

## Task 6 · Rendering and sanitizing Markdown ✅

**Files:** add `src/markup.rs`; `mod markup;`
**Produces:** `render(markdown: &str) -> String`, returning **sanitized** HTML.

The second security-relevant place after attachments, so the tests describe
the attack, not the function: `<script>`, `javascript:` and `data:` URLs, an
`onerror=` attribute, a raw HTML block, a nested/obfuscated form of each — all
must come out inert. Ordinary Markdown (emphasis, lists, code, links,
autolinks) must survive.

**Measured, not assumed:** with the sanitizer removed and comrak set to
`render.r#unsafe = true`, nine of the twelve tests go red — they describe the
attack and they can see it. `comrak` 0.54 spells the field `render.r#unsafe`,
not `render.unsafe_`.

**Two things the first draft of these tests got wrong**, both worth keeping in
mind for the attachment tests later: searching the output for the string
`"javascript:"` gives a false positive, because one disguise never becomes a
link at all and stays as visible escaped text — harmless, but it matches. And
ammonia does not delete a link with a forbidden scheme, it empties the
attribute (`href=""`), so "no `href=` at all" is the wrong assertion too. What
the test now checks is every attribute VALUE in the output.

## Task 7 · Sign-in: OIDC, session, sign-out ✅

**Files:** add `src/auth/mod.rs`, `src/auth/oidc.rs`; `mod auth;`
**Consumes:** `Identity`, `Db`.
**Produces:** `OidcSettings { issuer, client_id, client_secret, group_claim }`,
`OidcSettings::from_env()`,
`claims_to_identity(subject, name, claims, group_claim) -> Identity`,
`Sessions::create(&Db, &Identity) -> Result<String>`,
`Sessions::load(&Db, id) -> Result<Option<Identity>>`,
`Sessions::destroy(&Db, id)`.

**First step of this task is to check the library's API names against
`cargo doc`**, not to trust the names written here.

**Assertions to write first:** `from_env` fails when issuer or client id is
missing, and when the secret **file** is absent; the groups claim is read by
its configured name and a missing claim yields **no** groups (not all);
a claim that is a string rather than an array is rejected rather than
guessed at; `create`/`load`/`destroy` round-trip, and a destroyed session
loads as `None`; the discovery and token exchange are tested against a mock
HTTP server, so the suite never needs a real provider.

**Checking the API names at the source paid off twice.** `rand` 0.10 has
`rngs::SysRng` with the `TryRng` trait, not `OsRng` with `RngCore` as the plan
assumed. And `openidconnect` 4 makes the endpoint markers part of the client
type, so storing a configured client means naming all six of them in a type
alias. A third one only the mock server could tell us: **discovery fetches the
JWKS in the same call**, so a stub that answers only
`/.well-known/openid-configuration` fails with a 404 that reads like a wrong
URL.

**Deliberately not tested:** the ID token signature and nonce check. That is
the library's job, and imitating it with home-made fixtures would test the
library rather than this code. What is tested is what we decide — and the
`state` check runs **before** the provider is contacted at all. The test for
it leaves the token endpoint unmocked on purpose: if that order ever flipped,
the test would fail on a connection error instead of on its assertion.

**Beyond the plan:** session identifiers come from the system CSPRNG and a
failure to get randomness is an error rather than a weaker fallback — the
identifier *is* the credential. A session whose stored groups cannot be parsed
loads with **no** groups, so failing closed holds even against our own
storage.

## Task 8 · The web frame: host→space, sign-in gate, security headers ✅

**Files:** add `src/web/mod.rs`; `src/main.rs` starts the server
**Consumes:** `Config`, `Sessions`, `Db`.
**Produces:** `AppState { config, db, oidc, provider, cookie_key }`,
`AppState::new(...)`, `router(state) -> axum::Router`, extractors
`CurrentSpace` and `CurrentUser`.

**No `AppState::for_tests`, against the plan** — for two reasons that only
showed up while building it. Cargo refuses a self dev-dependency, which is the
usual way to switch on a `testing` feature for integration tests; without it
the constructor would either be missing from `tests/` or be compiled into the
shipped binary. And what it would carry there is a **fixed cookie key**. The
tests build the state from the public pieces instead (`tests/common/mod.rs`),
with `provider: None` — that is what keeps the suite independent of a
reachable identity provider, and it needs no special constructor.

**Added:** the cookie key is generated once and stored as `cookie.key` (mode
0600) in the data directory. A key made up per start would sign everyone out
on every restart although their sessions are still in the database.

**Assertions to write first:** an unknown `Host` is **403** (not a redirect to
the first space); a request without a session is redirected to sign-in, and
the redirect target is not attacker-controlled; every response carries the CSP
without `unsafe-inline`, `X-Content-Type-Options: nosniff`, a `Referrer-Policy`
and a `frame-ancestors` denial; the session cookie is `HttpOnly`, `Secure`,
`SameSite=Lax`.

**The gate is middleware in front of the routes**, not a check inside each
handler, so a page added later cannot forget it. `/auth/*` and `/assets/*` are
the only open paths — without that the redirect to the sign-in would point at
itself.

**Checked in both directions:** removing the host check drops two tests, and
commenting out the CSP layer drops a third. The header layers sit outermost on
purpose, so a refusal carries them too — a 403 is a page as well.

## Task 9 · Reading: timeline and topic list ✅

**Files:** add `src/web/views.rs`; change `src/web/mod.rs`
**Produces:** `views::layout`, `views::space_page`, `views::topic_page`;
routes `GET /`, `GET /c/:category`, `GET /t/:id`.

**Assertions to write first:** with a session, a member of the reading group
sees the rendered **bodies** on a `timeline` space and a **list of titles** on
a `topics` space; someone without the reading group gets **403**; a topic from
the blog is **404** through the forum's address; the page contains **no
`<script`** element at all.

## Task 9b · The category overview ✅

**Files:** change `src/web/views.rs`, `src/web/mod.rs`
**Produces:** `views::category_index`; `GET /` on a `topics` space lists the
categories instead of jumping into the first one.

Added on 2026-09-06, from the intended use: a forum with five categories
(films, series, off topic, feature requests, server services) has no sensible
"first" one, and the front page silently picked whichever the configuration
happened to list first.

**Assertions to write first:** the front page of a `topics` space names every
category the reader may see, with its topic count and the time of its last
activity; a category the reader may not `read` — that is, none, since `read`
is per space — still never appears through another space's address; a
`timeline` space keeps going straight to its entries, because a blog with one
category has nothing to choose from; an empty category is listed, not hidden.

**Done.** `category_counts` answers in one query rather than one per category,
with the space in the condition — two spaces may use the same slug, and
counting across them would put the blog's activity on the forum's front page
(there is a test for exactly that). Categories are listed in the order of the
configuration, not of the query: whoever wrote the file decided what comes
first. Counting **topics, not posts** — otherwise one busy thread makes a
category look busy.

Two things I got wrong on the way and corrected: a hand-rolled date routine
(leap years included) where `time` was already in the tree for the cookies,
and an assertion that searched the page for `"1 "` — a page is full of ones,
so it proved nothing. It now looks for the rendered count.

The existing test for the topic list moved from `/` to `/c/general`, because
that is where the list now lives. That change of behaviour is the point of
this task, not a casualty of it.

## Task 9c · Articles mirrored from a directory ✅

**Files:** add `src/articles.rs`, `migrations/0002_articles.sql`; change
`src/config.rs` (`articles` per space), `src/main.rs`
**Produces:** `articles::mirror(&Db, space, &Path) -> Result<Mirrored>`;
`topics.source_key` (UNIQUE, nullable).

Added on 2026-09-06 (design §4, "Articles that are written somewhere else").
The microblog is fed by the same Markdown files that announce a change
elsewhere — front matter `title`, date from the file name — so the article and
the announcement are one text and not two. Comments stay ordinary replies, so
everything already built keeps working.

**Assertions to write first:**

- A directory of two files becomes two topics, newest first by the date in the
  name; the front matter `title` becomes the title, the prose becomes the
  opening post.
- Mirroring **twice** changes nothing (idempotent, keyed by file name) — the
  test runs it three times and counts.
- An edited file updates title and body **and keeps the comments** underneath.
- A file that disappears stops being listed, and its comments survive in the
  database. Deleting what people wrote is not a side effect a file deletion
  may have.
- A file without front matter, with an unparsable date, or with a `title` that
  is empty is **skipped with a message on stderr**, and the other files still
  mirror. One broken article must not stop a start.
- The body goes through the same sanitizer as everything else: an article is
  not more trusted for coming from a file.
- A space **without** an `articles` directory is untouched; nothing mirrors
  into a forum by accident.
- A **missing directory** is an error and stops the start. A wrong path would
  otherwise look like a blog that is merely empty, which nobody reads as a
  mistake. One broken FILE is the opposite case and only a skip.
- **A file dated in the future does not appear**, and appears by itself once
  that day arrives (the test injects "today" rather than waiting). This is
  what lets an article be written while its subject is still being rolled out.
  The same limit was added to the other consumer of these files on the same
  day, so the two channels do not disagree about one text.

**Done.** Two things the assertions did not say, decided while building:

- A withdrawn article is **hidden, not deleted** — `ON DELETE CASCADE` would
  take the comments with it. `hidden = 0` therefore joins the WHERE clause of
  every read, and a file that comes back brings its article and its comments
  back with it. There is a test for the round trip.
- An article is stored under the subject `treff:article`, which is nobody. No
  account can edit it through the web, because `may_modify` compares subjects
  — which is right: the file decides.

And two of my own mistakes, both caught by the tests: `read_dir` gives no
order, so the first test wrongly assumed which file became topic 1; and
`hidden` was written but not yet read, so a withdrawn article stayed on the
page while the counter said it was gone.

## Task 10 · Writing: open a topic, reply ✅

**Files:** change `src/web/mod.rs`, `src/web/views.rs`
**Produces:** routes `POST /c/:category/new`, `POST /t/:id/reply`.

**Assertions to write first:** a member of the posting group opens a topic
(302 to it, and it is in the database); a member without that group gets
**403 and the database is unchanged** — that second half is the actual test;
replying follows `reply`, not `post`; an empty title or body is **400**; a
title over 200 characters is **400**; a body over 64 KiB is **400**; the form
page shows **no submit button** to someone who may not use it (display follows
the right).

**Done, with three notes:**

- The answer to a successful POST is **303**, not the 302 the plan named: 303
  is the code that turns a POST into a GET, so a reload does not post again.
- The limits check what they claim to: **characters** for the title, **bytes**
  for the body, with a unit test that pins both (an emoji is one character and
  four bytes). Whitespace counts as empty, so a title of three spaces is
  refused.
- `checked_title` / `checked_body` return a **reason**, not a response. clippy
  found the first version — a `Result<_, Response>` carries a very large error
  variant — and being made to fix it left the checks free of HTTP, which is
  where they belonged anyway.

**Also closed here: a promise from task 8 that was never checked.** The plan
asked for a session cookie that is `HttpOnly`, `Secure`, `SameSite=Lax`. It
was implemented and never asserted; `tests/frame.rs` now reads the real
`Set-Cookie` header. `SameSite=Lax` is what stops a form on another site from
posting here in someone else's name, so it is the CSRF defence this design
rests on — together with `form-action 'self'` in the CSP.

## Task 11 · Editing and deleting — only your own ✅

**Files:** change `src/web/mod.rs`, `src/db/topics.rs`, `src/web/views.rs`
**Produces:** `update_post(&Db, post_id, body, &Identity) -> Result<bool>`,
`delete_post(&Db, post_id, &Identity) -> Result<bool>`;
routes `POST /p/:id/edit`, `POST /p/:id/delete`.

**Assertions to write first:** the author may edit, and `edited` is set
afterwards; anyone else gets **403** and the text is unchanged; deleting the
first post of a topic is refused or takes the topic with it (decide once,
test it); the permission is enforced in the **query**, not only in the
handler.

**The decision the plan left open, made once:** deleting the **opening post**
takes the topic with it — the opening post *is* the topic — but only while
nobody else has written underneath. With someone else's reply hanging on it
the answer is **409**, because "only your own" has to mean that in both
directions: deleting yours must not delete theirs.

`Deleted` says which of the four things happened, and three different reasons
for "no" (not yours, not in this space, not there) share **one** answer —
telling them apart would say something about posts the asker may not see.

The space is part of both queries. Without it the author of a forum post could
edit it through the blog's address, and the two audiences would share a back
door; there is a test that calls the storage layer directly to prove it.

A mirrored article belongs to the subject `treff:article`, which is nobody, so
nobody edits an article through the web. That falls out of `may_modify`
comparing subjects — no special case was needed, and there is a test that
pins it.

## Task 12 · Attachments: magic bytes, storage, serving ✅

**Files:** add `src/media.rs`, `src/db/attachments.rs`; change `src/web/mod.rs`
**Produces:** `media::detect(&[u8]) -> Option<MediaType>`, `MediaType::mime()`,
`MediaType::extension()`; routes `POST /t/:id/attach`, `GET /a/:id`.

The only path on which someone else's bytes enter the server (`design.md` §6).

**Assertions to write first:** the four allowed types are detected from their
magic bytes; **an SVG, an HTML file and a script are rejected** even when
named `.png` and announced as `image/png`; a truncated or empty upload is
rejected; the size limit is enforced **while reading**, not after; the stored
name is generated, never taken from the client (no path traversal); `GET /a/:id`
answers with the detected type, `nosniff`, and only to someone who may read
the space.

**An upload is a reply with a picture in it.** It becomes a post of its own
whose body is `![](/a/<id>)`, so the image reaches the page through the same
renderer and the same sanitizer as everything else, and it follows the
`reply` right rather than a third one. That also answers the "attachments per
post" limit the design asks for: one upload is one post, so the count is
structurally one.

**Two limits, and both are needed**, as the plan said: a hard
`DefaultBodyLimit` that refuses before anything is read into memory, and the
per-space `attachment_max_bytes` that gives an answer a person can act on.
Getting the order wrong showed up immediately — the hard limit fired first and
answered **400**, so the test asked for 413 and got a bad request.

**A finding from writing these tests, and it is not about attachments:** the
sanitizer let an `img` from a foreign host through, because `https` is an
allowed scheme. The CSP says `img-src 'self'`, so a browser would refuse it —
but then the CSP is the only lock, and an image is fetched without anyone
deciding to, which tells its host who is reading and when. `markup::render`
now drops any `img src` that does not start with `/`. A **link** to another
site is still fine: following one is a decision.

## Task 13 · Languages: German and English ✅

**Files:** add `src/i18n.rs`, `i18n/de.toml`, `i18n/en.toml`; change
`src/web/views.rs`
**Produces:** `Lang` (`De`, `En`), `Lang::from_accept_language(&str) -> Lang`,
`Lang::t(&self, key: &str) -> &str`.

**Assertions to write first:** `de-DE,de;q=0.9` yields `De`, `en-US` yields
`En`, an unknown or empty header yields `En` (the project's default);
**every key in `en.toml` exists in `de.toml` and vice versa** — that test is
the actual point of the task, because a missing translation otherwise shows up
in a browser. Catalogues are embedded with `include_str!`; after this task no
user-visible text remains in `views.rs`.

## Task 14 · `treff export` — a backup that survives the snapshot ✅

**Files:** add `src/export.rs`; change `src/main.rs`
**Produces:** `export(db: &Db, target: &Path) -> Result<()>`; the
`treff export <file>` subcommand.

Not a convenience: a filesystem snapshot of a WAL-mode SQLite database catches
the data file and the log in an unknown relationship. `VACUUM INTO` writes a
self-contained file.

**Assertions to write first:** the exported file opens as a database and
contains the same rows; exporting **while a write is in flight** produces a
consistent file; an existing target is not silently overwritten; the exit code
and message are usable from a maintenance script.

**Done.** `sqlx` 0.9 refuses a dynamically built statement unless the caller
says out loud that it was audited (`AssertSqlSafe`), which is exactly the right
place for that sentence: `VACUUM INTO` takes a **literal** and no bound
parameter, so the path is checked here — for a quote and a null byte — rather
than made safe by a binding. It comes from the command line, never from a
request.

The concurrent test writes twenty topics from another task while the export
runs, then asserts that no topic in the copy is missing its opening post: a
copy holding half a transaction is the failure that matters, and "it did not
error" would not have seen it.

## Task 15 · The NixOS module and the VM test ✅

**Files:** add `nix/module.nix`, `nix/test.nix`; change `flake.nix`
**Produces:** `nixosModules.default` with
`services.treff.{enable, package, listen, dataDir, spaces,
oidc.{issuer, clientId, clientSecretFile, groupClaim}}`; `checks.<system>.vm`.

The module generates `spaces.toml` **from Nix**, so a typo in a group name
fails the build instead of becoming a silent category nobody may enter.

**Assertions to write first (in the VM):** the service comes up and answers on
its port; the secret arrives **without** being in the store or in the unit
file; an unknown `Host` is refused; the data directory and its owner are
created; a restart keeps sessions (they are in the database, not in memory).

**Done, and the VM test earned its keep before it was even green.** Writing it
found a real design fault: `serve()` discovered the identity provider at
startup, so a provider that is slow to come up after a power cut left treff
dead — the machine would sit there with a service that refuses to start
because a neighbour was late. Discovery now happens on **first sign-in** and a
failure is not remembered, so a provider that was merely slow recovers without
a restart. Nothing is opened by that: without a provider nobody signs in, and
every page needs a session. The VM test pins exactly this — the service runs,
`/` redirects, and `/auth/login` answers **503** in a VM that has no provider
at all.

**Checked in both directions:** with `TREFF_SABOTAGE = "the-client-secret"`
added to the unit's environment, the test fails with *"command `systemctl cat
treff.service | grep -q the-client-secret` unexpectedly succeeded"*. The
assertion that matters most is the one that can see its own violation.

The test also pins two things that only a real machine shows: the **cookie key
survives a restart** (a key made up per start would sign everyone out although
their sessions are in the database), and `treff export` runs against the live
database of the running service and writes a file with something in it.

The module carries four assertions that fail at **build** time: no spaces at
all, a space without a category, a space nobody may read, and two spaces
sharing a host.

## Task 16 · Release v0.1.0 — steps 1 to 3 done

- ✅ `README.md`: a complete `spaces.toml` example, the environment variables, a
  **"Behind a reverse proxy"** section (the `Host` header must be passed
  through, or every request lands in the 403 branch), and a "Configuring your
  provider" section with **Authentik and Keycloak** as worked examples. The
  Keycloak part carries the trap that costs an evening: *Full group path* must
  be **off**, or the claim reads `/Household` and treff compares exactly.
- ✅ `CHANGELOG.md`, under **Unreleased** rather than `0.1.0` — the version is
  a decision, and it is in "Waiting for a decision" above.
- ✅ CI, which was part of stage 1 all along (design §5, "package, NixOS
  module, tests, CI") and had quietly never been written: one workflow runs
  `nix flake check` so CI and a local check cannot drift apart, a second runs
  `cargo fmt --check` and clippy because a red one of those should be readable
  without scrolling through a VM boot. Plus `renovate.json` with the Nix
  manager on, as the design asks for.
- ⏸ The remaining steps — version, signed tag, making the repository public —
  wait for a decision.
- Everything together: `cargo fmt --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo test`, `nix flake check`. **Read the number of passing
  tests, not the exit code.**
- Version in `Cargo.toml`, `repository` field added, commit, **signed tag**
  `v0.1.0`.
- Publish the repository. The deployment (its own repository, its own plan)
  then pulls this tag as a flake input.

---

## Waiting for a decision

Nothing below is blocked on work; it is blocked on you.

1. **Where the repository goes, and when.** Task 16 ends with making it public
   and pushing. That needs an account and a name — `github.com/<you>/treff`,
   or somewhere else entirely — and it is the one step that cannot be undone
   quietly. Until then `Cargo.toml` keeps no `repository` field, which is
   itself a small lie by omission on crates.io if it were ever published.
2. **Whether v0.1.0 is cut now or after stage 2.** The design ties inviting
   friends to stage 2 (search and notifications), so a v0.1.0 today is a
   release nobody outside the household would be invited to use. Both are
   defensible: a tag is a marker, not a promise.
3. **The five forum categories and their groups.** `films`, `series`,
   `offtopic`, `wishes`, `server-services` are in the design as slugs; the
   groups that may post and reply in each are a decision about people, not
   about code. The deployment repository needs them either way.

## Coverage against the design

| design.md | Task |
|---|---|
| §1, §5 — scope, two views, what is left out | 5, 9, 10, 11, 13 |
| §2, ADR 0001 — the stack | 1 |
| §3, ADR 0002 — OIDC, groups from the claim, secret from a file, fail closed | 7, 15 |
| §4 — spaces, categories, rights from configuration, host→space | 2, 3, 8 |
| §6 — attachments, magic bytes, fixed content type, nosniff, CSP | 8, 12 |
| §7 — `treff export` for a consistent backup | 4, 14 |

**Not in this plan, on purpose:** the deployment (container, zone, vHosts,
identity-provider objects, the checks that hold it together) — that is a
separate repository with its own test cycle. And stage 2 (search,
subscriptions, notifications), which gets its own plan after stage 1 is
accepted.

## Open points carried into the work

1. **The plan this file is derived from was written in German and used German
   identifiers in a few places** (`erkennen`, `fuer_tests`,
   `aus_accept_language`, test files `lesen.rs`/`schreiben.rs`/`aendern.rs`),
   which contradicts the repository's own rule that code is English. They are
   translated here (`detect`, `for_tests`, `from_accept_language`,
   `reading.rs`/`writing.rs`/`editing.rs`). If a name reads badly in English,
   change it here rather than reintroducing German.
2. **Tasks 9–11 and 13 carry assertions rather than finished test code**,
   deliberately: their shape is decided while writing the interface, and
   pre-written test code would fix names that will fall anyway. The
   assertions themselves are complete — write exactly those tests, and see
   them red.
