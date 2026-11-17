#![cfg(unix)]

use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use uuid::Uuid;

struct Daemon {
    child: Child,
    path: PathBuf,
}
impl Drop for Daemon {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // The child is the test's daemon. Its shutdown owns the fixture PTYs.
            unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
            if !wait_for(|| self.child.try_wait().ok().flatten().is_some()) {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn wait_for(mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
    true
}

fn request(host: &str, path: &str, credential: (&str, &str), body: Option<Value>) -> (u16, Value) {
    let mut connection = TcpStream::connect(host).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(12)))
        .unwrap();
    connection
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let method = if body.is_some() { "POST" } else { "GET" };
    let bytes = body.map(|v| v.to_string()).unwrap_or_default();
    write!(
        connection,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{}: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bytes}",
        credential.0, credential.1, bytes.len()
    ).unwrap();
    let mut response = String::new();
    connection.read_to_string(&mut response).unwrap();
    let (headers, content) = response.split_once("\r\n\r\n").unwrap();
    let status = headers
        .split_ascii_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (
        status,
        if content.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(content).unwrap()
        },
    )
}

#[test]
fn new_claude_session_recovers_after_failed_probe_and_receives_activity() {
    let path = std::env::temp_dir().join(format!("mpac-claude-launch-{}", Uuid::new_v4()));
    let bin = path.join("bin");
    let project_dir = path.join("project");
    let data = path.join("state");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir(&project_dir).unwrap();
    let fake_claude = bin.join("claude");
    // The first launch can't read a version. The same executable later
    // succeeds, and a real PTY launch must receive the generated settings.
    fs::write(
        &fake_claude,
        r#"#!/bin/sh
if [ "$1" = '--version' ]; then
    [ -f "$0.ready" ] && printf '2.1.325 (Claude Code)\n'
    exit 0
fi
if [ "$1" = '--settings' ] && [ -s "$2" ]; then
    : > "$0.instrumented"
fi
while IFS= read -r line; do
    if [ "$line" = 'mpac-fixture-done' ]; then
        printf '\033]7501;state=done:app=claude-code\007'
    fi
done
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o700)).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_mpac-daemon"))
        .args(["--port", "0", "--data-dir"])
        .arg(&data)
        .arg("--ui-dir")
        .arg(&path)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _daemon = Daemon { child, path };
    let launch_file = data.join("launch.json");
    assert!(wait_for(|| fs::read(&launch_file).is_ok_and(|bytes| {
        serde_json::from_slice::<Value>(&bytes).is_ok()
    })));
    let launch: Value = serde_json::from_slice(&fs::read(launch_file).unwrap()).unwrap();
    let host = format!("127.0.0.1:{}", launch["port"]);
    let token = format!(
        "Bearer {}",
        fs::read_to_string(data.join("auth-token")).unwrap().trim()
    );
    let api = |path: &str, body| {
        let (status, data) = request(&host, path, ("Authorization", &token), body);
        assert_eq!(status, 200);
        data
    };
    let project = api(
        "/api/projects",
        Some(json!({
            "name": "Activity fixture", "paths": [project_dir.canonicalize().unwrap()],
        })),
    );
    let start = || {
        api(
            "/api/sessions",
            Some(json!({
                "project_id": project["id"], "root_id": project["roots"][0]["id"], "agent": "claude",
            })),
        )
    };
    let first = start();
    assert_eq!(first["status"], "running");
    assert_eq!(first["activity"]["adapter"], "legacy");
    fs::write(fake_claude.with_extension("ready"), "").unwrap();

    let second = start();
    assert_eq!(second["status"], "running");
    assert_eq!(second["activity"]["adapter"], "claude_hooks");
    assert!(wait_for(|| fake_claude
        .with_extension("instrumented")
        .is_file()));
    let id = second["id"].as_str().unwrap();
    let config: Value = serde_json::from_slice(
        &fs::read(
            data.join("agent-events")
                .join(id)
                .join("claude-settings.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let event = |name: &str, extra: Value| {
        let hook = &config["hooks"][name][0]["hooks"][0];
        let url = hook["url"].as_str().unwrap();
        let endpoint = url.strip_prefix(&format!("http://{host}")).unwrap();
        let secret = hook["headers"]["x-mpac-event-token"].as_str().unwrap();
        let mut body = json!({"hook_event_name":name,"session_id":"fixture-native-session","prompt_id":"fixture-turn"});
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            request(&host, endpoint, ("x-mpac-event-token", secret), Some(body)).0,
            204
        );
    };
    let current = || {
        api("/api/state", None)["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .unwrap()["activity"]
            .clone()
    };

    event("UserPromptSubmit", json!({}));
    assert_eq!(current()["phase"], "working");
    let tool = json!({"tool_name":"AskUserQuestion","tool_use_id":"fixture-question"});
    event("PreToolUse", tool.clone());
    assert_eq!(current()["attention"]["kind"], "question");
    event("PostToolUse", tool);
    assert!(current()["attention"].is_null());
    event("Stop", json!({"last_assistant_message":"Fixture reply"}));
    assert_eq!(current()["result"]["revision"], 1);
    api(
        &format!("/api/sessions/{id}/input"),
        Some(json!({"data":"mpac-fixture-done\r"})),
    );
    assert!(wait_for(|| current()["phase"] == "idle"));
    assert_eq!(current()["result"]["revision"], 1);
    assert_eq!(
        api(&format!("/api/sessions/{id}/answer"), None)["answer"]["text"],
        "Fixture reply"
    );
}
