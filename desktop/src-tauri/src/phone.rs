//! Mobile gateway management; never stops or restarts the workspace daemon.
use crate::{
    connection::{NativeClient, NativeResponse, VerifiedTerminal},
    gateway::Transport,
    settings,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, webview::NewWindowResponse};

struct Running {
    process: Process,
    data: PathBuf,
    ip: Ipv4Addr,
    page: String,
    admin: url::Url,
}
enum Process {
    Child(Child),
    Service {
        job: crate::phone_service::Service,
        pid: u32,
    },
}
impl Process {
    fn pid(&self) -> u32 {
        match self {
            Self::Child(c) => c.id(),
            Self::Service { pid, .. } => *pid,
        }
    }
    fn alive(&mut self) -> bool {
        match self {
            Self::Child(c) => c.try_wait().ok().is_some_and(|s| s.is_none()),
            Self::Service { job, pid } => job.pid() == Some(*pid),
        }
    }
    fn stop(&mut self) -> Result<()> {
        match self {
            Self::Child(c) => {
                if c.try_wait()?.is_none() {
                    c.kill()?;
                }
                c.wait()?;
            }
            Self::Service { job, .. } => job.stop()?,
        }
        Ok(())
    }
}
pub struct Phone {
    profile: PathBuf,
    executable: PathBuf,
    running: Mutex<Option<Running>>,
    shutting_down: AtomicBool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Open {
    ip: Ipv4Addr,
    share_page: String,
    #[serde(default)]
    background: bool,
}
impl Phone {
    pub fn new(profile: PathBuf, executable: PathBuf) -> Self {
        Self {
            profile,
            executable,
            running: Mutex::new(None),
            shutting_down: AtomicBool::new(false),
        }
    }
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        let mut state = self.running.lock().unwrap();
        // launchd owns background access; quitting the GUI must not stop it.
        if let Some(r) = state.as_mut()
            && matches!(r.process, Process::Child(_))
        {
            let _ = r.process.stop();
        }
        *state = None;
    }
    /// Watch the owned companion even when both workspace and settings are hidden.
    /// No invitations, Telegram messages or approvals are created by this watcher.
    pub fn watch(self: &Arc<Self>, app: tauri::AppHandle) {
        let phone = Arc::downgrade(self);
        std::thread::spawn(move || {
            let mut shown: Option<(u32, String)> = None;
            loop {
                let Some(phone) = phone.upgrade() else { break };
                if phone.shutting_down.load(Ordering::Acquire) {
                    break;
                }
                let owner = phone.owner();
                match owner {
                    None => {
                        close_prompt(&app);
                        shown = None;
                    }
                    Some((pid, admin)) => {
                        if let Ok(reply) =
                            crate::connection::companion_request(pid, &admin, "/admin/state", b"{}")
                            && reply.status == 200
                            && let Ok(state) = serde_json::from_slice::<PairingState>(&reply.body)
                            && phone
                                .owner()
                                .is_some_and(|current| current == (pid, admin.clone()))
                        {
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            let current = shown
                                .as_ref()
                                .filter(|(owner, _)| *owner == pid)
                                .map(|(_, id)| id.as_str());
                            let next =
                                next_prompt(state.pending, now, current).map(|p| (pid, p.id));
                            if next != shown {
                                close_prompt(&app);
                                shown = None;
                                if let Some((pid, id)) = next
                                    && show_prompt(&app, &admin, &id).is_ok()
                                {
                                    shown = Some((pid, id));
                                }
                            }
                        }
                    }
                }
                drop(phone);
                std::thread::sleep(Duration::from_millis(1200));
            }
        });
    }
    fn owner(&self) -> Option<(u32, url::Url)> {
        let recovery = {
            let mut state = self.running.lock().unwrap();
            state.as_mut().and_then(|r| {
                (matches!(r.process, Process::Service { .. }) && !r.process.alive())
                    .then(|| r.data.clone())
            })
        };
        if let Some(data) = recovery {
            let _ = self.restore(&data);
        }
        let mut state = self.running.lock().unwrap();
        state.as_mut().and_then(|r| {
            r.process
                .alive()
                .then(|| (r.process.pid(), r.admin.clone()))
        })
    }
    pub fn stop(&self) -> Result<()> {
        let mut state = self.running.lock().unwrap();
        if let Some(r) = state.as_mut() {
            r.process.stop()?;
            if let Ok(value) = read_launch(&self.preference_path(&r.data)) {
                let mut input: Open = serde_json::from_value(value)?;
                input.background = false;
                save_preference(&self.preference_path(&r.data), &input)?;
            }
        }
        *state = None;
        Ok(())
    }
    pub fn restore(&self, data: &Path) -> Result<()> {
        let data = data.canonicalize()?;
        if let Ok(value) = read_launch(&self.preference_path(&data)) {
            let input: Open = serde_json::from_value(value)?;
            if input.background {
                self.start(&data, input)?;
            }
        }
        Ok(())
    }
    fn state(&self, data: &Path) -> Value {
        let _ = self.restore(data);
        let mut state = self.running.lock().unwrap();
        if state.as_mut().is_some_and(|r| !r.process.alive()) {
            *state = None;
        }
        let preference = data
            .canonicalize()
            .ok()
            .and_then(|data| read_launch(&self.preference_path(&data)).ok());
        let available = interfaces();
        let preferred = preference
            .as_ref()
            .and_then(|p| p["ip"].as_str())
            .filter(|ip| available.iter().any(|a| a == ip));
        json!({"running":state.is_some(),"ip":state.as_ref().map(|r|r.ip.to_string()).or_else(|| preferred.map(str::to_owned)),"share_page":state.as_ref().map(|r|r.page.as_str()).or_else(|| preference.as_ref().and_then(|p|p["share_page"].as_str())),"background":state.as_ref().map(|r|matches!(r.process,Process::Service{..})).unwrap_or_else(||preference.as_ref().and_then(|p|p["background"].as_bool()).unwrap_or(false)),"interfaces":available})
    }
    fn start(&self, data: &Path, input: Open) -> Result<url::Url> {
        ensure!(lan_ip(input.ip), "Select a private IPv4 network interface");
        ensure!(
            interfaces().contains(&input.ip.to_string()),
            "This address is not assigned to this computer"
        );
        let page = mpac_mobile_link::sharing::page(&input.share_page)?.to_string();
        let data = data.canonicalize()?;
        let mut state = self.running.lock().unwrap();
        if let Some(r) = state.as_mut()
            && !r.process.alive()
        {
            *state = None;
        }
        if let Some(r) = state.as_ref() {
            ensure!(
                r.data == data
                    && r.ip == input.ip
                    && r.page == page
                    && matches!(r.process, Process::Service { .. }) == input.background,
                "Stop the current phone connection before changing its network or workspace"
            );
        } else {
            settings::private_directory(&self.profile)?;
            let directory = self.profile.join(format!(
                "phone-{}-{}",
                mpac_mobile_link::secret_hash(&data.to_string_lossy()),
                input.ip
            ));
            settings::private_directory(&directory)?;
            let telegram_source = directory.join("telegram-source.json");
            let source = if let Ok(saved) = read_launch(&telegram_source) {
                PathBuf::from(saved["path"].as_str().context("Invalid Telegram scope")?)
            } else {
                let previous: Option<Open> = read_launch(&self.preference_path(&data))
                    .ok()
                    .and_then(|p| serde_json::from_value(p).ok());
                let previous_dir = previous.map(|p| {
                    self.profile.join(format!(
                        "phone-{}-{}",
                        mpac_mobile_link::secret_hash(&data.to_string_lossy()),
                        p.ip
                    ))
                });
                let source = previous_dir
                    .filter(|p| p.exists())
                    .unwrap_or_else(|| directory.clone());
                let source = read_launch(&source.join("telegram-source.json"))
                    .ok()
                    .and_then(|p| p["path"].as_str().map(PathBuf::from))
                    .unwrap_or(source);
                private_save(
                    &telegram_source,
                    &serde_json::to_vec(&json!({"path":source}))?,
                )?;
                source
            };
            settings::private_directory(&source)?;
            let mut arguments = vec![
                "--data-dir".into(),
                data.to_string_lossy().into_owned(),
                "--state-dir".into(),
                directory.to_string_lossy().into_owned(),
                "--bind".into(),
                format!("{}:4330", input.ip),
                "--share-url".into(),
                page.clone(),
                "--telegram-state-dir".into(),
                source.to_string_lossy().into_owned(),
            ];
            let mut process = if input.background {
                let job = crate::phone_service::Service::new(
                    &self.profile,
                    &data,
                    &input.ip.to_string(),
                )?;
                job.start(&self.executable, &arguments)?;
                Process::Service { job, pid: 0 }
            } else {
                arguments.extend(["--parent-pid".into(), std::process::id().to_string()]);
                Process::Child(
                    Command::new(&self.executable)
                        .args(&arguments)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .context("Could not start the bundled phone companion")?,
                )
            };
            let started = Instant::now();
            let result = (|| -> Result<url::Url> {
                while started.elapsed() < Duration::from_secs(12) {
                    match &mut process {
                        Process::Child(child) => ensure!(
                            child.try_wait()?.is_none(),
                            "Phone companion could not start. Check port 4330 and macOS permissions."
                        ),
                        Process::Service { job, pid } => {
                            *pid = job.pid().unwrap_or(0);
                        }
                    }
                    if let Ok(launch) = read_launch(&directory.join("launch.json"))
                        && launch["pid"].as_u64() == Some(process.pid() as u64)
                    {
                        let admin = url::Url::parse(
                            launch["admin_url"]
                                .as_str()
                                .context("Invalid companion controls")?,
                        )?;
                        ensure!(
                            admin.scheme() == "http"
                                && admin.host_str() == Some("127.0.0.1")
                                && admin.port().is_some()
                                && admin.path() == "/"
                                && admin.username().is_empty()
                                && admin.password().is_none()
                                && admin.query().is_none()
                                && admin.fragment().is_some_and(|s| s.starts_with("token=")),
                            "Invalid companion controls"
                        );
                        ensure!(
                            launch["endpoint"].as_str()
                                == Some(format!("https://{}:4330/", input.ip).as_str()),
                            "Companion address mismatch"
                        );
                        // The loopback socket must belong to our Child or launchd job.
                        let response = crate::connection::companion_request(
                            process.pid(),
                            &admin,
                            "/admin/state",
                            b"{}",
                        )?;
                        ensure!(
                            response.status == 200,
                            "Phone companion identity check failed"
                        );
                        return Ok(admin);
                    }
                    std::thread::sleep(Duration::from_millis(60));
                }
                anyhow::bail!("Phone companion did not become ready")
            })();
            let admin = match result {
                Ok(a) => a,
                Err(e) => {
                    // Roll back registration if the first launch failed.
                    let _ = process.stop();
                    return Err(e);
                }
            };
            *state = Some(Running {
                process,
                data: data.clone(),
                ip: input.ip,
                page,
                admin,
            });
        }
        let admin = state.as_ref().unwrap().admin.clone();
        save_preference(&self.preference_path(&data), &input)?;
        Ok(admin)
    }
    fn preference_path(&self, data: &Path) -> PathBuf {
        self.profile.join(format!(
            "phone-config-{}.json",
            mpac_mobile_link::secret_hash(&data.to_string_lossy())
        ))
    }
    fn send_link(&self, data: &Path, body: &[u8]) -> Result<NativeResponse> {
        let payload = send_payload(body)?;
        let data = data.canonicalize()?;
        let needs_start = {
            let mut state = self.running.lock().unwrap();
            state.as_mut().is_none_or(|r| !r.process.alive())
        };
        if needs_start {
            let value = read_launch(&self.preference_path(&data))
                .context("Open Phone & Telegram to configure the connection first.")?;
            let input: Open = serde_json::from_value(value)?;
            self.start(&data, input)?;
        }
        let mut state = self.running.lock().unwrap();
        let running = state
            .as_mut()
            .context("Open Phone & Telegram to configure the connection first.")?;
        ensure!(
            running.data == data && running.process.alive(),
            "Open Phone & Telegram to configure the connection first."
        );
        crate::connection::companion_request(
            running.process.pid(),
            &running.admin,
            "/admin/telegram/send",
            &payload,
        )
        .map_err(|_| {
            anyhow::anyhow!(
                "Telegram delivery was not confirmed. Check your chat before trying again."
            )
        })
    }
    fn open(&self, app: &tauri::AppHandle, data: &Path, body: &[u8]) -> Result<Value> {
        let input: Open = serde_json::from_slice(body)?;
        let admin = self.start(data, input)?;
        if let Some(window) = app.get_webview_window("phone-controls") {
            window.show()?;
            window.set_focus()?;
            return Ok(json!({"opened":true}));
        }
        let origin = admin.origin();
        WebviewWindowBuilder::new(app, "phone-controls", WebviewUrl::External(admin))
            .title("MPAC · Phone & Telegram")
            .inner_size(850.0, 850.0)
            .min_inner_size(390.0, 500.0)
            .on_navigation(move |url| {
                url.origin() == origin && url.path() == "/" && url.query().is_none()
            })
            .on_new_window(|url, _| {
                if url.scheme() == "https"
                    && url.host_str() == Some("t.me")
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.port().is_none()
                {
                    #[cfg(target_os = "macos")]
                    let _ = Command::new("/usr/bin/open").arg(url.as_str()).spawn();
                }
                NewWindowResponse::Deny
            })
            .on_download(|_, _| false)
            .devtools(false)
            .build()?;
        Ok(json!({"opened":true}))
    }
}

#[derive(Deserialize)]
struct PairingState {
    pending: Vec<PairingPrompt>,
}
#[derive(Deserialize)]
struct PairingPrompt {
    id: String,
    code: String,
    expires_at: u64,
}
fn next_prompt(
    mut pending: Vec<PairingPrompt>,
    now: u64,
    current: Option<&str>,
) -> Option<PairingPrompt> {
    pending.retain(|p| {
        p.expires_at > now
            && uuid::Uuid::parse_str(&p.id).is_ok()
            && p.code.len() == 6
            && p.code.bytes().all(|b| b.is_ascii_digit())
    });
    // Never replace a live prompt under the user's pointer as other requests arrive.
    if let Some(index) = pending.iter().position(|p| Some(p.id.as_str()) == current) {
        return Some(pending.remove(index));
    }
    pending.sort_by(|a, b| (a.expires_at, &a.id).cmp(&(b.expires_at, &b.id)));
    pending.into_iter().next()
}
fn close_prompt(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("phone-pairing") {
        let _ = window.close();
    }
}
fn show_prompt(app: &tauri::AppHandle, admin: &url::Url, id: &str) -> Result<()> {
    let mut page = admin.clone();
    page.set_path("/pairing");
    let fragment = format!(
        "{}&id={id}",
        page.fragment().context("Missing controls credential")?
    );
    page.set_fragment(Some(&fragment));
    let origin = page.origin();
    WebviewWindowBuilder::new(app, "phone-pairing", WebviewUrl::External(page))
        .title("MPAC · Device connection")
        .inner_size(480.0, 535.0)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .closable(false)
        .always_on_top(true)
        .focused(true)
        .center()
        .on_navigation(move |url| {
            url.origin() == origin && url.path() == "/pairing" && url.query().is_none()
        })
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .on_download(|_, _| false)
        .devtools(false)
        .build()?;
    Ok(())
}
pub struct DesktopTransport {
    pub client: NativeClient,
    pub data: PathBuf,
    pub phone: Arc<Phone>,
    pub app: tauri::AppHandle,
}
impl Transport for DesktopTransport {
    fn terminal_socket(&self, id: &str) -> Result<VerifiedTerminal> {
        self.client.terminal_socket(id)
    }
    fn request(&self, method: &str, path: &str, body: &[u8]) -> Result<NativeResponse> {
        if !path.starts_with("/api/native/phone/") {
            return self.client.request(method, path, body);
        }
        let result = match (method, path) {
            ("GET", "/api/native/phone/state") => Ok(self.phone.state(&self.data)),
            ("POST", "/api/native/phone/open") => self.phone.open(&self.app, &self.data, body),
            ("POST", "/api/native/phone/send-link") => {
                return self.phone.send_link(&self.data, body).or_else(|e| {
                    Ok(NativeResponse {
                        status: 400,
                        content_type: "application/json".into(),
                        body: serde_json::to_vec(&json!({"error":e.to_string()}))?,
                    })
                });
            }
            ("POST", "/api/native/phone/stop") => {
                if let Some(window) = self.app.get_webview_window("phone-controls") {
                    let _ = window.close();
                }
                self.phone.stop()?;
                Ok(json!({"stopped":true}))
            }
            _ => anyhow::bail!("Unknown phone operation"),
        };
        let (status, value) = match result {
            Ok(v) => (200, v),
            Err(e) => (400, json!({"error":e.to_string()})),
        };
        Ok(NativeResponse {
            status,
            content_type: "application/json".into(),
            body: serde_json::to_vec(&value)?,
        })
    }
}
fn send_payload(body: &[u8]) -> Result<Vec<u8>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Send {
        request_id: String,
    }
    let request: Send = serde_json::from_slice(body)?;
    ensure!(
        mpac_mobile_link::valid_secret(&request.request_id),
        "Invalid delivery ID"
    );
    Ok(serde_json::to_vec(
        &json!({"kind":"pair","request_id":request.request_id}),
    )?)
}
fn save_preference(path: &Path, input: &Open) -> Result<()> {
    private_save(path, &serde_json::to_vec(input)?)
}
pub(super) fn private_save(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let result = (|| -> Result<()> {
        use std::io::Write;
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
fn tailnet_ip(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}
fn lan_ip(ip: Ipv4Addr) -> bool {
    !ip.is_loopback() && !ip.is_unspecified() && mpac_mobile_link::private_host_address(ip)
}
fn interfaces() -> Vec<String> {
    #[cfg(target_os = "macos")]
    if let Ok(output) = Command::new("/sbin/ifconfig").output() {
        let mut addresses: Vec<_> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                if words.next() != Some("inet") {
                    return None;
                }
                let ip: Ipv4Addr = words.next()?.parse().ok()?;
                lan_ip(ip).then(|| ip.to_string())
            })
            .collect();
        addresses.sort_by_key(|address| (!tailnet_ip(address.parse().unwrap()), address.clone()));
        addresses.dedup();
        return addresses;
    }
    Vec::new()
}
fn read_launch(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&read_launch_bytes(path, 8192)?)?)
}
pub(super) fn read_launch_bytes(path: &Path, max: u64) -> Result<Vec<u8>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.len() <= max,
        "Unsafe companion launch metadata"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
            "Unsafe companion metadata permissions"
        );
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= max, "Companion metadata too large");
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prompt_queue_ignores_expired_or_invalid_requests_and_keeps_oldest_first() {
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        let state: PairingState = serde_json::from_value(json!({"pending":[
            {"id":second,"code":"987654","expires_at":300},
            {"id":uuid::Uuid::new_v4(),"code":"123456","expires_at":100},
            {"id":first,"code":"000012","expires_at":200},
            {"id":"not-an-id","code":"123456","expires_at":150},
            {"id":uuid::Uuid::new_v4(),"code":"wrong!","expires_at":150}
        ]}))
        .unwrap();
        assert_eq!(next_prompt(state.pending, 100, None).unwrap().id, first);
        let state: PairingState = serde_json::from_value(json!({"pending":[
            {"id":first,"code":"000012","expires_at":200},
            {"id":second,"code":"987654","expires_at":300}
        ]}))
        .unwrap();
        assert_eq!(
            next_prompt(state.pending, 200, Some(&first)).unwrap().id,
            second
        );
        assert!(next_prompt(Vec::new(), 100, None).is_none());
        let state: PairingState = serde_json::from_value(json!({"pending":[
            {"id":first,"code":"000012","expires_at":200},
            {"id":second,"code":"987654","expires_at":300}
        ]}))
        .unwrap();
        assert_eq!(
            next_prompt(state.pending, 100, Some(&second)).unwrap().id,
            second
        );
    }
    #[test]
    fn rejects_public_and_loopback() {
        for ip in ["0.0.0.0", "127.0.0.1", "8.8.8.8"] {
            assert!(!lan_ip(ip.parse().unwrap()))
        }
        assert!(lan_ip("192.168.42.16".parse().unwrap()));
        assert!(lan_ip("100.64.42.42".parse().unwrap()));
        assert!(lan_ip("100.64.0.1".parse().unwrap()));
        assert!(lan_ip("100.127.255.254".parse().unwrap()));
        assert!(!lan_ip("100.63.255.254".parse().unwrap()));
        assert!(!lan_ip("100.128.0.1".parse().unwrap()));
    }
    #[test]
    fn quick_send_builds_pairing_request_and_rejects_renderer_overrides() {
        let id = "a".repeat(64);
        assert_eq!(
            serde_json::from_slice::<Value>(
                &send_payload(
                    serde_json::to_vec(&json!({"request_id":id}))
                        .unwrap()
                        .as_slice()
                )
                .unwrap()
            )
            .unwrap(),
            json!({"kind":"pair","request_id":id})
        );
        for extra in ["kind", "chat_id", "token", "link"] {
            let mut body = json!({"request_id":id});
            body.as_object_mut()
                .unwrap()
                .insert(extra.to_owned(), json!("untrusted"));
            assert!(send_payload(&serde_json::to_vec(&body).unwrap()).is_err());
        }
        assert!(send_payload(b"{\"request_id\":\"bad\"}").is_err());
    }
    #[test]
    fn connection_preferences_are_private_and_contain_no_credentials() {
        let directory =
            std::env::temp_dir().join(format!("mpac-phone-preferences-{}", uuid::Uuid::new_v4()));
        settings::private_directory(&directory).unwrap();
        let path = directory.join("phone.json");
        save_preference(
            &path,
            &Open {
                ip: "192.168.42.16".parse().unwrap(),
                share_page: "https://mpac.works/open/".into(),
                background: false,
            },
        )
        .unwrap();
        let saved = read_launch(&path).unwrap();
        assert_eq!(saved.as_object().unwrap().len(), 3);
        assert_eq!(saved["ip"], "192.168.42.16");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&path, directory.join("alias.json")).unwrap();
            assert!(read_launch(&directory.join("alias.json")).is_err());
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
