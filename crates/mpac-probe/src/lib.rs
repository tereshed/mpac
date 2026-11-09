pub mod contracts;
pub mod probe;
pub mod rpc;
pub mod terminal;

/// Only known descriptors may cross the diagnostic boundary. Raw account
/// responses, errors, output and arbitrary provider strings never do.
pub fn known_value<'a>(value: Option<&str>, allowed: &'a [&str]) -> &'a str {
    value
        .and_then(|v| allowed.iter().copied().find(|a| *a == v))
        .unwrap_or("unknown")
}

pub fn version_number(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|word| {
        let word = word.trim_start_matches('v');
        (word.len() <= 48
            && word.starts_with(|c: char| c.is_ascii_digit())
            && word.contains('.')
            && word
                .chars()
                .all(|c| c.is_ascii_digit() || ".-+rcbetaalphadev".contains(c)))
        .then(|| word.to_owned())
    })
}

pub fn safe_error_code(error: &anyhow::Error) -> &'static str {
    known_value(
        Some(&error.to_string()),
        &[
            "executable_not_found",
            "spawn_failed",
            "process_timeout",
            "process_stop_failed",
            "process_reap_timeout",
            "process_reap_failed",
            "output_limit_exceeded",
            "handshake_timeout",
            "unexpected_eof",
            "frame_limit_exceeded",
            "invalid_json_frame",
            "rpc_request_rejected",
            "result_missing",
            "notification_limit_exceeded",
            "invalid_status_response",
            "pty_timeout",
            "fixture_path_required",
            "unknown_command",
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_provider_values_are_not_reported() {
        assert_eq!(
            known_value(Some("sk-secret@example.com"), &["chatgpt", "apiKey"]),
            "unknown"
        );
        assert_eq!(
            known_value(Some("chatgpt"), &["chatgpt", "apiKey"]),
            "chatgpt"
        );
    }

    #[test]
    fn version_does_not_include_banner_or_identity() {
        assert_eq!(
            version_number("codex-cli 0.160.0\nuser@example.com sk-secret"),
            Some("0.160.0".into())
        );
        assert_eq!(version_number("token sk-secret email@example.com"), None);
    }

    #[test]
    fn error_classifier_does_not_forward_raw_upstream_messages() {
        assert_eq!(
            safe_error_code(&anyhow::anyhow!("user@example.com sk-secret")),
            "unknown"
        );
        assert_eq!(
            safe_error_code(&anyhow::anyhow!("handshake_timeout")),
            "handshake_timeout"
        );
    }
}
