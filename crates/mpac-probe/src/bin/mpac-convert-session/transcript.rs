use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs::File,
    io::Read,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use uuid::Uuid;

const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Harness {
    Claude,
    Codex,
}

impl Harness {
    pub fn parse(value: &OsStr) -> Result<Self> {
        match value.to_str() {
            Some("claude") => Ok(Self::Claude),
            Some("codex") => Ok(Self::Codex),
            _ => bail!("expected claude or codex"),
        }
    }

    pub fn program(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn version(self) -> &'static str {
        match self {
            Self::Claude => "2.1.325",
            Self::Codex => "0.167.0",
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Role {
    User,
    Assistant,
}

impl Role {
    fn parse(value: &Value) -> Option<Self> {
        match value.as_str()? {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

struct Message {
    role: Role,
    text: String,
    timestamp: String,
    end_turn: bool,
}

#[derive(Default)]
pub struct Transcript {
    messages: Vec<Message>,
    pub omitted: usize,
    pub incomplete: usize,
}

impl Transcript {
    pub fn read(path: &Path, kind: Harness) -> Result<Self> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take((MAX_INPUT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_INPUT_BYTES,
            "transcript exceeds prototype size limit"
        );
        // An active CLI can append a JSON record while the file is being read.
        let end = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .context("missing complete JSONL record")?;
        let rows = bytes[..end]
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
            .enumerate()
            .map(|(index, line)| {
                serde_json::from_slice(line)
                    .map_err(|_| anyhow::anyhow!("invalid JSON in completed record {}", index + 1))
            })
            .collect::<Result<Vec<Value>>>()?;
        let mut transcript = match kind {
            Harness::Claude => Self::from_claude(&rows)?,
            Harness::Codex => Self::from_codex(&rows)?,
        };
        transcript.incomplete +=
            usize::from(bytes[end + 1..].iter().any(|b| !b.is_ascii_whitespace()));
        ensure!(
            !transcript.messages.is_empty(),
            "no complete text turns found"
        );
        ensure!(
            transcript
                .messages
                .iter()
                .map(|m| m.text.len())
                .sum::<usize>()
                <= MAX_TEXT_BYTES,
            "normalized text exceeds prototype size limit"
        );
        Ok(transcript)
    }

    fn from_claude(rows: &[Value]) -> Result<Self> {
        let mut entries = HashMap::new();
        let mut leaf = None;
        let mut session_id = None;
        for row in rows {
            if row["isSidechain"] == true {
                continue;
            }
            if let Some(id) = row["uuid"].as_str() {
                ensure!(
                    entries.insert(id, row).is_none(),
                    "duplicate Claude message ID"
                );
                if Role::parse(&row["type"]).is_some() {
                    let current = row["sessionId"]
                        .as_str()
                        .context("missing Claude session ID")?;
                    ensure!(
                        session_id.get_or_insert(current) == &current,
                        "multiple Claude sessions in source"
                    );
                    leaf = Some(id);
                }
            }
        }
        let mut chain = Vec::new();
        let mut visited = HashSet::new();
        while let Some(id) = leaf {
            ensure!(visited.insert(id), "cyclic Claude transcript");
            let row = entries
                .get(id)
                .context("Claude transcript has an unresolved parent")?;
            ensure!(
                row["subtype"] != "compact_boundary",
                "compacted Claude transcripts are unsupported in this prototype"
            );
            chain.push(*row);
            leaf = row["parentUuid"].as_str();
        }
        let mut result = Self::default();
        for row in chain.into_iter().rev() {
            let Some(role) = Role::parse(&row["type"]) else {
                continue;
            };
            let message = &row["message"];
            ensure!(
                Role::parse(&message["role"]) == Some(role),
                "Claude message role mismatch"
            );
            let (text, omitted) = plain_text(&message["content"], "text");
            result.omitted += omitted;
            if !text.is_empty() {
                result.push(role, text, &row["timestamp"])?;
            }
            if role == Role::Assistant && message["stop_reason"] == "end_turn" {
                result.complete_turn();
            }
        }
        result.trim_incomplete();
        Ok(result)
    }

    fn from_codex(rows: &[Value]) -> Result<Self> {
        let header = rows.first().context("missing Codex session header")?;
        ensure!(
            header["type"] == "session_meta",
            "invalid Codex session header"
        );
        let mut result = Self::default();
        for row in &rows[1..] {
            let payload = &row["payload"];
            match row["type"].as_str() {
                Some("compacted") => {
                    bail!("compacted Codex transcripts are unsupported in this prototype")
                }
                Some("response_item") if payload["type"] == "message" => {
                    if let Some(role) = Role::parse(&payload["role"]) {
                        let content_type = if role == Role::User {
                            "input_text"
                        } else {
                            "output_text"
                        };
                        let (text, omitted) = plain_text(&payload["content"], content_type);
                        result.omitted += omitted;
                        if !text.is_empty() {
                            result.push(role, text, &row["timestamp"])?;
                        }
                    }
                }
                Some("response_item") => result.omitted += 1,
                Some("event_msg") => match payload["type"].as_str() {
                    Some("task_complete" | "turn_complete") => result.complete_turn(),
                    Some("task_started" | "turn_started" | "turn_aborted") => {
                        result.trim_incomplete()
                    }
                    Some("thread_rolled_back" | "context_compacted") => {
                        bail!(
                            "rewound or compacted Codex transcripts are unsupported in this prototype"
                        );
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        result.trim_incomplete();
        Ok(result)
    }

    fn push(&mut self, role: Role, text: String, timestamp: &Value) -> Result<()> {
        if role == Role::Assistant && self.messages.last().is_none_or(|m| m.end_turn) {
            self.omitted += 1;
            return Ok(());
        }
        let timestamp = timestamp
            .as_str()
            .filter(|value| valid_timestamp(value))
            .context("invalid message timestamp")?;
        self.messages.push(Message {
            role,
            text,
            timestamp: timestamp.to_owned(),
            end_turn: false,
        });
        Ok(())
    }

    fn complete_turn(&mut self) {
        if let Some(last) = self.messages.last_mut()
            && last.role == Role::Assistant
        {
            last.end_turn = true;
        }
    }

    fn trim_incomplete(&mut self) {
        let count = self
            .messages
            .iter()
            .rposition(|message| message.end_turn)
            .map_or(0, |index| index + 1);
        self.incomplete += self.messages.len() - count;
        self.messages.truncate(count);
    }

    pub fn first_timestamp(&self) -> &str {
        &self.messages[0].timestamp
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    pub fn claude_rows(&self, id: &str, cwd: &str, title: &str) -> Vec<Value> {
        let mut parent = None;
        let mut rows = Vec::new();
        for message in &self.messages {
            let uuid = Uuid::new_v4().to_string();
            let payload = match message.role {
                Role::User => json!({"role":"user","content":message.text}),
                Role::Assistant => json!({
                    "id":format!("msg_{}",Uuid::new_v4().simple()),
                    "type":"message","role":"assistant","model":"mpac-imported",
                    "content":[{"type":"text","text":message.text}],
                    "stop_reason":message.end_turn.then_some("end_turn"),"stop_sequence":null,
                    "usage":{"input_tokens":0,"output_tokens":0}
                }),
            };
            rows.push(json!({
                "type":message.role.as_str(),"message":payload,"uuid":uuid,"parentUuid":parent,
                "sessionId":id,"isSidechain":false,"cwd":cwd,"timestamp":message.timestamp,
                "version":Harness::Claude.version(),"userType":"external"
            }));
            parent = Some(uuid);
        }
        rows.push(json!({"type":"custom-title","customTitle":title,"sessionId":id}));
        rows
    }

    pub fn codex_rows(&self, id: &str, cwd: &Path) -> Vec<Value> {
        let mut rows = vec![codex_row(
            self.first_timestamp(),
            "session_meta",
            json!({
                "id":id,"timestamp":self.first_timestamp(),"cwd":cwd,"cli_version":Harness::Codex.version(),
                "originator":"codex_cli_rs","source":"cli","model_provider":"openai"
            }),
        )];
        for message in &self.messages {
            let (event, kind) = match message.role {
                Role::User => (
                    json!({
                        "type":"user_message","message":message.text,
                        "images":[],"local_images":[],"text_elements":[]
                    }),
                    "input_text",
                ),
                Role::Assistant => (
                    json!({"type":"agent_message","message":message.text}),
                    "output_text",
                ),
            };
            rows.push(codex_row(&message.timestamp, "event_msg", event));
            rows.push(codex_row(&message.timestamp, "response_item", json!({
                "type":"message","role":message.role.as_str(),"content":[{"type":kind,"text":message.text}]
            })));
            if message.end_turn {
                rows.push(codex_row(&message.timestamp, "event_msg", json!({
                    "type":"task_complete","turn_id":Uuid::new_v4().to_string(),"last_agent_message":message.text
                })));
            }
        }
        rows
    }
}

fn valid_timestamp(value: &str) -> bool {
    value.is_ascii()
        && value.len() >= 20
        && value.len() <= 32
        && value.ends_with('Z')
        && value
            .bytes()
            .take(19)
            .enumerate()
            .all(|(index, b)| match index {
                4 | 7 => b == b'-',
                10 => b == b'T',
                13 | 16 => b == b':',
                _ => b.is_ascii_digit(),
            })
        && (value.len() == 20
            || (value.as_bytes()[19] == b'.'
                && value.as_bytes()[20..value.len() - 1]
                    .iter()
                    .all(u8::is_ascii_digit)))
}

fn plain_text(content: &Value, kind: &str) -> (String, usize) {
    if let Some(text) = content.as_str() {
        return (text.to_owned(), 0);
    }
    let Some(blocks) = content.as_array() else {
        return (String::new(), 1);
    };
    let text: Vec<_> = blocks
        .iter()
        .filter_map(|block| {
            (block["type"] == kind)
                .then(|| block["text"].as_str())
                .flatten()
        })
        .collect();
    (text.join("\n"), blocks.len() - text.len())
}

fn codex_row(timestamp: &str, kind: &str, payload: Value) -> Value {
    json!({"timestamp":timestamp,"type":kind,"payload":payload})
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAMP: &str = "2026-11-15T07:17:00.000Z";

    #[test]
    fn round_trip_keeps_roles_and_complete_turns_and_omits_tools() -> Result<()> {
        let mut source = Transcript::default();
        source.push(Role::User, "First question".into(), &json!(STAMP))?;
        source.push(Role::Assistant, "First answer".into(), &json!(STAMP))?;
        source.complete_turn();
        source.push(Role::User, "Second question".into(), &json!(STAMP))?;
        source.push(Role::Assistant, "Second answer".into(), &json!(STAMP))?;
        source.complete_turn();
        let mut rows = source.codex_rows(&Uuid::new_v4().to_string(), Path::new("/project"));
        rows.push(codex_row(
            STAMP,
            "response_item",
            json!({"type":"function_call","arguments":"ignored"}),
        ));
        rows.push(codex_row(STAMP, "response_item", json!({"type":"message","role":"user","content":[{"type":"input_text","text":"Incomplete"}]})));
        let read = Transcript::from_codex(&rows)?;
        assert_eq!((read.omitted, read.incomplete), (1, 1));
        let read = Transcript::from_claude(&read.claude_rows(
            &Uuid::new_v4().to_string(),
            "/project",
            "Converted",
        ))?;
        let pairs = |t: &Transcript| {
            t.messages
                .iter()
                .map(|m| (m.role.as_str(), m.text.clone(), m.end_turn))
                .collect::<Vec<_>>()
        };
        assert_eq!(pairs(&source), pairs(&read));
        Ok(())
    }

    #[test]
    fn rejects_missing_claude_ancestry_and_codex_compaction() {
        assert!(
            Transcript::from_claude(&[
                json!({"type":"user","uuid":"leaf","parentUuid":"unknown","sessionId":"session"}),
            ])
            .is_err()
        );
        assert!(
            Transcript::from_codex(&[json!({"type":"session_meta"}), json!({"type":"compacted"}),])
                .is_err()
        );
    }
}
