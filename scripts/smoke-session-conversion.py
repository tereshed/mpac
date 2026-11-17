#!/usr/bin/env python3
"""Round-trip a synthetic transcript through native Claude and Codex pickers."""
import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
from threading import Thread
from queue import Queue
import time
import uuid

import pexpect

MESSAGES = [
    ("user", "MPAC QUESTION ALPHA"),
    ("assistant", "MPAC ANSWER MAUVE"),
    ("user", "MPAC QUESTION BETA"),
    ("assistant", "MPAC ANSWER PUFFIN"),
]


def write_claude_fixture(path, cwd):
    parent = None
    session = str(uuid.uuid4())
    stamp = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="milliseconds")
    stamp = stamp.replace("+00:00", "Z")
    with path.open("w") as out:
        for role, text in MESSAGES + [("user", "MPAC INCOMPLETE TURN")]:
            entry = str(uuid.uuid4())
            message = {"role": role, "content": text}
            if role == "assistant":
                message["content"] = [{"type": "text", "text": text}]
                message["stop_reason"] = "end_turn"
            row = {
                "type": role, "message": message, "uuid": entry, "parentUuid": parent,
                "sessionId": session, "cwd": str(cwd), "timestamp": stamp, "isSidechain": False,
            }
            out.write(json.dumps(row) + "\n")
            parent = entry


def prepare(profile, cwd, settings):
    profile.mkdir()
    (profile / ".mpac-conversion-test-profile").touch()
    (profile / ".claude.json").write_text(json.dumps({
        "hasCompletedOnboarding": True,
        "customApiKeyResponses": {"approved": ["test-fixture"]},
        "projects": {str(cwd): {
            "hasTrustDialogAccepted": True, "hasCompletedProjectOnboarding": True,
        }},
    }))
    (profile / "auth.json").write_text(json.dumps({
        "auth_mode": "apikey", "OPENAI_API_KEY": "test-fixture",
    }))
    (profile / "config.toml").write_text(
        'model_provider = "mpac_fixture"\n'
        '[model_providers.mpac_fixture]\nname = "MPAC fixture"\n'
        'base_url = "http://127.0.0.1:1/v1"\nwire_api = "responses"\n'
        f'[projects.{json.dumps(str(cwd))}]\ntrust_level = "trusted"\n'
        '[analytics]\nenabled = false\n[features]\ndaemon_auto_start = false\n'
    )
    return dict(settings, HOME=str(profile), CLAUDE_CONFIG_DIR=str(profile), CODEX_HOME=str(profile))


def exercise_picker(env, cwd, harness, title, resume_args):
    executable = shutil.which(harness)
    with pexpect.spawn(executable, resume_args, cwd=str(cwd), env=env,
                      dimensions=(45, 135), encoding="utf-8", timeout=12) as terminal:
        terminal.expect_exact(title)
        if resume_args in (["resume"], ["--resume"]):
            terminal.send("\r")
        for token in ("ALPHA", "MAUVE", "BETA", "PUFFIN"):
            terminal.expect_exact(token)
        if len(resume_args) == 2:
            terminal.send("/resume")
            terminal.send("\r")
            terminal.expect_exact("Resume session" if harness == "claude" else "sort/filter")
        terminal.close(force=True)


def verify_follow_up(env, cwd, harness, title, resume_args):
    requests = Queue()

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            if self.path.split("?")[0].endswith(("/messages", "/responses")):
                data = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                requests.put(data)
            self.send_error(400, "End of local history test")

        def log_message(self, *_):
            pass

    with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        listener = Thread(target=server.serve_forever, daemon=True)
        listener.start()
        try:
            url = f"http://127.0.0.1:{server.server_port}"
            env = dict(env, ANTHROPIC_BASE_URL=url, OPENAI_BASE_URL=f"{url}/v1")
            if harness == "codex":
                resume_args = [*resume_args, "-c", f'model_providers.mpac_fixture.base_url="{url}/v1"']
            with pexpect.spawn(shutil.which(harness), resume_args, cwd=str(cwd), env=env,
                              dimensions=(45, 135), encoding="utf-8", timeout=12) as terminal:
                terminal.expect_exact(title)
                terminal.expect_exact("PUFFIN")
                terminal.send("Which fixture color did we discuss first?")
                terminal.send("\r")
                output = ""
                deadline = time.monotonic() + 12
                while requests.empty() and time.monotonic() < deadline:
                    try:
                        output += terminal.read_nonblocking(4096, timeout=0.1)
                    except pexpect.TIMEOUT:
                        continue
                if requests.empty():
                    plain = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", output)
                    raise AssertionError(f"{harness} made no local inference request: {plain[-1000:]}")
                payload = requests.get_nowait()
                history = payload.get("input", payload.get("messages", []))
                seen = []
                for item in history:
                    serialized = json.dumps(item.get("content"))
                    for role, text in MESSAGES:
                        if text in serialized:
                            seen.append((item.get("role"), text))
                assert seen == MESSAGES, (harness, seen)
        finally:
            server.shutdown()
            listener.join()


def main(binary):
    with tempfile.TemporaryDirectory(prefix="mpac-convert-smoke-") as directory:
        root = Path(directory).resolve()
        cwd = root / "project"
        cwd.mkdir()
        # Supply fixture credentials only. Loopback targets prevent model inference.
        settings = {
            "PATH": os.environ["PATH"], "LANG": "en_US.UTF-8", "TERM": "xterm-256color",
            "ANTHROPIC_API_KEY": "test-fixture", "ANTHROPIC_BASE_URL": "http://127.0.0.1:1",
            "OPENAI_BASE_URL": "http://127.0.0.1:1", "DISABLE_TELEMETRY": "1",
            "DISABLE_ERROR_REPORTING": "1", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
        }
        source = root / "source.jsonl"
        write_claude_fixture(source, cwd)
        initial = source.read_bytes()
        codex = root / "codex"
        claude = root / "claude"
        codex_env = prepare(codex, cwd, settings)
        claude_env = prepare(claude, cwd, settings)

        def convert(source_kind, transcript, target, profile, title):
            result = subprocess.run(
                [binary, source_kind, str(transcript), target, str(profile), str(cwd), title],
                check=True, capture_output=True, text=True, timeout=30,
                env={"PATH": settings["PATH"]},
            )
            return json.loads(result.stdout)

        codex_title = "MPAC Imported Claude"
        imported = convert("claude", source, "codex", codex, codex_title)
        assert imported["messages"] == 4 and imported["incomplete_messages"] == 1, imported
        assert source.read_bytes() == initial
        exercise_picker(codex_env, cwd, "codex", codex_title, ["resume"])
        exercise_picker(codex_env, cwd, "codex", codex_title, ["resume", imported["id"]])
        [codex_path] = codex.glob(f'sessions/**/rollout-*-{imported["id"]}.jsonl')
        codex_bytes = codex_path.read_bytes()

        claude_title = "MPAC Imported Codex"
        converted = convert("codex", codex_path, "claude", claude, claude_title)
        assert converted["messages"] == 4, converted
        assert codex_path.read_bytes() == codex_bytes
        exercise_picker(claude_env, cwd, "claude", claude_title, ["--resume"])
        exercise_picker(claude_env, cwd, "claude", claude_title, ["--resume", converted["id"]])
        verify_follow_up(codex_env, cwd, "codex", codex_title, ["resume", imported["id"]])
        verify_follow_up(claude_env, cwd, "claude", claude_title, ["--resume", converted["id"]])
        assert not any(codex.glob(".mpac-conversion-*/*"))
        assert not any(claude.glob(".mpac-conversion-*/*"))
        print("Claude → Codex → Claude: native pickers, exact-ID resumes and local request history passed")


if __name__ == "__main__":
    main(str(Path(sys.argv[1]).resolve()))
