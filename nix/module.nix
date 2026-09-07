# The NixOS module. It generates `spaces.toml` from Nix, which is the point:
# a typo in a group name or a view fails the build instead of becoming a silent
# category nobody may enter.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.treff;

  category = lib.types.submodule {
    options = {
      slug = lib.mkOption {
        type = lib.types.strMatching "[a-z0-9-]+";
        description = "URL segment of the category. Lower case, digits, hyphens.";
      };
      title = lib.mkOption {
        type = lib.types.str;
        description = "What the category is called on the page.";
      };
      post = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        description = ''
          Groups whose members may open a topic here. The empty list means
          nobody, never everybody — which is what a space fed by articles
          wants.
        '';
      };
      reply = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        description = "Groups whose members may reply here.";
      };
    };
  };

  space = lib.types.submodule {
    options = {
      host = lib.mkOption {
        type = lib.types.str;
        description = "The Host header this space answers to.";
      };
      title = lib.mkOption {
        type = lib.types.str;
        description = "The name shown in the header of every page.";
      };
      view = lib.mkOption {
        type = lib.types.enum [
          "timeline"
          "topics"
        ];
        description = "A blog (timeline) or a forum (topics).";
      };
      read = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        description = "Groups whose members may see this space at all.";
      };
      articles = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = ''
          A directory of `YYYY-MM-DD-name.md` files with a `title` in their
          front matter. They are mirrored into the first category of this
          space at startup; comments stay in the database. A file dated in the
          future is a draft.

          Point this at a path in the store (a directory from your own flake,
          for instance) rather than at a mutable one: a new file then changes
          the unit, and the service restarts to pick it up. A bind mount would
          be silent.
        '';
      };
      home = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "https://example.org";
        description = ''
          Where this space came from — a landing page, or simply the other
          space. It appears in the header, labelled with its host name.

          A space is one address among several, and the way back belongs ON
          the page: the browser's back button is memory rather than
          navigation, and it is empty for anyone who arrived by bookmark.
        '';
      };
      titleKey = lib.mkOption {
        type = lib.types.str;
        default = "title";
        example = "titel";
        description = ''
          The front matter key that holds an article's title. Those files are
          written for something else — a newsletter, a static site — and their
          keys are in their author's language; asking every one of them to
          carry a second, English key saying the same thing is the duplication
          a one-way mirror exists to avoid.
        '';
      };
      attachmentMaxBytes = lib.mkOption {
        type = lib.types.ints.positive;
        default = 8 * 1024 * 1024;
        description = "Largest attachment this space accepts.";
      };
      category = lib.mkOption {
        type = lib.types.listOf category;
        description = "The categories of this space, in the order they appear.";
      };
    };
  };

  # The keys are the ones treff parses; `deny_unknown_fields` on its side means
  # a rename here fails loudly rather than quietly doing nothing.
  toToml = space: {
    inherit (space)
      host
      title
      view
      read
      ;
    attachment_max_bytes = space.attachmentMaxBytes;
    title_key = space.titleKey;
    category = map (c: {
      inherit (c)
        slug
        title
        post
        reply
        ;
    }) space.category;
  }
  // lib.optionalAttrs (space.articles != null) { articles = toString space.articles; }
  // lib.optionalAttrs (space.home != null) { home = space.home; };

  spacesFile = (pkgs.formats.toml { }).generate "treff-spaces.toml" {
    space = map toToml cfg.spaces;
  };
in
{
  options.services.treff = {
    enable = lib.mkEnableOption "treff, a small forum for closed groups";

    package = lib.mkOption {
      type = lib.types.package;
      description = "The treff package to run.";
    };

    listen = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1:8080";
      description = ''
        Address and port to listen on. treff speaks plain HTTP and expects a
        reverse proxy in front of it — one that passes the original Host
        header through, because that is how a space is chosen.
      '';
    };

    dataDir = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/treff";
      description = "Database, cookie key and attachments.";
    };

    spaces = lib.mkOption {
      type = lib.types.listOf space;
      description = "The addresses this instance serves.";
    };

    mail = {
      # NO `enable`. The host decides: set it and mail goes out, leave it and
      # nobody is notified of anything. A second switch would only be a second
      # thing to forget.
      host = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "mail.example.org";
        description = ''
          The SMTP server notifications go out over. Without it treff still
          runs and simply notifies nobody — mail is optional, not half-built.
        '';
      };
      port = lib.mkOption {
        type = lib.types.port;
        default = 587;
        description = "Submission port. 587 with STARTTLS is the default for a reason.";
      };
      username = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "The mailbox to authenticate as, if the server wants one.";
      };
      passwordFile = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "%d/smtp";
        description = ''
          A **path**, never the password itself — the same rule as
          `oidc.clientSecretFile`, for the same reason: a value in the
          environment stands in `/proc/<pid>/environ` and in every
          `systemctl show`. A string rather than a path, so a systemd
          specifier works with `LoadCredential`.
        '';
      };
      from = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "treff@example.org";
        description = ''
          The envelope sender, and where a reply to a notification goes. A
          real mailbox: people do reply to notifications, and a bounce into
          nowhere is a conversation nobody sees.
        '';
      };
      starttls = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Off is for a mail server on localhost and nothing else. treff
          refuses to start with a password and no TLS — sending credentials
          in the clear is not a configuration, it is an accident.
        '';
      };
    };

    webhook = {
      url = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "https://ntfy.example.org/";
        description = ''
          A second exit, besides mail: one `POST` per post, with `title`,
          `message`, `click` and — if set — `topic` as JSON. Generic on
          purpose, so it fits ntfy, Gotify, a Matrix bridge or a script behind
          a reverse proxy.

          **The URL comes from here and nowhere else.** A webhook whose target
          could be steered by something in a post would be an SSRF with a
          friendly name; redirects are not followed either, so a `302` cannot
          carry the token somewhere the operator never agreed to.
        '';
      };
      topic = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "treff";
        description = ''
          Put in the JSON body as `topic`. ntfy wants it there when the body
          is JSON; everything else ignores the field.
        '';
      };
      tokenFile = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "%d/webhook";
        description = ''
          A **path** to a bearer token, never the token itself — the same rule
          as every other secret here. A string rather than a path, so a
          systemd specifier works with `LoadCredential`.
        '';
      };
    };

    oidc = {
      issuer = lib.mkOption {
        type = lib.types.str;
        description = "Issuer URL; discovery hangs off it.";
      };
      clientId = lib.mkOption {
        type = lib.types.str;
        description = "The client id registered with the provider.";
      };
      clientSecretFile = lib.mkOption {
        # `str` and not `path`, deliberately. A systemd credential arrives
        # under `%d`, and the honest way to point at one is
        # `clientSecretFile = "%d/oidc"` together with
        # `systemd.services.treff.serviceConfig.LoadCredential`. A `path`
        # would reject that specifier and push operators towards writing the
        # secret somewhere world-readable instead — which is exactly what this
        # option exists to avoid. The VM test uses the credential form.
        type = lib.types.str;
        description = ''
          Where to read the client secret from. A path, never the secret
          itself: anything in the unit is readable in the store and in
          `systemctl show`.

          systemd specifiers work, and are the recommended form:
          `"%d/oidc"` with `LoadCredential = [ "oidc:/path/to/secret" ]` keeps
          the secret out of the filesystem the service can see.
        '';
      };
      groupClaim = lib.mkOption {
        type = lib.types.str;
        default = "groups";
        description = "Name of the claim carrying the user's groups.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.spaces != [ ];
        message = "services.treff.spaces is empty; treff would answer every request with 403.";
      }
      {
        assertion = lib.all (s: s.category != [ ]) cfg.spaces;
        message = "every treff space needs at least one category.";
      }
      {
        assertion = lib.all (s: s.read != [ ]) cfg.spaces;
        message = "a treff space with an empty `read` list could be entered by nobody.";
      }
      {
        # A host without a sender is a mail nobody can answer and many servers
        # refuse outright. Caught here rather than at the first notification,
        # which is hours later and in a journal nobody is reading.
        assertion = (cfg.mail.host == null) == (cfg.mail.from == null);
        message = "services.treff.mail needs `host` and `from` together, or neither.";
      }
      {
        assertion = !(cfg.mail.passwordFile != null && !cfg.mail.starttls);
        message =
          "services.treff.mail has a password and no STARTTLS. treff refuses to start that "
          + "way, so the build refuses first.";
      }
      {
        # Two spaces with the same host would make which one answers a matter
        # of order. treff refuses it too; failing here means failing at build
        # time.
        assertion = lib.length (lib.unique (map (s: s.host) cfg.spaces)) == lib.length cfg.spaces;
        message = "two treff spaces share a host.";
      }
    ];

    systemd.services.treff = {
      description = "treff — a small forum for closed groups";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];

      environment = {
        TREFF_CONFIG = "${spacesFile}";
        TREFF_LISTEN = cfg.listen;
        TREFF_DATA_DIR = cfg.dataDir;
        TREFF_OIDC_ISSUER = cfg.oidc.issuer;
        TREFF_OIDC_CLIENT_ID = cfg.oidc.clientId;
        # The PATH to the secret. The secret itself never appears here.
        TREFF_OIDC_CLIENT_SECRET_FILE = toString cfg.oidc.clientSecretFile;
        TREFF_OIDC_GROUP_CLAIM = cfg.oidc.groupClaim;
      }
      // lib.optionalAttrs (cfg.mail.host != null) {
        TREFF_SMTP_HOST = cfg.mail.host;
        TREFF_SMTP_PORT = toString cfg.mail.port;
        TREFF_SMTP_FROM = cfg.mail.from;
        TREFF_SMTP_STARTTLS = if cfg.mail.starttls then "1" else "0";
      }
      // lib.optionalAttrs (cfg.mail.username != null) {
        TREFF_SMTP_USERNAME = cfg.mail.username;
      }
      // lib.optionalAttrs (cfg.webhook.url != null) {
        TREFF_WEBHOOK_URL = cfg.webhook.url;
      }
      // lib.optionalAttrs (cfg.webhook.topic != null) {
        TREFF_WEBHOOK_TOPIC = cfg.webhook.topic;
      }
      // lib.optionalAttrs (cfg.webhook.tokenFile != null) {
        TREFF_WEBHOOK_TOKEN_FILE = cfg.webhook.tokenFile;
      }
      // lib.optionalAttrs (cfg.mail.passwordFile != null) {
        # INDEPENDENT OF `host`, and that is not an oversight. On a host where
        # the mail server's name is itself a secret, `host`, `username` and
        # `from` arrive through an `EnvironmentFile` at runtime and cannot be
        # known at build time — but the password is still a path, and a path
        # is what this option is. Tying it to `host` would force that operator
        # to put the name in the store to get the password out of it.
        TREFF_SMTP_PASSWORD_FILE = cfg.mail.passwordFile;
      };

      serviceConfig = {
        ExecStart = lib.getExe cfg.package;
        DynamicUser = true;
        StateDirectory = "treff";
        WorkingDirectory = cfg.dataDir;

        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        NoNewPrivileges = true;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
        ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" ];
        MemoryDenyWriteExecute = true;
        LockPersonality = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;

        Restart = "on-failure";
        RestartSec = "5s";
      };

      # A UNIT THAT RESTARTS FOREVER IS NEVER `failed`, AND NOTHING SEES IT.
      # Found on 2026-09-06 on the first host to run this: the client secret
      # was not reaching the credential, treff refused to start — correctly —
      # and systemd had restarted it 78 times. `systemctl --failed` was empty,
      # the container was `active`, and every check that asks whether the
      # service is running said yes. Six restarts and it gives up, which is
      # the state monitoring can actually see.
      startLimitIntervalSec = 300;
      startLimitBurst = 6;
    };
  };
}
