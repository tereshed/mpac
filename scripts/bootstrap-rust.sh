#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if command -v cargo >/dev/null 2>&1; then
    printf '%s\n' 'Using installed Rust; rust-toolchain.toml pins the project toolchain.'
    exit 0
fi
version=$(awk -F '"' '/^channel =/ { print $2; exit }' "$root/rust-toolchain.toml")
export CARGO_HOME="$root/.tools/cargo"
export RUSTUP_HOME="$root/.tools/rustup"
mkdir -p "$root/.tools"
curl --proto '=https' --tlsv1.2 -fsS https://sh.rustup.rs -o "$root/.tools/rustup-init.sh"
sh "$root/.tools/rustup-init.sh" -y --no-modify-path --profile minimal --default-toolchain "$version" --component rustfmt --component clippy
