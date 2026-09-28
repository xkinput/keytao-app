{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/bfc1b8a4574108ceef22f02bafcf6611380c100d";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      ...
    }:
    (flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          config = {
            allowUnfree = true;
            android_sdk.accept_license = true;
          };
        };

        ndkVersion = "26.1.10909125"; # NDK r26b
        cmdLineToolsVer = "13.0";

        androidComposition = pkgs.androidenv.composeAndroidPackages {
          cmdLineToolsVersion = cmdLineToolsVer;
          platformToolsVersion = "35.0.2";
          buildToolsVersions = [ "35.0.0" ];
          platformVersions = [ "36" ];
          includeNDK = true;
          ndkVersions = [ ndkVersion ];
          abiVersions = [
            "arm64-v8a"
            "x86_64"
          ];
          includeEmulator = false;
          includeSources = false;
          includeSystemImages = false;
          systemImageTypes = [ ];
          useGoogleAPIs = false;
          useGoogleTVAddOns = false;
        };

        androidSdk = androidComposition.androidsdk;
      in
      let
        version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
        # Update after each release with scripts/update-nix-release.sh.
        releaseVersion = "1.2.1-alpha.90";
        releaseHashes = {
          x86_64-linux = "sha256-sIiWnkSXjcypi6QwHRsJ4v45gYYNVlDn+20LGMS6P4s=";
          aarch64-linux = "sha256-uWCBwfRWj/eaZCaWWAbgTY+2TJIFivVaAPWiRX8ZGWU=";
        };
      in
      let
        keytaoLinuxIme = pkgs.rustPlatform.buildRustPackage {
          pname = "keytao-linux-ime";
          inherit version;
          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [
            "--package"
            "keytao-linux-ime"
            "--bin"
            "keytao-ime"
          ];
          nativeBuildInputs = with pkgs; [
            pkg-config
            llvmPackages.libclang
          ];
          buildInputs = with pkgs; [
            librime
            libxkbcommon
            libxcb
            libx11
            dbus
            glib
            gtk3
            openssl
            freetype
          ];
          doCheck = false;
          RIME_INCLUDE_DIR = "${pkgs.librime}/include";
          RIME_LIB_DIR = "${pkgs.librime}/lib";
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          # Install the KDE Virtual Keyboard desktop entry so keytao-ime appears in
          # KDE Settings → Virtual Keyboard.  X-KDE-Wayland-VirtualKeyboard=true tells
          # KWin to spawn the process with a private WAYLAND_SOCKET fd.
          postInstall = ''
            install -Dm644 \
              crates/keytao-linux-ime/keytao-wayland-launcher.desktop \
              $out/share/applications/keytao-wayland-launcher.desktop
            substituteInPlace $out/share/applications/keytao-wayland-launcher.desktop \
              --replace-fail 'Exec=keytao-ime' "Exec=$out/bin/keytao-ime"

            # ibus-daemon scans $XDG_DATA_DIRS/ibus/component on startup; without
            # this file KeyTao only exists for as long as the running daemon
            # remembers the RegisterComponent call.
            install -Dm644 \
              crates/keytao-linux-ime/keytao.xml \
              $out/share/ibus/component/keytao.xml
            substituteInPlace $out/share/ibus/component/keytao.xml \
              --replace-fail '<exec>keytao-ime' "<exec>$out/bin/keytao-ime"
          '';
        };

        keytaoAppRelease = pkgs.stdenv.mkDerivation {
          pname = "keytao-app-bin";
          version = releaseVersion;
          src = pkgs.fetchurl {
            url = "https://github.com/xkinput/keytao-app/releases/download/v${releaseVersion}/keytao-app-${releaseVersion}-linux-${
              {
                x86_64-linux = "x64";
                aarch64-linux = "arm64";
              }
              .${system}
            }.deb";
            hash = releaseHashes.${system};
          };
          nativeBuildInputs = with pkgs; [
            dpkg
            autoPatchelfHook
            wrapGAppsHook3
          ];
          buildInputs = with pkgs; [
            stdenv.cc.cc.lib
            libxkbcommon
            libxcb
            dbus
            glib
            gtk3
            gdk-pixbuf
            pango
            atk
            cairo
            harfbuzz
            libepoxy
            fontconfig
            gflags
            snappy
            libunwind
            zlib
          ];
          # Flutter loads GL/EGL dynamically, so DT_NEEDED alone is insufficient.
          runtimeDependencies = [ pkgs.libglvnd ];
          unpackPhase = ''
            runHook preUnpack
            dpkg-deb -x "$src" .
            runHook postUnpack
          '';
          dontConfigure = true;
          dontBuild = true;
          dontStrip = true;
          installPhase = ''
            runHook preInstall
            mkdir -p "$out"
            cp -a usr/bin usr/lib usr/share etc "$out/"
            substituteInPlace "$out/share/applications/KeyTao.desktop" \
              --replace-fail 'Exec=keytao-app' "Exec=$out/bin/keytao-app"
            substituteInPlace "$out/share/applications/keytao-wayland-launcher.desktop" \
              "$out/etc/xdg/autostart/keytao-ime.desktop" \
              --replace-fail 'Exec=keytao-ime' "Exec=$out/bin/keytao-ime"
            substituteInPlace "$out/share/ibus/component/keytao.xml" \
              --replace-fail '<exec>keytao-ime' "<exec>$out/bin/keytao-ime"
            runHook postInstall
          '';
          preFixup = ''
            addAutoPatchelfSearchPath "$out/lib/KeyTao/lib" "$out/lib/KeyTao/runtime/lib"
            # Wrap the bin symlinks; keep the real executables beside their data.
            gappsWrapperArgs+=(
              --prefix PATH : "${pkgs.lib.makeBinPath [ pkgs.procps ]}"
              --prefix LD_LIBRARY_PATH : "$out/lib/KeyTao/lib:$out/lib/KeyTao/runtime/lib"
            )
          '';
          meta = {
            description = "KeyTao official Linux release, repackaged from deb";
            homepage = "https://github.com/xkinput/keytao-app";
            platforms = [
              "x86_64-linux"
              "aarch64-linux"
            ];
            mainProgram = "keytao-app";
          };
        };
      in
      {
        packages = pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          default = keytaoAppRelease;
          keytao-app-bin = keytaoAppRelease;
          keytao-linux-ime = keytaoLinuxIme;
        };

        apps = pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          default = {
            type = "app";
            program = "${keytaoAppRelease}/bin/keytao-app";
          };
        };

        devShells.default = pkgs.mkShell {
          # Tools (go into PATH / nativeBuildInputs, no effect on RPATH)
          packages = [
            androidSdk
            pkgs.jdk17
            pkgs.curl
            pkgs.file
            pkgs.unzip
            pkgs.patchelf
            pkgs.pkg-config
            pkgs.sccache
            pkgs.mold
          ];

          # Runtime libraries: placed in buildInputs so the Nix cc-wrapper
          # automatically injects -L/-rpath into NIX_LDFLAGS, which cargo uses
          # when linking. This gives every compiled binary the correct RPATH
          # without needing patchelf.
          buildInputs = with pkgs; [
            librime
            libxcb
            libxkbcommon
            wayland
            dbus
            gtk3
            glib
            gdk-pixbuf
            gdk-pixbuf.dev
            pango
            atk
            cairo
            harfbuzz
            harfbuzz.dev
            freetype
            freetype.dev
            bzip2
            bzip2.dev
            xz
            xz.dev
            openssl
            xdotool
            ibus # Added for IBus IM module
          ];

          ANDROID_HOME = "${androidSdk}/libexec/android-sdk";
          NDK_HOME = "${androidSdk}/libexec/android-sdk/ndk/${ndkVersion}";
          RIME_INCLUDE_DIR = "${pkgs.librime}/include";
          RIME_LIB_DIR = "${pkgs.librime}/lib";
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          BINDGEN_EXTRA_CLANG_ARGS = "-I${pkgs.librime}/include";
          BZIP2_LIB_DIR = "${pkgs.bzip2}/lib";
          BZIP2_INCLUDE_DIR = "${pkgs.bzip2.dev}/include";
          LZMA_API_STATIC = "0";

          # Ensure runtime libs are findable even when cargo doesn't embed RPATH.
          # As a flake attribute (not shellHook), direnv's `use flake` exports this.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
            with pkgs;
            [
              librime
              libxcb
              libxkbcommon
              wayland
              dbus
              gtk3
              glib
              gdk-pixbuf
              pango
              atk
              cairo
              harfbuzz
              freetype
              bzip2
              xz
              openssl
              xdotool
              ibus
            ]
          );

          shellHook = ''
            export JAVA_HOME="${pkgs.jdk17}"
            export RUSTC_WRAPPER="${pkgs.sccache}/bin/sccache"
            export MOLD_PATH="${pkgs.mold}/bin/mold"
            export CARGO_INCREMENTAL=0

            # Generate GTK IM Modules cache safely so direnv doesn't destroy it
            export GTK_PATH="${pkgs.gtk3}/lib/gtk-3.0/3.0.0:${pkgs.ibus}/lib/gtk-3.0/3.0.0"
            _immodules_cache="$HOME/.cache/keytao-immodules.cache"
            if ${pkgs.gtk3}/bin/gtk-query-immodules-3.0 > "/tmp/keytao_im.tmp" 2>/dev/null; then
                ${pkgs.gtk3}/bin/gtk-query-immodules-3.0 ${pkgs.ibus}/lib/gtk-3.0/3.0.0/immodules/im-ibus.so >> "/tmp/keytao_im.tmp" 2>/dev/null
                mv "/tmp/keytao_im.tmp" "$_immodules_cache"
            fi
            export GTK_IM_MODULE_FILE="$_immodules_cache"

            # Add Android platform-tools (adb) and cmdline-tools to PATH
            export PATH="${androidSdk}/libexec/android-sdk/platform-tools:$PATH"
            export PATH="${androidSdk}/libexec/android-sdk/cmdline-tools/${cmdLineToolsVer}/bin:$PATH"

                        # Embed RPATH for all runtime libs so binaries work without LD_LIBRARY_PATH.
                        # The -L flags are injected via NIX_LDFLAGS by mkShell; here we add -rpath
                        # so the dynamic linker finds the libs at runtime even outside the shell.
                        # Note: Rust on Linux defaults to lld (-fuse-ld=lld via gcc-ld wrapper),
                        # so no explicit linker selection is needed here.
                        export RUSTFLAGS="-C link-arg=-Wl,-rpath,${
                          pkgs.lib.makeLibraryPath (
                            with pkgs;
                            [
                              librime
                              libxcb
                              libxkbcommon
                              wayland
                              dbus
                              gtk3
                              glib
                              gdk-pixbuf
                              pango
                              atk
                              cairo
                              harfbuzz
                              freetype
                              bzip2
                              openssl
                              ibus
                            ]
                          )
                        }"
                        # Install Android Rust cross-compilation targets (idempotent)
                        for _t in aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android; do
                          rustup target add "$_t" 2>/dev/null || true
                        done

                        # Nix's mkShell sets DEVELOPER_DIR / SDKROOT to the Nix apple-sdk, which
                        # lacks libiconv.tbd stubs. Cargo is configured to use /usr/bin/clang as
                        # linker for host builds, so we point
                        # SDKROOT at the Xcode CLT SDK so /usr/bin/clang can find system libs.
                        unset DEVELOPER_DIR
                        # Try CLT SDK paths in order of preference; fall back to leaving SDKROOT as-is.
                        for _sdk_candidate in \
                          "/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk" \
                          "/Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk" \
                          "/Library/Developer/CommandLineTools/SDKs/MacOSX15.sdk"; do
                          if [ -d "$_sdk_candidate" ]; then
                            export SDKROOT="$_sdk_candidate"
                            break
                          fi
                        done
          '';
        };
      }
    ))
    // {
      nixosModules.default = import ./nix/nixos.nix { inherit self; };
    };
}
