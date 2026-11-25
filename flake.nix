{
  description = "Pass a keyboard or mouse through an emulated Bluetooth device";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          bluekey = pkgs.rustPlatform.buildRustPackage {
            pname = "bluekey";
            version = "0.2.0";

            src = pkgs.lib.cleanSource ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            nativeBuildInputs = [
              pkgs.pkg-config
            ];

            buildInputs = [
              pkgs.dbus
            ];

            postInstall = ''
              install -Dm644 bluekey/install/bluekeyd.service \
                "$out/lib/systemd/system/bluekeyd.service"
              substituteInPlace "$out/lib/systemd/system/bluekeyd.service" \
                --replace-fail "/usr/local/bin/bluekeyd" "$out/bin/bluekeyd"

              install -Dm644 bluekey/install/us.colbystuff.Bluekey.conf \
                "$out/share/dbus-1/system.d/us.colbystuff.Bluekey.conf"

              install -Dm644 bluekey/install/us.colbystuff.Bluekey.service \
                "$out/share/dbus-1/system-services/us.colbystuff.Bluekey.service"
              substituteInPlace "$out/share/dbus-1/system-services/us.colbystuff.Bluekey.service" \
                --replace-fail "/usr/local/bin/bluekeyd" "$out/bin/bluekeyd" \
                --replace-fail "SystemdService=bluekey.service" "SystemdService=bluekeyd.service"
            '';

            meta = with pkgs.lib; {
              description = "Pass a keyboard or mouse through an emulated Bluetooth device";
              homepage = "https://github.com/olbyolby/bluekey";
              license = licenses.gpl3Only;
              platforms = platforms.linux;
              mainProgram = "bluekey";
            };
          };

          default = self.packages.${system}.bluekey;
        });

      nixosModules = {
        bluekey = { config, lib, pkgs, ... }:
          let
            cfg = config.services.bluekey;
          in
          {
            options.services.bluekey = {
              enable = lib.mkEnableOption "Bluekey Bluetooth keyboard and mouse emulator daemon";

              package = lib.mkOption {
                type = lib.types.package;
                default = self.packages.${pkgs.stdenv.hostPlatform.system}.bluekey;
                description = "The bluekey package to use.";
              };
            };

            config = lib.mkIf cfg.enable {
              environment.systemPackages = [ cfg.package ];

              services.dbus.packages = [ cfg.package ];
              systemd.packages = [ cfg.package ];
              systemd.services.bluekeyd.wantedBy = [ "multi-user.target" ];

              users.users.bluekey = {
                isSystemUser = true;
                group = "bluekey";
                extraGroups = [ "input" ];
              };

              users.groups.bluekey = {};
            };
          };

        default = self.nixosModules.bluekey;
      };

      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.default ];
            packages = with pkgs; [
              cargo
              rustc
              rust-analyzer
              clippy
              rustfmt
            ];
          };
        });
    };
}
