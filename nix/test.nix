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
        # The second door, with both tokens as credentials (ADR 0006).
        internal = {
          listen = "127.0.0.1:8081";
          eventsTokenFile = "%d/events";
          bellTokenFile = "%d/bell";
          scimTokenFile = "%d/scim";
        };
        events = {
          space = "forum.example.org";
          linkHosts = [ "jellyfin.example.org" ];
        };
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
        "events:/etc/treff-events"
        "bell:/etc/treff-bell"
        "scim:/etc/treff-scim"
      ];

      environment.etc."treff-secret".text = "the-client-secret";
      environment.etc."treff-events".text = "the-events-token";
      environment.etc."treff-bell".text = "the-bell-token";
      environment.etc."treff-scim".text = "the-scim-token";
      environment.systemPackages = [
        pkgs.curl
        # The VM test reads the ROW, not treff's answer about it (ADR 0007).
        pkgs.sqlite
      ];
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

    # THE SECOND DOOR, on its own port, with its own tokens — measured on the
    # machine, with the tokens arriving as credentials the way they will in
    # production.
    machine.wait_for_open_port(8081)
    def internal(args):
        return machine.succeed(
            f"curl -s -o /dev/null -w '%{{http_code}}' {args}"
        )
    bell = "-H 'X-Treff-User: konrad' -H 'X-Treff-Groups: Household' http://127.0.0.1:8081/internal/bell"
    assert internal(bell) == "401", "the bell answered without a token"
    assert internal(f"-H 'Authorization: Bearer the-events-token' {bell}") == "401", (
        "the events token opened the bell"
    )
    event = (
        "-X POST -H 'Content-Type: application/json' "
        "-H 'Authorization: Bearer the-events-token' "
        "--data '{\"handle\":\"konrad\",\"kind\":\"film_available\",\"title\":\"Dune\",\"source_key\":\"seerr:1\"}' "
        "http://127.0.0.1:8081/internal/events"
    )
    assert internal(event) == "201", "an event was not taken"
    assert internal(event) == "200", "the same event was taken twice"
    answer = machine.succeed(f"curl -s -H 'Authorization: Bearer the-bell-token' {bell}")
    assert '"unread":1' in answer, f"the bell did not count the event: {answer}"
    # And the public listener has none of it.
    assert code("forum.example.org", "/internal/bell") != "200", "the public side answered /internal"

    # THE SCIM DOOR (ADR 0007). There is no identity provider in this VM, so
    # nobody can sign in — which is exactly the case the stage is about: the
    # person has to be here WITHOUT ever having come.
    scim = "http://127.0.0.1:8081/scim/v2"
    ada = "5b1e0c1c-1111-4a4a-9b9b-000000000001"
    household = "5b1e0c1c-2222-4a4a-9b9b-00000000000a"
    def as_provider(args):
        return machine.succeed(
            f"curl -s -o /dev/null -w '%{{http_code}}' "
            f"-H 'Authorization: Bearer the-scim-token' "
            f"-H 'Content-Type: application/scim+json' {args}"
        )
    assert internal(f"{scim}/ServiceProviderConfig") == "401", "SCIM answered without a token"
    assert internal(
        f"-H 'Authorization: Bearer the-bell-token' {scim}/ServiceProviderConfig"
    ) == "401", "the bell token opened SCIM"
    assert as_provider(f"{scim}/ServiceProviderConfig") == "200", "no ServiceProviderConfig"

    user = (
        "--data '{\"schemas\":[\"urn:ietf:params:scim:schemas:core:2.0:User\"],"
        "\"userName\":\"ada\",\"displayName\":\"Ada Lovelace\","
        "\"emails\":[{\"value\":\"ada@example.org\",\"primary\":true}],"
        f"\"active\":true,\"externalId\":\"{ada}\"}}' -X POST {scim}/Users"
    )
    assert as_provider(user) == "201", "SCIM did not take the person"
    group = (
        "--data '{\"schemas\":[\"urn:ietf:params:scim:schemas:core:2.0:Group\"],"
        f"\"displayName\":\"Household\",\"externalId\":\"{household}\"}}' -X POST {scim}/Groups"
    )
    assert as_provider(group) == "201", "SCIM did not take the group"
    members = (
        "--data '{\"Operations\":[{\"op\":\"add\",\"path\":\"members\","
        f"\"value\":[{{\"value\":\"{ada}\"}}]}}]}}' -X PATCH {scim}/Groups/{household}"
    )
    assert as_provider(members) == "200", "SCIM did not take the membership"
    # A shape treff does not know changes nothing and says so.
    assert as_provider(
        "--data '{\"Operations\":[{\"op\":\"replace\",\"path\":\"displayName\","
        f"\"value\":\"Taken over\"}}]}}' -X PATCH {scim}/Groups/{household}"
    ) == "400", "an unknown PATCH was taken"

    # THE ROW ITSELF, not treff's own answer about it: this is the row the `@`
    # list and `may_read` read, and a SCIM door that wrote anywhere else would
    # pass every one of its own routes and still change nothing.
    machine.succeed("systemd-run --pipe --wait --property=DynamicUser=no "
                    "--setenv=TREFF_DATA_DIR=/var/lib/treff "
                    "${package}/bin/treff export /tmp/after-scim.db")
    row = machine.succeed(
        "sqlite3 /tmp/after-scim.db \"select handle || ' ' || groups_json "
        f"from accounts where subject = '{ada}'\""
    ).strip()
    assert row == 'ada [\"Household\"]', f"the person SCIM pushed is not a reader: {row}"
    assert code("forum.example.org", "/scim/v2/Users") != "200", "the public side answered /scim"
  '';
}
