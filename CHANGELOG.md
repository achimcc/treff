# Changelog

## Unreleased

The first working shape of stage 1. Not tagged yet — see
`docs/plan-stage-1.md`, "Waiting for a decision".

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
