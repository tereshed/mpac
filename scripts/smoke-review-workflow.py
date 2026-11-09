#!/usr/bin/env python3
"""Real HTTP/PTY review and commit workflow using owned files and fake CLI only."""
import base64
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]


def wait_for(check, label):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(.03)
    raise AssertionError(label)


def main():
    with tempfile.TemporaryDirectory(prefix="mpac-review-workflow-") as folder:
        data = Path(folder) / "state"
        bins = Path(folder) / "bin"
        bins.mkdir()
        cli = bins / "claude"
        cli.write_text(f"#!{sys.executable}\n" + """
import base64, os, tty
tty.setraw(0)
os.write(1, b'FIXTURE_READY\\r\\n')
while True:
 value = os.read(0, 4096)
 if not value: break
 os.write(1, b'INPUT=' + base64.b64encode(value) + b'\\r\\n')
""")
        cli.chmod(0o700)
        launch = data / "launch.json"
        daemon = None
        base = token = ""

        def boot():
            nonlocal daemon, base, token
            launch.unlink(missing_ok=True)
            daemon = subprocess.Popen(
                [str(ROOT / "target/debug/mpac-daemon"), "--port", "0", "--data-dir", str(data)],
                cwd=ROOT, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                env={**os.environ, "PATH": str(bins) + os.pathsep + os.environ.get("PATH", "")},
            )
            wait_for(launch.exists, "daemon startup")
            url = urllib.parse.urlsplit(json.loads(launch.read_text())["url"])
            base = "http://" + url.netloc
            token = urllib.parse.parse_qs(url.fragment)["token"][0]

        def stop():
            if daemon and daemon.poll() is None:
                daemon.send_signal(signal.SIGTERM)
                daemon.wait(timeout=12)

        def api(path, body=None, expected=200, authorized=True):
            headers = {"Authorization": "Bearer " + token} if authorized else {}
            headers["Content-Type"] = "application/json"
            request = urllib.request.Request(base + "/api" + path,
                data=None if body is None else json.dumps(body).encode(), headers=headers)
            try:
                with urllib.request.urlopen(request, timeout=15) as response:
                    status, result = response.status, json.load(response)
            except urllib.error.HTTPError as error:
                status, result = error.code, json.load(error)
            assert status == expected, (path, status, result)
            return result

        def git(root, *args):
            output = subprocess.run(["git", *args], cwd=root, check=True, capture_output=True,
                env={**os.environ, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull})
            return output.stdout.decode().strip()

        boot()
        try:
            state = api("/state")
            assert state["capabilities"]["review_comments"] and state["capabilities"]["git_commits"]
            project = state["projects"][0]
            first, second = project["roots"]
            source = Path(first["path"]) / "sample.py"
            for root in [first, second]:
                git(root["path"], "config", "user.name", "MPAC workflow fixture")
                git(root["path"], "config", "user.email", "fixture@localhost")
                git(root["path"], "config", "commit.gpgSign", "false")
                git(root["path"], "config", "core.hooksPath", os.devnull)
            source.write_text("def title(width):\n    return width\n")
            git(first["path"], "add", "--", "sample.py")
            git(first["path"], "commit", "-m", "Own baseline source")
            source.write_text("def title(width):\n    minimum = 24\n    return max(width, minimum)\n")
            source_query = urllib.parse.urlencode({"root_id": first["id"], "path": "sample.py"})
            review_query = urllib.parse.urlencode({"root_id": first["id"]})
            view = api(f"/projects/{project['id']}/review-lines?{review_query}")
            block = next(b for b in view["blocks"] if b["path"] == "sample.py")
            new_rows = [line for line in block["lines"] if line["kind"] == "addition"]
            assert len(new_rows) == 2
            comment_body = {
                "root_id": first["id"], "path": "sample.py", "source": "diff", "snapshot": view["snapshot"],
                "anchor": {"kind": "range", "block_id": block["id"], "start_row": new_rows[0]["index"],
                           "end_row": new_rows[-1]["index"], "side": "new"},
                "text": "Сохранить минимальный размер во всём выделенном диапазоне.",
            }
            api(f"/projects/{project['id']}/comments", comment_body, expected=401, authorized=False)
            comment = api(f"/projects/{project['id']}/comments", comment_body)
            assert comment["new_start"] == 2 and comment["new_end"] == 3
            assert "    minimum = 24" in comment["excerpt"] and "    return max(width, minimum)" in comment["excerpt"]
            api(f"/review-comments/{comment['id']}/update", {"expected_revision": 0, "text": "Stale edit"}, expected=400)
            comment = api(f"/review-comments/{comment['id']}/update", {"expected_revision": comment["revision"], "text": "Проверить все выбранные строки, включая отступы."})
            extra_query = urllib.parse.urlencode({"root_id": second["id"], "path": "README.md"})
            extra_source = api(f"/projects/{project['id']}/source-lines?{extra_query}")
            extra_comment = api(f"/projects/{project['id']}/comments", {
                "root_id": second["id"], "path": "README.md", "source": "file",
                "snapshot": extra_source["snapshot"], "anchor": {"kind": "file"}, "text": "Проверить документацию.",
            })
            def start(grant=False):
                session = api("/sessions", {
                    "project_id": project["id"], "root_id": first["id"], "agent": "claude", "title": "Own workflow fixture",
                    "access": {"mode": "manual", "additional_root_ids": [second["id"]] if grant else []},
                })
                wait_for(lambda: b"FIXTURE_READY" in output(session), "owned CLI ready")
                return session

            def output(session):
                return base64.b64decode(api(f"/sessions/{session['id']}/output?after=0")["data_base64"])

            def received(session):
                return b"".join(base64.b64decode(value) for value in re.findall(rb"INPUT=([A-Za-z0-9+/=]+)", output(session)))

            limited = start()
            refs = [{"id": c["id"], "revision": c["revision"]} for c in [comment, extra_comment]]
            api(f"/projects/{project['id']}/review-batches", {
                "session_id": limited["id"], "comments": refs, "message": "Cross-root denied", "request_id": str(uuid.uuid4()),
            }, expected=400)
            session = start(True)
            prepare = {"session_id": session["id"], "comments": refs, "message": "Исправь только эти замечания.", "request_id": str(uuid.uuid4())}
            batch = api(f"/projects/{project['id']}/review-batches", prepare)
            assert batch["status"] == "draft" and not received(session)
            assert "2–3" in batch["text"] and "    minimum = 24" in batch["text"]
            assert api(f"/projects/{project['id']}/review-batches", prepare)["id"] == batch["id"]
            api(f"/review-batches/{batch['id']}/confirm", {"confirmed": True}, expected=400)
            inserted = api(f"/review-batches/{batch['id']}/insert", {})
            assert inserted["status"] == "prepared"
            packet_path = Path(inserted["attachment_path"])
            assert packet_path.is_relative_to(Path(first["path"]) / ".mpac-attachments")
            assert packet_path.read_text() == batch["text"]
            wait_for(lambda: received(session), "packet path draft received")
            before_input = received(session)
            assert b"\n" not in before_input and b"\r" not in before_input and b"\x1b" not in before_input
            assert str(packet_path).encode() in before_input
            assert api(f"/review-batches/{batch['id']}/insert", {})["status"] == "prepared"
            assert received(session) == before_input
            api(f"/review-batches/{batch['id']}/confirm", {"confirmed": False}, expected=400)
            handed = api(f"/review-batches/{batch['id']}/confirm", {"confirmed": True})
            assert handed["status"] == "handed_off"
            catalog = api(f"/projects/{project['id']}/comments")
            saved_comment = next(c for c in catalog["comments"] if c["id"] == comment["id"])
            assert any(d["session_id"] == session["id"] and d["revision"] == comment["revision"] for d in saved_comment["deliveries"])
            # The owned packet file changed Git status, but selected code stayed valid.
            again = api(f"/projects/{project['id']}/review-batches", {**prepare, "request_id": str(uuid.uuid4())})
            assert again["status"] == "draft"
            source.write_text("def title(width):\n    minimum = 32\n    return max(width, minimum)\n")
            api(f"/review-batches/{again['id']}/insert", {}, expected=400)
            assert received(session) == before_input
            api(f"/review-comments/{comment['id']}/revalidate", {"expected_revision": comment["revision"]}, expected=400)
            source.write_text("def title(width):\n    minimum = 24\n    return max(width, minimum)\n")
            api(f"/review-comments/{comment['id']}/revalidate", {"expected_revision": comment["revision"]})
            # Commit only selected source while both owned CLI processes stay alive.
            preview = api(f"/projects/{project['id']}/commit-preview", {"root_id": first["id"], "paths": ["sample.py"]})
            commit_body = {"root_id": first["id"], "paths": ["sample.py"], "message": "Commit reviewed source",
                           "expected_snapshot": preview["snapshot"], "request_id": str(uuid.uuid4())}
            committed = api(f"/projects/{project['id']}/commit", commit_body)
            assert committed["committed"]
            assert api(f"/projects/{project['id']}/commit", commit_body)["head"] == committed["head"]
            assert packet_path.exists()
            prior_token = token
            stop()
            boot()
            assert token == prior_token
            restarted = api(f"/projects/{project['id']}/comments")
            assert len(restarted["comments"]) == 2
            assert any(b["id"] == batch["id"] and b["status"] == "handed_off" for b in restarted["batches"])
            assert api(f"/projects/{project['id']}/commit", commit_body)["head"] == committed["head"]
            print(json.dumps({"result": "passed", "inference_requested": False, "checks": [
                "authenticated_range_creation", "exact_multiline_server_excerpt", "comment_revision_CAS", "cross_root_access",
                "immutable_batch_idempotency", "prepare_does_not_send", "packet_file_exact_bytes", "no_auto_enter",
                "insert_retry_no_duplicate", "user_reported_handoff_only", "recipient_revision_ledger",
                "owned_artifact_does_not_invalidate_code", "changed_code_blocks_insert", "explicit_revalidation",
                "commit_selected_while_CLI_alive", "commit_receipt_idempotency_and_restart", "comments_batches_survive_restart",
            ]}, ensure_ascii=False))
        finally:
            stop()


if __name__ == "__main__":
    main()
