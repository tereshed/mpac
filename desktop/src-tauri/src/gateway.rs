//! Bundled renderer transport. The daemon credential never enters a WebView.
use crate::connection::{NativeClient, NativeResponse, VerifiedTerminal};
use anyhow::Result;
use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU16, AtomicUsize, Ordering},
    },
};
use tauri::http::{Request, Response};

const MAX_BODY: usize = 64 * 1024 * 1024;
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-src 'none'; form-action 'none'";

pub trait Transport: Send + Sync {
    fn request(&self, method: &str, path: &str, body: &[u8]) -> Result<NativeResponse>;
    fn terminal_socket(&self, _session_id: &str) -> Result<VerifiedTerminal> {
        anyhow::bail!("Terminal streaming is not available")
    }
}
impl Transport for NativeClient {
    fn terminal_socket(&self, session_id: &str) -> Result<VerifiedTerminal> {
        NativeClient::terminal_socket(self, session_id)
    }
    fn request(&self, method: &str, path: &str, body: &[u8]) -> Result<NativeResponse> {
        NativeClient::request(self, method, path, body)
    }
}
struct Session {
    token: String,
    client: Arc<dyn Transport>,
    cancel: tokio::sync::watch::Sender<bool>,
}

pub struct StreamLease {
    pub client: Arc<dyn Transport>,
    pub cancelled: tokio::sync::watch::Receiver<bool>,
}
pub struct Gateway {
    ui: PathBuf,
    session: Mutex<Option<Session>>,
    in_flight: AtomicUsize,
    bridge_port: AtomicU16,
    voice: mpac_voice::Voice,
}
pub struct RequestPermit(Arc<Gateway>);
impl Drop for RequestPermit {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn workspace_url(url: &url::Url) -> bool {
    url.scheme() == "mpac"
        && url.host_str() == Some("localhost")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}
impl Gateway {
    pub fn new(ui: PathBuf) -> Self {
        Self {
            ui,
            session: Mutex::new(None),
            in_flight: AtomicUsize::new(0),
            bridge_port: AtomicU16::new(0),
            voice: mpac_voice::Voice::default(),
        }
    }
    pub fn install(&self, client: Arc<dyn Transport>) -> url::Url {
        let token = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        let mut address = url::Url::parse("mpac://localhost/").unwrap();
        address.set_fragment(Some(&format!("token={token}")));
        // Pair the token and captured backend atomically. Old requests can only
        // finish on their old client; they never migrate to a new connection.
        let mut session = self.session.lock().unwrap();
        if let Some(previous) = session.as_ref() {
            let _ = previous.cancel.send(true);
        }
        let (cancel, _) = tokio::sync::watch::channel(false);
        *session = Some(Session {
            token,
            client,
            cancel,
        });
        address
    }
    pub fn clear(&self) {
        let mut session = self.session.lock().unwrap();
        if let Some(previous) = session.as_ref() {
            let _ = previous.cancel.send(true);
        }
        *session = None;
    }
    pub fn set_bridge_port(&self, port: u16) {
        self.bridge_port.store(port, Ordering::Release);
    }
    pub fn stream_lease(&self, token: &str) -> Option<StreamLease> {
        let session = self.session.lock().unwrap();
        let session = session.as_ref()?;
        if !same_token(token, &session.token) {
            return None;
        }
        Some(StreamLease {
            client: session.client.clone(),
            cancelled: session.cancel.subscribe(),
        })
    }
    pub fn ready(&self) -> bool {
        self.session.lock().unwrap().is_some()
    }
    pub fn permit(self: &Arc<Self>) -> Option<RequestPermit> {
        self.in_flight
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 16).then_some(count + 1)
            })
            .ok()?;
        Some(RequestPermit(self.clone()))
    }
    pub fn serve(&self, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
        let opaque_origin = request
            .headers()
            .get("origin")
            .is_some_and(|value| value == "null");
        let mut response = self.serve_inner(request);
        let port = self.bridge_port.load(Ordering::Acquire);
        if port != 0 {
            let csp = CSP.replace(
                "connect-src 'self'",
                &format!("connect-src 'self' ws://127.0.0.1:{port}"),
            );
            if let Ok(value) = tauri::http::HeaderValue::from_str(&csp) {
                response
                    .headers_mut()
                    .insert("content-security-policy", value);
            }
        }
        if opaque_origin {
            response.headers_mut().insert(
                "access-control-allow-origin",
                tauri::http::HeaderValue::from_static("null"),
            );
        }
        response
    }
    fn serve_inner(&self, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
        let Ok(address) = url::Url::parse(&request.uri().to_string()) else {
            return error(400, "Invalid local request");
        };
        if !workspace_url(&address) {
            return error(403, "Only the bundled workspace is available");
        }
        if request.body().len() > MAX_BODY {
            return error(413, "Request is too large");
        }
        if let Some(origin) = request.headers().get("origin")
            && !matches!(origin.to_str(), Ok("mpac://localhost") | Ok("null"))
        {
            return error(403, "Unexpected workspace origin");
        }
        let path = request.uri().path();
        if path.starts_with("/api/") {
            if request.method() == "OPTIONS" {
                return response(204, "application/json", Vec::new());
            }
            if !matches!(request.method().as_str(), "GET" | "POST") {
                return error(405, "Unsupported workspace method");
            }
            let client = {
                let session = self.session.lock().unwrap();
                let Some(session) = session.as_ref() else {
                    return error(
                        503,
                        "Local service is not connected. Use Workspace → Reconnect.",
                    );
                };
                let expected = format!("Bearer {}", session.token);
                if request
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    != Some(expected.as_str())
                {
                    return error(
                        401,
                        "Workspace connection has changed. Use Workspace → Reconnect.",
                    );
                }
                session.client.clone()
            };
            if let Some(operation) = path.strip_prefix("/api/native/voice/") {
                return match self.voice.request(
                    request.method().as_str(),
                    operation,
                    request.body(),
                ) {
                    Ok(value) => response(200, "application/json", value.to_string().into_bytes()),
                    Err(code) => error(400, code),
                };
            }
            if path == "/api/native/terminal" {
                let port = self.bridge_port.load(Ordering::Acquire);
                return if request.method() != "GET" {
                    error(405, "Unsupported workspace method")
                } else if port == 0 {
                    error(503, "Native terminal bridge is not available")
                } else {
                    response(
                        200,
                        "application/json",
                        serde_json::to_vec(
                            &serde_json::json!({"url":format!("ws://127.0.0.1:{port}/terminal")}),
                        )
                        .unwrap(),
                    )
                };
            }
            let target = request
                .uri()
                .path_and_query()
                .map(|value| value.as_str())
                .unwrap_or(path);
            // Headers from the renderer are intentionally not forwarded.
            return match client.request(request.method().as_str(), target, request.body()) {
                Ok(value) if !(300..400).contains(&value.status) => {
                    response(value.status, &value.content_type, value.body)
                }
                _ => error(
                    502,
                    "Local service connection failed. Use Workspace → Reconnect.",
                ),
            };
        }
        if request.method() != "GET" {
            return error(405, "Unsupported asset method");
        }
        match asset_path(&self.ui, path) {
            Some(path) => match fs::metadata(&path).and_then(|meta| {
                if meta.len() <= MAX_BODY as u64 {
                    fs::read(&path)
                } else {
                    Err(std::io::Error::other("Asset is too large"))
                }
            }) {
                Ok(body) => {
                    let kind = match path.extension().and_then(|value| value.to_str()) {
                        Some("html") => "text/html; charset=utf-8",
                        Some("js") => "application/javascript; charset=utf-8",
                        Some("css") => "text/css; charset=utf-8",
                        Some("svg") => "image/svg+xml",
                        Some("png") => "image/png",
                        _ => "application/octet-stream",
                    };
                    response(200, kind, body)
                }
                Err(_) => error(404, "Bundled file is unavailable"),
            },
            None => error(404, "Bundled file is unavailable"),
        }
    }
}
fn same_token(actual: &str, expected: &str) -> bool {
    actual.len() == expected.len()
        && actual
            .bytes()
            .zip(expected.bytes())
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn asset_path(base: &Path, request: &str) -> Option<PathBuf> {
    let relative = if request == "/" {
        "index.html"
    } else {
        request.strip_prefix('/')?
    };
    if !(relative == "index.html" || relative.starts_with("assets/"))
        || !relative
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || b"-._/".contains(&value))
    {
        return None;
    }
    let path = Path::new(relative);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let base = base.canonicalize().ok()?;
    let candidate = base.join(path).canonicalize().ok()?;
    (candidate.starts_with(&base) && candidate.is_file()).then_some(candidate)
}
pub fn error(status: u16, message: &str) -> Response<Vec<u8>> {
    response(
        status,
        "application/json",
        serde_json::to_vec(&serde_json::json!({"error":message})).unwrap(),
    )
}
fn response(status: u16, kind: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("content-type", kind)
        .header("cache-control", "no-store")
        .header("access-control-allow-origin", "mpac://localhost")
        .header("access-control-allow-methods", "GET, POST, OPTIONS")
        .header(
            "access-control-allow-headers",
            "Authorization, Content-Type",
        )
        .header("content-security-policy", CSP)
        .header("x-content-type-options", "nosniff")
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Mock(AtomicUsize);
    impl Transport for Mock {
        fn request(&self, method: &str, path: &str, body: &[u8]) -> Result<NativeResponse> {
            assert_eq!(
                (method, path, body),
                ("POST", "/api/test", b"{}".as_slice())
            );
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(NativeResponse {
                status: 200,
                content_type: "application/json".into(),
                body: b"{}".to_vec(),
            })
        }
    }
    #[test]
    fn renderer_session_is_ephemeral_and_bad_auth_never_reaches_transport() {
        let gateway = Gateway::new(PathBuf::new());
        let client = Arc::new(Mock(AtomicUsize::new(0)));
        let first = gateway.install(client.clone());
        let token = first.fragment().unwrap().strip_prefix("token=").unwrap();
        let req = || {
            Request::builder()
                .method("POST")
                .uri("mpac://localhost/api/test")
                .header("authorization", format!("Bearer {token}"))
                .body(b"{}".to_vec())
                .unwrap()
        };
        assert_eq!(gateway.serve(req()).status(), 200);
        assert_eq!(client.0.load(Ordering::SeqCst), 1);
        let next = gateway.install(Arc::new(Mock(AtomicUsize::new(0))));
        assert_ne!(first.fragment(), next.fragment());
        assert_eq!(gateway.serve(req()).status(), 401);
        assert_eq!(client.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            gateway
                .serve(
                    Request::builder()
                        .uri("mpac://localhost/api/state")
                        .body(Vec::new())
                        .unwrap()
                )
                .status(),
            401
        );
    }
    #[test]
    fn bridge_address_requires_gui_auth_and_uses_only_exact_csp_origin() {
        let gateway = Gateway::new(PathBuf::new());
        let address = gateway.install(Arc::new(Mock(AtomicUsize::new(0))));
        let token = address.fragment().unwrap().strip_prefix("token=").unwrap();
        let lease = gateway.stream_lease(token).unwrap();
        gateway.set_bridge_port(43210);
        let request = |authorization: &str| {
            Request::builder()
                .uri("mpac://localhost/api/native/terminal")
                .header("authorization", authorization)
                .body(Vec::new())
                .unwrap()
        };
        assert_eq!(gateway.serve(request("Bearer invalid")).status(), 401);
        let response = gateway.serve(request(&format!("Bearer {token}")));
        assert_eq!(response.status(), 200);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(response.body()).unwrap(),
            serde_json::json!({"url":"ws://127.0.0.1:43210/terminal"})
        );
        let csp = response.headers()["content-security-policy"]
            .to_str()
            .unwrap();
        assert!(csp.contains("connect-src 'self' ws://127.0.0.1:43210;"));
        assert!(!csp.contains("ws://127.0.0.1:*"));
        gateway.clear();
        assert!(*lease.cancelled.borrow());
        assert!(gateway.stream_lease(token).is_none());
    }

    #[test]
    fn assets_and_protocol_reject_escaping_paths_and_foreign_urls() {
        let folder = std::env::temp_dir().join(format!("mpac-gateway-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(folder.join("assets")).unwrap();
        fs::write(folder.join("index.html"), "bundled only").unwrap();
        fs::write(folder.join("assets/app.js"), "safe").unwrap();
        assert!(asset_path(&folder, "/assets/app.js").is_some());
        for path in [
            "/../secret",
            "/assets/../../secret",
            "/assets/%2e%2e/secret",
            "//etc/passwd",
        ] {
            assert!(asset_path(&folder, path).is_none());
        }
        let gateway = Gateway::new(folder.clone());
        assert_eq!(
            gateway
                .serve(
                    Request::builder()
                        .uri("mpac://localhost/")
                        .body(Vec::new())
                        .unwrap()
                )
                .body(),
            b"bundled only"
        );
        assert_eq!(
            gateway
                .serve(
                    Request::builder()
                        .uri("http://127.0.0.1:4317/")
                        .body(Vec::new())
                        .unwrap()
                )
                .status(),
            403
        );
        fs::remove_dir_all(folder).unwrap();
    }
}
