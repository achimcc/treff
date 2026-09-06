# ADR 0002 — Identity comes from the token, not from proxy headers

**Date:** 2026-09-02
**Status:** accepted (reverses the first draft of the design)
**Decision:** treff runs the OIDC authorization code flow itself and reads the
user's identity and groups from the ID token. It never derives identity from a
request header, not even behind a trusted forward-auth proxy.

## Context

The first draft had treff read `X-Forwarded-*`-style headers set by an
authenticating reverse proxy (Authentik's forward-auth mode sets user, name and
groups this way), verify an accompanying signed token, and skip having a
sign-in of its own. It looked like a real saving: no OIDC client, no session,
no cookie, no `state`/`nonce` handling.

## Why that was reversed

1. **The advantage was imagined.** The assumption was that group membership
   was only conveniently available through the proxy headers. It is not — the
   provider's standard `profile` scope already carries the groups claim, and
   reading it costs one configuration key.
2. **It would have made the application useless to anyone without that exact
   proxy** — that is, to almost everyone a public repository is meant to
   reach. With OIDC, treff runs against Keycloak, Zitadel, Pocket ID,
   Authentik or Google.
3. **It rested on an unverified measurement:** whether the signed-token header
   even arrives in the proxy mode we would use. The OIDC path, by contrast, is
   in production in comparable services.

**Offering both** was the obvious compromise and is rejected too: one
authentication path, one set of tests. Two paths means the untested one is the
one that opens the service.

## Consequences

- treff carries its own session handling — cookie, `state`, `nonce`, CSRF,
  session fixation. That is an attack surface the header design would not have
  had, and it is named as a risk in the design (§8).
- A forward-auth proxy in front stays perfectly fine, and is even recommended
  as a first door: after the first sign-in the second redirect is invisible,
  and a failure in either layer does not open the service on its own.
- The only thing treff requires from a proxy is the original `Host` header,
  because that is how a space is chosen.
