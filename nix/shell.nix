{
  mkShell,
  makeFontsConf,

  # Accept but ignore zed-editor so flake.nix can pass it without forcing evaluation
  zed-editor ? null,

  rust-analyzer,
  cargo-nextest,
  cargo-hakari,
  cargo-machete,
  nixfmt-rfc-style,
  protobuf,
  nodejs_22,
}:
mkShell {
  packages = [
    rust-analyzer
    cargo-nextest
    cargo-hakari
    cargo-machete
    nixfmt-rfc-style
    # TODO: package protobuf-language-server for editing zed.proto
    # TODO: add other tools used in our scripts

    # `build.nix` adds this to the `zed-editor` wrapper (see `postFixup`)
    # we'll just put it on `$PATH`:
    nodejs_22
  ];

  env = {
      # note: different than `$FONTCONFIG_FILE` in `build.nix` – this refers to relative paths
      # outside the nix store instead of to `$src`
      FONTCONFIG_FILE = makeFontsConf {
        fontDirectories = [
          "./assets/fonts/lilex"
          "./assets/fonts/ibm-plex-sans"
        ];
      };
      PROTOC = "${protobuf}/bin/protoc";
    };

  shellHook = ''
    if [[ "$(uname -s)" == "Darwin" ]]; then
      # Force system Xcode toolchain and target macOS 26.0
      # Clear any Nix-provided SDK/toolchain hints first
      unset SDKROOT
      unset DEVELOPER_DIR
      export PATH="/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
      export DEVELOPER_DIR="$(/usr/bin/xcode-select -p 2>/dev/null)"
      export SDKROOT="$(/usr/bin/xcrun --show-sdk-path 2>/dev/null)"
      export MACOSX_DEPLOYMENT_TARGET=26.0
      export CC="$(/usr/bin/xcrun --find clang 2>/dev/null)"
      export CXX="$(/usr/bin/xcrun --find clang++ 2>/dev/null)"
      export LD="$(/usr/bin/xcrun --find ld 2>/dev/null)"
      export AR="$(/usr/bin/xcrun --find ar 2>/dev/null)"
      export NM="$(/usr/bin/xcrun --find nm 2>/dev/null)"
      export RANLIB="$(/usr/bin/xcrun --find ranlib 2>/dev/null)"
      if [[ -n "$CC" ]]; then
        _TOOLDIR="$(dirname "$CC")"
        export PATH="$_TOOLDIR:$PATH"
      fi
      # Avoid Nix rpath injection and wrappers
      unset NIX_CFLAGS_COMPILE NIX_LDFLAGS NIX_CFLAGS_LINK NIX_HARDENING_ENABLE
      export NIX_DONT_SET_RPATH=1
      export NIX_NO_SELF_RPATH=1
      # Ensure rustc picks the min target
      export RUSTFLAGS="''${RUSTFLAGS:-} -C link-arg=-mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET"
      echo "[zed devShell] SDKROOT=$SDKROOT, MACOSX_DEPLOYMENT_TARGET=$MACOSX_DEPLOYMENT_TARGET"
    fi
  '';
}
