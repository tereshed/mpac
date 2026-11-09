use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json, Router,
    extract::{Path as RoutePath, Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    api::{ApiError, App},
    model::{self, Session},
    review_comments::*,
    store::Store,
};

type ApiResult = std::result::Result<Json<Value>, ApiError>;
async fn work(f: impl FnOnce() -> Result<Value> + Send + 'static) -> ApiResult {
    Ok(Json(
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|_| anyhow::anyhow!("Ошибка задачи ревью"))??,
    ))
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/projects/{id}/review-lines", get(review_lines))
        .route("/projects/{id}/source-lines", get(source_lines))
        .route(
            "/projects/{id}/comments",
            get(list_comments).post(create_comment),
        )
        .route("/review-comments/{id}/update", post(update_comment))
        .route("/review-comments/{id}/delete", post(delete_comment))
        .route("/review-comments/{id}/revalidate", post(revalidate_comment))
        .route("/projects/{id}/review-batches", post(prepare_batch))
        .route("/review-batches/{id}/insert", post(insert_batch))
        .route("/review-batches/{id}/confirm", post(confirm_batch))
}

#[derive(Deserialize)]
struct SourceQuery {
    root_id: String,
    #[serde(default)]
    path: String,
}
#[derive(Deserialize)]
struct CommentQuery {
    root_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateComment {
    expected_revision: u64,
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedRevision {
    expected_revision: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    confirmed: bool,
}

fn visible_project(app: &App, id: &str) -> Result<()> {
    ensure!(
        app.store.projects()?.iter().any(|project| project.id == id),
        "Проект не найден или удалён"
    );
    Ok(())
}

async fn review_lines(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<SourceQuery>,
) -> ApiResult {
    work(move || {
        let root = app.root(&id, &query.root_id)?;
        let review = mpac_workspace::git_review(Path::new(&root.path))
            .context("Не удалось прочитать изменения")?;
        Ok(serde_json::to_value(
            mpac_workspace::review_lines::structured_review(&review),
        )?)
    })
    .await
}
async fn source_lines(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<SourceQuery>,
) -> ApiResult {
    work(move || {
        let root = app.root(&id, &query.root_id)?;
        Ok(serde_json::to_value(
            mpac_workspace::review_lines::source_lines(Path::new(&root.path), &query.path)?,
        )?)
    })
    .await
}

enum Capture {
    Diff(mpac_workspace::review_lines::ReviewLines),
    File(mpac_workspace::review_lines::SourceLines),
}
struct CurrentCapture {
    identity: String,
    value: Capture,
}
type CaptureCache = HashMap<(String, String), CurrentCapture>;
fn root_identity(path: &Path) -> Result<String> {
    let metadata = path.metadata().context("Папка недоступна")?;
    ensure!(metadata.is_dir(), "Папка проекта заменена");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        Ok(path.canonicalize()?.to_string_lossy().into_owned())
    }
}
fn current_capture<'a>(
    app: &App,
    comment: &ReviewComment,
    cache: &'a mut CaptureCache,
) -> Result<&'a CurrentCapture> {
    let key = (
        comment.root_id.clone(),
        if comment.source == CommentSource::File {
            comment.path.clone()
        } else {
            String::new()
        },
    );
    if !cache.contains_key(&key) {
        let root = app.root(&comment.project_id, &comment.root_id)?;
        let path = Path::new(&root.path);
        let identity = root_identity(path)?;
        let value = match comment.source {
            CommentSource::Diff => {
                let review = mpac_workspace::git_review(path)?;
                ensure!(!review.truncated, "Снимок изменений слишком велик");
                Capture::Diff(mpac_workspace::review_lines::structured_review(&review))
            }
            CommentSource::File => Capture::File(mpac_workspace::review_lines::source_lines(
                path,
                &comment.path,
            )?),
        };
        ensure!(
            root_identity(path)? == identity,
            "Папка заменена во время проверки"
        );
        cache.insert(key.clone(), CurrentCapture { identity, value });
    }
    Ok(cache.get(&key).unwrap())
}
fn matched_anchor(
    comment: &ReviewComment,
    current: &CurrentCapture,
) -> Result<(String, CommentAnchor)> {
    ensure!(
        comment.root_identity == current.identity,
        "Папка комментария заменена; создайте новый комментарий"
    );
    match (&current.value, &comment.anchor) {
        (
            Capture::Diff(view),
            CommentAnchor::Range {
                start_row,
                end_row,
                side,
                ..
            },
        ) => {
            let mut matches = Vec::new();
            let width = end_row.saturating_sub(*start_row);
            for block in view.blocks.iter().filter(|block| {
                block.path == comment.path
                    && Some(block.section.as_str()) == comment.diff_section.as_deref()
            }) {
                for start in 0..block.lines.len().saturating_sub(width as usize) {
                    let window = &block.lines[start..=start + width as usize];
                    let old: Vec<_> = window
                        .iter()
                        .filter(|_| *side != AnchorSide::New)
                        .filter_map(|line| line.old_line)
                        .collect();
                    let new: Vec<_> = window
                        .iter()
                        .filter(|_| *side != AnchorSide::Old)
                        .filter_map(|line| line.new_line)
                        .collect();
                    if (
                        old.first().copied(),
                        old.last().copied(),
                        new.first().copied(),
                        new.last().copied(),
                    ) != (
                        comment.old_start,
                        comment.old_end,
                        comment.new_start,
                        comment.new_end,
                    ) {
                        continue;
                    }
                    let Ok((excerpt, a, b, c, d)) =
                        range_excerpt(&block.lines, start as u32, start as u32 + width, *side)
                    else {
                        continue;
                    };
                    if excerpt == comment.excerpt
                        && (a, b, c, d)
                            == (
                                comment.old_start,
                                comment.old_end,
                                comment.new_start,
                                comment.new_end,
                            )
                    {
                        matches.push(CommentAnchor::Range {
                            block_id: Some(block.id),
                            start_row: start as u32,
                            end_row: start as u32 + width,
                            side: *side,
                        });
                    }
                }
            }
            ensure!(
                matches.len() == 1,
                "Фрагмент изменился или неоднозначен. Проверьте привязку или создайте новый комментарий"
            );
            Ok((view.snapshot.clone(), matches.remove(0)))
        }
        (Capture::Diff(view), CommentAnchor::File) => {
            let text = view
                .blocks
                .iter()
                .filter(|block| block.path == comment.path)
                .map(|block| block.raw.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            ensure!(
                !text.is_empty()
                    && mpac_workspace::review_lines::review_text_fingerprint(&text)
                        == comment.capture_fingerprint,
                "Изменения файла обновились; проверьте комментарий"
            );
            Ok((view.snapshot.clone(), CommentAnchor::File))
        }
        (
            Capture::File(view),
            CommentAnchor::Range {
                start_row, end_row, ..
            },
        ) => {
            let (excerpt, a, b, c, d) =
                range_excerpt(&view.lines, *start_row, *end_row, AnchorSide::New)?;
            ensure!(
                excerpt == comment.excerpt
                    && (a, b, c, d)
                        == (
                            comment.old_start,
                            comment.old_end,
                            comment.new_start,
                            comment.new_end
                        ),
                "Выделенные строки файла изменились; создайте новый комментарий"
            );
            Ok((view.snapshot.clone(), comment.anchor.clone()))
        }
        (Capture::File(view), CommentAnchor::File) => {
            ensure!(
                view.snapshot == comment.capture_fingerprint,
                "Файл изменился; проверьте комментарий к файлу"
            );
            Ok((view.snapshot.clone(), CommentAnchor::File))
        }
    }
}

async fn list_comments(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<CommentQuery>,
) -> ApiResult {
    work(move || {
        let _lock = app.launch_lock.lock().unwrap();
        visible_project(&app, &id)?;
        if let Some(root) = &query.root_id {
            app.root(&id, root)?;
        }
        let mut comments = app.store.review_comments(&id)?;
        if let Some(root) = &query.root_id {
            comments.retain(|c| &c.root_id == root)
        }
        // Polling the catalog must not hold the launch mutex across filesystem
        // captures. Only explicit prepare/insert/revalidate performs validation.
        for comment in &mut comments {
            comment.validation_state = if comment.stale { "stale" } else { "unchecked" }.into();
        }
        Ok(json!({"comments":comments,"batches":app.store.review_batches(&id)?}))
    })
    .await
}

fn capture_comment(app: &App, project: &str, request: CreateComment) -> Result<ReviewComment> {
    let root = app.root(project, &request.root_id)?;
    let path = Path::new(&root.path);
    let identity = root_identity(path)?;
    let capture_fingerprint;
    let mut diff_section = None;
    let text = clean_note(&request.text)?;
    let (excerpt, truncated, old_start, old_end, new_start, new_end) = match request.source {
        CommentSource::Diff => {
            let review = mpac_workspace::git_review(path)?;
            ensure!(
                !review.truncated,
                "Снимок слишком велик; уменьшите изменения перед комментированием"
            );
            ensure!(
                review.snapshot == request.snapshot,
                "Изменения обновились; перечитайте diff"
            );
            ensure!(
                review.files.iter().any(|f| f.path == request.path),
                "Файл не входит в текущий снимок изменений"
            );
            let parsed = mpac_workspace::review_lines::structured_review(&review);
            match &request.anchor {
                CommentAnchor::File => {
                    let raw = parsed
                        .blocks
                        .iter()
                        .filter(|b| b.path == request.path)
                        .map(|b| b.raw.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    ensure!(
                        !raw.is_empty(),
                        "Diff файла недоступен; оставьте комментарий к исходнику"
                    );
                    capture_fingerprint =
                        mpac_workspace::review_lines::review_text_fingerprint(&raw);
                    let (excerpt, truncated) = bounded_excerpt(&raw);
                    (excerpt, truncated, None, None, None, None)
                }
                CommentAnchor::Range {
                    block_id,
                    start_row,
                    end_row,
                    side,
                } => {
                    let block = parsed
                        .blocks
                        .iter()
                        .find(|b| Some(b.id) == *block_id && b.path == request.path)
                        .context("Блок изменений больше недоступен")?;
                    diff_section = Some(block.section.clone());
                    let (excerpt, a, b, c, d) =
                        range_excerpt(&block.lines, *start_row, *end_row, *side)?;
                    capture_fingerprint =
                        mpac_workspace::review_lines::review_text_fingerprint(&excerpt);
                    (excerpt, false, a, b, c, d)
                }
            }
        }
        CommentSource::File => {
            let source = mpac_workspace::review_lines::source_lines(path, &request.path)?;
            ensure!(
                source.snapshot == request.snapshot,
                "Файл изменился; перечитайте исходник"
            );
            match &request.anchor {
                CommentAnchor::File => {
                    capture_fingerprint = source.snapshot.clone();
                    let raw = source
                        .lines
                        .iter()
                        .map(|line| line.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let (excerpt, truncated) = bounded_excerpt(&raw);
                    (
                        excerpt,
                        truncated || source.lines_truncated,
                        None,
                        None,
                        None,
                        None,
                    )
                }
                CommentAnchor::Range {
                    block_id,
                    start_row,
                    end_row,
                    side,
                } => {
                    ensure!(
                        block_id.is_none() && *side != AnchorSide::Old,
                        "Исходник содержит только новые строки"
                    );
                    let (excerpt, a, b, c, d) =
                        range_excerpt(&source.lines, *start_row, *end_row, AnchorSide::New)?;
                    capture_fingerprint =
                        mpac_workspace::review_lines::review_text_fingerprint(&excerpt);
                    (excerpt, false, a, b, c, d)
                }
            }
        }
    };
    ensure!(
        root_identity(path)? == identity,
        "Папка заменена во время сохранения комментария"
    );
    let time = model::now();
    Ok(ReviewComment {
        id: model::id(),
        project_id: project.into(),
        root_id: request.root_id,
        path: request.path,
        source: request.source,
        snapshot: request.snapshot,
        anchor: request.anchor,
        excerpt,
        excerpt_truncated: truncated,
        capture_fingerprint,
        diff_section,
        root_identity: identity,
        old_start,
        old_end,
        new_start,
        new_end,
        text,
        revision: 1,
        created_at: time,
        updated_at: time,
        deleted: false,
        stale: false,
        validation_state: "checked".into(),
        deliveries: vec![],
    })
}

async fn create_comment(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<CreateComment>,
) -> ApiResult {
    work(move || {
        let _lock = app.launch_lock.lock().unwrap();
        let comment = capture_comment(&app, &id, request)?;
        app.store.create_review_comment(&comment)?;
        Ok(serde_json::to_value(comment)?)
    })
    .await
}
async fn update_comment(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<UpdateComment>,
) -> ApiResult {
    work(move || {
        let _lock = app.launch_lock.lock().unwrap();
        Ok(serde_json::to_value(app.store.update_review_comment(
            &id,
            request.expected_revision,
            Some(&request.text),
        )?)?)
    })
    .await
}
async fn delete_comment(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<ExpectedRevision>,
) -> ApiResult {
    work(move || {
        let _lock = app.launch_lock.lock().unwrap();
        let comment = app
            .store
            .update_review_comment(&id, request.expected_revision, None)?;
        Ok(json!({"id":comment.id,"deleted":true,"revision":comment.revision}))
    })
    .await
}

fn session_access(app: &App, session: &Session, comments: &[ReviewComment]) -> Result<()> {
    visible_project(app, &session.project_id)?;
    for comment in comments {
        ensure!(
            comment.project_id == session.project_id,
            "Комментарий относится к другому проекту"
        );
        ensure!(
            session.root_id == comment.root_id
                || session
                    .additional_roots
                    .iter()
                    .any(|r| r.id == comment.root_id),
            "У выбранной сессии нет доступа ко всем папкам пакета"
        );
        app.root(&comment.project_id, &comment.root_id)?;
    }
    Ok(())
}

fn batch_comments(store: &Store, batch: &ReviewBatch) -> Result<Vec<ReviewComment>> {
    batch
        .comments
        .iter()
        .map(|reference| {
            let comment = store.review_comment(&reference.id)?;
            ensure!(
                !comment.deleted
                    && comment.project_id == batch.project_id
                    && comment.revision == reference.revision,
                "Комментарий пакета уже изменён или удалён. Подготовьте новый пакет"
            );
            Ok(comment)
        })
        .collect()
}
fn validate_snapshots(app: &App, comments: &[ReviewComment]) -> Result<()> {
    let mut cache = CaptureCache::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
    for comment in comments {
        ensure!(
            std::time::Instant::now() < deadline,
            "Проверка пакета заняла слишком много времени; выберите меньше папок"
        );
        let result = current_capture(app, comment, &mut cache)
            .and_then(|capture| matched_anchor(comment, capture));
        let _ = app
            .store
            .mark_review_comment_stale(&comment.id, comment.revision, result.is_err());
        result?;
    }
    Ok(())
}

async fn prepare_batch(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<PrepareBatch>,
) -> ApiResult {
    work(move || {
        let _lock = app.launch_lock.lock().unwrap();
        visible_project(&app, &id)?;
        validate_prepare(&request)?;
        if let Some(existing) = app
            .store
            .review_batch_by_request(&id, &request.request_id)?
        {
            ensure!(
                existing.session_id == request.session_id
                    && existing.comments == request.comments
                    && existing.message == request.message.trim(),
                "Идентификатор пакета уже использован для другой задачи"
            );
            return Ok(serde_json::to_value(existing)?);
        }
        let live = app.live_session(&request.session_id)?;
        let session = live.meta();
        ensure!(
            session.active() && session.project_id == id,
            "Выберите работающую сессию этого проекта"
        );
        let time = model::now();
        let mut batch = ReviewBatch {
            id: model::id(),
            project_id: id,
            session_id: request.session_id,
            request_id: request.request_id,
            comments: request.comments,
            message: request.message.trim().into(),
            text: String::new(),
            status: BatchStatus::Draft,
            attachment_path: None,
            created_at: time,
            updated_at: time,
        };
        let comments = batch_comments(&app.store, &batch)?;
        session_access(&app, &session, &comments)?;
        validate_snapshots(&app, &comments)?;
        let mut addressed = comments.clone();
        for comment in &mut addressed {
            comment.root_id = app.root(&comment.project_id, &comment.root_id)?.path;
        }
        batch.text = packet(&batch.message, &addressed)?;
        app.store.create_review_batch(&batch)?;
        Ok(serde_json::to_value(batch)?)
    })
    .await
}

async fn insert_batch(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(_request): Json<Empty>,
) -> ApiResult {
    work(move||{
        let _lock=app.launch_lock.lock().unwrap();let batch=app.store.review_batch(&id)?;visible_project(&app,&batch.project_id)?;
        match batch.status{
            BatchStatus::Prepared|BatchStatus::HandedOff=>return Ok(serde_json::to_value(batch)?),
            BatchStatus::Preparing|BatchStatus::Uncertain=>bail!("Результат прошлой вставки неизвестен. Проверьте терминал; повторная вставка заблокирована"),
            BatchStatus::Draft=>{}
        }
        let live=app.live_session(&batch.session_id)?;
        live.with_active(|meta|{
            ensure!(meta.project_id==batch.project_id,"Пакет относится к другому проекту");
            let comments=batch_comments(&app.store,&batch)?;session_access(&app,meta,&comments)?;
            for comment in &comments{let root=app.root(&comment.project_id,&comment.root_id)?;live.verify_root(&comment.root_id,Path::new(&root.path))?;}
            validate_snapshots(&app,&comments)?;
            for comment in &comments{let root=app.root(&comment.project_id,&comment.root_id)?;live.verify_root(&comment.root_id,Path::new(&root.path))?;}
            app.store.transition_review_batch(&id,BatchStatus::Draft,BatchStatus::Preparing)?;
            let attachment=(||->Result<String>{
                let main=app.root(&meta.project_id,&meta.root_id)?;
                let main_path=Path::new(&main.path);
                live.verify_root(&meta.root_id,main_path)?;
                if let Some(path)=&batch.attachment_path{
                    let relative=Path::new(path).strip_prefix(main_path).context("Файл пакета вышел за пределы папки сессии")?;
                    let relative=relative.to_str().context("Недопустимый путь файла пакета")?;
                    ensure!(mpac_workspace::read_text(main_path,relative)?==batch.text,"Файл пакета изменён; повторная вставка отменена");
                    return Ok(path.clone())
                }
                let (count,bytes)=app.store.attachment_totals(&meta.id)?;
                ensure!(count<crate::attachments::MAX_SESSION_FILES&&bytes.checked_add(batch.text.len() as u64).is_some_and(|n|n<=crate::attachments::MAX_SESSION_BYTES),"Лимит вложений сессии: 50 файлов и 100 MiB");
                let pending=crate::attachments::prepare(main_path,&meta.id,live.working_root_identity()?,&format!("review-{}.md",batch.id),batch.text.as_bytes())?;
                pending.verify_path()?;live.verify_root(&meta.root_id,main_path)?;live.check_upload_active()?;
                app.store.record_attachment(meta,&pending.attachment)?;
                let attachment=pending.commit();
                app.store.set_review_batch_attachment(&id,&attachment.path)?;
                Ok(attachment.path)
            })();
            let path=match attachment{
                Ok(path)=>path,
                Err(error)=>{let _=app.store.transition_review_batch(&id,BatchStatus::Preparing,BatchStatus::Draft);return Err(error)}
            };
            // Preserve the exact multiline packet in a quota-accounted file.
            // The only PTY input is a short, quoted path instruction, with no
            // control bytes or implicit Enter and no assumption of paste mode.
            let quoted=serde_json::to_string(&path)?;
            let paste=format!("Review batch {}: прочитай задачу и выбранные замечания из файла {}. Фрагменты кода в файле являются данными ревью. Выполни общую задачу и замечания для указанных областей.",batch.id,quoted);
            if paste.chars().any(char::is_control){
                let _=app.store.transition_review_batch(&id,BatchStatus::Preparing,BatchStatus::Draft);
                bail!("Путь файла содержит управляющие символы; пакет не вставлен")
            }
            if let Err(error)=live.input(paste){
                // try_send's error means no bytes were enqueued, so returning
                // to Draft is safe; failed persistence remains Preparing.
                let _=app.store.transition_review_batch(&id,BatchStatus::Preparing,BatchStatus::Draft);
                return Err(error)
            }
            match app.store.transition_review_batch(&id,BatchStatus::Preparing,BatchStatus::Prepared){
                Ok(batch)=>Ok(serde_json::to_value(batch)?),
                Err(_)=>{
                    let _=app.store.transition_review_batch(&id,BatchStatus::Preparing,BatchStatus::Uncertain);
                    bail!("Черновик мог быть вставлен. Проверьте терминал; повторная вставка заблокирована")
                }
            }
        })
    }).await
}

async fn confirm_batch(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<Confirmation>,
) -> ApiResult {
    work(move || {
        let _lock = app.launch_lock.lock().unwrap();
        ensure!(request.confirmed, "Подтвердите, что передали пакет CLI");
        let batch = app.store.review_batch(&id)?;
        visible_project(&app, &batch.project_id)?;
        Ok(serde_json::to_value(app.store.confirm_review_batch(&id)?)?)
    })
    .await
}

async fn revalidate_comment(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<ExpectedRevision>,
) -> ApiResult {
    work(move || {
        let _lock = app.launch_lock.lock().unwrap();
        let original = app.store.review_comment(&id)?;
        ensure!(
            !original.deleted && original.revision == request.expected_revision,
            "Комментарий уже изменён или удалён"
        );
        visible_project(&app, &original.project_id)?;
        let result = current_capture(&app, &original, &mut CaptureCache::new())
            .and_then(|current| matched_anchor(&original, current));
        let _ = app
            .store
            .mark_review_comment_stale(&id, original.revision, result.is_err());
        let (snapshot, anchor) = result?;
        if snapshot == original.snapshot {
            return Ok(serde_json::to_value(app.store.review_comment(&id)?)?);
        }
        let captured = capture_comment(
            &app,
            &original.project_id,
            CreateComment {
                root_id: original.root_id.clone(),
                path: original.path.clone(),
                source: original.source.clone(),
                snapshot,
                anchor,
                text: original.text.clone(),
            },
        )?;
        ensure!(
            captured.capture_fingerprint == original.capture_fingerprint
                && captured.excerpt == original.excerpt,
            "Фрагмент изменился. Создайте новый комментарий к текущему коду"
        );
        let updated = app.store.replace_review_comment(&original, captured)?;
        Ok(serde_json::to_value(updated)?)
    })
    .await
}

#[cfg(test)]
mod fragment_tests {
    use super::*;
    use mpac_workspace::review_lines::{LineKind, ReviewBlock, ReviewLine, ReviewLines};
    fn fixture() -> (ReviewComment, CurrentCapture) {
        let lines = vec![
            ReviewLine {
                index: 0,
                hunk: 1,
                old_line: Some(43),
                new_line: None,
                kind: LineKind::Deletion,
                text: "before".into(),
            },
            ReviewLine {
                index: 1,
                hunk: 1,
                old_line: None,
                new_line: Some(43),
                kind: LineKind::Addition,
                text: "after".into(),
            },
            ReviewLine {
                index: 2,
                hunk: 1,
                old_line: None,
                new_line: Some(44),
                kind: LineKind::Addition,
                text: "    indented".into(),
            },
        ];
        let (excerpt, a, b, c, d) = range_excerpt(&lines, 0, 2, AnchorSide::Both).unwrap();
        let note = ReviewComment {
            id: "note".into(),
            project_id: "project".into(),
            root_id: "root".into(),
            path: "note.rs".into(),
            source: CommentSource::Diff,
            snapshot: "old-global-snapshot".into(),
            anchor: CommentAnchor::Range {
                block_id: Some(0),
                start_row: 0,
                end_row: 2,
                side: AnchorSide::Both,
            },
            capture_fingerprint: mpac_workspace::review_lines::review_text_fingerprint(&excerpt),
            diff_section: Some("working".into()),
            root_identity: "dev:inode".into(),
            excerpt,
            excerpt_truncated: false,
            old_start: a,
            old_end: b,
            new_start: c,
            new_end: d,
            text: "all selected lines".into(),
            revision: 1,
            created_at: 1,
            updated_at: 1,
            deleted: false,
            stale: false,
            validation_state: "unchecked".into(),
            deliveries: vec![],
        };
        let capture = CurrentCapture {
            identity: "dev:inode".into(),
            value: Capture::Diff(ReviewLines {
                snapshot: "new-global-snapshot-after-own-upload".into(),
                head: "samehead".into(),
                truncated: false,
                blocks: vec![ReviewBlock {
                    id: 9,
                    path: "note.rs".into(),
                    section: "working".into(),
                    lines,
                    lines_truncated: false,
                    raw: "full patch".into(),
                }],
            }),
        };
        (note, capture)
    }
    #[test]
    fn unrelated_files_and_changed_block_ids_do_not_invalidate_exact_fragment() {
        let (note, mut capture) = fixture();
        let (snapshot, anchor) = matched_anchor(&note, &capture).unwrap();
        assert_ne!(snapshot, note.snapshot);
        assert!(matches!(
            anchor,
            CommentAnchor::Range {
                block_id: Some(9),
                ..
            }
        ));
        if let Capture::Diff(view) = &mut capture.value {
            view.blocks[0].lines[2].text = "changed".into();
        }
        assert!(matched_anchor(&note, &capture).is_err());
    }
    #[test]
    fn section_identity_and_unseen_whole_file_tail_fail_closed() {
        let (mut note, mut capture) = fixture();
        if let Capture::Diff(view) = &mut capture.value {
            view.blocks[0].section = "staged".into();
        }
        assert!(matched_anchor(&note, &capture).is_err());
        if let Capture::Diff(view) = &mut capture.value {
            view.blocks[0].section = "working".into();
        }
        capture.identity = "replacement inode".into();
        assert!(matched_anchor(&note, &capture).is_err());
        capture.identity = note.root_identity.clone();
        note.anchor = CommentAnchor::File;
        note.excerpt_truncated = true;
        note.excerpt = "full".into();
        note.capture_fingerprint =
            mpac_workspace::review_lines::review_text_fingerprint("full patch");
        assert!(matched_anchor(&note, &capture).is_ok());
        if let Capture::Diff(view) = &mut capture.value {
            view.blocks[0].raw = "full patch with modified unseen tail".into();
        }
        assert!(matched_anchor(&note, &capture).is_err());
    }
}
