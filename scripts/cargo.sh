#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if command -v cargo >/dev/null 2>&1; then
    exec cargo "$@"
fi
if [ -x "$root/.tools/cargo/bin/cargo" ]; then
    export CARGO_HOME="$root/.tools/cargo"
    export RUSTUP_HOME="$root/.tools/rustup"
    exec "$CARGO_HOME/bin/cargo" "$@"
fi
printf '%s\n' 'Rust is not installed. Run ./scripts/bootstrap-rust.sh (local .tools installation).'
exit 1
