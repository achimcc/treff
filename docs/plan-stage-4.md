# Stage 4 — completing `@handle`

Three tasks, test first, the same cycle as every stage (`../CLAUDE.md`). The
decision to ship a script at all, and its limits, are in
`decisions/0005-one-script-for-mentions.md`; read it first.

**What it does, in the operator's words (2026-09-22):** *when somebody types
`@`, show the list of everybody as an overlay to choose from; when they go on
typing after the `@`, complete it.*

## Status

| | |
|---|---|
| Done | **Task 1** — `GET /mentionable`: who may be offered |
| Done | **Task 2** — the script's route, the CSP, and the tag on the page |
| Open | **Task 3** — `mention.js`: the overlay |

---

## Task 1 · `GET /mentionable`: who may be offered

**Files:** `src/db/accounts.rs`, `src/mentions.rs`, `src/web/mod.rs`,
`tests/mentions.rs`

- [x] `db::accounts::with_handles(db) -> Vec<Known>` plus the name; filtered
      in `mentions::offered(db, space)` by the same `readers` rule as
      `to_tell` and `highlighted` — one rule, three uses.
- [x] The route answers JSON `[{"name": …, "handle": …}]`, sorted by name
      (case-insensitive), nothing else in it: no subject, no address.
      `Cache-Control: no-store` — it is per person and per space.
- [x] Behind the session gate like every page, and `may_read` on the space,
      or 403.
- [x] Tests: a reader is listed; somebody without the groups is not; an
      account without a handle is not; the answer carries no subject and no
      address; a person who may not read the space gets 403. (Not tested: a
      second space with other groups — the test configuration gives both
      spaces the same readers, and the rule is `may_read`, tested in `authz`.)

## Task 2 · The script's route, the CSP, and the tag on the page

**Files:** `src/web/mod.rs`, `src/web/views.rs`, `src/web/mention.js` (new,
empty at first), `tests/frame.rs`, and the two tests that had the old rule written into
them (`tests/reading.rs`, `tests/articles.rs`): they now allow exactly the
one tag the layout writes

- [x] `/assets/mention.js`, served like `/assets/style.css` (same caching and
      `ETag` scheme), `Content-Type: text/javascript; charset=utf-8`.
- [x] CSP: `script-src 'self'`, nothing more. Test that `'unsafe-inline'`
      and every foreign origin stay absent.
- [x] `<script src="/assets/mention.js" defer>` in the signed-in layout only
      (not on the bare unsubscribe pages). Test: no inline `<script>` with a
      body, and no `on…=` attribute on any page.
- [x] Every comment that says "no script on any page" says what is true now.

## Task 3 · `mention.js`: the overlay

**Files:** `src/web/mention.js`, `src/web/style.css`, `tests/preview.rs`,
`tests/js/mention.html` (new)

- [ ] On every `textarea` in a form: an `@` at a word boundary (the rule of
      `markup::mention_spans`) opens an overlay under the textarea with the
      whole list, loaded from `/mentionable` once per page on the first `@`.
- [ ] Typing after the `@` filters: prefix of handle or of any word of the
      name, case-insensitive. No match → the overlay closes.
- [ ] Arrow keys move, Enter or Tab take, Escape or a space closes; a click
      or tap takes. Taking writes `@handle ` at the caret, replacing what was
      typed after the `@`.
- [ ] Accessible: `role="listbox"`, `aria-activedescendant`, the textarea
      gets `aria-expanded`/`aria-controls` while it is open.
- [ ] The fetch failing, or the list being empty, leaves the textarea alone.
- [ ] **Measured in a browser**, since `cargo test` cannot run the script:
      `tests/js/mention.html` loads the real file with a stubbed `fetch`,
      drives it with input and key events, and writes the outcome into the
      page; headless Chrome's `--dump-dom` is read back. Plus screenshots of
      the overlay on the preview topic page, wide and at 390px.

## After the stage

- [ ] Preview opened, `nix flake check`, version `0.6.0`, CHANGELOG, README.
- [ ] Deployment as for 0.5.0, newsletter entry.
