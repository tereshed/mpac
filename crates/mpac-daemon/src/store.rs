use std::{fs, path::Path, sync::Mutex};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    attachments::{Attachment, MAX_SESSION_BYTES, MAX_SESSION_FILES},
    launch_defaults::{LaunchDefaults, SaveDefaults},
    model::{Project, Session},
    review_comments::{BatchStatus, Delivery, ReviewBatch, ReviewComment},
};

pub struct Store(Mutex<Connection>);

#[derive(serde::Serialize)]
pub(crate) struct AgentAnswer {
    text: String,
    saved_at: u64,
}

fn sql_unsigned(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

fn read_defaults(connection: &Connection) -> Result<LaunchDefaults> {
    let payload: Option<String> = connection
        .query_row(
            "SELECT payload FROM launch_defaults WHERE id=1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(payload
        .map(|s| serde_json::from_str::<LaunchDefaults>(&s))
        .transpose()?
        .unwrap_or_default()
        .without_acknowledgements())
}

fn write_defaults(connection: &Connection, defaults: &LaunchDefaults) -> Result<()> {
    let defaults = defaults.clone().without_acknowledgements();
    connection.execute("INSERT INTO launch_defaults(id,payload) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", [serde_json::to_string(&defaults)?])?;
    Ok(())
}

impl Store {
    pub(crate) fn data_dir(&self) -> Result<std::path::PathBuf> {
        let connection = self.0.lock().unwrap();
        let path = connection.path().context("Нет пути локальной базы")?;
        let parent = Path::new(path)
            .parent()
            .context("Нет папки локальной базы")?;
        Ok(parent.canonicalize()?)
    }
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY, payload TEXT NOT NULL); CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, payload TEXT NOT NULL);")?;
        for table in ["projects", "sessions"] {
            let has_archived = {
                let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
                let names = statement.query_map([], |row| row.get::<_, String>(1))?;
                names
                    .collect::<rusqlite::Result<Vec<_>>>()?
                    .iter()
                    .any(|name| name == "archived")
            };
            if !has_archived {
                connection.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN archived INTEGER NOT NULL DEFAULT 0"),
                    [],
                )?;
            }
        }
        connection.execute_batch("CREATE TABLE IF NOT EXISTS attachments(id TEXT PRIMARY KEY,session_id TEXT NOT NULL,payload TEXT NOT NULL,size INTEGER NOT NULL CHECK(size>=0)); CREATE INDEX IF NOT EXISTS attachments_session ON attachments(session_id);")?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS launch_defaults(id INTEGER PRIMARY KEY CHECK(id=1), payload TEXT NOT NULL);")?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS agent_answers(session_id TEXT PRIMARY KEY, text TEXT NOT NULL, saved_at INTEGER NOT NULL);")?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS review_comments(id TEXT PRIMARY KEY,project_id TEXT NOT NULL,root_id TEXT NOT NULL,payload TEXT NOT NULL); CREATE INDEX IF NOT EXISTS review_comments_project ON review_comments(project_id,root_id); CREATE TABLE IF NOT EXISTS review_batches(id TEXT PRIMARY KEY,project_id TEXT NOT NULL,request_id TEXT NOT NULL UNIQUE,payload TEXT NOT NULL); CREATE INDEX IF NOT EXISTS review_batches_project ON review_batches(project_id); CREATE TABLE IF NOT EXISTS review_deliveries(comment_id TEXT NOT NULL,revision INTEGER NOT NULL,session_id TEXT NOT NULL,batch_id TEXT NOT NULL,at INTEGER NOT NULL,PRIMARY KEY(comment_id,revision,session_id)); CREATE TABLE IF NOT EXISTS commit_receipts(request_id TEXT PRIMARY KEY,payload TEXT NOT NULL);")?;
        // A process may have died after enqueueing PTY bytes but before recording
        // success. Never replay such a draft automatically after restart.
        connection.execute("UPDATE review_batches SET payload=json_set(payload,'$.status','uncertain') WHERE json_extract(payload,'$.status')='preparing'", [])?;
        Ok(Self(Mutex::new(connection)))
    }

    pub fn launch_defaults(&self) -> Result<LaunchDefaults> {
        read_defaults(&self.0.lock().unwrap())
    }

    pub fn review_comments(&self, project: &str) -> Result<Vec<ReviewComment>> {
        let connection = self.0.lock().unwrap();
        let mut query = connection.prepare("SELECT payload FROM review_comments WHERE project_id=?1 AND json_extract(payload,'$.deleted')=0 ORDER BY rowid")?;
        let rows = query.query_map([project], |row| row.get::<_, String>(0))?;
        let mut comments = Vec::new();
        for row in rows {
            let mut comment: ReviewComment = serde_json::from_str(&row?)?;
            let mut deliveries = connection.prepare("SELECT session_id,batch_id,at FROM review_deliveries WHERE comment_id=?1 AND revision=?2 ORDER BY at")?;
            comment.deliveries = deliveries
                .query_map(
                    params![comment.id, i64::try_from(comment.revision)?],
                    |row| {
                        Ok(Delivery {
                            session_id: row.get(0)?,
                            revision: comment.revision,
                            batch_id: row.get(1)?,
                            at: sql_unsigned(row, 2)?,
                            status: "handed_off".into(),
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            comments.push(comment);
        }
        Ok(comments)
    }

    pub fn review_comment(&self, id: &str) -> Result<ReviewComment> {
        let payload: String = self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM review_comments WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .context("Комментарий не найден")?;
        Ok(serde_json::from_str(&payload)?)
    }

    pub fn create_review_comment(&self, comment: &ReviewComment) -> Result<()> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let visible: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1 AND archived=0)",
            [&comment.project_id],
            |row| row.get(0),
        )?;
        ensure!(visible, "Проект не найден или удалён");
        let (active,total):(i64,i64)=tx.query_row("SELECT coalesce(sum(json_extract(payload,'$.deleted')=0),0),count(*) FROM review_comments WHERE project_id=?1",[&comment.project_id],|row|Ok((row.get(0)?,row.get(1)?)))?;
        ensure!(
            active >= 0 && total >= 0 && active < 500 && total < 5000,
            "Лимит комментариев проекта: 500 открытых и 5000 записей истории"
        );
        tx.execute(
            "INSERT INTO review_comments(id,project_id,root_id,payload) VALUES(?1,?2,?3,?4)",
            params![
                comment.id,
                comment.project_id,
                comment.root_id,
                serde_json::to_string(comment)?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn update_review_comment(
        &self,
        id: &str,
        expected: u64,
        text: Option<&str>,
    ) -> Result<ReviewComment> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let (payload,visible):(String,bool)=tx.query_row("SELECT c.payload,p.archived=0 FROM review_comments c JOIN projects p ON p.id=c.project_id WHERE c.id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?.context("Комментарий или проект не найден")?;
        let mut comment: ReviewComment = serde_json::from_str(&payload)?;
        ensure!(visible, "Проект удалён; сначала восстановите его");
        ensure!(
            !comment.deleted && comment.revision == expected,
            "Комментарий уже изменён или удалён. Обновите данные"
        );
        if let Some(text) = text {
            comment.text = crate::review_comments::clean_note(text)?
        } else {
            comment.deleted = true
        }
        comment.revision = comment
            .revision
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .context("Исчерпан счётчик комментария")?;
        comment.updated_at = crate::model::now();
        comment.deliveries.clear();
        tx.execute(
            "UPDATE review_comments SET payload=?2 WHERE id=?1",
            params![id, serde_json::to_string(&comment)?],
        )?;
        tx.commit()?;
        Ok(comment)
    }

    pub fn replace_review_comment(
        &self,
        expected: &ReviewComment,
        mut captured: ReviewComment,
    ) -> Result<ReviewComment> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let (payload,visible):(String,bool)=tx.query_row("SELECT c.payload,p.archived=0 FROM review_comments c JOIN projects p ON p.id=c.project_id WHERE c.id=?1",[&expected.id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?.context("Комментарий или проект не найден")?;
        let current: ReviewComment = serde_json::from_str(&payload)?;
        ensure!(
            visible && !current.deleted && current.revision == expected.revision,
            "Комментарий или проект уже изменён"
        );
        ensure!(
            captured.project_id == current.project_id
                && captured.root_id == current.root_id
                && captured.path == current.path
                && captured.source == current.source,
            "Область комментария нельзя менять неявно"
        );
        captured.id = current.id;
        captured.created_at = current.created_at;
        captured.revision = current
            .revision
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .context("Исчерпан счётчик комментария")?;
        captured.updated_at = crate::model::now();
        captured.deliveries.clear();
        tx.execute(
            "UPDATE review_comments SET payload=?2 WHERE id=?1",
            params![captured.id, serde_json::to_string(&captured)?],
        )?;
        tx.commit()?;
        Ok(captured)
    }

    pub fn mark_review_comment_stale(&self, id: &str, revision: u64, stale: bool) -> Result<()> {
        self.0.lock().unwrap().execute("UPDATE review_comments SET payload=json_set(payload,'$.stale',json(?3),'$.validation_state',?4) WHERE id=?1 AND json_extract(payload,'$.revision')=?2",params![id,i64::try_from(revision)?,if stale{"true"}else{"false"},if stale{"stale"}else{"checked"}])?;
        Ok(())
    }

    pub fn review_batches(&self, project: &str) -> Result<Vec<ReviewBatch>> {
        let connection = self.0.lock().unwrap();
        let mut query = connection.prepare(
            "SELECT payload FROM review_batches WHERE project_id=?1 ORDER BY rowid DESC LIMIT 100",
        )?;
        query
            .query_map([project], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }

    pub fn review_batch(&self, id: &str) -> Result<ReviewBatch> {
        let payload: String = self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM review_batches WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .context("Пакет ревью не найден")?;
        Ok(serde_json::from_str(&payload)?)
    }

    pub fn review_batch_by_request(
        &self,
        project: &str,
        request: &str,
    ) -> Result<Option<ReviewBatch>> {
        let payload: Option<String> = self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM review_batches WHERE project_id=?1 AND request_id=?2",
                params![project, request],
                |row| row.get(0),
            )
            .optional()?;
        payload.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
    }

    pub fn create_review_batch(&self, batch: &ReviewBatch) -> Result<()> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM review_batches WHERE project_id=?1",
            [&batch.project_id],
            |row| row.get(0),
        )?;
        ensure!(
            (0..5000).contains(&count),
            "Лимит истории: 5000 пакетов ревью на проект"
        );
        tx.execute(
            "INSERT INTO review_batches(id,project_id,request_id,payload) VALUES(?1,?2,?3,?4)",
            params![
                batch.id,
                batch.project_id,
                batch.request_id,
                serde_json::to_string(batch)?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn transition_review_batch(
        &self,
        id: &str,
        expected: BatchStatus,
        next: BatchStatus,
    ) -> Result<ReviewBatch> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let payload: String = tx
            .query_row(
                "SELECT payload FROM review_batches WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .context("Пакет ревью не найден")?;
        let mut batch: ReviewBatch = serde_json::from_str(&payload)?;
        ensure!(
            batch.status == expected,
            "Статус пакета уже изменился. Обновите данные"
        );
        batch.status = next;
        batch.updated_at = crate::model::now();
        tx.execute(
            "UPDATE review_batches SET payload=?2 WHERE id=?1",
            params![id, serde_json::to_string(&batch)?],
        )?;
        tx.commit()?;
        Ok(batch)
    }

    pub fn set_review_batch_attachment(&self, id: &str, path: &str) -> Result<ReviewBatch> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let payload: String = tx
            .query_row(
                "SELECT payload FROM review_batches WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .context("Пакет не найден")?;
        let mut batch: ReviewBatch = serde_json::from_str(&payload)?;
        ensure!(
            batch.status == BatchStatus::Preparing,
            "Пакет уже изменился"
        );
        ensure!(
            batch
                .attachment_path
                .as_ref()
                .is_none_or(|prior| prior == path),
            "Файл пакета уже задан"
        );
        batch.attachment_path = Some(path.into());
        batch.updated_at = crate::model::now();
        tx.execute(
            "UPDATE review_batches SET payload=?2 WHERE id=?1",
            params![id, serde_json::to_string(&batch)?],
        )?;
        tx.commit()?;
        Ok(batch)
    }

    pub fn confirm_review_batch(&self, id: &str) -> Result<ReviewBatch> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let payload: String = tx
            .query_row(
                "SELECT payload FROM review_batches WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .context("Пакет ревью не найден")?;
        let mut batch: ReviewBatch = serde_json::from_str(&payload)?;
        ensure!(
            matches!(
                batch.status,
                BatchStatus::Prepared | BatchStatus::Uncertain | BatchStatus::HandedOff
            ),
            "Сначала вставьте пакет в CLI"
        );
        if batch.status != BatchStatus::HandedOff {
            batch.status = BatchStatus::HandedOff;
            batch.updated_at = crate::model::now();
            for comment in &batch.comments {
                tx.execute("INSERT OR IGNORE INTO review_deliveries(comment_id,revision,session_id,batch_id,at) VALUES(?1,?2,?3,?4,?5)",params![comment.id,i64::try_from(comment.revision)?,batch.session_id,batch.id,i64::try_from(batch.updated_at)?])?;
            }
            tx.execute(
                "UPDATE review_batches SET payload=?2 WHERE id=?1",
                params![id, serde_json::to_string(&batch)?],
            )?;
        }
        tx.commit()?;
        Ok(batch)
    }

    pub fn commit_receipt(&self, request_id: &str) -> Result<Option<serde_json::Value>> {
        let payload: Option<String> = self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM commit_receipts WHERE request_id=?1",
                [request_id],
                |row| row.get(0),
            )
            .optional()?;
        payload.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
    }
    pub fn save_commit_receipt(&self, request_id: &str, value: &serde_json::Value) -> Result<()> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let payload = serde_json::to_string(value)?;
        let prior: Option<String> = tx
            .query_row(
                "SELECT payload FROM commit_receipts WHERE request_id=?1",
                [request_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(prior) = prior {
            let prior: serde_json::Value = serde_json::from_str(&prior)?;
            if prior == *value {
                return Ok(());
            }
            ensure!(
                prior["request"] == value["request"]
                    && prior["status"] == "started"
                    && matches!(value["status"].as_str(), Some("completed" | "failed")),
                "Подтверждение коммита уже завершено или параметры изменились"
            );
            tx.execute(
                "UPDATE commit_receipts SET payload=?2 WHERE request_id=?1",
                params![request_id, payload],
            )?;
        } else {
            ensure!(
                value["status"] == "started",
                "Первое подтверждение коммита должно иметь статус started"
            );
            tx.execute(
                "INSERT INTO commit_receipts(request_id,payload) VALUES(?1,?2)",
                params![request_id, payload],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn save_launch_defaults(&self, request: &SaveDefaults) -> Result<LaunchDefaults> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let current = read_defaults(&tx)?;
        let project: Option<Project> = if let Some(id) = &request.project_id {
            tx.query_row(
                "SELECT payload FROM projects WHERE id=?1 AND archived=0",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|s| serde_json::from_str(&s))
            .transpose()?
        } else {
            None
        };
        let updated = current.updated(request, project.as_ref())?;
        write_defaults(&tx, &updated)?;
        tx.commit()?;
        Ok(updated)
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        self.load("SELECT payload FROM projects WHERE archived=0 ORDER BY rowid")
    }

    pub fn archived_projects(&self) -> Result<Vec<Project>> {
        self.load("SELECT payload FROM projects WHERE archived=1 ORDER BY rowid")
    }

    /// One read snapshot avoids a project appearing in both lists when another
    /// request archives/restores it between two separate catalog queries.
    pub fn project_catalog(&self) -> Result<(Vec<Project>, Vec<Project>)> {
        let connection = self.0.lock().unwrap();
        let mut statement =
            connection.prepare("SELECT payload,archived FROM projects ORDER BY rowid")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })?;
        let (mut active, mut archived) = (Vec::new(), Vec::new());
        for row in rows {
            let (payload, hidden) = row?;
            let project = serde_json::from_str(&payload)?;
            if hidden {
                archived.push(project);
            } else {
                active.push(project);
            }
        }
        Ok((active, archived))
    }

    pub fn has_any_projects(&self) -> Result<bool> {
        Ok(self.0.lock().unwrap().query_row(
            "SELECT EXISTS(SELECT 1 FROM projects)",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn sessions(&self) -> Result<Vec<Session>> {
        self.load("SELECT payload FROM sessions ORDER BY rowid DESC LIMIT 200")
    }

    pub fn visible_sessions(&self) -> Result<Vec<Session>> {
        self.load("SELECT s.payload FROM sessions s JOIN projects p ON p.id=json_extract(s.payload,'$.project_id') WHERE p.archived=0 AND s.archived=0 ORDER BY s.rowid DESC LIMIT 200")
    }

    /// The restore chooser is bounded independently of visible history.
    pub fn archived_sessions(&self) -> Result<Vec<Session>> {
        self.load("SELECT s.payload FROM sessions s JOIN projects p ON p.id=json_extract(s.payload,'$.project_id') WHERE p.archived=0 AND s.archived=1 ORDER BY s.rowid DESC LIMIT 200")
    }

    pub fn session_entry(&self, id: &str) -> Result<(Session, bool)> {
        let (payload, archived): (String, bool) = self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload,archived FROM sessions WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("Сессия не найдена")?;
        Ok((serde_json::from_str(&payload)?, archived))
    }

    pub fn session_archived(&self, id: &str) -> Result<bool> {
        self.0
            .lock()
            .unwrap()
            .query_row("SELECT archived FROM sessions WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()?
            .context("Сессия не найдена")
    }

    /// Mutation guards must include old active sessions beyond history paging.
    pub fn active_sessions(&self) -> Result<Vec<Session>> {
        self.load("SELECT payload FROM sessions WHERE json_extract(payload,'$.status') IN ('running','needs_attention')")
    }

    fn load<T: serde::de::DeserializeOwned>(&self, sql: &str) -> Result<Vec<T>> {
        let connection = self.0.lock().unwrap();
        let mut statement = connection.prepare(sql)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn project(&self, project: &Project) -> Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO projects(id,payload) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",
            params![project.id, serde_json::to_string(project)?],
        )?;
        Ok(())
    }

    pub fn update_project(&self, expected: &Project, updated: &Project) -> Result<()> {
        ensure!(
            expected.id == updated.id,
            "Идентификатор проекта нельзя менять"
        );
        let mut connection = self.0.lock().unwrap();
        let transaction = connection.transaction()?;
        let (payload, archived): (String, bool) = transaction
            .query_row(
                "SELECT payload,archived FROM projects WHERE id=?1",
                [&expected.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("Проект не найден")?;
        ensure!(!archived, "Проект удалён; сначала восстановите его");
        let current: Project = serde_json::from_str(&payload)?;
        ensure!(
            same_project(&current, expected),
            "Проект уже изменён. Обновите данные перед сохранением"
        );
        transaction.execute(
            "UPDATE projects SET payload=?2 WHERE id=?1 AND archived=0",
            params![updated.id, serde_json::to_string(updated)?],
        )?;
        let mut defaults = read_defaults(&transaction)?;
        if defaults.prune_roots(updated) {
            defaults.revision = defaults
                .revision
                .checked_add(1)
                .filter(|n| *n <= i64::MAX as u64)
                .context("Исчерпан счётчик настроек запуска")?;
            write_defaults(&transaction, &defaults)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// A repeated delete with the same snapshot is idempotent. A stale snapshot
    /// is rejected even when a previous request already archived the project.
    pub fn archive_project(&self, expected: &Project) -> Result<()> {
        let mut connection = self.0.lock().unwrap();
        let transaction = connection.transaction()?;
        let payload: String = transaction
            .query_row(
                "SELECT payload FROM projects WHERE id=?1",
                [&expected.id],
                |row| row.get(0),
            )
            .optional()?
            .context("Проект не найден")?;
        let current: Project = serde_json::from_str(&payload)?;
        ensure!(
            same_project(&current, expected),
            "Проект уже изменён. Обновите данные перед удалением"
        );
        transaction.execute("UPDATE projects SET archived=1 WHERE id=?1", [&expected.id])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn restore_project(&self, id: &str) -> Result<Project> {
        let mut connection = self.0.lock().unwrap();
        let transaction = connection.transaction()?;
        let (payload, archived): (String, bool) = transaction
            .query_row(
                "SELECT payload,archived FROM projects WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("Проект не найден")?;
        if archived {
            let visible: i64 = transaction.query_row(
                "SELECT count(*) FROM projects WHERE archived=0",
                [],
                |row| row.get(0),
            )?;
            ensure!(
                visible < 100,
                "Лимит прототипа: 100 проектов. Удалите другой проект перед восстановлением"
            );
            transaction.execute("UPDATE projects SET archived=0 WHERE id=?1", [id])?;
        }
        let project = serde_json::from_str(&payload)?;
        transaction.commit()?;
        Ok(project)
    }

    pub fn session(&self, session: &Session) -> Result<()> {
        self.0.lock().unwrap().execute(
            "INSERT INTO sessions(id,payload) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",
            params![session.id, serde_json::to_string(session)?],
        )?;
        Ok(())
    }

    pub(crate) fn save_agent_answer(
        &self,
        session: &Session,
        text: &str,
        saved_at: u64,
        activity_changed: bool,
    ) -> Result<()> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        if activity_changed {
            tx.execute(
                "UPDATE sessions SET payload=?2 WHERE id=?1",
                params![session.id, serde_json::to_string(session)?],
            )?;
        }
        tx.execute(
            "INSERT INTO agent_answers(session_id,text,saved_at) VALUES(?1,?2,?3) ON CONFLICT(session_id) DO UPDATE SET text=excluded.text,saved_at=excluded.saved_at",
            params![session.id, text, i64::try_from(saved_at)?],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn agent_answer(&self, id: &str) -> Result<Option<AgentAnswer>> {
        // Authorization applies even after execution ends, as with session history.
        let connection = self.0.lock().unwrap();
        let visible: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions s JOIN projects p ON p.id=json_extract(s.payload,'$.project_id') WHERE s.id=?1 AND s.archived=0 AND p.archived=0)",
            [id],
            |row| row.get(0),
        )?;
        ensure!(visible, "Сессия недоступна");
        Ok(connection
            .query_row(
                "SELECT text,saved_at FROM agent_answers WHERE session_id=?1",
                [id],
                |row| {
                    Ok(AgentAnswer {
                        text: row.get(0)?,
                        saved_at: sql_unsigned(row, 1)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn archive_session(&self, id: &str) -> Result<()> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let (payload,project_archived):(String,bool)=tx.query_row("SELECT s.payload,p.archived FROM sessions s JOIN projects p ON p.id=json_extract(s.payload,'$.project_id') WHERE s.id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?.context("Сессия или проект не найдены")?;
        let session: Session = serde_json::from_str(&payload)?;
        ensure!(!project_archived, "Сначала восстановите проект сессии");
        ensure!(
            !session.active(),
            "Работающую сессию нельзя удалить. Сначала явно остановите её"
        );
        tx.execute("UPDATE sessions SET archived=1 WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn rename_session(
        &self,
        id: &str,
        title: &str,
        expected_title: &str,
        live: Option<&Session>,
    ) -> Result<Session> {
        let title = crate::model::session_title(title)?;
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let (payload,archived,project_archived):(String,bool,bool)=tx.query_row("SELECT s.payload,s.archived,p.archived FROM sessions s JOIN projects p ON p.id=json_extract(s.payload,'$.project_id') WHERE s.id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?.context("Сессия или проект не найдены")?;
        let stored: Session = serde_json::from_str(&payload)?;
        ensure!(
            !archived && !project_archived,
            "Сначала восстановите сессию и её проект"
        );
        ensure!(
            stored.title == expected_title,
            "Имя сессии уже изменилось. Обновите данные перед сохранением"
        );
        let mut renamed = live.cloned().unwrap_or(stored.clone());
        ensure!(
            renamed.id == stored.id
                && renamed.project_id == stored.project_id
                && renamed.root_id == stored.root_id,
            "Состояние сессии изменилось"
        );
        renamed.title = title;
        renamed.title_source = crate::model::SessionTitleSource::Manual;
        renamed.auto_title_eligible = false;
        tx.execute(
            "UPDATE sessions SET payload=?2 WHERE id=?1",
            params![id, serde_json::to_string(&renamed)?],
        )?;
        tx.commit()?;
        Ok(renamed)
    }

    pub fn restore_session(&self, id: &str) -> Result<Session> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let (payload,project_archived):(String,bool)=tx.query_row("SELECT s.payload,p.archived FROM sessions s JOIN projects p ON p.id=json_extract(s.payload,'$.project_id') WHERE s.id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?))).optional()?.context("Сессия или проект не найдены")?;
        let session: Session = serde_json::from_str(&payload)?;
        ensure!(!project_archived, "Сначала восстановите проект сессии");
        ensure!(
            !session.active(),
            "Восстановление не запускает и не меняет работающую сессию"
        );
        tx.execute("UPDATE sessions SET archived=0 WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(session)
    }

    pub fn attachment_totals(&self, id: &str) -> Result<(u64, u64)> {
        let (count, bytes): (i64, i64) = self.0.lock().unwrap().query_row(
            "SELECT count(*),coalesce(sum(size),0) FROM attachments WHERE session_id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((u64::try_from(count)?, u64::try_from(bytes)?))
    }

    pub fn record_attachment(&self, session: &Session, attachment: &Attachment) -> Result<()> {
        let mut connection = self.0.lock().unwrap();
        let tx = connection.transaction()?;
        let (payload,archived,project_archived):(String,bool,bool)=tx.query_row("SELECT s.payload,s.archived,p.archived FROM sessions s JOIN projects p ON p.id=json_extract(s.payload,'$.project_id') WHERE s.id=?1",[&session.id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?.context("Сессия или проект не найдены")?;
        let current: Session = serde_json::from_str(&payload)?;
        ensure!(
            !archived
                && !project_archived
                && current.active()
                && current.project_id == session.project_id
                && current.root_id == session.root_id,
            "Вложения доступны только для работающей сессии видимого проекта"
        );
        let (count, bytes): (i64, i64) = tx.query_row(
            "SELECT count(*),coalesce(sum(size),0) FROM attachments WHERE session_id=?1",
            [&session.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        ensure!(
            count >= 0
                && bytes >= 0
                && (count as u64) < MAX_SESSION_FILES
                && (bytes as u64)
                    .checked_add(attachment.size)
                    .is_some_and(|total| total <= MAX_SESSION_BYTES),
            "Лимит сессии: 50 вложений и 100 MiB загруженных файлов"
        );
        tx.execute(
            "INSERT INTO attachments(id,session_id,payload,size) VALUES(?1,?2,?3,?4)",
            params![
                attachment.id,
                session.id,
                serde_json::to_string(attachment)?,
                i64::try_from(attachment.size)?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn reconcile_restart(&self) -> Result<()> {
        // History pagination must never limit crash reconciliation.
        for mut session in self.load::<Session>("SELECT payload FROM sessions")? {
            if session.active() {
                session.activity.restarted(crate::model::now());
                session.status = "interrupted".into();
                session.error =
                    Some("Daemon перезапущен; CLI автоматически не возобновляется".into());
                self.session(&session)?;
            }
        }
        Ok(())
    }
}

pub fn same_project(first: &Project, second: &Project) -> bool {
    first.id == second.id
        && first.name == second.name
        && first.roots.len() == second.roots.len()
        && first
            .roots
            .iter()
            .zip(&second.roots)
            .all(|(a, b)| a.id == b.id && a.name == b.name && a.path == b.path)
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use std::{path::PathBuf, sync::Arc};
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("mpac-session-store-{}", crate::model::id()));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn session() -> Session {
        Session {
            id: "session".into(),
            project_id: "project".into(),
            root_id: "root".into(),
            agent: "demo".into(),
            title: "fixture".into(),
            status: "running".into(),
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
    fn attachment() -> Attachment {
        Attachment {
            id: crate::model::id(),
            name: "note".into(),
            path: "quota metadata fixture only".into(),
            size: 1,
            mime_type: "text/plain".into(),
        }
    }

    #[test]
    fn session_archive_and_quota_survive_legacy_schema_migration_and_reopen() {
        let temp = Temp::new();
        let path = temp.0.join("state.sqlite3");
        let project = Project {
            id: "project".into(),
            name: "legacy".into(),
            roots: vec![],
        };
        let mut saved = session();
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY,payload TEXT NOT NULL);CREATE TABLE sessions(id TEXT PRIMARY KEY,payload TEXT NOT NULL);").unwrap();
            db.execute(
                "INSERT INTO projects VALUES(?1,?2)",
                params![project.id, serde_json::to_string(&project).unwrap()],
            )
            .unwrap();
            let mut old = serde_json::to_value(&saved).unwrap();
            for key in ["access", "additional_roots", "permission_args"] {
                old.as_object_mut().unwrap().remove(key);
            }
            db.execute(
                "INSERT INTO sessions VALUES(?1,?2)",
                params![saved.id, old.to_string()],
            )
            .unwrap();
        }
        {
            let store = Store::open(&path).unwrap();
            assert!(!store.session_archived(&saved.id).unwrap());
            store.record_attachment(&saved, &attachment()).unwrap();
            saved.status = "stopped".into();
            store.session(&saved).unwrap();
            store.archive_session(&saved.id).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert!(store.session_archived(&saved.id).unwrap());
        assert!(store.visible_sessions().unwrap().is_empty());
        assert_eq!(store.archived_sessions().unwrap().len(), 1);
        assert_eq!(store.attachment_totals(&saved.id).unwrap(), (1, 1));
        assert_eq!(store.restore_session(&saved.id).unwrap().status, "stopped");
        store.archive_session(&saved.id).unwrap();
        saved.status = "running".into();
        store.session(&saved).unwrap();
        assert!(store.session_archived(&saved.id).unwrap());
        assert_eq!(store.active_sessions().unwrap().len(), 1);
        assert!(store.restore_session(&saved.id).is_err());
    }

    #[test]
    fn concurrent_quota_commits_cannot_exceed_fifty_files() {
        let temp = Temp::new();
        let store = Arc::new(Store::open(&temp.0.join("state.sqlite3")).unwrap());
        let project = Project {
            id: "project".into(),
            name: "fixture".into(),
            roots: vec![],
        };
        store.project(&project).unwrap();
        let session = session();
        store.session(&session).unwrap();
        for _ in 0..49 {
            store.record_attachment(&session, &attachment()).unwrap();
        }
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let store = store.clone();
                let session = session.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.record_attachment(&session, &attachment()).is_ok()
                })
            })
            .collect();
        assert_eq!(
            workers
                .into_iter()
                .map(|w| w.join().unwrap())
                .filter(|ok| *ok)
                .count(),
            1
        );
        assert_eq!(store.attachment_totals(&session.id).unwrap(), (50, 50));
    }
}
