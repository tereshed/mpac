#!/usr/bin/env python3
"""Exercise only owned temporary desktop profiles; no real AI providers or main daemon.

Runs the bundled native --probe launcher, not GUI window/quit automation. Reports
only check names and numeric PID/port metadata; authenticated URLs stay private.
"""
import argparse
import ctypes
import fcntl
import json
import os
from pathlib import Path
import signal
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import urllib.request

TOKEN = "12345678-1234-1234-1234-123456789abc12345678-1234-1234-1234-123456789abc"


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def private_write(path, content):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(content)


def executable_of(pid):
    libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    libproc.proc_pidpath.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
    libproc.proc_pidpath.restype = ctypes.c_int
    buffer = ctypes.create_string_buffer(4096)
    if libproc.proc_pidpath(pid, buffer, len(buffer)) <= 0:
        return None
    return Path(os.fsdecode(buffer.value)).resolve()


class ForeignListener(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self, redirect=None):
        self.connections = 0
        self.requests = []
        self.redirect = redirect
        super().__init__(("127.0.0.1", 0), ForeignHandler)
        self.thread = threading.Thread(target=self.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.shutdown()
        self.server_close()
        self.thread.join(timeout=2)


class ForeignHandler(socketserver.BaseRequestHandler):
    def handle(self):
        self.server.connections += 1
        self.request.settimeout(3)
        try:
            data = self.request.recv(16384)
        except OSError:
            return
        if data:
            self.server.requests.append(data)
            location = self.server.redirect or "http://127.0.0.1:1/"
            self.request.sendall(("HTTP/1.1 302 Found\r\nLocation: " + location
                                  + "\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").encode())


class Smoke:
    def __init__(self, bundle, workspace):
        self.binary = bundle / "Contents/MacOS/mpac-desktop"
        self.sidecar = (bundle / "Contents/MacOS/mpac-daemon").resolve()
        self.workspace = workspace.resolve()
        self.owned = {}
        self.data_dirs = set()
        self.checks = []
        self.http = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

    def check(self, label, condition=True):
        require(condition, label)
        self.checks.append(label)
        print("PASS " + label, flush=True)

    def folder(self, name):
        path = self.workspace / name
        path.mkdir(mode=0o700)
        return path

    def command(self, profile, data=None):
        args = [str(self.binary), "--probe", "--profile-dir", str(profile)]
        if data is not None:
            self.data_dirs.add(data)
            args += ["--data-dir", str(data)]
        return args

    def decode_probe(self, process, expected=True):
        stdout, stderr = process.communicate(timeout=50)
        # Do not echo diagnostics: they could expose credentials after a regression.
        require(TOKEN.encode() not in stdout + stderr, "probe leaked fixture credential")
        require(b"#token=" not in stdout + stderr and b"Bearer " not in stdout + stderr,
                "probe leaked authenticated URL/header")
        if not expected:
            require(process.returncode != 0, "unsafe fixture unexpectedly connected")
            return None
        require(process.returncode == 0, "native probe failed (diagnostics withheld)")
        try:
            value = json.loads(stdout)
        except (ValueError, UnicodeDecodeError):
            raise AssertionError("probe did not return safe JSON metadata") from None
        require(set(value) == {"pid", "port", "reused"}, "unexpected public probe fields")
        require(type(value["pid"]) is int and value["pid"] > 0
                and type(value["port"]) is int and 0 < value["port"] < 65536
                and type(value["reused"]) is bool, "invalid public probe metadata")
        return value

    def probe(self, profile, data=None, expected=True):
        process = subprocess.Popen(self.command(profile, data), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   cwd=self.workspace, stdin=subprocess.DEVNULL)
        try:
            result = self.decode_probe(process, expected)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
        if result and data:
            self.register(data, result)
        return result

    def register(self, data, result):
        launch = json.loads((data / "launch.json").read_text())
        require(launch["pid"] == result["pid"] and launch["port"] == result["port"],
                "owned launch metadata disagrees with probe")
        require(executable_of(result["pid"]) == self.sidecar, "probe PID is not the owned bundled sidecar")
        self.owned[result["pid"]] = (data, result["port"])

    def api(self, data, route, body=None):
        launch = json.loads((data / "launch.json").read_text())
        token = (data / "auth-token").read_text()
        parsed = urllib.parse.urlsplit(launch["url"])
        require(parsed.scheme == "http" and parsed.hostname == "127.0.0.1"
                and parsed.port == launch["port"] and parsed.fragment == "token=" + token,
                "owned API address failed validation")
        require(launch["pid"] in self.owned, "API is restricted to registered owned sidecars")
        headers = {"Authorization": "Bearer " + token, "Content-Type": "application/json"}
        request = urllib.request.Request(f"http://127.0.0.1:{parsed.port}/api" + route,
                                         data=None if body is None else json.dumps(body).encode(), headers=headers)
        try:
            with self.http.open(request, timeout=8) as response:
                require(response.status == 200, "owned API returned non-200")
                return json.loads(response.read(8 * 1024 * 1024))
        except Exception:
            raise AssertionError("owned demo API request failed (details withheld)") from None

    def run(self):
        profile, data = self.folder("profile"), self.folder("data")
        first = self.probe(profile, data)
        self.check("bundled cold launcher starts isolated daemon", not first["reused"])
        second = self.probe(profile, data)
        self.check("launcher exit preserves daemon and warm probe reuses PID/port",
                   second["reused"] and second["pid"] == first["pid"] and second["port"] == first["port"])
        # A brand-new temporary project and the built-in Rust demo CLI only.
        repository = self.folder("demo-project")
        project = self.api(data, "/projects", {"name": "Owned desktop smoke", "paths": [str(repository)]})
        session = self.api(data, "/sessions", {"project_id": project["id"], "root_id": project["roots"][0]["id"],
                                                "agent": "demo", "title": "Owned no-model native persistence"})
        third = self.probe(profile, data)
        current = next(item for item in self.api(data, "/state")["sessions"] if item["id"] == session["id"])
        self.check("warm launch preserves active no-model demo CLI",
                   third["pid"] == first["pid"] and third["reused"]
                   and current["agent"] == "demo" and current["status"] in ("running", "needs_attention")
                   and current["exit_code"] is None)
        concurrent_data = self.folder("concurrent-data")
        profiles = [self.folder("concurrent-profile-" + str(index)) for index in range(2)]
        children = [subprocess.Popen(self.command(profile, concurrent_data), cwd=self.workspace,
                                    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                    for profile in profiles]
        try:
            results = [self.decode_probe(child) for child in children]
            for result in results:
                self.register(concurrent_data, result)
            self.check("concurrent cold launchers share one daemon winner",
                       len({(item["pid"], item["port"]) for item in results}) == 1
                       and sum(not item["reused"] for item in results) == 1)
        finally:
            for child in children:
                if child.poll() is None:
                    child.kill()
                    child.wait()
        invalid_profile, invalid_data = self.folder("invalid-profile"), self.folder("invalid-data")
        private_write(invalid_data / "auth-token", "invalid-do-not-replace")
        self.probe(invalid_profile, invalid_data, expected=False)
        self.check("invalid existing credential is rejected without rotation/spawn",
                   (invalid_data / "auth-token").read_text() == "invalid-do-not-replace"
                   and not (invalid_data / "launch.json").exists())
        symlink_profile = self.folder("symlink-profile")
        target = self.workspace / "private-settings-target.json"
        private_write(target, json.dumps({"data_dir": str(data)}))
        (symlink_profile / "native.json").symlink_to(target)
        self.probe(symlink_profile, expected=False)
        self.check("symlink settings are rejected without following/replacing link",
                   (symlink_profile / "native.json").is_symlink()
                   and json.loads(target.read_text())["data_dir"] == str(data))
        foreign_profile, foreign_data = self.folder("foreign-profile"), self.folder("foreign-data")
        trap = ForeignListener()
        foreign = ForeignListener(f"http://127.0.0.1:{trap.server_address[1]}/leak")
        try:
            private_write(foreign_data / "auth-token", TOKEN)
            private_write(foreign_data / "launch.json", json.dumps({"prototype": True, "pid": os.getpid(),
                          "port": foreign.server_address[1], "url": f"http://127.0.0.1:{foreign.server_address[1]}/#token={TOKEN}"}))
            with open(foreign_data / "daemon.lock", "x+b") as lock:
                os.chmod(lock.fileno(), 0o600)
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                self.probe(foreign_profile, foreign_data, expected=False)
            self.check("foreign listener receives no request or Bearer credential",
                       foreign.connections > 0 and not foreign.requests)
            self.check("foreign redirect destination is never contacted", trap.connections == 0)
            self.check("foreign/stale launch cannot rotate credential or clobber PID",
                       (foreign_data / "auth-token").read_text() == TOKEN
                       and json.loads((foreign_data / "launch.json").read_text())["pid"] == os.getpid())
        finally:
            foreign.close()
            trap.close()

    def cleanup(self):
        # Recover a sidecar started just before a failing/timeout probe, restricted
        # to our own temporary data paths and the exact bundled executable.
        for data in self.data_dirs:
            try:
                launch = json.loads((data / "launch.json").read_text())
                pid = launch["pid"]
                if pid != os.getpid() and executable_of(pid) == self.sidecar:
                    self.owned.setdefault(pid, (data, launch["port"]))
            except (OSError, ValueError, KeyError):
                pass
        for pid, (data, _) in self.owned.items():
            if self.workspace not in data.resolve().parents or executable_of(pid) != self.sidecar:
                continue
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                continue
            deadline = time.monotonic() + 10
            while executable_of(pid) == self.sidecar and time.monotonic() < deadline:
                time.sleep(0.05)
            require(executable_of(pid) != self.sidecar, "owned daemon did not stop gracefully")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, file, code, message, headers, newurl):
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    require(sys.platform == "darwin", "desktop smoke requires macOS libproc")
    bundle = args.app.resolve(strict=True)
    require(bundle.name.endswith(".app"), "--app must identify a standalone .app bundle")
    require((bundle / "Contents/MacOS/mpac-desktop").is_file()
            and (bundle / "Contents/MacOS/mpac-daemon").is_file(), "bundle lacks native launcher/sidecar")
    result = {"passed": False, "checks": [], "scope": "isolated bundled --probe; no GUI automation or model calls"}
    with tempfile.TemporaryDirectory(prefix="mpac-desktop-smoke-") as temporary:
        os.chmod(temporary, 0o700)
        smoke = Smoke(bundle, Path(temporary))
        try:
            smoke.run()
            result["passed"] = True
        finally:
            smoke.cleanup()
            result["checks"] = smoke.checks
            if args.report:
                args.report.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        # Prevent urllib/subprocess exception text from exposing authenticated URLs.
        print("Desktop smoke failed: " + (str(error) if isinstance(error, AssertionError)
                                          else type(error).__name__), file=sys.stderr)
        raise SystemExit(1)
