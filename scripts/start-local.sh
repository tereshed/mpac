#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"
if [ ! -d ui/node_modules ]; then
    npm --prefix ui ci --ignore-scripts
fi
npm --prefix ui run build
./scripts/cargo.sh build -p mpac-daemon --locked
exec ./target/debug/mpac-daemon "$@"
