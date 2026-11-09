//! Explicit local commits. Receipts prevent a lost HTTP response from repeating
//! an operation; the engine independently verifies the reviewed repository tree.
use std::{path::Path, sync::Arc};

use anyhow::{Context, ensure};
use axum::{
    Json, Router,
    extract::{Path as RoutePath, State},
    routing::post,
};
use serde::Deserialize;
use serde_json::json;

use crate::api::{ApiResult, App, work};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewCommit {
    root_id: String,
    paths: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateCommit {
    root_id: String,
    paths: Vec<String>,
    message: String,
    expected_snapshot: String,
    request_id: String,
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/projects/{id}/commit-preview", post(preview))
        .route("/projects/{id}/commit", post(commit))
}

async fn preview(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<PreviewCommit>,
) -> ApiResult {
    work(move || {
        let _operation = app.launch_lock.lock().unwrap();
        let root = app.root(&id, &request.root_id)?;
        Ok(serde_json::to_value(mpac_workspace::commit_preview(
            Path::new(&root.path),
            &request.paths,
        )?)?)
    })
    .await
}

async fn commit(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<CreateCommit>,
) -> ApiResult {
    work(move || {
        let _operation = app.launch_lock.lock().unwrap();
        let root = app.root(&id, &request.root_id)?;
        let request_id = uuid::Uuid::parse_str(&request.request_id)
            .context("Нужен уникальный идентификатор операции коммита")?;
        ensure!(!request_id.is_nil(), "Нужен уникальный идентификатор операции коммита");
        let specification = json!({
            "project_id": id,
            "root_id": request.root_id,
            "paths": request.paths,
            "message": request.message,
            "expected_snapshot": request.expected_snapshot,
        });
        if let Some(receipt) = app.store.commit_receipt(&request.request_id)? {
            ensure!(receipt["request"] == specification, "Идентификатор коммита уже использован с другими параметрами");
            ensure!(receipt["status"] == "completed", "Операция коммита уже запускалась. Обновите Git-состояние и проверьте последний коммит перед новым действием");
            return Ok(receipt["result"].clone());
        }
        app.store.save_commit_receipt(&request.request_id, &json!({
            "request": specification,
            "status": "started",
        }))?;
        let result = mpac_workspace::commit_selected(
            Path::new(&root.path),
            &request.paths,
            &request.message,
            &request.expected_snapshot,
        );
        match result {
            Ok(result) => {
                let mut result = serde_json::to_value(result)?;
                // A Git commit may already exist even if saving its receipt
                // fails. Preserve that success and prevent blind retry.
                if app.store.save_commit_receipt(&request.request_id, &json!({
                    "request": specification,
                    "status": "completed",
                    "result": result,
                })).is_err() {
                    let prior = result.get("warning").and_then(|value| value.as_str()).unwrap_or("");
                    let outcome = if result["committed"] == true {
                        "Коммит создан, но подтверждение операции не сохранено. Проверьте показанный хеш перед повтором."
                    } else {
                        "Подтверждение операции не сохранено. Исход коммита неизвестен; проверьте историю Git перед повтором."
                    };
                    result["warning"] = json!(format!("{prior} {outcome}").trim());
                }
                Ok(result)
            }
            Err(error) => {
                let _ = app.store.save_commit_receipt(&request.request_id, &json!({
                    "request": specification,
                    "status": "failed",
                }));
                Err(error)
            }
        }
    }).await
}
