#![cfg(unix)]

//! In-memory HTTP integration fixtures: no CLI, listener, model, or user data.
use std::{path::PathBuf, sync::Arc};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::{
    activity::{ActivityCapabilities, ActivityPhase, ActivityState},
    activity_hooks::Channel,
    api::{self, App},
    model::{self, Project, Session},
    pty::LiveSession,
    store::Store,
};

const HOST: &str = "127.0.0.1:45321";
const API_TOKEN: &str = "fixture-full-api-token";
const EVENT_TOKEN: &str = "fixture-private-native-channel";

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    app: Arc<App>,
    live: Arc<LiveSession>,
    id: String,
    dir: PathBuf,
    _cleanup: Cleanup,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("mpac-activity-http-{}", model::id()));
        std::fs::create_dir_all(dir.join("project")).unwrap();
        let root = dir.join("project").canonicalize().unwrap();
        let store = Arc::new(Store::open(&dir.join("state.sqlite3")).unwrap());
        let project = Project {
            id: model::id(),
            name: "Native event fixture".into(),
            roots: vec![mpac_workspace::Root {
                id: model::id(),
                name: "fixture".into(),
                path: root.to_str().unwrap().into(),
            }],
        };
        store.project(&project).unwrap();
        let id = model::id();
        let mut session: Session = serde_json::from_value(json!({
            "id":id,"project_id":project.id,"root_id":project.roots[0].id,
            "agent":"claude","title":"No CLI is launched","status":"running",
            "started_at":1,"last_activity":1,"seq":0,"mode":"interactive_pty"
        }))
        .unwrap();
        session.activity = ActivityState::new(
            "claude",
            ActivityCapabilities {
                foreground: true,
                questions: true,
                approval: true,
                results: true,
                background: false,
            },
            1,
        );
        store.session(&session).unwrap();
        let live = LiveSession::test_fixture(session, &root);
        let app = Arc::new(App::new(store, API_TOKEN.into(), HOST.into()));
        app.live.lock().unwrap().insert(id.clone(), live.clone());
        app.activity_channels.lock().unwrap().insert(
            id.clone(),
            Channel {
                secret: EVENT_TOKEN.into(),
                provider: "claude".into(),
            },
        );
        Self {
            app,
            live,
            id,
            dir: dir.clone(),
            _cleanup: Cleanup(dir),
        }
    }

    fn event(&self, name: &str, prompt: &str) -> Value {
        json!({"hook_event_name":name,"session_id":"native-session-a","prompt_id":prompt})
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Value,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = api::router(self.app.clone(), &self.dir.join("no-ui"))
            .oneshot(
                request
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| json!({"text":String::from_utf8_lossy(&bytes)}))
        };
        (status, body)
    }

    async fn event_request(&self, body: Value) -> StatusCode {
        self.request(
            "POST",
            &format!("/agent-events/{}", self.id),
            body,
            &[("host", HOST), ("x-mpac-event-token", EVENT_TOKEN)],
        )
        .await
        .0
    }

    async fn read(&self, revision: u64, bearer: &str) -> (StatusCode, Value) {
        self.request(
            "POST",
            &format!("/api/sessions/{}/activity/read", self.id),
            json!({"expected_revision":revision}),
            &[("host", HOST), ("authorization", bearer)],
        )
        .await
    }

    async fn response(&self, prompt: &str) {
        assert_eq!(
            self.event_request(self.event("UserPromptSubmit", prompt))
                .await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            self.event_request(self.event("Stop", prompt)).await,
            StatusCode::NO_CONTENT
        );
    }
}

#[tokio::test]
async fn full_reply_is_readable_without_pty_and_excludes_subagents_and_stale_turns() {
    let f = Fixture::new();
    let path = format!("/api/sessions/{}/answer", f.id);
    let auth = [
        ("host", HOST),
        ("authorization", "Bearer fixture-full-api-token"),
    ];
    assert_eq!(
        f.request("GET", &path, Value::Null, &auth).await.1,
        json!({"answer":null})
    );
    assert_eq!(
        f.event_request(f.event("UserPromptSubmit", "prompt-a"))
            .await,
        StatusCode::NO_CONTENT
    );
    let expected = format!(
        "Начало ответа 👋\n\n```rust\n{}\n```\nКонец ответа",
        "    let value = 42;\n".repeat(4000)
    );
    let mut stop = f.event("Stop", "prompt-a");
    stop["last_assistant_message"] = json!(expected);
    assert_eq!(f.event_request(stop).await, StatusCode::NO_CONTENT);
    assert_eq!(
        f.event_request(f.event("UserPromptSubmit", "prompt-b"))
            .await,
        StatusCode::NO_CONTENT
    );
    let mut stale = f.event("Stop", "prompt-a");
    stale["last_assistant_message"] = json!("Old reply");
    assert_eq!(f.event_request(stale).await, StatusCode::NO_CONTENT);
    let mut child = f.event("SubagentStop", "prompt-b");
    child["agent_id"] = json!("child");
    child["last_assistant_message"] = json!("Child reply");
    assert_eq!(f.event_request(child).await, StatusCode::NO_CONTENT);
    f.app.live.lock().unwrap().clear();
    assert_eq!(
        f.request("GET", &path, Value::Null, &[("host", HOST)])
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, result) = f.request("GET", &path, Value::Null, &auth).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["answer"]["text"], expected);
    let reopened = Store::open(&f.dir.join("state.sqlite3")).unwrap();
    assert_eq!(
        serde_json::to_value(reopened.agent_answer(&f.id).unwrap()).unwrap(),
        result["answer"]
    );
}

#[tokio::test]
async fn native_channel_requires_own_secret_and_rejects_all_browser_origins() {
    let f = Fixture::new();
    let path = format!("/agent-events/{}", f.id);
    let body = f.event("UserPromptSubmit", "a");
    for headers in [
        vec![("host", HOST)],
        vec![
            ("host", HOST),
            ("x-mpac-event-token", "wrong-private-channel"),
        ],
        vec![
            ("host", HOST),
            ("authorization", "Bearer fixture-full-api-token"),
        ],
        vec![("host", HOST), ("x-mpac-event-token", API_TOKEN)],
    ] {
        assert_eq!(
            f.request("POST", &path, body.clone(), &headers).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    for origin in ["http://127.0.0.1:45321", "https://foreign.example", "null"] {
        assert_eq!(
            f.request(
                "POST",
                &path,
                body.clone(),
                &[
                    ("host", HOST),
                    ("x-mpac-event-token", EVENT_TOKEN),
                    ("origin", origin)
                ]
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        f.request(
            "POST",
            &path,
            body.clone(),
            &[
                ("host", "foreign.example"),
                ("x-mpac-event-token", EVENT_TOKEN)
            ]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.request(
            "POST",
            &path,
            body.clone(),
            &[("x-mpac-event-token", EVENT_TOKEN)]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.event_request(body).await, StatusCode::NO_CONTENT);
    assert!(matches!(
        f.live.meta().activity.phase,
        ActivityPhase::Working
    ));
}

#[tokio::test]
async fn native_question_survives_stop_and_child_cannot_replace_primary_state() {
    let f = Fixture::new();
    assert_eq!(
        f.event_request(f.event("UserPromptSubmit", "a")).await,
        StatusCode::NO_CONTENT
    );
    let mut question = f.event("PreToolUse", "a");
    question["tool_name"] = json!("AskUserQuestion");
    question["tool_use_id"] = json!("private-tool-id");
    question["tool_input"] = json!({"question":"private-question-content"});
    assert_eq!(
        f.event_request(question.clone()).await,
        StatusCode::NO_CONTENT
    );
    assert!(f.live.meta().activity.attention.is_some());
    assert_eq!(
        f.event_request(f.event("Stop", "a")).await,
        StatusCode::NO_CONTENT
    );
    assert!(f.live.meta().activity.attention.is_some());
    let before = serde_json::to_value(f.live.meta().activity).unwrap();
    let mut child = f.event("UserPromptSubmit", "child-turn");
    child["agent_id"] = json!("subagent-id");
    assert_eq!(f.event_request(child).await, StatusCode::NO_CONTENT);
    assert_eq!(
        serde_json::to_value(f.live.meta().activity).unwrap(),
        before
    );
    question["hook_event_name"] = json!("PostToolUse");
    assert_eq!(f.event_request(question).await, StatusCode::NO_CONTENT);
    assert!(f.live.meta().activity.attention.is_none());
}

#[tokio::test]
async fn wrong_native_session_and_malformed_event_are_atomic_and_do_not_leak_payloads() {
    let f = Fixture::new();
    assert_eq!(
        f.event_request(f.event("UserPromptSubmit", "private-prompt-id"))
            .await,
        StatusCode::NO_CONTENT
    );
    let before = serde_json::to_value(f.live.meta()).unwrap();
    let mut foreign = f.event("UserPromptSubmit", "another");
    foreign["session_id"] = json!("foreign-native-session");
    foreign["prompt"] = json!("private-prompt-content");
    assert_eq!(f.event_request(foreign).await, StatusCode::BAD_REQUEST);
    assert_eq!(serde_json::to_value(f.live.meta()).unwrap(), before);
    let mut malformed = f.event("PreToolUse", "private-prompt-id");
    malformed["tool_name"] = json!("AskUserQuestion");
    assert_eq!(f.event_request(malformed).await, StatusCode::BAD_REQUEST);
    assert_eq!(serde_json::to_value(f.live.meta()).unwrap(), before);
    let (status, public) = f
        .request(
            "GET",
            "/api/state",
            Value::Null,
            &[
                ("host", HOST),
                ("authorization", "Bearer fixture-full-api-token"),
            ],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let encoded = public.to_string();
    for secret in [
        API_TOKEN,
        EVENT_TOKEN,
        "private-prompt-id",
        "private-prompt-content",
        "foreign-native-session",
    ] {
        assert!(!encoded.contains(secret), "public state exposed {secret}");
    }
    let stored = serde_json::to_string(&f.app.store.session_entry(&f.id).unwrap().0).unwrap();
    assert!(!stored.contains(EVENT_TOKEN));
    assert!(!stored.contains("private-prompt-id"));
}

#[tokio::test]
async fn explicit_read_uses_full_api_auth_and_revision_cas_without_hiding_newer_response() {
    let f = Fixture::new();
    f.response("a").await;
    f.response("b").await;
    assert_eq!(f.live.meta().activity.result.revision, 2);
    assert_eq!(
        f.read(2, "Bearer fixture-private-native-channel").await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.read(1, "Bearer fixture-full-api-token").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(f.live.meta().activity.result.read_revision, 0);
    assert_eq!(
        f.app
            .store
            .session_entry(&f.id)
            .unwrap()
            .0
            .activity
            .result
            .read_revision,
        0
    );
    let (status, marked) = f.read(2, "Bearer fixture-full-api-token").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(marked["activity"]["result"]["read_revision"], 2);
    assert_eq!(
        f.read(2, "Bearer fixture-full-api-token").await.0,
        StatusCode::OK
    );
    assert_eq!(
        f.read(3, "Bearer fixture-full-api-token").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.app
            .store
            .session_entry(&f.id)
            .unwrap()
            .0
            .activity
            .result
            .read_revision,
        2
    );
}

#[tokio::test]
async fn history_read_persists_but_restart_discards_live_native_certainty() {
    let f = Fixture::new();
    f.response("a").await;
    f.app.live.lock().unwrap().remove(&f.id);
    assert_eq!(
        f.read(1, "Bearer fixture-full-api-token").await.0,
        StatusCode::OK
    );
    let reopened = Store::open(&f.dir.join("state.sqlite3")).unwrap();
    reopened.reconcile_restart().unwrap();
    let stored = reopened.session_entry(&f.id).unwrap().0;
    assert_eq!(stored.status, "interrupted");
    assert!(matches!(stored.activity.phase, ActivityPhase::Unknown));
    assert!(stored.activity.attention.is_none());
    assert_eq!(stored.activity.source, "daemon_restart");
    assert_eq!(stored.activity.result.revision, 1);
    assert_eq!(stored.activity.result.read_revision, 1);
    assert_eq!(
        f.event_request(f.event("UserPromptSubmit", "after-restart"))
            .await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.read(1, "Bearer fixture-full-api-token").await.0,
        StatusCode::OK
    );
}
