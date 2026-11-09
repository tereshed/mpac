#!/bin/sh
# Local build + IPA export only. No GitHub Actions and no upload.
set -eu
mpac_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mpac_build=${1:-}
case "$mpac_build" in ''|*[!0-9]*) printf '%s\n' 'Usage: scripts/build-testflight.sh BUILD_NUMBER' >&2; exit 2;; esac
[ "$#" -eq 1 ] || exit 2
[ "$(uname -s)" = Darwin ] || exit 1
export CARGO_HOME="$mpac_root/.tools/cargo"
export RUSTUP_HOME="$mpac_root/.tools/rustup"
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_TARGET_DIR="$mpac_root/target"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export APPLE_DEVELOPMENT_TEAM="${APPLE_DEVELOPMENT_TEAM:?Set your Apple Developer Team ID}"
mpac_version=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$mpac_root/mobile/src-tauri/tauri.conf.json")
mpac_stamp=$(date -u +%Y%m%dT%H%M%SZ)
# Keep signing/export outside Documents and its File Provider metadata.
mpac_output="$HOME/Library/Developer/MPAC/releases/$mpac_version-$mpac_build-$mpac_stamp"
mkdir -p "$mpac_output"
npm --prefix "$mpac_root/ui" run build
python3 "$mpac_root/scripts/prepare-mobile.py" ios
cd "$mpac_root/mobile"
mpac_ios_config=$(python3 - "$mpac_build" <<'PYCONFIG'
import json, os, re, sys
team = os.environ["APPLE_DEVELOPMENT_TEAM"]
if not re.fullmatch(r"[A-Z0-9]{10}", team):
    raise SystemExit("Set APPLE_DEVELOPMENT_TEAM to a valid Apple Team ID")
print(json.dumps({"bundle": {"iOS": {"developmentTeam": team, "bundleVersion": sys.argv[1]}}}))
PYCONFIG
)
../desktop/node_modules/.bin/tauri ios build --target aarch64 --ci \
  --config src-tauri/tauri.testflight.conf.json \
  --config "$mpac_ios_config" \
  --export-method app-store-connect --archive-only
python3 - "$mpac_root" "$mpac_output" <<'PY'
from pathlib import Path
import shutil,sys,subprocess,plistlib,os
root,output=map(Path,sys.argv[1:]);source=root/'mobile/src-tauri/gen/apple/build/mpac-mobile_iOS.xcarchive'
# Copy bytes and modes, without generated Finder/File Provider metadata.
archive=output/'MPAC-Mobile.xcarchive'
shutil.copytree(source,archive,copy_function=shutil.copy)
app=archive/'Products/Applications/MPAC Mobile.app'
subprocess.run(['codesign','--verify','--deep','--strict',str(app)],check=True)
# A successful archive is not enough: a first plugin build can update the
# source entitlements too late for Xcode. Refuse an IPA without signed links.
actual=plistlib.loads(subprocess.check_output(['codesign','-d','--entitlements',':-',str(app)],stderr=subprocess.DEVNULL))
expected=plistlib.loads((root/'mobile/src-tauri/gen/apple/mpac-mobile_iOS/mpac-mobile_iOS.entitlements').read_bytes()).get('com.apple.developer.associated-domains',[])
if expected and actual.get('com.apple.developer.associated-domains') != expected:
    raise SystemExit('Archive lacks the required signed Associated Domains entitlement')
with (root/'mobile/src-tauri/ExportOptions.testflight.plist').open('rb') as f:options=plistlib.load(f)
options['teamID']=os.environ['APPLE_DEVELOPMENT_TEAM']
with (output/'ExportOptions.plist').open('wb') as f:plistlib.dump(options,f)
PY
/usr/bin/xcodebuild -exportArchive -archivePath "$mpac_output/MPAC-Mobile.xcarchive" \
  -exportPath "$mpac_output/export" -exportOptionsPlist "$mpac_output/ExportOptions.plist" \
  -allowProvisioningUpdates
printf '%s\n' "Local archive and IPA: $mpac_output" 'Upload is a separate step: scripts/upload-testflight.sh ARCHIVE_PATH'
