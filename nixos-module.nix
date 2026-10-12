{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.rusbmux;
in
{
  options.services.rusbmux = {
    enable = lib.mkEnableOption "rusbmux usbmuxd-compatible daemon";

    package = lib.mkPackageOption pkgs "rusbmux" { };

    extraArgs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = lib.literalExpression ''[ "--socket /var/run/rusbmux" ]'';
      description = "Extra command-line arguments passed to the rusbmux daemon.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.rusbmux = {
      description = "rusbmux usbmuxd-compatible daemon";
      documentation = [ "https://github.com/abdubanna/rusbmux" ];
      after = [
        "network.target"
        "systemd-udev-trigger.service"
      ];
      conflicts = [ "usbmuxd.service" ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "simple";
        ExecStart = [ "${cfg.package}/bin/rusbmux" ] ++ cfg.extraArgs;
        Restart = "on-failure";
        RestartSec = 2;
        StateDirectory = "lockdown";
      };
    };
  };
}
