use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mpac_daemon::{
    api::{self, App},
    model::Session,
    store::Store,
};
use serde_json::{Value, json};
use tower::ServiceExt;

const HOST: &str = "127.0.0.1:4317";
const TOKEN: &str = "local-review-test-token";

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    // Field drop order closes SQLite before removing its directory on Windows.
    store: Arc<Store>,
    _cleanup: Cleanup,
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("mpac-review-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        Self {
            store: Arc::new(Store::open(&directory.join("state.sqlite3")).unwrap()),
            _cleanup: Cleanup(directory),
        }
    }

    fn router(&self) -> Router {
        let app = Arc::new(App::new(self.store.clone(), TOKEN.into(), HOST.into()));
        api::router(app, Path::new("ui-directory-not-needed-by-api-tests"))
    }
}

async fn request(
    router: &Router,
    host: Option<&str>,
    token: Option<&str>,
    origin: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().uri("/api/state");
    if let Some(host) = host {
        request = request.header("host", host);
    }
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn private_api_requires_bearer_token_even_from_expected_local_host() {
    let fixture = Fixture::new();
    let router = fixture.router();
    for token in [
        None,
        Some(""),
        Some("incorrect-token"),
        Some("local-review-test-token-extra"),
    ] {
        let (status, body) = request(&router, Some(HOST), token, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.get("projects").is_none());
        assert!(body.get("sessions").is_none());
    }
    assert_eq!(
        request(&router, Some(HOST), Some(TOKEN), None).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn directory_chooser_exposes_names_only_to_authenticated_local_clients() {
    let fixture = Fixture::new();
    let router = fixture.router();
    let directory = fixture._cleanup.0.join("selected folder");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(
        fixture._cleanup.0.join("private.txt"),
        "secret-content-not-for-chooser",
    )
    .unwrap();
    let uri = format!(
        "/api/directories?path={}",
        fixture._cleanup.0.to_str().unwrap()
    );
    for (token, origin, expected) in [
        (None, None, StatusCode::UNAUTHORIZED),
        (
            Some(TOKEN),
            Some("https://attacker.example"),
            StatusCode::FORBIDDEN,
        ),
        (Some(TOKEN), Some("http://127.0.0.1:4317"), StatusCode::OK),
    ] {
        let mut request = Request::builder().uri(&uri).header("host", HOST);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("secret-content-not-for-chooser"));
        if expected == StatusCode::OK {
            assert_eq!(value["entries"].as_array().unwrap().len(), 1);
            assert_eq!(value["entries"][0]["name"], "selected folder");
        } else {
            assert!(value.get("entries").is_none());
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn selected_directory_with_trailing_space_keeps_its_physical_identity() {
    let fixture = Fixture::new();
    let selected = fixture._cleanup.0.join("project ");
    std::fs::create_dir(&selected).unwrap();
    std::fs::create_dir(fixture._cleanup.0.join("project")).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("host", HOST)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name":"Exact folder","paths":[selected.to_str().unwrap()]}).to_string(),
        ))
        .unwrap();
    let response = fixture.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    let project: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        project["roots"][0]["path"],
        selected.canonicalize().unwrap().to_str().unwrap()
    );
    assert!(project["roots"][0]["path"].as_str().unwrap().ends_with(' '));
}

#[tokio::test]
async fn host_allowlist_rejects_missing_alternate_and_rebound_hosts() {
    let fixture = Fixture::new();
    let router = fixture.router();
    for host in [
        None,
        Some("localhost:4317"),
        Some("127.0.0.1:9999"),
        Some("attacker.example:4317"),
    ] {
        let (status, _) = request(&router, host, Some(TOKEN), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn origin_allowlist_rejects_foreign_and_opaque_origins_with_valid_token() {
    let fixture = Fixture::new();
    let router = fixture.router();
    for origin in [
        "https://attacker.example",
        "null",
        "http://localhost:4317",
        "http://127.0.0.1:9999",
    ] {
        let (status, _) = request(&router, Some(HOST), Some(TOKEN), Some(origin)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let (status, body) = request(
        &router,
        Some(HOST),
        Some(TOKEN),
        Some("http://127.0.0.1:4317"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["capabilities"]["semantic_agent_status"], true);
    assert_eq!(body["capabilities"]["session_activity"], true);
    assert_eq!(body["capabilities"]["mobile_remote"], false);
}

#[tokio::test]
async fn foreign_origin_cannot_mutate_projects_even_with_valid_token() {
    let fixture = Fixture::new();
    let router = fixture.router();
    let request = Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("host", HOST)
        .header("origin", "https://attacker.example")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"name":"untrusted","paths":[std::env::temp_dir().to_string_lossy()]})
                .to_string(),
        ))
        .unwrap();
    assert_eq!(
        router.oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert!(fixture.store.projects().unwrap().is_empty());
}

fn session(id: &str, status: &str) -> Session {
    Session {
        id: id.into(),
        project_id: "project".into(),
        root_id: "root".into(),
        agent: "demo".into(),
        title: "review fixture".into(),
        status: status.into(),
        started_at: 1,
        last_activity: 1,
        exit_code: None,
        error: None,
        seq: 0,
        mode: "interactive_pty".into(),
        access: Default::default(),
        additional_roots: vec![],
        permission_args: vec![],
        cli_title: None,
        title_source: mpac_daemon::model::SessionTitleSource::Manual,
        auto_title_eligible: false,
        activity: Default::default(),
        environment: None,
        terminal_launch: None,
    }
}

#[tokio::test]
async fn launch_rejects_invalid_grants_before_spawning_or_persisting() {
    let fixture = Fixture::new();
    let main = mpac_workspace::Root {
        id: "main".into(),
        name: "main".into(),
        path: fixture
            ._cleanup
            .0
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .into(),
    };
    fixture
        .store
        .project(&mpac_daemon::model::Project {
            id: "project".into(),
            name: "test".into(),
            roots: vec![main.clone()],
        })
        .unwrap();
    fixture
        .store
        .project(&mpac_daemon::model::Project {
            id: "foreign".into(),
            name: "foreign".into(),
            roots: vec![mpac_workspace::Root {
                id: "foreign-root".into(),
                ..main
            }],
        })
        .unwrap();
    let router = fixture.router();
    for access in [
        json!({"mode":"full_access"}),
        json!({"mode":"inherit","network_access":true}),
        json!({"mode":"inherit","additional_root_ids":["main"]}),
        json!({"mode":"inherit","additional_root_ids":["foreign-root"]}),
        json!({"mode":"inherit","additional_root_ids":["missing"]}),
        json!({"mode":"inherit","unknown_flag":true}),
        json!({"mode":"arbitrary"}),
    ] {
        let req = Request::builder()
            .method("POST")
            .uri("/api/sessions")
            .header("host", HOST)
            .header("authorization", format!("Bearer {TOKEN}"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"project_id":"project","root_id":"main","agent":"demo","access":access})
                    .to_string(),
            ))
            .unwrap();
        assert!(
            router
                .clone()
                .oneshot(req)
                .await
                .unwrap()
                .status()
                .is_client_error()
        );
        assert!(fixture.store.sessions().unwrap().is_empty());
    }
}

#[test]
fn old_session_records_default_to_inherited_permissions_without_replay() {
    let fixture = Fixture::new();
    let mut old = serde_json::to_value(session("legacy", "running")).unwrap();
    for key in ["access", "additional_roots", "permission_args"] {
        old.as_object_mut().unwrap().remove(key);
    }
    let connection = rusqlite::Connection::open(fixture._cleanup.0.join("state.sqlite3")).unwrap();
    connection
        .execute(
            "INSERT INTO sessions(id,payload) VALUES('legacy',?1)",
            [old.to_string()],
        )
        .unwrap();
    fixture.store.reconcile_restart().unwrap();
    let restored = &fixture.store.sessions().unwrap()[0];
    assert_eq!(restored.access, Default::default());
    assert!(restored.additional_roots.is_empty());
    assert!(restored.permission_args.is_empty());
    assert_eq!(restored.status, "interrupted");
}

#[test]
fn restart_reconciliation_is_not_limited_by_recent_history_pagination() {
    let fixture = Fixture::new();
    fixture
        .store
        .session(&session("old-active", "running"))
        .unwrap();
    fixture
        .store
        .session(&session("old-question", "needs_attention"))
        .unwrap();
    for n in 0..205 {
        fixture
            .store
            .session(&session(&format!("history-{n}"), "exited"))
            .unwrap();
    }
    fixture.store.reconcile_restart().unwrap();
    // Updating these IDs moves no rows. Check the full store directly rather
    // than accidentally asserting only the newest history page again.
    let connection = rusqlite::Connection::open(fixture._cleanup.0.join("state.sqlite3")).unwrap();
    for id in ["old-active", "old-question"] {
        let payload: String = connection
            .query_row("SELECT payload FROM sessions WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .unwrap();
        let restored: Session = serde_json::from_str(&payload).unwrap();
        assert_eq!(restored.status, "interrupted");
        assert!(!restored.active());
        assert!(restored.error.is_some());
    }
    assert!(
        fixture
            .store
            .sessions()
            .unwrap()
            .iter()
            .filter(|s| s.id.starts_with("history-"))
            .all(|s| s.status == "exited")
    );
}
