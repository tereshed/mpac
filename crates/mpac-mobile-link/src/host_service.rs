use crate::connection::{DesktopConfig, NativeClient};
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use fs2::FileExt;
use mpac_mobile_link::{
    Offer, equal,
    pairing::{Device, PAIR_SECONDS, Pairing},
    safe_api_path, secret,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

struct Host {
    client: NativeClient,
    pairing: Mutex<Pairing>,
    state_dir: PathBuf,
    telegram_state_dir: PathBuf,
    endpoint: String,
    certificate: Vec<u8>,
    admin_token: String,
    admin_host: String,
    remote_host: String,
    slots: Arc<tokio::sync::Semaphore>,
    share_page: Option<String>,
    #[cfg(target_os = "macos")]
    telegram: Mutex<Option<crate::telegram::Telegram>>,
}
fn computer_name() -> String {
    let mut name = [0u8; 256];
    // gethostname writes at most the provided length; no shell or environment file is evaluated.
    if unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len()) } != 0 {
        return "MPAC computer".into();
    }
    let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
    let label: String = String::from_utf8_lossy(&name[..end])
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect();
    if label.trim().is_empty() {
        "MPAC computer".into()
    } else {
        label
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::{
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    let staging = path.with_extension("new");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o600))?;
    fs::rename(staging, path)?;
    Ok(())
}
fn private_read(path: &Path, max: u64) -> Result<Vec<u8>> {
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file()
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o777 == 0o600
            && meta.len() <= max,
        "Unsafe companion file"
    );
    let mut bytes = Vec::new();
    file.by_ref().take(max + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= max, "Companion file too large");
    Ok(bytes)
}
fn save_devices(host: &Host, devices: &[Device]) -> Result<()> {
    private_write(
        &host.state_dir.join("devices.json"),
        &serde_json::to_vec(devices)?,
    )
}
fn bearer(headers: &HeaderMap) -> &str {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
}
fn admin(host: &Host, headers: &HeaderMap) -> bool {
    headers.get("host").and_then(|v| v.to_str().ok()) == Some(&host.admin_host)
        && headers
            .get("origin")
            .is_none_or(|v| v.to_str().ok() == Some(format!("http://{}", host.admin_host).as_str()))
        && equal(bearer(headers), &host.admin_token)
}
fn remote_peer(host: &Host, headers: &HeaderMap) -> bool {
    // Native client only. No browser origin receives cross-origin credentials.
    headers.get("host").and_then(|v| v.to_str().ok()) == Some(&host.remote_host)
        && !headers.contains_key("origin")
}
fn failure(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error":message}))).into_response()
}
async fn admin_state(State(host): State<Arc<Host>>, headers: HeaderMap) -> Response {
    if !admin(&host, &headers) {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Local administration requires its private link",
        );
    }
    let pairing = host.pairing.lock().unwrap();
    Json(json!({"endpoint":host.endpoint,"share_page":host.share_page,"pending":pairing.pending(now()),"devices":pairing.devices.iter().map(|d|json!({"id":d.id,"name":d.name,"approved_at":d.approved_at})).collect::<Vec<_>>()})).into_response()
}
async fn invite(State(host): State<Arc<Host>>, headers: HeaderMap) -> Response {
    if !admin(&host, &headers) {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Local administration requires its private link",
        );
    }
    let invite = host.pairing.lock().unwrap().invite(now());
    let offer = Offer {
        endpoint: host.endpoint.clone(),
        certificate: host.certificate.clone(),
        invite,
    };
    let Ok(link) = offer.link() else {
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not prepare invitation",
        );
    };
    let Ok(qr) = qrcode::QrCode::new(link.as_bytes()) else {
        return failure(StatusCode::INTERNAL_SERVER_ERROR, "Could not prepare QR");
    };
    let svg = qr
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(280, 280)
        .build();
    let shared = host
        .share_page
        .as_ref()
        .and_then(|page| mpac_mobile_link::sharing::link(page, &link).ok());
    let telegram = shared
        .as_ref()
        .and_then(|link| mpac_mobile_link::sharing::telegram(link).ok());
    Json(json!({"link":link,"share_link":shared,"telegram_url":telegram,"qr":svg,"expires_at":now()+PAIR_SECONDS})).into_response()
}

#[cfg(target_os = "macos")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TelegramAction {
    token: Option<String>,
    chat_id: Option<i64>,
    kind: Option<String>,
    request_id: Option<String>,
}
#[cfg(target_os = "macos")]
async fn telegram(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    axum::extract::Path(action): axum::extract::Path<String>,
    Json(body): Json<TelegramAction>,
) -> Response {
    if !admin(&host, &headers) {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Local administration requires its private link",
        );
    }
    let result = tokio::task::spawn_blocking(move || -> Result<Value> {
        let mut state = host.telegram.lock().unwrap();
        if state.is_none() {
            *state = Some(crate::telegram::Telegram::open(&host.telegram_state_dir)?);
        }
        let tg = state.as_mut().unwrap();
        match action.as_str() {
            "state" => Ok(tg.state(now())),
            "connect" => tg.begin(
                body.token.as_deref().context("Enter your bot token")?,
                now(),
            ),
            "check" => tg.check(now()),
            "renew" => tg.renew(now()),
            "confirm" => tg.confirm(body.chat_id.context("Select your verified chat")?, now()),
            "cancel" => {
                tg.cancel();
                Ok(tg.state(now()))
            }
            "disconnect" => {
                tg.disconnect()?;
                Ok(tg.state(now()))
            }
            "send" => {
                let page = host
                    .share_page
                    .as_deref()
                    .context("Configure a public HTTPS opening page first")?;
                let kind = body.kind.as_deref().context("Select a link type")?;
                ensure!(matches!(kind, "open" | "pair"), "Unknown link type");
                tg.send(
                    body.request_id.as_deref().context("Missing delivery ID")?,
                    || {
                        let link = if kind == "pair" {
                            let invite = host.pairing.lock().unwrap().invite(now());
                            let offer = Offer {
                                endpoint: host.endpoint.clone(),
                                certificate: host.certificate.clone(),
                                invite,
                            };
                            mpac_mobile_link::sharing::link(page, &offer.link()?)?
                        } else {
                            page.to_owned()
                        };
                        Ok(link)
                    },
                    kind == "pair",
                    &computer_name(),
                )
            }
            _ => anyhow::bail!("Unknown Telegram action"),
        }
    })
    .await;
    match result {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => failure(StatusCode::BAD_REQUEST, &error.to_string()),
        Err(_) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Telegram operation interrupted",
        ),
    }
}
#[cfg(target_os = "linux")]
async fn telegram(State(host): State<Arc<Host>>, headers: HeaderMap) -> Response {
    if !admin(&host, &headers) {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Private local administration required",
        );
    }
    failure(
        StatusCode::NOT_IMPLEMENTED,
        "Telegram setup is currently available in the desktop app. Use the server administration command to create an invitation.",
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    id: String,
    code: String,
}
async fn approve(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Json(body): Json<Approval>,
) -> Response {
    if !admin(&host, &headers) {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Local administration requires its private link",
        );
    }
    let mut pairing = host.pairing.lock().unwrap();
    let before = pairing.devices.clone();
    match pairing.approve(&body.id, &body.code, now()) {
        Ok(_) => {
            if save_devices(&host, &pairing.devices).is_ok() {
                Json(json!({"ok":true})).into_response()
            } else {
                pairing.devices = before;
                failure(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not save device; create a new invitation",
                )
            }
        }
        Err(_) => failure(
            StatusCode::CONFLICT,
            "Pairing request expired or fingerprint changed",
        ),
    }
}
async fn reject(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Json(body): Json<Approval>,
) -> Response {
    if !admin(&host, &headers) {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Local administration requires its private link",
        );
    }
    match host
        .pairing
        .lock()
        .unwrap()
        .reject(&body.id, &body.code, now())
    {
        Ok(()) => Json(json!({"ok":true})).into_response(),
        Err(_) => failure(
            StatusCode::CONFLICT,
            "Pairing request expired or fingerprint changed",
        ),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceId {
    id: String,
}
async fn revoke(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Json(body): Json<DeviceId>,
) -> Response {
    if !admin(&host, &headers) {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Local administration requires its private link",
        );
    }
    let mut pairing = host.pairing.lock().unwrap();
    let before = pairing.devices.clone();
    pairing.revoke(&body.id);
    if save_devices(&host, &pairing.devices).is_err() {
        pairing.devices = before;
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not save device revocation",
        );
    }
    Json(json!({"ok":true})).into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairRequest {
    invite: String,
    name: String,
}
async fn pair_request(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Json(body): Json<PairRequest>,
) -> Response {
    if !remote_peer(&host, &headers) {
        return failure(StatusCode::FORBIDDEN, "Native pairing client required");
    }
    match host
        .pairing
        .lock()
        .unwrap()
        .request(&body.invite, bearer(&headers), &body.name, now())
    {
        Ok(pending) => {
            Json(json!({"id":pending.id,"code":pending.code,"expires_at":pending.expires_at}))
                .into_response()
        }
        Err(_) => failure(
            StatusCode::FORBIDDEN,
            "Invitation expired, already used, or invalid",
        ),
    }
}
async fn pair_status(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Json(body): Json<DeviceId>,
) -> Response {
    if !remote_peer(&host, &headers) {
        return failure(StatusCode::FORBIDDEN, "Native pairing client required");
    }
    let pairing = host.pairing.lock().unwrap();
    Json(json!({"status":pairing.status(bearer(&headers),&body.id,now()),"computer_name":computer_name()})).into_response()
}
async fn proxy(State(host): State<Arc<Host>>, request: Request) -> Response {
    if !remote_peer(&host, request.headers()) {
        return failure(StatusCode::FORBIDDEN, "Native mobile client required");
    }
    let key = bearer(request.headers()).to_owned();
    if !host.pairing.lock().unwrap().authenticated(&key) {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Device not paired or access revoked",
        );
    }
    let path = request
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("")
        .to_owned();
    let method = request.method().as_str().to_owned();
    if !safe_api_path(&path)
        || !matches!(method.as_str(), "GET" | "POST")
        || path.split('?').next().unwrap_or("").ends_with("/resize")
    {
        return failure(
            StatusCode::FORBIDDEN,
            "This mobile preview does not resize the shared terminal",
        );
    }
    let Ok(permit) = host.slots.clone().try_acquire_owned() else {
        return failure(StatusCode::TOO_MANY_REQUESTS, "Mobile connection is busy");
    };
    let Ok(body) = axum::body::to_bytes(request.into_body(), 16 * 1024 * 1024).await else {
        return failure(StatusCode::PAYLOAD_TOO_LARGE, "Mobile request too large");
    };
    // Authentication is checked again at dispatch; revoke is serialized with
    // already-started requests. There are no persistent remote streams yet.
    let result=tokio::task::spawn_blocking(move || {
        let _permit=permit;
        if !host.pairing.lock().unwrap().authenticated(&key){return failure(StatusCode::UNAUTHORIZED,"Device access revoked")}
        let reply=host.client.request(&method,&path,&body);
        // Revoke never waits for upstream I/O. A command already dispatched
        // may have completed, but no further result is disclosed to that key.
        if !host.pairing.lock().unwrap().authenticated(&key){return failure(StatusCode::UNAUTHORIZED,"Device access revoked")}
        match reply {
            Ok(mut reply) if !(300..400).contains(&reply.status)=>{
                if path=="/api/state" && reply.status==200 {
                    let Ok(mut state)=serde_json::from_slice::<Value>(&reply.body) else{return failure(StatusCode::BAD_GATEWAY,"Invalid computer state")};
                    state["capabilities"]["terminal_stream"]=json!(false);
                    state["capabilities"]["terminal_resize"]=json!(false);
                    state["capabilities"]["mobile_remote"]=json!(true);
                    reply.body=serde_json::to_vec(&state).unwrap();
                }
                (StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_GATEWAY),[("content-type",reply.content_type),("cache-control","no-store".into())],reply.body).into_response()
            },
            _=>failure(StatusCode::BAD_GATEWAY,"Computer service changed or disconnected. Reconnect the companion on the computer; no command was retried.")
        }
    }).await;
    result.unwrap_or_else(|_| {
        failure(
            StatusCode::BAD_GATEWAY,
            "Computer connection interrupted; check before sending again",
        )
    })
}
/// Headless administration over the attested, already-connected loopback socket.
/// Invitations are written to a private file, never to service logs or argv.
pub fn admin_cli() -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let mut args = std::env::args().skip(2);
    ensure!(
        args.next().as_deref() == Some("--state-dir"),
        "Usage: mpac-mobile-host admin --state-dir PRIVATE_DIR state|invite --output PRIVATE_FILE|approve REQUEST_ID CODE|reject REQUEST_ID CODE|revoke DEVICE_ID"
    );
    let directory = PathBuf::from(args.next().context("Missing state directory")?);
    let metadata = fs::symlink_metadata(&directory)?;
    ensure!(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o777 == 0o700,
        "Use the host's private state directory"
    );
    let launch: Value =
        serde_json::from_slice(&private_read(&directory.join("launch.json"), 16384)?)?;
    let pid = u32::try_from(launch["pid"].as_u64().context("Invalid host process")?)?;
    let url = launch["admin_url"]
        .as_str()
        .context("Missing local administration URL")?
        .parse()?;
    let command = args.next().context("Missing administration command")?;
    let mut output = None;
    let (path, body) = match command.as_str() {
        "state" => ("/admin/state", json!({})),
        "invite" => {
            ensure!(
                args.next().as_deref() == Some("--output"),
                "Invitation requires --output PRIVATE_FILE"
            );
            let file = PathBuf::from(args.next().context("Missing output file")?);
            let parent = file
                .parent()
                .context("Use an absolute private output path")?;
            let meta = fs::symlink_metadata(parent)?;
            ensure!(
                file.is_absolute()
                    && meta.is_dir()
                    && !meta.file_type().is_symlink()
                    && meta.uid() == unsafe { libc::geteuid() }
                    && meta.mode() & 0o777 == 0o700,
                "Invitation output requires a private directory (0700)"
            );
            output = Some(file);
            ("/admin/invite", json!({}))
        }
        "approve" | "reject" => {
            let id = args.next().context("Missing request ID")?;
            let code = args.next().context("Missing code shown on the device")?;
            (
                if command == "approve" {
                    "/admin/approve"
                } else {
                    "/admin/reject"
                },
                json!({"id":id,"code":code}),
            )
        }
        "revoke" => (
            "/admin/revoke",
            json!({"id":args.next().context("Missing device ID")?}),
        ),
        _ => anyhow::bail!("Unknown administration command"),
    };
    ensure!(args.next().is_none(), "Unexpected administration argument");
    let response =
        crate::connection::companion_request(pid, &url, path, &serde_json::to_vec(&body)?)?;
    ensure!(
        (200..300).contains(&response.status),
        "Host rejected administration request (HTTP {})",
        response.status
    );
    if let Some(output) = output {
        private_write(&output, &response.body)?;
        println!("Invitation saved to a private file; valid for {PAIR_SECONDS} seconds.");
    } else {
        println!("{}", String::from_utf8(response.body)?);
    }
    Ok(())
}

pub fn run() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut data = None;
    let mut state_dir = None;
    let mut telegram_state_dir: Option<PathBuf> = None;
    let mut bind = "127.0.0.1:0".parse::<SocketAddr>()?;
    let mut advertised = None;
    let mut open_control = false;
    let mut share_page = None;
    let mut parent_pid: Option<u32> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--parent-pid" => {
                let pid = args.next().context("Missing parent process")?.parse()?;
                ensure!(
                    pid > 1 && pid == unsafe { libc::getppid() } as u32,
                    "Companion parent does not match its launcher"
                );
                parent_pid = Some(pid);
            }
            "--open-control" => open_control = true,
            "--share-url" => {
                share_page = Some(
                    mpac_mobile_link::sharing::page(&args.next().context("Missing share URL")?)?
                        .to_string(),
                )
            }
            "--data-dir" => data = Some(PathBuf::from(args.next().context("Missing data-dir")?)),
            "--state-dir" => {
                state_dir = Some(PathBuf::from(args.next().context("Missing state-dir")?))
            }
            "--bind" => bind = args.next().context("Missing bind address")?.parse()?,
            "--telegram-state-dir" => {
                telegram_state_dir = Some(PathBuf::from(
                    args.next().context("Missing Telegram state directory")?,
                ))
            }
            "--advertise-host" => advertised = Some(args.next().context("Missing advertise-host")?),
            "--help" => {
                println!(
                    "mpac-mobile-host --data-dir DAEMON_DATA --state-dir PRIVATE_DIR [--bind LOCAL_IP:PORT] [--advertise-host LOCAL_IP] [--share-url https://YOUR_DOMAIN/open/]\nDefault: loopback only. A LAN listener requires an explicit --bind address."
                );
                return Ok(());
            }
            _ => anyhow::bail!("Unknown companion argument"),
        }
    }
    let data = data
        .context("Pass the existing daemon --data-dir")?
        .canonicalize()?;
    let state_dir = state_dir.context("Pass a separate --state-dir")?;
    ensure!(
        bind.ip().is_ipv4() && !bind.ip().is_unspecified(),
        "Bind one explicit local IPv4 interface"
    );
    let advertised = advertised.unwrap_or_else(|| bind.ip().to_string());
    let advertised_ip: Ipv4Addr = advertised.parse()?;
    ensure!(
        mpac_mobile_link::private_host_address(advertised_ip),
        "Advertise one local or private overlay network address"
    );
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    if !state_dir.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(&state_dir)?;
    }
    let meta = fs::symlink_metadata(&state_dir)?;
    ensure!(
        meta.is_dir()
            && !meta.file_type().is_symlink()
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o777 == 0o700,
        "Use a private companion state directory (0700)"
    );
    let state_dir = state_dir.canonicalize()?;
    let telegram_state_dir = telegram_state_dir.unwrap_or_else(|| state_dir.clone());
    let telegram_meta = fs::symlink_metadata(&telegram_state_dir)?;
    ensure!(
        telegram_meta.is_dir()
            && !telegram_meta.file_type().is_symlink()
            && telegram_meta.uid() == unsafe { libc::geteuid() }
            && telegram_meta.mode() & 0o077 == 0,
        "Use a private Telegram state directory"
    );
    let telegram_state_dir = telegram_state_dir.canonicalize()?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(state_dir.join("host.lock"))?;
    lock.try_lock_exclusive()
        .context("Companion state already in use")?;
    let config = DesktopConfig { data_dir: data };
    // An absent executable guarantees this companion never starts a daemon.
    let connection = crate::connection::connect_or_start(
        &config,
        Path::new("/mpac-mobile-no-spawn"),
        Path::new("/mpac-mobile-no-ui"),
    )?;
    let client = NativeClient::new(config, connection)?;
    let tls_listener = TcpListener::bind(bind)?;
    tls_listener.set_nonblocking(true)?;
    let port = tls_listener.local_addr()?.port();
    let endpoint = format!("https://{advertised}:{port}/");
    mpac_mobile_link::endpoint(&endpoint)?;
    let cert_file = state_dir.join("tls.json");
    let (certificate, pem, key) = if cert_file.exists() {
        let saved: Value = serde_json::from_slice(&private_read(&cert_file, 16 * 1024)?)?;
        ensure!(
            saved["host"].as_str() == Some(&advertised),
            "Companion address changed; use a new state directory and pair explicitly"
        );
        (
            saved["certificate"]
                .as_array()
                .context("Invalid certificate")?
                .iter()
                .map(|n| {
                    n.as_u64()
                        .filter(|n| *n <= 255)
                        .map(|n| n as u8)
                        .context("Invalid certificate")
                })
                .collect::<Result<Vec<_>>>()?,
            saved["pem"]
                .as_str()
                .context("Invalid TLS state")?
                .to_owned(),
            saved["key"]
                .as_str()
                .context("Invalid TLS state")?
                .to_owned(),
        )
    } else {
        let generated = rcgen::generate_simple_self_signed(vec![advertised.clone()])?;
        let certificate = generated.cert.der().to_vec();
        let pem = generated.cert.pem();
        let key = generated.signing_key.serialize_pem();
        private_write(
            &cert_file,
            &serde_json::to_vec(
                &json!({"host":advertised,"certificate":certificate,"pem":pem,"key":key}),
            )?,
        )?;
        (certificate, pem, key)
    };
    let devices_file = state_dir.join("devices.json");
    let devices = if devices_file.exists() {
        serde_json::from_slice(&private_read(&devices_file, 64 * 1024)?)?
    } else {
        Vec::new()
    };
    let admin_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    admin_listener.set_nonblocking(true)?;
    let admin_host = format!("127.0.0.1:{}", admin_listener.local_addr()?.port());
    let admin_token = secret();
    let launch = json!({"pid":std::process::id(),"endpoint":endpoint,"admin_url":format!("http://{admin_host}/#token={admin_token}")});
    private_write(
        &state_dir.join("launch.json"),
        &serde_json::to_vec_pretty(&launch)?,
    )?;
    // Open a private local file, keeping the administration key out of argv.
    let control_file = state_dir.join("connect-phone.html");
    private_write(&control_file,format!("<!doctype html><meta name=referrer content=no-referrer><meta http-equiv=refresh content=\"0;url={}\"><title>MPAC · Connect phone</title><a href=\"{}\">Open phone connections</a>",launch["admin_url"].as_str().unwrap(),launch["admin_url"].as_str().unwrap()).as_bytes())?;
    let host = Arc::new(Host {
        client,
        pairing: Mutex::new(Pairing::from_devices(devices)),
        state_dir: state_dir.clone(),
        telegram_state_dir,
        endpoint,
        certificate,
        admin_token,
        admin_host,
        remote_host: format!("{advertised}:{port}"),
        slots: Arc::new(tokio::sync::Semaphore::new(8)),
        share_page,
        #[cfg(target_os = "macos")]
        telegram: Mutex::new(None),
    });
    println!(
        "MPAC mobile companion ready. Open the private admin_url in {}. No pairing credentials are printed.",
        state_dir.join("launch.json").display()
    );
    // reqwest's blocking client owns a runtime. Keep its last Arc outside the
    // async context so shutdown never drops that runtime from a Tokio task.
    let _host_lifetime = host.clone();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let tls=axum_server::tls_rustls::RustlsConfig::from_pem(pem.into_bytes(),key.into_bytes()).await?;
        let remote=Router::new().route("/pair/request",post(pair_request).layer(DefaultBodyLimit::max(4096))).route("/pair/status",post(pair_status).layer(DefaultBodyLimit::max(4096))).route("/api/{*path}",get(proxy).post(proxy))
            .layer(DefaultBodyLimit::max(16*1024*1024)).with_state(host.clone());
        let admin_ui=Router::new().route("/",get(||async{Html(include_str!("host.html"))}))
            .route("/pairing",get(||async{Html(include_str!("pairing.html"))}))
            .route("/admin/state",get(admin_state).post(admin_state)).route("/admin/invite",post(invite)).route("/admin/approve",post(approve)).route("/admin/reject",post(reject)).route("/admin/revoke",post(revoke))
            .route("/admin/telegram/{action}",post(telegram))
            .layer(DefaultBodyLimit::max(4096)).with_state(host);
        let admin_listener=tokio::net::TcpListener::from_std(admin_listener)?;
        let server=axum_server::from_tcp_rustls(tls_listener,tls)?.serve(remote.into_make_service());
        let admin=axum::serve(admin_listener,admin_ui);
        if open_control {
            std::process::Command::new("/usr/bin/open").arg(&control_file).spawn().context("Could not open local phone controls")?;
        }
        let parent_exit=async move {
            if let Some(pid)=parent_pid {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if unsafe {libc::getppid()} as u32 != pid {break;}
                }
            } else {std::future::pending::<()>().await;}
        };
        tokio::select! {r=server=>r.context("TLS service stopped")?,r=admin=>r.context("Local administration stopped")?,_=tokio::signal::ctrl_c()=>{},_=parent_exit=>{}}
        anyhow::Ok(())
    })?;
    let _ = fs::remove_file(state_dir.join("launch.json"));
    drop(lock);
    Ok(())
}
