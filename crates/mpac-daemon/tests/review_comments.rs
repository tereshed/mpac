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
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tower::ServiceExt;

const HOST: &str = "127.0.0.1:4317";
struct Fixture {
    store: Arc<Store>,
    dir: PathBuf,
    project: Project,
    router: Router,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("mpac-comments-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let root = dir.join("code");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("note.rs"), "one\ntwo\nthree\nfour\n").unwrap();
        let project = Project {
            id: "project".into(),
            name: "Own fixture".into(),
            roots: vec![mpac_workspace::Root {
                id: "root".into(),
                name: "code".into(),
                path: root.canonicalize().unwrap().to_str().unwrap().into(),
            }],
        };
        let store = Arc::new(Store::open(&dir.join("state.sqlite3")).unwrap());
        store.project(&project).unwrap();
        let router = api::router(
            Arc::new(App::new(store.clone(), "token".into(), HOST.into())),
            Path::new("unused"),
        );
        Self {
            store,
            dir,
            project,
            router,
        }
    }
    async fn request(&self, path: &str, body: Option<Value>, expected: StatusCode) -> Value {
        let mut request = Request::builder()
            .uri(path)
            .header("host", HOST)
            .header("authorization", "Bearer token");
        if body.is_some() {
            request = request
                .method("POST")
                .header("content-type", "application/json")
        }
        let response = self
            .router
            .clone()
            .oneshot(
                request
                    .body(
                        body.map(|v| Body::from(v.to_string()))
                            .unwrap_or_else(Body::empty),
                    )
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, expected, "{result}");
        result
    }
    async fn source_comment(&self) -> Value {
        let source = self
            .request(
                "/api/projects/project/source-lines?root_id=root&path=note.rs",
                None,
                StatusCode::OK,
            )
            .await;
        self.request("/api/projects/project/comments",Some(json!({"root_id":"root","path":"note.rs","source":"file","snapshot":source["snapshot"],"anchor":{"kind":"range","start_row":1,"end_row":2,"side":"new"},"text":"Review BOTH selected lines"})),StatusCode::OK).await
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test]
async fn exact_source_range_persists_and_stale_editor_cannot_overwrite() {
    let f = Fixture::new();
    let comment = f.source_comment().await;
    assert_eq!(comment["new_start"], 2);
    assert_eq!(comment["new_end"], 3);
    let excerpt = comment["excerpt"].as_str().unwrap();
    assert!(excerpt.contains("two") && excerpt.contains("three") && !excerpt.contains("four"));
    let id = comment["id"].as_str().unwrap();
    let updated = f
        .request(
            &format!("/api/review-comments/{id}/update"),
            Some(json!({"expected_revision":1,"text":"Changed note"})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(updated["revision"], 2);
    f.request(
        &format!("/api/review-comments/{id}/update"),
        Some(json!({"expected_revision":1,"text":"Stale overwrite"})),
        StatusCode::BAD_REQUEST,
    )
    .await;
    let catalog = f
        .request("/api/projects/project/comments", None, StatusCode::OK)
        .await;
    assert_eq!(catalog["comments"][0]["text"], "Changed note");
    assert_eq!(catalog["comments"][0]["validation_state"], "unchecked");
    let reopened = Store::open(&f.dir.join("state.sqlite3")).unwrap();
    assert_eq!(reopened.review_comment(id).unwrap().revision, 2);
    f.request(
        &format!("/api/review-comments/{id}/delete"),
        Some(json!({"expected_revision":2})),
        StatusCode::OK,
    )
    .await;
    assert!(
        f.request("/api/projects/project/comments", None, StatusCode::OK)
            .await["comments"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn unrelated_edits_allow_explicit_exact_revalidation_but_changed_range_fails() {
    let f = Fixture::new();
    let comment = f.source_comment().await;
    let id = comment["id"].as_str().unwrap();
    let root = Path::new(&f.project.roots[0].path);
    std::fs::write(root.join("note.rs"), "ONE changed\ntwo\nthree\nfour\n").unwrap();
    let fresh = f
        .request(
            &format!("/api/review-comments/{id}/revalidate"),
            Some(json!({"expected_revision":1})),
            StatusCode::OK,
        )
        .await;
    assert_eq!(fresh["revision"], 2);
    assert_ne!(fresh["snapshot"], comment["snapshot"]);
    assert_eq!(fresh["excerpt"], comment["excerpt"]);
    std::fs::write(
        root.join("note.rs"),
        "ONE changed\ntwo CHANGED\nthree\nfour\n",
    )
    .unwrap();
    f.request(
        &format!("/api/review-comments/{id}/revalidate"),
        Some(json!({"expected_revision":2})),
        StatusCode::BAD_REQUEST,
    )
    .await;
    let catalog = f
        .request("/api/projects/project/comments", None, StatusCode::OK)
        .await;
    assert_eq!(catalog["comments"][0]["stale"], true);
}

#[tokio::test]
async fn project_archive_and_source_traversal_are_rejected() {
    let f = Fixture::new();
    f.request(
        "/api/projects/project/source-lines?root_id=root&path=../../state.sqlite3",
        None,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let comment = f.source_comment().await;
    f.store.archive_project(&f.project).unwrap();
    f.request(
        "/api/projects/project/comments",
        None,
        StatusCode::BAD_REQUEST,
    )
    .await;
    f.request(
        &format!(
            "/api/review-comments/{}/update",
            comment["id"].as_str().unwrap()
        ),
        Some(json!({"expected_revision":1,"text":"Hidden project"})),
        StatusCode::BAD_REQUEST,
    )
    .await;
}

#[test]
fn receipt_guarded_transitions_and_uncertain_batch_restart() {
    use mpac_daemon::review_comments::{BatchStatus, ReviewBatch};
    let f = Fixture::new();
    let request = json!({"project":"own","paths":["a"]});
    f.store
        .save_commit_receipt("one", &json!({"request":request,"status":"started"}))
        .unwrap();
    assert!(
        f.store
            .save_commit_receipt(
                "one",
                &json!({"request":{"different":true},"status":"completed"})
            )
            .is_err()
    );
    f.store
        .save_commit_receipt(
            "one",
            &json!({"request":request,"status":"completed","result":{"head":"abc"}}),
        )
        .unwrap();
    f.store
        .save_commit_receipt(
            "one",
            &json!({"request":request,"status":"completed","result":{"head":"abc"}}),
        )
        .unwrap();
    assert!(
        f.store
            .save_commit_receipt("one", &json!({"request":request,"status":"failed"}))
            .is_err()
    );
    let batch = ReviewBatch {
        id: "batch".into(),
        project_id: "project".into(),
        session_id: "session".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        comments: vec![],
        message: "task".into(),
        text: "unchanged multiline\n    code".into(),
        status: BatchStatus::Draft,
        attachment_path: None,
        created_at: 1,
        updated_at: 1,
    };
    f.store.create_review_batch(&batch).unwrap();
    f.store
        .transition_review_batch("batch", BatchStatus::Draft, BatchStatus::Preparing)
        .unwrap();
    let reopened = Store::open(&f.dir.join("state.sqlite3")).unwrap();
    assert_eq!(
        reopened.review_batch("batch").unwrap().status,
        BatchStatus::Uncertain
    );
    assert!(
        reopened
            .transition_review_batch("batch", BatchStatus::Draft, BatchStatus::Prepared)
            .is_err()
    );
}

#[tokio::test]
async fn delivery_history_is_per_session_and_comment_version() {
    use mpac_daemon::review_comments::{BatchStatus, CommentRef, ReviewBatch};
    let f = Fixture::new();
    let comment = f.source_comment().await;
    let id = comment["id"].as_str().unwrap();
    let batch = ReviewBatch {
        id: "delivered".into(),
        project_id: "project".into(),
        session_id: "first-recipient".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        comments: vec![CommentRef {
            id: id.into(),
            revision: 1,
        }],
        message: "task".into(),
        text: "packet".into(),
        status: BatchStatus::Draft,
        attachment_path: None,
        created_at: 1,
        updated_at: 1,
    };
    f.store.create_review_batch(&batch).unwrap();
    f.store
        .transition_review_batch(&batch.id, BatchStatus::Draft, BatchStatus::Preparing)
        .unwrap();
    f.store
        .transition_review_batch(&batch.id, BatchStatus::Preparing, BatchStatus::Prepared)
        .unwrap();
    assert!(
        f.store.review_comments("project").unwrap()[0]
            .deliveries
            .is_empty()
    );
    f.store.confirm_review_batch(&batch.id).unwrap();
    f.store.confirm_review_batch(&batch.id).unwrap();
    let notes = f.store.review_comments("project").unwrap();
    assert_eq!(notes[0].deliveries.len(), 1);
    assert_eq!(notes[0].deliveries[0].session_id, "first-recipient");
    f.store
        .update_review_comment(id, 1, Some("Updated comment version"))
        .unwrap();
    assert!(
        f.store.review_comments("project").unwrap()[0]
            .deliveries
            .is_empty()
    );
    assert_eq!(
        f.store.review_batch(&batch.id).unwrap().comments[0].revision,
        1
    );
}
