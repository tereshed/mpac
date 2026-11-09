//! Authenticated, resumable PTY transport. A viewer never owns CLI lifetime.
use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::App;

const CLIENT_FRAME_LIMIT: usize = 24 * 1024;
const MAX_COMMAND_ID: u64 = 9_007_199_254_740_991;

pub fn routes() -> Router<Arc<App>> {
    Router::new().route("/api/sessions/{id}/terminal", get(upgrade))
}

fn valid_peer(headers: &HeaderMap, host: &str, uri: &Uri) -> bool {
    let expected = format!("http://{host}");
    headers.get(header::HOST).and_then(|v| v.to_str().ok()) == Some(host)
        && headers
            .get(header::ORIGIN)
            .is_none_or(|v| v.to_str().ok() == Some(expected.as_str()))
        && uri.query().is_none()
}

async fn upgrade(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
    ws: WebSocketUpgrade,
) -> Response {
    if app.shutting_down.load(std::sync::atomic::Ordering::Acquire) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if !valid_peer(&headers, &app.host, &uri) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Invalid terminal peer"})),
        )
            .into_response();
    }
    let Ok(permit) = app.terminal_slots.clone().try_acquire_owned() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"Too many terminal viewers"})),
        )
            .into_response();
    };
    ws.max_message_size(CLIENT_FRAME_LIMIT)
        .max_frame_size(CLIENT_FRAME_LIMIT)
        .write_buffer_size(0)
        .max_write_buffer_size(128 * 1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            serve(socket, app, id).await;
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Auth {
    #[serde(rename = "type")]
    kind: String,
    token: String,
    #[serde(default)]
    after: u64,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Input { id: u64, data: String },
    Resize { id: u64, cols: u16, rows: u16 },
    Ack { seq: u64 },
    Ping,
}

fn authenticated(actual: &str, expected: &str) -> bool {
    actual.len() == expected.len()
        && actual
            .bytes()
            .zip(expected.bytes())
            .fold(0_u8, |a, (x, y)| a | (x ^ y))
            == 0
}

async fn send(socket: &mut WebSocket, message: Value) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_secs(1),
            socket.send(Message::Text(message.to_string().into()))
        )
        .await,
        Ok(Ok(()))
    )
}

async fn serve(mut socket: WebSocket, app: Arc<App>, id: String) {
    // No session existence check, output, or input is allowed before auth.
    let auth = match tokio::time::timeout(Duration::from_secs(3), socket.recv()).await {
        Ok(Some(Ok(Message::Text(value)))) => serde_json::from_str::<Auth>(&value).ok(),
        _ => None,
    };
    let Some(auth) =
        auth.filter(|value| value.kind == "auth" && authenticated(&value.token, &app.token))
    else {
        return;
    };
    let session = match app.live_session(&id) {
        Ok(session) => session,
        Err(_) => {
            let _ = send(
                &mut socket,
                json!({"type":"error","message":"Terminal execution is unavailable"}),
            )
            .await;
            return;
        }
    };
    // Subscribe before reading output, so an append between read and await is
    // retained by the watch version and cannot be lost.
    let mut changed = session.subscribe_output();
    let mut cursor = auth.after;
    let mut geometry = session.output(cursor).final_geometry();
    let mut outstanding = None;
    let mut last_command = 0;
    if !send(&mut socket, json!({"type":"ready"})).await {
        return;
    }
    loop {
        if app.shutting_down.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        if outstanding.is_none() {
            let output = session.output(cursor);
            if output.reset || !output.data_base64.is_empty() || output.final_geometry() != geometry
            {
                geometry = output.final_geometry();
                cursor = output.seq;
                outstanding = Some(cursor);
                if !send(&mut socket, json!({"type":"output","data_base64":output.data_base64,"seq":output.seq,"reset":output.reset,"geometry":output.geometry,"resizes":output.resizes})).await {
                    return;
                }
            } else if session.terminal_finished() {
                let _ = send(&mut socket, json!({"type":"end"})).await;
                return;
            }
        }
        tokio::select! {
            change = changed.changed() => {
                if change.is_err() { return; }
            }
            message = socket.recv() => {
                let Some(Ok(message)) = message else { return; };
                match message {
                    Message::Close(_) => return,
                    Message::Ping(value) => {
                        if !matches!(tokio::time::timeout(Duration::from_secs(1), socket.send(Message::Pong(value))).await, Ok(Ok(()))) { return; }
                    }
                    Message::Pong(_) => {},
                    Message::Text(text) => {
                        let Ok(command) = serde_json::from_str::<Command>(&text) else {
                            let _ = send(&mut socket, json!({"type":"error","message":"Invalid terminal command"})).await;
                            return;
                        };
                        match command {
                            Command::Ack { seq } => {
                                if outstanding != Some(seq) {
                                    let _ = send(&mut socket, json!({"type":"error","message":"Invalid output acknowledgement"})).await;
                                    return;
                                }
                                outstanding = None;
                            }
                            Command::Ping => {
                                if !send(&mut socket, json!({"type":"pong"})).await { return; }
                            }
                            Command::Input { id, data } => {
                                if id <= last_command || id > MAX_COMMAND_ID {
                                    let _ = send(&mut socket, json!({"type":"error","id":id,"message":"Invalid command sequence"})).await;
                                    return;
                                }
                                last_command = id;
                                let result = session.input(data);
                                let reply = if result.is_ok() { json!({"type":"ack","id":id}) } else { json!({"type":"error","id":id,"message":"Terminal input was not accepted; check session and queue"}) };
                                if !send(&mut socket, reply).await { return; }
                            }
                            Command::Resize { id, cols, rows } => {
                                if id <= last_command || id > MAX_COMMAND_ID {
                                    let _ = send(&mut socket, json!({"type":"error","id":id,"message":"Invalid command sequence"})).await;
                                    return;
                                }
                                last_command = id;
                                let result = session.resize(cols, rows);
                                let reply = if result.is_ok() { json!({"type":"ack","id":id}) } else { json!({"type":"error","id":id,"message":"Terminal resize was not accepted"}) };
                                if !send(&mut socket, reply).await { return; }
                            }
                        }
                    }
                    _ => return,
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "terminal_stream_tests.rs"]
mod tests;
