# treff

A small forum for closed groups. Members sign in with your own OIDC provider —
Authentik, Keycloak, Zitadel, Pocket ID — and what they may do follows from the
groups in their token. No local accounts, no passwords, no open registration.

- **Spaces** are addresses. One instance can serve `blog.example.org` and
  `forum.example.org` to different audiences from the same process.
- **Categories** carry the rights: who may open a topic, who may reply.
- A space renders either as a **timeline** (newest first — a blog) or as a
  **topic list** (a forum).
- A timeline can be **fed from a directory of Markdown files**, so the text
  that announces a change elsewhere is the same text people comment on here.

An empty group list grants nothing, never everything. An unknown `Host` is
refused rather than mapped to the first space. Missing OIDC settings stop the
program at startup instead of opening it up.

## Status

Early. **v0.1.0** — everything below works and is covered by tests, but it has
not run in front of real people yet. Search and notifications are the next
stage.

## Configuration

One file describes the addresses. It is TOML, and unknown keys are an error —
a typo fails at startup rather than quietly granting nothing.

```toml
# The blog: articles come from files, everybody with an account comments.
[[space]]
host     = "blog.example.org"
title    = "Notes"
view     = "timeline"
read     = ["Household", "Friends"]
articles = "/etc/treff/articles"
attachment_max_bytes = 8388608          # optional, this is the default

  [[space.category]]
  slug  = "notes"
  title = "Notes"
  post  = []                            # nobody opens an article in a browser
  reply = ["Household", "Friends"]

# The forum: topics with a title, discussion underneath.
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

The group names are yours: treff matches them against the groups claim of the
token, exactly, without case folding — a fuzzy comparison would be privilege
escalation by typo.

### Environment

| Variable | Meaning |
|---|---|
| `TREFF_CONFIG` | path to the file above; **required** |
| `TREFF_DATA_DIR` | database, cookie key and attachments (default `/var/lib/treff`) |
| `TREFF_LISTEN` | address and port (default `127.0.0.1:8080`) |
| `TREFF_OIDC_ISSUER` | issuer URL; discovery hangs off it; **required** |
| `TREFF_OIDC_CLIENT_ID` | **required** |
| `TREFF_OIDC_CLIENT_SECRET_FILE` | a **path**, never the secret itself; **required** |
| `TREFF_OIDC_REDIRECT_URI` | where the provider sends people back to; **required** |
| `TREFF_OIDC_GROUP_CLAIM` | default `groups` |

## Behind a reverse proxy

treff speaks plain HTTP and expects a proxy in front of it. **The proxy must
pass the original `Host` header through.** A space is chosen by that header,
and an unknown one is refused — so a proxy that rewrites it makes every request
end in a 403. That is the intended failure: guessing would turn the separation
between two audiences into a matter of luck.

The cookies are marked `Secure`, so the site must be served over HTTPS.

If your proxy also does forward authentication, keep it: treff still runs its
own sign-in behind it, two redirects against the same provider session are
invisible after the first one, and a fault in either layer does not open the
service on its own.

## Configuring your provider

treff needs a confidential client with the authorization code flow, PKCE, and
a groups claim in the ID token. Two worked examples:

**Authentik** — create an OAuth2/OpenID provider with client type
*confidential*, redirect URI `https://forum.example.org/auth/callback`, and the
`openid profile` scopes. The standard `profile` scope already carries
`groups`, so `TREFF_OIDC_GROUP_CLAIM=groups` is all treff needs; no property
mapping to write. The issuer is
`https://auth.example.org/application/o/<slug>/`.

**Keycloak** — create a client with *Client authentication* on and *Standard
flow* enabled, redirect URI as above. Groups are not in the token by default:
add a mapper of type *Group Membership*, name it `groups`, set *Full group
path* to **off** (otherwise the claim reads `/Household`, and treff compares
exactly), and tick *Add to ID token*. The issuer is
`https://keycloak.example.org/realms/<realm>`.

Any provider that speaks OIDC discovery and can put group names into a claim
will do; those two are written out because they are the ones that were tried.

## Running it

With Nix, the flake offers a package and a module:

```nix
{
  inputs.treff.url = "github:achimcc/treff";

  # in your NixOS configuration
  imports = [ inputs.treff.nixosModules.default ];
  services.treff = {
    enable = true;
    package = inputs.treff.packages.${pkgs.system}.default;
    oidc = {
      issuer = "https://auth.example.org/application/o/treff/";
      clientId = "treff";
      clientSecretFile = "/run/secrets/treff-oidc";
      redirectUri = "https://forum.example.org/auth/callback";
    };
    spaces = [ /* the same shape as the TOML above */ ];
  };
}
```

The module generates the configuration from Nix, so a wrong view or a
malformed slug fails the build. `nix flake check` runs the unit tests, the
integration tests and a NixOS VM test.

## Backups

The data directory holds a SQLite database in WAL mode. A filesystem snapshot
of that catches a data file plus a write-ahead log in an unknown relationship —
usually recoverable, sometimes not. Run

```
treff export /path/to/backup.db
```

before the snapshot. It works against the running service and writes a file
that stands on its own. It refuses to overwrite an existing one.

## Licence

AGPL-3.0-only. If you run a modified copy as a network service, your users are
entitled to its source.
