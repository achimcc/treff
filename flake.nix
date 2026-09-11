{
  description = "treff — a small forum for closed groups behind an OIDC provider";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in
    {
      packages = forAll (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "treff";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;

          # THE TESTS NEED A TIME ZONE DATABASE, the build does not. Two of
          # them convert a moment to `Europe/Berlin` to prove that summer time
          # is not an hour the reader has to add, and the build sandbox has no
          # `/etc/zoneinfo` to look it up in. `TZDIR` is where jiff looks
          # first, so pointing it at the tzdata in the store is enough; in
          # service the zone comes from the machine, which is the point.
          nativeCheckInputs = [ pkgs.tzdata ];
          preCheck = ''
            export TZDIR=${pkgs.tzdata}/share/zoneinfo
          '';
          meta = {
            description = "A small forum for closed groups, authenticated by your own OIDC provider";
            license = pkgs.lib.licenses.agpl3Only;
            mainProgram = "treff";
          };
        };
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            sqlite
          ];
        };
      });

      nixosModules.default = ./nix/module.nix;

      checks = forAll (
        pkgs:
        {
          package = self.packages.${pkgs.system}.default;
        }
        # The VM test needs a machine of the same architecture to boot, so it
        # is only offered where that is the case.
        // nixpkgs.lib.optionalAttrs (pkgs.stdenv.hostPlatform.isLinux) {
          vm = import ./nix/test.nix {
            inherit pkgs;
            module = self.nixosModules.default;
            package = self.packages.${pkgs.system}.default;
          };
        }
      );
    };
}
