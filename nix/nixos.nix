{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.keytao-app;
  system = pkgs.stdenv.hostPlatform.system;
in
{
  options.services.keytao-app = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = pkgs.stdenv.isLinux;
      description = "Install the KeyTao app and expose keytao-ime environment defaults.";
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${system}.keytao-app-bin;
      defaultText = "inputs.keytao-app.packages.${system}.keytao-app-bin";
      description = "Package providing keytao-app and keytao-ime.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    # NixOS links desktop entries/icons by default; also expose IBus descriptors.
    environment.pathsToLink = [ "/share/ibus" ];
    environment.etc."xdg/autostart/keytao-ime.desktop".source =
      "${cfg.package}/etc/xdg/autostart/keytao-ime.desktop";
    environment.variables.XMODIFIERS = lib.mkDefault "@im=keytao";
  };
}
