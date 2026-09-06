# treff — design

**Status:** the skeleton builds; the features are being written one at a time
(`plan-stage-1.md`).
**Authority:** this file is authoritative for *the application*. Anything about
one particular deployment — a host name, a container, a reverse proxy, an
identity provider's own configuration — belongs to whoever runs the instance,
not here.

This document exists so that a decision is written down once and not argued
twice. Where a decision was reversed, the old reasoning stays in place with the
reason it fell.

---

## 1. What it is

A small forum for a closed group. Two things share one program:

| View | What it looks like | Typical use |
|---|---|---|
| `timeline` | posts newest first, replies underneath | a blog for a handful of authors |
| `topics` | topics sorted by last reply, grouped in categories | a forum |

They are **the same mechanism with different values**. The difference is one
line of configuration, not a second code path.

**What it deliberately is not:** a public forum. There is no open
registration, no sign-up form, no password reset — accounts exist in your
identity provider or they do not exist at all. That is what makes the rest of
the design small: no spam defence, no moderation queue, no trust levels, and
none of the legal apparatus a public forum owes to strangers. The circle is
closed and stays closed.

## 2. Why not something off the shelf

Checked in order, with the reason each one fell. This is here so the question
does not start over in six months.

| Candidate | Fell because |
|---|---|
| **Ech0** (Go) | SSO and permissions are set **in the admin web interface** and live in its database. No group→role mapping: the owner ticks a box per account. And it is a microblog, not a forum. |
| **Discourse** | The only ready-made system that can be configured declaratively — and it would have worked. It fell on size: Ruby, Postgres, Redis and a job runner for a handful to a hundred people, plus a database migration on **every** start. A failed migration is a dead forum whose way back runs through a database restore. |
| **Flarum** | OIDC only as a Composer extension, and the packaged form has no extension mechanism. Not declarable. |
| **Lemmy** | OAuth providers are configured in the admin interface. And it is a federated link aggregator, not a forum for a closed circle. |
| **WriteFreely + remark42** | Covers the blog, not the forum. Two services for the smaller half of the job. |
| **WordPress** | Brings the role model along, and with it PHP, a database server, a plugin from outside the distribution, and an attack surface this whole design argues against. |
| **Autumn** (Rust framework) | See [ADR 0001](decisions/0001-axum-over-a-framework.md) — measured, not guessed. |

What settled it is that the thing itself is small. Topics, replies, categories,
attachments — that is a manageable program as long as you do not rebuild half
of Discourse's feature list (§5 says what is left out). Sign-in is no longer
the expensive part it once was: the OIDC libraries in Rust are solved work.

## 3. Sign-in: OIDC itself, no side door

**treff speaks OIDC directly** — authorization code flow with PKCE, discovery
from the issuer, its own session cookie. Everything is configurable, so the
application runs against any provider (Authentik, Keycloak, Zitadel, Pocket ID,
Google), not just the one it was written against.

| Variable | Meaning |
|---|---|
| `TREFF_OIDC_ISSUER` | issuer URL; discovery hangs off it |
| `TREFF_OIDC_CLIENT_ID` | the client |
| `TREFF_OIDC_CLIENT_SECRET_FILE` | a **path**, never the secret itself |
| `TREFF_OIDC_GROUP_CLAIM` | default `groups` |
| `TREFF_CONFIG` | path to `spaces.toml` |

**It does not start without issuer and client id.** No default, no fallback to
"then it runs without sign-in". A missing secret file is a startup failure, not
a warning.

**Roles come from the token, not from a header.** Most providers already put
the user's groups into a claim under the standard `profile` scope; treff reads
that claim by name and matches it against the group lists in the
configuration. No property mapping to write, no admin interface to click.

**An empty group list grants nothing.** It never means "everything" and never
falls back to a default role. Every permission question is asked as "is this
identity in one of *these* groups", and an unknown group name simply matches
nobody.

**A reverse proxy in front is welcome, but treff never trusts it.** If your
proxy does forward-auth, treff still runs its own sign-in behind it — two
redirects against the same provider session are invisible after the first one,
and a bug in either layer does not open the service. What treff *does* need
from a proxy is the original `Host` header (see §4); everything else it can
decide by itself.

The reasoning for reading identity from the token rather than from proxy
headers, including what was wrong with the first draft, is
[ADR 0002](decisions/0002-identity-from-the-token.md).

## 4. The data model: space, category, topic, post

```toml
# spaces.toml — the interface to whoever runs an instance.

[[space]]
host     = "blog.example.org"
title    = "Notes"
view     = "timeline"
read     = ["Household", "Friends"]
# Articles are mirrored from this directory instead of being written here.
articles = "/etc/treff/articles"

  [[space.category]]
  slug  = "notes"
  title = "Notes"
  post  = []                            # nobody opens an article in a browser
  reply = ["Household", "Friends"]      # everybody comments

[[space]]
host  = "forum.example.org"
title = "Treff"
view  = "topics"
read  = ["Household", "Friends"]

  [[space.category]]
  slug  = "films"
  title = "Films"
  post  = ["Household", "Friends"]
  reply = ["Household", "Friends"]

  [[space.category]]
  slug  = "offtopic"
  title = "Off topic"
  post  = ["Household", "Friends"]
  reply = ["Household", "Friends"]
```

- **A space is an address.** One process serves several hosts to different
  audiences.
- **Categories carry the rights.** `read` on the space, `post` and `reply` per
  category. The names are group names from your provider; treff does not
  interpret them.
- **Categories live in the configuration, not in the database.** The price is
  that a new category is a deployment; the gain is that a typo in a group name
  is visible in a file under review instead of becoming a silent category
  nobody may enter.
- **The space is decided by the `Host` header, and an unknown host is
  refused** — not mapped to the first space. Behind a reverse proxy this means
  the proxy must pass the original `Host` through; otherwise every request ends
  in the refusal branch. That is the intended failure: guessing would make the
  separation between two audiences a matter of luck.

### Articles that are written somewhere else

A space may name an `articles` directory. Everything in it that looks like
`YYYY-MM-DD-<name>.md` with a `title` in its front matter becomes a topic —
mirrored into the database at startup, keyed by its file name, so a comment is
an ordinary reply to an ordinary topic and every rule above still holds.

The point is not the file format. It is that **the article and the
announcement are the same text**: whoever runs an instance already writes a
line about what changed, and this makes that line the article rather than a
second thing to write. A category fed this way sets `post = []` — an article
is opened by a commit in the directory, not by a form in a browser. That is
also why "a category nobody may post in" is legal and not an error.

Mirroring is idempotent and one-way: files decide the title and the body,
the database decides nothing about them and keeps the comments. An article
whose file disappears stops being listed; its comments are kept, because
deleting what people wrote is not a side effect a file deletion should have.

**The date in the file name is the publication date.** A file dated in the
future is a draft and is skipped until its day arrives. This is what lets
someone write the article while the thing it describes is still being rolled
out — date it for the day it will be true, and it appears by itself. It also
keeps treff honest against whatever else consumes the same directory: if
another channel publishes those files too, both should hold a draft back on
the same day, or the two disagree about the same text.

## 5. Scope

**Stage 1 — what makes it usable at all:**

- Topics and replies in Markdown, rendered and put through a sanitizer.
- Two views: timeline (reverse chronological) and topic list (by last reply).
- **A category overview** for a space with more than one category — which is
  the normal case for a forum, and was an oversight in the first draft: the
  front page showed whichever category happened to be first.
- **Articles mirrored from a directory** (§4), so a microblog can be fed by
  the same text that announces a change elsewhere.
- Attachments: images (§6).
- **Everyone may edit and delete their own, and nobody else's** — including
  whoever runs the instance. In a closed circle every post carries a name; a
  moderation layer would be building for a problem that is not there.
- OIDC sign-in with a session and a sign-out.
- Interface in German and English.
- Package, NixOS module, tests, CI.

**Stage 2, and the order has a reason:**

- Full-text search (SQLite FTS5).
- Per-topic subscriptions, notified over SMTP and a generic webhook.

A forum without notifications gets read exactly once by someone who does not
live with it. Stage 2 is therefore not optional polish — it is the point at
which inviting people makes sense.

**Left out permanently, not "later":** RSS (nothing behind a closed sign-in can
subscribe), federation, trust levels, reactions, private messages, open
registration. Whoever misses one of these changes this section before writing
code.

## 6. Attachments are the attack surface

It is the only path on which someone else's bytes enter the server, so it is
spelled out rather than left to judgement:

- **Allowlist by magic bytes**, never by file extension or the browser's
  content type: JPEG, PNG, WebP, GIF. **No SVG** — it is an XSS vector with a
  file extension.
- Served with a **fixed** `Content-Type` from our own detection,
  `X-Content-Type-Options: nosniff`, and `Content-Disposition: inline` only for
  the four allowed types.
- A **CSP without `unsafe-inline`**: no inline script, no inline style
  attribute, no third-party script, no CDN, no remote font. The stylesheet is
  served as its own route.
- A size limit and a per-post count limit, both configurable.
- Rendered Markdown goes through a sanitizer, not through a regular
  expression.

## 7. Storage and backup

SQLite, one file, `journal_mode=WAL` and `foreign_keys=ON` set on every
connection.

A filesystem snapshot of a WAL-mode database catches a data file plus a
write-ahead log in an unknown relationship — usually recoverable, sometimes
not. treff therefore ships **`treff export <file>`**, which writes a
self-contained copy via `VACUUM INTO`. Call it before your snapshot; that, and
not the snapshot itself, is the backup.

## 8. Risks, named

- **Our own code with other people's input** is the new thing here. §6 is the
  answer to it, and it is only as good as its implementation.
- **Our own session handling** (cookie, `state`, `nonce`, CSRF, session
  fixation) is an attack surface that a header-based design would not have
  had. The path is well trodden and so are the libraries — but it is our code,
  and it is tested like it.
- **One maintainer.** The AGPL and a plain, boring stack are the mitigation:
  someone else can pick it up.
