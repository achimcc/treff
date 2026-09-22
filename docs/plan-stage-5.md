# Stage 5 — events from elsewhere, and the bell outside treff

The operator's design is in the homeserver repository,
`docs/2026-09-22-glocke-fuer-alles-design.md` (German); what it means for
treff is in `decisions/0006-an-internal-door.md`. Same cycle as every stage.

**In one sentence:** a second, internal listener takes events for a person
from another service (the first: a film request that became available or
failed) and answers one question for another page — how many unread entries
does this person have?

## Status

| | |
|---|---|
| Done | **Task 1** — events: table, intake rules, entries in the bell |
| Open | **Task 2** — the internal listener: `POST /internal/events`, `GET /internal/bell` |
| Open | **Task 3** — live: a change signal, two streams, `bell.js` |
| Open | **Task 4** — the NixOS module, the VM test, ADR 0006 |

---

## Task 1 · Events: table, intake rules, entries in the bell

**Files:** `migrations/0012_events.sql`, `src/config.rs`, `src/db/events.rs`
(new), `src/db/inbox.rs`, `src/web/mod.rs`, `src/web/views.rs`, `i18n/*.toml`,
`tests/events.rs` (new)

- [x] Configuration: an optional `[events]` section, `space = "<host>"` (must
      be a configured space, or treff does not start) and `link_hosts = [..]`
      (the only hosts an event may link to).
- [x] Table `events`, keyed by **handle**, not subject: the person may never
      have signed in to treff; the entries are there when they do. `kind` is
      `film_available` or `film_failed`; unique on `(source_key, kind)`, so a
      webhook that arrives twice is one entry.
- [x] `db::events::Event::checked(json)` — every field checked, or the reason
      why not: handle by `auth::checked_handle`, kind from the list, title
      1–200 characters, reason up to 300, `link` only `https://` on a
      `link_hosts` host, `source_key` `[A-Za-z0-9:_-]{1,64}`.
- [x] The bell counts unread events of the viewer's handle in the events
      space; `/notifications` lists them one by one ("🎬 *Dune* is here" /
      "*Dune* could not be got — *reason*"); `GET /notifications/e/{id}` marks
      it read and redirects to its link (the stored, checked link — never one
      from the request). "Mark all as read" includes them. No mail, ever.
- [x] Tests (units and routes): each refusal of `checked`; one entry for a
      doubled event; an event before the first sign-in shows after it; the
      count; the redirect marks read; someone else's event id is 404.

## Task 2 · The internal listener

**Files:** `src/web/internal.rs` (new), `src/main.rs`, `tests/internal.rs`
(new)

- [ ] `TREFF_INTERNAL_LISTEN`, `TREFF_EVENTS_TOKEN_FILE`,
      `TREFF_BELL_TOKEN_FILE`. No listen address → no listener. A route whose
      token file is not configured does not exist (404) — never "open because
      nothing was set". An unreadable or empty token file stops treff at
      startup.
- [ ] Its own router: no sessions, no `Host` routing, no public page. Each
      route asks `Authorization: Bearer <its token>`, compared in constant
      time; the events token does not open the bell and the other way round.
- [ ] `POST /internal/events` → 201 (new), 200 (already there), 400 with a
      reason (and no row), 401 (token).
- [ ] `GET /internal/bell` reads `X-Treff-User` (a handle) and
      `X-Treff-Groups` (Authentik's `|`-separated list). If those groups may
      read the events space: the unread count of that handle — replies,
      mentions and events, the same number the bell in treff shows — and the
      entries (at most 20, links absolute to the space), as
      `{"unread": n, "entries": [...]}`; otherwise `{"unread": 0, "entries":
      []}`. `Cache-Control: no-store`. Read-only: nothing is marked read here.
- [ ] Tests: wrong token and swapped tokens are 401, an absent token file is
      404, bad input leaves no row, the bell's number equals treff's own for
      the same person, foreign groups are 0, a header that is not a handle is
      0, and the public router does not answer `/internal/*`.

## Task 3 · Live: a change signal, two streams, `bell.js`

**Files:** `src/live.rs` (new), `src/db/*` call sites, `src/web/mod.rs`,
`src/web/internal.rs`, `src/web/bell.js` (new), `tests/live.rs` (new),
`tests/js/`

- [ ] A process-wide change signal (`tokio::sync::broadcast`): after the
      COMMIT of anything that changes somebody's unread — a reply, a topic,
      an edit with a mention, an event, a read — the space is announced.
      Announced after the commit, never inside the transaction: a stream that
      asks before the commit reads the old state.
- [ ] `GET /notifications/stream` (session, public listener) and
      `GET /internal/bell/stream` (token B, internal): Server-Sent Events. One
      `bell` event on connect, one after every change in the space whose
      payload differs from the last one sent, a comment every 25 s. A lagging
      receiver re-reads instead of failing.
- [ ] `bell.js` in treff keeps the number in the header current; without it
      the number is what it was when the page loaded. ADR 0005 is amended
      (two scripts, both from `'self'`, nothing depends on either).
- [ ] Tests: a stream sees a reply, a mention, an event and a read at once;
      a stream for somebody else sees nothing of it; the internal stream
      needs its token. `tests/js/` drives `bell.js` against a stubbed
      `EventSource`.

## Task 4 · The NixOS module, the VM test, ADR 0006

**Files:** `nix/module.nix`, `nix/test.nix`, `docs/decisions/0006-an-internal-door.md`,
`README.md`

- [ ] `services.treff.internal.{listen,eventsTokenFile,bellTokenFile}` and
      `services.treff.events.{space,linkHosts}`, into the environment and the
      TOML. An assertion: a token file without `internal.listen` is a
      mistake.
- [ ] The VM test starts treff with the internal listener and asks it once
      for the bell with and without the token.
- [ ] ADR 0006: the second door, and why it is narrow.

## After the stage

- [ ] Preview, `nix flake check`, version `0.7.0`, CHANGELOG, README.
