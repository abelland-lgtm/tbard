{
  description = "tbard — Audible desde la terminal";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
      src = nixpkgs.lib.cleanSource ./.;
    in
    {
      packages = forAll (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "tbard";
          version = "0.1.0";
          inherit src;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.makeWrapper ];
          # reqwest carga los certificados del sistema al crear el cliente (tests).
          nativeCheckInputs = [ pkgs.cacert ];
          # mpv y ffmpeg se invocan como procesos externos (no se enlazan).
          postInstall = ''
            wrapProgram $out/bin/tbard --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.mpv pkgs.ffmpeg ]}
          '';
          # Sin meta.license: `unfree` obligaría a activar allowUnfree solo para
          # evaluarlo. La licencia propietaria está en LICENSE.
          meta = {
            description = "Audible desde la terminal";
            mainProgram = "tbard";
          };
        };
      });

      checks = forAll (pkgs: {
        # Compila y pasa los tests.
        package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        # Solo dependencias con licencias permisivas (deny.toml).
        licenses = pkgs.rustPlatform.buildRustPackage {
          pname = "tbard-licenses";
          version = "0.1.0";
          inherit src;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.cargo-deny ];
          buildPhase = "cargo deny --offline check licenses";
          doCheck = false;
          installPhase = "touch $out";
        };
      });

      homeManagerModules.default = import ./nix/hm-module.nix self;

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
