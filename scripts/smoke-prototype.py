#!/usr/bin/env python3
"""End-to-end local daemon checks using own demo CLI, no model requests."""
import base64
import json
import os
import re
from pathlib import Path
import signal
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target/debug/mpac-daemon"


def wait_for(fn, label, seconds=8):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = fn()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError(f"timeout: {label}")


def main():
    with tempfile.TemporaryDirectory(prefix="mpac-smoke-") as folder:
        data = Path(folder) / "state"
        launch = data / "launch.json"
        daemon = None
        fixture_bin = Path(folder) / "bin"
        fixture_bin.mkdir()
        fixture_cli = fixture_bin / "claude"
        fixture_cli.write_text(f"#!{sys.executable}\nimport sys\nprint('OWN_FIXTURE_READY', flush=True)\nfor line in sys.stdin:\n print(line, flush=True)\n")
        fixture_cli.chmod(0o700)

        def fixture_git(directory, *args):
            return subprocess.run(
                ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgSign=false", "-c", "user.name=MPAC fixture", "-c", "user.email=fixture@localhost", *args],
                cwd=directory, check=True, capture_output=True, text=True,
                env={**os.environ, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull, "GIT_ATTR_NOSYSTEM": "1"},
            ).stdout.strip()

        def boot():
            nonlocal daemon
            launch.unlink(missing_ok=True)
            daemon = subprocess.Popen(
                [str(BINARY), "--port", "0", "--data-dir", str(data)],
                cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                env={**os.environ, "PATH": str(fixture_bin) + os.pathsep + os.environ.get("PATH", "")},
            )
            wait_for(lambda: launch.exists(), "daemon startup")
            config = json.loads(launch.read_text())
            url = urllib.parse.urlsplit(config["url"])
            return f"http://{url.netloc}", urllib.parse.parse_qs(url.fragment)["token"][0]

        def shutdown():
            if daemon and daemon.poll() is None:
                daemon.send_signal(signal.SIGTERM)
                try:
                    daemon.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    daemon.kill()
                    daemon.wait(timeout=3)
                    raise

        base, token = boot()

        def request(path, body=None, expected=200, auth=True, origin=None, host=None):
            headers = {"Authorization": f"Bearer {token}"} if auth else {}
            if body is not None:
                headers["Content-Type"] = "application/json"
            if origin:
                headers["Origin"] = origin
            if host:
                headers["Host"] = host
            req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(), headers=headers)
            try:
                with urllib.request.urlopen(req, timeout=8) as response:
                    code, result = response.status, json.load(response)
            except urllib.error.HTTPError as error:
                code, result = error.code, json.load(error)
            assert code == expected, (path, code, result)
            return result

        try:
            request("/api/state", auth=False, expected=401)
            request("/api/state", origin="https://untrusted.example", expected=403)
            request("/api/state", host="evil.localhost", expected=403)
            state = request("/api/state")
            project = state["projects"][0]
            first, second = project["roots"]
            assert state["capabilities"]["project_management"] is True
            assert state["capabilities"]["session_management"] is True
            assert state["capabilities"]["session_attachments"] is True
            assert state["capabilities"]["session_rename"] is True
            assert state["capabilities"]["git_branches"] is True
            managed = request("/api/projects", {"name": "CRUD fixture", "paths": [first["path"], second["path"]]})
            managed_endpoint = f"/api/projects/{managed['id']}"
            original = managed
            request(managed_endpoint + "/update", {"name": "Unauthorized", "paths": [first["path"]], "expected": managed}, auth=False, expected=401)
            request(managed_endpoint + "/delete", {"expected": managed}, origin="https://untrusted.example", expected=403)
            managed = request(managed_endpoint + "/update", {"name": "Renamed CRUD fixture", "paths": [first["path"], second["path"]], "expected": managed})
            assert [r["id"] for r in managed["roots"]] == [r["id"] for r in original["roots"]]
            request(managed_endpoint + "/update", {"name": "Stale overwrite", "paths": [first["path"]], "expected": original}, expected=400)
            managed_session = request("/api/sessions", {"project_id": managed["id"], "root_id": managed["roots"][0]["id"], "agent": "demo", "title": "Own CRUD active-root fixture"})
            managed_session_endpoint = f"/api/sessions/{managed_session['id']}"
            previous_title = managed_session["title"]
            request(managed_session_endpoint + "/rename", {"title": "Unauthorized", "expected_title": previous_title}, auth=False, expected=401)
            managed_session = request(managed_session_endpoint + "/rename", {"title": "Переименованная сессия", "expected_title": previous_title})
            assert managed_session["title"] == "Переименованная сессия" and managed_session["status"] in ("running", "needs_attention")
            request(managed_session_endpoint + "/rename", {"title": "Stale", "expected_title": previous_title}, expected=400)
            request(managed_endpoint + "/update", {"name": managed["name"], "paths": [second["path"]], "expected": managed}, expected=400)
            request(managed_endpoint + "/delete", {"expected": managed}, expected=400)
            added_folder = Path(folder) / "added folder "
            added_folder.mkdir()
            managed = request(managed_endpoint + "/update", {"name": "Renamed while running", "paths": [first["path"], second["path"], str(added_folder)], "expected": managed})
            assert managed["roots"][0]["id"] == original["roots"][0]["id"]
            assert managed["roots"][2]["path"].endswith(" ")
            managed = request(managed_endpoint + "/update", {"name": managed["name"], "paths": [first["path"], str(added_folder)], "expected": managed})
            fixture_git(added_folder, "init", "-b", "main")
            note = added_folder / "note.md"
            note.write_text("main branch\n")
            fixture_git(added_folder, "add", "--", "note.md")
            fixture_git(added_folder, "commit", "-m", "Own main fixture")
            fixture_git(added_folder, "switch", "-c", "feature/smoke")
            note.write_text("feature branch\n")
            fixture_git(added_folder, "add", "--", "note.md")
            fixture_git(added_folder, "commit", "-m", "Own feature fixture")
            fixture_git(added_folder, "switch", "main")
            branch_root = managed["roots"][1]
            branch_query = urllib.parse.urlencode({"root_id": branch_root["id"]})
            branch_endpoint = managed_endpoint + "/branches?" + branch_query

            def switch_body(catalog, target):
                return {"root_id": branch_root["id"], "branch": target, "expected_head": catalog["head"], "expected_branch": catalog["current"]}

            branches = request(branch_endpoint)
            assert branches["current"] == "main" and not branches["dirty"] and not branches["blocked_reason"], "An agent in a different repository of this project must not block switching"
            request(managed_endpoint + "/branch", switch_body(branches, "feature/smoke"), origin="https://untrusted.example", expected=403)
            switched = request(managed_endpoint + "/branch", switch_body(branches, "feature/smoke"))
            assert switched["current"] == "feature/smoke" and note.read_text() == "feature branch\n"
            request(managed_endpoint + "/branch", switch_body(branches, "main"), expected=400)
            note.write_text("keep this own dirty edit\n")
            assert request(branch_endpoint)["dirty"]
            request(managed_endpoint + "/branch", switch_body(switched, "main"), expected=400)
            assert note.read_text() == "keep this own dirty edit\n"
            note.write_text("feature branch\n")
            branches = request(managed_endpoint + "/branch", switch_body(switched, "main"))
            branch_session = request("/api/sessions", {"project_id": managed["id"], "root_id": branch_root["id"], "agent": "demo", "title": "Own branch guard fixture"})
            assert request(branch_endpoint)["blocked_reason"]
            request(managed_endpoint + "/branch", switch_body(branches, "feature/smoke"), expected=400)
            request(f"/api/sessions/{branch_session['id']}/stop", {})
            request(f"/api/sessions/{managed_session['id']}/stop", {})
            wait_for(lambda: next(s for s in request("/api/state")["sessions"] if s["id"] == managed_session["id"])["status"] == "stopped", "CRUD fixture stopped")
            request(managed_endpoint + "/delete", {"expected": managed})
            archived_state = request("/api/state")
            assert not any(p["id"] == managed["id"] for p in archived_state["projects"])
            assert any(p["id"] == managed["id"] for p in archived_state["archived_projects"])
            assert not any(s["id"] == managed_session["id"] for s in archived_state["sessions"])
            assert added_folder.is_dir() and (Path(first["path"]) / "README.md").is_file()
            restored_project = request(managed_endpoint + "/restore", {})
            assert restored_project == managed
            assert any(s["id"] == managed_session["id"] for s in request("/api/state")["sessions"])
            assert next(s for s in request("/api/state")["sessions"] if s["id"] == managed_session["id"])["title"] == "Переименованная сессия"
            request(managed_endpoint + "/delete", {"expected": restored_project})
            root_query = urllib.parse.urlencode({"root_id": first["id"]})
            listing = request(f"/api/projects/{project['id']}/files?{root_query}")
            assert any(f["path"] == "README.md" for f in listing["files"])
            artifact = request(f"/api/projects/{project['id']}/file?{root_query}&path=README.md")
            assert artifact["content"].startswith("# backend")
            request(f"/api/projects/{project['id']}/file?{root_query}&path=../../state.sqlite3", expected=400)
            review = request(f"/api/projects/{project['id']}/review?{root_query}")
            assert "README.md" in review["diff"] and "plan.md" in review["diff"]

            session = request("/api/sessions", {"project_id": project["id"], "root_id": first["id"], "agent": "demo", "title": "Smoke demo"})
            sid = session["id"]
            assert session["access"]["mode"] == "inherit"
            assert session["additional_roots"] == [] and session["permission_args"] == []
            request("/api/sessions", {"project_id": project["id"], "root_id": first["id"], "agent": "demo", "access": {"mode": "full_access", "acknowledge_full_access": True}}, expected=400)

            def current():
                return next(s for s in request("/api/state")["sessions"] if s["id"] == sid)

            wait_for(lambda: current()["status"] == "needs_attention", "demo question")
            request(f"/api/sessions/{sid}/delete", {}, expected=400)
            attachment_body = {"name": "заметка с пробелом.txt", "data_base64": base64.b64encode("Файл для собственной демо-проверки".encode()).decode()}
            request(f"/api/sessions/{sid}/attachments", attachment_body, auth=False, expected=401)
            request(f"/api/sessions/{sid}/attachments", attachment_body, origin="https://untrusted.example", expected=403)
            request(f"/api/sessions/{sid}/attachments", {**attachment_body, "name": "../escape.txt"}, expected=400)
            attached = request(f"/api/sessions/{sid}/attachments", attachment_body)
            attached_path = Path(attached["path"])
            assert attached_path.is_relative_to(Path(first["path"]) / ".mpac-attachments")
            assert attached_path.read_bytes() == base64.b64decode(attachment_body["data_base64"])
            assert attached["size"] == len(attached_path.read_bytes())
            assert current()["status"] == "needs_attention", "An upload must never submit CLI input"
            output = request(f"/api/sessions/{sid}/output?after=0")
            assert "демо-агент" in base64.b64decode(output["data_base64"]).decode(errors="replace")
            request(f"/api/sessions/{sid}/resize", {"cols": 120, "rows": 32})
            request(f"/api/sessions/{sid}/review", {"root_id": second["id"], "snapshot": review["snapshot"], "comments": "Wrong repo"}, expected=400)
            stale_snapshot = review["snapshot"]
            with (Path(first["path"]) / "README.md").open("a") as file:
                file.write("\nNew edit after review\n")
            request(f"/api/sessions/{sid}/review", {"root_id": first["id"], "snapshot": stale_snapshot, "comments": "Stale note"}, expected=400)
            review = request(f"/api/projects/{project['id']}/review?{root_query}")
            pasted = request(f"/api/sessions/{sid}/review", {"root_id": first["id"], "snapshot": review["snapshot"], "comments": "Check this change\nDo not auto submit"})
            assert pasted["requires_enter"] and not pasted["submitted"]
            request(f"/api/sessions/{sid}/input", {"data": "\r"})
            wait_for(lambda: current()["status"] == "running", "answered question clears attention", seconds=2)
            wait_for(lambda: current()["status"] == "needs_attention", "next demo question")
            request(f"/api/sessions/{sid}/input", {"data": "done\r"})
            wait_for(lambda: current()["status"] == "exited", "natural exit")
            request(f"/api/sessions/{sid}/input", {"data": "later"}, expected=400)
            request(f"/api/sessions/{sid}/attachments", attachment_body, expected=400)
            request(f"/api/sessions/{sid}/delete", {}, auth=False, expected=401)
            request(f"/api/sessions/{sid}/delete", {}, origin="https://untrusted.example", expected=403)
            request(f"/api/sessions/{sid}/delete", {})
            request(f"/api/sessions/{sid}/delete", {})
            archived = request("/api/state")
            assert not any(s["id"] == sid for s in archived["sessions"])
            assert any(s["id"] == sid for s in archived["archived_sessions"])
            assert attached_path.is_file(), "Archiving must preserve uploaded files"
            request(f"/api/sessions/{sid}/restore", {})
            request(f"/api/sessions/{sid}/restore", {})
            assert current()["status"] == "exited", "Restore must not launch CLI"
            request(f"/api/sessions/{sid}/delete", {})

            draft_session = request("/api/sessions", {"project_id": project["id"], "root_id": second["id"], "agent": "demo", "title": "Stop unsubmitted draft"})
            did = draft_session["id"]
            wait_for(lambda: next(s for s in request("/api/state")["sessions"] if s["id"] == did)["status"] == "needs_attention", "draft fixture ready")
            offset = request(f"/api/sessions/{did}/output?after=0")["seq"]
            request(f"/api/sessions/{did}/input", {"data": "done"})
            wait_for(lambda: b"done" in base64.b64decode(request(f"/api/sessions/{did}/output?after={offset}")["data_base64"]), "draft echoed without Enter")
            request(f"/api/sessions/{did}/stop", {})
            wait_for(lambda: next(s for s in request("/api/state")["sessions"] if s["id"] == did)["status"] == "stopped", "Stop draft")
            draft_output = base64.b64decode(request(f"/api/sessions/{did}/output?after={offset}")["data_base64"]).decode(errors="replace")
            assert "Готово." not in draft_output, "Stop must not submit pending terminal input"

            replace_session = request("/api/sessions", {"project_id": project["id"], "root_id": second["id"], "agent": "demo", "title": "Root identity guard"})
            second_query = urllib.parse.urlencode({"root_id": second["id"]})
            second_review = request(f"/api/projects/{project['id']}/review?{second_query}")
            old_root = Path(second["path"]).with_name("frontend-old")
            Path(second["path"]).rename(old_root)
            shutil.copytree(old_root, second["path"], symlinks=True)
            request(f"/api/sessions/{replace_session['id']}/review", {"root_id": second["id"], "snapshot": second_review["snapshot"], "comments": "Wrong physical checkout"}, expected=400)
            request(f"/api/sessions/{replace_session['id']}/stop", {})

            # Own CLI fixture checks argv delivery and explicit review-root
            # binding only. Actual Codex sandbox semantics are checked natively.
            granted = request("/api/sessions", {"project_id": project["id"], "root_id": first["id"], "agent": "claude", "title": "Own grant fixture", "access": {"mode": "manual", "additional_root_ids": [second["id"]]}})
            assert granted["permission_args"] == ["--permission-mode", "manual", "--add-dir", second["path"]]
            assert granted["additional_roots"] == [second]
            second_review = request(f"/api/projects/{project['id']}/review?{second_query}")
            request(f"/api/sessions/{granted['id']}/review", {"root_id": second["id"], "snapshot": second_review["snapshot"], "comments": "Explicit additional root"})
            Path(second["path"]).rename(Path(second["path"]).with_name("frontend-granted-old"))
            shutil.copytree(old_root, second["path"], symlinks=True)
            second_review = request(f"/api/projects/{project['id']}/review?{second_query}")
            request(f"/api/sessions/{granted['id']}/review", {"root_id": second["id"], "snapshot": second_review["snapshot"], "comments": "Replaced extra root"}, expected=400)
            request(f"/api/sessions/{granted['id']}/stop", {})

            tree = request("/api/sessions", {"project_id": project["id"], "root_id": first["id"], "agent": "demo", "title": "PTY descendant cleanup"})
            tid = tree["id"]
            request(f"/api/sessions/{tid}/input", {"data": "tree\r"})
            def tree_pids():
                text = base64.b64decode(request(f"/api/sessions/{tid}/output?after=0")["data_base64"]).decode(errors="replace")
                pids = re.findall(r"\[demo-(?:grandchild|child) (\d+)\]", text)
                return list(map(int, pids)) if len(pids) == 2 else None
            pids = wait_for(tree_pids, "PTY child and grandchild")
            request(f"/api/sessions/{tid}/stop", {})
            def stopped(pid):
                probe = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True)
                return not probe.stdout.strip() or probe.stdout.strip().startswith("Z")
            wait_for(lambda: all(stopped(pid) for pid in pids), "PTY descendants terminated")

            active = request("/api/sessions", {"project_id": project["id"], "root_id": second["id"], "agent": "demo", "title": "Stop smoke"})
            request(f"/api/sessions/{active['id']}/stop", {})
            wait_for(lambda: next(s for s in request("/api/state")["sessions"] if s["id"] == active["id"])["status"] == "stopped", "explicit Stop")
            # Reboot reconciles a crash-interrupted session without replaying it.
            active = request("/api/sessions", {"project_id": project["id"], "root_id": second["id"], "agent": "demo", "title": "Restart smoke"})
            saved_token = token
            shutdown()
            # Graceful shutdown is separately required to persist stopped state.
            base, token = boot()
            assert token == saved_token, "Ordinary daemon restart must preserve browser authentication"
            restored = request("/api/state")
            assert restored["projects"][0]["id"] == project["id"]
            assert all(s["status"] not in ("running", "needs_attention") for s in restored["sessions"])
            assert len(restored["sessions"]) == 6
            assert any(s["id"] == sid for s in restored["archived_sessions"])
            request(f"/api/projects/{project['id']}/delete", {"expected": project})
            shutdown()
            base, token = boot()
            empty_state = request("/api/state")
            assert empty_state["projects"] == [] and empty_state["sessions"] == []
            request(f"/api/sessions/{sid}/restore", {}, expected=400)
            assert len(empty_state["archived_projects"]) == 2, "Archiving every project must not seed another demo on restart"
            assert request(f"/api/projects/{project['id']}/restore", {}) == project
            request(f"/api/sessions/{sid}/restore", {})
            assert len(request("/api/state")["sessions"]) == 7
            print(json.dumps({"result": "passed", "checks": ["auth_host_origin", "project_files_markdown", "path_confinement", "multi_repo_diff", "pty_io_resize", "pty_descendant_cleanup", "attention_lifecycle", "review_root_binding", "physical_checkout_identity", "stale_review_rejected", "review_draft_no_auto_enter", "stop_never_submits_draft", "natural_exit", "stop", "restart_no_replay", "permission_argv_and_additional_root_identity", "project_edit_ids_and_stale_guard", "project_active_root_and_delete_guards", "project_archive_restore_preserves_files_history", "all_projects_archived_restart_stays_empty", "active_session_cannot_be_archived", "session_archive_restore_idempotence_and_restart", "session_restore_requires_visible_project", "attachment_upload_bytes_and_safe_name", "attachment_auth_origin_active_guards_and_no_input", "active_session_rename_and_stale_guard", "rename_preserved_after_project_restore", "local_branch_switch_and_stale_head", "dirty_branch_switch_preserves_edits", "branch_active_repo_guard_allows_unrelated_repo"], "inference_requested": False}, ensure_ascii=False))
        except Exception:
            try:
                print("Failure state:", json.dumps(request("/api/state"), ensure_ascii=False))
                print("Demo output:", base64.b64decode(request(f"/api/sessions/{sid}/output?after=0")["data_base64"]).decode(errors="replace"))
            except Exception:
                pass
            raise
        finally:
            shutdown()
            if daemon and daemon.stderr:
                errors = daemon.stderr.read().decode(errors="replace")
                if errors.strip():
                    print("Daemon diagnostics:", errors)
                daemon.stderr.close()


if __name__ == "__main__":
    main()
