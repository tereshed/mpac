#!/bin/sh
# Builds Linux x86_64 locally. Only source files enter the Docker context.
set -eu
mpac_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mpac_build_dir=$(mktemp -d "${TMPDIR:-/tmp}/mpac-linux-build.XXXXXX")
mpac_container=
trap 'if [ -n "$mpac_container" ]; then docker rm "$mpac_container" >/dev/null; fi; rm -rf "$mpac_build_dir"' EXIT HUP INT TERM
python3 - "$mpac_root" "$mpac_build_dir" <<'PY'
from pathlib import Path
import shutil, sys
root, out = map(Path, sys.argv[1:])
for name in ['Cargo.toml', 'Cargo.lock', 'crates', 'vendor/tao',
             'desktop/src-tauri/Cargo.toml', 'desktop/src-tauri/src',
             'mobile/src-tauri/Cargo.toml', 'mobile/src-tauri/src']:
    src, dst = root / name, out / name
    dst.parent.mkdir(parents=True, exist_ok=True)
    if src.is_dir():
        shutil.copytree(src, dst, ignore=shutil.ignore_patterns('target', '__pycache__', '.DS_Store'))
    else:
        shutil.copyfile(src, dst)
shutil.copyfile(root / 'deploy/linux/Dockerfile', out / 'Dockerfile')
PY
docker build --platform linux/amd64 -t mpac-linux-host:local "$mpac_build_dir"
mpac_container=$(docker create mpac-linux-host:local /mpac-daemon)
mkdir -p "$mpac_root/.mpac/linux-build"
docker cp "$mpac_container:/mpac-daemon" "$mpac_root/.mpac/linux-build/mpac-daemon"
docker cp "$mpac_container:/mpac-mobile-host" "$mpac_root/.mpac/linux-build/mpac-mobile-host"
printf '%s\n' 'Linux binaries: .mpac/linux-build (requires Linux glibc 2.36 or newer).'
