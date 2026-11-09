//! Bounded streaming native terminal status frames. Native hook events supply
//! authenticated session/turn ownership; an arbitrary printed notification
//! alone never proves a successful agent response.
use crate::activity::ActivityState;

#[derive(Default)]
pub struct ActivityOscParser {
    escape: bool,
    osc: bool,
    osc_escape: bool,
    discard: bool,
    replied: bool,
    payload: Vec<u8>,
}

#[derive(Default)]
pub struct Effects {
    pub changed: bool,
    pub reply_capability: bool,
}

impl ActivityOscParser {
    pub fn push(
        &mut self,
        bytes: &[u8],
        state: &mut ActivityState,
        agent: &str,
        now: u64,
    ) -> Effects {
        let mut effects = Effects::default();
        // The PTY reader may drain queued frames after the process waiter has
        // finalized metadata. Those frames cannot resurrect a dead execution.
        if !state.capabilities.foreground || state.phase == crate::activity::ActivityPhase::Stopped
        {
            return effects;
        }
        for &byte in bytes {
            if self.osc {
                if (self.osc_escape && byte == b'\\') || byte == 7 {
                    if !self.discard {
                        if agent == "claude" && self.payload == b"7501;?" && !self.replied {
                            self.replied = true;
                            effects.reply_capability = true;
                        } else if agent == "claude" && state.has_claude_session() {
                            if let Some(status) = self
                                .payload
                                .strip_prefix(b"7501;")
                                .and_then(crate::auto_title::root_status)
                            {
                                effects.changed |= state.apply_native_status(status, now);
                            }
                        } else if agent == "codex"
                            && let Some(text) = self.payload.strip_prefix(b"9;")
                            && !text.is_empty()
                            && !text.starts_with(b"4;")
                            && std::str::from_utf8(text)
                                .is_ok_and(|text| !text.chars().any(char::is_control))
                        {
                            effects.changed |= state.apply_codex_completion(now);
                        }
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
                self.escape = byte == 0x1b;
            }
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::ActivityCapabilities;
    use serde_json::json;

    fn claude() -> ActivityState {
        let mut state = ActivityState::new(
            "claude_hooks",
            ActivityCapabilities {
                foreground: true,
                questions: true,
                approval: true,
                results: true,
                background: true,
            },
            1,
        );
        state
            .apply_claude(
                &json!({"hook_event_name":"SessionStart","session_id":"owned","source":"startup"}),
                1,
            )
            .unwrap();
        state
    }
    #[test]
    fn arbitrary_notifications_and_child_status_never_create_results() {
        let mut state = claude();
        let mut parser = ActivityOscParser::default();
        parser.push(b"\x1b]7501;state=working:app=claude-code:id=child\x07\x1b]7501;state=done:app=claude-code:id=child\x07\x1b]9;Response ready\x07",&mut state,"claude",2);
        assert_eq!(state.result.revision, 0);
        let mut codex = ActivityState::new(
            "codex_hooks",
            ActivityCapabilities {
                foreground: true,
                results: true,
                ..Default::default()
            },
            1,
        );
        parser.push(b"\x1b]9;Response ready\x07", &mut codex, "codex", 2);
        assert_eq!(codex.result.revision, 0);
    }
    #[test]
    fn queued_pty_frames_cannot_resurrect_finalized_process() {
        let mut state = claude();
        state.process_ended(5);
        let mut parser = ActivityOscParser::default();
        assert!(!parser.push(b"\x1b]7501;state=working:app=claude-code\x07\x1b]7501;state=idle:app=claude-code\x07", &mut state, "claude", 6).changed);
        assert_eq!(state.phase, crate::activity::ActivityPhase::Stopped);
        assert_eq!(state.source, "process_exit");
    }
    #[test]
    fn split_capability_reply_is_once_and_oversized_frame_recovers() {
        let mut state = claude();
        let mut parser = ActivityOscParser::default();
        assert!(
            !parser
                .push(b"\x1b]7501;", &mut state, "claude", 1)
                .reply_capability
        );
        assert!(
            parser
                .push(b"?\x1b\\", &mut state, "claude", 1)
                .reply_capability
        );
        assert!(
            !parser
                .push(b"\x1b]7501;?\x07", &mut state, "claude", 1)
                .reply_capability
        );
        let mut bytes = b"\x1b]7501;".to_vec();
        bytes.extend(vec![b'x'; 5000]);
        bytes.extend_from_slice(b"\x07\x1b]7501;state=working:app=claude-code\x07");
        assert!(parser.push(&bytes, &mut state, "claude", 2).changed);
        assert_eq!(state.phase, crate::activity::ActivityPhase::Working);
    }
}
