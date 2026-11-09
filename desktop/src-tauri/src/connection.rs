//! Native bootstrap only: private state + OS ownership are verified before a
//! credential is written to the already-connected loopback socket.
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, TryLockError, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

const MAX_LAUNCH: u64 = 8192;
const MAX_HEADERS: usize = 16 * 1024;
const MAX_STATE: usize = 8 * 1024 * 1024;
const START_BUDGET: Duration = Duration::from_secs(15);
const MAX_NATIVE_BODY: usize = 64 * 1024 * 1024;
const NATIVE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesktopConfig {
    pub data_dir: PathBuf,
}

#[derive(Clone)]
pub struct Connection {
    pub url: Url,
    pub pid: u32,
    pub port: u16,
    pub reused: bool,
}

/// Credentials remain in Rust. Each slot owns a single non-pipelined TCP
/// connection; only its first request needs the expensive OS listener proof.
pub struct NativeClient {
    data: PathBuf,
    connection: Connection,
    token: String,
    identity: ProcessIdentity,
    _executable: File,
    lock: File,
    slots: [Mutex<Option<TcpStream>>; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcessIdentity {
    pid: u32,
    device: u64,
    inode: u64,
    started_seconds: u64,
    started_microseconds: u64,
}

pub struct VerifiedTerminal {
    pub stream: TcpStream,
    pub uri: String,
    pub token: String,
    pub origin: String,
}

pub struct NativeResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl NativeClient {
    pub fn new(config: DesktopConfig, connection: Connection) -> Result<Self> {
        let data = private_directory(&config.data_dir)?;
        let token = auth_token(&data)?.context("The local credential is unavailable")?;
        let (bytes, metadata) = read_private(&data.join("launch.json"), MAX_LAUNCH)?;
        let launch: Launch =
            serde_json::from_slice(&bytes).context("Invalid local launch metadata")?;
        validate_pinned_launch(&launch, &token, &connection)?;
        let lock = open_regular(&data.join("daemon.lock"), true, false, false, 4096)?;
        verify_identity(&data, &launch, metadata.modified()?, &lock)?;
        let (identity, executable) =
            process_observation(&data, launch.pid, metadata.modified()?, None)?;
        Ok(Self {
            data,
            connection,
            token,
            identity,
            _executable: executable,
            lock,
            slots: std::array::from_fn(|_| Mutex::new(None)),
        })
    }

    fn current_launch(&self) -> Result<(Launch, SystemTime)> {
        let token = auth_token(&self.data)?.context("The local credential is unavailable")?;
        ensure!(
            token == self.token,
            "Local service credential changed; reconnect explicitly"
        );
        let (bytes, metadata) = read_private(&self.data.join("launch.json"), MAX_LAUNCH)?;
        let launch: Launch =
            serde_json::from_slice(&bytes).context("Invalid local launch metadata")?;
        validate_pinned_launch(&launch, &token, &self.connection)?;
        let published = metadata.modified()?;
        ensure!(
            process_identity(&self.data, launch.pid, published, Some(&self._executable))?
                == self.identity,
            "Local service process changed; reconnect explicitly"
        );
        Ok((launch, published))
    }

    fn verified_socket(&self, launch: &Launch, published: SystemTime) -> Result<TcpStream> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, self.connection.port));
        let stream = TcpStream::connect_timeout(&address, Duration::from_millis(500))
            .context("The local service is not accepting connections")?;
        // Interactive input must not wait for a delayed ACK between the HTTP
        // header/body writes or between small WebSocket frames.
        stream.set_nodelay(true)?;
        // This is the actual socket that will receive Authorization. No
        // independent HTTP client may open a replacement after this proof.
        verify_identity_with_pin(
            &self.data,
            launch,
            published,
            &self.lock,
            Some(&self._executable),
        )?;
        let (current, current_time) = self.current_launch()?;
        ensure!(
            current.pid == launch.pid && current.port == launch.port && current_time == published,
            "Local launch changed during socket verification"
        );
        Ok(stream)
    }

    /// The WebSocket handshake and authentication use this exact socket. A
    /// WebSocket URL must never be passed to an independently connecting client.
    pub fn terminal_socket(&self, session_id: &str) -> Result<VerifiedTerminal> {
        let id = uuid::Uuid::parse_str(session_id).context("Invalid terminal session")?;
        ensure!(id.to_string() == session_id, "Invalid terminal session");
        let (launch, published) = self.current_launch()?;
        let stream = self.verified_socket(&launch, published)?;
        stream.set_nonblocking(true)?;
        Ok(VerifiedTerminal {
            stream,
            uri: format!(
                "ws://127.0.0.1:{}/api/sessions/{session_id}/terminal",
                self.connection.port
            ),
            token: self.token.clone(),
            origin: format!("http://127.0.0.1:{}", self.connection.port),
        })
    }

    pub fn request(&self, method: &str, path: &str, body: &[u8]) -> Result<NativeResponse> {
        validate_native_request(method, path, body)?;
        let deadline = Instant::now() + NATIVE_REQUEST_TIMEOUT;
        loop {
            for slot in &self.slots {
                let mut slot = match slot.try_lock() {
                    Ok(slot) => slot,
                    Err(TryLockError::WouldBlock) => continue,
                    Err(TryLockError::Poisoned(_)) => {
                        bail!("Native connection slot is unavailable")
                    }
                };
                let (launch, published) = self.current_launch()?;
                if slot
                    .as_ref()
                    .is_some_and(|stream| !idle_socket_healthy(stream))
                {
                    *slot = None;
                }
                if slot.is_none() {
                    *slot = Some(self.verified_socket(&launch, published)?);
                }
                // Always take ownership before writing: any error, including a
                // partial write/read, drops the socket and NEVER replays a POST.
                let mut stream = slot.take().expect("verified socket was installed");
                stream.set_write_timeout(Some(NATIVE_REQUEST_TIMEOUT))?;
                let header = format!(
                    "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                    self.connection.port,
                    self.token,
                    body.len()
                );
                stream.write_all(header.as_bytes()).context(
                    "Native request write failed; outcome may be uncertain and was not retried",
                )?;
                stream.write_all(body).context(
                    "Native request write failed; outcome may be uncertain and was not retried",
                )?;
                let (response, keep_alive) = read_native_reply(&mut TimedReader {
                    stream: &mut stream,
                    deadline: Instant::now() + NATIVE_REQUEST_TIMEOUT,
                })
                .context("Native response failed; request was not retried")?;
                if keep_alive {
                    *slot = Some(stream);
                }
                return Ok(response);
            }
            ensure!(
                Instant::now() < deadline,
                "All native request slots are busy"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

fn validate_pinned_launch(launch: &Launch, token: &str, connection: &Connection) -> Result<()> {
    ensure!(
        connection_url(launch, token)? == connection.url
            && launch.pid == connection.pid
            && launch.port == connection.port,
        "Local service launch changed; reconnect explicitly"
    );
    Ok(())
}

fn validate_native_request(method: &str, path: &str, body: &[u8]) -> Result<()> {
    ensure!(
        matches!(method, "GET" | "POST")
            && path.starts_with("/api/")
            && path.len() <= 16 * 1024
            && path
                .bytes()
                .all(|byte| (0x21..=0x7e).contains(&byte) && byte != b'#')
            && body.len() <= MAX_NATIVE_BODY
            && (method != "GET" || body.is_empty()),
        "Invalid native API request"
    );
    Ok(())
}

fn idle_socket_healthy(stream: &TcpStream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return false;
    }
    let mut byte = [0_u8; 1];
    let healthy =
        matches!(stream.peek(&mut byte), Err(error) if error.kind() == io::ErrorKind::WouldBlock);
    healthy && stream.set_nonblocking(false).is_ok()
}

fn read_native_reply(reader: &mut impl Read) -> Result<(NativeResponse, bool)> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    let split = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index;
        }
        ensure!(
            bytes.len() <= MAX_HEADERS,
            "Local HTTP headers are too large"
        );
        let count = reader.read(&mut chunk)?;
        ensure!(count > 0, "Incomplete native HTTP headers");
        bytes.extend_from_slice(&chunk[..count]);
    };
    ensure!(split <= MAX_HEADERS, "Local HTTP headers are too large");
    let header = std::str::from_utf8(&bytes[..split]).context("Invalid local HTTP headers")?;
    let mut lines = header.split("\r\n");
    let status: Vec<_> = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    ensure!(
        status.len() >= 2 && matches!(status[0], "HTTP/1.1" | "HTTP/1.0"),
        "Invalid native HTTP status"
    );
    let code: u16 = status[1].parse()?;
    ensure!(
        (200..=599).contains(&code) && !(300..400).contains(&code),
        "Native HTTP redirects are not accepted"
    );
    let mut keep_alive = status[0] == "HTTP/1.1";
    let mut length = None;
    let mut json = false;
    for line in lines {
        let (name, value) = line.split_once(':').context("Invalid local HTTP header")?;
        match name.to_ascii_lowercase().as_str() {
            "content-length" => {
                ensure!(length.is_none(), "Duplicate HTTP length");
                length = Some(value.trim().parse::<usize>()?);
            }
            "transfer-encoding" => bail!("Transfer-encoded native responses are not accepted"),
            "content-type" => {
                ensure!(!json, "Duplicate JSON content type");
                json = value
                    .trim()
                    .split(';')
                    .next()
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("application/json"));
                ensure!(json, "Native API returned a non-JSON content type");
            }
            "connection"
                if value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("close")) =>
            {
                keep_alive = false;
            }
            _ => {}
        }
    }
    let length = length.context("Native response did not provide a content length")?;
    ensure!(
        length <= MAX_NATIVE_BODY && (json || length == 0),
        "Invalid native response body"
    );
    let mut body = bytes.split_off(split + 4);
    ensure!(
        body.len() <= length,
        "Unexpected trailing native response bytes"
    );
    while body.len() < length {
        let available = chunk.len().min(length - body.len());
        let count = reader.read(&mut chunk[..available])?;
        ensure!(count > 0, "Incomplete native response body");
        body.extend_from_slice(&chunk[..count]);
    }
    Ok((
        NativeResponse {
            status: code,
            content_type: "application/json".into(),
            body,
        },
        keep_alive,
    ))
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("origin", &self.url.origin().ascii_serialization())
            .field("pid", &self.pid)
            .field("port", &self.port)
            .field("reused", &self.reused)
            .finish()
    }
}

#[derive(Deserialize)]
struct Launch {
    url: String,
    pid: u32,
    port: u16,
    prototype: bool,
}

fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

fn private_directory(path: &Path) -> Result<PathBuf> {
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(path)
            .context("Could not create the private application folder")?;
    }
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "The data folder must be a real directory"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.uid() == current_uid() && metadata.mode() & 0o777 == 0o700,
            "The data folder must be owned by you with permissions 0700"
        );
    }
    Ok(path.canonicalize()?)
}

fn open_regular(path: &Path, write: bool, create: bool, secret: bool, max: u64) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(write).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .context("Could not open a private connection file")?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= max,
        "Invalid connection file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.uid() == current_uid(),
            "Connection file is not owned by you"
        );
        ensure!(
            if secret {
                metadata.mode() & 0o777 == 0o600
            } else {
                metadata.mode() & 0o022 == 0
            },
            "Connection file permissions are unsafe"
        );
    }
    Ok(file)
}

fn read_private(path: &Path, max: u64) -> Result<(Vec<u8>, fs::Metadata)> {
    let mut file = open_regular(path, false, false, true, max)?;
    let metadata = file.metadata()?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= max, "Connection file is too large");
    Ok((bytes, metadata))
}

fn valid_token(token: &str) -> bool {
    if token.len() != 72 || !token.is_ascii() {
        return false;
    }
    [0, 36].into_iter().all(|offset| {
        let uuid = &token.as_bytes()[offset..offset + 36];
        uuid.iter().enumerate().all(|(i, byte)| {
            if [8, 13, 18, 23].contains(&i) {
                *byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
            }
        }) && uuid
            .iter()
            .any(|byte| (b'1'..=b'9').contains(byte) || (b'a'..=b'f').contains(byte))
    })
}

fn auth_token(data: &Path) -> Result<Option<String>> {
    let path = data.join("auth-token");
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let (bytes, _) = read_private(&path, 72)?;
    let token = String::from_utf8(bytes).context("Invalid local authentication file")?;
    ensure!(
        valid_token(&token),
        "Invalid local authentication file; it was not replaced"
    );
    Ok(Some(token))
}

fn connection_url(launch: &Launch, token: &str) -> Result<Url> {
    ensure!(
        launch.pid > 0 && launch.port > 0 && launch.prototype,
        "Invalid local launch metadata"
    );
    let url = Url::parse(&launch.url).context("Invalid local launch address")?;
    ensure!(
        url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port() == Some(launch.port)
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment() == Some(format!("token={token}").as_str()),
        "Local launch address does not match the private credential"
    );
    Ok(url)
}

#[cfg(test)]
pub fn validate_navigation(target: &Url, origin: &Url) -> bool {
    target.scheme() == "http"
        && origin.scheme() == "http"
        && target.host_str() == Some("127.0.0.1")
        && origin.host_str() == Some("127.0.0.1")
        && target.port_or_known_default() == origin.port_or_known_default()
        && target.username().is_empty()
        && target.password().is_none()
}

fn validate_state(value: &Value) -> Result<()> {
    ensure!(
        value.get("prototype").and_then(Value::as_bool) == Some(true)
            && value.get("capabilities").is_some_and(Value::is_object)
            && value.get("projects").is_some_and(Value::is_array)
            && value.get("sessions").is_some_and(Value::is_array),
        "The listener is not a compatible MPAC service"
    );
    Ok(())
}

fn read_state_reply(reader: &mut impl Read) -> Result<()> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 2048];
    let split = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index;
        }
        ensure!(
            bytes.len() <= MAX_HEADERS,
            "Local HTTP headers are too large"
        );
        let read = reader.read(&mut chunk)?;
        ensure!(read > 0, "Local service closed an incomplete HTTP response");
        bytes.extend_from_slice(&chunk[..read]);
    };
    ensure!(split <= MAX_HEADERS, "Local HTTP headers are too large");
    let header = std::str::from_utf8(&bytes[..split]).context("Invalid local HTTP headers")?;
    let mut lines = header.split("\r\n");
    let status: Vec<_> = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    ensure!(
        status.len() >= 2 && matches!(status[0], "HTTP/1.1" | "HTTP/1.0") && status[1] == "200",
        "Local service did not return HTTP 200; redirects are not followed"
    );
    let mut length = None;
    let mut json = false;
    for line in lines {
        let (name, value) = line.split_once(':').context("Invalid local HTTP header")?;
        match name.to_ascii_lowercase().as_str() {
            "content-length" => {
                ensure!(length.is_none(), "Duplicate HTTP length");
                length = Some(value.trim().parse::<usize>()?);
            }
            "transfer-encoding" => bail!("Transfer-encoded local responses are not accepted"),
            "content-type" => {
                json = value
                    .trim()
                    .split(';')
                    .next()
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("application/json"));
            }
            _ => {}
        }
    }
    let length = length.context("Local service did not provide a response length")?;
    ensure!(
        json && length <= MAX_STATE,
        "Invalid or oversized local state response"
    );
    let mut body = bytes.split_off(split + 4);
    ensure!(
        body.len() <= length,
        "Unexpected bytes after the local state response"
    );
    while body.len() < length {
        let available = chunk.len().min(length - body.len());
        let count = reader.read(&mut chunk[..available])?;
        ensure!(count > 0, "Incomplete local state response");
        body.extend_from_slice(&chunk[..count]);
    }
    validate_state(&serde_json::from_slice::<Value>(&body).context("Invalid local state JSON")?)
}

struct TimedReader<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}
impl Read for TimedReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Local state read timed out",
            ));
        }
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(bytes)
    }
}

fn command_output(mut command: Command) -> Result<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not verify local process ownership")?;
    let result = (|| -> Result<Vec<u8>> {
        let stdout = child
            .stdout
            .take()
            .context("Missing ownership probe output")?;
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout
                .take(64 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = sender.send(result);
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(result) = receiver.try_recv() {
                let bytes = result?;
                ensure!(
                    bytes.len() <= 64 * 1024,
                    "Ownership probe output is too large"
                );
                while Instant::now() < deadline {
                    if let Some(status) = child.try_wait()? {
                        ensure!(
                            status.success(),
                            "Could not establish local process ownership"
                        );
                        return Ok(bytes);
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        bail!("Local ownership verification timed out")
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn lsof_fields(bytes: &[u8]) -> Result<Vec<(u8, &[u8])>> {
    ensure!(bytes.len() <= 64 * 1024, "Ownership response is too large");
    bytes
        .split(|byte| *byte == 0)
        .filter_map(|part| {
            let part = part.strip_prefix(b"\n").unwrap_or(part);
            (!part.is_empty()).then_some(part)
        })
        .map(|part| {
            ensure!(part[0].is_ascii_alphabetic(), "Invalid ownership response");
            Ok((part[0], &part[1..]))
        })
        .collect()
}

fn process_identity(
    data: &Path,
    pid: u32,
    launch_time: SystemTime,
    pinned: Option<&File>,
) -> Result<ProcessIdentity> {
    process_observation(data, pid, launch_time, pinned).map(|(identity, _)| identity)
}

#[cfg(target_os = "macos")]
fn process_birth(pid: u32, launch_time: SystemTime) -> Result<(u64, u64)> {
    ensure!(pid <= i32::MAX as u32, "Invalid local process ID");
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info) as i32;
    let found = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    ensure!(
        found == size
            && info.pbi_uid == current_uid()
            && info.pbi_ruid == current_uid()
            && info.pbi_status != libc::SZOMB,
        "Local service PID is not a live process owned by you"
    );
    let published = launch_time.duration_since(UNIX_EPOCH)?.as_micros();
    ensure!(
        published
            >= u128::from(info.pbi_start_tvsec) * 1_000_000 + u128::from(info.pbi_start_tvusec),
        "Launch metadata predates the local process"
    );
    Ok((info.pbi_start_tvsec, info.pbi_start_tvusec))
}

#[cfg(target_os = "macos")]
fn process_observation(
    data: &Path,
    pid: u32,
    launch_time: SystemTime,
    pinned: Option<&File>,
) -> Result<(ProcessIdentity, File)> {
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::MetadataExt;
    let started = process_birth(pid, launch_time)?;
    let mut path = vec![0_u8; 4096];
    let count =
        unsafe { libc::proc_pidpath(pid as i32, path.as_mut_ptr().cast(), path.len() as u32) };
    ensure!(count > 0, "Could not identify the local service executable");
    path.truncate(
        path.iter()
            .position(|byte| *byte == 0)
            .unwrap_or(count as usize),
    );
    let path = PathBuf::from(std::ffi::OsString::from_vec(path));
    let executable = observed_process_executable(data, &path, pid, pinned)?;
    let metadata = executable.metadata()?;
    let (device, inode) = (metadata.dev(), metadata.ino());
    ensure!(
        process_birth(pid, launch_time)? == started,
        "Local process changed during executable verification"
    );
    Ok((
        ProcessIdentity {
            pid,
            device,
            inode,
            started_seconds: started.0,
            started_microseconds: started.1,
        },
        executable,
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_observation(
    _data: &Path,
    _pid: u32,
    _launch_time: SystemTime,
    _pinned: Option<&File>,
) -> Result<(ProcessIdentity, File)> {
    bail!("Native process verification requires macOS or Linux")
}

#[cfg(target_os = "linux")]
fn linux_process_birth(pid: u32, published: SystemTime) -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    ensure!(pid > 0 && pid <= i32::MAX as u32, "Invalid process ID");
    let directory = PathBuf::from(format!("/proc/{pid}"));
    ensure!(
        directory.metadata()?.uid() == current_uid(),
        "Service is owned by another user"
    );
    let status = fs::read_to_string(directory.join("status"))?;
    let uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .context("Missing process user IDs")?;
    let ids = uid
        .split_whitespace()
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        ids.len() == 4 && ids.iter().all(|uid| *uid == current_uid()),
        "Process user identity changed"
    );
    let stat = fs::read_to_string(directory.join("stat"))?;
    let fields: Vec<_> = stat
        .rsplit_once(") ")
        .context("Invalid process stat")?
        .1
        .split_whitespace()
        .collect();
    ensure!(
        fields.len() > 19 && !matches!(fields[0], "Z" | "X"),
        "Service process is not live"
    );
    let started: u64 = fields[19].parse()?;
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    ensure!(ticks > 0, "Invalid kernel clock tick rate");
    let boot: u64 = fs::read_to_string("/proc/stat")?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))
        .context("Missing kernel boot time")?
        .parse()?;
    ensure!(
        published.duration_since(UNIX_EPOCH)?.as_secs()
            >= boot.saturating_add(started / ticks as u64),
        "Launch metadata predates the process"
    );
    Ok(started)
}

#[cfg(target_os = "linux")]
fn process_observation(
    data: &Path,
    pid: u32,
    published: SystemTime,
    pinned: Option<&File>,
) -> Result<(ProcessIdentity, File)> {
    use std::os::unix::fs::MetadataExt;
    let started = linux_process_birth(pid, published)?;
    let proc_exe = PathBuf::from(format!("/proc/{pid}/exe"));
    let path = fs::read_link(&proc_exe)?;
    let executable = trusted_process_executable(data, &path)?;
    let metadata = executable.metadata()?;
    let kernel = fs::metadata(&proc_exe)?;
    ensure!(
        metadata.dev() == kernel.dev() && metadata.ino() == kernel.ino(),
        "Kernel executable image does not match its file"
    );
    if let Some(pinned) = pinned {
        ensure!(
            same_executable_file(pinned, &executable)?,
            "Service executable changed"
        );
    }
    ensure!(
        linux_process_birth(pid, published)? == started,
        "Process identity changed during verification"
    );
    Ok((
        ProcessIdentity {
            pid,
            device: metadata.dev(),
            inode: metadata.ino(),
            started_seconds: started,
            started_microseconds: 0,
        },
        executable,
    ))
}

fn lsof_program() -> &'static str {
    if cfg!(target_os = "linux") {
        "/usr/bin/lsof"
    } else {
        "/usr/sbin/lsof"
    }
}

fn trusted_executable(path: &Path) -> Result<()> {
    ensure!(
        path.file_name().is_some_and(|name| name == "mpac-daemon"),
        "Unexpected local service executable name"
    );
    trusted_executable_file(path)?;
    Ok(())
}

fn trusted_executable_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .context("Could not open the local service executable")?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "The local service executable must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            [0, current_uid()].contains(&metadata.uid())
                && metadata.mode() & 0o022 == 0
                && metadata.mode() & 0o111 != 0,
            "The local service executable has unsafe ownership or permissions"
        );
    }
    Ok(file)
}

/// macOS may report another hard link to the executable vnode after an agent
/// starts. Accept only the one alias that our daemon creates in its private
/// data directory; pin the executable vnode rather than its changeable name.
#[cfg(unix)]
fn trusted_process_executable(data: &Path, path: &Path) -> Result<File> {
    use std::os::unix::fs::MetadataExt;
    let file = if path.file_name().is_some_and(|name| name == "mpac-daemon") {
        trusted_executable_file(path)?
    } else {
        let directory = data.join("agent-events");
        ensure!(
            path == directory.join("activity-helper"),
            "Unexpected local service executable alias"
        );
        let parent = fs::symlink_metadata(&directory)?;
        ensure!(
            parent.is_dir()
                && !parent.file_type().is_symlink()
                && parent.uid() == current_uid()
                && parent.mode() & 0o777 == 0o700
                && directory.canonicalize()? == directory,
            "The local service helper directory is not private"
        );
        let file = trusted_executable_file(path)?;
        ensure!(
            path.canonicalize()? == path,
            "The local service helper is not a trusted executable alias"
        );
        file
    };
    Ok(file)
}

#[cfg(target_os = "macos")]
fn kernel_original_executable(pid: u32) -> Result<PathBuf> {
    // KERN_PROCARGS2 starts with argc followed by the OS-reported original
    // executable-path hint. It is not immutable attestation: trust comes only
    // from independently opened file ownership and exact vnode equality.
    // Never interpret or expose the argv/environment strings after this path.
    let mut bytes = vec![0_u8; 256 * 1024];
    let mut size = bytes.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        bytes.fill(0);
        return Err(io::Error::last_os_error())
            .context("Could not read the kernel executable identity");
    }
    let result = if size <= bytes.len() {
        parse_kernel_original_executable(&bytes[..size])
    } else {
        Err(anyhow::anyhow!("Invalid kernel executable identity"))
    };
    bytes.fill(0);
    result
}

#[cfg(unix)]
fn parse_kernel_original_executable(bytes: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let offset = std::mem::size_of::<libc::c_int>();
    ensure!(
        bytes.len() > offset && bytes.len() <= 256 * 1024,
        "Invalid kernel executable identity"
    );
    let name = &bytes[offset..bytes.len().min(offset + 4096)];
    let end = name
        .iter()
        .position(|byte| *byte == 0)
        .context("Invalid kernel executable pathname")?;
    ensure!(end > 0, "Empty kernel executable pathname");
    let path = PathBuf::from(std::ffi::OsString::from_vec(name[..end].to_vec()));
    ensure!(
        path.is_absolute() && path.file_name().is_some_and(|name| name == "mpac-daemon"),
        "Unexpected kernel executable pathname"
    );
    Ok(path)
}

#[cfg(unix)]
fn same_executable_file(first: &File, second: &File) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let first = first.metadata()?;
    let second = second.metadata()?;
    Ok(first.dev() == second.dev() && first.ino() == second.ino())
}

#[cfg(target_os = "macos")]
fn observed_process_executable(
    data: &Path,
    path: &Path,
    pid: u32,
    pinned: Option<&File>,
) -> Result<File> {
    if let Ok(file) = trusted_process_executable(data, path) {
        return Ok(file);
    }
    // One shared executable can run several data directories. macOS may report
    // any of its helper hard links, even one created by another daemon instance.
    // Such an alias requires independent opened-file vnode equality; the
    // original path hint alone is never an executable identity.
    ensure!(
        path.file_name()
            .is_some_and(|name| name == "activity-helper"),
        "Unexpected local executable alias"
    );
    let directory = path
        .parent()
        .context("Invalid executable alias directory")?;
    ensure!(
        directory
            .file_name()
            .is_some_and(|name| name == "agent-events"),
        "Unexpected local executable alias directory"
    );
    let other_data = directory
        .parent()
        .context("Invalid executable alias data directory")?;
    let parent = fs::symlink_metadata(other_data)?;
    use std::os::unix::fs::MetadataExt;
    ensure!(
        parent.is_dir()
            && !parent.file_type().is_symlink()
            && parent.uid() == current_uid()
            && parent.mode() & 0o777 == 0o700
            && other_data.canonicalize()? == other_data,
        "The executable alias data directory is not private"
    );
    let alias = trusted_process_executable(other_data, path)?;
    if let Some(pinned) = pinned {
        ensure!(
            same_executable_file(pinned, &alias)?,
            "Local executable alias changed its image"
        );
    } else {
        let original = kernel_original_executable(pid)?;
        trusted_executable(&original)?;
        let original = trusted_executable_file(&original)?;
        ensure!(
            same_executable_file(&original, &alias)?,
            "Executable alias does not match the kernel image"
        );
    }
    Ok(alias)
}

fn verify_loopback_listener(process_id: u32, port: u16) -> Result<()> {
    let mut listener = Command::new(lsof_program());
    listener.args([
        "-nP",
        "-a",
        "-p",
        &process_id.to_string(),
        "-sTCP:LISTEN",
        "-F0pun",
    ]);
    // Restrict at the command level too; unrelated sockets must not be collected.
    listener.arg(format!("-iTCP:{}", port));
    let output = command_output(listener)?;
    let fields = lsof_fields(&output)?;
    let mut pid = false;
    let mut uid = false;
    let mut addresses = Vec::new();
    for (kind, value) in fields {
        match kind {
            b'p' => pid = value == process_id.to_string().as_bytes(),
            b'u' => uid = value == current_uid().to_string().as_bytes(),
            b'n' => addresses.push(value),
            _ => {}
        }
    }
    let expected = format!("127.0.0.1:{}", port);
    ensure!(
        pid && uid && addresses.len() == 1 && addresses[0] == expected.as_bytes(),
        "The service does not own an exclusive loopback listener"
    );
    Ok(())
}

/// Connect first, then prove ownership before sending the private admin key.
pub(super) fn companion_request(
    pid: u32,
    admin: &Url,
    path: &str,
    body: &[u8],
) -> Result<NativeResponse> {
    ensure!(
        matches!(
            path,
            "/admin/telegram/send"
                | "/admin/state"
                | "/admin/invite"
                | "/admin/approve"
                | "/admin/reject"
                | "/admin/revoke"
        ),
        "Unsupported companion request"
    );
    ensure!(
        admin.scheme() == "http"
            && admin.host_str() == Some("127.0.0.1")
            && admin.username().is_empty()
            && admin.password().is_none()
            && admin.path() == "/"
            && admin.query().is_none(),
        "Invalid companion controls"
    );
    let port = admin.port().context("Missing companion port")?;
    let fields: Vec<_> =
        url::form_urlencoded::parse(admin.fragment().unwrap_or_default().as_bytes()).collect();
    ensure!(
        fields.len() == 1 && fields[0].0 == "token" && mpac_mobile_link::valid_secret(&fields[0].1),
        "Invalid companion credential"
    );
    ensure!(body.len() <= 4096, "Companion request too large");
    let mut stream = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_secs(2),
    )?;
    verify_loopback_listener(pid, port)?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let header = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        fields[0].1,
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    let mut reader = TimedReader {
        stream: &mut stream,
        deadline: Instant::now() + Duration::from_secs(20),
    };
    let (reply, _) = read_native_reply(&mut reader)?;
    Ok(reply)
}

fn verify_identity(data: &Path, launch: &Launch, published: SystemTime, lock: &File) -> Result<()> {
    verify_identity_with_pin(data, launch, published, lock, None)
}

fn verify_identity_with_pin(
    data: &Path,
    launch: &Launch,
    published: SystemTime,
    lock: &File,
    pinned: Option<&File>,
) -> Result<()> {
    let identity = process_identity(data, launch.pid, published, pinned)?;
    match lock.try_lock_exclusive() {
        Ok(()) => {
            FileExt::unlock(lock)?;
            bail!("The local service does not hold its data lock");
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
        Err(error) => return Err(error.into()),
    }
    verify_loopback_listener(launch.pid, launch.port)?;
    let lock_path = data.join("daemon.lock");
    let mut open_lock = Command::new(lsof_program());
    open_lock
        .args(["-nP", "-a", "-p", &launch.pid.to_string(), "-F0pn", "--"])
        .arg(&lock_path);
    let output = command_output(open_lock)?;
    let fields = lsof_fields(&output)?;
    ensure!(
        fields
            .iter()
            .any(|(kind, value)| *kind == b'p' && *value == launch.pid.to_string().as_bytes())
            && fields
                .iter()
                .any(|(kind, value)| *kind == b'n'
                    && *value == lock_path.as_os_str().as_encoded_bytes()),
        "The local process does not own this data directory"
    );
    ensure!(
        process_identity(data, launch.pid, published, pinned)? == identity,
        "Local process identity changed during verification"
    );
    Ok(())
}

fn connect_existing(data: &Path, lock: &File, token: &str, reused: bool) -> Result<Connection> {
    let (bytes, metadata) = read_private(&data.join("launch.json"), MAX_LAUNCH)?;
    let launch: Launch = serde_json::from_slice(&bytes).context("Invalid local launch metadata")?;
    let url = connection_url(&launch, token)?;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, launch.port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(500))
        .context("The local service is not accepting connections")?;
    // Do not replace this stream after validation: the credential belongs only
    // on the connection established before native ownership was checked.
    verify_identity(data, &launch, metadata.modified()?, lock)?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let request = format!(
        "GET /api/state HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        launch.port, token
    );
    stream.write_all(request.as_bytes())?;
    read_state_reply(&mut TimedReader {
        stream: &mut stream,
        deadline: Instant::now() + Duration::from_secs(2),
    })?;
    Ok(Connection {
        url,
        pid: launch.pid,
        port: launch.port,
        reused,
    })
}

fn trusted_bin(path: &Path) -> Option<PathBuf> {
    let path = path.canonicalize().ok()?;
    let metadata = path.metadata().ok()?;
    if !metadata.is_dir() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if ![0, current_uid()].contains(&metadata.uid()) || metadata.mode() & 0o022 != 0 {
            return None;
        }
    }
    Some(path)
}

fn launch_path() -> Result<std::ffi::OsString> {
    let mut paths = Vec::new();
    // Preserve the caller's selected CLI versions, then add Finder-launch
    // fallbacks. Every candidate is independently checked before use.
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for folder in [
            ".local/bin",
            ".cargo/bin",
            ".npm-global/bin",
            ".volta/bin",
            ".bun/bin",
            ".nvm/current/bin",
        ] {
            paths.push(home.join(folder));
        }
        if let Ok(entries) = fs::read_dir(home.join(".nvm/versions/node")) {
            let mut versions: Vec<_> = entries
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with('v'))
                .map(|entry| entry.path())
                .collect();
            versions.sort_by_key(|path| {
                let numbers: Vec<u64> = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .trim_start_matches('v')
                    .split('.')
                    .filter_map(|part| part.parse().ok())
                    .collect();
                std::cmp::Reverse(numbers)
            });
            paths.extend(
                versions
                    .into_iter()
                    .take(16)
                    .map(|version| version.join("bin")),
            );
        }
    }
    paths.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ]
        .map(PathBuf::from),
    );
    let mut seen = HashSet::new();
    let paths: Vec<_> = paths
        .iter()
        .filter_map(|path| trusted_bin(path))
        .filter(|path| seen.insert(path.clone()))
        .collect();
    Ok(std::env::join_paths(paths)?)
}

fn spawn_daemon(data: &Path, sidecar: &Path, ui: &Path) -> Result<u32> {
    trusted_executable(sidecar)?;
    let sidecar = sidecar.canonicalize()?;
    let ui = ui.canonicalize().context("Bundled UI is unavailable")?;
    ensure!(
        ui.is_dir() && ui.join("index.html").is_file(),
        "Bundled UI is incomplete"
    );
    let mut log = open_regular(
        &data.join("desktop-daemon.log"),
        true,
        true,
        true,
        16 * 1024 * 1024,
    )?;
    log.seek(SeekFrom::End(0))?;
    let mut command = Command::new(sidecar);
    command
        .args(["--data-dir"])
        .arg(data)
        .arg("--ui-dir")
        .arg(ui)
        .args(["--port", "0"])
        .current_dir(data)
        .env("PATH", launch_path()?)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    let mut child = command
        .spawn()
        .context("Could not start the bundled local service")?;
    let pid = child.id();
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

pub fn connect_or_start(
    config: &DesktopConfig,
    sidecar: &Path,
    ui_dir: &Path,
) -> Result<Connection> {
    ensure!(
        cfg!(any(target_os = "macos", target_os = "linux")),
        "Local connection verification requires macOS or Linux"
    );
    let data = private_directory(&config.data_dir)?;
    let startup = open_regular(&data.join("desktop-startup.lock"), true, true, true, 4096)?;
    let deadline = Instant::now() + START_BUDGET;
    loop {
        match startup.try_lock_exclusive() {
            Ok(()) => break,
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(50))
            }
            Err(error) => {
                return Err(error)
                    .context("Another desktop launcher is still starting the service");
            }
        }
    }
    let deadline = Instant::now() + START_BUDGET;
    let lock = open_regular(&data.join("daemon.lock"), true, true, false, 4096)?;
    let token = auth_token(&data)?; // Existing invalid credentials are never replaced.
    let held = match lock.try_lock_exclusive() {
        Ok(()) => {
            FileExt::unlock(&lock)?;
            false
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => true,
        Err(error) => return Err(error.into()),
    };
    let spawned = if held {
        None
    } else {
        Some(spawn_daemon(&data, sidecar, ui_dir)?)
    };
    loop {
        let current = auth_token(&data)?.or_else(|| token.clone());
        let last_error = match current {
            Some(token) => match connect_existing(&data, &lock, &token, held) {
                Ok(mut connection) => {
                    connection.reused = spawned != Some(connection.pid);
                    return Ok(connection);
                }
                Err(error) => error,
            },
            None => anyhow::anyhow!("The local service has not published its credential"),
        };
        if Instant::now() >= deadline {
            // This chain contains only bounded parser/OS errors and static
            // verification contexts, never the authenticated URL or payload.
            return Err(last_error).context("Could not verify the local service. Existing processes and credentials were left unchanged");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_process_birth_rejects_stale_metadata_and_invalid_processes() {
        let now = SystemTime::now();
        let birth = linux_process_birth(std::process::id(), now).unwrap();
        assert_eq!(linux_process_birth(std::process::id(), now).unwrap(), birth);
        assert!(linux_process_birth(std::process::id(), UNIX_EPOCH).is_err());
        assert!(linux_process_birth(0, now).is_err());
        assert!(linux_process_birth(u32::MAX, now).is_err());
    }
    const TOKEN: &str = "12345678-1234-1234-1234-123456789abc12345678-1234-1234-1234-123456789abc";
    fn launch(address: &str) -> Launch {
        Launch {
            url: address.into(),
            pid: 123,
            port: 4317,
            prototype: true,
        }
    }

    #[test]
    fn companion_bearer_requires_the_owner_of_the_already_connected_socket() {
        use std::net::TcpListener;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        verify_loopback_listener(std::process::id(), port).unwrap();
        let accepted = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut byte = [0u8];
            assert_eq!(
                socket.read(&mut byte).unwrap(),
                0,
                "foreign listener must receive no credential bytes"
            );
        });
        let admin: Url = format!("http://127.0.0.1:{port}/#token={}", "a".repeat(64))
            .parse()
            .unwrap();
        assert!(companion_request(u32::MAX, &admin, "/admin/telegram/send", b"{}").is_err());
        accepted.join().unwrap();
    }
    #[test]
    fn companion_request_preserves_reply_without_exposing_admin_key_to_renderer() {
        use std::net::TcpListener;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut byte = [0u8];
            while !bytes.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
            }
            assert!(
                String::from_utf8(bytes)
                    .unwrap()
                    .contains("Authorization: Bearer ")
            );
            let mut body = [0u8; 2];
            socket.read_exact(&mut body).unwrap();
            assert_eq!(&body, b"{}");
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 13\r\nConnection: close\r\n\r\n{\"sent\":true}").unwrap();
        });
        let admin: Url = format!("http://127.0.0.1:{port}/#token={}", "a".repeat(64))
            .parse()
            .unwrap();
        let reply =
            companion_request(std::process::id(), &admin, "/admin/telegram/send", b"{}").unwrap();
        assert_eq!(reply.body, b"{\"sent\":true}");
        accepted.join().unwrap();
    }

    #[test]
    fn only_private_matching_loopback_urls_are_accepted() {
        assert!(valid_token(TOKEN));
        let url = format!("http://127.0.0.1:4317/#token={TOKEN}");
        let allowed = connection_url(&launch(&url), TOKEN).unwrap();
        for address in [
            format!("http://localhost:4317/#token={TOKEN}"),
            format!("https://127.0.0.1:4317/#token={TOKEN}"),
            format!("http://user@127.0.0.1:4317/#token={TOKEN}"),
            format!("http://127.0.0.1:4318/#token={TOKEN}"),
            format!("http://127.0.0.1:4317/api/#token={TOKEN}"),
            "http://127.0.0.1:4317/#token=wrong".into(),
        ] {
            assert!(connection_url(&launch(&address), TOKEN).is_err());
        }
        assert!(validate_navigation(
            &allowed.join("/files").unwrap(),
            &allowed
        ));
        assert!(!validate_navigation(
            &Url::parse("file:///etc/passwd").unwrap(),
            &allowed
        ));
        assert!(!validate_navigation(
            &Url::parse("http://127.0.0.1:4318/").unwrap(),
            &allowed
        ));
        assert!(
            !format!(
                "{:?}",
                Connection {
                    url: allowed,
                    pid: 123,
                    port: 4317,
                    reused: true
                }
            )
            .contains(TOKEN)
        );
    }

    fn reply(body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }
    #[test]
    fn http_probe_checks_framing_schema_and_never_redirects() {
        let body = r#"{"prototype":true,"capabilities":{},"projects":[],"sessions":[]}"#;
        assert!(read_state_reply(&mut Cursor::new(reply(body))).is_ok());
        for bad in [b"HTTP/1.1 302 Found\r\nLocation: http://foreign/\r\nContent-Length: 0\r\n\r\n".to_vec(), b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec(), reply("{}"), reply("not json"), b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 999999999\r\n\r\n".to_vec(), b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}".to_vec()] { assert!(read_state_reply(&mut Cursor::new(bad)).is_err()); }
        assert!(read_state_reply(&mut Cursor::new(reply(body)[..20].to_vec())).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn state_files_reject_symlinks_wrong_permissions_and_oversized_content() {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
        let folder = std::env::temp_dir().join(format!(
            "mpac-native-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&folder).unwrap();
        let data = private_directory(&folder).unwrap();
        let path = data.join("auth-token");
        let mut file = open_regular(&path, true, true, true, 72).unwrap();
        file.write_all(TOKEN.as_bytes()).unwrap();
        drop(file);
        assert_eq!(auth_token(&data).unwrap().as_deref(), Some(TOKEN));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(auth_token(&data).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_file(&path).unwrap();
        symlink("missing", &path).unwrap();
        assert!(auth_token(&data).is_err());
        fs::remove_file(&path).unwrap();
        let mut file = open_regular(&path, true, true, true, 72).unwrap();
        file.write_all(&[b'x'; 73]).unwrap();
        drop(file);
        assert!(auth_token(&data).is_err());
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn ownership_records_preserve_literal_spaces_and_reject_malformed_records() {
        let fields = lsof_fields(b"p123\0u501\0\nf4\0n/data with spaces/daemon.lock\0\n").unwrap();
        assert!(fields.contains(&(b'n', b"/data with spaces/daemon.lock".as_slice())));
        assert!(lsof_fields(b"\x00:bad\0").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn executable_hard_link_alias_is_scoped_private_and_pinned_by_inode() {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};
        let folder =
            std::env::temp_dir().join(format!("mpac-executable-alias-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&folder).unwrap();
        let folder = folder.canonicalize().unwrap();
        let directory = folder.join("agent-events");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let daemon = folder.join("mpac-daemon");
        fs::write(&daemon, b"original executable fixture").unwrap();
        fs::set_permissions(&daemon, fs::Permissions::from_mode(0o700)).unwrap();
        let pinned = trusted_process_executable(&folder, &daemon).unwrap();
        let identity = |file: &File| {
            let meta = file.metadata().unwrap();
            (meta.dev(), meta.ino())
        };
        let original = identity(&pinned);
        let helper = directory.join("activity-helper");
        fs::hard_link(&daemon, &helper).unwrap();
        assert_eq!(
            identity(&trusted_process_executable(&folder, &helper).unwrap()),
            original
        );
        let foreign = folder.join("activity-helper");
        fs::hard_link(&daemon, &foreign).unwrap();
        assert!(trusted_process_executable(&folder, &foreign).is_err());
        fs::remove_file(&foreign).unwrap();
        // An app update can unlink/replace its former bundle path while the
        // same process continues using the surviving helper alias.
        fs::remove_file(&daemon).unwrap();
        assert_eq!(
            identity(&trusted_process_executable(&folder, &helper).unwrap()),
            original
        );
        fs::write(&daemon, b"replacement executable fixture").unwrap();
        fs::set_permissions(&daemon, fs::Permissions::from_mode(0o700)).unwrap();
        assert_ne!(
            identity(&trusted_process_executable(&folder, &daemon).unwrap()),
            original
        );
        assert_eq!(identity(&pinned), original);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(trusted_process_executable(&folder, &helper).is_err());
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_file(&helper).unwrap();
        symlink(&daemon, &helper).unwrap();
        assert!(trusted_process_executable(&folder, &helper).is_err());
        fs::remove_file(&helper).unwrap();
        fs::hard_link(&daemon, &helper).unwrap();
        fs::rename(&directory, folder.join("moved-events")).unwrap();
        symlink(folder.join("moved-events"), &directory).unwrap();
        assert!(trusted_process_executable(&folder, &helper).is_err());
        fs::remove_dir_all(folder).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn kernel_original_path_parser_never_uses_argv_or_environment() {
        let mut bytes = 2_i32.to_ne_bytes().to_vec();
        bytes.extend_from_slice(
            b"/private/example/mpac-daemon\0\0untrusted-argv0\0TOKEN=never-inspect\0",
        );
        assert_eq!(
            parse_kernel_original_executable(&bytes).unwrap(),
            PathBuf::from("/private/example/mpac-daemon")
        );
        for path in [
            b"/tmp/activity-helper\0".as_slice(),
            b"mpac-daemon\0",
            b"/tmp/mpac-daemon",
            b"\0",
        ] {
            let mut invalid = 2_i32.to_ne_bytes().to_vec();
            invalid.extend_from_slice(path);
            assert!(parse_kernel_original_executable(&invalid).is_err());
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn warm_cross_data_helper_requires_the_pinned_executable_inode() {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let folder = std::env::temp_dir().join(format!("mpac-cross-data-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&folder).unwrap();
        let folder = folder.canonicalize().unwrap();
        let selected = folder.join("selected");
        let other = folder.join("other");
        for data in [&selected, &other] {
            fs::DirBuilder::new().mode(0o700).create(data).unwrap();
            fs::DirBuilder::new()
                .mode(0o700)
                .create(data.join("agent-events"))
                .unwrap();
        }
        let original = folder.join("mpac-daemon");
        fs::write(&original, b"original fixture").unwrap();
        fs::set_permissions(&original, fs::Permissions::from_mode(0o700)).unwrap();
        let pinned = trusted_process_executable(&selected, &original).unwrap();
        let alias = other.join("agent-events/activity-helper");
        fs::hard_link(&original, &alias).unwrap();
        fs::remove_file(&original).unwrap();
        let accepted = observed_process_executable(&selected, &alias, 0, Some(&pinned)).unwrap();
        assert!(same_executable_file(&pinned, &accepted).unwrap());
        assert!(observed_process_executable(&selected, &alias, std::process::id(), None).is_err());
        fs::remove_file(&alias).unwrap();
        fs::write(&alias, b"replacement fixture").unwrap();
        fs::set_permissions(&alias, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(observed_process_executable(&selected, &alias, 0, Some(&pinned)).is_err());
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn native_bridge_rejects_injection_and_changed_launches() {
        assert!(validate_native_request("POST", "/api/sessions/example/input", b"{}").is_ok());
        for (method, path, body) in [
            ("DELETE", "/api/state", b"".as_slice()),
            (
                "GET",
                "/api/state\r\nAuthorization: injected",
                b"".as_slice(),
            ),
            ("GET", "http://127.0.0.1/api/state", b"".as_slice()),
            ("GET", "/api/state#fragment", b"".as_slice()),
            ("GET", "/api/state", b"unexpected".as_slice()),
        ] {
            assert!(validate_native_request(method, path, body).is_err());
        }
        let address = format!("http://127.0.0.1:4317/#token={TOKEN}");
        let mut launch = launch(&address);
        let connection = Connection {
            url: address.parse().unwrap(),
            pid: 123,
            port: 4317,
            reused: true,
        };
        assert!(validate_pinned_launch(&launch, TOKEN, &connection).is_ok());
        launch.pid += 1;
        assert!(validate_pinned_launch(&launch, TOKEN, &connection).is_err());
    }

    #[test]
    fn native_http_preserves_json_errors_and_drops_close_connections() {
        let body = r#"{"error":"own fixture"}"#;
        let raw = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (response, keep_alive) = read_native_reply(&mut Cursor::new(raw)).unwrap();
        assert_eq!(response.status, 400);
        assert_eq!(response.content_type, "application/json");
        assert_eq!(response.body, body.as_bytes());
        assert!(!keep_alive);
        assert!(read_native_reply(&mut Cursor::new(reply("{}"))).unwrap().1);
        for raw in [
            "HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 67108865\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{",
        ] {
            assert!(read_native_reply(&mut Cursor::new(raw)).is_err());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fresh_unverified_socket_is_closed_before_any_credential_bytes() {
        use std::net::TcpListener;
        use std::os::unix::fs::DirBuilderExt;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let folder = std::env::temp_dir().join(format!(
            "mpac-unverified-native-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&folder).unwrap();
        let lock = open_regular(&folder.join("daemon.lock"), true, true, false, 4096).unwrap();
        let client = NativeClient {
            data: folder.clone(),
            connection: Connection {
                url: format!("http://127.0.0.1:{port}/#token={TOKEN}")
                    .parse()
                    .unwrap(),
                pid: std::process::id(),
                port,
                reused: true,
            },
            token: TOKEN.into(),
            identity: ProcessIdentity {
                pid: 0,
                device: 0,
                inode: 0,
                started_seconds: 0,
                started_microseconds: 0,
            },
            _executable: lock.try_clone().unwrap(),
            lock,
            slots: std::array::from_fn(|_| Mutex::new(None)),
        };
        let launch = Launch {
            url: client.connection.url.to_string(),
            pid: std::process::id(),
            port,
            prototype: true,
        };
        assert!(client.verified_socket(&launch, SystemTime::now()).is_err());
        let (mut accepted, _) = listener.accept().unwrap();
        accepted
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        assert_eq!(
            accepted.read(&mut [0_u8; 1]).unwrap(),
            0,
            "no HTTP bytes may precede native ownership proof"
        );
        drop(client);
        fs::remove_dir_all(folder).unwrap();
    }

    /// Explicit real-process regression for the native gateway port-reuse threat.
    /// The fixture owns both the daemon child and its temporary data directory.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires built sidecar; run explicit env"]
    fn established_client_rejects_foreign_port_after_owned_daemon_exit() {
        use std::net::TcpListener;
        use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
        use std::os::unix::fs::DirBuilderExt;
        use std::process::Child;

        struct OwnedDaemon {
            folder: PathBuf,
            child: Option<Child>,
        }
        impl OwnedDaemon {
            fn stop_gracefully(&mut self) -> bool {
                let Some(child) = self.child.as_mut() else {
                    return true;
                };
                // No reaping precedes signaling: the unreaped owned child pins
                // this PID even if it exited immediately before SIGTERM.
                unsafe {
                    libc::kill(child.id() as i32, libc::SIGTERM);
                }
                let deadline = Instant::now() + Duration::from_secs(3);
                while Instant::now() < deadline {
                    if child.try_wait().is_ok_and(|status| status.is_some()) {
                        self.child.take();
                        return true;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                false
            }
        }
        impl Drop for OwnedDaemon {
            fn drop(&mut self) {
                if !self.stop_gracefully()
                    && let Some(mut child) = self.child.take()
                {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                let _ = fs::remove_dir_all(&self.folder);
            }
        }

        let daemon = PathBuf::from(
            std::env::var_os("MPAC_DESKTOP_TEST_DAEMON")
                .expect("set MPAC_DESKTOP_TEST_DAEMON to the absolute bundled sidecar"),
        );
        let ui = PathBuf::from(
            std::env::var_os("MPAC_DESKTOP_TEST_UI")
                .expect("set MPAC_DESKTOP_TEST_UI to absolute bundled Resources/ui"),
        );
        assert!(daemon.is_absolute() && ui.is_absolute());
        trusted_executable(&daemon).unwrap();
        assert!(ui.join("index.html").is_file());
        let folder = std::env::temp_dir().join(format!(
            "mpac-real-port-reuse-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::DirBuilder::new().mode(0o700).create(&folder).unwrap();
        let mut owned = OwnedDaemon {
            folder: folder.canonicalize().unwrap(),
            child: None,
        };
        owned.child = Some(
            Command::new(&daemon)
                .args(["--data-dir"])
                .arg(&owned.folder)
                .arg("--ui-dir")
                .arg(&ui)
                .args(["--port", "0"])
                .current_dir(&owned.folder)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(6);
        let connection = loop {
            if let Ok(Some(token)) = auth_token(&owned.folder)
                && let Ok(lock) =
                    open_regular(&owned.folder.join("daemon.lock"), true, false, false, 4096)
                && let Ok(connection) = connect_existing(&owned.folder, &lock, &token, false)
            {
                break connection;
            }
            assert!(
                Instant::now() < deadline,
                "owned daemon did not publish a verified connection"
            );
            thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(connection.pid, owned.child.as_ref().unwrap().id());
        let port = connection.port;
        let client = NativeClient::new(
            DesktopConfig {
                data_dir: owned.folder.clone(),
            },
            connection,
        )
        .unwrap();
        assert_eq!(
            client.request("GET", "/api/state", &[]).unwrap().status,
            200
        );
        assert!(
            client
                .slots
                .iter()
                .any(|slot| slot.lock().unwrap().is_some()),
            "initial verified request must leave an established pool socket"
        );
        assert!(
            owned.stop_gracefully(),
            "owned daemon did not stop after SIGTERM"
        );

        // Reuse the actual daemon's just-released port, including TIME_WAIT
        // accepted sockets. The foreign listener never obtains the credential.
        let descriptor = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        assert!(descriptor >= 0);
        let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
        let enabled: libc::c_int = 1;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    descriptor.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_REUSEADDR,
                    (&enabled as *const libc::c_int).cast(),
                    std::mem::size_of_val(&enabled) as libc::socklen_t,
                )
            },
            0
        );
        let address = libc::sockaddr_in {
            sin_len: std::mem::size_of::<libc::sockaddr_in>() as u8,
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: port.to_be(),
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
            },
            sin_zero: [0; 8],
        };
        assert_eq!(
            unsafe {
                libc::bind(
                    descriptor.as_raw_fd(),
                    (&address as *const libc::sockaddr_in).cast(),
                    std::mem::size_of_val(&address) as libc::socklen_t,
                )
            },
            0,
            "foreign fixture could not reclaim the exact daemon port"
        );
        assert_eq!(unsafe { libc::listen(descriptor.as_raw_fd(), 8) }, 0);
        let foreign = unsafe { TcpListener::from_raw_fd(descriptor.into_raw_fd()) };
        foreign.set_nonblocking(true).unwrap();
        assert!(client.request("GET", "/api/state", &[]).is_err());
        assert!(
            client
                .terminal_socket("12345678-1234-4234-8234-123456789abc")
                .is_err()
        );
        assert!(
            client
                .request(
                    "POST",
                    "/api/projects",
                    br#"{"name":"must not reach foreign listener","paths":[]}"#
                )
                .is_err()
        );
        assert!(
            matches!(foreign.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock),
            "same client must neither reconnect nor send Authorization/mutations to a recycled foreign port"
        );
        drop(client);
        drop(foreign);
    }
}
