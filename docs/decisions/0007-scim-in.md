# ADR 0007 — The provider pushes everybody in

**Date:** 2026-09-22
**Status:** accepted
**Decision:** the identity provider pushes its people and groups into treff
over SCIM, on a third route of the internal door (`/scim/v2`, ADR 0006) behind
its own token. treff does not ask the provider for anybody. A person who
arrives this way lands in the same `accounts` row a sign-in writes, and a
person who leaves keeps that row, emptied.

## Context

treff learned a person's handle and their groups **at a sign-in**, and only
then (ADR 0002: identity comes from the token). That was enough as long as
everything treff did concerned somebody who was looking at it.

Mentions ended that. On the evening `@` went live, the list of who could be
mentioned offered exactly one person: whoever was writing. Six accounts
existed, one had a handle. Everybody else had either never opened the forum or
was still carrying a session from before the release. **A mention is for
somebody who is NOT looking** — that is the whole reason it sends mail — so a
forum that only knows its visitors cannot mention anybody worth mentioning.

## Why the provider pushes, and treff does not ask

Asking would mean treff holding a credential for the provider's own API,
learning its shape, and choosing how often to ask — three things that are the
provider's business and would each drift on their own. Pushing costs treff a
door and the provider a configured URL, and the provider already sends a
change the moment it happens: a new account or a group membership is in the
forum in seconds rather than at the next sign-in.

SCIM is what this provider speaks for pushing. It is not a great protocol, but
it is the one already implemented on the other side, and a protocol on one
side only is a protocol nobody has tested.

## What the door takes

Read from the client's source (2026.5.6), because what a SCIM client *sends*
and what SCIM *says* are two different documents:

- **`/ServiceProviderConfig` decides what arrives afterwards.** treff answers
  `patch: false`, so groups come whole by `PUT`; `bulk.maxOperations: 0`, so
  every operation of one change comes in one request; `filter: true`, for the
  one lookup the client falls back to. A field missing from that answer is not
  an error anywhere — the client warns and uses its own defaults — so the test
  names every field it validates.
- **Members come as `PATCH` regardless**, because the client's `create` calls
  `_patch_add_users` whatever the configuration said. Three shapes exist:
  `add` on `members`, `remove` on `members`, and `remove` on
  `members[value eq "…"]` with no value. treff takes those three and refuses
  everything else — the whole request, not the part it understood.
- **`POST` on somebody already here writes and answers like a creation.** The
  alternative is `409`, after which the client searches by filter and takes
  the first hit: a detour with two more ways to go wrong, for a case that is
  only "the sync ran twice".

## The id is the UUID, or the request is refused

treff's OIDC provider hands out `sub = user.uuid`; the operator's SCIM mapping
sets `externalId` to the same value. **That equality is what makes the person
pushed in and the person who signs in one row.** The client's own default
would be `str(user.uid)`, a hash — and a door that accepted it would quietly
build a second population that no sign-in ever matches, with the `@` list
offering ghosts. So a non-UUID id is not a malformed field to route around; it
is the mapping being wrong, and it is a `400`.

## Leaving clears the row, it does not erase it

`active: false` and `DELETE` both empty the handle, the address and every
group: not mentionable, not mailed, not a reader. The row stays, because posts
carry `author_subject` and **a forum's history is not rewritten because
somebody went.** Deleting a person's posts is a separate decision, and nobody
has asked for it.

`DELETE` also takes the person out of the DIRECTORY — afterwards `GET
/Users/{id}` is a `404` and the listing does not show them. That is not
tidiness. The client's `discover` matches a listed resource to one of ITS
users by user name and address, so a new account with the same user name
would be linked to the UUID of the person who left, and everything written
for the newcomer would land in the leaver's row. A second `DELETE` is
therefore a `404` too, and that costs nothing: the client logs "Object not
found in remote provider" at info level and carries on (`lib/sync/outgoing/
tasks.py:331`) — a `404` on delete is not an error over there.

## Consequences

- Whoever can reach the internal port **and** holds the SCIM token can rewrite
  who exists in the forum, including handing themselves a handle and a group.
  The port is reachable from the provider's zone only, and the token is the
  provider's credential — the same price ADR 0006 already names, on a route
  that writes instead of reads. `/scim/v2` is part of every review from here.
- **The provider's group names are now load-bearing.** `groups_json` is
  recomputed from the memberships it pushes, and `may_read` reads it. A group
  renamed there changes who may read here, at once and without a deploy.
- treff has a population it did not meet. The `@` list shows people who have
  never opened it, and a mention mails them. That is the point, and it is also
  the first time treff tells one person that another exists — which is why the
  list stays what it always was: the readers of THIS space, with a handle, and
  nothing but their name and handle.
