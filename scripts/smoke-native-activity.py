#!/usr/bin/env python3
"""Real daemon/PTY/helper/HTTP smoke using owned fake CLIs; never calls a model."""
import argparse
import base64
import json
import os
from pathlib import Path
import signal
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
PRIVATE = "OWN_SMOKE_PRIVATE_CONTENT_95c24d"
NEXT_FIRE_MS = 4102444800000

FAKE_CLI = r'''
import json, os, shlex, stat, subprocess, sys, threading, time, tomllib, tty
from pathlib import Path
from urllib.request import Request, build_opener, ProxyHandler
http = build_opener(ProxyHandler({}))

provider = Path(sys.argv[0]).name
if "--version" in sys.argv:
    print("2.1.318 (Claude Code)" if provider == "claude" else "codex-cli 0.166.0")
    raise SystemExit(0)
parent = os.getppid()
def watchdog():
    for _ in range(120):
        time.sleep(1)
        if os.getppid() != parent: os._exit(0)
    os._exit(0)
threading.Thread(target=watchdog, daemon=True).start()
private = "OWN_SMOKE_PRIVATE_CONTENT_95c24d"
channel_path = Path(os.environ["MPAC_EVENT_CHANNEL"])
channel = json.loads(channel_path.read_text())
assert channel["provider"] == provider == os.environ["MPAC_EVENT_PROVIDER"]
assert stat.S_IMODE(channel_path.stat().st_mode) == 0o600
assert stat.S_IMODE(channel_path.parent.stat().st_mode) == 0o700
hooks = {}
if provider == "claude":
    settings_path = Path(sys.argv[sys.argv.index("--settings") + 1])
    assert stat.S_IMODE(settings_path.stat().st_mode) == 0o600
    hooks = json.loads(settings_path.read_text())["hooks"]
    start = hooks["SessionStart"][0]["hooks"][0]
    assert start["type"] == "command" and start["timeout"] == 1
    for name, definitions in hooks.items():
        for definition in definitions:
            for hook in definition["hooks"]:
                if hook["type"] == "http":
                    assert hook["url"] == f'http://127.0.0.1:{channel["port"]}/agent-events/{channel["session_id"]}'
                    assert hook["headers"]["x-mpac-event-token"] == channel["secret"]
                    assert hook["timeout"] == 1
else:
    values = {}
    for i, arg in enumerate(sys.argv[:-1]):
        if arg == "-c":
            key, value = sys.argv[i + 1].split("=", 1)
            values[key] = tomllib.loads("value=" + value)["value"]
    assert values["features.hooks"] is True
    assert values["tui.notifications"] == ["agent-turn-complete"]
    assert values["tui.notification_method"] == "osc9"
    assert values["tui.notification_condition"] == "always"
    assert "--dangerously-bypass-hook-trust" not in sys.argv
    hooks = {key[6:]: value for key, value in values.items() if key.startswith("hooks.")}
    for definitions in hooks.values():
        assert definitions[0]["hooks"][0]["timeout"] == 1
        assert definitions[0]["hooks"][0]["type"] == "command"
helper = shlex.split(hooks["SessionStart"][0]["hooks"][0]["command"])[0]
assert Path(helper).is_file()
assert channel["secret"] not in " ".join(sys.argv)
Path(os.environ["MPAC_SMOKE_OBSERVATIONS"]).joinpath(channel["session_id"] + ".json").write_text(json.dumps({
    "provider":provider,"generated_configuration_valid":True,"private_channel":True,
    "stable_helper":helper,"pid":os.getpid()
}))
turn = 0
def event(name, **extra):
    payload = {"hook_event_name":name,"session_id":provider + "-native-owned",
        "prompt_id":"prompt-" + str(turn),"turn_id":"turn-" + str(turn),
        "cwd":os.getcwd(),"transcript_path":"/never/read/private/transcript",
        "model":"fixture-no-inference","permission_mode":"default", **extra}
    for group in hooks[name]:
        for hook in group["hooks"]:
            if hook["type"] == "command":
                result = subprocess.run(shlex.split(hook["command"]), input=json.dumps(payload),
                    capture_output=True, text=True, timeout=4, check=True)
                assert result.stdout == ("{}" if provider == "codex" else ""), result.stdout
                assert result.stderr == ""
            else:
                request = Request(hook["url"], data=json.dumps(payload).encode(),
                    headers={**hook["headers"],"Content-Type":"application/json"})
                with http.open(request, timeout=4) as response: assert response.status == 204
def native(status):
    if provider == "claude": os.write(1, ("\x1b]7501;state=" + status + ":app=claude-code\x07").encode())
    elif status == "done": os.write(1, b"\x1b]9;Own fixture response ready\x07")
tty.setraw(0)
os.write(1, b"OWN_NATIVE_FIXTURE_READY\r\n")
buffer = b""
while True:
    chunk = os.read(0, 4096)
    if not chunk: break
    buffer += chunk
    while b"\n" in buffer:
        line, buffer = buffer.split(b"\n", 1)
        command = line.decode()
        if command == "session": event("SessionStart", source="startup")
        elif command == "work":
            turn += 1
            event("UserPromptSubmit", prompt=private)
            native("working")
        elif command == "tool":
            event("PreToolUse", tool_name="Read", tool_use_id="tool-owned", tool_input={"file_path":private})
        elif command == "tool-done":
            event("PostToolUse", tool_name="Read", tool_use_id="tool-owned", tool_input={"file_path":private}, tool_response={"text":private})
        elif command == "question":
            event("PreToolUse", tool_name="AskUserQuestion", tool_use_id="question-1", tool_input={"question":private})
            native("blocked")
        elif command == "answer":
            event("PostToolUse", tool_name="AskUserQuestion", tool_use_id="question-1", tool_input={"question":private}, tool_response={"answer":private})
            native("working")
        elif command == "permission":
            event("PreToolUse", tool_name="Bash", tool_use_id="permission-1", tool_input={"command":"fixture-only"})
            event("PermissionRequest", tool_name="Bash", tool_input={"command":"fixture-only"})
        elif command == "resolve":
            event("PostToolUse", tool_name="Bash", tool_use_id="permission-1", tool_input={"command":"fixture-only"}, tool_response={"text":private})
        elif command == "stop":
            event("Stop", last_assistant_message=private, stop_hook_active=False)
            native("done")
        elif command == "snapshot":
            event("Stop", last_assistant_message=private, background_tasks=[
                {"id":"background-owned","type":"local_agent","status":"running","description":"Owned fixture worker"}],
                session_crons=[{"id":"cron-owned","schedule":"*/5 * * * *","recurring":True,"prompt":private}])
        elif command == "cron":
            event("PostToolUse", tool_name="CronCreate", tool_use_id="cron-tool", tool_input={"prompt":private},
                tool_response={"id":"cron-owned","nextFireAt":4102444800000,"humanSchedule":"Every five minutes","prompt":private})
        elif command == "missing": event("Stop", last_assistant_message=private)
        elif command == "empty": event("Stop", background_tasks=[], session_crons=[], last_assistant_message=private)
        elif command == "osc": native("done")
        else: raise AssertionError("Unexpected own fixture command")
        os.write(1, ("OWN_ACK:" + command + "\r\n").encode())
'''


def wait_for(check, label, seconds=12):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.04)
    raise AssertionError("timeout: " + label)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/mpac-daemon")
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    checks = []
    with tempfile.TemporaryDirectory(prefix="mpac-native-activity-") as folder:
        workspace = Path(folder)
        data, bins, observations = (workspace / name for name in ("state", "bin", "observations"))
        bins.mkdir(); observations.mkdir()
        for provider in ("claude", "codex"):
            path = bins / provider
            path.write_text("#!" + sys.executable + "\n" + FAKE_CLI)
            path.chmod(0o700)
        environment = {**os.environ, "PATH": str(bins) + os.pathsep + os.environ.get("PATH", ""),
            "MPAC_SMOKE_OBSERVATIONS": str(observations), "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1", "GIT_ATTR_NOSYSTEM": "1"}
        daemon = None
        with (workspace / "daemon.log").open("wb") as log:
            try:
                daemon = subprocess.Popen([str(binary), "--port", "0", "--data-dir", str(data)],
                    cwd=ROOT, env=environment, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
                launch = data / "launch.json"
                wait_for(lambda: launch.exists() or daemon.poll() is not None, "owned daemon startup")
                assert daemon.poll() is None, "owned daemon exited before startup"
                parsed = urllib.parse.urlsplit(json.loads(launch.read_text())["url"])
                base = "http://" + parsed.netloc
                token = urllib.parse.parse_qs(parsed.fragment)["token"][0]

                http = urllib.request.build_opener(urllib.request.ProxyHandler({}))

                def request(path, body=None, expected=200):
                    headers = {"Authorization":"Bearer " + token,"Content-Type":"application/json"}
                    req = urllib.request.Request(base + "/api" + path,
                        data=None if body is None else json.dumps(body).encode(), headers=headers)
                    try:
                        with http.open(req, timeout=8) as response:
                            status, raw = response.status, response.read()
                    except urllib.error.HTTPError as error:
                        status, raw = error.code, error.read()
                    result = json.loads(raw) if raw else None
                    assert status == expected, (path, status, result)
                    return result

                project = request("/state")["projects"][0]
                sessions = {}
                for provider in ("claude", "codex"):
                    session = request("/sessions", {"project_id":project["id"],"root_id":project["roots"][0]["id"],
                        "agent":provider,"title":"Owned native " + provider})
                    sessions[provider] = session["id"]

                def current(provider):
                    return next(session for session in request("/state")["sessions"] if session["id"] == sessions[provider])

                def output(provider):
                    value = request("/sessions/" + sessions[provider] + "/output?after=0")
                    return base64.b64decode(value["data_base64"]).decode(errors="replace")

                def command(provider, value):
                    marker = "OWN_ACK:" + value
                    previous = output(provider).count(marker)
                    request("/sessions/" + sessions[provider] + "/input", {"data":value + "\n"})
                    wait_for(lambda: output(provider).count(marker) > previous, provider + " " + value)
                    return current(provider)["activity"]

                for provider in sessions:
                    try:
                        wait_for(lambda: "OWN_NATIVE_FIXTURE_READY" in output(provider), provider + " ready")
                    except AssertionError:
                        raise AssertionError(provider + " fixture startup: " + repr(output(provider)[-1600:])) from None
                    assert current(provider)["activity"]["adapter"] == provider + "_hooks"
                    command(provider, "session")
                    observed = json.loads((observations / (sessions[provider] + ".json")).read_text())
                    assert observed["generated_configuration_valid"] and observed["private_channel"]
                checks.append("generated HTTP headers, launch-local TOML, private settings/channel, real stable helper")

                for provider in sessions:
                    command(provider, "work")
                    activity = command(provider, "tool")
                    calls = activity["tools"]
                    assert len(calls) == 1 and calls[0]["name"] == "Read"
                    assert set(calls[0]) == {"id", "name", "started_at"}
                    assert activity["tools_observed_at"] is not None
                    assert command(provider, "tool-done")["tools"] == []
                checks.append("authenticated Claude/Codex tool observations exposed as bounded metadata and removed after PostToolUse")

                activity = command("claude", "work")
                assert activity["phase"] == "working"
                activity = command("claude", "question")
                assert activity["attention"]["kind"] == "question"
                assert activity["attention"]["certainty"] == "confirmed"
                activity = command("claude", "stop")
                assert activity["attention"]["kind"] == "question", "Stop must not clear pending question"
                revision = activity["result"]["revision"]
                assert revision == 1
                assert command("claude", "answer")["attention"] is None
                command("claude", "work"); activity = command("claude", "stop")
                assert activity["result"]["revision"] == revision + 1
                endpoint = "/sessions/" + sessions["claude"] + "/activity/read"
                request(endpoint, {"expected_revision":revision}, expected=400)
                assert current("claude")["activity"]["result"]["read_revision"] == 0
                request(endpoint, {"expected_revision":revision + 1})
                assert current("claude")["activity"]["result"]["read_revision"] == revision + 1
                checks.append("Claude native question/answer, Stop preservation, result revision and read CAS")

                command("claude", "work")
                activity = command("claude", "snapshot")
                assert activity["background"]["coverage"] == "snapshot"
                assert len(activity["background"]["jobs"]) == 2
                assert any(job["status"] == "running" for job in activity["background"]["jobs"])
                activity = command("claude", "cron")
                cron = next(job for job in activity["background"]["jobs"] if job["id"] == "cron-owned")
                assert cron["next_run_at"] == NEXT_FIRE_MS // 1000
                assert command("claude", "missing")["background"]["coverage"] == "unknown"
                activity = command("claude", "empty")
                assert activity["background"]["coverage"] == "snapshot" and activity["background"]["jobs"] == []
                checks.append("background missing vs explicit empty; native running job and CronCreate nextFireAt")

                assert command("codex", "osc")["result"]["revision"] == 0
                assert command("codex", "work")["phase"] == "working"
                activity = command("codex", "permission")
                assert activity["attention"]["kind"] == "permission"
                assert command("codex", "resolve")["attention"] is None
                activity = command("codex", "stop")
                assert activity["result"]["revision"] == 1 and activity["phase"] == "idle"
                assert command("codex", "osc")["result"]["revision"] == 1
                checks.append("Codex real helper + native OSC9 two-signal completion; requested permission; dedup")

                for provider, session_id in sessions.items():
                    saved = request("/sessions/" + session_id + "/answer")["answer"]
                    assert saved["text"] == PRIVATE and saved["saved_at"] > 0, provider
                checks.append("latest assistant text delivered by Claude HTTP hook and Codex command hook")

                public = json.dumps(request("/state"))
                assert PRIVATE not in public and token not in public
                with sqlite3.connect(data / "state.sqlite3") as db:
                    rows = db.execute("SELECT payload FROM sessions").fetchall()
                    assert rows and all(PRIVATE not in row[0] for row in rows)
                for provider in sessions:
                    channel = data / "agent-events" / sessions[provider] / "channel.json"
                    secret = json.loads(channel.read_text())["secret"]
                    assert secret not in public
                    assert stat.S_IMODE(channel.stat().st_mode) == 0o600
                    assert stat.S_IMODE(channel.parent.stat().st_mode) == 0o700
                    assert secret not in " ".join(current(provider)["permission_args"])
                checks.append("raw prompt/response/cron prompt absent from public catalog and SQLite session payloads")
            finally:
                if daemon and daemon.poll() is None:
                    daemon.send_signal(signal.SIGTERM)
                    try:
                        daemon.wait(timeout=12)
                    except subprocess.TimeoutExpired:
                        daemon.kill(); daemon.wait(timeout=3)
                        raise AssertionError("owned daemon failed graceful shutdown")
    report = {"passed":True,"checks":checks,"models_called":False,"user_daemon_touched":False}
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(report, ensure_ascii=False))


if __name__ == "__main__":
    main()
