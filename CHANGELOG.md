# Changelog

## 0.3.0 — 2026-09-07

**Stage 2, the part that makes inviting people make sense.** A forum without
notifications gets read exactly once by someone who does not live with it.

### Added

- **Writing subscribes you.** Opening a topic or replying to one is the
  clearest statement that you want to know what happens next; asking
  afterwards is a dialogue nobody wants. Undoable from a button on the topic
  page, and **never a mail about your own post** — a forum that mails you your
  own words teaches people to filter it away, and then it teaches them nothing
  else.
- **Mail, through an outbox in the database**, written in the same transaction
  as the post and drained by a background task. A reply must not be lost
  because SMTP is down, and a request must not wait on the network. Failures
  back off and, after eight attempts, stop — the row stays in the table with
  its last error, because a row that vanishes takes the reason with it.
- **One-click unsubscribe that does not ask anybody to sign in.** Behind a
  sign-in it is not an unsubscribe link, it is a sign-in link, and the person
  reaches for the spam button instead — which costs the whole domain rather
  than one subscription. `List-Unsubscribe` and `List-Unsubscribe-Post` so a
  mail client can offer the button itself. It cancels **one** subscription,
  and a tampered token changes nothing and says nothing.
- **An `accounts` table.** The address arrives with a sign-in, and a session
  expires after twelve hours: keeping it there would have meant notifying
  whoever happens to be logged in. Refreshed on every sign-in, so a changed
  address is right again the next time somebody comes by.
- **Full-text search over FTS5**, scoped to one space and to the categories
  that person may read. That is a security property, not a convenience: a hit
  list leaking a title from the other audience looks like a feature until
  somebody notices. `porter unicode61 remove_diacritics 2`, so `wunsche`
  finds `Wünsche`. The index is maintained by triggers — the application
  forgets, a trigger does not.
- `TREFF_SMTP_*` and `services.treff.mail`. Without a host nobody is
  notified and treff runs anyway: mail is optional, not half-built. A password
  without STARTTLS is refused at startup and at build time.

### Two things worth writing down

- **`content='posts'` reads the original text by COLUMN NAME.** An FTS5 column
  called `body` over a table whose column is `body_markdown` creates fine,
  indexes fine and matches fine — and then answers `no such column: T.body`
  for every query that touches the content, `snippet()` and `count(*)`
  included.
- **`src = self` is the git tree here too.** `notify/` was untracked, so the
  Nix build could not see it and `nix flake check` failed with
  `file not found for module`, while `cargo test` was green.

## 0.2.3 — 2026-09-06

- **`home` per space**: the way back to wherever people came from, shown in
  the header and labelled with its host name rather than an arrow. A space is
  one address among several, and the back button is memory, not navigation —
  it is empty for anyone who arrived by bookmark.
- **The stylesheet carries an `ETag` and `Cache-Control: no-cache`.** Without
  a validator a browser may reuse it for as long as it likes, so a redesign
  that is deployed, running and correctly served can still be invisible to the
  person looking at the page — everything measurable says yes and the screen
  says no. `no-cache` means "keep it, but ask first": the usual answer is a
  304 and no bytes.

## 0.2.2 — 2026-09-06

Looks. The surface says what the software is: something you run yourself, on a
machine you can point at, for people you know by name.

- **A terminal, and dark only.** Near-black ground, one green that carries
  headings, links and the cursor, one red that appears only where something
  cannot be taken back. Two font stacks: monospace sets the tone, the prose
  people came to read stays sans-serif. A light variant would be a second
  design, not a lighter one, and two designs drift apart the moment one is
  edited.
- **A prompt line** naming who you are and where you are — shell notation,
  which needs no translation — and a cursor that blinks in CSS, because there
  is no script on any page here and this was not worth becoming the first
  exception. It honours `prefers-reduced-motion`.
- Section headings carry `[ brackets ]` from the stylesheet rather than from
  the templates: the text inside them is translated, the brackets are not.
- **Dates in the bylines.** A timeline is ordered by date and, for mirrored
  articles, that date even decides whether an entry appears at all — leaving
  it out hid the only visible reason for the order.
- **`cargo test --test preview -- --ignored`** writes the real pages to
  `target/preview/`. It found four faults in its first pass. A stylesheet is
  the one part of a program whose failures are invisible to `cargo test`.

## 0.2.1 — 2026-09-06

- **The groups never left the token.** `claims.additional_claims()` on a
  `CoreClient` is `EmptyAdditionalClaims` — it returns `{}` whatever the
  provider sent, by construction. So every sign-in succeeded and every page
  then refused: `groups: []` in the session, *not for you* on the screen, with
  a correctly configured provider and a passing unit test for
  `claims_to_identity`, which had been handed a hand-written JSON value rather
  than what this path actually produces. The claims are now read back out of
  the ID token that was verified one line above — the same bytes, after
  signature, issuer, audience and nonce were checked, and the comment says so
  because the type system cannot.
- The **third** bug of the same family in one day: a function that is correct
  and a path that never reaches it. Tested at the seam this time, not on
  either side of it.

## 0.2.0 — 2026-09-06

**Nobody could sign in.** Everything below is one bug, found the first time a
real person clicked a link.

### Fixed

- **`/auth/callback` was never mounted.** `finish_login` was written, and unit
  tested against a mock provider, and the router had `/auth/login` and nothing
  else. The provider returned people to `/auth/callback` and treff answered
  **404**. Every test was green, because they exercised the functions and not
  the routes: *a function that works and is not reachable is not a feature.*
  `tests/signing_in.rs` now asks the router the question the provider asks it.
- **`/auth/logout` was missing too**, while every page linked to it. Signing
  out ends the session in the database, not only in the browser.
- **The redirect URI is derived per space** instead of configured once.
  One process serves several hosts, and a cookie belongs to exactly one of
  them: a sign-in begun on `blog.example.org` cannot be finished on
  `forum.example.org`, because the short-lived cookie carrying `state`, the
  nonce and the PKCE verifier is never sent there. Each space is now sent back
  to `https://<its host>/auth/callback`.

### Removed — breaking

- `TREFF_OIDC_REDIRECT_URI` and `services.treff.oidc.redirectUri`. Register one
  redirect URI **per space** with your provider instead. Keeping the option
  would have meant keeping a setting that could only ever be right for one of
  the addresses it applied to.

## 0.1.4 — 2026-09-06

- **`title_key`** per space: the front matter key that holds an article's
  title, `title` by default. Found the hard way on the first host to mirror a
  real directory — a German newsletter writes `titel:`, all fifty files were
  skipped, and the blog came up empty. An empty blog looks exactly like a blog
  nobody has written in yet, which is why this needed a configuration option
  and not a second key in fifty files.
- The skip message now **names the key it looked for** instead of saying "no
  title", which is what made that diagnosis take longer than it should have.
- The unit gives up after **six restarts in five minutes**
  (`StartLimitBurst`). A unit that restarts forever is never `failed`: on that
  same host treff had correctly refused to start 78 times over a missing
  client secret, and `systemctl --failed`, the container status and every
  check that asks "is it running" all said everything was fine.

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
