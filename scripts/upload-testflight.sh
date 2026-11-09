#!/bin/sh
# Explicit upload of an existing local archive using the Xcode Apple account.
set -eu
mpac_archive=${1:-}
[ "$#" -eq 1 ] && [ -d "$mpac_archive" ] || { printf '%s\n' 'Usage: scripts/upload-testflight.sh ARCHIVE_PATH' >&2; exit 2; }
mpac_archive=$(CDPATH= cd -- "$mpac_archive" && pwd)
mpac_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mpac_options=$(mktemp -t mpac-upload)
trap 'rm -f "$mpac_options"' EXIT
python3 - "$mpac_root/mobile/src-tauri/ExportOptions.testflight.plist" "$mpac_options" "$mpac_archive/Info.plist" <<'PY'
import plistlib,sys
with open(sys.argv[1],'rb') as f:options=plistlib.load(f)
options['destination']='upload'
with open(sys.argv[3],'rb') as f:archive=plistlib.load(f)
options['teamID']=archive['ApplicationProperties']['Team']
with open(sys.argv[2],'wb') as f:plistlib.dump(options,f)
PY
/usr/bin/xcodebuild -exportArchive -archivePath "$mpac_archive" \
  -exportOptionsPlist "$mpac_options" -allowProvisioningUpdates
