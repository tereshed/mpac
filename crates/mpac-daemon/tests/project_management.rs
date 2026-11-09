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
    model::{Project, Session},
    store::{Store, same_project},
};
use serde_json::{Value, json};
use tower::ServiceExt;

const HOST: &str = "127.0.0.1:4317";
const TOKEN: &str = "project-management-test-token";

struct Cleanup(PathBuf);
impl Cleanup {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mpac-projects-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    store: Arc<Store>,
    cleanup: Cleanup,
}
impl Fixture {
    fn new() -> Self {
        let cleanup = Cleanup::new();
        Self {
            store: Arc::new(Store::open(&cleanup.0.join("state.sqlite3")).unwrap()),
            cleanup,
        }
    }
    fn router(&self) -> Router {
        api::router(
            Arc::new(App::new(self.store.clone(), TOKEN.into(), HOST.into())),
            Path::new("no-ui-needed"),
        )
    }
    fn directory(&self, name: &str) -> String {
        let path = self.cleanup.0.join(name);
        std::fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap().to_str().unwrap().to_owned()
    }
}

async fn call(
    router: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", HOST)
        .header("authorization", format!("Bearer {TOKEN}"));
    let body = match body {
        Some(body) => {
            request = request.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = router
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn create(router: &Router, paths: &[String]) -> Value {
    let (status, project) = call(
        router,
        "POST",
        "/api/projects",
        Some(json!({"name":"Original","paths":paths})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{project}");
    project
}

fn path(project: &Value, action: &str) -> String {
    format!("/api/projects/{}/{action}", project["id"].as_str().unwrap())
}
fn project(value: &Value) -> Project {
    serde_json::from_value(value.clone()).unwrap()
}

fn session(id: &str, project: &Project, status: &str) -> Session {
    Session {
        id: id.into(),
        project_id: project.id.clone(),
        root_id: project.roots[0].id.clone(),
        agent: "demo".into(),
        title: "metadata-only fixture".into(),
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
async fn project_mutations_require_token_and_reject_foreign_origin() {
    let fixture = Fixture::new();
    let router = fixture.router();
    let root = fixture.directory("root");
    let current = create(&router, std::slice::from_ref(&root)).await;
    for action in ["update", "delete", "restore"] {
        let body = match action {
            "update" => json!({"name":"changed","paths":[root],"expected":current}),
            "delete" => json!({"expected":current}),
            _ => json!({}),
        };
        for (token, origin, status) in [
            (None, None, StatusCode::UNAUTHORIZED),
            (
                Some(TOKEN),
                Some("https://foreign.example"),
                StatusCode::FORBIDDEN,
            ),
        ] {
            let mut request = Request::builder()
                .method("POST")
                .uri(path(&current, action))
                .header("host", HOST)
                .header("content-type", "application/json");
            if let Some(token) = token {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            if let Some(origin) = origin {
                request = request.header("origin", origin);
            }
            assert_eq!(
                router
                    .clone()
                    .oneshot(request.body(Body::from(body.to_string())).unwrap())
                    .await
                    .unwrap()
                    .status(),
                status
            );
        }
    }
    assert!(same_project(
        &fixture.store.projects().unwrap()[0],
        &project(&current)
    ));
    assert!(fixture.store.archived_projects().unwrap().is_empty());
}

#[tokio::test]
async fn update_preserves_ids_for_retained_and_canonical_alias_paths_and_allocates_new_ids() {
    let fixture = Fixture::new();
    let router = fixture.router();
    let first = fixture.directory("first");
    let second = fixture.directory("second");
    let added = fixture.directory("added");
    let current = create(&router, &[first.clone(), second.clone()]).await;
    let (status, updated) = call(&router,"POST",&path(&current,"update"),Some(json!({"name":"  Renamed  ","paths":[format!("{second}/."),first,added],"expected":current}))).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["name"], "Renamed");
    assert_eq!(updated["roots"][0]["id"], current["roots"][1]["id"]);
    assert_eq!(updated["roots"][1]["id"], current["roots"][0]["id"]);
    assert_ne!(updated["roots"][2]["id"], current["roots"][0]["id"]);
    assert_eq!(updated["id"], current["id"]);
}

#[tokio::test]
async fn stale_or_foreign_expected_snapshots_cannot_update_or_archive_current_project() {
    let fixture = Fixture::new();
    let router = fixture.router();
    let root = fixture.directory("root");
    let original = create(&router, std::slice::from_ref(&root)).await;
    let (status, updated) = call(
        &router,
        "POST",
        &path(&original, "update"),
        Some(json!({"name":"New name","paths":[root],"expected":original})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for action in ["update", "delete"] {
        let body = if action == "update" {
            json!({"name":"Lost update","paths":[updated["roots"][0]["path"]],"expected":original})
        } else {
            json!({"expected":original})
        };
        assert!(
            call(&router, "POST", &path(&original, action), Some(body))
                .await
                .0
                .is_client_error()
        );
    }
    let mut wrong = updated.clone();
    wrong["id"] = json!("foreign-project");
    assert!(
        call(
            &router,
            "POST",
            &path(&updated, "delete"),
            Some(json!({"expected":wrong}))
        )
        .await
        .0
        .is_client_error()
    );
    assert!(same_project(
        &fixture.store.projects().unwrap()[0],
        &project(&updated)
    ));
    assert!(fixture.store.archived_projects().unwrap().is_empty());
}

#[tokio::test]
async fn offline_unchanged_roots_can_be_retained_or_removed_but_new_missing_paths_are_rejected() {
    let fixture = Fixture::new();
    let router = fixture.router();
    let offline = fixture.directory("offline");
    let available = fixture.directory("available");
    let original = create(&router, &[offline.clone(), available.clone()]).await;
    std::fs::remove_dir(&offline).unwrap();
    let (status, retained) = call(
        &router,
        "POST",
        &path(&original, "update"),
        Some(json!({"name":"Offline rename","paths":[offline,available],"expected":original})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{retained}");
    assert_eq!(retained["roots"], original["roots"]);
    let missing = fixture
        .cleanup
        .0
        .join("new-missing")
        .to_string_lossy()
        .into_owned();
    assert!(
        call(
            &router,
            "POST",
            &path(&retained, "update"),
            Some(json!({"name":"invalid","paths":[missing],"expected":retained}))
        )
        .await
        .0
        .is_client_error()
    );
    let (status, removed) = call(&router,"POST",&path(&retained,"update"),Some(json!({"name":"Remove offline","paths":[retained["roots"][1]["path"]],"expected":retained}))).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["roots"].as_array().unwrap().len(), 1);
    assert_eq!(removed["roots"][0]["id"], original["roots"][1]["id"]);
}

#[tokio::test]
async fn update_rejects_empty_duplicate_relative_and_oversized_path_lists_without_mutating() {
    let fixture = Fixture::new();
    let router = fixture.router();
    let root = fixture.directory("root");
    let original = create(&router, std::slice::from_ref(&root)).await;
    for paths in [
        vec![],
        vec![root.clone(), format!("{root}/.")],
        vec!["relative".into()],
        vec![root; 13],
    ] {
        assert!(
            call(
                &router,
                "POST",
                &path(&original, "update"),
                Some(json!({"name":"invalid","paths":paths,"expected":original}))
            )
            .await
            .0
            .is_client_error()
        );
    }
    assert!(same_project(
        &fixture.store.projects().unwrap()[0],
        &project(&original)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn edit_new_trailing_space_root_does_not_select_its_trimmed_sibling() {
    let fixture = Fixture::new();
    let router = fixture.router();
    let initial = fixture.directory("initial");
    let exact = fixture.directory("selected ");
    let _sibling = fixture.directory("selected");
    let original = create(&router, std::slice::from_ref(&initial)).await;
    let (status, updated) = call(
        &router,
        "POST",
        &path(&original, "update"),
        Some(json!({"name":"exact","paths":[initial,exact],"expected":original})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert!(updated["roots"][1]["path"].as_str().unwrap().ends_with(' '));
}

#[tokio::test]
async fn active_main_and_additional_roots_remain_protected_beyond_recent_history_page() {
    let fixture = Fixture::new();
    let router = fixture.router();
    let paths = [
        fixture.directory("main"),
        fixture.directory("extra"),
        fixture.directory("unused"),
    ];
    let current = create(&router, &paths).await;
    let current_project = project(&current);
    let mut active = session("old-active", &current_project, "needs_attention");
    active.additional_roots = vec![current_project.roots[1].clone()];
    active.access.additional_root_ids = vec![current_project.roots[1].id.clone()];
    active.permission_args = vec!["frozen-argv".into()];
    fixture.store.session(&active).unwrap();
    for n in 0..205 {
        fixture
            .store
            .session(&session(&format!("recent-{n}"), &current_project, "exited"))
            .unwrap();
    }
    assert!(
        fixture
            .store
            .sessions()
            .unwrap()
            .iter()
            .all(|s| s.id != "old-active")
    );
    assert_eq!(fixture.store.active_sessions().unwrap().len(), 1);
    for retained in [
        vec![paths[1].clone(), paths[2].clone()],
        vec![paths[0].clone(), paths[2].clone()],
    ] {
        let (status, body) = call(
            &router,
            "POST",
            &path(&current, "update"),
            Some(json!({"name":"blocked","paths":retained,"expected":current})),
        )
        .await;
        assert!(status.is_client_error(), "{body}");
    }
    assert!(
        call(
            &router,
            "POST",
            &path(&current, "delete"),
            Some(json!({"expected":current}))
        )
        .await
        .0
        .is_client_error()
    );
    let added = fixture.directory("new");
    let (status,updated) = call(&router,"POST",&path(&current,"update"),Some(json!({"name":"Allowed rename and add","paths":[paths[0],paths[1],added],"expected":current}))).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    let after = fixture.store.active_sessions().unwrap().pop().unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(active).unwrap()
    );
}

#[tokio::test]
async fn archive_hides_project_and_history_blocks_launch_and_reads_then_restore_recovers_ids_and_files()
 {
    let fixture = Fixture::new();
    let router = fixture.router();
    let root = fixture.directory("root");
    std::fs::write(Path::new(&root).join("notes.md"), "keep this artifact").unwrap();
    let current = create(&router, std::slice::from_ref(&root)).await;
    let saved = project(&current);
    fixture
        .store
        .session(&session("finished", &saved, "exited"))
        .unwrap();
    for _ in 0..2 {
        assert_eq!(
            call(
                &router,
                "POST",
                &path(&current, "delete"),
                Some(json!({"expected":current}))
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let (_, hidden) = call(&router, "GET", "/api/state", None).await;
    assert!(hidden["projects"].as_array().unwrap().is_empty());
    assert!(hidden["sessions"].as_array().unwrap().is_empty());
    assert_eq!(hidden["archived_projects"][0], current);
    assert_eq!(hidden["capabilities"]["project_management"], true);
    assert!(fixture.store.has_any_projects().unwrap());
    assert!(
        call(
            &router,
            "GET",
            &format!(
                "/api/projects/{}/file?root_id={}&path=notes.md",
                saved.id, saved.roots[0].id
            ),
            None
        )
        .await
        .0
        .is_client_error()
    );
    assert!(
        call(
            &router,
            "POST",
            "/api/sessions",
            Some(json!({"project_id":saved.id,"root_id":saved.roots[0].id,"agent":"demo"}))
        )
        .await
        .0
        .is_client_error()
    );
    for _ in 0..2 {
        let (status, restored) =
            call(&router, "POST", &path(&current, "restore"), Some(json!({}))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(restored, current);
    }
    let (_, shown) = call(&router, "GET", "/api/state", None).await;
    assert_eq!(shown["projects"][0], current);
    assert_eq!(shown["sessions"][0]["id"], "finished");
    assert!(shown["archived_projects"].as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(Path::new(&root).join("notes.md")).unwrap(),
        "keep this artifact"
    );
}

#[tokio::test]
async fn visible_history_is_paginated_after_archived_projects_are_excluded() {
    let fixture = Fixture::new();
    let router = fixture.router();
    let root = fixture.directory("root");
    let visible = create(&router, std::slice::from_ref(&root)).await;
    let archived = create(&router, &[root]).await;
    fixture
        .store
        .session(&session("visible-old", &project(&visible), "exited"))
        .unwrap();
    for n in 0..205 {
        fixture
            .store
            .session(&session(
                &format!("hidden-{n}"),
                &project(&archived),
                "exited",
            ))
            .unwrap();
    }
    assert_eq!(
        call(
            &router,
            "POST",
            &path(&archived, "delete"),
            Some(json!({"expected":archived}))
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, state) = call(&router, "GET", "/api/state", None).await;
    assert_eq!(state["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(state["sessions"][0]["id"], "visible-old");
}

#[test]
fn migration_and_archive_survive_reopen_and_do_not_make_the_database_empty_for_demo_seed() {
    let cleanup = Cleanup::new();
    let path = cleanup.0.join("legacy.sqlite3");
    let original = Project {
        id: "legacy".into(),
        name: "Legacy".into(),
        roots: vec![],
    };
    {
        let database = rusqlite::Connection::open(&path).unwrap();
        database.execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY,payload TEXT NOT NULL); CREATE TABLE sessions(id TEXT PRIMARY KEY,payload TEXT NOT NULL);").unwrap();
        database
            .execute(
                "INSERT INTO projects VALUES(?1,?2)",
                rusqlite::params![original.id, serde_json::to_string(&original).unwrap()],
            )
            .unwrap();
    }
    {
        let store = Store::open(&path).unwrap();
        assert!(same_project(&store.projects().unwrap()[0], &original));
        store.archive_project(&original).unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert!(store.projects().unwrap().is_empty());
    assert!(store.has_any_projects().unwrap());
    assert!(same_project(
        &store.archived_projects().unwrap()[0],
        &original
    ));
    assert!(same_project(
        &store.restore_project("legacy").unwrap(),
        &original
    ));
}

#[test]
fn restore_limit_is_atomic_and_already_visible_restore_is_idempotent_at_the_limit() {
    let fixture = Fixture::new();
    let target = Project {
        id: "restore-target".into(),
        name: "Archived".into(),
        roots: vec![],
    };
    fixture.store.project(&target).unwrap();
    fixture.store.archive_project(&target).unwrap();
    for n in 0..100 {
        fixture
            .store
            .project(&Project {
                id: format!("project-{n}"),
                name: format!("Project {n}"),
                roots: vec![],
            })
            .unwrap();
    }
    assert!(fixture.store.restore_project(&target.id).is_err());
    assert_eq!(fixture.store.projects().unwrap().len(), 100);
    let first = fixture.store.projects().unwrap()[0].clone();
    assert!(same_project(
        &fixture.store.restore_project(&first.id).unwrap(),
        &first
    ));
    fixture.store.archive_project(&first).unwrap();
    assert!(same_project(
        &fixture.store.restore_project(&target.id).unwrap(),
        &target
    ));
    assert_eq!(fixture.store.projects().unwrap().len(), 100);
}

#[test]
fn concurrent_expected_updates_cannot_both_commit_and_stale_archive_rolls_back() {
    let fixture = Fixture::new();
    let original = Project {
        id: "atomic".into(),
        name: "Before".into(),
        roots: vec![],
    };
    fixture.store.project(&original).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = ["One", "Two"]
        .into_iter()
        .map(|name| {
            let store = fixture.store.clone();
            let barrier = barrier.clone();
            let expected = original.clone();
            std::thread::spawn(move || {
                let updated = Project {
                    name: name.into(),
                    ..expected.clone()
                };
                barrier.wait();
                store.update_project(&expected, &updated).is_ok()
            })
        })
        .collect();
    assert_eq!(
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|committed| *committed)
            .count(),
        1
    );
    assert!(fixture.store.archive_project(&original).is_err());
    assert_eq!(fixture.store.projects().unwrap().len(), 1);
    assert!(fixture.store.archived_projects().unwrap().is_empty());
}
