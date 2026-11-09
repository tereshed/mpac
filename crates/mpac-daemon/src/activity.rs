//! Observations from a CLI are separate from process liveness and task completion.
//! This module never infers work, success, or monitor health from terminal output.
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};

const MAX_IDS: usize = 128;
const MAX_JOBS: usize = 64;
const MAX_TOOL_OBSERVATIONS: usize = 64;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ActivityCapabilities {
    pub foreground: bool,
    pub questions: bool,
    pub approval: bool,
    pub results: bool,
    pub background: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityPhase {
    #[default]
    Unknown,
    Working,
    Idle,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    Question,
    Permission,
    Input,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionCertainty {
    Requested,
    Confirmed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityAttention {
    pub kind: AttentionKind,
    pub certainty: AttentionCertainty,
    pub since: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ActivityResult {
    pub revision: u64,
    pub read_revision: u64,
    pub at: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundCoverage {
    #[default]
    Unsupported,
    AwaitingSnapshot,
    Snapshot,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundStatus {
    Running,
    Scheduled,
    Completed,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivityJob {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub status: BackgroundStatus,
    pub observed_at: u64,
    pub next_run_at: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ActivityBackground {
    pub coverage: BackgroundCoverage,
    pub observed_at: Option<u64>,
    pub jobs: Vec<ActivityJob>,
}

/// `started_at` is the first observed PreToolUse request, not proof that
/// permission checks finished or that a command actually started executing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityTool {
    pub id: String,
    pub name: String,
    pub started_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ActivityState {
    pub adapter: String,
    pub capabilities: ActivityCapabilities,
    pub phase: ActivityPhase,
    pub attention: Option<ActivityAttention>,
    pub result: ActivityResult,
    pub background: ActivityBackground,
    #[serde(default)]
    pub tools: Vec<ActivityTool>,
    #[serde(default)]
    pub tools_observed_at: Option<u64>,
    pub updated_at: Option<u64>,
    pub source: String,
    #[serde(skip)]
    runtime: Runtime,
    #[serde(skip)]
    pub(crate) codex: crate::codex_activity::CodexRuntime,
}

#[derive(Clone, Debug, Default)]
struct Runtime {
    session: Option<String>,
    prompt: Option<String>,
    previous_prompts: VecDeque<String>,
    previous_sessions: VecDeque<String>,
    generation: u64,
    turn_result_recorded: bool,
    native_done: bool,
    native_attention: Option<ActivityAttention>,
    tools: Vec<Tool>,
    pending: Vec<Interaction>,
    results: VecDeque<String>,
    finished_tools: VecDeque<String>,
}

#[derive(Clone, Debug)]
struct Tool {
    id: String,
    name: String,
    signature: Option<u64>,
}

#[derive(Clone, Debug)]
struct Interaction {
    tool: Option<String>,
    name: String,
    signature: Option<u64>,
    attention: ActivityAttention,
}

impl Default for ActivityState {
    fn default() -> Self {
        Self {
            adapter: "legacy".into(),
            capabilities: ActivityCapabilities::default(),
            phase: ActivityPhase::Unknown,
            attention: None,
            result: ActivityResult::default(),
            background: ActivityBackground::default(),
            tools: Vec::new(),
            tools_observed_at: None,
            updated_at: None,
            source: "unobserved".into(),
            runtime: Runtime::default(),
            codex: Default::default(),
        }
    }
}

impl ActivityState {
    pub fn new(adapter: impl Into<String>, capabilities: ActivityCapabilities, _now: u64) -> Self {
        Self {
            adapter: clean(&adapter.into(), 80),
            capabilities,
            updated_at: None,
            background: ActivityBackground {
                coverage: if capabilities.background {
                    BackgroundCoverage::AwaitingSnapshot
                } else {
                    BackgroundCoverage::Unsupported
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Validate and reduce atomically; malformed input never partially changes state.
    pub fn apply_claude(&mut self, payload: &Value, now: u64) -> Result<bool> {
        let mut next = self.clone();
        if !next.claude_inner(payload, now)? {
            return Ok(false);
        }
        *self = next;
        Ok(true)
    }

    fn claude_inner(&mut self, payload: &Value, now: u64) -> Result<bool> {
        ensure!(payload.is_object(), "CLI event must be an object");
        let event = required_id(payload, "hook_event_name")?;
        if !matches!(
            event.as_str(),
            "SessionStart"
                | "SessionEnd"
                | "UserPromptSubmit"
                | "PreToolUse"
                | "PermissionRequest"
                | "PostToolUse"
                | "PostToolUseFailure"
                | "PermissionDenied"
                | "Stop"
                | "StopFailure"
                | "Notification"
                | "SubagentStart"
                | "SubagentStop"
                | "Elicitation"
                | "ElicitationResult"
        ) {
            return Ok(false);
        }
        let session = required_id(payload, "session_id")?;
        let prompt = optional_id(payload, "prompt_id")?;
        let agent = optional_id(payload, "agent_id")?;
        let main = agent.is_none() && !matches!(event.as_str(), "SubagentStart" | "SubagentStop");
        if !main && event != "SubagentStop" {
            return Ok(false);
        }
        let source = format!("claude_hook:{event}");

        if event == "SessionStart" {
            let origin = required_id(payload, "source")?;
            ensure!(
                matches!(
                    origin.as_str(),
                    "startup" | "resume" | "clear" | "compact" | "fork"
                ),
                "Unknown session start source"
            );
            if let Some(current) = &self.runtime.session {
                if current != &session {
                    ensure!(
                        matches!(origin.as_str(), "clear" | "resume" | "fork"),
                        "Wrong native session"
                    );
                    self.reset_native_session(&session);
                    self.phase = ActivityPhase::Unknown;
                    self.attention = None;
                    self.invalidate_background();
                } else if matches!(origin.as_str(), "clear" | "resume" | "fork") {
                    self.reset_native_session(&session);
                    self.phase = ActivityPhase::Unknown;
                    self.attention = None;
                    self.invalidate_background();
                }
            }
        }
        if let Some(current) = &self.runtime.session {
            ensure!(current == &session, "Wrong native session");
        } else {
            self.runtime.session = Some(session.clone());
        }

        if let Some(id) = &prompt {
            if self.runtime.previous_prompts.contains(id) {
                return Ok(false);
            }
            if main && event == "UserPromptSubmit" {
                if self.runtime.prompt.as_ref() != Some(id) {
                    if let Some(old) = self.runtime.prompt.replace(id.clone()) {
                        bounded_push(&mut self.runtime.previous_prompts, old);
                    }
                    self.runtime.generation = self.runtime.generation.saturating_add(1);
                    self.runtime.turn_result_recorded = false;
                    self.runtime.native_done = false;
                    self.runtime.tools.clear();
                    self.begin_tool_epoch(now);
                    // A new submitted prompt is a new interaction epoch. Old input
                    // requests are not carried into this unrelated turn.
                    self.runtime.pending.clear();
                    self.runtime.native_attention = None;
                    self.attention = None;
                }
            } else if let Some(current) = &self.runtime.prompt {
                if current != id {
                    return Ok(false);
                }
            } else if main {
                self.runtime.prompt = Some(id.clone());
            }
        } else if main && event == "UserPromptSubmit" {
            self.runtime.generation = self.runtime.generation.saturating_add(1);
            self.runtime.turn_result_recorded = false;
            self.runtime.native_done = false;
            self.runtime.tools.clear();
            self.begin_tool_epoch(now);
            self.runtime.pending.clear();
            self.runtime.native_attention = None;
            self.attention = None;
        }

        match event.as_str() {
            "SessionStart" => {
                // A late startup hook for an already active turn must not
                // erase calls observed while SessionStart ran in background.
                if self.runtime.prompt.is_none() && self.tools.is_empty() {
                    self.tools_observed_at = Some(now);
                }
            }
            "SessionEnd" if main => {
                let reason = required_id(payload, "reason")?;
                ensure!(
                    matches!(
                        reason.as_str(),
                        "clear" | "resume" | "logout" | "prompt_input_exit" | "other"
                    ),
                    "Invalid session end reason"
                );
                // /clear and /resume end a conversation without ending its PTY.
                self.phase = ActivityPhase::Unknown;
                self.attention = None;
                self.runtime.pending.clear();
                self.runtime.native_attention = None;
                self.runtime.tools.clear();
                self.clear_activity_tools(now);
                self.invalidate_background();
            }
            "UserPromptSubmit" if main => {
                if self.capabilities.foreground {
                    self.phase = ActivityPhase::Working;
                }
            }
            "PreToolUse" if main => {
                let tool = read_tool(payload, true)?;
                if self.tool_was_finished(&tool.id) {
                    return Ok(false);
                }
                self.observe_tool_start(&tool.id, &tool.name, now)?;
                self.runtime.tools.retain(|old| old.id != tool.id);
                ensure!(
                    self.runtime.tools.len() < MAX_IDS,
                    "Too many active CLI tools"
                );
                if self.capabilities.foreground {
                    self.phase = ActivityPhase::Working;
                }
                let kind = match tool.name.as_str() {
                    "AskUserQuestion" if self.capabilities.questions => {
                        Some(AttentionKind::Question)
                    }
                    "ExitPlanMode" if self.capabilities.approval => Some(AttentionKind::Permission),
                    _ => None,
                };
                if let Some(kind) = kind {
                    self.add_interaction(
                        Some(tool.id.clone()),
                        &tool.name,
                        tool.signature,
                        kind,
                        AttentionCertainty::Requested,
                        now,
                    )?;
                }
                self.runtime.tools.push(tool);
            }
            "PermissionRequest" if main && self.capabilities.approval => {
                let tool = read_tool(payload, false)?;
                let candidates: Vec<_> = self
                    .runtime
                    .tools
                    .iter()
                    .filter(|t| t.name == tool.name && t.signature == tool.signature)
                    .collect();
                let id = if candidates.len() == 1 {
                    Some(candidates[0].id.clone())
                } else {
                    None
                };
                // Preserve a structured question rather than replacing it with
                // the permission stage of the same tool call.
                if !self
                    .runtime
                    .pending
                    .iter()
                    .any(|p| id.is_some() && p.tool == id)
                {
                    self.add_interaction(
                        id,
                        &tool.name,
                        tool.signature,
                        AttentionKind::Permission,
                        AttentionCertainty::Requested,
                        now,
                    )?;
                }
            }
            "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" if main => {
                let tool = read_tool(payload, true)?;
                self.observe_tool_end(&tool.id, now);
                self.resolve_tool(&tool);
                if self.capabilities.foreground {
                    self.phase = ActivityPhase::Working;
                }
                if event == "PostToolUse" && self.capabilities.background {
                    self.observe_cron_result(payload, &tool.name, now)?;
                }
            }
            "Stop" if main => {
                if self.capabilities.results {
                    self.record_claude_turn_result(now);
                }
                // Other Stop hooks may continue the turn. This is evidence of a
                // produced response, not proof of an idle or completed task.
                if !self.runtime.native_done {
                    self.phase = ActivityPhase::Unknown;
                }
                self.clear_activity_tools(now);
                self.observe_background(payload, now)?;
            }
            "SubagentStop" => {
                self.observe_background(payload, now)?;
            }
            "StopFailure" if main => {
                let _ = required_id(payload, "error")?;
                self.phase = ActivityPhase::Unknown;
                self.clear_activity_tools(now);
                self.invalidate_background();
            }
            "Notification" if main => {
                self.observe_notification(payload, now)?;
            }
            "Elicitation" if main && self.capabilities.questions => {
                let server = required_id(payload, "mcp_server_name")?;
                let id =
                    optional_id(payload, "elicitation_id")?.map(|id| format!("elicitation:{id}"));
                self.add_interaction(
                    id,
                    &format!("mcp:{server}"),
                    None,
                    AttentionKind::Input,
                    AttentionCertainty::Requested,
                    now,
                )?;
            }
            "ElicitationResult" if main => {
                let server = required_id(payload, "mcp_server_name")?;
                let id =
                    optional_id(payload, "elicitation_id")?.map(|id| format!("elicitation:{id}"));
                let action = required_id(payload, "action")?;
                ensure!(
                    matches!(action.as_str(), "accept" | "decline" | "cancel"),
                    "Invalid elicitation action"
                );
                self.runtime
                    .pending
                    .retain(|p| !(p.name == format!("mcp:{server}") && p.tool == id));
                self.refresh_attention();
            }
            _ => {
                return Ok(false);
            }
        }
        self.updated_at = Some(now);
        self.source = source;
        Ok(true)
    }

    fn reset_native_session(&mut self, session: &str) {
        let mut previous = std::mem::take(&mut self.runtime.previous_sessions);
        if let Some(old) = self.runtime.session.take() {
            bounded_push(&mut previous, old);
        }
        self.runtime = Runtime {
            session: Some(session.into()),
            previous_sessions: previous,
            ..Default::default()
        };
        self.tools.clear();
        self.tools_observed_at = None;
    }

    pub(crate) fn begin_tool_epoch(&mut self, now: u64) {
        self.tools.clear();
        self.tools_observed_at = Some(now);
        self.runtime.finished_tools.clear();
    }

    pub(crate) fn tool_was_finished(&self, id: &str) -> bool {
        self.runtime.finished_tools.iter().any(|old| old == id)
    }

    pub(crate) fn observe_tool_start(&mut self, id: &str, name: &str, now: u64) -> Result<bool> {
        ensure!(
            !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control),
            "Invalid tool observation ID"
        );
        let name = clean(name, 160);
        ensure!(!name.trim().is_empty(), "Invalid tool observation name");
        if self.tool_was_finished(id) {
            return Ok(false);
        }
        let inserted = !self.tools.iter().any(|tool| tool.id == id);
        if let Some(existing) = self.tools.iter().find(|tool| tool.id == id) {
            ensure!(existing.name == name, "Tool observation name changed");
        } else {
            ensure!(
                self.tools.len() < MAX_TOOL_OBSERVATIONS,
                "Too many active tool observations"
            );
            self.tools.push(ActivityTool {
                id: id.into(),
                name,
                started_at: now,
            });
        }
        let changed = inserted || self.tools_observed_at != Some(now);
        self.tools_observed_at = Some(now);
        Ok(changed)
    }

    pub(crate) fn observe_tool_end(&mut self, id: &str, now: u64) {
        self.tools.retain(|tool| tool.id != id);
        if !self.tool_was_finished(id) {
            bounded_push(&mut self.runtime.finished_tools, id.into());
        }
        self.tools_observed_at = Some(now);
    }

    pub(crate) fn clear_activity_tools(&mut self, now: u64) -> bool {
        if self.tools.is_empty() {
            return false;
        }
        for tool in std::mem::take(&mut self.tools) {
            if !self.tool_was_finished(&tool.id) {
                bounded_push(&mut self.runtime.finished_tools, tool.id);
            }
        }
        self.tools_observed_at = Some(now);
        true
    }

    fn add_interaction(
        &mut self,
        tool: Option<String>,
        name: &str,
        signature: Option<u64>,
        kind: AttentionKind,
        certainty: AttentionCertainty,
        now: u64,
    ) -> Result<()> {
        if let Some(existing) = self
            .runtime
            .pending
            .iter_mut()
            .find(|p| p.tool == tool && p.name == name && p.signature == signature)
        {
            if certainty == AttentionCertainty::Confirmed {
                existing.attention.certainty = certainty;
            }
        } else {
            ensure!(
                self.runtime.pending.len() < MAX_IDS,
                "Too many pending CLI interactions"
            );
            self.runtime.pending.push(Interaction {
                tool,
                name: name.into(),
                signature,
                attention: ActivityAttention {
                    kind,
                    certainty,
                    since: now,
                },
            });
        }
        self.refresh_attention();
        Ok(())
    }

    fn resolve_tool(&mut self, tool: &Tool) {
        let unambiguous = self
            .runtime
            .tools
            .iter()
            .filter(|t| t.name == tool.name && t.signature == tool.signature)
            .count()
            == 1;
        self.runtime.pending.retain(|p| {
            !(p.tool.as_deref() == Some(&tool.id)
                || (p.tool.is_none()
                    && p.name == tool.name
                    && p.signature == tool.signature
                    && unambiguous))
        });
        self.runtime.tools.retain(|t| t.id != tool.id);
        self.refresh_attention();
    }

    fn refresh_attention(&mut self) {
        self.attention = self
            .runtime
            .pending
            .iter()
            .min_by_key(|p| {
                (
                    match p.attention.kind {
                        AttentionKind::Question => 0,
                        AttentionKind::Input => 1,
                        AttentionKind::Permission => 2,
                    },
                    p.attention.since,
                )
            })
            .map(|p| p.attention.clone())
            .or_else(|| self.runtime.native_attention.clone());
    }

    fn observe_notification(&mut self, payload: &Value, now: u64) -> Result<()> {
        let kind = required_id(payload, "notification_type")?;
        let attention = match kind.as_str() {
            "permission_prompt" if self.capabilities.approval => Some(AttentionKind::Permission),
            "elicitation_dialog" | "elicitation_url_dialog" if self.capabilities.questions => {
                Some(AttentionKind::Input)
            }
            "agent_needs_input" if self.capabilities.questions => Some(AttentionKind::Input),
            _ => None,
        };
        if let Some(kind) = attention {
            if let Some(pending) = self
                .runtime
                .pending
                .iter_mut()
                .find(|p| p.attention.kind == kind)
            {
                pending.attention.certainty = AttentionCertainty::Confirmed;
                self.refresh_attention();
            } else {
                self.add_interaction(
                    None,
                    "notification",
                    None,
                    kind,
                    AttentionCertainty::Confirmed,
                    now,
                )?;
            }
        }
        Ok(())
    }

    fn observe_background(&mut self, payload: &Value, now: u64) -> Result<()> {
        if !self.capabilities.background {
            return Ok(());
        }
        let (Some(tasks), Some(crons)) = (
            payload.get("background_tasks"),
            payload.get("session_crons"),
        ) else {
            self.background.coverage = BackgroundCoverage::Unknown;
            return Ok(());
        };
        let tasks = tasks
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid background tasks"))?;
        let crons = crons
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid session crons"))?;
        ensure!(
            tasks.len() + crons.len() <= MAX_JOBS,
            "Too many background jobs"
        );
        let mut ids = HashSet::new();
        let mut jobs = Vec::with_capacity(tasks.len() + crons.len());
        for task in tasks {
            let id = required_id(task, "id")?;
            ensure!(ids.insert(id.clone()), "Duplicate background job");
            let kind = required_text(task, "type", 160)?;
            ensure!(clean(&kind, 160) == kind, "Invalid background job type");
            let status = match required_text(task, "status", 160)?.as_str() {
                "running" => BackgroundStatus::Running,
                "completed" => BackgroundStatus::Completed,
                "failed" => BackgroundStatus::Failed,
                _ => BackgroundStatus::Unknown,
            };
            jobs.push(ActivityJob {
                id,
                name: display_field(task, "description", &kind)?,
                kind: clean(&kind, 80),
                status,
                observed_at: now,
                next_run_at: None,
            });
        }
        for cron in crons {
            let id = required_id(cron, "id")?;
            ensure!(ids.insert(id.clone()), "Duplicate background job");
            let schedule = required_text(cron, "schedule", 256)?;
            ensure!(
                cron.get("recurring").and_then(Value::as_bool).is_some(),
                "Invalid recurring flag"
            );
            // Do not retain cron prompts. A schedule identifies the job without
            // copying user tasks or secrets into the session catalog.
            let next = self
                .background
                .jobs
                .iter()
                .find(|j| j.id == id && j.kind == "cron")
                .and_then(|j| j.next_run_at)
                .filter(|next| *next > now);
            jobs.push(ActivityJob {
                id,
                name: clean(&schedule, 160),
                kind: "cron".into(),
                status: BackgroundStatus::Scheduled,
                observed_at: now,
                next_run_at: next,
            });
        }
        self.background = ActivityBackground {
            coverage: BackgroundCoverage::Snapshot,
            observed_at: Some(now),
            jobs,
        };
        Ok(())
    }

    fn observe_cron_result(&mut self, payload: &Value, tool: &str, now: u64) -> Result<()> {
        if !matches!(tool, "CronCreate" | "CronList") {
            return Ok(());
        }
        let Some(response) = payload.get("tool_response") else {
            return Ok(());
        };
        let rows: Vec<&Value> = if tool == "CronList" {
            let Some(jobs) = response.get("jobs").and_then(Value::as_array) else {
                return Ok(());
            };
            ensure!(jobs.len() <= MAX_JOBS, "Too many cron jobs");
            jobs.iter().collect()
        } else {
            vec![response]
        };
        for row in rows {
            let id = required_id(row, "id")?;
            let next_run_at = match row.get("nextFireAt") {
                None | Some(Value::Null) => None,
                Some(v) => {
                    let ms = v
                        .as_u64()
                        .ok_or_else(|| anyhow::anyhow!("Invalid nextFireAt"))?;
                    ensure!(ms > 0 && ms <= 9_007_199_254_740_991, "Invalid nextFireAt");
                    Some(ms / 1000)
                }
            };
            if let Some(job) = self
                .background
                .jobs
                .iter_mut()
                .find(|j| j.id == id && j.kind == "cron")
            {
                job.next_run_at = next_run_at;
                job.observed_at = now;
            } else {
                ensure!(
                    self.background.jobs.len() < MAX_JOBS,
                    "Too many background jobs"
                );
                self.background.jobs.push(ActivityJob {
                    id,
                    name: display_field(row, "humanSchedule", "Scheduled task")?,
                    kind: "cron".into(),
                    status: BackgroundStatus::Scheduled,
                    observed_at: now,
                    next_run_at,
                });
            }
        }
        Ok(())
    }

    fn invalidate_background(&mut self) {
        if self.capabilities.background {
            self.background.coverage = BackgroundCoverage::Unknown;
        }
        for job in &mut self.background.jobs {
            job.status = BackgroundStatus::Unknown;
            job.next_run_at = None;
        }
    }

    pub fn mark_read(&mut self, expected_revision: u64) -> Result<bool> {
        ensure!(
            expected_revision == self.result.revision,
            "A newer agent response is available"
        );
        if self.result.read_revision == expected_revision {
            return Ok(false);
        }
        self.result.read_revision = expected_revision;
        Ok(true)
    }

    /// Only call with a verified main-agent native frame. Unlike a Stop hook,
    /// these frames describe the harness's actual terminal status surface.
    pub fn apply_native_status(&mut self, state: &str, now: u64) -> bool {
        if !self.capabilities.foreground {
            return false;
        }
        let before = (
            self.phase,
            self.attention.clone(),
            self.result.revision,
            self.result.at,
            self.source.clone(),
            self.updated_at,
            self.tools.clone(),
            self.tools_observed_at,
        );
        match state {
            "working" => {
                if self.runtime.native_done {
                    self.runtime.generation = self.runtime.generation.saturating_add(1);
                    self.runtime.turn_result_recorded = false;
                    self.runtime.native_done = false;
                }
                self.phase = ActivityPhase::Working;
            }
            "idle" => {
                self.phase = ActivityPhase::Idle;
            }
            "done" => {
                // An ID-less frame cannot establish a new response or prove
                // which turn finished. Only authenticated Stop creates a result.
                self.runtime.native_done = self.runtime.turn_result_recorded;
                self.phase = if self.runtime.native_done {
                    ActivityPhase::Idle
                } else {
                    ActivityPhase::Unknown
                };
            }
            "blocked" => {
                if self.runtime.pending.len() == 1 {
                    self.runtime.pending[0].attention.certainty = AttentionCertainty::Confirmed;
                    self.refresh_attention();
                } else if self.runtime.pending.is_empty()
                    && (self.capabilities.questions || self.capabilities.approval)
                {
                    let attention = ActivityAttention {
                        kind: AttentionKind::Input,
                        certainty: AttentionCertainty::Confirmed,
                        since: now,
                    };
                    self.runtime.native_attention = Some(attention.clone());
                    self.attention = Some(attention);
                }
                self.phase = ActivityPhase::Idle;
            }
            "error" | "clear" => {
                self.phase = ActivityPhase::Unknown;
            }
            _ => return false,
        }
        // These frames carry no prompt/tool ID. A delayed frame from a prior
        // turn cannot retire the current turn's authenticated tool observations.
        self.updated_at = Some(now);
        self.source = format!("claude_native:{state}");
        before
            != (
                self.phase,
                self.attention.clone(),
                self.result.revision,
                self.result.at,
                self.source.clone(),
                self.updated_at,
                self.tools.clone(),
                self.tools_observed_at,
            )
    }

    pub(crate) fn has_claude_session(&self) -> bool {
        self.runtime.session.is_some()
    }

    fn record_claude_turn_result(&mut self, now: u64) -> bool {
        if self.runtime.turn_result_recorded {
            return false;
        }
        self.runtime.turn_result_recorded = true;
        let key = format!(
            "claude:{}:{}:{}",
            self.runtime.session.as_deref().unwrap_or("unbound"),
            self.runtime.generation,
            self.runtime.prompt.as_deref().unwrap_or("no-prompt")
        );
        self.record_result(Some(&key), now)
    }

    pub fn process_ended(&mut self, now: u64) {
        self.phase = ActivityPhase::Stopped;
        self.attention = None;
        self.runtime.pending.clear();
        self.runtime.tools.clear();
        self.clear_activity_tools(now);
        self.tools_observed_at = Some(now);
        self.runtime.native_attention = None;
        self.invalidate_background();
        self.updated_at = Some(now);
        self.source = "process_exit".into();
    }

    pub fn restarted(&mut self, now: u64) {
        self.runtime = Runtime::default();
        self.codex = Default::default();
        self.phase = ActivityPhase::Unknown;
        self.attention = None;
        self.tools.clear();
        self.tools_observed_at = None;
        self.invalidate_background();
        self.updated_at = Some(now);
        self.source = "daemon_restart".into();
    }

    pub(crate) fn set_phase(&mut self, phase: ActivityPhase, source: &str, now: u64) -> bool {
        self.phase = phase;
        self.source = clean(source, 80);
        self.updated_at = Some(now);
        true
    }

    pub(crate) fn set_attention(
        &mut self,
        kind: AttentionKind,
        certainty: AttentionCertainty,
        source: &str,
        now: u64,
    ) -> bool {
        let since = self
            .attention
            .as_ref()
            .filter(|a| a.kind == kind)
            .map_or(now, |a| a.since);
        self.attention = Some(ActivityAttention {
            kind,
            certainty,
            since,
        });
        self.source = clean(source, 80);
        self.updated_at = Some(now);
        true
    }

    pub(crate) fn clear_attention(&mut self, source: &str, now: u64) -> bool {
        self.attention = None;
        self.source = clean(source, 80);
        self.updated_at = Some(now);
        true
    }

    pub(crate) fn record_result(&mut self, key: Option<&str>, now: u64) -> bool {
        if let Some(key) = key {
            if self.runtime.results.iter().any(|old| old == key) {
                return false;
            }
            bounded_push(&mut self.runtime.results, key.into());
        }
        self.result.revision = self.result.revision.saturating_add(1);
        self.result.at = Some(now);
        true
    }
}

fn bounded_push(ids: &mut VecDeque<String>, id: String) {
    if ids.len() == MAX_IDS {
        ids.pop_front();
    }
    ids.push_back(id);
}

fn clean(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(limit)
        .collect()
}

fn required_text(value: &Value, name: &str, max: usize) -> Result<String> {
    let text = value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Missing or invalid {name}"))?;
    ensure!(
        !text.is_empty() && text.len() <= max,
        "Invalid {name} length"
    );
    Ok(text.into())
}

fn required_id(value: &Value, name: &str) -> Result<String> {
    let id = required_text(value, name, 160)?;
    ensure!(
        id.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.:".contains(c)),
        "Invalid {name}"
    );
    Ok(id)
}

fn optional_id(value: &Value, name: &str) -> Result<Option<String>> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        _ => required_id(value, name).map(Some),
    }
}

fn display_field(value: &Value, field: &str, fallback: &str) -> Result<String> {
    let text = match value.get(field) {
        None | Some(Value::Null) => fallback,
        Some(Value::String(text)) => {
            ensure!(text.len() <= 16 * 1024, "Display text too long");
            text
        }
        _ => bail!("Invalid display text"),
    };
    let text = clean(text, 160);
    Ok(if text.trim().is_empty() {
        clean(fallback, 160)
    } else {
        text
    })
}

fn read_tool(payload: &Value, require_id: bool) -> Result<Tool> {
    let name = required_id(payload, "tool_name")?;
    let id = if require_id {
        required_id(payload, "tool_use_id")?
    } else {
        String::new()
    };
    let signature = if let Some(input) = payload.get("tool_input") {
        let bytes = serde_json::to_vec(input)?;
        ensure!(bytes.len() <= 128 * 1024, "Tool input too large");
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        Some(hasher.finish())
    } else {
        None
    };
    Ok(Tool {
        id,
        name,
        signature,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> ActivityState {
        ActivityState::new(
            "claude",
            ActivityCapabilities {
                foreground: true,
                questions: true,
                approval: true,
                results: true,
                background: true,
            },
            1,
        )
    }
    fn event(name: &str, prompt: &str) -> Value {
        json!({"session_id":"session-a","prompt_id":prompt,"hook_event_name":name})
    }
    fn submit(state: &mut ActivityState, prompt: &str, now: u64) {
        assert!(
            state
                .apply_claude(&event("UserPromptSubmit", prompt), now)
                .unwrap()
        );
    }
    fn tool(name: &str, prompt: &str, id: &str, tool_name: &str) -> Value {
        let mut value = event(name, prompt);
        value["tool_use_id"] = json!(id);
        value["tool_name"] = json!(tool_name);
        value["tool_input"] = json!({"secret":"never serialize this"});
        value
    }

    #[test]
    fn legacy_serialization_has_no_runtime_or_sensitive_payload() {
        let legacy = ActivityState::default();
        assert_eq!(legacy.phase, ActivityPhase::Unknown);
        assert!(!legacy.capabilities.foreground);
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        s.apply_claude(
            &tool("PreToolUse", "prompt-a", "tool-a", "AskUserQuestion"),
            3,
        )
        .unwrap();
        let encoded = serde_json::to_string(&s).unwrap();
        assert!(!encoded.contains("never serialize"));
        assert!(encoded.contains("tool-a"));
        assert!(!encoded.contains("prompt-a"));
        assert!(!encoded.contains("runtime"));
        assert!(!encoded.contains("codex"));
        let restored: ActivityState = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored.result.revision, s.result.revision);
    }

    #[test]
    fn wrong_session_and_old_turn_cannot_replace_current_observation() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        submit(&mut s, "prompt-b", 3);
        let mut wrong = event("Stop", "prompt-b");
        wrong["session_id"] = json!("foreign");
        let before = serde_json::to_value(&s).unwrap();
        assert!(s.apply_claude(&wrong, 4).is_err());
        assert!(!s.apply_claude(&event("Stop", "prompt-a"), 5).unwrap());
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        let mut clear = event("SessionStart", "prompt-c");
        clear["session_id"] = json!("session-b");
        clear["source"] = json!("clear");
        assert!(s.apply_claude(&clear, 6).unwrap());
        assert_eq!(s.phase, ActivityPhase::Unknown);
        assert!(s.apply_claude(&event("Stop", "prompt-b"), 7).is_err());
    }

    #[test]
    fn child_tools_do_not_change_main_question_or_result() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        s.apply_claude(
            &tool("PreToolUse", "prompt-a", "question", "AskUserQuestion"),
            3,
        )
        .unwrap();
        let mut child = tool("PostToolUse", "prompt-a", "question", "AskUserQuestion");
        child["agent_id"] = json!("child-a");
        assert!(!s.apply_claude(&child, 4).unwrap());
        let mut stopped = event("SubagentStop", "prompt-a");
        stopped["agent_id"] = json!("child-a");
        stopped["background_tasks"] = json!([]);
        stopped["session_crons"] = json!([]);
        assert!(s.apply_claude(&stopped, 5).unwrap());
        assert_eq!(s.attention.as_ref().unwrap().kind, AttentionKind::Question);
        assert_eq!(s.result.revision, 0);
        assert_eq!(s.phase, ActivityPhase::Working);
        assert_eq!(s.background.coverage, BackgroundCoverage::Snapshot);
    }

    #[test]
    fn stop_and_native_done_deduplicate_but_preserve_pending_question() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        s.apply_claude(
            &tool("PreToolUse", "prompt-a", "question", "AskUserQuestion"),
            3,
        )
        .unwrap();
        s.apply_claude(&event("Stop", "prompt-a"), 4).unwrap();
        assert_eq!(s.phase, ActivityPhase::Unknown);
        assert_eq!(s.result.revision, 1);
        assert!(s.apply_native_status("blocked", 5));
        assert_eq!(
            s.attention.as_ref().unwrap().certainty,
            AttentionCertainty::Confirmed
        );
        assert!(s.apply_native_status("done", 6));
        s.apply_claude(&event("Stop", "prompt-a"), 7).unwrap();
        assert_eq!(s.phase, ActivityPhase::Idle);
        assert_eq!(s.result.revision, 1);
        assert!(s.attention.is_some());
        s.apply_claude(&tool("PostToolUse", "prompt-a", "other", "Bash"), 8)
            .unwrap();
        assert!(s.attention.is_some());
        s.apply_claude(
            &tool("PostToolUse", "prompt-a", "question", "AskUserQuestion"),
            9,
        )
        .unwrap();
        assert!(s.attention.is_none());
    }

    #[test]
    fn native_completion_first_does_not_duplicate_late_stop() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        s.apply_native_status("done", 3);
        assert_eq!(s.result.revision, 0);
        s.apply_claude(&event("Stop", "prompt-a"), 4).unwrap();
        assert_eq!(s.result.revision, 1);
        assert_eq!(s.phase, ActivityPhase::Unknown);
        submit(&mut s, "prompt-b", 5);
        s.apply_claude(&event("Stop", "prompt-b"), 6).unwrap();
        assert_eq!(s.result.revision, 2);
    }

    #[test]
    fn startup_native_done_does_not_invent_a_completed_response() {
        let mut s = state();
        assert!(s.apply_native_status("done", 2));
        assert_eq!(s.result.revision, 0);
        assert!(s.apply_native_status("working", 3));
        assert!(s.apply_native_status("done", 4));
        assert_eq!(s.result.revision, 0);
    }

    #[test]
    fn bare_done_cannot_complete_a_pending_question_or_confirm_parallel_requests() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        s.apply_claude(
            &tool("PreToolUse", "prompt-a", "question-a", "AskUserQuestion"),
            3,
        )
        .unwrap();
        s.apply_native_status("done", 4);
        assert_eq!(s.result.revision, 0);
        assert_eq!(s.attention.as_ref().unwrap().kind, AttentionKind::Question);
        s.apply_claude(
            &tool("PreToolUse", "prompt-a", "question-b", "AskUserQuestion"),
            5,
        )
        .unwrap();
        s.apply_native_status("blocked", 6);
        assert_eq!(
            s.attention.as_ref().unwrap().certainty,
            AttentionCertainty::Requested
        );
        assert!(
            s.runtime
                .pending
                .iter()
                .all(|p| p.attention.certainty == AttentionCertainty::Requested)
        );
        assert!(!s.apply_native_status("blocked", 6));
    }

    #[test]
    fn session_switch_is_not_a_process_exit() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        let mut end = event("SessionEnd", "prompt-a");
        end["reason"] = json!("resume");
        s.apply_claude(&end, 3).unwrap();
        assert_eq!(s.phase, ActivityPhase::Unknown);
        let mut start = event("SessionStart", "prompt-b");
        start["source"] = json!("resume");
        s.apply_claude(&start, 4).unwrap();
        submit(&mut s, "prompt-b", 5);
        assert_eq!(s.phase, ActivityPhase::Working);
    }

    #[test]
    fn read_cas_never_acknowledges_a_newer_response() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        s.apply_claude(&event("Stop", "prompt-a"), 3).unwrap();
        submit(&mut s, "prompt-b", 4);
        s.apply_claude(&event("Stop", "prompt-b"), 5).unwrap();
        assert!(s.mark_read(1).is_err());
        assert_eq!(s.result.read_revision, 0);
        assert!(s.mark_read(2).unwrap());
        assert!(!s.mark_read(2).unwrap());
    }

    #[test]
    fn missing_background_arrays_are_not_an_empty_or_healthy_snapshot() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        let mut stop = event("Stop", "prompt-a");
        stop["background_tasks"] = json!([{"id":"monitor-a","type":"MCP task","status":"future_status","description":"a\u{001b}[0m\nmonitor"}]);
        stop["session_crons"] = json!([]);
        s.apply_claude(&stop, 3).unwrap();
        assert_eq!(s.background.jobs[0].status, BackgroundStatus::Unknown);
        assert!(!s.background.jobs[0].name.contains('\u{001b}'));
        assert_eq!(s.background.observed_at, Some(3));
        s.apply_claude(&event("Stop", "prompt-a"), 4).unwrap();
        assert_eq!(s.background.coverage, BackgroundCoverage::Unknown);
        assert_eq!(s.background.jobs.len(), 1);
        assert_eq!(s.background.observed_at, Some(3));
        stop["background_tasks"] = json!([]);
        s.apply_claude(&stop, 5).unwrap();
        assert_eq!(s.background.coverage, BackgroundCoverage::Snapshot);
        assert!(s.background.jobs.is_empty());
    }

    #[test]
    fn next_run_is_only_taken_from_explicit_native_cron_result() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        let mut stop = event("Stop", "prompt-a");
        stop["background_tasks"] = json!([]);
        stop["session_crons"] = json!([{"id":"cron-a","schedule":"* * * * *","recurring":true,"prompt":"do not persist this prompt"}]);
        s.apply_claude(&stop, 3).unwrap();
        assert_eq!(s.background.jobs[0].next_run_at, None);
        let mut result = tool("PostToolUse", "prompt-a", "cron-create", "CronCreate");
        result["tool_response"] =
            json!({"id":"cron-a","humanSchedule":"every minute","nextFireAt":120000});
        s.apply_claude(&result, 4).unwrap();
        assert_eq!(s.background.jobs[0].next_run_at, Some(120));
        s.apply_claude(&stop, 5).unwrap();
        assert_eq!(s.background.jobs[0].next_run_at, Some(120));
        s.apply_claude(&stop, 121).unwrap();
        assert_eq!(s.background.jobs[0].next_run_at, None);
        assert!(
            !serde_json::to_string(&s)
                .unwrap()
                .contains("do not persist")
        );
    }

    #[test]
    fn bounds_failure_rolls_back_result_increment_and_snapshot() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        let before = serde_json::to_value(&s).unwrap();
        let mut stop = event("Stop", "prompt-a");
        stop["background_tasks"] = json!(
            (0..65)
                .map(|i| json!({"id":format!("job-{i}"),"type":"shell","status":"running"}))
                .collect::<Vec<_>>()
        );
        stop["session_crons"] = json!([]);
        assert!(s.apply_claude(&stop, 3).is_err());
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }

    #[test]
    fn termination_and_restart_preserve_unread_result_without_live_job_claims() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        s.apply_claude(&event("Stop", "prompt-a"), 3).unwrap();
        s.background.jobs.push(ActivityJob {
            id: "job".into(),
            name: "monitor".into(),
            kind: "monitor".into(),
            status: BackgroundStatus::Running,
            observed_at: 3,
            next_run_at: Some(100),
        });
        s.process_ended(4);
        assert_eq!(s.phase, ActivityPhase::Stopped);
        assert_eq!(s.background.jobs[0].status, BackgroundStatus::Unknown);
        s.restarted(5);
        assert_eq!(s.phase, ActivityPhase::Unknown);
        assert_eq!(s.result.revision, 1);
        assert_eq!(s.result.read_revision, 0);
        assert_eq!(s.background.jobs[0].observed_at, 3);
        assert_eq!(s.background.jobs[0].next_run_at, None);
    }

    #[test]
    fn tool_observations_are_metadata_only_and_keep_first_request_time() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        let start = tool("PreToolUse", "prompt-a", "call-a", "Bash");
        s.apply_claude(&start, 3).unwrap();
        s.apply_claude(&start, 4).unwrap();
        assert_eq!(s.tools.len(), 1);
        assert_eq!(s.tools[0].started_at, 3);
        assert_eq!(s.tools_observed_at, Some(4));
        let encoded = serde_json::to_value(&s).unwrap();
        assert_eq!(
            encoded["tools"][0],
            json!({"id":"call-a","name":"Bash","started_at":3})
        );
        assert!(!encoded.to_string().contains("never serialize"));
        let mut completed = tool("PostToolUse", "prompt-a", "call-a", "Bash");
        completed["tool_response"] = json!({"output":"private command output"});
        s.apply_claude(&completed, 5).unwrap();
        assert!(s.tools.is_empty());
        assert!(!s.apply_claude(&start, 6).unwrap());
        assert_eq!(s.tools_observed_at, Some(5));
    }

    #[test]
    fn stopping_observed_tools_does_not_resolve_a_question_or_replay_old_calls() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        let question = tool("PreToolUse", "prompt-a", "question", "AskUserQuestion");
        s.apply_claude(&question, 3).unwrap();
        s.apply_claude(&event("Stop", "prompt-a"), 4).unwrap();
        assert!(s.tools.is_empty());
        assert!(s.attention.is_some());
        assert!(!s.apply_claude(&question, 5).unwrap());
        submit(&mut s, "prompt-b", 6);
        assert!(!s.apply_claude(&question, 7).unwrap());
        let mut child_start = event("SessionStart", "prompt-b");
        child_start["agent_id"] = json!("child");
        child_start["source"] = json!("resume");
        let parent = tool("PreToolUse", "prompt-b", "parent", "Read");
        s.apply_claude(&parent, 8).unwrap();
        assert!(!s.apply_claude(&child_start, 9).unwrap());
        assert_eq!(s.tools[0].id, "parent");
    }

    #[test]
    fn tool_failure_authenticated_stop_exit_and_restart_clear_current_calls() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        s.apply_claude(&tool("PreToolUse", "prompt-a", "failure", "Read"), 3)
            .unwrap();
        s.apply_claude(
            &tool("PostToolUseFailure", "prompt-a", "failure", "Read"),
            4,
        )
        .unwrap();
        assert!(s.tools.is_empty());
        for i in 0..4 {
            let id = format!("call-{i}");
            s.apply_claude(
                &tool("PreToolUse", "prompt-a", &id, "Read"),
                5 + i as u64 * 2,
            )
            .unwrap();
            s.apply_claude(&event("Stop", "prompt-a"), 6 + i as u64 * 2)
                .unwrap();
            assert!(s.tools.is_empty());
        }
        s.apply_claude(&tool("PreToolUse", "prompt-a", "last", "Read"), 20)
            .unwrap();
        s.process_ended(21);
        assert!(s.tools.is_empty());
        assert_eq!(s.tools_observed_at, Some(21));
        s.restarted(22);
        assert!(s.tools.is_empty());
        assert_eq!(s.tools_observed_at, None);
    }

    #[test]
    fn uncorrelated_native_frames_cannot_retire_current_tool_observations() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        let start = tool("PreToolUse", "prompt-a", "current-call", "Read");
        s.apply_claude(&start, 3).unwrap();
        for (i, status) in ["idle", "done", "error", "clear"].iter().enumerate() {
            s.apply_native_status(status, 4 + i as u64 * 2);
            s.apply_claude(&start, 5 + i as u64 * 2).unwrap();
            assert_eq!(s.tools.len(), 1);
            assert_eq!(s.tools[0].started_at, 3);
            assert_eq!(s.result.revision, 0);
            assert!(s.attention.is_none());
        }
        s.apply_claude(&event("Stop", "prompt-a"), 20).unwrap();
        assert!(s.tools.is_empty());
        assert!(!s.apply_claude(&start, 21).unwrap());
    }

    #[test]
    fn tool_observation_bounds_roll_back_and_legacy_json_has_unknown_coverage() {
        let mut s = state();
        submit(&mut s, "prompt-a", 2);
        for i in 0..MAX_TOOL_OBSERVATIONS {
            s.apply_claude(
                &tool("PreToolUse", "prompt-a", &format!("call-{i}"), "Read"),
                3,
            )
            .unwrap();
        }
        let before = serde_json::to_value(&s).unwrap();
        assert!(
            s.apply_claude(&tool("PreToolUse", "prompt-a", "overflow", "Read"), 4)
                .is_err()
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        let mut legacy = serde_json::to_value(ActivityState::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("tools");
        legacy.as_object_mut().unwrap().remove("tools_observed_at");
        let restored: ActivityState = serde_json::from_value(legacy).unwrap();
        assert!(restored.tools.is_empty());
        assert_eq!(restored.tools_observed_at, None);
    }
}
