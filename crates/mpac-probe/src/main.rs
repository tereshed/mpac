use anyhow::Result;
use mpac_probe::probe;
use serde_json::json;

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("help");
    let executable = args.get(1).map(String::as_str);
    let result: Result<_> = match command {
        "doctor" => Ok(probe::doctor().await),
        "report" => Ok(probe::report().await),
        "codex-handshake" => probe::handshake(executable.unwrap_or("codex"), true).await,
        "claude-auth" => probe::claude_auth(executable.unwrap_or("claude")).await,
        "hermes-handshake" => probe::handshake(executable.unwrap_or("hermes"), false).await,
        "pty-smoke" => probe::pty_smoke(),
        "avt-snapshots" => match executable {
            Some(path) => std::fs::read(path)
                .map_err(anyhow::Error::from)
                .and_then(|bytes| serde_json::from_slice(&bytes).map_err(anyhow::Error::from))
                .and_then(|fixtures| mpac_probe::terminal::export_fixtures(&fixtures)),
            None => Err(anyhow::anyhow!("fixture_path_required")),
        },
        "help" | "--help" | "-h" => {
            println!(
                "mpac-probe <doctor|report|codex-handshake [binary]|claude-auth [binary]|hermes-handshake [binary]|pty-smoke|avt-snapshots fixtures.json>\nPhase 0 diagnostics only. No inference, prompts or token values are emitted."
            );
            return;
        }
        _ => Err(anyhow::anyhow!("unknown_command")),
    };
    match result {
        Ok(value) => println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("serializable result")
        ),
        Err(error) => {
            eprintln!(
                "{}",
                json!({"status":"failed","probe":match command {"codex-handshake"=>"codex-handshake","claude-auth"=>"claude-auth","hermes-handshake"=>"hermes-handshake","pty-smoke"=>"pty-smoke",_=>"unknown"},"reason":mpac_probe::safe_error_code(&error),"raw_output_suppressed":true})
            );
            std::process::exit(1);
        }
    }
}
