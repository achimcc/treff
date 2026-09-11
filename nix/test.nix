# A NixOS VM test. It proves the things that only a real machine can show:
# that the unit comes up, that the secret is NOT in it, and that the generated
# configuration is what the service actually reads.
{
  pkgs,
  module,
  package,
}:
let
  articles = pkgs.runCommand "treff-articles" { } ''
    mkdir -p $out
    cat > $out/2020-01-01-hello.md <<'EOF'
    ---
    title: Hello from a file
    kind: note
    ---

    This article was **never** typed into a browser.
    EOF
    cat > $out/2999-01-01-later.md <<'EOF'
    ---
    title: Not yet
    ---

    Dated in the future, so it is a draft.
    EOF
  '';
in
pkgs.testers.runNixOSTest {
  name = "treff";

  nodes.machine =
    { ... }:
    {
      imports = [ module ];

      # The machine keeps UTC while the forum is read in Berlin — the case the
      # option exists for, and the one that proves the zone comes from the
      # configuration rather than from `/etc/localtime`.
      time.timeZone = "UTC";

      services.treff = {
        enable = true;
        inherit package;
        listen = "127.0.0.1:8080";
        timezone = "Europe/Berlin";
        oidc = {
          issuer = "https://auth.example.org/application/o/treff/";
          clientId = "treff";
          # The credential form, because that is the one that has to work:
          # a systemd specifier, resolved at start, with the secret never in a
          # place the service could read by accident.
          clientSecretFile = "%d/oidc";
        };
        spaces = [
          {
            host = "blog.example.org";
            title = "Notes";
            view = "timeline";
            read = [
              "Household"
              "Friends"
            ];
            inherit articles;
            category = [
              {
                slug = "notes";
                title = "Notes";
                post = [ ];
                reply = [
                  "Household"
                  "Friends"
                ];
              }
            ];
          }
          {
            host = "forum.example.org";
            title = "Treff";
            view = "topics";
            read = [
              "Household"
              "Friends"
            ];
            category = [
              {
                slug = "general";
                title = "General";
                post = [ "Household" ];
                reply = [ "Household" ];
              }
            ];
          }
        ];
      };

      systemd.services.treff.serviceConfig.LoadCredential = [
        "oidc:/etc/treff-secret"
      ];

      environment.etc."treff-secret".text = "the-client-secret";
      environment.systemPackages = [ pkgs.curl ];
    };

  testScript = ''
    machine.wait_for_unit("treff.service")
    machine.wait_for_open_port(8080)

    def code(host, path="/"):
        return machine.succeed(
            f"curl -s -o /dev/null -w '%{{http_code}}' -H 'Host: {host}' http://127.0.0.1:8080{path}"
        )

    # An unknown host is refused. The separation between two audiences holds
    # on a real machine too, not only in the unit tests.
    assert code("evil.example.org") == "403", "an unknown host was served"

    # A known host without a session goes to the sign-in.
    assert code("blog.example.org") == "303", "a known host did not redirect"

    # The provider does not exist in this VM, and that is the point: the
    # service still runs, and the sign-in says so instead of the machine
    # sitting there dead after a restart in which the provider was late.
    assert code("blog.example.org", "/auth/login") == "503", "login did not report the outage"

    # THE SECRET IS NOT IN THE UNIT. Only the path to it is.
    machine.fail("systemctl cat treff.service | grep -q the-client-secret")
    machine.fail("systemctl show treff.service | grep -q the-client-secret")
    # The unit points at a CREDENTIAL, not at a file lying around for the
    # service to read, and certainly not at the secret. systemd expands %d
    # before `systemctl show` gets to see it, so the assertion is on the
    # expanded form — and on the unit actually loading the credential.
    machine.succeed("systemctl cat treff.service | grep -q 'LoadCredential=oidc:/etc/treff-secret'")
    machine.succeed(
        "systemctl show -p Environment treff.service | grep -q '/credentials/treff.service/oidc'"
    )
    # And the proof that it works is not a string at all: the service is up,
    # which it only manages if it could read the secret — treff refuses to
    # start when the file behind TREFF_OIDC_CLIENT_SECRET_FILE is missing.
    machine.succeed("systemctl is-active treff.service")

    # The generated configuration is what the service reads, and it carries the
    # groups from the module — so a typo would have failed the build.
    config_path = machine.succeed(
        "systemctl show -p Environment treff.service | tr ' ' '\\n' | grep TREFF_CONFIG | cut -d= -f2-"
    ).strip()
    machine.succeed(f"grep -q Household {config_path}")
    machine.succeed(f"grep -q 'view = \"timeline\"' {config_path}")

    # The articles arrived from the store directory, and the one dated in the
    # future did not.
    journal = machine.succeed("journalctl -u treff.service --no-pager")
    assert "1 articles" in journal, f"the article was not mirrored: {journal}"

    # THE NAMED ZONE RESOLVES INSIDE THE HARDENED UNIT. `ProtectSystem =
    # "strict"` leaves /etc readable, so the zone database is there — but that
    # is a claim about systemd, and this is the measurement. The machine keeps
    # UTC; if the name had not resolved, treff would have stopped at startup
    # rather than serving a page two hours beside the truth.
    assert "shown in Europe/Berlin" in journal, (
        f"the configured zone did not reach the service: {journal}"
    )
    machine.succeed(f"grep -q 'timezone = \"Europe/Berlin\"' {config_path}")

    # And the state that must survive a restart does. The cookie key is
    # generated once; if it were made up per start, every restart would sign
    # everyone out although their sessions are still in the database.
    key = machine.succeed("sha256sum /var/lib/treff/cookie.key").split()[0]
    machine.succeed("systemctl restart treff.service")
    machine.wait_for_open_port(8080)
    assert machine.succeed("sha256sum /var/lib/treff/cookie.key").split()[0] == key, (
        "the cookie key changed across a restart"
    )
    machine.succeed("test -f /var/lib/treff/treff.db")

    # The export runs against the live database and writes a file that stands
    # on its own — this is what the maintenance window calls before a snapshot.
    machine.succeed("systemd-run --pipe --wait --property=DynamicUser=no "
                    "--setenv=TREFF_DATA_DIR=/var/lib/treff "
                    "${package}/bin/treff export /tmp/backup.db")
    machine.succeed("test -s /tmp/backup.db")
  '';
}
