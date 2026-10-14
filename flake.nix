{
  description = "tbard — audiobook manager and listener for the terminal";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
      src = nixpkgs.lib.cleanSource ./.;
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
    in
    {
      packages = forAll (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "tbard";
          inherit version src;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.makeWrapper ];
          # reqwest loads the system certificates when the client is built (tests).
          nativeCheckInputs = [ pkgs.cacert ];
          # mpv and ffmpeg run as separate processes; put them on tbard's PATH.
          postInstall = ''
            wrapProgram $out/bin/tbard --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.mpv pkgs.ffmpeg ]}
          '';
          meta = {
            description = "Audiobook manager and listener for the terminal";
            homepage = "https://github.com/abelland-lgtm/tbard";
            license = pkgs.lib.licenses.gpl3Plus;
            mainProgram = "tbard";
          };
        };
      });

      checks = forAll (pkgs: {
        # Builds and runs the tests.
        package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        # Only GPL-compatible dependency licenses (deny.toml).
        licenses = pkgs.rustPlatform.buildRustPackage {
          pname = "tbard-licenses";
          inherit version src;
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
            audible-cli # only to explore the API; tbard does not depend on it
          ];
        };
      });
    };
}
