# home-manager module: `programs.tbard`.
self:
{ config, lib, pkgs, ... }:
let
  cfg = config.programs.tbard;
  toml = pkgs.formats.toml { };
in
{
  options.programs.tbard = {
    enable = lib.mkEnableOption "tbard, Audible from the terminal";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      description = "The tbard package.";
    };

    settings = lib.mkOption {
      type = toml.type;
      default = { };
      example = { library_dir = "~/Audiobooks"; language = "en"; };
      description = ''
        Contents of tbard's config.toml. When set, the file becomes read-only
        and the TUI settings screen cannot change it.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
    xdg.configFile."tbard/config.toml" = lib.mkIf (cfg.settings != { }) {
      source = toml.generate "tbard-config.toml" cfg.settings;
    };
  };
}
