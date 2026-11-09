//! Per-launch, passive native event channels. These callbacks never return
//! permission decisions, add model context, replay input, or read transcripts.
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path as RoutePath, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::post,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    activity::{ActivityCapabilities, ActivityState},
    api::App,
    model::{self, Session},
};

const MAX_EVENT_BYTES: usize = 256 * 1024;
const MAX_ANSWER_BYTES: usize = 192 * 1024;
const EVENT_HEADER: &str = "x-mpac-event-token";

/// Keep a whole message or skip it. Tool inputs, prompts and transcript paths
/// are never used to populate the reading view.
pub(crate) fn stop_answer(payload: &Value) -> Option<&str> {
    if payload.get("hook_event_name").and_then(Value::as_str) != Some("Stop") {
        return None;
    }
    payload
        .get("last_assistant_message")
        .and_then(Value::as_str)
        .filter(|text| text.len() <= MAX_ANSWER_BYTES && !text.trim().is_empty())
}

#[derive(Clone)]
pub(crate) struct Channel {
    pub(crate) secret: String,
    pub(crate) provider: String,
}

pub struct LaunchHooks {
    pub args: Vec<String>,
    pub channel_path: PathBuf,
    pub provider: String,
}

#[derive(Serialize, Deserialize)]
struct HelperChannel {
    port: u16,
    session_id: String,
    secret: String,
    provider: String,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u32, u32, u32);

fn parse_version(text: &str) -> Option<Version> {
    text.split_whitespace().find_map(|word| {
        let clean = word.trim_start_matches('v');
        let mut parts = clean.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts
            .next()?
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()?;
        Some(Version(major, minor, patch))
    })
}

fn version(path: &Path) -> Option<Version> {
    type Cache = HashMap<(PathBuf, u64, Option<SystemTime>), Option<Version>>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let metadata = path.metadata().ok()?;
    let key = (
        path.canonicalize().ok()?,
        metadata.len(),
        metadata.modified().ok(),
    );
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(found) = cache.lock().unwrap().get(&key) {
        return *found;
    }
    let handle = tokio::runtime::Handle::try_current().ok()?;
    let result = handle.block_on(async {
        use tokio::io::AsyncReadExt;
        let mut command = tokio::process::Command::new(path);
        command.arg("--version");
        command.env_remove("MPAC_EVENT_CHANNEL");
        command.env_remove("MPAC_EVENT_PROVIDER");
        let (mut owner, io) = mpac_runtime::process::ProtocolProcess::spawn(command).ok()?;
        drop(io.stdin);
        let work = async {
            let mut bytes = Vec::new();
            let mut stdout = io.stdout.take(4097);
            let read = stdout.read_to_end(&mut bytes);
            let discard = mpac_runtime::process::discard_output(io.stderr);
            let (read, discard) = tokio::join!(read, discard);
            read.ok()?;
            discard.ok()?;
            (bytes.len() <= 4096).then_some(bytes)
        };
        let captured = tokio::time::timeout(Duration::from_secs(2), work)
            .await
            .ok()
            .flatten();
        // We explicitly stop the entire owned execution after both streams
        // close. The resulting status may be our SIGKILL if the CLI closes
        // stdio just before exiting; it is not a reliable version-probe code.
        owner.stop().await.ok()?;
        parse_version(std::str::from_utf8(&captured?).ok()?)
    });
    let mut cache = cache.lock().unwrap();
    if cache.len() >= 64 {
        cache.clear();
    }
    cache.insert(key, result);
    result
}

fn private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir(path)?;
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Папка событий должна быть обычной папкой"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "Папка событий принадлежит другому пользователю"
        );
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn private_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    Ok(())
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

fn helper(app: &App, directory: &Path) -> Result<PathBuf> {
    let mut slot = app.activity_helper.lock().unwrap();
    if let Some(path) = slot.as_ref() {
        return Ok(path.clone());
    }
    let target = directory.join("activity-helper");
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "Подменён helper событий"
        );
    }
    let temporary = directory.join(format!(".helper-{}", model::id()));
    let source = std::env::current_exe()?;
    let linked = fs::hard_link(&source, &temporary).is_ok();
    if !linked {
        fs::copy(source, &temporary)?;
    }
    #[cfg(unix)]
    if !linked {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700))?;
    }
    fs::rename(&temporary, &target)?;
    *slot = Some(target.clone());
    Ok(target)
}

/// Metadata probes and settings files are local to this daemon's data-dir.
/// Unsupported CLI versions keep the original launch arguments unchanged.
pub(crate) fn prepare(app: &App, meta: &mut Session) -> Result<Option<LaunchHooks>> {
    let Some(executable) = model::executable(&meta.agent) else {
        return Ok(None);
    };
    if !matches!(meta.agent.as_str(), "claude" | "codex") {
        return Ok(None);
    }
    let detected = version(&executable);
    let supported = match (meta.agent.as_str(), detected) {
        ("claude", Some(v)) => v.0 == 2 && v >= Version(2, 1, 145),
        ("codex", Some(v)) => v.0 == 0 && v >= Version(0, 166, 0),
        _ => false,
    };
    if !supported {
        return Ok(None);
    }
    let directory = app.store.data_dir()?.join("agent-events");
    private_directory(&directory)?;
    let helper = helper(app, &directory)?;
    let launch_dir = directory.join(&meta.id);
    private_directory(&launch_dir)?;
    let secret = format!("{}{}", model::id(), model::id());
    let provider = meta.agent.clone();
    let port = app
        .host
        .strip_prefix("127.0.0.1:")
        .context("Нужен локальный адрес событий")?
        .parse::<u16>()?;
    let channel_path = launch_dir.join("channel.json");
    private_json(
        &channel_path,
        &HelperChannel {
            port,
            session_id: meta.id.clone(),
            secret: secret.clone(),
            provider: provider.clone(),
        },
    )?;
    let command = format!("{} --agent-event", shell_quote(&helper));
    let mut args = Vec::new();
    let capabilities = if provider == "claude" {
        let endpoint = format!("http://127.0.0.1:{port}/agent-events/{}", meta.id);
        let mut hooks = serde_json::Map::new();
        for event in [
            "UserPromptSubmit",
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "PostToolUseFailure",
            "Stop",
            "StopFailure",
            "Notification",
            "SubagentStart",
            "SubagentStop",
            "SessionEnd",
        ] {
            hooks.insert(event.into(), json!([{"hooks":[{"type":"http","url":endpoint,"headers":{EVENT_HEADER:secret},"timeout":1}]}]));
        }
        hooks.insert(
            "SessionStart".into(),
            json!([{"hooks":[{"type":"command","command":command,"timeout":1}]}]),
        );
        if detected.is_some_and(|v| v >= Version(2, 1, 318)) {
            for event in ["Elicitation", "ElicitationResult"] {
                hooks.insert(event.into(), json!([{"hooks":[{"type":"http","url":endpoint,"headers":{EVENT_HEADER:secret},"timeout":1}]}]));
            }
        }
        let settings = launch_dir.join("claude-settings.json");
        private_json(&settings, &json!({"hooks":hooks}))?;
        args.extend(["--settings".into(), settings.to_string_lossy().into_owned()]);
        ActivityCapabilities {
            foreground: true,
            questions: true,
            approval: true,
            results: true,
            background: true,
        }
    } else {
        // Hook-specific config layers append to native user/project hook layers.
        // Native /hooks trust remains required; never bypass hook approval.
        args.extend(["-c".into(), "features.hooks=true".into()]);
        for event in [
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "Stop",
            "Interrupt",
        ] {
            let definition = format!(
                "[{{hooks=[{{type=\"command\",command={},timeout=1}}]}}]",
                serde_json::to_string(&command)?
            );
            args.extend(["-c".into(), format!("hooks.{event}={definition}")]);
        }
        ActivityCapabilities {
            foreground: true,
            questions: false,
            approval: true,
            results: true,
            background: false,
        }
    };
    meta.activity = ActivityState::new(format!("{provider}_hooks"), capabilities, model::now());
    app.activity_channels.lock().unwrap().insert(
        meta.id.clone(),
        Channel {
            secret,
            provider: provider.clone(),
        },
    );
    Ok(Some(LaunchHooks {
        args,
        channel_path,
        provider,
    }))
}

pub(crate) fn forget(app: &App, id: &str) {
    app.activity_channels.lock().unwrap().remove(id);
}

pub fn routes() -> Router<std::sync::Arc<App>> {
    Router::new().route(
        "/agent-events/{id}",
        post(receive).layer(DefaultBodyLimit::max(MAX_EVENT_BYTES)),
    )
}

fn same_secret(expected: &str, actual: &str) -> bool {
    expected.len() == actual.len()
        && expected
            .bytes()
            .zip(actual.bytes())
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
}

async fn receive(
    State(app): State<std::sync::Arc<App>>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
    request: Request,
) -> impl IntoResponse {
    if headers.contains_key(header::ORIGIN)
        || headers.get(header::HOST).and_then(|v| v.to_str().ok()) != Some(app.host.as_str())
    {
        return StatusCode::FORBIDDEN;
    }
    let channel = app.activity_channels.lock().unwrap().get(&id).cloned();
    let Some(channel) = channel else {
        return StatusCode::UNAUTHORIZED;
    };
    let Some(secret) = headers.get(EVENT_HEADER).and_then(|v| v.to_str().ok()) else {
        return StatusCode::UNAUTHORIZED;
    };
    if !same_secret(&channel.secret, secret) {
        return StatusCode::UNAUTHORIZED;
    }
    let bytes = match axum::body::to_bytes(request.into_body(), MAX_EVENT_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE,
    };
    let payload: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return StatusCode::BAD_REQUEST,
    };
    let operation = tokio::task::spawn_blocking(move || -> Result<()> {
        let live = app.live_session(&id)?;
        live.activity_event(&app.store, &channel.provider, &payload)?;
        Ok(())
    })
    .await;
    match operation {
        Ok(Ok(())) => StatusCode::NO_CONTENT,
        _ => StatusCode::BAD_REQUEST,
    }
}

/// Invoked by the same executable in passive hook-helper mode.
/// Failures are intentionally silent and never veto an agent operation.
pub fn run_helper() {
    let _ = send_helper_event();
    if std::env::var("MPAC_EVENT_PROVIDER").as_deref() == Ok("codex") {
        print!("{{}}");
    }
}

fn send_helper_event() -> Result<()> {
    let path = PathBuf::from(std::env::var_os("MPAC_EVENT_CHANNEL").context("Нет канала событий")?);
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() < 8192,
        "Недоступен канал"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
            "Канал должен быть приватным"
        );
    }
    let channel: HelperChannel = serde_json::from_slice(&fs::read(path)?)?;
    ensure!(
        uuid::Uuid::parse_str(&channel.session_id).is_ok()
            && channel.secret.len() <= 128
            && channel
                .secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "Неверный канал"
    );
    ensure!(
        matches!(channel.provider.as_str(), "claude" | "codex"),
        "Неверный провайдер"
    );
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_EVENT_BYTES as u64 + 1)
        .read_to_end(&mut input)?;
    ensure!(input.len() <= MAX_EVENT_BYTES, "Событие слишком большое");
    let value: Value = serde_json::from_slice(&input)?;
    let object = value.as_object().context("Неверное событие")?;
    let mut allowed = serde_json::Map::new();
    for key in [
        "hook_event_name",
        "session_id",
        "prompt_id",
        "turn_id",
        "agent_id",
        "agent_type",
        "tool_name",
        "tool_use_id",
        "notification_type",
        "source",
        "reason",
    ] {
        if let Some(value) = object.get(key) {
            allowed.insert(key.into(), value.clone());
        }
    }
    if let Some(text) = stop_answer(&value) {
        allowed.insert("last_assistant_message".into(), json!(text));
    }
    let body = serde_json::to_vec(&allowed)?;
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), channel.port);
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(200))?;
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    let header = format!(
        "POST /agent-events/{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{EVENT_HEADER}: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        channel.session_id,
        channel.port,
        channel.secret,
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&body)?;
    let mut response = [0_u8; 64];
    let _ = stream.read(&mut response)?;
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct MarkRead {
    pub expected_revision: u64,
}

pub(crate) async fn mark_read(
    State(app): State<std::sync::Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<MarkRead>,
) -> crate::api::ApiResult {
    crate::api::work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        let live = app.live.lock().unwrap().get(&id).cloned();
        // This also checks project visibility and archive state for history.
        let (mut stored, archived) = app.store.session_entry(&id)?;
        ensure!(
            !archived
                && app
                    .store
                    .projects()?
                    .iter()
                    .any(|p| p.id == stored.project_id),
            "Сессия недоступна"
        );
        let updated = if let Some(live) = live {
            live.mark_activity_read(&app.store, request.expected_revision)?
        } else {
            stored.activity.mark_read(request.expected_revision)?;
            app.store.session(&stored)?;
            stored
        };
        Ok(serde_json::to_value(updated)?)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[tokio::test]
    async fn owned_version_probe_accepts_completed_metadata_command() {
        use std::os::unix::fs::PermissionsExt;
        let folder = std::env::temp_dir().join(format!("mpac-version-{}", model::id()));
        fs::create_dir(&folder).unwrap();
        let path = folder.join("cli");
        fs::write(&path, "#!/bin/sh\nprintf '2.1.318 (Claude Code)\\n'\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let found = tokio::task::spawn_blocking(move || version(&path))
            .await
            .unwrap();
        fs::remove_dir_all(folder).unwrap();
        assert!(
            found == Some(Version(2, 1, 318)),
            "owned metadata probe failed"
        );
    }
    #[test]
    fn versions_and_secrets_are_strict() {
        assert!(parse_version("2.1.318 (Claude Code)") == Some(Version(2, 1, 318)));
        assert!(parse_version("codex-cli 0.166.0") == Some(Version(0, 166, 0)));
        assert!(parse_version("not a version").is_none());
        assert!(same_secret("private-value", "private-value"));
        assert!(!same_secret("private-value", "private-other"));
        assert!(!same_secret("private-value", "private-value-long"));
    }
}
