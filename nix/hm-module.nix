# Módulo de home-manager: `programs.tbard`.
self:
{ config, lib, pkgs, ... }:
let
  cfg = config.programs.tbard;
  toml = pkgs.formats.toml { };
in
{
  options.programs.tbard = {
    enable = lib.mkEnableOption "tbard, Audible desde la terminal";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      description = "Paquete de tbard.";
    };

    settings = lib.mkOption {
      type = toml.type;
      default = { };
      example = { library_dir = "~/Audiolibros"; };
      description = "Contenido de ~/.config/tbard/config.toml.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
    xdg.configFile."tbard/config.toml" = lib.mkIf (cfg.settings != { }) {
      source = toml.generate "tbard-config.toml" cfg.settings;
    };
  };
}
