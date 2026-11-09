use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path as RoutePath, Query, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer};

use crate::{
    model::{self, Project, Session},
    pty::LiveSession,
    store::{Store, same_project},
};

pub struct App {
    pub store: Arc<Store>,
    pub token: String,
    pub host: String,
    pub live: Mutex<HashMap<String, Arc<LiveSession>>>,
    pub(crate) launch_lock: Mutex<()>,
    pub(crate) activity_channels: Mutex<HashMap<String, crate::activity_hooks::Channel>>,
    pub(crate) activity_helper: Mutex<Option<PathBuf>>,
    pub(crate) terminal_slots: Arc<tokio::sync::Semaphore>,
    pub(crate) shutting_down: AtomicBool,
}

impl App {
    pub fn new(store: Arc<Store>, token: String, host: String) -> Self {
        Self {
            store,
            token,
            host,
            live: Mutex::new(HashMap::new()),
            launch_lock: Mutex::new(()),
            activity_channels: Mutex::new(HashMap::new()),
            activity_helper: Mutex::new(None),
            terminal_slots: Arc::new(tokio::sync::Semaphore::new(64)),
            shutting_down: AtomicBool::new(false),
        }
    }

    pub(crate) fn root(&self, project_id: &str, root_id: &str) -> Result<mpac_workspace::Root> {
        let root = self
            .store
            .projects()?
            .into_iter()
            .find(|p| p.id == project_id)
            .context("Проект не найден")?
            .roots
            .into_iter()
            .find(|r| r.id == root_id)
            .context("Папка не принадлежит проекту")?;
        let canonical = Path::new(&root.path)
            .canonicalize()
            .context("Папка проекта недоступна")?;
        if canonical.to_str() != Some(&root.path) {
            bail!("Путь проекта изменился; добавьте проект заново");
        }
        Ok(root)
    }

    pub(crate) fn live_session(&self, id: &str) -> Result<Arc<LiveSession>> {
        let (meta, archived) = self.store.session_entry(id)?;
        if archived {
            bail!("Сессия удалена; сначала восстановите её");
        }
        if !self
            .store
            .projects()?
            .iter()
            .any(|project| project.id == meta.project_id)
        {
            bail!("Проект сессии удалён; сначала восстановите его");
        }
        self.live
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .context("Исполнение недоступно: сессия завершена или daemon перезапущен")
    }

    fn active_project_sessions(&self, id: &str) -> Result<Vec<Session>> {
        let persisted = self.store.active_sessions()?;
        let live = self
            .live
            .lock()
            .unwrap()
            .values()
            .map(|session| session.meta())
            .collect();
        Ok(merge_active_project_sessions(id, persisted, live))
    }

    pub fn stop_all(&self) {
        self.shutting_down.store(true, Ordering::Release);
        for session in self.live.lock().unwrap().values() {
            session.stop();
        }
    }
}

pub fn router(app: Arc<App>, ui_dir: &Path) -> Router {
    let api = Router::new()
        .route("/state", get(state))
        .route("/directories", get(directories))
        .route("/environment-files", get(environment_files))
        .route("/projects", post(add_project))
        .route("/projects/{id}/update", post(update_project))
        .route("/projects/{id}/delete", post(delete_project))
        .route("/projects/{id}/restore", post(restore_project))
        .route("/projects/{id}/files", get(files))
        .route("/projects/{id}/file", get(file))
        .route("/projects/{id}/review", get(review))
        .route("/projects/{id}/branches", get(branches))
        .route("/projects/{id}/branch", post(switch_branch))
        .route("/sessions", post(start_session))
        .route("/launch-defaults", post(save_launch_defaults))
        .route("/sessions/{id}/output", get(output))
        .route("/sessions/{id}/answer", get(agent_answer))
        .route("/sessions/{id}/input", post(input))
        .route("/sessions/{id}/resize", post(resize))
        .route("/sessions/{id}/stop", post(stop))
        .route("/sessions/{id}/delete", post(delete_session))
        .route("/sessions/{id}/restore", post(restore_session))
        .route("/sessions/{id}/rename", post(rename_session))
        .route(
            "/sessions/{id}/activity/read",
            post(crate::activity_hooks::mark_read),
        )
        .route(
            "/sessions/{id}/attachments",
            post(upload_attachment)
                .layer(DefaultBodyLimit::max(crate::attachments::MAX_JSON_BYTES)),
        )
        .route("/sessions/{id}/review", post(send_review))
        .merge(crate::review_routes::routes())
        .merge(crate::commit_routes::routes())
        .layer(DefaultBodyLimit::max(64 * 1024))
        .route_layer(middleware::from_fn_with_state(app.clone(), authorize));
    Router::new()
        .merge(crate::terminal_stream::routes())
        .merge(crate::activity_hooks::routes())
        .nest("/api", api)
        .fallback_service(ServeDir::new(ui_dir))
        .layer(SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-store")))
        .layer(SetResponseHeaderLayer::overriding(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer")))
        .layer(SetResponseHeaderLayer::overriding(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::overriding(header::CONTENT_SECURITY_POLICY, HeaderValue::from_str(&format!("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self' ws://{}; font-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'", app.host)).expect("local service CSP")))
        .with_state(app)
}

async fn authorize(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let expected_origin = format!("http://{}", app.host);
    if host != Some(&app.host) || origin.is_some_and(|v| v != expected_origin) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Недопустимый Host или Origin"})),
        )
            .into_response();
    }
    let actual = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let matches = actual.len() == app.token.len()
        && actual
            .bytes()
            .zip(app.token.bytes())
            .fold(0_u8, |a, (x, y)| a | (x ^ y))
            == 0;
    if !matches {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Требуется токен локального приложения"})),
        )
            .into_response();
    }
    next.run(request).await
}

pub struct ApiError(anyhow::Error);
impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self(error)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Top-level context only; never expose command stderr/account data.
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":self.0.to_string()})),
        )
            .into_response()
    }
}
pub(crate) type ApiResult = std::result::Result<Json<Value>, ApiError>;

#[cfg(all(test, unix))]
#[path = "api_session_tests.rs"]
mod session_tests;

#[derive(Deserialize)]
struct EnvironmentFilesQuery {
    path: Option<String>,
}
async fn environment_files(
    State(_app): State<Arc<App>>,
    Query(query): Query<EnvironmentFilesQuery>,
) -> ApiResult {
    work(move || {
        Ok(serde_json::to_value(crate::environment::list_files(
            query.path.as_deref(),
        )?)?)
    })
    .await
}

#[derive(Deserialize)]
struct DirectoryQuery {
    path: Option<String>,
    #[serde(default)]
    show_hidden: bool,
}

async fn directories(Query(query): Query<DirectoryQuery>) -> ApiResult {
    work(move || {
        Ok(serde_json::to_value(crate::directories::list_directories(
            query.path.as_deref(),
            query.show_hidden,
        )?)?)
    })
    .await
}

pub(crate) async fn work<F>(f: F) -> ApiResult
where
    F: FnOnce() -> Result<Value> + Send + 'static,
{
    Ok(Json(tokio::task::spawn_blocking(f).await.map_err(
        |_| anyhow::anyhow!("Ошибка локальной задачи"),
    )??))
}

async fn state(State(app): State<Arc<App>>) -> ApiResult {
    work(move || {
        let _catalog = app.launch_lock.lock().unwrap();
        let (projects, archived_projects) = app.store.project_catalog()?;
        let archived_sessions=app.store.archived_sessions()?;
        let launch_defaults=app.store.launch_defaults()?;
        let visible: HashSet<_> = projects.iter().map(|project| project.id.as_str()).collect();
        let live = app.live.lock().unwrap();
        let mut sessions: Vec<_> = app.store.visible_sessions()?.into_iter().map(|s| live.get(&s.id).map_or(s.clone(), |v| v.meta())).collect();
        // Recent-history pagination must not hide a long-running execution.
        for execution in live.values() {
            let current = execution.meta();
            if current.active() && visible.contains(current.project_id.as_str()) && !app.store.session_archived(&current.id)? && !sessions.iter().any(|s| s.id == current.id) { sessions.push(current); }
        }
        sessions.sort_by_key(|s| std::cmp::Reverse(s.started_at));
        Ok(json!({"projects":projects,"archived_projects":archived_projects,"sessions":sessions,"archived_sessions":archived_sessions,"launch_defaults":launch_defaults,"agents":model::agents(),"terminal_launches":model::terminal_launchers(),"prototype":true,"host":"local","capabilities":{"pty":"macOS/Linux","semantic_agent_status":true,"session_activity":true,"agent_answers":true,"terminal_stream":true,"session_env_files":true,"terminal_replay":"bounded_raw_tail","mobile_remote":false,"project_management":true,"session_management":true,"session_rename":true,"git_branches":true,"launch_defaults":true,"auto_session_titles":true,"git_commits":true,"review_comments":true,"session_attachments":cfg!(unix),"archived_sessions_limit":200,"attachment_file_bytes":crate::attachments::MAX_FILE_BYTES,"attachment_session_bytes":crate::attachments::MAX_SESSION_BYTES,"attachment_session_files":crate::attachments::MAX_SESSION_FILES}}))
    }).await
}

#[derive(Deserialize)]
struct AddProject {
    name: String,
    paths: Vec<String>,
}

fn merge_active_project_sessions(
    project_id: &str,
    persisted: Vec<Session>,
    live: Vec<Session>,
) -> Vec<Session> {
    let mut sessions: HashMap<_, _> = persisted
        .into_iter()
        .filter(|s| s.project_id == project_id)
        .map(|s| (s.id.clone(), s))
        .collect();
    // Live state is authoritative when persisted status has not caught up with
    // completion yet, and includes active executions beyond recent history.
    for session in live {
        if session.project_id == project_id {
            sessions.insert(session.id.clone(), session);
        }
    }
    sessions.into_values().filter(Session::active).collect()
}

fn project_roots(
    paths: Vec<String>,
    previous: &[mpac_workspace::Root],
) -> Result<Vec<mpac_workspace::Root>> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    for requested in paths {
        // Exact existing identities can remain selected even while offline.
        // Renaming a project must not require remounting every selected drive.
        let root = if let Some(root) = previous.iter().find(|root| root.path == requested) {
            root.clone()
        } else {
            let path = PathBuf::from(requested);
            if !path.is_absolute() {
                bail!("Укажите абсолютный путь к папке");
            }
            let path = path
                .canonicalize()
                .context("Папка не найдена или недоступна")?;
            if !path.is_dir() {
                bail!("Путь должен указывать на папку");
            }
            let path_str = path
                .to_str()
                .context("Путь должен поддерживать UTF-8")?
                .to_owned();
            if let Some(root) = previous.iter().find(|root| root.path == path_str) {
                root.clone()
            } else {
                mpac_workspace::Root {
                    id: model::id(),
                    name: path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path_str.clone()),
                    path: path_str,
                }
            }
        };
        if !seen.insert(root.path.clone()) {
            bail!("Папка указана дважды");
        }
        roots.push(root);
    }
    Ok(roots)
}

fn validate_project_fields(name: &str, paths: &[String]) -> Result<()> {
    if name.trim().is_empty() || name.len() > 120 || paths.is_empty() || paths.len() > 12 {
        bail!("Нужно название проекта и от 1 до 12 абсолютных путей");
    }
    Ok(())
}

async fn add_project(State(app): State<Arc<App>>, Json(request): Json<AddProject>) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        validate_project_fields(&request.name, &request.paths)?;
        if app.store.projects()?.len() >= 100 {
            bail!("Лимит прототипа: 100 проектов");
        }
        let roots = project_roots(request.paths, &[])?;
        let project = Project {
            id: model::id(),
            name: request.name.trim().into(),
            roots,
        };
        app.store.project(&project)?;
        Ok(serde_json::to_value(project)?)
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateProject {
    name: String,
    paths: Vec<String>,
    expected: Project,
}

async fn update_project(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<UpdateProject>,
) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        if request.expected.id != id { bail!("Снимок относится к другому проекту"); }
        let current = app.store.projects()?.into_iter().find(|project| project.id == id).context("Проект не найден или удалён")?;
        if !same_project(&current, &request.expected) { bail!("Проект уже изменён. Обновите данные перед сохранением"); }
        validate_project_fields(&request.name, &request.paths)?;
        let roots = project_roots(request.paths, &current.roots)?;
        let retained: HashSet<_> = roots.iter().map(|root| root.id.as_str()).collect();
        let removed: HashSet<_> = current.roots.iter().filter(|root| !retained.contains(root.id.as_str())).map(|root| root.id.as_str()).collect();
        for session in app.active_project_sessions(&id)? {
            if removed.contains(session.root_id.as_str())
                || session.additional_roots.iter().any(|root| removed.contains(root.id.as_str()))
                || session.access.additional_root_ids.iter().any(|root| removed.contains(root.as_str())) {
                bail!("Нельзя убрать папку, которую использует работающая сессия. Сначала остановите эту сессию");
            }
        }
        let updated = Project { id, name: request.name.trim().to_owned(), roots };
        app.store.update_project(&request.expected, &updated)?;
        Ok(serde_json::to_value(updated)?)
    }).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteProject {
    expected: Project,
}

async fn delete_project(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<DeleteProject>,
) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        if request.expected.id != id {
            bail!("Снимок относится к другому проекту");
        }
        if !app.active_project_sessions(&id)?.is_empty() {
            bail!("В проекте есть работающие сессии. Сначала остановите их");
        }
        app.store.archive_project(&request.expected)?;
        Ok(json!({"archived":true,"project_id":id}))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreProject {}

async fn restore_project(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(_request): Json<RestoreProject>,
) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        Ok(serde_json::to_value(app.store.restore_project(&id)?)?)
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartSession {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    env_file: Option<String>,
    project_id: String,
    root_id: String,
    agent: String,
    title: Option<String>,
    #[serde(default)]
    access: Option<crate::access::AccessSettings>,
}

async fn save_launch_defaults(
    State(app): State<Arc<App>>,
    Json(request): Json<crate::launch_defaults::SaveDefaults>,
) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        Ok(serde_json::to_value(
            app.store.save_launch_defaults(&request)?,
        )?)
    })
    .await
}

async fn start_session(
    State(app): State<Arc<App>>,
    Json(request): Json<StartSession>,
) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        if !model::agents()
            .iter()
            .chain(model::terminal_launchers().iter())
            .any(|a| a.id == request.agent && a.available)
        {
            bail!("Агент не установлен или профиль недоступен");
        }
        let terminal_launch =
            crate::terminal_launch::prepare(&request.agent, request.command.as_deref())?;
        let root = app.root(&request.project_id, &request.root_id)?;
        let access = match request.access {
            Some(access) => access,
            None if terminal_launch.is_some() => Default::default(),
            None => app.store.launch_defaults()?.resolve(
                &request.project_id,
                &request.agent,
                &request.root_id,
            ),
        };
        let cwd = PathBuf::from(&root.path)
            .canonicalize()
            .context("Папка проекта недоступна")?;
        if cwd.to_str() != Some(&root.path) {
            bail!("Путь проекта изменился; добавьте проект заново");
        }
        let environment = request
            .env_file
            .as_deref()
            .map(|file| {
                if request.agent == "demo" {
                    bail!("Демо-агент не принимает ENV-файл");
                }
                crate::environment::Environment::load(file, &cwd)
            })
            .transpose()?;
        let mut additional_roots = Vec::new();
        let mut physical_paths = std::collections::HashSet::from([root.path.clone()]);
        let mut ids = std::collections::HashSet::new();
        if access.additional_root_ids.len() > 12 {
            bail!("Слишком много дополнительных папок");
        }
        for id in &access.additional_root_ids {
            if id == &request.root_id || !ids.insert(id) {
                bail!("Дополнительные папки не должны повторяться или совпадать с рабочей папкой");
            }
            let extra = app.root(&request.project_id, id)?;
            if !physical_paths.insert(extra.path.clone()) {
                bail!("Одна физическая папка указана дважды");
            }
            additional_roots.push(extra);
        }
        let paths: Vec<_> = additional_roots
            .iter()
            .map(|root| root.path.clone())
            .collect();
        let session_id = model::id();
        let permission_args =
            crate::access::launch_args_for_session(&request.agent, &access, &paths, &session_id)?;
        let mut live = app.live.lock().unwrap();
        if live.values().filter(|s| s.meta().active()).count() >= 16 {
            bail!("Лимит прототипа: 16 работающих сессий");
        }
        if live.len() >= 80 {
            let oldest = live
                .iter()
                .filter(|(_, s)| !s.meta().active())
                .min_by_key(|(_, s)| s.meta().started_at)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                live.remove(&id);
                crate::activity_hooks::forget(&app, &id);
            }
        }
        let (title, title_source, auto_title_eligible) = if terminal_launch.is_some() {
            (
                model::session_title(
                    request
                        .title
                        .as_deref()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or(if request.agent == "shell" {
                            "Terminal"
                        } else {
                            "Command"
                        }),
                )?,
                model::SessionTitleSource::Manual,
                false,
            )
        } else {
            model::initial_session_title(&request.agent, request.title.as_deref())?
        };
        let mut session = Session {
            id: session_id,
            project_id: request.project_id,
            root_id: request.root_id,
            agent: request.agent,
            title,
            status: "running".into(),
            started_at: model::now(),
            last_activity: model::now(),
            exit_code: None,
            error: None,
            seq: 0,
            mode: "interactive_pty".into(),
            access,
            additional_roots,
            permission_args,
            cli_title: None,
            title_source,
            auto_title_eligible,
            activity: Default::default(),
            environment: environment.as_ref().map(|env| env.metadata()),
            terminal_launch,
        };
        let instrumentation = crate::activity_hooks::prepare(&app, &mut session)?;
        let id = session.id.clone();
        let execution = match LiveSession::spawn_with_environment(
            session,
            &cwd,
            app.store.clone(),
            instrumentation.as_ref(),
            environment.as_ref(),
        ) {
            Ok(execution) => execution,
            Err(error) => {
                crate::activity_hooks::forget(&app, &id);
                return Err(error);
            }
        };
        let session = execution.meta();
        live.insert(session.id.clone(), execution);
        Ok(serde_json::to_value(session)?)
    })
    .await
}

#[derive(Deserialize)]
struct After {
    #[serde(default)]
    after: u64,
}
async fn agent_answer(State(app): State<Arc<App>>, RoutePath(id): RoutePath<String>) -> ApiResult {
    work(move || Ok(json!({"answer": app.store.agent_answer(&id)?}))).await
}

async fn output(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<After>,
) -> ApiResult {
    let session = app.live_session(&id)?;
    Ok(Json(
        serde_json::to_value(session.output(query.after)).map_err(anyhow::Error::from)?,
    ))
}

#[derive(Deserialize)]
struct Input {
    data: String,
}
async fn input(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<Input>,
) -> ApiResult {
    app.live_session(&id)?.input(request.data)?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
struct Resize {
    cols: u16,
    rows: u16,
}
async fn resize(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<Resize>,
) -> ApiResult {
    app.live_session(&id)?.resize(request.cols, request.rows)?;
    Ok(Json(json!({})))
}
async fn stop(State(app): State<Arc<App>>, RoutePath(id): RoutePath<String>) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        app.live_session(&id)?.stop();
        Ok(json!({}))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionAction {}

async fn delete_session(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(_request): Json<SessionAction>,
) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        let (stored, _) = app.store.session_entry(&id)?;
        let current = app
            .live
            .lock()
            .unwrap()
            .get(&id)
            .map(|live| live.meta())
            .unwrap_or(stored);
        if current.active() {
            bail!("Работающую сессию нельзя удалить. Сначала явно остановите её");
        }
        // Preserve the canonical final state for Undo even if a caller's modal
        // was opened while the execution was still running.
        app.store.session(&current)?;
        app.store.archive_session(&id)?;
        Ok(json!({"archived":true,"session_id":id}))
    })
    .await
}

async fn restore_session(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(_request): Json<SessionAction>,
) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        if app
            .live
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|live| live.meta().active())
        {
            bail!("Работающую сессию нельзя восстанавливать или переигрывать");
        }
        let session = app.store.restore_session(&id)?;
        Ok(json!({"archived":false,"session_id":id,"session":session}))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameSession {
    title: String,
    expected_title: String,
}
async fn rename_session(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<RenameSession>,
) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        let live = app.live.lock().unwrap().get(&id).cloned();
        let renamed = if let Some(live) = live {
            live.rename(&app.store, &request.title, &request.expected_title)?
        } else {
            app.store
                .rename_session(&id, &request.title, &request.expected_title, None)?
        };
        Ok(serde_json::to_value(renamed)?)
    })
    .await
}

fn same_physical_directory(first: &Path, second: &Path) -> bool {
    if first == second {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (first.metadata(), second.metadata()) {
            return a.dev() == b.dev() && a.ino() == b.ino();
        }
    }
    false
}

impl App {
    fn branch_blocker(&self, target: &Path) -> Result<Option<String>> {
        let mut sessions: HashMap<_, _> = self
            .store
            .active_sessions()?
            .into_iter()
            .map(|s| (s.id.clone(), s))
            .collect();
        let mut frozen = HashMap::new();
        for live in self.live.lock().unwrap().values() {
            let meta = live.meta();
            frozen.insert(meta.id.clone(), live.execution_paths());
            sessions.insert(meta.id.clone(), meta);
        }
        let (mut projects, archived) = self.store.project_catalog()?;
        projects.extend(archived);
        let target = target.canonicalize().context("Папка проекта недоступна")?;
        let mut checked = HashSet::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        for session in sessions.into_values().filter(Session::active) {
            let paths = if let Some(paths) = frozen.get(&session.id) {
                paths.clone()
            } else {
                let project = projects
                    .iter()
                    .find(|p| p.id == session.project_id)
                    .context("Не удалось проверить проект работающей сессии")?;
                let main = project
                    .roots
                    .iter()
                    .find(|r| r.id == session.root_id)
                    .context("Не удалось проверить папку работающей сессии")?;
                let mut paths = vec![main.path.clone()];
                paths.extend(
                    session
                        .additional_roots
                        .iter()
                        .map(|root| root.path.clone()),
                );
                for id in &session.access.additional_root_ids {
                    if let Some(root) = project.roots.iter().find(|root| &root.id == id) {
                        paths.push(root.path.clone());
                    }
                }
                paths
            };
            for path in paths {
                if !checked.insert(path.clone()) {
                    continue;
                }
                let original = PathBuf::from(&path);
                if original == target {
                    return Ok(Some(
                        "В этой рабочей папке запущена CLI-сессия. Сначала остановите её".into(),
                    ));
                }
                let physical = match original.canonicalize() {
                    Ok(path) => path,
                    Err(_) => {
                        if original.starts_with(&target) {
                            return Ok(Some(
                                "Папка работающей сессии недоступна. Сначала остановите её".into(),
                            ));
                        }
                        continue;
                    }
                };
                if same_physical_directory(&physical, &target) {
                    return Ok(Some(
                        "В этой рабочей папке запущена CLI-сессия. Сначала остановите её".into(),
                    ));
                }
                if target.starts_with(&physical) {
                    return Ok(Some("Работающая CLI-сессия имеет доступ к родительской папке этого репозитория. Сначала остановите её".into()));
                }
                if std::time::Instant::now() >= deadline {
                    return Ok(Some("Проверка работающих сессий заняла слишком много времени. Повторите после их остановки".into()));
                }
                let repo = mpac_workspace::repository_root(&physical)
                    .context("Не удалось проверить Git-папку работающей сессии")?;
                if repo
                    .as_ref()
                    .is_some_and(|root| same_physical_directory(root, &target))
                {
                    return Ok(Some("Работающая CLI-сессия использует этот репозиторий, включая дополнительную папку или другой проект. Сначала остановите её".into()));
                }
            }
        }
        Ok(None)
    }
}

async fn branches(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<FileQuery>,
) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        let root = app.root(&id, &query.root_id)?;
        let mut state = mpac_workspace::branch_state(Path::new(&root.path))?;
        if state.is_repo && state.blocked_reason.is_none() {
            state.blocked_reason = app.branch_blocker(Path::new(&root.path))?;
        }
        Ok(serde_json::to_value(state)?)
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SwitchBranch {
    root_id: String,
    branch: String,
    expected_head: String,
    expected_branch: Option<String>,
}
async fn switch_branch(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<SwitchBranch>,
) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        let root = app.root(&id, &request.root_id)?;
        if let Some(reason) = app.branch_blocker(Path::new(&root.path))? {
            bail!("{reason}");
        }
        Ok(serde_json::to_value(mpac_workspace::switch_branch(
            Path::new(&root.path),
            &request.branch,
            &request.expected_head,
            request.expected_branch.as_deref(),
        )?)?)
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadAttachment {
    name: String,
    data_base64: String,
}

async fn upload_attachment(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<UploadAttachment>,
) -> ApiResult {
    work(move || {
        let _launch = app.launch_lock.lock().unwrap();
        let live = app.live_session(&id)?;
        let (name, bytes) = crate::attachments::decode_upload(&request.name, &request.data_base64)?;
        live.with_active(|meta| {
            let root = app.root(&meta.project_id, &meta.root_id)?;
            live.verify_root(&meta.root_id, Path::new(&root.path))?;
            let (count, total) = app.store.attachment_totals(&id)?;
            if count >= crate::attachments::MAX_SESSION_FILES
                || total
                    .checked_add(bytes.len() as u64)
                    .is_none_or(|sum| sum > crate::attachments::MAX_SESSION_BYTES)
            {
                bail!("Лимит сессии: 50 вложений и 100 MiB загруженных файлов");
            }
            let pending = crate::attachments::prepare(
                Path::new(&root.path),
                &id,
                live.working_root_identity()?,
                &name,
                &bytes,
            )?;
            live.verify_root(&meta.root_id, Path::new(&root.path))?;
            pending.verify_path()?;
            live.check_upload_active()?;
            app.store.record_attachment(meta, &pending.attachment)?;
            Ok(serde_json::to_value(pending.commit())?)
        })
    })
    .await
}

#[derive(Deserialize)]
struct FileQuery {
    root_id: String,
    #[serde(default)]
    path: String,
}
async fn files(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<FileQuery>,
) -> ApiResult {
    work(move || {
        let root = app.root(&id, &query.root_id)?;
        Ok(json!({"files":mpac_workspace::list_files(Path::new(&root.path), &query.path).context("Не удалось прочитать папку внутри проекта")?}))
    }).await
}
async fn file(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<FileQuery>,
) -> ApiResult {
    work(move || {
        let root = app.root(&id, &query.root_id)?;
        Ok(json!({"path":query.path,"content":mpac_workspace::read_text(Path::new(&root.path), &query.path).context("Текстовый файл недоступен, слишком велик или выходит за границы проекта")?}))
    }).await
}
async fn review(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<FileQuery>,
) -> ApiResult {
    work(move || {
        let root = app.root(&id, &query.root_id)?;
        Ok(serde_json::to_value(
            mpac_workspace::git_review(Path::new(&root.path))
                .context("Не удалось получить Git review этой папки")?,
        )?)
    })
    .await
}

#[derive(Deserialize)]
struct ReviewInput {
    root_id: String,
    snapshot: String,
    comments: String,
}
async fn send_review(
    State(app): State<Arc<App>>,
    RoutePath(id): RoutePath<String>,
    Json(request): Json<ReviewInput>,
) -> ApiResult {
    work(move || {
        if request.comments.trim().is_empty() || request.comments.len() > 10_000 {
            bail!("Нужно замечание длиной до 10 000 байт");
        }
        let session = app.live_session(&id)?;
        let meta = session.meta();
        if meta.root_id != request.root_id
            && !meta
                .additional_roots
                .iter()
                .any(|root| root.id == request.root_id)
        {
            bail!("Review относится к другой папке; выберите сессию этой папки");
        }
        let root = app.root(&meta.project_id, &request.root_id)?;
        session.verify_root(&request.root_id, Path::new(&root.path))?;
        mpac_workspace::validate_review(Path::new(&root.path), &request.snapshot)
            .context("Изменения обновились; перечитайте diff перед отправкой замечаний")?;
        session.verify_root(&request.root_id, Path::new(&root.path))?;
        // A single-line draft has no CR/LF/ESC that could submit commands in a
        // CLI without bracketed-paste support. User explicitly presses Enter.
        let note = format!(
            "Review {} [{}]: {}",
            root.name, request.snapshot, request.comments
        );
        let note: String = note
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        session.input(note)?;
        Ok(json!({"submitted":false,"delivery":"terminal_paste","requires_enter":true}))
    })
    .await
}

#[cfg(test)]
mod project_guard_tests {
    use super::*;

    fn session(id: &str, project_id: &str, status: &str) -> Session {
        Session {
            id: id.into(),
            project_id: project_id.into(),
            root_id: "main".into(),
            agent: "demo".into(),
            title: "guard fixture".into(),
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
            title_source: crate::model::SessionTitleSource::Manual,
            auto_title_eligible: false,
            activity: Default::default(),
            environment: None,
            terminal_launch: None,
        }
    }

    #[test]
    fn live_completion_overrides_stale_persisted_active_status_and_foreign_projects_do_not_block() {
        let merged = merge_active_project_sessions(
            "selected",
            vec![session("stale", "selected", "running")],
            vec![
                session("stale", "selected", "exited"),
                session("foreign", "another", "running"),
            ],
        );
        assert!(merged.is_empty());
    }

    #[test]
    fn all_live_active_executions_and_their_frozen_additional_roots_are_included() {
        let mut live: Vec<_> = (0..205)
            .map(|n| session(&format!("finished-{n}"), "selected", "exited"))
            .collect();
        let mut active = session("live-active", "selected", "needs_attention");
        active.additional_roots.push(mpac_workspace::Root {
            id: "extra".into(),
            name: "Extra".into(),
            path: "frozen path".into(),
        });
        active.access.additional_root_ids.push("extra".into());
        live.insert(0, active);
        let merged = merge_active_project_sessions("selected", vec![], live);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "live-active");
        assert_eq!(merged[0].additional_roots[0].id, "extra");
        assert_eq!(merged[0].access.additional_root_ids, vec!["extra"]);
    }
}
