# Stage 2 — the implementation plan

Six tasks, worked one at a time and test first. `design.md` says *what* and
*why*; this file says *in which order* and *what counts as done*.

**Why this stage exists**, in the design's own words: *a forum without
notifications gets read exactly once by someone who does not live with it.*
Stage 2 is the point at which inviting people makes sense — so the
notifications come first and the search comes after, even though the search is
the easier piece.

**How to work a task:** write the test, **see it fail** with the expected
message, implement the smallest thing that passes, see it green (`cargo test`,
`cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`), commit with
a targeted `git add`. Before finishing, `nix flake check`. The working rules
are in `../AGENTS.md`.

## Status

| | |
|---|---|
| Done | **Task 1** — an address from the token |
| Done | **Task 2** — subscriptions: who hears about what |
| Done | **Task 3** — the outbox, and a sender that survives a restart |
| Done | **Task 4** — one-click unsubscribe, without signing in |
| Next | **Task 5** — a generic webhook as a second exit (waiting for a decision) |
| Done | **Task 6** — full-text search over FTS5 |

---

## Task 1 · An address from the token

**Files:** `src/auth/mod.rs`, `src/auth/oidc.rs`, `src/db/`, migrations

Sessions carry a subject, a name and groups. None of those is an address, and
without one there is nothing to send to.

- [x] Read the `email` claim alongside `name`, and store it on the session and
      on the account row. It is **optional**: a provider that does not send one
      is a provider whose users get no mail, not a provider whose users are
      locked out. Reading, writing and signing in must all keep working
      without it.
- [x] The claim reaches us the same way the groups do — out of the ID token
      that was verified, not out of `additional_claims()`, which is empty by
      construction (0.2.1 exists because of that).
- [x] **An address is not an identity.** Keep matching on `subject`; the
      address is a delivery detail that may change, and a provider that lets
      people edit it would otherwise hand one account to another person.
- [x] The provider must be asked for the scope. `openid profile` does not carry
      `email`; the scope goes in the authorization request, and the NixOS
      module's example configuration says so.

**Done when:** a signed-in session carries the address when the provider sends
one, carries `None` when it does not, and a test asserts both — plus one that
signs in twice with a changed address and gets the same account.

## Task 2 · Subscriptions: who hears about what

**Files:** migrations, `src/db/subscriptions.rs`, `src/web/`

- [x] A `subscriptions` table: subject, topic, how (mail, webhook, both), when
      it was created. Foreign key to the topic, deleted with it.
- [x] **Writing subscribes you.** Opening a topic or replying to one is the
      clearest statement that you want to know what happens next, and asking
      afterwards would be a dialogue nobody wants. It can be undone.
- [x] A control on the topic page: following / not following. Display follows
      the right, as everywhere else — someone who may not read the category is
      not offered a subscription to it.
- [x] **Never notify the author of the post that triggered it.** Not a nicety:
      a forum that mails you your own words teaches people to filter it away.

**Done when:** replying creates a subscription, the button removes it, a
second reply by the same person does not create a second one, and deleting a
topic takes its subscriptions with it — each asserted against the database.

## Task 3 · The outbox, and a sender that survives a restart

**Files:** migrations, `src/notify/`, `src/main.rs`, `nix/module.nix`

- [x] Notifications are **written to the database inside the same transaction
      as the post**, and sent by a background task. Two reasons, and both are
      the point: a reply must not be lost because SMTP is down, and a request
      must not wait on the network while somebody watches a spinner.
- [x] Rows carry attempts and a next-attempt time. A permanent refusal (5xx at
      the protocol level, an address the server rejects) stops after a bounded
      number of tries and stays visible in the table rather than looping.
- [x] SMTP over `lettre`: host, port, username from the environment, **password
      from a file** — the same rule as the OIDC secret, for the same reason.
      STARTTLS by default; refuse to start with credentials but no TLS.
- [x] One mail per person per event, with the topic title as the subject, the
      body of the reply as text, and a link back. Plain text and HTML, because
      the from-address is a real mailbox and the reader may be anywhere.
- [x] The module gets the options, and the VM test proves the unit starts
      without a mail server and does not die when there is nothing to send.

**Done when:** a reply leaves exactly one row per subscriber in the outbox, a
sender with a mock SMTP server empties it, a sender with no SMTP server leaves
it and retries, and a restart loses nothing.

## Task 4 · One-click unsubscribe, without signing in

**Files:** `src/web/`, `src/notify/`

- [x] Every notification carries an unsubscribe link that works **without
      signing in**. Behind a sign-in it is not an unsubscribe link, it is a
      sign-in link, and the person will use their mail client's spam button
      instead — which costs the whole domain, not one subscription.
- [x] The link is an HMAC over (subject, topic), with a key from a file. It
      cancels **one** subscription, never all of them, and it is idempotent.
- [x] A wrong or truncated token and an unknown subscription answer the same
      way. Anything else says whether a person is in this forum.
- [x] `List-Unsubscribe` and `List-Unsubscribe-Post`, so a mail client can
      offer the button itself.

**Done when:** the link cancels exactly one subscription, twice in a row
without an error, and a tampered token changes nothing and says nothing.

## Task 5 · A generic webhook as a second exit

**Files:** `src/notify/`, `nix/module.nix`

- [ ] Per instance: a URL, an optional bearer token from a file, and a
      timeout. `POST` with title, text and link as JSON — generic on purpose,
      so it fits ntfy, Gotify, Matrix bridges and a shell script behind a
      reverse proxy.
- [ ] Same outbox, same retries, same bounded attempts. A webhook that is down
      must not hold up the mail.
- [ ] **No redirects followed, and no arbitrary host at request time.** The URL
      comes from the configuration file and nowhere else; a webhook whose
      target could be steered by a post would be an SSRF with a friendly name.

**Done when:** a mock server receives one request per event with the fields
documented in the README, and a mock server that answers 500 is retried and
then given up on.

## Task 6 · Full-text search over FTS5

**Files:** migrations, `src/db/search.rs`, `src/web/`, `src/web/views.rs`

- [x] An FTS5 table over post bodies and topic titles, kept up to date by
      triggers rather than by application code — the application forgets, a
      trigger does not.
- [x] **A search is scoped to one space and to the categories that person may
      read.** This is a security property, not a convenience: a hit list that
      leaks a title from the other audience is a leak, and it is the kind that
      looks like a feature until someone notices.
- [x] A search field in the header, results with the matching line in context.
- [x] `porter unicode61` so German and English both stem, and a test with
      umlauts that would fail under the default tokenizer.

**Done when:** a search finds a word in a body and in a title, finds nothing
from a space the person cannot read, and a test proves that the index survives
an edit and a deletion.

---

## Waiting for a decision

**Task 5, the webhook target.** ntfy listens on `127.0.0.1:2586` inside
infra-01 and is reachable only through Caddy, which puts forward-auth in front
of it — and a webhook from treff has no Authentik session. Three ways out, and
the choice touches the zone table rather than this repository:

1. a zone edge `treff-01 → infra-01:2586` and ntfy listening on the guest
   address (narrowest, but a new edge),
2. a Caddy exception for `POST /<topic>` authenticated by an ntfy token
   instead of forward-auth,
3. skip ntfy and let the webhook point at something else entirely.

Mail (tasks 3 and 4) does not depend on this, so it is built first. Noted
2026-09-06 for whoever runs the instance to decide.

## What this stage does NOT cover

Everything the design lists as left out permanently: RSS, federation, trust
levels, reactions, private messages, open registration. And moderation — in a
closed circle every post carries a name.
