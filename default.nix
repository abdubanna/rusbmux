{
  lib,
  rustPlatform,
}:
let
  cargoToml = lib.importTOML ./Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = cargoToml.package.name;
  version = cargoToml.package.version;
  src = ./.;
  cargoLock.lockFile = ./Cargo.lock;

  meta = {
    description = "A usbmuxd replacement in pure Rust";
    homepage = "https://github.com/abdubanna/rusbmux";
    license = with lib.licenses; [
      mit
      asl20
    ];
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
    mainProgram = "rusbmux";
  };
}
