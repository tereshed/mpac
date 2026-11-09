#!/bin/sh
# Build and stage only local desktop bundle inputs; never starts a daemon.
set -eu
mpac_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mpac_profile=release
case "${1:-}" in
    '') ;;
    --debug) mpac_profile=debug; shift ;;
    --help|-h)
        printf '%s\n' 'Usage: ./scripts/prepare-desktop.sh [--debug]' 'Builds the UI and daemon, then stages Tauri sidecar/resources. Does not start or install the app.'
        exit 0 ;;
    *) printf '%s\n' 'Unknown option. Usage: ./scripts/prepare-desktop.sh [--debug]' >&2; exit 2 ;;
esac
if [ "$#" -ne 0 ]; then
    printf '%s\n' 'Unexpected arguments. Usage: ./scripts/prepare-desktop.sh [--debug]' >&2
    exit 2
fi
if [ "$(uname -s)" != Darwin ]; then
    printf '%s\n' 'The desktop build currently supports macOS only.' >&2
    exit 1
fi
if ! command -v npm >/dev/null 2>&1; then
    printf '%s\n' 'Node.js and npm are required to build the desktop UI.' >&2
    exit 1
fi
cd "$mpac_root"
# Tauri invokes cargo as a subprocess. Match scripts/cargo.sh fallback without
# modifying a shell profile, global toolchain or global Rust configuration.
if ! command -v cargo >/dev/null 2>&1 && [ -x "$mpac_root/.tools/cargo/bin/cargo" ]; then
    export CARGO_HOME="$mpac_root/.tools/cargo"
    export RUSTUP_HOME="$mpac_root/.tools/rustup"
    export PATH="$CARGO_HOME/bin:$PATH"
fi
export CARGO_TARGET_DIR="$mpac_root/target"
if ! mpac_cargo_info=$("$mpac_root/scripts/cargo.sh" -vV); then
    printf '%s\n' "$mpac_cargo_info" >&2
    exit 1
fi
mpac_host=$(printf '%s\n' "$mpac_cargo_info" | awk '/^host:/ { print $2; exit }')
case "$mpac_host" in
    aarch64-apple-darwin|x86_64-apple-darwin) ;;
    *) printf '%s\n' 'A native macOS Rust toolchain is required. See ./scripts/bootstrap-rust.sh.' >&2; exit 1 ;;
esac
if [ ! -d "$mpac_root/ui/node_modules" ]; then
    npm --prefix "$mpac_root/ui" ci --ignore-scripts --no-audit --no-fund
fi
printf '%s\n' 'Building MPAC UI…'
npm --prefix "$mpac_root/ui" run build
printf '%s\n' "Building MPAC daemon ($mpac_profile)…"
if [ "$mpac_profile" = release ]; then
    "$mpac_root/scripts/cargo.sh" build -p mpac-daemon --release --locked -j 1
else
    "$mpac_root/scripts/cargo.sh" build -p mpac-daemon --locked -j 1
fi
printf '%s\n' "Building MPAC phone companion ($mpac_profile)…"
if [ "$mpac_profile" = release ]; then
    "$mpac_root/scripts/cargo.sh" build -p mpac-mobile-link --bin mpac-mobile-host --release --locked -j 1
else
    "$mpac_root/scripts/cargo.sh" build -p mpac-mobile-link --bin mpac-mobile-host --locked -j 1
fi
mpac_tauri="$mpac_root/desktop/src-tauri"
mpac_resources="$mpac_tauri/resources"
mpac_staging="$mpac_resources/.ui-staging.$$"
mkdir -p "$mpac_tauri/binaries" "$mpac_resources" "$mpac_staging"
trap 'rm -rf "$mpac_staging"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
cp -R "$mpac_root/ui/dist/." "$mpac_staging/"
# These paths are generated build inputs under desktop/src-tauri only.
cp "$mpac_root/target/$mpac_profile/mpac-daemon" "$mpac_tauri/binaries/mpac-daemon-$mpac_host.tmp"
chmod 755 "$mpac_tauri/binaries/mpac-daemon-$mpac_host.tmp"
mv -f "$mpac_tauri/binaries/mpac-daemon-$mpac_host.tmp" "$mpac_tauri/binaries/mpac-daemon-$mpac_host"
cp "$mpac_root/target/$mpac_profile/mpac-mobile-host" "$mpac_tauri/binaries/mpac-mobile-host-$mpac_host.tmp"
chmod 755 "$mpac_tauri/binaries/mpac-mobile-host-$mpac_host.tmp"
mv -f "$mpac_tauri/binaries/mpac-mobile-host-$mpac_host.tmp" "$mpac_tauri/binaries/mpac-mobile-host-$mpac_host"
rm -rf "$mpac_resources/ui"
mv "$mpac_staging" "$mpac_resources/ui"
trap - EXIT HUP INT TERM
printf '%s\n' "Staged mpac-daemon-$mpac_host and bundled UI. No running daemon was changed."
