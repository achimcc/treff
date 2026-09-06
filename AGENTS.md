# Working rules for this repository

Read `docs/design.md` before changing behaviour and `docs/plan-stage-1.md`
before starting a task. Decisions that were argued once live in
`docs/decisions/`; if you want to overturn one, change the ADR — not just the
code.

## Language

**This repository is English** — identifiers, comments, commit messages,
README, error messages, and the configuration file. That is a deliberate
exception to its author's usual habit of writing in German: the project is
meant to be taken over by someone else, and a project whose identifiers and
README are German is closed to most of the people who could use or extend it.

The **user interface** is bilingual (German and English, task 13). User-facing
strings live in `i18n/*.toml`, never in the templates.

## The test cycle

Every change goes through it, in this order:

1. **Write the test first.**
2. **See it red** — `cargo test <name>` must fail with the message you expect.
   A test that was never red has proved nothing.
3. **Implement the smallest thing that passes.**
4. **See it green** — `cargo test`, plus
   `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`.
5. **Commit**, staging the files by name.

Before finishing a task, also run `nix flake check`.

**Run all four, every time, and read their output.** On 2026-09-06 a commit
went out with `nix flake check` green and `cargo fmt --check` never run — the
flake check does not format-check, so nothing caught it until CI did, on the
first push after the repository went public. Skipping a step because the
expensive one passed is how the cheap one stops being a step.

**Ask for the result, not the exit code.** `cargo test` with a filter that
matches nothing exits 0 and reports `0 passed`. Read the number. And beware of
pipes: `cargo fmt --check | head` reports `head`'s exit code, not the check's —
the same trap as `nix build … | tail`.

## Rules that hold everywhere

- **Fail closed.** Missing OIDC configuration → the program does not start.
  Unknown `Host` → 403. Unknown category → 404. Missing group → 403. Never a
  fallback to "then without the check".
- **An empty group list grants nothing**, and never everything.
- **No runtime dependency on the network except the issuer.** No CDN, no
  remote font, no third-party script.
- **CSP without `unsafe-inline`.** No inline script, no inline style
  attribute; the stylesheet is its own route.
- **Everything from a token, header, form or file is unchecked** until a
  function has checked it. Markdown and uploaded bytes especially.
- **No `unwrap()` / `expect()` in a request path.** In tests and at startup
  both are fine.
- **`sqlx::query`, not `sqlx::query!`.** The macro checks against a database
  *at compile time* and needs either `DATABASE_URL` or a checked-in `.sqlx`
  directory; both are foreign bodies in a Nix build. The tests do that job.
- **SQLite with `journal_mode=WAL` and `foreign_keys=ON`**, set on every
  connection — and verified on a pooled connection, not on the one that ran
  the migration.
- **Versions are not guessed.** `cargo add <crate>` decides them, `Cargo.lock`
  holds them, the Nix package reads `cargoLock.lockFile`. Where a document
  names another library's API, check it against `cargo doc` before using it.
- **Stage files by name.** Never `git add -A`.
- Commits are **signed**.

## Layout

| Path | Responsibility |
|---|---|
| `src/main.rs` | startup: read configuration, build the router, listen |
| `src/config.rs` | `Config`, `Space`, `Category` — parsing and checking the TOML |
| `src/authz.rs` | permissions as pure functions — no HTTP, no database |
| `src/auth/` | identity, session, cookie; OIDC discovery, callback, sign-out |
| `src/db/` | pool, schema, topics, posts, attachments |
| `src/markup.rs` | Markdown → sanitized HTML |
| `src/media.rs` | magic-byte detection for uploads |
| `src/web/` | router, extractors, security headers, views |
| `src/i18n.rs`, `i18n/` | language selection and catalogues |
| `nix/module.nix`, `nix/test.nix` | the NixOS module and its VM test |
| `docs/` | design, plan, decisions |

Two places are security-critical and are tested as such — the sign-in flow
(`src/auth/`) and the attachment path (`src/media.rs`). Their tests describe
the attack, not the function.
