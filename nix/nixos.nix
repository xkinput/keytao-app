{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.keytao-installer;
  system = pkgs.stdenv.hostPlatform.system;
in
{
  options.services.keytao-installer = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = pkgs.stdenv.isLinux;
      description = "Install KeyTao installer and expose keytao-ime environment defaults.";
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${system}.default;
      defaultText = "inputs.keytao-installer.packages.${system}.default";
      description = "Package providing keytao-installer and keytao-ime.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    environment.variables.XMODIFIERS = lib.mkDefault "@im=keytao";
  };
}
