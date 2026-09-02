# treff

A small forum for closed groups. Members sign in with your own OIDC provider —
Authentik, Keycloak, Zitadel, Pocket ID — and what they may do follows from the
groups in their token. No local accounts, no passwords, no open registration.

- **Spaces** are addresses. One instance can serve `blog.example.org` and
  `forum.example.org` to different audiences from the same process.
- **Categories** carry the rights: who may open a topic, who may reply.
- A space renders either as a **timeline** (newest first — a blog) or as a
  **topic list** (a forum).

An empty group list grants nothing, never everything. An unknown `Host` is
refused rather than mapped to the first space. Missing OIDC settings stop the
program at startup instead of opening it up.

## Status

Early. The skeleton builds and the shape is settled; the features are being
written one at a time. Not yet ready to run.

## Licence

AGPL-3.0-only. If you run a modified copy as a network service, your users are
entitled to its source.
