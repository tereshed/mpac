use super::*;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use tower::ServiceExt;

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Fixture {
    app: Arc<App>,
    store: Arc<Store>,
    root: PathBuf,
    session: Session,
    _cleanup: Cleanup,
}
impl Fixture {
    fn new(stored: &str, live: Option<&str>) -> Self {
        let path = std::env::temp_dir().join(format!("mpac-session-api-{}", model::id()));
        std::fs::create_dir_all(path.join("root")).unwrap();
        let root = path.join("root").canonicalize().unwrap();
        let store = Arc::new(Store::open(&path.join("state.sqlite3")).unwrap());
        let project = Project {
            id: model::id(),
            name: "Session fixture".into(),
            roots: vec![mpac_workspace::Root {
                id: model::id(),
                name: "root".into(),
                path: root.to_str().unwrap().into(),
            }],
        };
        store.project(&project).unwrap();
        let session = Session {
            id: model::id(),
            project_id: project.id.clone(),
            root_id: project.roots[0].id.clone(),
            agent: "demo".into(),
            title: "No CLI is launched".into(),
            status: stored.into(),
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
            title_source: crate::model::SessionTitleSource::Manual,
            auto_title_eligible: false,
            activity: Default::default(),
            environment: None,
            terminal_launch: None,
        };
        store.session(&session).unwrap();
        let app = Arc::new(App::new(
            store.clone(),
            "test-token".into(),
            "127.0.0.1:4317".into(),
        ));
        if let Some(status) = live {
            let mut meta = session.clone();
            meta.status = status.into();
            app.live
                .lock()
                .unwrap()
                .insert(meta.id.clone(), LiveSession::test_fixture(meta, &root));
        }
        Self {
            app,
            store,
            root,
            session,
            _cleanup: Cleanup(path),
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        authorized: bool,
        origin: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:4317");
        if authorized {
            request = request.header("authorization", "Bearer test-token");
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let body = if let Some(body) = body {
            request = request.header("content-type", "application/json");
            Body::from(body.to_string())
        } else {
            Body::empty()
        };
        let response = router(self.app.clone(), Path::new("no-ui"))
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    async fn action(&self, name: &str) -> (StatusCode, Value) {
        self.request(
            "POST",
            &format!("/api/sessions/{}/{name}", self.session.id),
            Some(json!({})),
            true,
            None,
        )
        .await
    }
    async fn upload(&self, name: &str, bytes: &[u8]) -> (StatusCode, Value) {
        self.request(
            "POST",
            &format!("/api/sessions/{}/attachments", self.session.id),
            Some(json!({"name":name,"data_base64":STANDARD.encode(bytes)})),
            true,
            None,
        )
        .await
    }
    fn record(&self, size: u64) {
        self.store
            .record_attachment(
                &self.session,
                &crate::attachments::Attachment {
                    id: model::id(),
                    name: "quota-fixture".into(),
                    path: "metadata-only fixture".into(),
                    size,
                    mime_type: "application/octet-stream".into(),
                },
            )
            .unwrap();
    }
}

#[tokio::test]
async fn delete_upload_and_restore_require_auth_and_expected_origin() {
    let fixture = Fixture::new("running", Some("running"));
    for action in ["delete", "restore", "attachments"] {
        let body = if action == "attachments" {
            json!({"name":"note.md","data_base64":"eA=="})
        } else {
            json!({})
        };
        let path = format!("/api/sessions/{}/{action}", fixture.session.id);
        assert_eq!(
            fixture
                .request("POST", &path, Some(body.clone()), false, None)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            fixture
                .request(
                    "POST",
                    &path,
                    Some(body),
                    true,
                    Some("https://foreign.example")
                )
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }
    assert!(!fixture.store.session_archived(&fixture.session.id).unwrap());
    assert_eq!(std::fs::read_dir(&fixture.root).unwrap().count(), 0);
}

#[tokio::test]
async fn active_delete_never_stops_execution_and_closed_delete_restore_is_idempotent() {
    let active = Fixture::new("running", Some("running"));
    assert!(active.action("delete").await.0.is_client_error());
    active
        .app
        .live_session(&active.session.id)
        .unwrap()
        .with_active(|_| Ok(()))
        .unwrap();
    assert!(!active.store.session_archived(&active.session.id).unwrap());
    let closed = Fixture::new("running", Some("stopped"));
    for _ in 0..2 {
        assert_eq!(closed.action("delete").await.0, StatusCode::OK);
    }
    let (_, state) = closed.request("GET", "/api/state", None, true, None).await;
    assert!(state["sessions"].as_array().unwrap().is_empty());
    assert_eq!(state["archived_sessions"][0]["status"], "stopped");
    assert_eq!(state["capabilities"]["session_management"], true);
    assert!(
        closed
            .request(
                "GET",
                &format!("/api/sessions/{}/output", closed.session.id),
                None,
                true,
                None
            )
            .await
            .0
            .is_client_error()
    );
    for _ in 0..2 {
        let (status, restored) = closed.action("restore").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(restored["session"]["status"], "stopped");
        assert_eq!(restored["session"]["id"], closed.session.id);
    }
    assert_eq!(
        closed
            .store
            .session_entry(&closed.session.id)
            .unwrap()
            .0
            .status,
        "stopped"
    );
}

#[tokio::test]
async fn archived_flags_survive_late_payload_writes_and_live_merge_cannot_resurrect() {
    let fixture = Fixture::new("stopped", Some("running"));
    fixture.store.archive_session(&fixture.session.id).unwrap();
    fixture.store.session(&fixture.session).unwrap();
    assert!(fixture.store.session_archived(&fixture.session.id).unwrap());
    let (_, state) = fixture.request("GET", "/api/state", None, true, None).await;
    assert!(state["sessions"].as_array().unwrap().is_empty());
    assert_eq!(state["archived_sessions"].as_array().unwrap().len(), 1);
    assert!(fixture.action("restore").await.0.is_client_error());
    assert!(fixture.store.session_archived(&fixture.session.id).unwrap());
    assert!(
        fixture
            .app
            .active_project_sessions(&fixture.session.project_id)
            .unwrap()
            .iter()
            .any(|s| s.id == fixture.session.id)
    );
}

#[tokio::test]
async fn archived_project_hides_sessions_and_session_restore_cannot_restore_project() {
    let fixture = Fixture::new("exited", None);
    fixture.store.archive_session(&fixture.session.id).unwrap();
    let project = fixture.store.projects().unwrap().pop().unwrap();
    fixture.store.archive_project(&project).unwrap();
    assert!(fixture.action("restore").await.0.is_client_error());
    let (_, state) = fixture.request("GET", "/api/state", None, true, None).await;
    assert!(state["sessions"].as_array().unwrap().is_empty());
    assert!(state["archived_sessions"].as_array().unwrap().is_empty());
    fixture.store.restore_project(&project.id).unwrap();
    assert_eq!(fixture.action("restore").await.0, StatusCode::OK);
    assert_eq!(
        fixture
            .store
            .session_entry(&fixture.session.id)
            .unwrap()
            .0
            .status,
        "exited"
    );
}

#[tokio::test]
async fn attachment_upload_over_normal_json_limit_copies_exact_bytes_without_cli_input() {
    let fixture = Fixture::new("running", Some("running"));
    assert_eq!(std::fs::read_dir(&fixture.root).unwrap().count(), 0);
    let bytes = vec![42; 100 * 1024];
    let (status, attachment) = fixture.upload("данные.bin", &bytes).await;
    assert_eq!(status, StatusCode::OK, "{attachment}");
    assert_eq!(attachment["size"], bytes.len());
    assert_eq!(
        std::fs::read(attachment["path"].as_str().unwrap()).unwrap(),
        bytes
    );
    assert_eq!(
        fixture
            .store
            .attachment_totals(&fixture.session.id)
            .unwrap(),
        (1, bytes.len() as u64)
    );
    assert!(!fixture.root.join(".gitignore").exists());
    assert_eq!(
        fixture
            .app
            .live_session(&fixture.session.id)
            .unwrap()
            .meta()
            .status,
        "running"
    );
}

#[tokio::test]
async fn attachments_reject_history_closed_stopping_archived_and_hidden_project_sessions() {
    for (stored, live) in [
        ("running", None),
        ("exited", None),
        ("stopped", Some("stopped")),
    ] {
        let fixture = Fixture::new(stored, live);
        assert!(fixture.upload("note.md", b"x").await.0.is_client_error());
        assert!(!fixture.root.join(".mpac-attachments").exists());
    }
    let fixture = Fixture::new("running", Some("running"));
    fixture
        .app
        .live_session(&fixture.session.id)
        .unwrap()
        .stop();
    assert!(fixture.upload("note", b"x").await.0.is_client_error());
    assert!(!fixture.root.join(".mpac-attachments").exists());
    let fixture = Fixture::new("stopped", Some("stopped"));
    fixture.store.archive_session(&fixture.session.id).unwrap();
    assert!(fixture.upload("note", b"x").await.0.is_client_error());
    let fixture = Fixture::new("running", Some("running"));
    let project = fixture.store.projects().unwrap().pop().unwrap();
    fixture.store.archive_project(&project).unwrap();
    assert!(fixture.upload("note", b"x").await.0.is_client_error());
    assert!(!fixture.root.join(".mpac-attachments").exists());
}

#[tokio::test]
async fn attachment_quotas_check_count_and_bytes_before_creating_files() {
    let fixture = Fixture::new("running", Some("running"));
    for _ in 0..50 {
        fixture.record(1);
    }
    assert!(fixture.upload("over", b"x").await.0.is_client_error());
    assert!(!fixture.root.join(".mpac-attachments").exists());
    assert_eq!(
        fixture
            .store
            .attachment_totals(&fixture.session.id)
            .unwrap(),
        (50, 50)
    );
    let fixture = Fixture::new("running", Some("running"));
    for _ in 0..10 {
        fixture.record(crate::attachments::MAX_FILE_BYTES as u64);
    }
    assert!(fixture.upload("over", b"x").await.0.is_client_error());
    assert!(!fixture.root.join(".mpac-attachments").exists());
    let metadata = crate::attachments::Attachment {
        id: model::id(),
        name: "over".into(),
        path: "unused".into(),
        size: 1,
        mime_type: "text/plain".into(),
    };
    assert!(
        fixture
            .store
            .record_attachment(&fixture.session, &metadata)
            .is_err()
    );
}

#[tokio::test]
async fn invalid_uploads_and_replaced_roots_leave_no_attachment_files() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new("running", Some("running"));
    assert!(fixture.upload("../escape", b"x").await.0.is_client_error());
    let (status, _) = fixture
        .request(
            "POST",
            &format!("/api/sessions/{}/attachments", fixture.session.id),
            Some(json!({"name":"note","data_base64":"invalid"})),
            true,
            None,
        )
        .await;
    assert!(status.is_client_error());
    assert!(!fixture.root.join(".mpac-attachments").exists());
    let original = fixture.root.with_file_name("moved-root");
    std::fs::rename(&fixture.root, &original).unwrap();
    std::fs::create_dir(&fixture.root).unwrap();
    assert!(fixture.upload("wrong-root", b"x").await.0.is_client_error());
    assert_eq!(std::fs::read_dir(&fixture.root).unwrap().count(), 0);
    std::fs::remove_dir(&fixture.root).unwrap();
    symlink(&original, &fixture.root).unwrap();
    assert!(
        fixture
            .upload("symlink-root", b"x")
            .await
            .0
            .is_client_error()
    );
    assert!(!original.join(".mpac-attachments").exists());
}

#[test]
fn lifecycle_stop_waits_for_upload_transaction_then_prevents_late_upload() {
    let fixture = Fixture::new("running", Some("running"));
    let live = fixture.app.live_session(&fixture.session.id).unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let upload = live.clone();
    let operation = std::thread::spawn(move || {
        upload.with_active(|_| {
            entered_tx.send(()).unwrap();
            release_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            Ok(())
        })
    });
    entered_rx.recv().unwrap();
    let stop_live = live.clone();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let stop = std::thread::spawn(move || {
        stop_live.stop();
        done_tx.send(()).unwrap();
    });
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_millis(20))
            .is_err()
    );
    release_tx.send(()).unwrap();
    operation.join().unwrap().unwrap();
    stop.join().unwrap();
    assert!(live.with_active(|_| Ok(())).is_err());
}

#[tokio::test]
async fn concurrent_duplicate_delete_requests_are_idempotent_and_preserve_history() {
    let fixture = Fixture::new("exited", None);
    let (first, second) = tokio::join!(fixture.action("delete"), fixture.action("delete"));
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(second.0, StatusCode::OK);
    assert!(fixture.store.session_archived(&fixture.session.id).unwrap());
    assert_eq!(
        fixture
            .store
            .session_entry(&fixture.session.id)
            .unwrap()
            .0
            .status,
        "exited"
    );
    assert_eq!(fixture.action("restore").await.0, StatusCode::OK);
}

async fn rename(fixture: &Fixture, title: &str, expected: &str) -> (StatusCode, Value) {
    fixture
        .request(
            "POST",
            &format!("/api/sessions/{}/rename", fixture.session.id),
            Some(json!({"title":title,"expected_title":expected})),
            true,
            None,
        )
        .await
}

#[tokio::test]
async fn manual_rename_updates_live_and_stored_metadata_without_input_and_preserves_cli_candidate()
{
    let fixture = Fixture::new("running", Some("running"));
    let mut meta = fixture.session.clone();
    meta.cli_title = Some("CLI candidate".into());
    fixture.app.live.lock().unwrap().insert(
        meta.id.clone(),
        LiveSession::test_fixture(meta, &fixture.root),
    );
    let (status, renamed) = rename(&fixture, "  Ручное имя  ", &fixture.session.title).await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["title"], "Ручное имя");
    assert_eq!(renamed["cli_title"], "CLI candidate");
    assert_eq!(renamed["status"], "running");
    let stored = fixture.store.session_entry(&fixture.session.id).unwrap().0;
    assert_eq!(stored.title, "Ручное имя");
    assert_eq!(stored.cli_title.as_deref(), Some("CLI candidate"));
    assert_eq!(
        fixture
            .app
            .live_session(&fixture.session.id)
            .unwrap()
            .meta()
            .title,
        "Ручное имя"
    );
    assert!(
        rename(&fixture, "Lost update", &fixture.session.title)
            .await
            .0
            .is_client_error()
    );
}

#[tokio::test]
async fn rename_validates_utf8_length_controls_and_exact_expected_title_for_history() {
    let fixture = Fixture::new("stopped", None);
    for invalid in [
        "".to_owned(),
        "  ".into(),
        "x".repeat(121),
        "я".repeat(61),
        "bad\nname".into(),
        "bad\u{0085}name".into(),
        "bad\u{202e}name".into(),
        "bad\u{2066}name".into(),
    ] {
        assert!(
            rename(&fixture, &invalid, &fixture.session.title)
                .await
                .0
                .is_client_error()
        );
    }
    let valid = "я".repeat(60);
    assert_eq!(
        rename(&fixture, &valid, &fixture.session.title).await.0,
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .store
            .session_entry(&fixture.session.id)
            .unwrap()
            .0
            .title,
        valid
    );
    let mut legacy = fixture.session.clone();
    legacy.title = " original ".into();
    fixture.store.session(&legacy).unwrap();
    assert!(
        rename(&fixture, "Updated", "original")
            .await
            .0
            .is_client_error()
    );
    assert_eq!(
        rename(&fixture, " Updated ", " original ").await.0,
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .store
            .session_entry(&fixture.session.id)
            .unwrap()
            .0
            .title,
        "Updated"
    );
}

#[tokio::test]
async fn rename_rejects_archived_session_archived_project_unauthorized_and_foreign_origin() {
    let fixture = Fixture::new("exited", None);
    let uri = format!("/api/sessions/{}/rename", fixture.session.id);
    let body = json!({"title":"New","expected_title":fixture.session.title});
    assert_eq!(
        fixture
            .request("POST", &uri, Some(body.clone()), false, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(
                "POST",
                &uri,
                Some(body),
                true,
                Some("https://foreign.example")
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    fixture.store.archive_session(&fixture.session.id).unwrap();
    assert!(
        rename(&fixture, "New", &fixture.session.title)
            .await
            .0
            .is_client_error()
    );
    fixture.store.restore_session(&fixture.session.id).unwrap();
    let project = fixture.store.projects().unwrap().pop().unwrap();
    fixture.store.archive_project(&project).unwrap();
    assert!(
        rename(&fixture, "New", &fixture.session.title)
            .await
            .0
            .is_client_error()
    );
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=MPAC Test",
            "-c",
            "user.email=mpac@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn repo(fixture: &Fixture) {
    git(&fixture.root, &["init", "--quiet", "-b", "main"]);
    std::fs::write(fixture.root.join("notes.md"), "main\n").unwrap();
    git(&fixture.root, &["add", "notes.md"]);
    git(&fixture.root, &["commit", "--quiet", "-m", "fixture"]);
    git(&fixture.root, &["branch", "other"]);
}
fn branch_uri(fixture: &Fixture) -> String {
    format!(
        "/api/projects/{}/branches?root_id={}",
        fixture.session.project_id, fixture.session.root_id
    )
}
async fn switch(fixture: &Fixture, branch: &str) -> (StatusCode, Value) {
    let state = mpac_workspace::branch_state(&fixture.root).unwrap();
    fixture.request("POST",&format!("/api/projects/{}/branch",fixture.session.project_id),Some(json!({"root_id":fixture.session.root_id,"branch":branch,"expected_head":state.head,"expected_branch":state.current})),true,None).await
}

#[tokio::test]
async fn branch_routes_are_authenticated_and_do_not_switch_while_current_cli_is_active() {
    let fixture = Fixture::new("running", Some("running"));
    repo(&fixture);
    assert_eq!(
        fixture
            .request("GET", &branch_uri(&fixture), None, false, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let uri = format!("/api/projects/{}/branch", fixture.session.project_id);
    assert_eq!(
        fixture
            .request(
                "POST",
                &uri,
                Some(json!({})),
                true,
                Some("https://foreign.example")
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, branches) = fixture
        .request("GET", &branch_uri(&fixture), None, true, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(branches["blocked_reason"].is_string());
    assert!(switch(&fixture, "other").await.0.is_client_error());
    assert_eq!(
        mpac_workspace::branch_state(&fixture.root)
            .unwrap()
            .current
            .as_deref(),
        Some("main")
    );
    assert!(
        fixture
            .app
            .live_session(&fixture.session.id)
            .unwrap()
            .meta()
            .active()
    );
}

#[tokio::test]
async fn branch_guard_checks_foreign_project_frozen_extras_and_old_active_records() {
    let fixture = Fixture::new("exited", None);
    repo(&fixture);
    let other = fixture._cleanup.0.join("other-root");
    std::fs::create_dir(&other).unwrap();
    let other = other.canonicalize().unwrap();
    let project = Project {
        id: model::id(),
        name: "Foreign".into(),
        roots: vec![
            mpac_workspace::Root {
                id: model::id(),
                name: "Other".into(),
                path: other.to_str().unwrap().into(),
            },
            mpac_workspace::Root {
                id: model::id(),
                name: "Same target".into(),
                path: fixture.root.to_str().unwrap().into(),
            },
        ],
    };
    fixture.store.project(&project).unwrap();
    let mut active = fixture.session.clone();
    active.id = model::id();
    active.project_id = project.id.clone();
    active.root_id = project.roots[0].id.clone();
    active.status = "running".into();
    active.additional_roots = vec![project.roots[1].clone()];
    fixture.store.session(&active).unwrap();
    for _ in 0..205 {
        let mut old = fixture.session.clone();
        old.id = model::id();
        fixture.store.session(&old).unwrap();
    }
    assert!(
        fixture
            .store
            .sessions()
            .unwrap()
            .iter()
            .all(|s| s.id != active.id)
    );
    let (_, state) = fixture
        .request("GET", &branch_uri(&fixture), None, true, None)
        .await;
    assert!(state["blocked_reason"].is_string());
    assert!(switch(&fixture, "other").await.0.is_client_error());
}

#[tokio::test]
async fn branch_guard_blocks_ancestor_git_root_but_allows_sibling_worktree_with_shared_git_directory()
 {
    let fixture = Fixture::new("exited", None);
    repo(&fixture);
    git(&fixture.root, &["branch", "free"]);
    let worktree = fixture._cleanup.0.join("sibling-worktree");
    git(
        &fixture.root,
        &[
            "worktree",
            "add",
            "--quiet",
            worktree.to_str().unwrap(),
            "other",
        ],
    );
    let worktree = worktree.canonicalize().unwrap();
    let project = Project {
        id: model::id(),
        name: "Sibling worktree".into(),
        roots: vec![mpac_workspace::Root {
            id: model::id(),
            name: "Sibling".into(),
            path: worktree.to_str().unwrap().into(),
        }],
    };
    fixture.store.project(&project).unwrap();
    let mut active = fixture.session.clone();
    active.id = model::id();
    active.project_id = project.id.clone();
    active.root_id = project.roots[0].id.clone();
    active.status = "running".into();
    fixture.store.session(&active).unwrap();
    fixture.app.live.lock().unwrap().insert(
        active.id.clone(),
        LiveSession::test_fixture(active, &worktree),
    );
    let (_, state) = fixture
        .request("GET", &branch_uri(&fixture), None, true, None)
        .await;
    assert!(state["blocked_reason"].is_null(), "{state}");
    assert_eq!(switch(&fixture, "free").await.0, StatusCode::OK);
    assert_eq!(
        mpac_workspace::branch_state(&worktree)
            .unwrap()
            .current
            .as_deref(),
        Some("other")
    );
    git(&fixture._cleanup.0, &["init", "--quiet"]);
    let parent = fixture._cleanup.0.canonicalize().unwrap();
    let project = Project {
        id: model::id(),
        name: "Parent grant".into(),
        roots: vec![mpac_workspace::Root {
            id: model::id(),
            name: "Parent".into(),
            path: parent.to_str().unwrap().into(),
        }],
    };
    fixture.store.project(&project).unwrap();
    let mut active = fixture.session.clone();
    active.id = model::id();
    active.project_id = project.id;
    active.root_id = project.roots[0].id.clone();
    active.status = "running".into();
    fixture.store.session(&active).unwrap();
    assert!(switch(&fixture, "main").await.0.is_client_error());
}

#[tokio::test]
async fn branch_guard_resolves_active_foreign_project_subdirectory_to_selected_repository() {
    let fixture = Fixture::new("exited", None);
    repo(&fixture);
    let subdir = fixture.root.join("src");
    std::fs::create_dir(&subdir).unwrap();
    let project = Project {
        id: model::id(),
        name: "Subdirectory session".into(),
        roots: vec![mpac_workspace::Root {
            id: model::id(),
            name: "src".into(),
            path: subdir.to_str().unwrap().into(),
        }],
    };
    fixture.store.project(&project).unwrap();
    let mut active = fixture.session.clone();
    active.id = model::id();
    active.project_id = project.id;
    active.root_id = project.roots[0].id.clone();
    active.status = "running".into();
    fixture.store.session(&active).unwrap();
    assert!(switch(&fixture, "other").await.0.is_client_error());
    assert_eq!(
        mpac_workspace::branch_state(&fixture.root)
            .unwrap()
            .current
            .as_deref(),
        Some("main")
    );
}

#[tokio::test]
async fn environment_picker_is_authorized_metadata_only_and_demo_rejects_env_before_launch() {
    let fixture = Fixture::new("stopped", None);
    std::fs::write(
        fixture.root.join(".env.custom"),
        "TOKEN=ENV_SECRET_NOT_FOR_API_901\n",
    )
    .unwrap();
    let query = format!(
        "/api/environment-files?path={}",
        fixture.root.to_str().unwrap()
    );
    let (status, _) = fixture.request("GET", &query, None, false, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = fixture
        .request("GET", &query, None, true, Some("https://foreign.invalid"))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = fixture.request("GET", &query, None, true, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.to_string().contains("ENV_SECRET_NOT_FOR_API_901"));
    assert!(
        body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == ".env.custom")
    );
    let count = fixture.store.sessions().unwrap().len();
    let (status,body)=fixture.request("POST","/api/sessions",Some(json!({"project_id":fixture.session.project_id,"root_id":fixture.session.root_id,"agent":"demo","title":"No launch","env_file":".env.custom"})),true,None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!body.to_string().contains("ENV_SECRET_NOT_FOR_API_901"));
    assert_eq!(fixture.store.sessions().unwrap().len(), count);
    assert!(fixture.app.live.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_command_uses_project_env_and_preserves_exit_status() {
    let fixture = Fixture::new("exited", None);
    std::fs::write(
        fixture.root.join("command.env"),
        "MPAC_TERMINAL_VALUE='quoted value'\n",
    )
    .unwrap();
    let (status, created) = fixture.request("POST", "/api/sessions", Some(json!({
        "project_id":fixture.session.project_id,"root_id":fixture.session.root_id,
        "agent":"command","command":"printf '%s|%s\\n' \"$PWD\" \"$MPAC_TERMINAL_VALUE\" | cat; exit 7", "env_file":"command.env"
    })), true, None).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().unwrap();
    let live = fixture.app.live_session(id).unwrap();
    for _ in 0..200 {
        if !live.meta().active() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let meta = live.meta();
    assert_eq!(meta.status, "failed");
    assert_eq!(meta.exit_code, Some(7));
    assert!(!meta.auto_title_eligible);
    let output = live.output(0);
    let text = String::from_utf8_lossy(&STANDARD.decode(output.data_base64).unwrap()).into_owned();
    assert!(
        text.contains(&format!("{}|quoted value", fixture.root.display())),
        "{text}"
    );
    let persisted = fixture.store.session_entry(id).unwrap().0;
    assert_eq!(
        persisted.terminal_launch.unwrap().command,
        meta.terminal_launch.unwrap().command
    );
    assert!(
        !serde_json::to_string(&persisted.environment)
            .unwrap()
            .contains("quoted value")
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_terminal_accepts_input_and_can_be_stopped_without_an_agent() {
    let fixture = Fixture::new("exited", None);
    let (status, created) = fixture.request("POST", "/api/sessions", Some(json!({
        "project_id":fixture.session.project_id,"root_id":fixture.session.root_id,"agent":"shell"
    })), true, None).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().unwrap();
    let live = fixture.app.live_session(id).unwrap();
    live.input("printf 'SHELL-QA-%s\\n' ready\r".into())
        .unwrap();
    let mut found = false;
    for _ in 0..200 {
        let output = live.output(0);
        let bytes = STANDARD.decode(output.data_base64).unwrap();
        if String::from_utf8_lossy(&bytes).contains("SHELL-QA-ready") {
            found = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(found, "interactive shell did not execute input");
    let (status, _) = fixture
        .request(
            "POST",
            &format!("/api/sessions/{id}/stop"),
            Some(json!({})),
            true,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    for _ in 0..200 {
        if !live.meta().active() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(live.meta().status, "stopped");
}

#[tokio::test]
async fn launch_command_cannot_leak_into_a_named_agent_or_empty_shell() {
    let fixture = Fixture::new("exited", None);
    for (agent, command) in [
        ("demo", Some("echo unexpected")),
        ("shell", Some("echo unexpected")),
        ("command", None),
        ("command", Some(" \n\t")),
    ] {
        let before = fixture.store.sessions().unwrap().len();
        let (status, _) = fixture.request("POST", "/api/sessions", Some(json!({
            "project_id":fixture.session.project_id,"root_id":fixture.session.root_id,"agent":agent,"command":command
        })), true, None).await;
        assert_ne!(status, StatusCode::OK);
        assert_eq!(fixture.store.sessions().unwrap().len(), before);
    }
}

#[tokio::test]
async fn terminal_launch_catalog_does_not_break_older_agent_only_clients() {
    let fixture = Fixture::new("exited", None);
    let (status, state) = fixture.request("GET", "/api/state", None, true, None).await;
    assert_eq!(status, StatusCode::OK);
    let ids = |key: &str| {
        state[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>()
    };
    assert!(
        !ids("agents")
            .iter()
            .any(|id| ["shell", "command"].contains(id))
    );
    assert_eq!(ids("terminal_launches"), vec!["shell", "command"]);
}
