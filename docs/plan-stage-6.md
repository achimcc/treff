# Stage 6 — knowing everybody: SCIM from the identity provider

The operator's design is in the homeserver repository,
`docs/2026-09-22-treff-scim-design.md` (German); what it means for treff is in
`decisions/0007-scim-in.md`. Same cycle as every stage.

**Why:** treff learned a person's handle and groups only at a sign-in. On the
evening mentions went live, the `@` list offered one person — the one writing.
Everybody else had either never opened the forum or still held a session from
before. A mention is for somebody who is NOT looking, so treff has to know
people before they come.

**What Authentik sends** was read in its source (2026.5.6), not assumed: users
by `POST`/`PUT`/`DELETE` on `/Users`; groups by `POST` on `/Groups` and then,
always, their members by `PATCH` (add on `members`, remove on `members` with a
value, remove on `members[value eq "<id>"]`); a `GET /Groups/<id>` to compare;
`/ServiceProviderConfig` to learn what treff can do. `externalId` defaults to a
hash, so the operator's mapping sets it to the user's UUID — which is what
treff sees as `sub`.

## Status

| | |
|---|---|
| Open | **Task 1** — the `@` list leaves out whoever is writing |
| Open | **Task 2** — the directory: accounts and groups from SCIM, in the database |
| Open | **Task 3** — `/scim/v2`: the routes Authentik calls, on the internal listener |
| Open | **Task 4** — module option, VM test, ADR 0007 |

---

## Task 1 · The `@` list leaves out whoever is writing

- [ ] `mentions::offered(db, space, except)`; `GET /mentionable` passes the
      viewer. Mentioning yourself does nothing, so offering yourself is noise.

## Task 2 · The directory

**Files:** `migrations/0013_directory.sql`, `src/db/directory.rs` (new)

- [ ] `scim_groups (id, name UNIQUE)`, `scim_members (group_id, subject)`;
      `accounts` gains `scim_user_name` (as sent, for the filter) and `active`.
- [ ] `put_user(subject, user_name, display_name, email, active)` — the same
      row a sign-in writes: handle through `auth::checked_handle`, taken only if
      nobody else holds it; inactive clears handle and address and leaves the
      row (posts keep their author).
- [ ] `delete_user`: like inactive, and out of every group.
- [ ] Groups: `put_group(id, name, members)`, `add_members`, `remove_members`,
      `delete_group`; every change recomputes `groups_json` of the people it
      touched from the memberships — so `may_read` follows the directory.
- [ ] Every change announces `EVERY_SPACE` (a person's bell may change).
- [ ] Unit tests: a user from SCIM is mentionable once in a readable group; a
      taken handle is not given twice; inactive and deleted strip the row and
      keep it; group changes move `groups_json`; a later sign-in writes the same
      row, no second one.

## Task 3 · `/scim/v2`

**Files:** `src/web/scim.rs` (new), `src/web/internal.rs`, `src/main.rs`,
`tests/scim.rs` (new)

- [ ] On the internal listener, its own token (`TREFF_SCIM_TOKEN_FILE`); absent
      → the routes do not exist.
- [ ] `GET /scim/v2/ServiceProviderConfig`: patch false, bulk false
      (`maxOperations` 0), filter true, no sort, no etag, bearer auth.
- [ ] `/Users`: `POST` (upsert, 201), `GET` (list with `startIndex`/`count`,
      `filter=userName eq "…"`), `GET /{id}`, `PUT /{id}`, `DELETE /{id}` (204).
      `id` = `externalId`, a UUID, or 400.
- [ ] `/Groups`: `POST` (upsert by name, 201), `GET` (list, `filter=displayName
      eq "…"`), `GET /{id}` with members, `PUT /{id}`, `PATCH /{id}` (the three
      member forms only; anything else 400), `DELETE /{id}` (204).
- [ ] `application/scim+json`; errors in the SCIM error shape.
- [ ] Route tests: the whole sequence Authentik runs, bodies as its client
      builds them; wrong token 401; a non-UUID id 400 and no row; a `PATCH` of
      another kind 400 and no change; the result: a person from SCIM is in the
      `@` list of a space their group may read, and nowhere else.

## Task 4 · Module option, VM test, ADR 0007

- [ ] `services.treff.internal.scimTokenFile`, and the listener assertion
      covers it.
- [ ] The VM test creates a user and a group over SCIM and finds the handle
      in `/mentionable`... through the internal side only (the VM has no
      provider to sign in with): it checks the rows with `treff export`.
- [ ] ADR 0007: why SCIM and not treff asking the provider; what the door
      accepts; that a deleted person keeps their posts.

## After the stage

- [ ] `nix flake check`, version `0.8.0`, CHANGELOG, README.
