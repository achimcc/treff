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
