//! Completion signals from app-owned CLI instances. Neither notification text
//! nor status messages are retained, executed, or used as session names.
use std::{collections::BTreeMap, path::Path};

use base64::{Engine, engine::general_purpose::STANDARD};

pub const CAPABILITY_REPLY: &[u8] = b"\x1b]7501;?\x1b\\";

#[derive(Default)]
pub struct AutoTitleEffects {
    pub reply_capability: bool,
    pub completed: bool,
}

pub struct AutoTitleTracker {
    claude: bool,
    codex: bool,
    replied: bool,
    working: bool,
    completed: bool,
    escape: bool,
    osc: bool,
    osc_escape: bool,
    discard: bool,
    payload: Vec<u8>,
}

impl AutoTitleTracker {
    pub fn new(agent: &str) -> Self {
        Self {
            claude: matches!(agent, "claude" | "zai"),
            codex: agent == "codex",
            replied: false,
            working: false,
            completed: false,
            escape: false,
            osc: false,
            osc_escape: false,
            discard: false,
            payload: Vec::new(),
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> AutoTitleEffects {
        let mut effects = AutoTitleEffects::default();
        for &byte in bytes {
            if self.osc {
                if (self.osc_escape && byte == b'\\') || byte == 7 {
                    if !self.discard {
                        let payload = std::mem::take(&mut self.payload);
                        self.report(&payload, &mut effects);
                        self.payload = payload;
                    }
                    self.osc = false;
                    self.osc_escape = false;
                    self.discard = false;
                    self.payload.clear();
                } else if byte == 0x1b {
                    self.discard |= self.osc_escape;
                    self.osc_escape = true;
                } else {
                    self.discard |= self.osc_escape;
                    self.osc_escape = false;
                    if !self.discard && self.payload.len() < 4088 {
                        self.payload.push(byte);
                    } else {
                        self.discard = true;
                    }
                }
            } else if self.escape && byte == b']' {
                self.escape = false;
                self.osc = true;
                self.payload.clear();
            } else {
                if self.escape && byte == b'c' {
                    self.working = false;
                }
                self.escape = byte == 0x1b;
            }
        }
        effects.completed = self.completed;
        effects
    }

    fn report(&mut self, payload: &[u8], effects: &mut AutoTitleEffects) {
        if self.claude && payload == b"7501;?" && !self.replied {
            self.replied = true;
            effects.reply_capability = true;
        }
        if self.codex
            && let Some(message) = payload.strip_prefix(b"9;")
            && !message.is_empty()
            && !message.starts_with(b"4;")
            && let Ok(message) = std::str::from_utf8(message)
            && !message.chars().any(char::is_control)
        {
            // Only enable this adapter on unnamed Codex launches configured
            // with the native agent-turn-complete OSC9 notification profile.
            self.completed = true;
        }
        if !self.claude {
            return;
        }
        let Some(fields) = payload.strip_prefix(b"7501;") else {
            return;
        };
        let Some(state) = root_status(fields) else {
            return;
        };
        match state {
            "working" => self.working = true,
            "done" if self.working => {
                self.completed = true;
                self.working = false;
            }
            "idle" | "error" | "clear" => self.working = false,
            _ => {}
        }
    }
}

pub(crate) fn root_status(payload: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(payload).ok()?;
    let mut fields = BTreeMap::new();
    for pair in text.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || key.len() > 16 || !key.bytes().all(|b| b.is_ascii_lowercase()) {
            continue;
        }
        // Resolve duplicates first; validate the resulting complete report.
        fields.insert(key, value);
    }
    // A child report, including a malformed/empty id, can never become root.
    if fields.contains_key("id") {
        return None;
    }
    for (key, limit) in [("title", 192_usize), ("msg", 2048_usize)] {
        if let Some(value) = fields.get(key) {
            let max_encoded = limit.div_ceil(3) * 4;
            if value.len() > max_encoded {
                return None;
            }
            let mut padded = value.to_string();
            while padded.len() % 4 != 0 {
                padded.push('=');
            }
            let decoded = STANDARD.decode(padded).ok()?;
            if decoded.len() > limit
                || std::str::from_utf8(&decoded)
                    .ok()?
                    .chars()
                    .any(char::is_control)
            {
                return None;
            }
        }
    }
    let state = *fields.get("state")?;
    if !matches!(
        state,
        "idle" | "working" | "done" | "blocked" | "error" | "clear"
    ) {
        return None;
    }
    // Claude's native root clear deliberately omits app.
    if state != "clear" && fields.get("app").copied() != Some("claude-code") {
        return None;
    }
    Some(state)
}

/// A generated harness name, excluding startup labels, paths and busy frames.
pub fn candidate_title(raw: &str, cwd: &Path) -> Option<String> {
    let raw = raw.trim();
    let title = raw
        .strip_prefix('✳')
        .map(|s| s.trim_start_matches('\u{fe0f}').trim_start())
        .unwrap_or(raw);
    if title.is_empty()
        || title.chars().any(|c| {
            c.is_control() || matches!(c, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        || title.starts_with(is_busy)
        || title.ends_with(is_busy)
        || [
            "claude",
            "claude code",
            "codex",
            "openai codex",
            "hermes",
            "hermes agent",
        ]
        .iter()
        .any(|generic| title.eq_ignore_ascii_case(generic))
        || cwd.to_str() == Some(title)
        || cwd.file_name().and_then(|s| s.to_str()) == Some(title)
        || title.starts_with('/')
        || title.starts_with("~/")
    {
        return None;
    }
    let mut end = title.len().min(120);
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    Some(title[..end].trim_end().to_owned())
}

fn is_busy(c: char) -> bool {
    matches!(c, '⠋' | '⠙' | '⠹' | '⠸' | '⠼' | '⠴' | '⠦' | '⠧' | '⠇' | '⠏')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(state: &str) -> Vec<u8> {
        format!("\x1b]7501;state={state}:app=claude-code\x07").into_bytes()
    }

    #[test]
    fn claude_requires_root_completed_turn_not_startup_child_or_aborted_work() {
        let mut tracker = AutoTitleTracker::new("claude");
        assert!(!tracker.push(&report("done")).completed);
        tracker.push(&report("working"));
        assert!(
            !tracker
                .push(b"\x1b]7501;state=done:app=claude-code:id=child\x07")
                .completed
        );
        assert!(
            !tracker
                .push(b"\x1b]7501;state=done:app=claude-code:id=\x07")
                .completed
        );
        tracker.push(&report("idle"));
        assert!(!tracker.push(&report("done")).completed);
        tracker.push(&report("working"));
        tracker.push(&report("blocked"));
        assert!(tracker.push(&report("done")).completed);
    }

    #[test]
    fn streaming_handshake_is_fixed_once_and_clear_resets_working() {
        let mut tracker = AutoTitleTracker::new("claude");
        let mut replies = 0;
        for byte in CAPABILITY_REPLY
            .iter()
            .cycle()
            .take(CAPABILITY_REPLY.len() * 2)
        {
            replies += usize::from(tracker.push(&[*byte]).reply_capability);
        }
        assert_eq!(replies, 1);
        tracker.push(&report("working"));
        tracker.push(b"\x1b]7501;state=clear\x1b\\");
        assert!(!tracker.push(&report("done")).completed);
        tracker.push(&report("working"));
        tracker.push(b"\x1bc");
        assert!(!tracker.push(&report("done")).completed);
        for byte in report("working").iter().chain(report("done").iter()) {
            tracker.push(&[*byte]);
        }
        assert!(tracker.push(b"").completed);
    }

    #[test]
    fn validate_whole_status_and_last_duplicate_before_applying() {
        let mut tracker = AutoTitleTracker::new("claude");
        tracker.push(&report("working"));
        for invalid in [
            "state=done:app=claude-code:msg=AA==",
            "state=done:app=claude-code:title=%%%%",
            "state=done:app=claude-code:state=error",
            "state=done:app=codex",
            "state=done",
        ] {
            assert!(
                !tracker
                    .push(format!("\x1b]7501;{invalid}\x07").as_bytes())
                    .completed
            );
        }
        // Previous error reset working; startup done must still not complete.
        assert!(!tracker.push(&report("done")).completed);
        tracker.push(&report("working"));
        assert!(
            tracker
                .push(b"\x1b]7501;state=error:state=done:app=claude-code:future=value:msg=b2s\x07")
                .completed
        );
    }

    #[test]
    fn bounded_recovery_and_codex_adapter_is_scoped() {
        let mut tracker = AutoTitleTracker::new("codex");
        tracker.push(b"\x1b]9;");
        tracker.push(&vec![b'a'; 10_000]);
        assert!(tracker.payload.len() <= 4088);
        assert!(!tracker.push(b"\x07\x1b]9;4;1;20\x07").completed);
        assert!(tracker.push(b"\x1b]9;Response ready\x1b\\").completed);
        assert!(
            !AutoTitleTracker::new("hermes")
                .push(b"\x1b]9;ready\x07")
                .completed
        );
        assert!(
            !AutoTitleTracker::new("claude")
                .push(b"\x1b]9;ready\x07")
                .completed
        );
    }

    #[test]
    fn only_meaningful_nonbusy_native_names_are_promoted() {
        let cwd = Path::new("/Users/me/project");
        for raw in [
            "✳ Claude Code",
            "codex",
            "project",
            "/Users/me/project",
            "Task ⠋",
            "⠋ Task",
            "bad\nname",
        ] {
            assert_eq!(candidate_title(raw, cwd), None, "{raw}");
        }
        assert_eq!(
            candidate_title("✳\u{fe0f} Разбор авторизации", cwd),
            Some("Разбор авторизации".into())
        );
        assert_eq!(
            candidate_title("Разбор задачи", cwd),
            Some("Разбор задачи".into())
        );
    }
}
