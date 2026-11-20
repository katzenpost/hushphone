{
  description = "hushphone: group chat with push-to-talk voice over the Katzenpost mixnet";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f (import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      }));
    in {
      packages = forAll (pkgs:
        let
          # The channel rust-toolchain.toml pins, without its Android targets.
          channel = (builtins.fromTOML (builtins.readFile ./rust-toolchain.toml)).toolchain.channel;
          rust = pkgs.rust-bin.stable.${channel}.minimal;
          rustPlatform = pkgs.makeRustPlatform { cargo = rust; rustc = rust; };
          # winit and the renderer open these with dlopen, so they are not in the binary's NEEDED.
          runtimeLibs = with pkgs; [
            libGL libxkbcommon wayland
            libx11 libxcursor libxi libxrandr libxcb
          ];
          cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        in {
          default = rustPlatform.buildRustPackage {
            pname = "hushphone";
            version = cargoToml.package.version;
            src = pkgs.lib.cleanSource ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
              allowBuiltinFetchGit = true;
            };
            nativeBuildInputs = with pkgs; [ pkg-config makeWrapper ];
            buildInputs = with pkgs; [ alsa-lib fontconfig ];
            doCheck = false;
            postInstall = ''
              rm -r $out/lib  # the cdylib is for the Android APK
              install -Dm644 packaging/network.katzenpost.hushphone.desktop -t $out/share/applications
              install -Dm644 packaging/network.katzenpost.hushphone.svg -t $out/share/icons/hicolor/scalable/apps
              wrapProgram $out/bin/hushphone --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath runtimeLibs}
            '';
            meta = {
              description = cargoToml.package.description;
              homepage = "https://github.com/katzenpost/hushphone";
              license = pkgs.lib.licenses.agpl3Only;
              mainProgram = "hushphone";
              platforms = pkgs.lib.platforms.linux;
            };
          };
        });
    };
}
