//! Version-scoped observers for Codex native lifecycle hooks.
//!
//! A hook reports that an operation was requested, not that every other hook
//! accepted it. In particular, `Stop` may continue the turn and is never used
//! as a successful-result signal. Successful root turns are confirmed by the
//! native TUI's final-only OSC 9 notification instead.

use std::collections::VecDeque;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::activity::{ActivityPhase, ActivityState, AttentionCertainty, AttentionKind};

const MAX_ID_BYTES: usize = 256;
const RETIRED_TURNS: usize = 64;
const SOURCE: &str = "codex_hook";

#[derive(Clone, Debug, Default)]
pub(crate) struct CodexRuntime {
    native_session: Option<String>,
    retired_sessions: VecDeque<String>,
    current_turn: Option<String>,
    retired_turns: VecDeque<String>,
    completed_turn: Option<String>,
    stop_candidate: Option<String>,
    pending_questions: Vec<String>,
    ambiguous_question: bool,
    active_tools: Vec<(String, String)>,
    pending_permissions: Vec<String>,
    ambiguous_permission: bool,
}

fn identifier<'a>(value: &'a Value, name: &str) -> Result<Option<&'a str>> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text))
            if !text.is_empty()
                && text.len() <= MAX_ID_BYTES
                && !text.chars().any(char::is_control) =>
        {
            Ok(Some(text))
        }
        _ => bail!("invalid Codex activity identifier: {name}"),
    }
}

impl CodexRuntime {
    fn retire_current(&mut self) {
        if let Some(turn) = self.current_turn.take() {
            self.retired_turns.push_back(turn);
            while self.retired_turns.len() > RETIRED_TURNS {
                self.retired_turns.pop_front();
            }
        }
        self.pending_questions.clear();
        self.ambiguous_question = false;
        self.stop_candidate = None;
        self.active_tools.clear();
        self.pending_permissions.clear();
        self.ambiguous_permission = false;
    }

    fn clear_pending(&mut self) {
        self.pending_questions.clear();
        self.ambiguous_question = false;
        self.pending_permissions.clear();
        self.ambiguous_permission = false;
    }
}

impl ActivityState {
    pub(crate) fn accepted_codex_stop(&self, value: &Value) -> bool {
        let turn = value.get("turn_id").and_then(Value::as_str);
        turn.is_some()
            && self.codex.stop_candidate.as_deref() == turn
            && self.codex.current_turn.as_deref() == turn
            && self.codex.completed_turn.as_deref() != turn
            && self.codex.native_session.as_deref()
                == value.get("session_id").and_then(Value::as_str)
            && value.get("agent_id").is_none_or(Value::is_null)
            && value.get("agent_type").is_none_or(Value::is_null)
    }

    /// Consume metadata from an authenticated, launch-owned hook channel.
    /// Prompt, transcript and tool argument contents are intentionally ignored.
    pub(crate) fn apply_codex(&mut self, value: &Value, now: u64) -> Result<bool> {
        let mut next = self.clone();
        let changed = next.apply_codex_inner(value, now)?;
        // Stop and ignored delayed events can change correlation without a
        // visible state change. Commit runtime even when `changed` is false.
        *self = next;
        Ok(changed)
    }

    fn apply_codex_inner(&mut self, value: &Value, now: u64) -> Result<bool> {
        let Some(event) = value.get("hook_event_name").and_then(Value::as_str) else {
            bail!("missing Codex hook event name");
        };
        if !matches!(
            event,
            "SessionStart"
                | "SessionEnd"
                | "UserPromptSubmit"
                | "PreToolUse"
                | "PostToolUse"
                | "PermissionRequest"
                | "PreModelRequest"
                | "Stop"
                | "Interrupt"
        ) {
            return Ok(false);
        }
        // Codex subagents share the parent's session_id. Session id alone is
        // therefore insufficient to distinguish root-turn lifecycle events.
        if identifier(value, "agent_id")?.is_some() || identifier(value, "agent_type")?.is_some() {
            return Ok(false);
        }
        let Some(session) = identifier(value, "session_id")? else {
            bail!("missing Codex activity session id");
        };
        let turn = identifier(value, "turn_id")?;
        let tool = identifier(value, "tool_name")?;
        let tool_id = identifier(value, "tool_use_id")?;

        // Validate before mutating runtime correlation. Untrusted/disabled hooks
        // may first become visible midway through an existing native session.
        if !matches!(event, "SessionStart" | "SessionEnd") && turn.is_none() {
            bail!("missing Codex activity turn id");
        }
        if matches!(event, "PreToolUse" | "PostToolUse" | "PermissionRequest") && tool.is_none() {
            bail!("missing Codex activity tool name");
        }
        if matches!(event, "PreToolUse" | "PostToolUse") && tool_id.is_none() {
            bail!("missing Codex activity tool id");
        }
        if event == "SessionStart"
            && !matches!(
                value.get("source").and_then(Value::as_str),
                Some("startup" | "clear" | "resume" | "fork" | "compact")
            )
        {
            bail!("invalid Codex session start source");
        }

        let mut reset_session = false;
        if self.codex.retired_sessions.iter().any(|old| old == session) {
            // An authenticated explicit /resume may return to a conversation
            // visited earlier in this PTY. Ordinary delayed events still
            // cannot rebind ownership to a retired native session.
            if event == "SessionStart"
                && value.get("source").and_then(Value::as_str) == Some("resume")
            {
                self.codex.retired_sessions.retain(|old| old != session);
            } else {
                self.codex.stop_candidate = None;
                return Ok(false);
            }
        }
        if let Some(bound) = self.codex.native_session.as_deref()
            && bound != session
        {
            // /clear, /resume, /fork and compaction can switch the native
            // root. An unrelated delayed event cannot switch ownership.
            let reset = event == "SessionStart"
                && matches!(
                    value.get("source").and_then(Value::as_str),
                    Some("clear" | "resume" | "fork" | "compact")
                );
            if !reset {
                self.codex.stop_candidate = None;
                return Ok(false);
            }
            let mut retired_sessions = std::mem::take(&mut self.codex.retired_sessions);
            if let Some(previous) = self.codex.native_session.take() {
                retired_sessions.push_back(previous);
            }
            while retired_sessions.len() > RETIRED_TURNS {
                retired_sessions.pop_front();
            }
            self.codex = CodexRuntime {
                retired_sessions,
                ..CodexRuntime::default()
            };
            reset_session = true;
        }
        let first_session = self.codex.native_session.is_none();
        self.codex.native_session = Some(session.to_owned());

        if event == "SessionStart" {
            let explicit_epoch = matches!(
                value.get("source").and_then(Value::as_str),
                Some("clear" | "resume" | "fork")
            );
            if reset_session || explicit_epoch {
                // Resuming the same native conversation is still an explicit
                // lifecycle boundary. Retire its previous turn so delayed
                // tool callbacks cannot repopulate the new session view.
                self.codex.retire_current();
                self.begin_tool_epoch(now);
                let changed = self.clear_attention(SOURCE, now);
                return Ok(self.set_phase(ActivityPhase::Unknown, SOURCE, now) || changed);
            }
            if first_session {
                self.begin_tool_epoch(now);
                return Ok(self.set_phase(ActivityPhase::Unknown, SOURCE, now));
            }
            return Ok(false);
        }
        if event == "SessionEnd" {
            self.codex.retire_current();
            self.clear_activity_tools(now);
            let changed = self.clear_attention(SOURCE, now);
            // Native thread/session lifecycle is distinct from CLI process
            // liveness; /clear or /resume can unload a thread in a live PTY.
            return Ok(self.set_phase(ActivityPhase::Unknown, SOURCE, now) || changed);
        }

        let turn = turn.expect("validated turn id");
        if self.codex.retired_turns.iter().any(|old| old == turn) {
            return Ok(false);
        }
        if event == "UserPromptSubmit" && self.codex.current_turn.as_deref() == Some(turn) {
            self.codex.stop_candidate = None;
            // A duplicate or delayed submit cannot erase a question that a
            // later event of this same native turn has already established.
            return Ok(false);
        }
        let new_turn = self.codex.current_turn.as_deref() != Some(turn);
        if new_turn {
            if self.codex.current_turn.is_some()
                && self.codex.completed_turn.as_deref() != self.codex.current_turn.as_deref()
                && event != "UserPromptSubmit"
            {
                return Ok(false);
            }
            self.codex.retire_current();
            self.begin_tool_epoch(now);
            self.codex.current_turn = Some(turn.to_owned());
            self.codex.completed_turn = None;
        } else if self.codex.completed_turn.as_deref() == Some(turn) {
            // Synchronously dispatched hooks can still reach HTTP out of order
            // relative to PTY reads; they must not resurrect a completed turn.
            return Ok(false);
        }

        let changed = match event {
            "UserPromptSubmit" | "PreModelRequest" => {
                self.codex.stop_candidate = None;
                self.codex.clear_pending();
                let changed = self.clear_attention(SOURCE, now);
                self.set_phase(ActivityPhase::Working, SOURCE, now) || changed
            }
            "PermissionRequest" => {
                self.codex.stop_candidate = None;
                let candidates: Vec<_> = self
                    .codex
                    .active_tools
                    .iter()
                    .filter(|(_, name)| Some(name.as_str()) == tool)
                    .map(|(id, _)| id.clone())
                    .collect();
                if candidates.len() == 1 {
                    if !self.codex.pending_permissions.contains(&candidates[0]) {
                        self.codex.pending_permissions.push(candidates[0].clone());
                    }
                } else {
                    // No tool_use_id is supplied on PermissionRequest. Same-name
                    // parallel calls cannot be resolved by name alone.
                    self.codex.ambiguous_permission = true;
                }
                let changed = self.set_phase(ActivityPhase::Working, SOURCE, now);
                if !self.codex.pending_questions.is_empty() || self.codex.ambiguous_question {
                    changed
                } else {
                    self.set_attention(
                        AttentionKind::Permission,
                        AttentionCertainty::Requested,
                        SOURCE,
                        now,
                    ) || changed
                }
            }
            "PreToolUse" => {
                let id = tool_id.expect("validated tool id");
                if self.tool_was_finished(id) {
                    return Ok(false);
                }
                self.observe_tool_start(id, tool.expect("validated tool name"), now)?;
                self.codex.stop_candidate = None;
                self.codex.active_tools.retain(|(old, _)| old != id);
                if self.codex.active_tools.len() < RETIRED_TURNS {
                    self.codex
                        .active_tools
                        .push((id.to_owned(), tool.unwrap().to_owned()));
                } else {
                    // Resource limits reduce certainty rather than erasing an
                    // outstanding interaction that can no longer be correlated.
                    self.codex.ambiguous_permission = true;
                }
                let changed = self.set_phase(ActivityPhase::Working, SOURCE, now);
                if tool == Some("request_user_input") {
                    if !self
                        .codex
                        .pending_questions
                        .iter()
                        .any(|question| question == id)
                    {
                        if self.codex.pending_questions.len() < RETIRED_TURNS {
                            self.codex.pending_questions.push(id.to_owned());
                        } else {
                            self.codex.ambiguous_question = true;
                        }
                    }
                    self.set_attention(
                        AttentionKind::Question,
                        AttentionCertainty::Requested,
                        SOURCE,
                        now,
                    ) || changed
                } else {
                    changed
                }
            }
            "PostToolUse" => {
                self.codex.stop_candidate = None;
                self.observe_tool_end(tool_id.expect("validated tool id"), now);
                let question_done = self
                    .codex
                    .pending_questions
                    .iter()
                    .any(|id| Some(id.as_str()) == tool_id);
                let permission_done = self
                    .codex
                    .pending_permissions
                    .iter()
                    .any(|id| Some(id.as_str()) == tool_id);
                self.codex
                    .active_tools
                    .retain(|(id, _)| Some(id.as_str()) != tool_id);
                if question_done {
                    self.codex
                        .pending_questions
                        .retain(|id| Some(id.as_str()) != tool_id);
                }
                if permission_done {
                    self.codex
                        .pending_permissions
                        .retain(|id| Some(id.as_str()) != tool_id);
                }
                let changed = if (question_done || permission_done)
                    && self.codex.pending_questions.is_empty()
                    && !self.codex.ambiguous_question
                    && self.codex.pending_permissions.is_empty()
                    && !self.codex.ambiguous_permission
                {
                    self.clear_attention(SOURCE, now)
                } else if question_done
                    && self.codex.pending_questions.is_empty()
                    && !self.codex.ambiguous_question
                {
                    self.set_attention(
                        AttentionKind::Permission,
                        AttentionCertainty::Requested,
                        SOURCE,
                        now,
                    )
                } else {
                    false
                };
                self.set_phase(ActivityPhase::Working, SOURCE, now) || changed
            }
            "Interrupt" => {
                self.codex.retire_current();
                self.clear_activity_tools(now);
                let changed = self.clear_attention(SOURCE, now);
                // Native interrupt does not establish a successful new result.
                self.set_phase(ActivityPhase::Idle, SOURCE, now) || changed
            }
            // Other Stop hooks may add a continuation. Only final-only native
            // OSC9 completion can confirm a successful result in this adapter.
            "Stop" => {
                self.codex.stop_candidate = Some(turn.to_owned());
                self.clear_activity_tools(now)
            }
            _ => unreachable!("event allowlist handled above"),
        };
        Ok(changed)
    }

    /// Called only for the launch's configured root TUI final-turn OSC9 event.
    /// OSC9 is ID-less and may also appear in tool output. Require an
    /// authenticated root Stop candidate for the same current turn; without
    /// trusted hooks the semantic state remains unknown.
    pub(crate) fn apply_codex_completion(&mut self, now: u64) -> bool {
        // OSC9 carries no native turn/request id. A notification from a
        // previous response can arrive after a new turn asks a question. It
        // cannot resolve, complete, or hide that pending native request.
        if self.attention.is_some() {
            return false;
        }
        let Some(turn) = self.codex.current_turn.as_ref() else {
            return false;
        };
        if self.codex.stop_candidate.as_ref() != Some(turn)
            || self.codex.completed_turn.as_ref() == Some(turn)
        {
            return false;
        }
        let key = format!(
            "codex:{}:{turn}",
            self.codex.native_session.as_deref().unwrap_or("")
        );
        self.codex.stop_candidate = None;
        self.codex.completed_turn = self.codex.current_turn.clone();
        self.codex.clear_pending();
        self.clear_activity_tools(now);
        let changed = self.set_phase(ActivityPhase::Idle, "codex_native_notification", now);
        self.record_result(Some(&key), now) || changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(name: &str, turn: &str) -> Value {
        json!({"hook_event_name":name,"session_id":"root","turn_id":turn})
    }

    #[test]
    fn stop_is_not_completion_and_delayed_hooks_do_not_resurrect_turn() {
        let mut state = ActivityState::default();
        assert!(
            state
                .apply_codex(&event("UserPromptSubmit", "one"), 1)
                .unwrap()
        );
        assert!(!state.apply_codex(&event("Stop", "one"), 2).unwrap());
        assert_eq!(state.result.revision, 0);
        assert!(state.apply_codex_completion(3));
        assert!(!state.apply_codex_completion(4));
        assert!(
            !state
                .apply_codex(&event("UserPromptSubmit", "one"), 5)
                .unwrap()
        );
        assert!(matches!(state.phase, ActivityPhase::Idle));
        assert!(
            state
                .apply_codex(&event("UserPromptSubmit", "two"), 6)
                .unwrap()
        );
        assert!(!state.apply_codex(&event("Stop", "one"), 7).unwrap());
        assert!(matches!(state.phase, ActivityPhase::Working));
    }

    #[test]
    fn child_and_foreign_session_events_are_ignored() {
        let mut state = ActivityState::default();
        state
            .apply_codex(&event("UserPromptSubmit", "one"), 1)
            .unwrap();
        let mut child = event("Interrupt", "one");
        child["agent_id"] = json!("child-id");
        assert!(!state.apply_codex(&child, 2).unwrap());
        let mut foreign = event("SessionEnd", "one");
        foreign["session_id"] = json!("foreign");
        assert!(!state.apply_codex(&foreign, 3).unwrap());
        assert!(matches!(state.phase, ActivityPhase::Working));
    }

    #[test]
    fn question_is_requested_and_only_matching_tool_completion_clears_it() {
        let mut state = ActivityState::default();
        let mut request = event("PreToolUse", "one");
        request["tool_name"] = json!("request_user_input");
        request["tool_use_id"] = json!("q1");
        assert!(state.apply_codex(&request, 1).unwrap());
        assert!(matches!(
            state.attention.as_ref().unwrap().kind,
            AttentionKind::Question
        ));
        assert!(matches!(
            state.attention.as_ref().unwrap().certainty,
            AttentionCertainty::Requested
        ));
        let mut unrelated = request.clone();
        unrelated["hook_event_name"] = json!("PostToolUse");
        unrelated["tool_use_id"] = json!("other");
        state.apply_codex(&unrelated, 2).unwrap();
        assert!(state.attention.is_some());
        request["hook_event_name"] = json!("PostToolUse");
        state.apply_codex(&request, 3).unwrap();
        assert!(state.attention.is_none());
    }

    #[test]
    fn malformed_event_does_not_bind_runtime_and_interrupt_has_no_result() {
        let mut state = ActivityState::default();
        let mut bad = event("PreToolUse", "one");
        bad["session_id"] = json!("invalid\nidentifier");
        assert!(state.apply_codex(&bad, 1).is_err());
        assert!(state.codex.native_session.is_none());
        state
            .apply_codex(&event("UserPromptSubmit", "one"), 2)
            .unwrap();
        state.apply_codex(&event("Interrupt", "one"), 3).unwrap();
        assert!(state.codex.current_turn.is_none());
        assert!(matches!(state.phase, ActivityPhase::Idle));
        assert_eq!(state.result.revision, 0);
    }

    #[test]
    fn late_completion_and_duplicate_submit_cannot_hide_pending_question() {
        let mut state = ActivityState::default();
        state
            .apply_codex(&event("UserPromptSubmit", "one"), 1)
            .unwrap();
        let mut question = event("PreToolUse", "one");
        question["tool_name"] = json!("request_user_input");
        question["tool_use_id"] = json!("question");
        state.apply_codex(&question, 2).unwrap();
        assert!(!state.apply_codex_completion(3));
        assert!(
            !state
                .apply_codex(&event("UserPromptSubmit", "one"), 4)
                .unwrap()
        );
        assert!(state.attention.is_some());
        assert_eq!(state.result.revision, 0);
        assert!(matches!(state.phase, ActivityPhase::Working));
    }

    #[test]
    fn parallel_approval_with_no_unique_native_id_is_not_cleared_by_name() {
        let mut state = ActivityState::default();
        for id in ["a", "b"] {
            let mut tool = event("PreToolUse", "one");
            tool["tool_name"] = json!("Bash");
            tool["tool_use_id"] = json!(id);
            state.apply_codex(&tool, 1).unwrap();
        }
        let mut permission = event("PermissionRequest", "one");
        permission["tool_name"] = json!("Bash");
        state.apply_codex(&permission, 2).unwrap();
        let mut complete = event("PostToolUse", "one");
        complete["tool_name"] = json!("Bash");
        complete["tool_use_id"] = json!("a");
        state.apply_codex(&complete, 3).unwrap();
        assert!(state.attention.is_some());
        state
            .apply_codex(&event("PreModelRequest", "one"), 4)
            .unwrap();
        assert!(state.attention.is_none());
    }

    #[test]
    fn native_session_reset_rejects_delayed_previous_session_lifecycle() {
        let mut state = ActivityState::default();
        state
            .apply_codex(&event("UserPromptSubmit", "one"), 1)
            .unwrap();
        let mut reset = event("SessionStart", "unused");
        reset["session_id"] = json!("new");
        reset["source"] = json!("clear");
        assert!(state.apply_codex(&reset, 2).unwrap());
        assert!(matches!(state.phase, ActivityPhase::Unknown));
        let mut old = reset;
        old["session_id"] = json!("root");
        assert!(!state.apply_codex(&old, 3).unwrap());
        assert_eq!(state.codex.native_session.as_deref(), Some("new"));
    }

    #[test]
    fn osc_without_authenticated_stop_candidate_is_not_a_result() {
        let mut state = ActivityState::default();
        assert!(!state.apply_codex_completion(1));
        assert!(matches!(state.phase, ActivityPhase::Unknown));
        state
            .apply_codex(&event("UserPromptSubmit", "one"), 2)
            .unwrap();
        assert!(!state.apply_codex_completion(3));
        assert!(matches!(state.phase, ActivityPhase::Working));
        assert_eq!(state.result.revision, 0);
    }

    #[test]
    fn resumed_work_invalidates_stop_candidate() {
        let mut state = ActivityState::default();
        state
            .apply_codex(&event("UserPromptSubmit", "one"), 1)
            .unwrap();
        state.apply_codex(&event("Stop", "one"), 2).unwrap();
        state
            .apply_codex(&event("PreModelRequest", "one"), 3)
            .unwrap();
        assert!(!state.apply_codex_completion(4));
        assert!(matches!(state.phase, ActivityPhase::Working));
        state.apply_codex(&event("Stop", "one"), 5).unwrap();
        assert!(state.apply_codex_completion(6));
        assert_eq!(state.result.revision, 1);
        assert!(!state.apply_codex_completion(7));
    }

    #[test]
    fn child_and_stale_stop_never_authorize_osc_completion() {
        let mut state = ActivityState::default();
        state
            .apply_codex(&event("UserPromptSubmit", "one"), 1)
            .unwrap();
        let mut child = event("Stop", "one");
        child["agent_id"] = json!("child");
        assert!(!state.apply_codex(&child, 2).unwrap());
        assert!(!state.apply_codex_completion(3));
        state.apply_codex(&event("Stop", "one"), 4).unwrap();
        state
            .apply_codex(&event("UserPromptSubmit", "two"), 5)
            .unwrap();
        assert!(!state.apply_codex(&event("Stop", "one"), 6).unwrap());
        assert!(!state.apply_codex_completion(7));
        assert_eq!(state.result.revision, 0);
    }

    #[test]
    fn native_session_end_does_not_claim_that_cli_process_exited() {
        let mut state = ActivityState::default();
        state
            .apply_codex(&event("UserPromptSubmit", "one"), 1)
            .unwrap();
        state.apply_codex(&event("Stop", "one"), 2).unwrap();
        state.apply_codex(&event("SessionEnd", "one"), 3).unwrap();
        assert!(matches!(state.phase, ActivityPhase::Unknown));
        assert_eq!(state.result.revision, 0);
        assert!(!state.apply_codex_completion(4));
    }

    #[test]
    fn explicit_resume_can_return_to_previously_visited_native_conversation() {
        let mut state = ActivityState::default();
        state
            .apply_codex(&event("UserPromptSubmit", "a-first"), 1)
            .unwrap();
        let mut resume = event("SessionStart", "unused");
        resume["source"] = json!("resume");
        resume["session_id"] = json!("b");
        assert!(state.apply_codex(&resume, 2).unwrap());
        assert!(!state.apply_codex(&event("Stop", "a-first"), 3).unwrap());
        resume["session_id"] = json!("root");
        assert!(state.apply_codex(&resume, 4).unwrap());
        assert_eq!(state.codex.native_session.as_deref(), Some("root"));
        assert!(!state.codex.retired_sessions.iter().any(|old| old == "root"));
        assert!(
            state
                .apply_codex(&event("UserPromptSubmit", "a-second"), 5)
                .unwrap()
        );
        let mut old_b = event("SessionEnd", "b-old");
        old_b["session_id"] = json!("b");
        assert!(!state.apply_codex(&old_b, 6).unwrap());
        assert!(matches!(state.phase, ActivityPhase::Working));
    }

    fn tool_event(name: &str, turn: &str, id: &str) -> Value {
        let mut value = event(name, turn);
        value["tool_name"] = json!("exec_command");
        value["tool_use_id"] = json!(id);
        value["tool_input"] = json!({"command":"secret command argument"});
        value["tool_response"] = json!({"output":"secret tool output"});
        value
    }

    #[test]
    fn current_tool_metadata_is_idempotent_and_excludes_arguments_and_output() {
        let mut state = ActivityState::default();
        let start = tool_event("PreToolUse", "one", "call-a");
        state.apply_codex(&start, 1).unwrap();
        state.apply_codex(&start, 2).unwrap();
        assert_eq!(state.tools.len(), 1);
        assert_eq!(state.tools[0].started_at, 1);
        assert_eq!(
            serde_json::to_value(&state).unwrap()["tools"][0],
            json!({"id":"call-a","name":"exec_command","started_at":1})
        );
        let encoded = serde_json::to_string(&state).unwrap();
        assert!(!encoded.contains("secret command"));
        assert!(!encoded.contains("secret tool"));
        state
            .apply_codex(&tool_event("PostToolUse", "one", "call-a"), 3)
            .unwrap();
        assert!(state.tools.is_empty());
        assert!(!state.apply_codex(&start, 4).unwrap());
    }

    #[test]
    fn root_stop_clears_calls_but_keeps_candidate_and_pending_question() {
        let mut state = ActivityState::default();
        let mut question = tool_event("PreToolUse", "one", "question");
        question["tool_name"] = json!("request_user_input");
        state.apply_codex(&question, 1).unwrap();
        assert!(state.apply_codex(&event("Stop", "one"), 2).unwrap());
        assert!(state.tools.is_empty());
        assert!(state.attention.is_some());
        assert_eq!(state.codex.stop_candidate.as_deref(), Some("one"));
        assert!(!state.apply_codex_completion(3));
        assert!(!state.apply_codex(&question, 4).unwrap());
        assert!(state.attention.is_some());
    }

    #[test]
    fn tool_projection_ignores_children_stale_turns_and_clears_on_interrupt_and_switch() {
        let mut state = ActivityState::default();
        state
            .apply_codex(&tool_event("PreToolUse", "one", "parent"), 1)
            .unwrap();
        let mut child = tool_event("PostToolUse", "one", "parent");
        child["agent_id"] = json!("child");
        assert!(!state.apply_codex(&child, 2).unwrap());
        assert_eq!(state.tools.len(), 1);
        state
            .apply_codex(&event("UserPromptSubmit", "two"), 3)
            .unwrap();
        assert!(state.tools.is_empty());
        assert!(
            !state
                .apply_codex(&tool_event("PreToolUse", "one", "old"), 4)
                .unwrap()
        );
        state
            .apply_codex(&tool_event("PreToolUse", "two", "next"), 5)
            .unwrap();
        state.apply_codex(&event("Interrupt", "two"), 6).unwrap();
        assert!(state.tools.is_empty());
        state
            .apply_codex(&tool_event("PreToolUse", "three", "switch"), 7)
            .unwrap();
        let mut resume = event("SessionStart", "unused");
        resume["session_id"] = json!("other");
        resume["source"] = json!("resume");
        state.apply_codex(&resume, 8).unwrap();
        assert!(state.tools.is_empty());
    }

    #[test]
    fn oversized_tool_metadata_rolls_back_runtime_and_visible_state() {
        let mut state = ActivityState::default();
        for i in 0..64 {
            state
                .apply_codex(&tool_event("PreToolUse", "one", &format!("call-{i}")), 1)
                .unwrap();
        }
        let before = serde_json::to_value(&state).unwrap();
        let active = state.codex.active_tools.clone();
        assert!(
            state
                .apply_codex(&tool_event("PreToolUse", "one", "overflow"), 2)
                .is_err()
        );
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        assert_eq!(state.codex.active_tools, active);
    }

    #[test]
    fn explicit_same_session_resume_retires_calls_questions_and_old_turn() {
        let mut state = ActivityState::default();
        let mut question = tool_event("PreToolUse", "one", "question");
        question["tool_name"] = json!("request_user_input");
        state.apply_codex(&question, 1).unwrap();
        assert!(state.attention.is_some());
        let mut resume = event("SessionStart", "unused");
        resume["source"] = json!("resume");
        assert!(state.apply_codex(&resume, 2).unwrap());
        assert!(state.tools.is_empty());
        assert!(state.attention.is_none());
        assert_eq!(state.phase, ActivityPhase::Unknown);
        assert_eq!(state.tools_observed_at, Some(2));
        assert!(state.codex.current_turn.is_none());
        assert!(!state.apply_codex(&question, 3).unwrap());
        assert!(!state.apply_codex(&event("Stop", "one"), 4).unwrap());
        assert!(!state.apply_codex_completion(5));
        assert!(
            state
                .apply_codex(&tool_event("PreToolUse", "two", "new-call"), 6)
                .unwrap()
        );
        let mut duplicate_start = event("SessionStart", "unused");
        duplicate_start["source"] = json!("startup");
        assert!(!state.apply_codex(&duplicate_start, 7).unwrap());
        assert_eq!(state.tools[0].id, "new-call");
    }
}
