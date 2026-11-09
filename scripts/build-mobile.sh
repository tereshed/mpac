#!/bin/sh
# Build locally; never starts a service, installs on a phone, or signs in.
set -eu
mpac_mobile_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mpac_mobile_platform=${1:-}
case "$mpac_mobile_platform" in ios|android) ;; *) printf '%s\n' 'Usage: scripts/build-mobile.sh ios|android'; exit 2;; esac
export CARGO_HOME="$mpac_mobile_root/.tools/cargo"
export RUSTUP_HOME="$mpac_mobile_root/.tools/rustup"
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_TARGET_DIR="$mpac_mobile_root/target"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
npm --prefix "$mpac_mobile_root/ui" run build
python3 "$mpac_mobile_root/scripts/prepare-mobile.py" "$mpac_mobile_platform"
cd "$mpac_mobile_root/mobile"
if [ "$mpac_mobile_platform" = ios ]; then
    mkdir -p "$mpac_mobile_root/.tools/mobile-ios-bin"
    cp "$mpac_mobile_root/scripts/ios-simulator-xcodebuild.py" "$mpac_mobile_root/.tools/mobile-ios-bin/xcodebuild"
    chmod +x "$mpac_mobile_root/.tools/mobile-ios-bin/xcodebuild"
    export MPAC_MOBILE_ROOT="$mpac_mobile_root"
    export PATH="$mpac_mobile_root/.tools/mobile-ios-bin:$PATH"
    exec ../desktop/node_modules/.bin/tauri ios build --debug --target aarch64-sim --ci --no-sign --archive-only
else
    export JAVA_HOME="${JAVA_HOME:-$(/usr/libexec/java_home -v 17)}"
    export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
    export NDK_HOME="${NDK_HOME:-$ANDROID_HOME/ndk/28.2.13676358}"
    exec ../desktop/node_modules/.bin/tauri android build --debug --target aarch64 --apk --ci
fi
