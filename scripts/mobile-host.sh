#!/bin/sh
# Explicit opt-in to one LAN interface. Does not restart the existing daemon.
set -eu
mpac_phone_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
if [ "${1:-}" = --help ] || [ "${1:-}" = -h ]; then
    printf '%s\n' 'Usage: scripts/mobile-host.sh LOCAL_IPV4 [DAEMON_DATA_DIR] [PUBLIC_OPEN_PAGE]' 'Starts a separate HTTPS companion on port 4330 and opens private local phone controls.'
    exit 0
fi
if [ "$#" -lt 1 ] || [ "$#" -gt 3 ]; then
    printf '%s\n' 'Usage: scripts/mobile-host.sh LOCAL_IPV4 [DAEMON_DATA_DIR] [PUBLIC_OPEN_PAGE]' 'Example: scripts/mobile-host.sh 192.168.1.16' 'Starts a separate HTTPS companion on port 4330 and opens private local phone controls.'
    exit 2
fi
mpac_phone_interface=$1
python3 - "$mpac_phone_interface" <<'PY'
import ipaddress, sys
try:
    address = ipaddress.IPv4Address(sys.argv[1])
    assert not address.is_unspecified and (address.is_private or address.is_loopback or address.is_link_local)
except (ValueError, AssertionError):
    raise SystemExit('Pass one explicit local IPv4 interface address')
PY
mpac_phone_data=${2:-$mpac_phone_root/.mpac}
export CARGO_HOME="$mpac_phone_root/.tools/cargo"
export RUSTUP_HOME="$mpac_phone_root/.tools/rustup"
export CARGO_TARGET_DIR="$mpac_phone_root/target"
export PATH="$CARGO_HOME/bin:$PATH"
cd "$mpac_phone_root"
cargo build -p mpac-mobile-link --bin mpac-mobile-host --release --locked
if [ "$#" -eq 3 ]; then
    set -- --share-url "$3"
else
    set --
fi
exec "$mpac_phone_root/target/release/mpac-mobile-host" \
    --data-dir "$mpac_phone_data" --state-dir "$mpac_phone_root/.mpac/mobile-host" \
    --bind "$mpac_phone_interface:4330" --open-control "$@"
