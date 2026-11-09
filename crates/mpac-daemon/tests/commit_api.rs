use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mpac_daemon::{
    api::{self, App},
    model::Project,
    store::Store,
};
use serde_json::{Value, json};
use tower::ServiceExt;

struct Fixture {
    router: Router,
    directory: PathBuf,
    project: Project,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().into()
}
impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("mpac-commit-api-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let repo = directory.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        for (key, value) in [
            ("user.name", "MPAC API Fixture"),
            ("user.email", "fixture@localhost"),
            ("commit.gpgSign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            git(&repo, &["config", key, value]);
        }
        std::fs::write(repo.join("a.txt"), "before\n").unwrap();
        std::fs::write(repo.join("b.txt"), "unrelated before\n").unwrap();
        git(&repo, &["add", "--", "a.txt", "b.txt"]);
        git(&repo, &["commit", "-m", "Initial fixture"]);
        std::fs::write(repo.join("a.txt"), "reviewed change\n").unwrap();
        std::fs::write(repo.join("b.txt"), "unrelated staging\n").unwrap();
        git(&repo, &["add", "--", "b.txt"]);
        let store = Arc::new(Store::open(&directory.join("state.sqlite3")).unwrap());
        let root = mpac_workspace::Root {
            id: "root".into(),
            name: "repo".into(),
            path: repo.canonicalize().unwrap().to_str().unwrap().into(),
        };
        let project = Project {
            id: "project".into(),
            name: "Fixture".into(),
            roots: vec![root],
        };
        store.project(&project).unwrap();
        let router = api::router(
            Arc::new(App::new(
                store,
                "fixture-token".into(),
                "127.0.0.1:4317".into(),
            )),
            Path::new("unused"),
        );
        Self {
            router,
            directory,
            project,
        }
    }
    fn repo(&self) -> &Path {
        Path::new(&self.project.roots[0].path)
    }
    async fn request(
        &self,
        path: &str,
        body: Value,
        authorized: bool,
        origin: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("/api{path}"))
            .header("host", "127.0.0.1:4317")
            .header("content-type", "application/json");
        if authorized {
            request = request.header("authorization", "Bearer fixture-token");
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    async fn preview(&self) -> Value {
        let (status, body) = self
            .request(
                "/projects/project/commit-preview",
                json!({"root_id":"root","paths":["a.txt"]}),
                true,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }
    fn commit_request(preview: &Value) -> Value {
        json!({"root_id":"root","paths":["a.txt"],"message":"Commit reviewed fixture","expected_snapshot":preview["snapshot"],"request_id":uuid::Uuid::new_v4().to_string()})
    }
}

#[tokio::test]
async fn commit_receipt_retries_do_not_create_a_second_commit_or_sweep_unrelated_staging() {
    let fixture = Fixture::new();
    let preview = fixture.preview().await;
    let request = Fixture::commit_request(&preview);
    let (status, committed) = fixture
        .request("/projects/project/commit", request.clone(), true, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    assert_eq!(committed["committed"], true);
    let head = git(fixture.repo(), &["rev-parse", "HEAD"]);
    let (status, retry) = fixture
        .request("/projects/project/commit", request.clone(), true, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{retry}");
    assert_eq!(committed, retry);
    assert_eq!(git(fixture.repo(), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(fixture.repo(), &["show", "HEAD:b.txt"]),
        "unrelated before"
    );
    assert_eq!(
        git(fixture.repo(), &["show", ":b.txt"]),
        "unrelated staging"
    );
    let mut conflicting = request;
    conflicting["message"] = json!("Different operation with the same id");
    assert_eq!(
        fixture
            .request("/projects/project/commit", conflicting, true, None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn stale_commit_is_rejected_without_changing_git_and_retry_cannot_blindly_replay() {
    let fixture = Fixture::new();
    let preview = fixture.preview().await;
    let request = Fixture::commit_request(&preview);
    let before = git(fixture.repo(), &["rev-parse", "HEAD"]);
    std::fs::write(fixture.repo().join("a.txt"), "new edits after preview\n").unwrap();
    assert_eq!(
        fixture
            .request("/projects/project/commit", request.clone(), true, None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(git(fixture.repo(), &["rev-parse", "HEAD"]), before);
    assert_eq!(
        std::fs::read_to_string(fixture.repo().join("a.txt")).unwrap(),
        "new edits after preview\n"
    );
    assert_eq!(
        fixture
            .request("/projects/project/commit", request, true, None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn commit_requires_local_authorization_and_a_root_belonging_to_the_visible_project() {
    let fixture = Fixture::new();
    let preview = fixture.preview().await;
    let request = Fixture::commit_request(&preview);
    let before = git(fixture.repo(), &["rev-parse", "HEAD"]);
    assert_eq!(
        fixture
            .request("/projects/project/commit", request.clone(), false, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        fixture
            .request(
                "/projects/project/commit",
                request.clone(),
                true,
                Some("https://untrusted.example")
            )
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let mut wrong_root = request;
    wrong_root["root_id"] = json!("another-root");
    assert_eq!(
        fixture
            .request("/projects/project/commit", wrong_root, true, None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(git(fixture.repo(), &["rev-parse", "HEAD"]), before);
}
