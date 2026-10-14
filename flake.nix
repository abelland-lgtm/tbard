{
  description = "tbard — Audible desde la terminal";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in {
      # shortcut: build básico sin wrapper de mpv/ffmpeg, se completa en la Fase 6.
      packages = forAll (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "tbard";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
        };
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            rustc cargo clippy rustfmt rust-analyzer cargo-deny
            mpv ffmpeg sqlite pkg-config
            audible-cli # solo para el spike (Fase 1), no es dependencia de tbard
          ];
        };
      });
    };
}
