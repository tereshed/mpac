use super::*;
use axum::http::{HeaderValue, Request};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{self, client::IntoClientRequest},
};

#[test]
fn peer_filter_never_accepts_foreign_origin_host_or_query_tokens() {
    let mut headers = HeaderMap::new();
    headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:4317"));
    let uri = "/api/sessions/fixture/terminal".parse().unwrap();
    assert!(valid_peer(&headers, "127.0.0.1:4317", &uri));
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("http://127.0.0.1:4317"),
    );
    assert!(valid_peer(&headers, "127.0.0.1:4317", &uri));
    for origin in ["null", "https://127.0.0.1:4317", "http://evil.example"] {
        headers.insert(header::ORIGIN, origin.parse().unwrap());
        assert!(!valid_peer(&headers, "127.0.0.1:4317", &uri));
    }
    headers.remove(header::ORIGIN);
    assert!(!valid_peer(&headers, "127.0.0.1:9999", &uri));
    let query = "/api/sessions/fixture/terminal?token=secret"
        .parse()
        .unwrap();
    assert!(!valid_peer(&headers, "127.0.0.1:4317", &query));
    assert!(authenticated("secret", "secret"));
    assert!(!authenticated("secrex", "secret"));
    assert!(!authenticated("secret-extra", "secret"));
}

#[cfg(unix)]
struct Fixture {
    app: Arc<App>,
    live: Arc<crate::pty::LiveSession>,
    input: std::sync::mpsc::Receiver<Vec<u8>>,
    resize: std::sync::mpsc::Receiver<portable_pty::PtySize>,
    task: tokio::task::JoinHandle<()>,
    directory: std::path::PathBuf,
}
#[cfg(unix)]
impl Fixture {
    async fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("mpac-ws-{}", crate::model::id()));
        std::fs::create_dir(&directory).unwrap();
        let root = directory.canonicalize().unwrap();
        let store = Arc::new(crate::store::Store::open(&root.join("state.sqlite3")).unwrap());
        let project = crate::model::Project {
            id: "project".into(),
            name: "Owned stream fixture".into(),
            roots: vec![mpac_workspace::Root {
                id: "root".into(),
                name: "root".into(),
                path: root.to_str().unwrap().into(),
            }],
        };
        store.project(&project).unwrap();
        let meta: crate::model::Session = serde_json::from_value(json!({
            "id":"session", "project_id":"project", "root_id":"root", "agent":"demo",
            "title":"Owned fixture; no process launched", "status":"running", "started_at":1,
            "last_activity":1,"seq":0,"mode":"interactive_pty"
        }))
        .unwrap();
        store.session(&meta).unwrap();
        let (live, input, resize) = crate::pty::LiveSession::test_fixture_channels(meta, &root);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let app = Arc::new(App::new(
            store,
            "private-fixture-token".into(),
            listener.local_addr().unwrap().to_string(),
        ));
        app.live
            .lock()
            .unwrap()
            .insert("session".into(), live.clone());
        let router = crate::api::router(app.clone(), &root);
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            app,
            live,
            input,
            resize,
            task,
            directory,
        }
    }
    fn request(&self, id: &str) -> Request<()> {
        let mut request = format!("ws://{}/api/sessions/{id}/terminal", self.app.host)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            header::ORIGIN,
            format!("http://{}", self.app.host).parse().unwrap(),
        );
        request
    }
    async fn connect(&self, id: &str) -> WebSocketStream<MaybeTlsStream<TcpStream>> {
        tokio_tungstenite::connect_async(self.request(id))
            .await
            .unwrap()
            .0
    }
    async fn authenticated(&self, after: u64) -> WebSocketStream<MaybeTlsStream<TcpStream>> {
        let mut socket = self.connect("session").await;
        send_json(
            &mut socket,
            json!({"type":"auth","token":self.app.token,"after":after}),
        )
        .await;
        assert_eq!(next_json(&mut socket).await, json!({"type":"ready"}));
        socket
    }
}
#[cfg(unix)]
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
        self.live.stop();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
async fn send_json(socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>, value: Value) {
    socket
        .send(tungstenite::Message::Text(value.to_string().into()))
        .await
        .unwrap();
}
async fn next_json(socket: &mut WebSocketStream<MaybeTlsStream<TcpStream>>) -> Value {
    let value = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(value.to_text().unwrap()).unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn no_data_before_auth_or_for_invalid_tokens_even_on_existing_session() {
    let fixture = Fixture::new().await;
    fixture.live.append_test_output(b"private existing output");
    let mut socket = fixture.connect("session").await;
    assert!(
        tokio::time::timeout(Duration::from_millis(75), socket.next())
            .await
            .is_err()
    );
    send_json(
        &mut socket,
        json!({"type":"auth","token":"wrong","after":0}),
    )
    .await;
    let closed = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .unwrap();
    assert!(!matches!(closed, Some(Ok(tungstenite::Message::Text(_)))));
    assert!(fixture.input.try_recv().is_err());
    let mut socket = fixture.connect("does-not-exist").await;
    send_json(
        &mut socket,
        json!({"type":"auth","token":"wrong","after":0}),
    )
    .await;
    let closed = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .unwrap();
    assert!(!matches!(closed, Some(Ok(tungstenite::Message::Text(_)))));
}

#[cfg(unix)]
#[tokio::test]
async fn output_is_event_driven_resumable_and_backpressured_without_owning_cli() {
    let fixture = Fixture::new().await;
    fixture.live.append_test_output(b"prefix");
    let mut socket = fixture.authenticated(6).await;
    fixture.live.append_test_output(&vec![b'x'; 70 * 1024]);
    let first = next_json(&mut socket).await;
    assert_eq!(first["type"], "output");
    assert_eq!(first["seq"], 6 + 32 * 1024);
    assert_eq!(first["reset"], false);
    assert!(
        tokio::time::timeout(Duration::from_millis(75), socket.next())
            .await
            .is_err()
    );
    send_json(&mut socket, json!({"type":"ack","seq":first["seq"]})).await;
    let second = next_json(&mut socket).await;
    assert_eq!(second["seq"], 6 + 64 * 1024);
    socket.close(None).await.unwrap();
    assert!(!fixture.live.stop_requested());
    assert!(fixture.live.meta().active());
    let mut resumed = fixture.authenticated(second["seq"].as_u64().unwrap()).await;
    let tail = next_json(&mut resumed).await;
    assert_eq!(tail["seq"], 6 + 70 * 1024);
    send_json(&mut resumed, json!({"type":"ack","seq":tail["seq"]})).await;
    fixture.live.finish_test_output();
    assert_eq!(next_json(&mut resumed).await, json!({"type":"end"}));
}

#[cfg(unix)]
#[tokio::test]
async fn a_passive_viewer_receives_idle_geometry_changes_without_any_cli_input() {
    let fixture = Fixture::new().await;
    fixture.live.append_test_output(b"prompt");
    let mut socket = fixture.authenticated(0).await;
    let first = next_json(&mut socket).await;
    assert_eq!(first["geometry"], json!({"cols":100,"rows":24}));
    send_json(&mut socket, json!({"type":"ack","seq":first["seq"]})).await;
    fixture.live.resize_test_output(180, 50);
    let resized = next_json(&mut socket).await;
    assert_eq!(resized["geometry"], json!({"cols":180,"rows":50}));
    assert_eq!(resized["data_base64"], "");
    assert_eq!(resized["seq"], first["seq"]);
    assert!(fixture.input.try_recv().is_err());
    assert!(fixture.resize.try_recv().is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn commands_are_acknowledged_only_after_queueing_and_never_replayed() {
    let fixture = Fixture::new().await;
    let mut socket = fixture.authenticated(0).await;
    send_json(&mut socket, json!({"type":"input","id":1,"data":"first\r"})).await;
    assert_eq!(next_json(&mut socket).await, json!({"type":"ack","id":1}));
    assert_eq!(fixture.input.try_recv().unwrap(), b"first\r");
    send_json(
        &mut socket,
        json!({"type":"resize","id":2,"cols":100,"rows":30}),
    )
    .await;
    assert_eq!(next_json(&mut socket).await, json!({"type":"ack","id":2}));
    assert_eq!(fixture.resize.try_recv().unwrap().cols, 100);
    send_json(
        &mut socket,
        json!({"type":"input","id":2,"data":"must not replay"}),
    )
    .await;
    assert_eq!(next_json(&mut socket).await["type"], "error");
    assert!(fixture.input.try_recv().is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_interrupts_idle_and_slow_viewers_without_waiting_for_ack() {
    let fixture = Fixture::new().await;
    let mut socket = fixture.authenticated(0).await;
    fixture.live.append_test_output(b"unacknowledged");
    assert_eq!(next_json(&mut socket).await["type"], "output");
    fixture.app.stop_all();
    let closed = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .unwrap();
    assert!(!matches!(closed, Some(Ok(tungstenite::Message::Text(_)))));
}

#[cfg(unix)]
#[tokio::test]
async fn reconnect_to_stopped_session_still_replays_final_tail() {
    let fixture = Fixture::new().await;
    fixture.live.append_test_output(b"final tail after stop");
    fixture.live.stop();
    fixture.live.finish_test_output();
    let mut socket = fixture.authenticated(0).await;
    let output = next_json(&mut socket).await;
    assert_eq!(output["type"], "output");
    assert_eq!(output["seq"], 21);
    send_json(&mut socket, json!({"type":"ack","seq":output["seq"]})).await;
    assert_eq!(next_json(&mut socket).await, json!({"type":"end"}));
}

#[cfg(unix)]
#[tokio::test]
async fn invalid_sizes_are_rejected_without_mutating_input_or_resize_queues() {
    let fixture = Fixture::new().await;
    let mut socket = fixture.authenticated(0).await;
    send_json(
        &mut socket,
        json!({"type":"input","id":1,"data":"x".repeat(16*1024+1)}),
    )
    .await;
    assert_eq!(next_json(&mut socket).await["type"], "error");
    assert!(fixture.input.try_recv().is_err());
    send_json(
        &mut socket,
        json!({"type":"resize","id":2,"cols":301,"rows":30}),
    )
    .await;
    assert_eq!(next_json(&mut socket).await["type"], "error");
    assert!(fixture.resize.try_recv().is_err());
    send_json(
        &mut socket,
        json!({"type":"input","id":3,"data":"still works"}),
    )
    .await;
    assert_eq!(next_json(&mut socket).await, json!({"type":"ack","id":3}));
    assert_eq!(fixture.input.try_recv().unwrap(), b"still works");
}
