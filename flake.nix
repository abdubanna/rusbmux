{
  description = "rusbmux — a usbmuxd replacement in pure Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      crane,
    }:
    flake-utils.lib.eachSystem
      [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ]
      (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          craneLib = crane.mkLib pkgs;

          crateName = craneLib.crateNameFromCargoToml { cargoToml = ./Cargo.toml; };

          src = craneLib.cleanCargoSource (craneLib.path ./.);

          commonArgs = {
            pname = crateName.pname;
            version = crateName.version;
            inherit src;
            strictDeps = true;
            meta = with pkgs.lib; {
              description = "A usbmuxd replacement in pure Rust";
              homepage = "https://github.com/abdubanna/rusbmux";
              license = with licenses; [
                mit
                asl20
              ];
              platforms = platforms.linux ++ platforms.darwin;
              mainProgram = "rusbmux";
            };
          };

          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          rusbmux = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
            }
          );

        in
        {
          packages = {
            rusbmux = rusbmux;
            default = rusbmux;
          };
        }
      )
    // {
      overlays.default = final: prev: {
        rusbmux = self.packages.${final.stdenv.hostPlatform.system}.default;
      };

      nixosModules.default = {
        _file = ./nixos-module.nix;
        imports = [ ./nixos-module.nix ];

        # Make pkgs.rusbmux available so the module's default package
        # (services.rusbmux.package = pkgs.rusbmux) resolves on its own.
        nixpkgs.overlays = [ self.overlays.default ];
      };
    };
}
