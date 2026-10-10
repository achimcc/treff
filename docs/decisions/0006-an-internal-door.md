# ADR 0006 — An internal door, beside the sign-in

**Date:** 2026-09-22
**Status:** accepted
**Decision:** treff gets a second listener for other services on the same
machine. `POST /internal/events` takes an event for a person;
`GET /internal/bell` and `/internal/bell/stream` answer, for a page outside
treff, what that person's bell shows. The second pair believes a header that
names the person. It is the one place in treff where identity does not come
from treff's own token, and it is built to be as narrow as that allows.

## Context

ADR 0002 says identity comes from the token and there is no side door. It
still holds for everything a browser reaches.

On 2026-09-22 the operator asked for the bell to stand on the start page of
the server treff runs on — a different host, a static page behind the same
identity provider — with the list opening in place and the number following
every change at once; and for film requests from another service to ring the
same bell. treff's own session lives on treff's host and ends after twelve
hours; the start page cannot use it. The start page's proxy, however, has
just asked the identity provider who is there, and knows.

## The door

- **Its own listener**, its own router. No public virtual host reaches it;
  the public router has none of its routes (tested).
- **A token per route**, from a file, compared in constant time. The events
  token does not open the bell and the other way round. A route whose token
  is not configured does not exist — never "open because nothing was set".
  A token without a listener stops treff, and the NixOS module refuses it at
  build time.
- **The bell believes `X-Treff-User` and `X-Treff-Groups`**, set by the proxy
  from the identity provider's answer after it has removed whatever the
  browser sent under those names. That is the trust this door asks for, and
  the reason it must never be exposed.
- **It only reads.** The number and the entries (titles, names, film titles)
  — the list opens on the start page, so the entries have to come through.
  Nothing is marked read through it; reading happens in treff, behind its own
  sign-in.
- **One empty answer** for "nobody by that handle", "those groups may not read
  the space" and "nothing new", so the door says nothing about who exists.

## Events

Keyed by handle, because the person may never have signed in to treff; one
configured space; every field checked on arrival (`db::events::checked`),
links only `https://` on configured hosts; idempotent per source key. No
mail — the service that sends them notifies on its own channel.

## Consequences

Whoever can reach the internal port **and** holds the bell token can read
anybody's unread entries by naming them. The port is reachable only from the
proxy's machine (the operator's zone table), and the token lives in the
proxy's credentials. That is the price of the bell outside treff, paid on
purpose; `/internal/*` is part of every review from here on.

## Addendum, 2026-10-10 — the bell's door marks read (0.13.0)

"It only reads" above is overturned in one point. The overlay on the start
page got the button "mark all as read", and a button on a page outside treff
has only this door to go through: `POST /internal/bell/read`, behind the
bell token, for the person in `X-Treff-User`, resolved exactly as for
reading (the name byte for byte, the groups' spaces).

What it can do is deliberately the least that serves the button: set
`read_at` on that person's entries. It takes no body, names no entry, and
answers 204 whether or not it knew the person. Nothing is created, changed
or deleted, and who exists is still written only by SCIM under its own
token.

**The price.** Whoever reaches the port and holds the bell token could
already read anybody's unread entries; now they can also silence anybody's
bell. And unlike the two reading routes, a browser stands behind this one,
so a foreign page could try to post through the proxy with the person's
sign-in: the route refuses any `Sec-Fetch-Site` other than `same-origin`
(the rule of the public router), and the proxy in front is expected to do
the same and to pass `POST` on this path only.

## Addendum, 2026-10-01 — the door can be encrypted (0.12.0)

"On the same machine" stopped being the whole truth as soon as the first
caller sat in another network zone: the proxy and the identity provider
reach this port over a network, and the tokens and the answers crossed it in
plain HTTP (homeserver audit 3, B145).

The listener now takes a certificate and a key (`TREFF_INTERNAL_TLS_CERT_FILE`,
`TREFF_INTERNAL_TLS_KEY_FILE`) and then speaks TLS only. The decision above
is unchanged — one listener, a token per route, nothing public — and so is
what opens a route: treff asks for no client certificate. TLS keeps the
token from being read on the way; it does not replace it.

Without the two files the door still opens in plain HTTP, and says so at
every start. That is a concession to installations that exist, not a second
recommended mode: with one of the two files set, or with a file that cannot
be used, treff does not start rather than fall back.
