# Changelog

## 0.1.3 — 2026-09-06

- An **empty** client secret file is refused at startup, like a missing one.
  The interesting case is not a wrong path but a credential that arrived
  carrying nothing — a template rendered from a value nobody filled in.
  Before this, treff started, looked healthy, and failed only when somebody
  tried to sign in. Found while wiring it into a host where the secret does
  not exist yet.

## 0.1.2 — 2026-09-06

- `services.treff.oidc.clientSecretFile` is a **string**, not a path, so a
  systemd specifier works: `"%d/oidc"` together with
  `LoadCredential = [ "oidc:/path/to/secret" ]`. A `path` rejected that and
  pushed towards writing the secret somewhere the service can read on its own,
  which is the opposite of what the option is for. Found while wiring treff
  into a host where credentials are the only way in.
- The VM test uses that form now, and asserts the effect rather than the
  string: systemd expands `%d` before `systemctl show` sees it, and the real
  proof is that the service is up — it refuses to start when the secret file
  is missing.

## 0.1.1 — 2026-09-06

Housekeeping, an hour after 0.1.0. Nothing about the program changed; the tag
exists because a tag is not moved once it is pushed.

- The formatting that 0.1.0 was missing. The commit behind that tag went out
  with `nix flake check` green and `cargo fmt --check` never run, so
  `cargo fmt --check` fails on v0.1.0.
- The `fmt` CI job now runs through `nix develop`, so it checks with the same
  tools a local check uses. On v0.1.0 that job is red.

## 0.1.0 — 2026-09-06

The first working shape of stage 1: everything below is built and covered by
tests, and none of it has run in front of real people yet. Search and
notifications are the next stage, and the design ties inviting anyone outside
the household to that stage.

### Added

- **Spaces.** One process serves several addresses; the space is chosen by the
  `Host` header, and an unknown one is refused rather than mapped to the first
  space.
- **Categories carry the rights.** `read` per space, `post` and `reply` per
  category, matched against the groups in the token. An empty list grants
  nobody.
- **Two views.** A timeline (a blog) and a topic list (a forum) — the same data
  one line of configuration apart. A forum's front page lists its categories
  with their topic count and last activity.
- **Sign-in over OIDC**, with PKCE, `state`, `nonce` and a JWKS-verified ID
  token. The client secret comes from a file. Sessions live in the database, so
  a restart signs nobody out.
- **Writing**, with the limits checked where they belong: 200 characters of
  title, 64 KiB of body, whitespace counting as empty.
- **Editing and deleting your own, and nobody else's** — enforced in the query
  rather than in the handler. Deleting the opening post takes the topic with
  it, but never while someone else has replied.
- **Markdown, rendered and sanitized.** No raw HTML, no `javascript:`, no
  `data:` URL, and no image from another host: a picture is fetched without
  anyone deciding to, and that would tell its host who is reading.
- **Attachments** — JPEG, PNG, GIF, WebP, recognised by their magic bytes and
  never by the name or the announced type. Stored under a generated name,
  served with the detected type and `nosniff`.
- **Articles mirrored from a directory**, keyed by file name, one-way: the file
  decides the title and the text, the database keeps the comments. A file dated
  in the future is a draft.
- **`treff export`**, a self-contained copy via `VACUUM INTO`, for the
  maintenance window to call before a filesystem snapshot.
- **German and English**, with a test that keeps the two catalogues equal.
- **A NixOS module and a VM test** that proves, among other things, that the
  client secret never reaches the unit.

### Security properties worth naming

- Fail closed everywhere: no configuration, no start; unknown host, 403;
  unknown category, 404; missing group, 403.
- A refusal writes nothing — asserted against the database, not only against
  the status code.
- CSP without `unsafe-inline`, `script-src 'none'`, `img-src 'self'`, and the
  headers are set outermost so a refusal carries them too.
- The session cookie is private (signed and encrypted), `HttpOnly`, `Secure`,
  `SameSite=Lax`.
