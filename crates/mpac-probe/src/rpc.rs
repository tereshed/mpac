use anyhow::{Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

pub fn diagnostic_request(id: u64, method: &str, params: Value) -> Result<Value> {
    // This executable cannot start turns, log in/out, refresh credentials or
    // execute tools. Live inference belongs to a separately authorized spike.
    if !matches!(method, "initialize" | "account/read") {
        bail!("method_not_allowed");
    }
    if method == "account/read" && params.get("refreshToken") != Some(&Value::Bool(false)) {
        bail!("token_refresh_not_allowed");
    }
    Ok(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
}

pub async fn send(writer: &mut (impl AsyncWrite + Unpin), value: &Value) -> Result<()> {
    let mut data = serde_json::to_vec(value)?;
    data.push(b'\n');
    writer.write_all(&data).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_frame(reader: &mut (impl AsyncBufRead + Unpin)) -> Result<Value> {
    let mut line = Vec::new();
    loop {
        let bytes = reader.fill_buf().await?;
        if bytes.is_empty() {
            bail!("unexpected_eof");
        }
        let end = bytes.iter().position(|&c| c == b'\n').map(|n| n + 1);
        let count = end.unwrap_or(bytes.len());
        if line.len() + count > MAX_FRAME_BYTES {
            bail!("frame_limit_exceeded");
        }
        line.extend_from_slice(&bytes[..count]);
        reader.consume(count);
        if end.is_some() {
            if line.iter().all(u8::is_ascii_whitespace) {
                line.clear();
                continue;
            }
            return serde_json::from_slice(&line)
                .map_err(|_| anyhow::anyhow!("invalid_json_frame"));
        }
    }
}

pub async fn response(reader: &mut (impl AsyncBufRead + Unpin), id: u64) -> Result<Value> {
    for _ in 0..128 {
        let frame = read_frame(reader).await?;
        if frame.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if frame.get("error").is_some() {
            bail!("rpc_request_rejected");
        }
        return frame
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("result_missing"));
    }
    bail!("notification_limit_exceeded")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[test]
    fn metadata_probe_cannot_send_inference_or_modify_auth() {
        for method in [
            "turn/start",
            "thread/start",
            "account/login/start",
            "account/logout",
            "session/prompt",
        ] {
            assert!(diagnostic_request(1, method, json!({})).is_err());
        }
        assert!(diagnostic_request(1, "account/read", json!({"refreshToken":true})).is_err());
        assert!(diagnostic_request(1, "account/read", json!({"refreshToken":false})).is_ok());
    }

    #[tokio::test]
    async fn notifications_and_other_ids_do_not_impersonate_response() {
        let bytes = b"{\"method\":\"notice\",\"params\":{}}\n{\"id\":9,\"result\":{\"wrong\":true}}\n{\"id\":1,\"result\":{\"ok\":true}}\n";
        let mut reader = BufReader::new(&bytes[..]);
        assert_eq!(response(&mut reader, 1).await.unwrap(), json!({"ok":true}));
    }

    #[tokio::test]
    async fn oversize_and_partial_frames_fail_closed() {
        let data = vec![b'x'; MAX_FRAME_BYTES + 1];
        assert!(read_frame(&mut BufReader::new(&data[..])).await.is_err());
        assert!(
            read_frame(&mut BufReader::new(&b"{\"id\":1"[..]))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn upstream_error_body_is_never_exposed() {
        let bytes = b"{\"id\":1,\"error\":{\"message\":\"sk-secret user@example.com\"}}\n";
        let error = response(&mut BufReader::new(&bytes[..]), 1)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(error, "rpc_request_rejected");
    }
}
