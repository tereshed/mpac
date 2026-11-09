//! A GUI-lifetime loopback WebSocket. Only a short-lived renderer key crosses
//! this boundary; the daemon credential and verified upstream socket stay Rust.
use crate::gateway::{Gateway, StreamLease, Transport};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Router,
    extract::{
        OriginalUri, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    net::{Ipv4Addr, TcpListener},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, oneshot};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

const AUTH_TIMEOUT: Duration = Duration::from_secs(3);
const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const FRAME_LIMIT: usize = 24 * 1024;
const OUTPUT_LIMIT: usize = 64 * 1024;
const MAX_SAFE_ID: u64 = 9_007_199_254_740_991;

pub struct Bridge {
    shutdown: Option<oneshot::Sender<()>>,
}
impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}
#[derive(Clone)]
struct BridgeState {
    gateway: Arc<Gateway>,
    host: String,
    connections: Arc<Semaphore>,
}
impl Bridge {
    pub fn start(gateway: Arc<Gateway>) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let (shutdown, stopped) = oneshot::channel();
        gateway.set_bridge_port(port);
        std::thread::Builder::new()
            .name("mpac-terminal-bridge".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    let state = BridgeState {
                        gateway,
                        host: format!("127.0.0.1:{port}"),
                        connections: Arc::new(Semaphore::new(8)),
                    };
                    let app = Router::new()
                        .route("/terminal", get(upgrade))
                        .with_state(state);
                    tokio::select! {
                        _ = axum::serve(listener, app) => {},
                        _ = stopped => {},
                    }
                });
                runtime.shutdown_timeout(Duration::from_secs(1));
            })?;
        Ok(Self {
            shutdown: Some(shutdown),
        })
    }
}
fn permitted_headers(headers: &HeaderMap, expected_host: &str) -> bool {
    headers.get("host").and_then(|v| v.to_str().ok()) == Some(expected_host)
        && matches!(
            headers.get("origin").and_then(|v| v.to_str().ok()),
            Some("mpac://localhost") | Some("null")
        )
}
async fn upgrade(
    State(state): State<BridgeState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if uri.query().is_some() || !permitted_headers(&headers, &state.host) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(permit) = state.connections.try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    ws.max_message_size(FRAME_LIMIT)
        .max_frame_size(FRAME_LIMIT)
        .write_buffer_size(0)
        .max_write_buffer_size(128 * 1024)
        .on_upgrade(move |mut socket| async move {
            let _permit = permit;
            let _ = serve(&mut socket, state.gateway).await;
            let _ = tokio::time::timeout(SEND_TIMEOUT, socket.close()).await;
        })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Authentication {
    #[serde(rename = "type")]
    kind: String,
    token: String,
    session_id: String,
    after: u64,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ClientFrame {
    Input { id: u64, data: String },
    Resize { id: u64, cols: u16, rows: u16 },
    Ack { seq: u64 },
    Ping,
}
fn validate_command(frame: &ClientFrame, previous: &mut u64) -> Result<()> {
    let id = match frame {
        ClientFrame::Input { id, data } => {
            ensure!(data.len() <= 16 * 1024, "Input is too large");
            Some(*id)
        }
        ClientFrame::Resize { id, cols, rows } => {
            ensure!(
                (20..=300).contains(cols) && (5..=150).contains(rows),
                "Invalid terminal size"
            );
            Some(*id)
        }
        ClientFrame::Ack { seq } => {
            ensure!(*seq <= MAX_SAFE_ID, "Invalid terminal sequence");
            None
        }
        ClientFrame::Ping => None,
    };
    if let Some(id) = id {
        ensure!(
            id > *previous && id <= MAX_SAFE_ID,
            "Invalid terminal command order"
        );
        *previous = id;
    }
    Ok(())
}
struct FrameRate {
    started: Instant,
    count: usize,
}
impl FrameRate {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            count: 0,
        }
    }
    fn accept(&mut self) -> Result<()> {
        if self.started.elapsed() >= Duration::from_secs(1) {
            self.started = Instant::now();
            self.count = 0;
        }
        self.count += 1;
        ensure!(self.count <= 512, "Terminal message rate exceeded");
        Ok(())
    }
}

async fn send(socket: &mut WebSocket, value: Value) -> Result<()> {
    tokio::time::timeout(
        SEND_TIMEOUT,
        socket.send(Message::Text(value.to_string().into())),
    )
    .await??;
    Ok(())
}
async fn request(
    client: Arc<dyn Transport>,
    method: &'static str,
    path: String,
    body: Vec<u8>,
) -> Result<Value> {
    let result =
        tokio::task::spawn_blocking(move || client.request(method, &path, &body)).await??;
    ensure!(
        (200..300).contains(&result.status),
        "Terminal service rejected the request"
    );
    serde_json::from_slice(&result.body).context("Invalid terminal service response")
}
async fn captured_request(
    lease: &mut StreamLease,
    method: &'static str,
    path: String,
    body: Vec<u8>,
) -> Result<Value> {
    ensure!(!*lease.cancelled.borrow(), "Terminal connection changed");
    tokio::select! {
        biased;
        _ = lease.cancelled.changed() => { bail!("Terminal connection changed"); },
        result = request(lease.client.clone(), method, path, body) => result,
    }
}

async fn serve(socket: &mut WebSocket, gateway: Arc<Gateway>) -> Result<()> {
    let first = tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await?;
    let Some(Ok(Message::Text(text))) = first else {
        bail!("Terminal authentication required");
    };
    let auth: Authentication = serde_json::from_str(&text)?;
    ensure!(
        auth.kind == "auth" && auth.after <= MAX_SAFE_ID,
        "Invalid terminal authentication"
    );
    let id = uuid::Uuid::parse_str(&auth.session_id)?;
    ensure!(
        id.to_string() == auth.session_id,
        "Invalid terminal session"
    );
    let mut lease = gateway
        .stream_lease(&auth.token)
        .context("Terminal connection changed")?;
    // Neither /state nor a session lookup is sent before renderer auth succeeds.
    let state = captured_request(&mut lease, "GET", "/api/state".into(), Vec::new()).await?;
    if state
        .pointer("/capabilities/terminal_stream")
        .and_then(Value::as_bool)
        == Some(true)
    {
        relay(socket, lease, auth.session_id, auth.after).await
    } else {
        compatibility(socket, lease, auth.session_id, auth.after).await
    }
}
async fn relay(
    socket: &mut WebSocket,
    mut lease: StreamLease,
    session_id: String,
    after: u64,
) -> Result<()> {
    let client = lease.client.clone();
    let verified = tokio::select! {
        _ = lease.cancelled.changed() => { return Ok(()); },
        result = tokio::task::spawn_blocking(move || client.terminal_socket(&session_id)) => result??,
    };
    // client_async is given the already-verified socket: it cannot reconnect or
    // follow a redirect to another listener before writing the credential.
    let tcp = tokio::net::TcpStream::from_std(verified.stream)?;
    let mut handshake = verified.uri.into_client_request()?;
    handshake
        .headers_mut()
        .insert("origin", verified.origin.parse()?);
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(128 * 1024))
        .max_frame_size(Some(128 * 1024))
        .write_buffer_size(0)
        .max_write_buffer_size(256 * 1024);
    let (mut upstream, _) = tokio::time::timeout(
        AUTH_TIMEOUT,
        tokio_tungstenite::client_async_with_config(handshake, tcp, Some(config)),
    )
    .await??;
    tokio::time::timeout(
        SEND_TIMEOUT,
        upstream.send(tungstenite::Message::Text(
            json!({"type":"auth","token":verified.token,"after":after})
                .to_string()
                .into(),
        )),
    )
    .await??;
    let ready = tokio::select! {
        _ = lease.cancelled.changed() => { return Ok(()); },
        result = tokio::time::timeout(AUTH_TIMEOUT, upstream.next()) => result?.context("Terminal service closed")??,
    };
    let tungstenite::Message::Text(ready) = ready else {
        bail!("Terminal service did not authenticate");
    };
    let mut ready: Value = serde_json::from_str(&ready)?;
    if ready["type"] != "ready" {
        send(
            socket,
            json!({"type":"error","message":"Terminal service did not authenticate"}),
        )
        .await?;
        return Ok(());
    }
    ready["transport"] = json!("websocket");
    send(socket, ready).await?;
    let mut previous = 0;
    let mut rate = FrameRate::new();
    loop {
        tokio::select! {
            _ = lease.cancelled.changed() => break,
            next = upstream.next() => {
                match next {
                    Some(Ok(tungstenite::Message::Text(text))) => {
                        let mut value: Value = serde_json::from_str(&text)?;
                        if value["type"] == "ready" { value["transport"] = json!("websocket"); }
                        send(socket, value).await?;
                    },
                    Some(Ok(tungstenite::Message::Ping(_))) => { tokio::time::timeout(SEND_TIMEOUT, upstream.flush()).await??; },
                    Some(Ok(tungstenite::Message::Close(_))) | None => break,
                    Some(Err(error)) => return Err(error.into()),
                    _ => {},
                }
            },
            next = socket.recv() => {
                match next {
                    Some(Ok(Message::Text(text))) => {
                        rate.accept()?;
                        let frame: ClientFrame = serde_json::from_str(&text)?;
                        validate_command(&frame, &mut previous)?;
                        tokio::time::timeout(SEND_TIMEOUT, upstream.send(tungstenite::Message::Text(text.to_string().into()))).await??;
                    },
                    Some(Ok(Message::Ping(_))) => {},
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(error)) => return Err(error.into()),
                    _ => bail!("Unsupported terminal frame"),
                }
            },
        }
    }
    Ok(())
}
async fn compatibility(
    socket: &mut WebSocket,
    mut lease: StreamLease,
    session_id: String,
    mut seq: u64,
) -> Result<()> {
    send(socket, json!({"type":"ready","transport":"compatibility"})).await?;
    let mut previous = 0;
    let mut awaiting: Option<(u64, Instant)> = None;
    let mut next_poll = Instant::now();
    let mut busy_until = Instant::now() + Duration::from_millis(600);
    let mut status_at = Instant::now();
    let mut rate = FrameRate::new();
    loop {
        let delay = awaiting
            .map(|(_, deadline)| deadline)
            .unwrap_or(next_poll)
            .saturating_duration_since(Instant::now());
        tokio::select! {
            _ = lease.cancelled.changed() => break,
            next = socket.recv() => {
                let frame = match next {
                    Some(Ok(Message::Text(text))) => serde_json::from_str::<ClientFrame>(&text)?,
                    Some(Ok(Message::Ping(_))) => continue,
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(error)) => return Err(error.into()),
                    _ => bail!("Unsupported terminal frame"),
                };
                rate.accept()?;
                validate_command(&frame, &mut previous)?;
                match frame {
                    ClientFrame::Input { id, data } => {
                        let response = captured_request(&mut lease, "POST", format!("/api/sessions/{session_id}/input"), serde_json::to_vec(&json!({"data":data}))?).await;
                        // A failed or uncertain POST is never retried. The UI
                        // retains its own unsent draft and must ask the user.
                        if response.is_err() { send(socket,json!({"type":"error","id":id,"message":"Terminal input was not confirmed; check the CLI before retrying"})).await?; break; }
                        send(socket,json!({"type":"ack","id":id})).await?;
                        busy_until = Instant::now() + Duration::from_millis(600);
                        next_poll = Instant::now();
                    },
                    ClientFrame::Resize { id, cols, rows } => {
                        let response = captured_request(&mut lease, "POST", format!("/api/sessions/{session_id}/resize"), serde_json::to_vec(&json!({"cols":cols,"rows":rows}))?).await;
                        if response.is_err() { send(socket,json!({"type":"error","id":id,"message":"Terminal size update was not confirmed"})).await?; break; }
                        send(socket,json!({"type":"ack","id":id})).await?;
                        next_poll = Instant::now();
                    },
                    ClientFrame::Ack { seq: accepted } => {
                        ensure!(awaiting.map(|(seq, _)| seq) == Some(accepted), "Unexpected terminal output acknowledgement");
                        awaiting = None;
                        next_poll = Instant::now();
                    },
                    ClientFrame::Ping => { send(socket,json!({"type":"pong"})).await?; },
                }
            },
            _ = tokio::time::sleep(delay) => {
                if awaiting.is_some() { break; }
                let result = captured_request(&mut lease, "GET", format!("/api/sessions/{session_id}/output?after={seq}"), Vec::new()).await?;
                let data = result["data_base64"].as_str().context("Invalid terminal output")?;
                ensure!(data.len() <= OUTPUT_LIMIT * 2, "Terminal output is too large");
                let next = result["seq"].as_u64().context("Invalid terminal output position")?;
                ensure!(next <= MAX_SAFE_ID, "Invalid terminal output position");
                let reset = result["reset"].as_bool().context("Invalid terminal output reset")?;
                if !data.is_empty() || reset {
                    send(socket,json!({"type":"output","data_base64":data,"seq":next,"reset":reset})).await?;
                    seq = next;
                    awaiting = Some((seq, Instant::now() + Duration::from_secs(15)));
                    busy_until = Instant::now() + Duration::from_millis(600);
                } else if status_at.elapsed() >= Duration::from_secs(1) {
                    let state = captured_request(&mut lease, "GET", "/api/state".into(), Vec::new()).await?;
                    let active = state["sessions"].as_array().and_then(|sessions| sessions.iter().find(|s|s["id"]==session_id)).and_then(|s|s["status"].as_str()).is_some_and(|s|matches!(s,"running"|"needs_attention"|"starting"));
                    if !active { send(socket,json!({"type":"end"})).await?; break; }
                    status_at = Instant::now();
                }
                next_poll = Instant::now() + if Instant::now() < busy_until { Duration::from_millis(16) } else { Duration::from_millis(120) };
            },
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::NativeResponse;
    use std::{
        path::PathBuf,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    struct FixtureTransport {
        calls: AtomicUsize,
        input: Mutex<Vec<String>>,
    }
    impl Transport for FixtureTransport {
        fn request(&self, method: &str, path: &str, body: &[u8]) -> Result<NativeResponse> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let data = if path == "/api/state" {
                json!({"capabilities":{"terminal_stream":false},"sessions":[{"id":"12345678-1234-4234-8234-123456789abc","status":"running"}]})
            } else if method == "GET" && path.ends_with("/output?after=0") {
                json!({"data_base64":"aGk=","seq":2,"reset":false})
            } else if method == "GET" && path.ends_with("/output?after=2") {
                json!({"data_base64":"","seq":2,"reset":false})
            } else if method == "POST" && path.ends_with("/input") {
                let value: Value = serde_json::from_slice(body)?;
                let value = value["data"].as_str().unwrap().to_owned();
                self.input.lock().unwrap().push(value.clone());
                if value == "uncertain" {
                    bail!("Uncertain fixture outcome");
                }
                json!({})
            } else {
                bail!("Unexpected fixture request");
            };
            Ok(NativeResponse {
                status: 200,
                content_type: "application/json".into(),
                body: serde_json::to_vec(&data)?,
            })
        }
    }
    fn fixture() -> (Arc<Gateway>, Arc<FixtureTransport>, Bridge, String, String) {
        let gateway = Arc::new(Gateway::new(PathBuf::new()));
        let client = Arc::new(FixtureTransport {
            calls: AtomicUsize::new(0),
            input: Mutex::new(vec![]),
        });
        let address = gateway.install(client.clone());
        let token = address
            .fragment()
            .unwrap()
            .strip_prefix("token=")
            .unwrap()
            .to_owned();
        let bridge = Bridge::start(gateway.clone()).unwrap();
        let response = gateway.serve(
            tauri::http::Request::builder()
                .uri("mpac://localhost/api/native/terminal")
                .header("authorization", format!("Bearer {token}"))
                .body(Vec::new())
                .unwrap(),
        );
        assert_eq!(response.status(), 200);
        let value: Value = serde_json::from_slice(response.body()).unwrap();
        let url = value["url"].as_str().unwrap().to_owned();
        (gateway, client, bridge, url, token)
    }
    async fn connect(
        url: &str,
        token: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("origin", "mpac://localhost".parse().unwrap());
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        socket.send(tungstenite::Message::Text(json!({"type":"auth","token":token,"session_id":"12345678-1234-4234-8234-123456789abc","after":0}).to_string().into())).await.unwrap();
        socket
    }
    async fn next(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Value {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    }
    #[tokio::test]
    async fn compatibility_stream_is_authenticated_flow_controlled_and_cancelled_on_reconnect() {
        let (gateway, client, _bridge, url, token) = fixture();
        let mut rejected = connect(&url, "wrong").await;
        let _ = tokio::time::timeout(Duration::from_secs(1), rejected.next())
            .await
            .unwrap();
        assert_eq!(client.calls.load(Ordering::Relaxed), 0);
        let mut socket = connect(&url, &token).await;
        assert_eq!(next(&mut socket).await["transport"], "compatibility");
        assert_eq!(
            next(&mut socket).await,
            json!({"type":"output","data_base64":"aGk=","seq":2,"reset":false})
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(60), socket.next())
                .await
                .is_err()
        );
        socket
            .send(tungstenite::Message::Text(
                json!({"type":"input","id":1,"data":"hello"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        assert_eq!(next(&mut socket).await, json!({"type":"ack","id":1}));
        assert_eq!(*client.input.lock().unwrap(), vec!["hello"]);
        gateway.install(Arc::new(FixtureTransport {
            calls: AtomicUsize::new(0),
            input: Mutex::new(vec![]),
        }));
        let closed = tokio::time::timeout(Duration::from_secs(1), socket.next())
            .await
            .unwrap();
        assert!(matches!(
            closed,
            Some(Ok(tungstenite::Message::Close(_))) | None
        ));
        let count = client.calls.load(Ordering::Relaxed);
        let mut stale = connect(&url, &token).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), stale.next())
            .await
            .unwrap();
        assert_eq!(client.calls.load(Ordering::Relaxed), count);
    }
    #[tokio::test]
    async fn compatibility_stream_never_replays_uncertain_input_and_does_not_stop_execution() {
        let (_gateway, client, _bridge, url, token) = fixture();
        let mut socket = connect(&url, &token).await;
        assert_eq!(next(&mut socket).await["type"], "ready");
        assert_eq!(next(&mut socket).await["type"], "output");
        socket
            .send(tungstenite::Message::Text(
                json!({"type":"input","id":1,"data":"uncertain"})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let error = next(&mut socket).await;
        assert_eq!(error["type"], "error");
        assert_eq!(error["id"], 1);
        let _ = socket.close(None).await;
        assert_eq!(*client.input.lock().unwrap(), vec!["uncertain"]);
    }
    #[test]
    fn bridge_origin_and_command_boundaries_are_strict() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "127.0.0.1:1234".parse().unwrap());
        headers.insert("origin", "mpac://localhost".parse().unwrap());
        assert!(permitted_headers(&headers, "127.0.0.1:1234"));
        for foreign in [
            "http://evil.test",
            "http://127.0.0.1:1234",
            "tauri://localhost",
        ] {
            headers.insert("origin", foreign.parse().unwrap());
            assert!(!permitted_headers(&headers, "127.0.0.1:1234"));
        }
        assert!(!permitted_headers(&headers, "127.0.0.1:5678"));
        let mut previous = 0;
        validate_command(
            &ClientFrame::Input {
                id: 1,
                data: "x".into(),
            },
            &mut previous,
        )
        .unwrap();
        assert!(
            validate_command(
                &ClientFrame::Input {
                    id: 1,
                    data: "x".into()
                },
                &mut previous
            )
            .is_err()
        );
        assert!(
            validate_command(
                &ClientFrame::Resize {
                    id: 2,
                    cols: 1,
                    rows: 20
                },
                &mut previous
            )
            .is_err()
        );
        assert!(
            validate_command(
                &ClientFrame::Input {
                    id: 2,
                    data: "x".repeat(16 * 1024 + 1)
                },
                &mut previous
            )
            .is_err()
        );
    }
}
