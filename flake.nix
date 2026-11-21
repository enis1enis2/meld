{
  description = "Meld: language composition compiler, CLI and language server";

  # No narHash is invented here. flake.lock is generated with `nix flake lock`
  # and records the verified nixpkgs revision and hash; refresh it with the
  # same command rather than editing the lock by hand.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";

  outputs = { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forEachSystem = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forEachSystem (pkgs: rec {
        default = meld-lang;
        meld-lang = pkgs.callPackage ./package.nix { };
      });

      apps = forEachSystem (pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.system}.default}/bin/meld";
        };
      });

      devShells = forEachSystem (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.system}.default ];
          packages = with pkgs; [ cargo rustc rustfmt clippy ];
        };
      });
    };
}
