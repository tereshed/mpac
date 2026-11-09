#!/usr/bin/env python3
"""Launch presets and native title signals through owned PTYs; no inference."""
import base64
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def wait_for(check, label):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.03)
    raise AssertionError(label)


def main():
    with tempfile.TemporaryDirectory(prefix="mpac-launch-smoke-") as folder:
        data = Path(folder) / "state"
        bins = Path(folder) / "bin"
        bins.mkdir()
        fixture = f"#!{sys.executable}\n" + r'''
import os, sys, tty
tty.setraw(sys.stdin.fileno())
def emit(s):
 os.write(1, s.encode())
emit('\x1b]7501;?\x1b\\')
emit('FIXTURE_READY\n')
pending = b''
while True:
 chunk = os.read(0, 4096)
 if not chunk: break
 pending += chunk
 pending = pending.replace(b'\x1b]7501;?\x1b\\', b'')
 pending = pending.replace(b'\r', b'\n')
 while b'\n' in pending:
  line, pending = pending.split(b'\n', 1)
  line = line.decode()
  if line.startswith('title:'): emit('\x1b]2;' + line[6:] + '\x07')
  elif line == 'codexdone': emit('\x1b]9;Fixture response completed\x07')
  elif line == 'argv': emit('ARGV=' + repr(sys.argv[1:]) + '\n')
  else: emit('\x1b]7501;state=' + line + ':app=claude-code\x07')
  emit('ACK=' + line + '\n')
'''
        for agent in ["claude", "codex"]:
            path = bins / agent
            path.write_text(fixture)
            path.chmod(0o700)
        daemon = None
        launch = data / "launch.json"

        def boot():
            nonlocal daemon, base, token
            launch.unlink(missing_ok=True)
            daemon = subprocess.Popen(
                [str(ROOT / "target/debug/mpac-daemon"), "--port", "0", "--data-dir", str(data)],
                cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                env={**os.environ, "PATH": str(bins) + os.pathsep + os.environ.get("PATH", "")},
            )
            wait_for(launch.exists, "daemon launch")
            config = urllib.parse.urlsplit(json.loads(launch.read_text())["url"])
            base = "http://" + config.netloc
            token = urllib.parse.parse_qs(config.fragment)["token"][0]

        def shutdown():
            if daemon and daemon.poll() is None:
                daemon.send_signal(signal.SIGTERM)
                daemon.wait(timeout=10)

        def api(path, body=None, expected=200):
            request = urllib.request.Request(base + "/api" + path,
                data=None if body is None else json.dumps(body).encode(),
                headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
            try:
                with urllib.request.urlopen(request, timeout=10) as response:
                    status, result = response.status, json.load(response)
            except urllib.error.HTTPError as error:
                status, result = error.code, json.load(error)
            assert status == expected, (path, status, result)
            return result

        base = token = ""
        boot()
        try:
            state = api("/state")
            assert state["capabilities"]["auto_session_titles"] is True
            project = state["projects"][0]
            first, second = project["roots"]
            defaults = state["launch_defaults"]

            def save(**body):
                nonlocal defaults
                defaults = api("/launch-defaults", {"expected_revision": defaults["revision"], **body})
                return defaults

            save(scope="agent", agent_id="claude", access={"mode": "plan"}, make_default_agent=True)
            save(scope="project", project_id=project["id"], root_id=first["id"], agent_id="claude",
                 access={"mode": "manual", "additional_root_ids": [second["id"]]}, make_default_agent=True)
            api("/launch-defaults", {"scope": "agent", "agent_id": "claude", "access": None, "expected_revision": 0}, expected=400)

            def start(agent="claude", **extra):
                session = api("/sessions", {"project_id": project["id"], "root_id": first["id"], "agent": agent, **extra})
                wait_for(lambda: b"FIXTURE_READY" in output(session), "fixture ready")
                return session

            def current(session):
                return next(s for s in api("/state")["sessions"] if s["id"] == session["id"])

            def output(session):
                return base64.b64decode(api(f"/sessions/{session['id']}/output?after=0")["data_base64"])

            def send(session, line):
                after = api(f"/sessions/{session['id']}/output?after=0")["seq"]
                api(f"/sessions/{session['id']}/input", {"data": line + "\n"})
                wait_for(lambda: ("ACK=" + line).encode() in base64.b64decode(api(f"/sessions/{session['id']}/output?after={after}")["data_base64"]), "fixture consumed input")

            unnamed = start()
            assert unnamed["access"]["mode"] == "manual"
            assert unnamed["additional_roots"][0]["id"] == second["id"]
            assert unnamed["title_source"] == "pending_first_answer"
            send(unnamed, "title:Разбор авторизации")
            send(unnamed, "done")  # Startup snapshot without work is insufficient.
            assert current(unnamed)["title"] == "claude"
            send(unnamed, "working")
            send(unnamed, "done")
            assert current(unnamed)["title"] == "Разбор авторизации", (current(unnamed), output(unnamed).decode(errors="replace"))
            assert current(unnamed)["title_source"] == "harness"
            send(unnamed, "title:Later name")
            assert current(unnamed)["title"] == "Разбор авторизации"

            manual = start(title="Моё имя", access={"mode": "inherit"})
            assert manual["access"]["mode"] == "inherit"
            for line in ["working", "title:Harness name", "done"]:
                send(manual, line)
            assert current(manual)["title"] == "Моё имя"
            same_name = start()
            api(f"/sessions/{same_name['id']}/rename", {"title": "claude", "expected_title": "claude"})
            for line in ["working", "title:Should not win", "done"]:
                send(same_name, line)
            assert current(same_name)["title_source"] == "manual"
            assert current(same_name)["title"] == "claude"

            codex = start("codex")
            send(codex, "argv")
            assert b'notification_condition="always"' in output(codex)
            send(codex, "title:Codex generated name")
            assert current(codex)["title"] == "codex"
            send(codex, "codexdone")
            assert current(codex)["title"] == "Codex generated name"

            save(scope="project", project_id=project["id"], agent_id="claude", access=None)
            inherited = start()
            assert inherited["access"]["mode"] == "plan"
            save(scope="agent", agent_id="codex", access={"mode": "full_access", "approval": "on_request", "acknowledge_full_access": True})
            assert not defaults["agents"]["codex"]["acknowledge_full_access"]
            api("/sessions", {"project_id": project["id"], "root_id": first["id"], "agent": "codex"}, expected=400)
            revision = defaults["revision"]
            shutdown()
            boot()
            assert api("/state")["launch_defaults"]["revision"] == revision
            assert current(unnamed)["title"] == "Разбор авторизации"
            assert current(unnamed)["title_source"] == "harness"
            assert not any(s["status"] in ("running", "needs_attention") for s in api("/state")["sessions"])
            print(json.dumps({"result": "passed", "checks": ["global_and_project_presets", "CAS", "project_over_global_over_CLI", "explicit_launch_override", "startup_not_completed", "native_claude_completion", "auto_name_once", "manual_name_priority", "same_name_manual_cancels_auto", "codex_native_profile_and_completion", "full_access_requires_fresh_ack", "presets_and_names_survive_restart", "no_CLI_replay"], "inference_requested": False}))
        finally:
            shutdown()


if __name__ == "__main__":
    main()
