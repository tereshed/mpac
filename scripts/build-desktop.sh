#!/bin/sh
# Build a signed local .app. Never installs or starts the application.
set -eu
mpac_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mpac_debug=false
case "${1:-}" in
    '') ;;
    --debug) mpac_debug=true; shift ;;
    --help|-h)
        printf '%s\n' 'Usage: ./scripts/build-desktop.sh [--debug]' 'Set APPLE_SIGNING_IDENTITY to reuse a macOS signing identity across builds.' 'Release output: target/release/bundle/macos/MPAC.app' 'Debug output: target/debug/bundle/macos/MPAC.app' 'macOS only. Builds local files; does not install, launch or restart a daemon.'
        exit 0 ;;
    *) printf '%s\n' 'Unknown option. Usage: ./scripts/build-desktop.sh [--debug]' >&2; exit 2 ;;
esac
if [ "$#" -ne 0 ]; then
    printf '%s\n' 'Unexpected arguments. Usage: ./scripts/build-desktop.sh [--debug]' >&2
    exit 2
fi
if [ "$(uname -s)" != Darwin ]; then
    printf '%s\n' 'The desktop build currently supports macOS only.' >&2
    exit 1
fi
if ! command -v npm >/dev/null 2>&1; then
    printf '%s\n' 'Node.js and npm are required to build MPAC Desktop.' >&2
    exit 1
fi
if ! command -v cargo >/dev/null 2>&1 && [ -x "$mpac_root/.tools/cargo/bin/cargo" ]; then
    export CARGO_HOME="$mpac_root/.tools/cargo"
    export RUSTUP_HOME="$mpac_root/.tools/rustup"
    export PATH="$CARGO_HOME/bin:$PATH"
fi
export CARGO_TARGET_DIR="$mpac_root/target"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
# Installs the pinned project-local CLI only. Lifecycle scripts are disabled.
if [ ! -x "$mpac_root/desktop/node_modules/.bin/tauri" ]; then
    npm --prefix "$mpac_root/desktop" ci --ignore-scripts --no-audit --no-fund
fi
if [ "$mpac_debug" = true ]; then
    "$mpac_root/scripts/prepare-desktop.sh" --debug
else
    "$mpac_root/scripts/prepare-desktop.sh"
fi
cd "$mpac_root/desktop"
printf '%s\n' 'Bundling MPAC.app…'
if [ "$mpac_debug" = true ]; then
    "$mpac_root/desktop/node_modules/.bin/tauri" build --debug --bundles app
    mpac_profile=debug
else
    "$mpac_root/desktop/node_modules/.bin/tauri" build --bundles app
    mpac_profile=release
fi
mpac_bundle="$mpac_root/target/$mpac_profile/bundle/macos/MPAC.app"
# Reuse the helper's Keychain identity across Developer ID builds.
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    codesign --force --sign "$APPLE_SIGNING_IDENTITY" --identifier com.mpac.desktop.mobile-host --options runtime "$mpac_bundle/Contents/MacOS/mpac-mobile-host"
fi
# Retain the selected signing identity and microphone entitlement in the final seal.
codesign --force --sign "${APPLE_SIGNING_IDENTITY:--}" --options runtime --entitlements "$mpac_root/desktop/src-tauri/Entitlements.plist" "$mpac_bundle"
codesign --verify --deep --strict "$mpac_bundle"
printf '%s\n' "Desktop bundle: $mpac_bundle"
