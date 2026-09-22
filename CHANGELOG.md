# Changelog

## 0.6.0 — 2026-09-22

Completing `@handle`, as asked for the day 0.5.0 shipped: *when somebody
types `@`, show everybody as an overlay; when they type on, complete it.*

- **Typing `@` opens a list** under the text field of everybody who may be
  mentioned in this space; typing on narrows it by handle or by any word of
  the name (`@mü` finds Konrad Müller). Arrow keys, Enter or Tab, Escape, or a
  click. What is written is always `@handle `.
- **The first script** — `/assets/mention.js`, from its own route, no
  library, no inline code. The CSP moves from `script-src 'none'` to
  `script-src 'self'` and no further. Nothing depends on it: without it,
  `@handle` is typed by hand as before (ADR 0005).
- **`GET /mentionable`** gives the list: readers of the space with a handle,
  name and handle only, `no-store` — the same rule that decides who a
  mention tells. A list of everybody with an account would say who exists.
- Measured in headless Chrome by `tests/js/run.sh`, which compares what the
  script did against `tests/js/expected.txt`.

## 0.5.0 — 2026-09-22

The bell and `@`-mentions — asked for by a member of the forum, who gets a
mail for every reply and wanted the same at a glance (design and plan:
`docs/design-bell-and-mentions.md`, `docs/plan-stage-3.md`).

- **A bell in the header**, with the number of unread entries, leading to
  `/notifications`. Replies in a topic you follow are **bundled per topic**
  ("3 new replies in …, latest from …"), mentions stand **one by one**.
  Opening a topic reads its entries; "mark all as read" reads the rest. Each
  space has its own bell. Still no script: the number is rendered with the
  page.
- **`@handle` mentions.** The handle is the provider's `preferred_username`,
  stored on the account at sign-in, and shown next to the name on every post
  so it can be copied. A mention tells the person in the bell and by mail —
  **only if the groups of their last sign-in let them read the space**, and
  checked again when the mail is written. Otherwise nothing happens and the
  handle stays plain text, rendered exactly like one that belongs to nobody.
  A follower who is mentioned is told once, as a mention. Mentions in code,
  in links and in addresses are not mentions.
- **A mention mail has its own way out**: its one-click link turns off mails
  for mentions (the bell keeps them), and `/notifications` turns them back
  on. A reply mail's link still unfollows the topic.
- The header wraps on a narrow screen instead of pushing the page sideways —
  it did that before the bell as well.
- Three migrations: `0009_handles`, `0010_inbox`, `0011_mention_mail`.

## 0.4.0 — 2026-09-20

Three findings from the homeserver security audit (B43). Nothing in the forum
looks different, except that signing out is now a button instead of a link.

- **A photograph no longer brings its location along.** An upload was stored
  and served back byte for byte, so a picture taken with a phone handed
  everyone who could read the topic the place it was taken, the moment, and
  the camera. `media::strip_metadata` now removes the metadata segments of
  JPEG, PNG and WebP before anything is written — the file on the disk no
  longer carries it either, which is the half that matters in a backup. It
  strips rather than re-encodes: the reason, and what is deliberately kept,
  are in ADR 0004.
- **Signing out is a POST.** `GET /auth/logout` ended the session, so anything
  that merely *fetches* a link ended it too: a mail client collecting
  previews, a chat unfurling a pasted address, an `<img src>` on any page in
  the world. None of that needs a forged form, and `SameSite=Lax` does not
  hold a top-level GET back. It is the same reason `/t/{id}/follow` has always
  been a POST — this was the route that had been forgotten.
- **A second line of defence against CSRF.** There was one: `SameSite=Lax` on
  the session cookie, which works in a browser and nowhere else. The server
  itself asked nothing, and a cross-site POST to `/t/{id}/reply` created the
  post. It now reads `Sec-Fetch-Site` on every method that changes something
  — not `Origin` or `Referer`, because this site sends `Referrer-Policy:
  no-referrer` on purpose and a browser attaches the fetch metadata anyway.
  `same-site` is refused like `cross-site`: every space is its own host with
  its own groups. A request with no fetch metadata at all is let through,
  because a client that sends none has no borrowed cookies either.

## 0.3.9 — 2026-09-18

- **rustls 0.23.45 (RUSTSEC-2026-0285).** Older versions accepted TLS 1.3
  handshake messages across encryption level boundaries. Only the lock file
  moves; nothing in the forum changes.

## 0.3.8 — 2026-09-14

- **An article is dated by its day, and shows no hour.** The file name of a
  mirrored article carries a date and nothing else; that became midnight UTC,
  and shown in the forum's own zone every article read `02:00` in summer and
  `01:00` in winter — an hour nobody chose. The day now begins at midnight
  where the forum stands, and the timeline, the topic list and the head of the
  article's page print the day alone. The comments under an article were
  written at a moment and keep their hour.

## 0.3.7 — 2026-09-14

- **A link leads where it points — after the sign-in, too.** Someone who
  opened a link to a topic without a session was sent to sign in and then
  landed on the front page, and had to find the topic by hand. The page now
  travels along: into the login redirect, from there into the short-lived
  sign-in cookie, and the callback returns to it. Only a page on this site is
  followed; anything else — another site, the sign-in itself, a form that was
  posted — ends on the front page as before.

## 0.3.6 — 2026-09-11

- **A topic list says who opened a thread AND who answered last.** It carried
  one of the two and hid the other: first the opener's name next to the date
  of somebody else's reply, then — correctly, but half-blind — only whoever
  wrote last. Both are asked after, so the list is a table now: the title with
  its opener underneath, and beside it a column headed "last reply". A thread
  nobody has answered says so instead of crediting its opener with an answer
  they never wrote. On a phone the two columns stack, and the reply carries
  the heading's own word with it.
- **Every date carries the hour and the minute.** A bare date made a thread
  answered this morning look like one answered a week ago last Tuesday, and
  lost the order of two posts written in the same afternoon — the one day the
  order is worth reading. Posts, the topic list and the list of sections all
  say the time now.
- **A clock needs a place, so treff names one.** Timestamps used to be
  rendered in UTC, which a bare date hid; shown with an hour, that is wrong by
  an hour or two for everybody it does not fit. The zone is the machine's own
  unless `timezone` (or `services.treff.timezone`) names another, it is fixed
  once at startup, and a name treff cannot resolve stops it there rather than
  putting every timestamp quietly beside the truth. See
  `docs/decisions/0003-a-clock-needs-a-place.md`.

## 0.3.5 — 2026-09-11

- **A topic leads back to its category.** Whoever arrives from a mail, a
  search or a bookmark stood in a dead end: the browser's back button is not a
  design, and the front page is a level too far up. The line above the title
  says where the topic lives and goes there.

## 0.3.4 — 2026-09-11

- **The button that opens a form says how to close it again.** Open, it
  stretched across the whole box and read as the box's heading rather than as
  a control: the first person to open the reply box found no way back out
  short of submitting. It keeps its button width now, and open it shows a
  cross instead of the prompt and steps back in colour, while the button that
  actually posts keeps the accent.

## 0.3.3 — 2026-09-11

- **Forms wait to be asked for.** Opening a topic, replying, and editing your
  own post each stand behind a control now instead of an open box: a list is
  read far more often than it is written to, and a thread you had written in
  read twice as long as anybody else's, because every post of yours carried
  its whole text a second time in a textarea. Replying takes the picture
  upload with it — answering in words and answering with a picture are one
  intention, and asking which of the two you want before you may write either
  makes two decisions out of one.
- **Editing and deleting are icons in the post**, drawn as inline SVG rather
  than borrowed from a font: `✎` is a hairline in one font and a coloured
  emoji in the next, and an icon font would be the remote dependency this
  project refuses.
- **Deleting asks first.** The button used to act on the first click; there is
  no `confirm()` on a page without JavaScript, so the question has a page of
  its own at `GET /p/<id>/delete`. The GET changes nothing and refuses anybody
  who may not press the button.
- **A topic list says who wrote last.** It used to pair the name of whoever
  OPENED the topic with the date of its last activity — two halves of two
  different events, read as one line. Title, and under it in small type the
  name and the date that belong together.

## 0.3.2 — 2026-09-07

- **A new topic notifies too.** The queue was only filled on a reply. For mail
  that followed — a fresh topic has no subscribers but its author, and nobody
  is notified about their own post — but for the operator's channel it was a
  gap: whoever runs the instance wants to know that a topic was opened, and
  nothing said so.
- **The webhook no longer carries the post itself**, only who wrote where and
  a link. It is the operator's channel, and the operator has deliberately no
  special rights here: *everyone may edit and delete their own, and nobody
  else's — including whoever runs the instance.* A push carrying every
  stranger's words to a telephone hands out some of those rights quietly —
  you read along without opening the forum and without anybody noticing. The
  title of a topic is visible to the circle anyway; the text stays where
  everyone reads it under the same conditions.

## 0.3.1 — 2026-09-07

- **A webhook as a second exit**: one `POST` per post — not one per
  subscriber, because whatever is behind it fans out on its own and one
  notification per person would be a stack of identical messages on one
  telephone. JSON with `title`, `message`, `click` and an optional `topic`,
  so it fits ntfy, Gotify, a bridge or a script.
- **The two channels share the queue and nothing else.** A mail server that is
  down must not hold up the webhook, and a webhook answering 500 must not hold
  up the mail; they are drained separately, with the same retries and the same
  bounded giving up.
- No redirects are followed, and the URL comes from the configuration file
  alone: a webhook whose target could be steered by a post would be an SSRF
  with a friendly name, and a `302` would carry the bearer token to an address
  nobody agreed to.
- reqwest comes **through `openidconnect`**, which already has it. A second
  copy with its own `rustls` would have pulled in a second TLS provider
  (aws-lc-rs next to lettre's `ring`) — more attack surface and more build
  time for the same HTTP request.

### Found by the test

`compose` demanded an address for every queued notification. Only mail needs
one — a webhook goes to a place, not to a person — so every webhook row was
being marked "nothing to send to" whenever the writing account had no address.
The webhook would have been silent exactly when mail was, and for the same
reason, which is what makes a second channel worth having.

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
