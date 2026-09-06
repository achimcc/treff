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
    category = map (c: {
      inherit (c)
        slug
        title
        post
        reply
        ;
    }) space.category;
  } // lib.optionalAttrs (space.articles != null) { articles = toString space.articles; };

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
        type = lib.types.path;
        description = ''
          A file holding the client secret. A path, never the secret itself:
          anything in the unit is world-readable in the store and in
          `systemctl show`.
        '';
      };
      redirectUri = lib.mkOption {
        type = lib.types.str;
        description = "Where the provider sends people back to, e.g. https://forum.example.org/auth/callback.";
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
        TREFF_OIDC_REDIRECT_URI = cfg.oidc.redirectUri;
        TREFF_OIDC_GROUP_CLAIM = cfg.oidc.groupClaim;
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
    };
  };
}
