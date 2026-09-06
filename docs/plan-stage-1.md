# Stage 1 — the implementation plan

Sixteen tasks, worked one at a time, test first. Each task ends in something
that runs and can be checked on its own. `design.md` says *what* and *why*;
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
| Next | **Task 6** — rendering and sanitizing Markdown |
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

## Task 6 · Rendering and sanitizing Markdown

**Files:** add `src/markup.rs`; `mod markup;`
**Produces:** `render(markdown: &str) -> String`, returning **sanitized** HTML.

The second security-relevant place after attachments, so the tests describe
the attack, not the function: `<script>`, `javascript:` and `data:` URLs, an
`onerror=` attribute, a raw HTML block, a nested/obfuscated form of each — all
must come out inert. Ordinary Markdown (emphasis, lists, code, links,
autolinks) must survive.

## Task 7 · Sign-in: OIDC, session, sign-out

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

## Task 8 · The web frame: host→space, sign-in gate, security headers

**Files:** add `src/web/mod.rs`; `src/main.rs` starts the server
**Consumes:** `Config`, `Sessions`, `Db`.
**Produces:** `AppState { config, db, oidc, cookie_key }`,
`router(state) -> axum::Router`, extractors `CurrentSpace` and `CurrentUser`,
plus `AppState::for_tests(...)` under `#[cfg(any(test, feature = "testing"))]`
— a fixed cookie key and OIDC settings pointing nowhere. Without that
constructor every integration test would need a reachable identity provider,
and the tests would be a network dependency instead of tests.

**Assertions to write first:** an unknown `Host` is **403** (not a redirect to
the first space); a request without a session is redirected to sign-in, and
the redirect target is not attacker-controlled; every response carries the CSP
without `unsafe-inline`, `X-Content-Type-Options: nosniff`, a `Referrer-Policy`
and a `frame-ancestors` denial; the session cookie is `HttpOnly`, `Secure`,
`SameSite=Lax`.

## Task 9 · Reading: timeline and topic list

**Files:** add `src/web/views.rs`; change `src/web/mod.rs`
**Produces:** `views::layout`, `views::space_page`, `views::topic_page`;
routes `GET /`, `GET /c/:category`, `GET /t/:id`.

**Assertions to write first:** with a session, a member of the reading group
sees the rendered **bodies** on a `timeline` space and a **list of titles** on
a `topics` space; someone without the reading group gets **403**; a topic from
the blog is **404** through the forum's address; the page contains **no
`<script`** element at all.

## Task 10 · Writing: open a topic, reply

**Files:** change `src/web/mod.rs`, `src/web/views.rs`
**Produces:** routes `POST /c/:category/new`, `POST /t/:id/reply`.

**Assertions to write first:** a member of the posting group opens a topic
(302 to it, and it is in the database); a member without that group gets
**403 and the database is unchanged** — that second half is the actual test;
replying follows `reply`, not `post`; an empty title or body is **400**; a
title over 200 characters is **400**; a body over 64 KiB is **400**; the form
page shows **no submit button** to someone who may not use it (display follows
the right).

## Task 11 · Editing and deleting — only your own

**Files:** change `src/web/mod.rs`, `src/db/topics.rs`, `src/web/views.rs`
**Produces:** `update_post(&Db, post_id, body, &Identity) -> Result<bool>`,
`delete_post(&Db, post_id, &Identity) -> Result<bool>`;
routes `POST /p/:id/edit`, `POST /p/:id/delete`.

**Assertions to write first:** the author may edit, and `edited` is set
afterwards; anyone else gets **403** and the text is unchanged; deleting the
first post of a topic is refused or takes the topic with it (decide once,
test it); the permission is enforced in the **query**, not only in the
handler.

## Task 12 · Attachments: magic bytes, storage, serving

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

## Task 13 · Languages: German and English

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

## Task 14 · `treff export` — a backup that survives the snapshot

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

## Task 15 · The NixOS module and the VM test

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

## Task 16 · Release v0.1.0

- `README.md`: a complete `spaces.toml` example, the environment variables, a
  **"Behind a reverse proxy"** section (the `Host` header must be passed
  through, or every request lands in the 403 branch), and a "Configuring your
  provider" section with **Authentik and Keycloak** as worked examples.
- `CHANGELOG.md` with `0.1.0` and the scope from `design.md` §5.
- Everything together: `cargo fmt --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo test`, `nix flake check`. **Read the number of passing
  tests, not the exit code.**
- Version in `Cargo.toml`, `repository` field added, commit, **signed tag**
  `v0.1.0`.
- Publish the repository. The deployment (its own repository, its own plan)
  then pulls this tag as a flake input.

---

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
