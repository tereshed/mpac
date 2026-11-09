use crate::{known_value, rpc, version_number};
use anyhow::{Context, Result, bail};
use mpac_runtime::process::{ProtocolProcess, discard_output};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::Command,
    time::timeout,
};

const BUDGET: Duration = Duration::from_secs(12);
const MAX_OUTPUT: u64 = 256 * 1024;

pub fn executable(name: &str) -> Option<PathBuf> {
    let supplied = Path::new(name);
    if supplied.components().count() > 1 {
        return supplied.is_file().then(|| supplied.to_path_buf());
    }
    for folder in std::env::split_paths(&std::env::var_os("PATH")?) {
        let candidates = if cfg!(windows) {
            vec![
                name.to_owned(),
                format!("{name}.exe"),
                format!("{name}.cmd"),
                format!("{name}.bat"),
            ]
        } else {
            vec![name.to_owned()]
        };
        for candidate in candidates {
            let path = folder.join(candidate);
            if path.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if path.metadata().ok()?.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
                return Some(path);
            }
        }
    }
    None
}

pub async fn capture(path: &Path, args: &[&str]) -> Result<(bool, Vec<u8>)> {
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("spawn_failed")?;
    let stdout = child.stdout.take().context("stdout_missing")?;
    let stderr = child.stderr.take().context("stderr_missing")?;
    let work = async {
        let read = async {
            let mut data = Vec::new();
            stdout.take(MAX_OUTPUT + 1).read_to_end(&mut data).await?;
            Ok::<_, std::io::Error>(data)
        };
        let discard = async {
            let mut data = Vec::new();
            stderr.take(MAX_OUTPUT + 1).read_to_end(&mut data).await?;
            Ok::<_, std::io::Error>(())
        };
        let (out, err, status) = tokio::join!(read, discard, child.wait());
        let out = out?;
        err?;
        if out.len() as u64 > MAX_OUTPUT {
            bail!("output_limit_exceeded");
        }
        Ok((status?.success(), out))
    };
    match timeout(BUDGET, work).await {
        Ok(result) => result,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            bail!("process_timeout")
        }
    }
}

pub async fn doctor() -> Value {
    let mut agents = Vec::new();
    for (agent, names) in [
        ("claude_code", vec!["claude"]),
        ("codex", vec!["codex"]),
        ("hermes", vec!["hermes"]),
    ] {
        let found = names
            .iter()
            .find_map(|name| executable(name).map(|path| (*name, path)));
        let Some((name, path)) = found else {
            agents.push(json!({"agent":agent,"discovery":"not_found_on_path","live_capabilities":"not_verified"}));
            continue;
        };
        let captured = capture(&path, &["--version"]).await;
        let (status, version) = match captured {
            Ok((true, bytes)) => ("detected", version_number(&String::from_utf8_lossy(&bytes))),
            _ => ("version_probe_failed", None),
        };
        agents.push(json!({"agent":agent,"executable_name":name,"discovery":status,"version":version,"live_capabilities":"not_verified"}));
    }
    agents.push(json!({"agent":"deepseek_cli","discovery":"deferred_by_user","live_capabilities":"not_verified"}));
    let env = [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "OPENROUTER_API_KEY",
        "DEEPSEEK_API_KEY",
        "ZAI_API_KEY",
    ]
    .map(|name| json!({"name":name,"present":std::env::var_os(name).is_some()}));
    json!({"schema_version":1,"scope":"local_metadata_only","platform":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH},"agents":agents,"ambient_environment_presence":env,"inference_requested":false,"billing_enforcement":"not_verified"})
}

pub async fn report() -> Value {
    fn checked(result: Result<Value>) -> Value {
        match result {
            Ok(value) => json!({"status":"passed","evidence":value}),
            Err(error) => {
                json!({"status":"not_verified","reason":crate::safe_error_code(&error),"raw_output_suppressed":true})
            }
        }
    }
    let inventory = doctor().await;
    let codex = checked(handshake("codex", true).await);
    let claude = checked(claude_auth("claude").await);
    let hermes = checked(handshake("hermes", false).await);
    let pty = checked(pty_smoke());
    json!({"schema_version":1,"probe_version":env!("CARGO_PKG_VERSION"),"unix_timestamp":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
        "inventory":inventory,"codex_protocol":codex,"claude_auth":claude,"hermes_acp":hermes,"pty_fixture":pty,
        "inference_requested":false,"account_spend":"not_measured","native_windows":"not_tested","linux_runtime":"not_tested","physical_mobile":"not_tested"})
}

pub fn claude_summary(raw: &Value) -> Value {
    json!({"logged_in":raw.get("loggedIn").and_then(Value::as_bool),
        "auth_method":known_value(raw.get("authMethod").and_then(Value::as_str), &["claude.ai","oauth","oauth_token","api_key","apiKey","none"]),
        "provider":known_value(raw.get("apiProvider").and_then(Value::as_str), &["firstParty","anthropic","bedrock","vertex","foundry"]),
        "plan":known_value(raw.get("subscriptionType").and_then(Value::as_str), &["free","pro","max","ultra","team","enterprise"]),
        "billing_enforcement":"not_verified","inference_requested":false})
}

pub async fn claude_auth(name: &str) -> Result<Value> {
    let path = executable(name).context("executable_not_found")?;
    let (success, bytes) = capture(&path, &["auth", "status", "--json"]).await?;
    let raw: Value =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid_status_response"))?;
    let mut result = claude_summary(&raw);
    result["command_succeeded"] = success.into();
    Ok(result)
}

pub fn codex_account_summary(raw: &Value) -> Value {
    let account = &raw["account"];
    json!({"auth_kind":known_value(account.get("type").and_then(Value::as_str), &["chatgpt","apiKey","amazonBedrock"]),
        "plan":known_value(account.get("planType").and_then(Value::as_str), &["free","go","plus","pro","team","business","enterprise","edu"]),
        "requires_openai_auth":raw.get("requiresOpenaiAuth").and_then(Value::as_bool),
        "billing_enforcement":"not_verified"})
}

pub async fn handshake(name: &str, codex: bool) -> Result<Value> {
    let path = executable(name).context("executable_not_found")?;
    let mut command = Command::new(path);
    if codex {
        command.args(["app-server", "--listen", "stdio://"]);
    } else {
        command.arg("acp");
    }
    // No user turn, session creation, tools, auth mutation, token refresh or
    // inference is requested. Upstream stderr is drained but never persisted.
    let (mut child, io) = ProtocolProcess::spawn(command)?;
    let mut input = io.stdin;
    let mut output = BufReader::new(io.stdout);
    let stderr = io.stderr;
    let stderr_task = tokio::spawn(async move {
        let _ = discard_output(stderr).await;
    });
    let result = timeout(BUDGET, async {
        let params = if codex {
            json!({"clientInfo":{"name":"mpac_phase0_probe","title":"MPAC Phase 0 Probe","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":false}})
        } else {
            json!({"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},"clientInfo":{"name":"mpac_phase0_probe","version":env!("CARGO_PKG_VERSION")}})
        };
        rpc::send(&mut input, &rpc::diagnostic_request(1,"initialize",params)?).await?;
        let initialized = rpc::response(&mut output,1).await?;
        let mut summary = json!({"initialize":"passed","transport":"stdio","execution_mode":"protocol","inference_requested":false,"live_turn":"not_tested","approval_flow":"not_tested","interrupt":"not_tested","billing_enforcement":"not_verified"});
        if codex {
            rpc::send(&mut input,&json!({"method":"initialized"})).await?;
            rpc::send(&mut input,&rpc::diagnostic_request(2,"account/read",json!({"refreshToken":false}))?).await?;
            let account = rpc::response(&mut output,2).await?;
            summary["account"] = codex_account_summary(&account);
        } else {
            summary["protocol_version"] = initialized.get("protocolVersion").and_then(Value::as_u64).map(Value::from).unwrap_or(Value::Null);
            summary["auth_method_count"] = initialized.get("authMethods").and_then(Value::as_array).map(|a| Value::from(a.len())).unwrap_or(Value::Null);
        }
        Ok::<Value,anyhow::Error>(summary)
    }).await;
    drop(input);
    let stopped = child.stop().await;
    stderr_task.abort();
    stopped?;
    result.map_err(|_| anyhow::anyhow!("handshake_timeout"))?
}

pub fn pty_smoke() -> Result<Value> {
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    use std::{io::Read, sync::mpsc, thread};
    let pair = native_pty_system().openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut command = if cfg!(windows) {
        let mut c = CommandBuilder::new("cmd.exe");
        c.args(["/D", "/C", "echo MPAC_PTY_SMOKE"]);
        c
    } else {
        let mut c = CommandBuilder::new("/bin/sh");
        c.args([
            "-c",
            "test -t 0 && test -t 1 && printf 'MPAC_PTY_SMOKE\\r\\n'",
        ]);
        c
    };
    command.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut data = Vec::new();
        let mut chunk = [0_u8; 1024];
        while data.len() < 16384 {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    data.extend_from_slice(&chunk[..n]);
                    if data
                        .windows(b"MPAC_PTY_SMOKE".len())
                        .any(|w| w == b"MPAC_PTY_SMOKE")
                    {
                        break;
                    }
                }
            }
        }
        let _ = tx.send(data);
    });
    let output = rx.recv_timeout(Duration::from_secs(5));
    let mut exit = None;
    for _ in 0..50 {
        exit = child.try_wait()?;
        if exit.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let status = match exit {
        Some(status) => status,
        None => {
            let _ = child.kill();
            child.wait()?
        }
    };
    drop(pair.master);
    // Do not await a blocked reader after timeout: closing a ConPTY does not
    // guarantee EOF on every implementation. The fixture process is reaped.
    if output.is_ok() {
        let _ = handle.join();
    }
    let bytes = output.context("pty_timeout")?;
    let marker = bytes
        .windows(b"MPAC_PTY_SMOKE".len())
        .any(|w| w == b"MPAC_PTY_SMOKE");
    Ok(
        json!({"marker_received":marker,"child_success":status.success(),"platform":std::env::consts::OS,"scope":"own_fixture_process_only","terminal_snapshot":"not_tested","process_tree_termination":"not_tested","ui_independence":"not_tested"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_fixture_has_a_tty_and_exits_cleanly() {
        let result = pty_smoke().unwrap();
        assert_eq!(result["marker_received"], true);
        assert_eq!(result["child_success"], true);
    }

    #[test]
    fn auth_summaries_exclude_identity_credentials_and_unknown_fields() {
        let raw = json!({"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max","email":"private@example.com","apiKey":"sk-secret","organizationId":"private-org","other":{"token":"hidden"}});
        let output = claude_summary(&raw).to_string();
        for secret in ["private@example.com", "sk-secret", "private-org", "hidden"] {
            assert!(!output.contains(secret));
        }
        let raw = json!({"account":{"type":"chatgpt","planType":"pro","email":"private@example.com","accessToken":"sk-secret"}});
        let output = codex_account_summary(&raw).to_string();
        assert!(!output.contains("private@example.com"));
        assert!(!output.contains("sk-secret"));
        assert_eq!(
            codex_account_summary(&raw)["billing_enforcement"],
            "not_verified"
        );
    }
}
