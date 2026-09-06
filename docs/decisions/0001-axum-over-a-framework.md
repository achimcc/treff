# ADR 0001 — Axum directly, not a batteries-included framework

**Date:** 2026-09-06 (first taken 2026-09-02, re-measured on 2026-09-06)
**Status:** accepted, with a review trigger
**Decision:** treff is built on Axum, Maud, SQLx/SQLite and a handful of
single-purpose crates, not on a web framework that assembles them for us.

## Context

Rust has one serious candidate for the batteries-included, convention-over-
configuration approach in this space: [Autumn](https://github.com/autumn-foundation/autumn)
("Ship the App, not the Plumbing. Built on Axum."). It is built on exactly the
stack treff would otherwise assemble by hand, and it would hand us several
pieces of treff stage 1 already written. The question deserved measuring
rather than a shrug, so it was measured — twice, because the first answer
rested on the wrong numbers.

## What was measured (2026-09-06)

| | |
|---|---|
| Activity | created 2026-03-21, pushed the same day this was written; **8 stars, 1 fork, 0 watchers**; ~192 open issues and ~92 open PRs |
| People | **one human** (1230 commits) plus an AI contributor and bots; `AGENTS.md`, `CLAUDE.md` and an agent harness in the repo root |
| Releases | v0.1.0 (March) … **v0.7.0 (August)** — seven minor releases in five months |
| Stability | `STABILITY.md`: the guarantees "will become binding at the 1.0 release"; every `0.x` bump is a breaking change as far as Cargo is concerned, and that is stated as intentional |
| Licence | **no LICENSE file anywhere in the repo** (GitHub reports none), but `MIT OR Apache-2.0` is declared in `[workspace.package]` and on the published crate |
| Availability | published to crates.io as **`autumn-web` 0.7.0** — a plain dependency, no git pin needed |
| Weight | **50 non-optional direct dependencies** plus 45 optional ones; Diesel as the data layer; `db` among the default features, with a bundled `libpq` |

### The deciding finding: SQLite

treff is a single-file SQLite application by design (§7). Autumn supports
SQLite as an opt-in tier — Postgres is the default — and that tier is a
**rollout in progress**. From its own support matrix
(`docs/guide/sqlite-in-production.md`), checked against the issue tracker:

| On SQLite | State |
|---|---|
| Models, CRUD, repositories | available |
| Full-text search (FTS5) | available |
| **DB-backed sessions and auth** | **open — issue #1908** |
| Durable jobs, scheduler | open — #1907 |
| `db backup` / `restore` | open — #1909 |

The configured session backends today are `"memory"` and `"redis"`. Memory
signs everyone out on every restart; Redis is a second service to run. So the
exact intersection treff stands on — **SQLite plus its own session behind
OIDC** — is the one that is not finished. The way around it is Postgres, which
is the same argument that disqualified Discourse: a database server, a
migration on every start, and a restore as the way back.

### What we are giving up, stated honestly

`autumn/src/auth.rs` contains a finished OIDC client, and it is good: provider
configuration is generic (`discovery_url`, `issuer`, `jwks_url` — so it points
at any provider, not only the three presets), PKCE S256, `state` compared with
`subtle::ConstantTimeEq`, an OIDC `nonce` checked on the callback, the ID token
verified against JWKS, and `raw_claims` exposed so a `groups` claim can be read
straight out. That is design §3, and it is the most safety-critical part of
stage 1.

We are writing that ourselves. The reason is that it does not come by itself:
`oauth2` is a feature of the whole framework, so taking it means taking Diesel,
`autumn.toml`, the `AUTUMN_*` configuration surface, a Tailwind binary
downloaded by `autumn setup` and invoked from `build.rs`, and a deployment
story built around Docker and a proxy we do not use. We would use perhaps a
sixth of the framework and carry all of its breaking changes.

## Decision

Use Axum directly. Assemble the stack from crates that do one thing:
`axum`, `maud`, `sqlx`, `openidconnect`, `comrak`, `ammonia`,
`axum-extra` (private cookie jar), `tower-http`.

Accept that we write the OIDC flow and the session handling ourselves, and
treat that code as security-critical: it gets tests that describe the attack,
not the function.

## Review trigger

This is not "never". Re-evaluate Autumn when **both** hold:

1. issue **#1908** (DB-backed sessions on SQLite) is closed, and
2. **1.0** is released, so `STABILITY.md` is binding.

Both are checkable in a minute. Neither was true on 2026-09-06. Nothing about
star counts belongs in that decision.

## Consequences

- More of treff's own code is security-relevant, in exactly two places: the
  sign-in flow and the attachment path. Both are called out in the design.
- Dependency updates stay small and independent; there is no framework release
  that moves everything at once.
- Nix packaging stays a plain `buildRustPackage` with a `Cargo.lock` and no
  build-time downloads.
