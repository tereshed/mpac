use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mpac_daemon::{
    access::{AccessMode, AccessSettings},
    api::{self, App},
    launch_defaults::{LaunchDefaults, SaveDefaults},
    model::Project,
    store::Store,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;

struct Fixture {
    path: PathBuf,
    store: Arc<Store>,
    project: Project,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mpac-defaults-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let store = Arc::new(Store::open(&path.join("db.sqlite3")).unwrap());
        let project = Project {
            id: "project".into(),
            name: "presets".into(),
            roots: vec![
                mpac_workspace::Root {
                    id: "main".into(),
                    name: "main".into(),
                    path: path.to_str().unwrap().into(),
                },
                mpac_workspace::Root {
                    id: "extra".into(),
                    name: "extra".into(),
                    path: path.join("extra").to_str().unwrap().into(),
                },
            ],
        };
        store.project(&project).unwrap();
        Self {
            path,
            store,
            project,
        }
    }
    fn save(&self, value: Value) -> anyhow::Result<LaunchDefaults> {
        self.store
            .save_launch_defaults(&serde_json::from_value::<SaveDefaults>(value)?)
    }
    async fn post(&self, body: Value, authorized: bool) -> (StatusCode, Value) {
        let router = api::router(
            Arc::new(App::new(
                self.store.clone(),
                "token".into(),
                "127.0.0.1:4317".into(),
            )),
            std::path::Path::new("no-ui"),
        );
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/launch-defaults")
            .header("host", "127.0.0.1:4317")
            .header("origin", "http://127.0.0.1:4317")
            .header("content-type", "application/json");
        if authorized {
            request = request.header("authorization", "Bearer token");
        }
        let response = router
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
fn global(revision: u64, mode: &str) -> Value {
    json!({"scope":"agent","agent_id":"codex","access":{"mode":mode},"expected_revision":revision,"make_default_agent":true})
}
#[test]
fn precedence_reset_and_durable_cas() {
    let f = Fixture::new();
    assert_eq!(f.store.launch_defaults().unwrap().revision, 0);
    let saved = f.save(global(0, "read_only")).unwrap();
    assert_eq!(saved.preferred_agent.as_deref(), Some("codex"));
    assert!(f.save(global(0, "inherit")).is_err());
    f.save(json!({"scope":"project","agent_id":"codex","project_id":"project","root_id":"main","access":{"mode":"workspace_write","additional_root_ids":["extra"]},"expected_revision":1})).unwrap();
    let catalog = f.store.launch_defaults().unwrap();
    assert_eq!(catalog.projects["project"].agent_id, None);
    assert_eq!(
        catalog.resolve("project", "codex", "main").mode,
        AccessMode::WorkspaceWrite
    );
    assert!(
        catalog
            .resolve("project", "codex", "extra")
            .additional_root_ids
            .is_empty()
    );
    f.save(json!({"scope":"project","agent_id":"codex","project_id":"project","access":null,"expected_revision":2})).unwrap();
    let restored = Store::open(&f.path.join("db.sqlite3"))
        .unwrap()
        .launch_defaults()
        .unwrap();
    assert_eq!(
        restored.resolve("project", "codex", "main").mode,
        AccessMode::ReadOnly
    );
    assert_eq!(
        restored.projects["project"].root_id.as_deref(),
        Some("main")
    );
    assert_eq!(
        restored.resolve("other", "hermes", "whatever"),
        AccessSettings::default()
    );
}
#[test]
fn validate_modes_scopes_and_never_retain_approval() {
    let f = Fixture::new();
    for request in [
        json!({"scope":"agent","agent_id":"codex","access":{"mode":"workspace_write","additional_root_ids":["extra"]},"expected_revision":0}),
        json!({"scope":"agent","agent_id":"claude","access":{"mode":"read_only"},"expected_revision":0}),
        json!({"scope":"agent","agent_id":"zai","access":{},"expected_revision":0}),
        json!({"scope":"project","agent_id":"codex","project_id":"project","root_id":"main","access":{"mode":"workspace_write","additional_root_ids":["foreign"]},"expected_revision":0}),
        json!({"scope":"project","agent_id":"codex","project_id":"project","root_id":"main","access":{"mode":"workspace_write","additional_root_ids":["main"]},"expected_revision":0}),
    ] {
        assert!(f.save(request).is_err());
    }
    let mut request = global(0, "full_access");
    request["access"]["acknowledge_full_access"] = json!(true);
    let catalog = f.save(request).unwrap();
    assert!(!catalog.agents["codex"].acknowledge_full_access);
    let access = catalog.resolve("project", "codex", "main");
    assert!(
        mpac_daemon::access::launch_args_for_session(
            "codex",
            &access,
            &[],
            &uuid::Uuid::new_v4().to_string()
        )
        .is_err()
    );
    assert!(!f.store.launch_defaults().unwrap().agents["codex"].acknowledge_full_access);
}
#[test]
fn project_edit_prunes_roots_and_preferences_atomically() {
    let f = Fixture::new();
    f.save(json!({"scope":"project","agent_id":"codex","project_id":"project","root_id":"main","make_default_agent":true,"access":{"mode":"workspace_write","additional_root_ids":["extra"]},"expected_revision":0})).unwrap();
    let mut updated = f.project.clone();
    updated.roots.remove(0);
    f.store.update_project(&f.project, &updated).unwrap();
    let catalog = f.store.launch_defaults().unwrap();
    assert_eq!(catalog.revision, 2);
    assert_eq!(catalog.projects["project"].root_id, None);
    assert_eq!(
        catalog.projects["project"].agent_id.as_deref(),
        Some("codex")
    );
    assert!(f.store.update_project(&f.project, &f.project).is_err());
    assert_eq!(f.store.launch_defaults().unwrap().revision, 2);
    let mut without = updated.clone();
    without.roots.clear();
    f.store.update_project(&updated, &without).unwrap();
    assert!(
        f.store.launch_defaults().unwrap().projects["project"].agents["codex"]
            .additional_root_ids
            .is_empty()
    );
}
#[test]
fn concurrent_updates_have_one_winner() {
    let f = Fixture::new();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let store = f.store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store
                    .save_launch_defaults(&serde_json::from_value(global(0, "read_only")).unwrap())
                    .is_ok()
            })
        })
        .collect();
    assert_eq!(
        handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter(|v| *v)
            .count(),
        1
    );
}
#[tokio::test]
async fn endpoint_auth_validation_and_conflict() {
    let f = Fixture::new();
    assert_eq!(
        f.post(global(0, "read_only"), false).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, catalog) = f.post(global(0, "read_only"), true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(catalog["revision"], 1);
    assert_eq!(
        f.post(global(0, "read_only"), true).await.0,
        StatusCode::BAD_REQUEST
    );
    f.store.archive_project(&f.project).unwrap();
    assert!(f.save(json!({"scope":"project","agent_id":"codex","project_id":"project","access":null,"expected_revision":1})).is_err());
}
