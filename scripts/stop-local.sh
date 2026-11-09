#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
python3 - "$root/.mpac/launch.json" <<'PY'
import json
import os
from pathlib import Path
import signal
import subprocess
import sys

launch = Path(sys.argv[1])
if not launch.exists():
    print('Локальный daemon не запущен.')
    raise SystemExit(0)
pid = json.loads(launch.read_text())['pid']
probe = subprocess.run(['ps', '-o', 'comm=', '-p', str(pid)], capture_output=True, text=True)
if probe.returncode != 0:
    print('Процесс daemon уже завершён.')
    raise SystemExit(0)
if Path(probe.stdout.strip()).name != 'mpac-daemon':
    raise SystemExit('PID больше не принадлежит mpac-daemon; остановка отменена.')
os.kill(pid, signal.SIGTERM)
print('Остановка daemon и его сессий запрошена.')
PY
