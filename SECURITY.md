# Security

## Reporting a vulnerability

Please report a suspected vulnerability privately, through GitHub's
**"Report a vulnerability"** button on the *Security* tab of this repository
(private vulnerability reporting). Do not open a public issue for it.

Say what you observed, how to reproduce it, and which version or commit you
looked at. You will get an answer within a week.

## Supported versions

Only the latest tagged release is supported. A fix lands on `main` and in a
new tag; older tags are not patched.

## What is checked on every commit

`nix flake check` runs, besides the tests and the NixOS VM test:

- **`audit`** — `cargo-audit` against the RustSec advisory database, pinned as
  a flake input and read offline. Renovate refreshes the pin weekly. One
  advisory is ignored by name, RUSTSEC-2023-0071 (`rsa`, through
  `openidconnect`); the reason stands next to the check in `flake.nix`.
- **`deny`** — `cargo-deny` on bans, sources and licenses (`deny.toml`): no
  dependency from an unknown registry or git repository, no wildcard version.

Clippy and rustfmt run in CI through the dev shell (`.github/workflows/fmt.yml`).

`unsafe` code is forbidden crate-wide (`[lints.rust]` in `Cargo.toml`).
