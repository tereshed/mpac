use std::{
    collections::VecDeque,
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::Serialize;

use crate::{
    model::{Session, executable, now},
    store::Store,
};

const OUTPUT_LIMIT: usize = 512 * 1024;
const CHUNK_LIMIT: usize = 32 * 1024;

fn promote_auto_title(
    meta: &mut Session,
    completed: bool,
    cwd: &Path,
    store: &Store,
) -> Result<bool> {
    if !completed || !meta.auto_title_eligible || !meta.active() {
        return Ok(false);
    }
    let Some(title) = meta
        .cli_title
        .as_deref()
        .and_then(|title| crate::auto_title::candidate_title(title, cwd))
    else {
        return Ok(false);
    };
    let mut promoted = meta.clone();
    promoted.title = crate::model::session_title(&title)?;
    promoted.title_source = crate::model::SessionTitleSource::Harness;
    promoted.auto_title_eligible = false;
    // The caller holds the same metadata lock as manual rename/finalization.
    // Commit first: a persistence failure must not consume one-shot eligibility.
    store.session(&promoted)?;
    *meta = promoted;
    Ok(true)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Copy)]
struct ResizeMark {
    seq: u64,
    size: TerminalSize,
}

pub struct OutputBuffer {
    bytes: VecDeque<u8>,
    start: u64,
    geometry: VecDeque<ResizeMark>,
}
impl Default for OutputBuffer {
    fn default() -> Self {
        Self {
            bytes: VecDeque::new(),
            start: 0,
            geometry: VecDeque::from([ResizeMark {
                seq: 0,
                size: TerminalSize {
                    cols: 100,
                    rows: 24,
                },
            }]),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct OutputResize {
    offset: usize,
    cols: u16,
    rows: u16,
}

#[derive(Serialize)]
pub struct Output {
    pub(crate) data_base64: String,
    pub(crate) seq: u64,
    pub(crate) reset: bool,
    pub(crate) geometry: TerminalSize,
    pub(crate) resizes: Vec<OutputResize>,
}
impl Output {
    pub(crate) fn final_geometry(&self) -> TerminalSize {
        self.resizes
            .last()
            .map_or(self.geometry, |size| TerminalSize {
                cols: size.cols,
                rows: size.rows,
            })
    }
}

impl OutputBuffer {
    pub fn append(&mut self, data: &[u8]) {
        self.bytes.extend(data);
        let discard = self.bytes.len().saturating_sub(OUTPUT_LIMIT);
        self.bytes.drain(..discard);
        self.start += discard as u64;
        self.prune_geometry();
    }

    fn prune_geometry(&mut self) {
        while self.geometry.len() > 1 && self.geometry[1].seq <= self.start {
            self.geometry.pop_front();
        }
    }

    /// Record successful kernel resizes in the same ordering as output bytes.
    /// No-output resize bursts coalesce; metadata and retained bytes are bounded.
    fn resized(&mut self, cols: u16, rows: u16) {
        let size = TerminalSize { cols, rows };
        if self.geometry.back().unwrap().size == size {
            return;
        }
        let seq = self.seq();
        if self.geometry.back().unwrap().seq == seq {
            self.geometry.back_mut().unwrap().size = size;
        } else {
            self.geometry.push_back(ResizeMark { seq, size });
        }
        while self.geometry.len() > 512 {
            let start = self.geometry[1].seq;
            self.bytes.drain(..(start - self.start) as usize);
            self.start = start;
            self.geometry.pop_front();
        }
        self.prune_geometry();
    }

    pub fn read(&self, after: u64) -> Output {
        let end = self.start + self.bytes.len() as u64;
        let reset = after < self.start || after > end;
        let from = if reset { self.start } else { after };
        let bytes: Vec<_> = self
            .bytes
            .iter()
            .skip((from - self.start) as usize)
            .take(CHUNK_LIMIT)
            .copied()
            .collect();
        let geometry = self
            .geometry
            .iter()
            .rev()
            .find(|mark| mark.seq <= from)
            .unwrap()
            .size;
        let resizes = self
            .geometry
            .iter()
            .filter(|mark| mark.seq > from && mark.seq <= from + bytes.len() as u64)
            .map(|mark| OutputResize {
                offset: (mark.seq - from) as usize,
                cols: mark.size.cols,
                rows: mark.size.rows,
            })
            .collect();
        Output {
            seq: from + bytes.len() as u64,
            data_base64: STANDARD.encode(bytes),
            reset,
            geometry,
            resizes,
        }
    }

    pub fn seq(&self) -> u64 {
        self.start + self.bytes.len() as u64
    }
}

#[cfg(all(test, unix))]
type TestChannels = (
    Arc<LiveSession>,
    mpsc::Receiver<Vec<u8>>,
    mpsc::Receiver<PtySize>,
);

pub struct LiveSession {
    meta: Arc<Mutex<Session>>,
    output: Arc<Mutex<OutputBuffer>>,
    output_changed: tokio::sync::watch::Sender<u64>,
    output_closed: Arc<AtomicBool>,
    input: SyncSender<Vec<u8>>,
    resize: SyncSender<PtySize>,
    stop: Arc<AtomicBool>,
    lifecycle: Arc<Mutex<()>>,
    #[cfg(unix)]
    process_id: Option<u32>,
    #[cfg(unix)]
    root_identity: (u64, u64),
    #[cfg(unix)]
    root_path: String,
    #[cfg(unix)]
    extra_identities: Vec<(String, String, (u64, u64))>,
}

impl LiveSession {
    #[cfg(all(test, unix))]
    pub(crate) fn test_fixture(meta: Session, cwd: &Path) -> Arc<Self> {
        Self::test_fixture_channels(meta, cwd).0
    }
    #[cfg(all(test, unix))]
    pub(crate) fn test_fixture_channels(meta: Session, cwd: &Path) -> TestChannels {
        use std::os::unix::fs::MetadataExt;
        let metadata = cwd.metadata().unwrap();
        let (input, input_rx) = mpsc::sync_channel(16);
        let (resize, resize_rx) = mpsc::sync_channel(4);
        let session = Arc::new(Self {
            meta: Arc::new(Mutex::new(meta)),
            output: Arc::new(Mutex::new(OutputBuffer::default())),
            output_changed: tokio::sync::watch::channel(0).0,
            output_closed: Arc::new(AtomicBool::new(false)),
            input,
            resize,
            stop: Arc::new(AtomicBool::new(false)),
            lifecycle: Arc::new(Mutex::new(())),
            process_id: None,
            root_identity: (metadata.dev(), metadata.ino()),
            root_path: cwd.to_str().unwrap().into(),
            extra_identities: vec![],
        });
        (session, input_rx, resize_rx)
    }
    #[cfg(unix)]
    pub fn spawn(meta: Session, cwd: &Path, store: Arc<Store>) -> Result<Arc<Self>> {
        Self::spawn_with_activity(meta, cwd, store, None)
    }
    #[cfg(unix)]
    pub fn spawn_with_activity(
        meta: Session,
        cwd: &Path,
        store: Arc<Store>,
        instrumentation: Option<&crate::activity_hooks::LaunchHooks>,
    ) -> Result<Arc<Self>> {
        Self::spawn_with_environment(meta, cwd, store, instrumentation, None)
    }
    #[cfg(unix)]
    pub fn spawn_with_environment(
        mut meta: Session,
        cwd: &Path,
        store: Arc<Store>,
        instrumentation: Option<&crate::activity_hooks::LaunchHooks>,
        environment: Option<&crate::environment::Environment>,
    ) -> Result<Arc<Self>> {
        use std::os::unix::fs::MetadataExt;
        let metadata = cwd.metadata()?;
        let root_identity = (metadata.dev(), metadata.ino());
        let extra_identities = meta
            .additional_roots
            .iter()
            .map(|root| {
                let path = Path::new(&root.path);
                let canonical = path
                    .canonicalize()
                    .context("Дополнительная папка недоступна")?;
                anyhow::ensure!(
                    canonical.to_str() == Some(&root.path),
                    "Путь дополнительной папки изменился"
                );
                let metadata = path.metadata()?;
                Ok((
                    root.id.clone(),
                    root.path.clone(),
                    (metadata.dev(), metadata.ino()),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut command = match meta.agent.as_str() {
            "demo" => {
                let mut command = CommandBuilder::new(std::env::current_exe()?);
                command.arg("--demo-agent");
                command
            }
            "shell" | "command" => crate::terminal_launch::build(
                meta.terminal_launch
                    .as_ref()
                    .context("Нет параметров терминала")?,
            ),
            "claude" | "codex" | "hermes" => {
                CommandBuilder::new(executable(&meta.agent).context("CLI не найден в PATH")?)
            }
            _ => bail!("Профиль агента недоступен"),
        };
        command.args(&meta.permission_args);
        command.env_remove("MPAC_EVENT_CHANNEL");
        command.env_remove("MPAC_EVENT_PROVIDER");
        if let Some(instrumentation) = instrumentation {
            command.args(&instrumentation.args);
        }
        if (meta.auto_title_eligible || instrumentation.is_some()) && meta.agent == "codex" {
            command.args([
                "-c",
                "tui.terminal_title=[\"thread-name\"]",
                "-c",
                "tui.notifications=[\"agent-turn-complete\"]",
                "-c",
                "tui.notification_method=\"osc9\"",
                "-c",
                "tui.notification_condition=\"always\"",
            ]);
        }
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        if meta.agent == "claude" {
            for key in [
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_BASE_URL",
            ] {
                command.env_remove(key);
            }
        }
        if meta.agent == "codex" {
            for key in ["OPENAI_API_KEY", "CODEX_API_KEY", "OPENAI_BASE_URL"] {
                command.env_remove(key);
            }
        }
        if let Some(environment) = environment {
            environment.apply(&mut command);
        }
        if let Some(instrumentation) = instrumentation {
            command.env("MPAC_EVENT_CHANNEL", &instrumentation.channel_path);
            command.env("MPAC_EVENT_PROVIDER", &instrumentation.provider);
        }
        let pair = native_pty_system().openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let child = pair
            .slave
            .spawn_command(command)
            .context("Не удалось запустить CLI")?;
        drop(pair.slave);
        let pid = child.process_id().context("Нет process ID")?;
        // portable-pty on Unix starts a session leader. Retain it unreaped until
        // the one final group signal; never try_wait from a second thread.
        let mut guard = GroupGuard {
            pid: pid as i32,
            signalled: false,
            child,
        };
        let metadata = cwd.metadata()?;
        if (metadata.dev(), metadata.ino()) != root_identity {
            bail!("Папка изменилась во время запуска CLI");
        }
        for (_, path, identity) in &extra_identities {
            let metadata = Path::new(path).metadata()?;
            anyhow::ensure!(
                (metadata.dev(), metadata.ino()) == *identity,
                "Дополнительная папка изменилась во время запуска CLI"
            );
        }
        let mut reader = pair.master.try_clone_reader()?;
        let fd = pair.master.as_raw_fd().context("Нет PTY descriptor")?;
        // portable-pty's UnixMasterWriter Drop writes newline/EOT. A plain
        // duplicated file descriptor only closes: neither Stop nor a failed
        // startup may ever submit pending user input as a side effect of Drop.
        let mut writer = std::fs::File::from(
            unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?,
        );
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            bail!("Не удалось настроить неблокирующий PTY");
        }
        meta.status = "running".into();
        store.session(&meta)?;
        let meta = Arc::new(Mutex::new(meta));
        let output = Arc::new(Mutex::new(OutputBuffer::default()));
        let output_changed = tokio::sync::watch::channel(0_u64).0;
        let output_closed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let lifecycle = Arc::new(Mutex::new(()));
        let io_closed = Arc::new(AtomicBool::new(false));
        let (input_tx, input_rx) = mpsc::sync_channel::<Vec<u8>>(16);
        let (resize_tx, resize_rx) = mpsc::sync_channel::<PtySize>(4);
        let session = Arc::new(Self {
            meta: meta.clone(),
            output: output.clone(),
            output_changed: output_changed.clone(),
            output_closed: output_closed.clone(),
            input: input_tx,
            resize: resize_tx,
            stop: stop.clone(),
            lifecycle: lifecycle.clone(),
            process_id: Some(pid),
            root_identity,
            root_path: cwd
                .to_str()
                .context("Путь должен поддерживать UTF-8")?
                .to_owned(),
            extra_identities,
        });

        let writer_stop = stop.clone();
        thread::spawn(move || {
            // A blocked paste cannot prevent supervisor Stop. This thread and
            // the reader are never synchronously joined by shutdown.
            'writer: while !writer_stop.load(Ordering::Acquire) {
                let data = match input_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(data) => data,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let mut offset = 0;
                while offset < data.len() && !writer_stop.load(Ordering::Acquire) {
                    match writer.write(&data[offset..]) {
                        Ok(0) => break 'writer,
                        Ok(n) => offset += n,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => break 'writer,
                    }
                }
            }
        });
        let read_meta = meta.clone();
        let reader_stop = io_closed.clone();
        let reader_store = store.clone();
        let reader_cwd = cwd.to_path_buf();
        let capability_input = session.input.clone();
        let mut auto_title_tracker = {
            let meta = read_meta.lock().unwrap();
            meta.auto_title_eligible
                .then(|| crate::auto_title::AutoTitleTracker::new(&meta.agent))
        };
        let read_changed = output_changed.clone();
        let resize_output = output.clone();
        let resize_changed = output_changed.clone();
        thread::spawn(move || {
            let mut bytes = [0_u8; 8192];
            let mut demo_tail = String::new();
            let mut title_parser = crate::terminal_title::OscTitleParser::default();
            let mut activity_parser = crate::activity_osc::ActivityOscParser::default();
            loop {
                let n = match reader.read(&mut bytes) {
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if reader_stop.load(Ordering::Acquire) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                if n == 0 {
                    break;
                }
                let completed = if let Some(tracker) = &mut auto_title_tracker {
                    let effects = tracker.push(&bytes[..n]);
                    if effects.reply_capability
                        && !read_meta.lock().unwrap().activity.capabilities.foreground
                    {
                        // Static terminal capability reply only; never replay OSC payload as input.
                        let _ =
                            capability_input.try_send(crate::auto_title::CAPABILITY_REPLY.to_vec());
                    }
                    effects.completed
                } else {
                    false
                };
                let mut out = output.lock().unwrap();
                out.append(&bytes[..n]);
                let mut meta = read_meta.lock().unwrap();
                meta.last_activity = now();
                meta.seq = out.seq();
                if let Some(candidate) = title_parser.push(&bytes[..n]) {
                    meta.cli_title = Some(candidate);
                }
                let agent = meta.agent.clone();
                let before_activity = meta.activity.clone();
                let effects = activity_parser.push(&bytes[..n], &mut meta.activity, &agent, now());
                if effects.reply_capability {
                    let _ = capability_input.try_send(crate::auto_title::CAPABILITY_REPLY.to_vec());
                }
                if effects.changed
                    && serde_json::to_value(&before_activity).ok()
                        != serde_json::to_value(&meta.activity).ok()
                {
                    let _ = reader_store.session(&meta);
                }
                let _ = promote_auto_title(&mut meta, completed, &reader_cwd, &reader_store);
                if meta.agent == "demo" && meta.active() {
                    demo_tail.push_str(&String::from_utf8_lossy(&bytes[..n]));
                    if demo_tail.contains("mpac-demo;question") {
                        meta.status = "needs_attention".into();
                        demo_tail.clear();
                    }
                    if demo_tail.len() > 4096 {
                        demo_tail.clear();
                    }
                }
                drop(meta);
                drop(out);
                read_changed.send_modify(|revision| *revision = revision.wrapping_add(1));
            }
            output_closed.store(true, Ordering::Release);
            read_changed.send_modify(|revision| *revision = revision.wrapping_add(1));
        });
        thread::spawn(move || {
            let master = pair.master;
            let natural = loop {
                if stop.load(Ordering::Acquire) {
                    break false;
                }
                while let Ok(size) = resize_rx.try_recv() {
                    let mut output = resize_output.lock().unwrap();
                    if master.resize(size).is_ok() {
                        output.resized(size.cols, size.rows);
                        resize_changed.send_modify(|revision| *revision = revision.wrapping_add(1));
                    }
                }
                match exited_unreaped(pid) {
                    Ok(true) => break true,
                    Ok(false) => {}
                    Err(_) => {
                        meta.lock().unwrap().error = Some("Ошибка наблюдения за процессом".into());
                        break false;
                    }
                }
                thread::sleep(Duration::from_millis(30));
            };
            let _lifecycle = lifecycle.lock().unwrap();
            let killed = guard.signal();
            stop.store(true, Ordering::Release);
            // With a successful SIGKILL no root can remain running. If signal
            // delivery fails, do not falsely claim a completed Stop.
            if let Err(error) = killed {
                eprintln!(
                    "PTY group stop failed: errno={:?}, natural={natural}",
                    error.raw_os_error()
                );
                let mut meta = meta.lock().unwrap();
                meta.status = "failed".into();
                meta.error = Some("Не удалось остановить группу процессов".into());
                let _ = store.session(&meta);
                output_changed.send_modify(|revision| *revision = revision.wrapping_add(1));
                return;
            }
            io_closed.store(true, Ordering::Release);
            let status = guard.child.wait();
            drop(master);
            let mut meta = meta.lock().unwrap();
            meta.status = match &status {
                Ok(s) if natural && s.success() => "exited",
                Ok(_) if natural => "failed",
                Ok(_) => "stopped",
                Err(_) => "failed",
            }
            .into();
            meta.exit_code = status.ok().map(|s| s.exit_code());
            meta.activity.process_ended(now());
            if meta.status == "failed" && meta.error.is_none() {
                meta.error = Some("CLI завершился с ошибкой".into());
            }
            let _ = store.session(&meta);
            drop(meta);
            output_changed.send_modify(|revision| *revision = revision.wrapping_add(1));
        });
        Ok(session)
    }

    #[cfg(not(unix))]
    pub fn spawn(_meta: Session, _cwd: &Path, _store: Arc<Store>) -> Result<Arc<Self>> {
        bail!("PTY прототип доступен на macOS/Linux; Windows Job Object + ConPTY ещё не проверены")
    }
    #[cfg(not(unix))]
    pub fn spawn_with_activity(
        meta: Session,
        cwd: &Path,
        store: Arc<Store>,
        _instrumentation: Option<&crate::activity_hooks::LaunchHooks>,
    ) -> Result<Arc<Self>> {
        Self::spawn(meta, cwd, store)
    }

    pub fn meta(&self) -> Session {
        self.meta.lock().unwrap().clone()
    }
    pub(crate) fn activity_event(
        &self,
        store: &Store,
        provider: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let mut meta = self.meta.lock().unwrap();
        anyhow::ensure!(
            meta.active() && !self.stop.load(Ordering::Acquire),
            "Исполнение завершено"
        );
        anyhow::ensure!(meta.agent == provider, "Не тот источник события");
        let mut next = meta.clone();
        let observed_at = now();
        let changed = match provider {
            "claude" => next.activity.apply_claude(payload, observed_at)?,
            "codex" => next.activity.apply_codex(payload, observed_at)?,
            _ => false,
        };
        let answer = crate::activity_hooks::stop_answer(payload).filter(|_| match provider {
            "claude" => changed,
            "codex" => next.activity.accepted_codex_stop(payload),
            _ => false,
        });
        if let Some(text) = answer {
            store.save_agent_answer(&next, text, observed_at, changed)?;
        } else if changed {
            store.session(&next)?;
        }
        *meta = next;
        Ok(())
    }
    pub(crate) fn mark_activity_read(
        &self,
        store: &Store,
        expected_revision: u64,
    ) -> Result<Session> {
        let mut meta = self.meta.lock().unwrap();
        let mut next = meta.clone();
        next.activity.mark_read(expected_revision)?;
        store.session(&next)?;
        *meta = next.clone();
        Ok(next)
    }
    pub fn rename(&self, store: &Store, title: &str, expected: &str) -> Result<Session> {
        let _lifecycle = self.lifecycle.lock().unwrap();
        let mut meta = self.meta.lock().unwrap();
        if meta.title != expected {
            bail!("Имя сессии уже изменилось. Обновите данные перед сохранением");
        }
        let renamed = store.rename_session(&meta.id, title, expected, Some(&meta))?;
        *meta = renamed.clone();
        Ok(renamed)
    }

    pub fn execution_paths(&self) -> Vec<String> {
        #[cfg(unix)]
        {
            std::iter::once(self.root_path.clone())
                .chain(
                    self.extra_identities
                        .iter()
                        .map(|(_, path, _)| path.clone()),
                )
                .collect()
        }
        #[cfg(not(unix))]
        {
            vec![]
        }
    }
    pub fn output(&self, after: u64) -> Output {
        self.output.lock().unwrap().read(after)
    }
    pub(crate) fn subscribe_output(&self) -> tokio::sync::watch::Receiver<u64> {
        self.output_changed.subscribe()
    }
    pub(crate) fn terminal_finished(&self) -> bool {
        self.output_closed.load(Ordering::Acquire) && !self.meta.lock().unwrap().active()
    }
    #[cfg(all(test, unix))]
    pub(crate) fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
    #[cfg(all(test, unix))]
    pub(crate) fn append_test_output(&self, data: &[u8]) {
        self.output.lock().unwrap().append(data);
        self.output_changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    #[cfg(all(test, unix))]
    pub(crate) fn resize_test_output(&self, cols: u16, rows: u16) {
        self.output.lock().unwrap().resized(cols, rows);
        self.output_changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    #[cfg(all(test, unix))]
    pub(crate) fn finish_test_output(&self) {
        self.meta.lock().unwrap().status = "exited".into();
        self.output_closed.store(true, Ordering::Release);
        self.output_changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    pub fn stop(&self) {
        let _lifecycle = self.lifecycle.lock().unwrap();
        self.stop.store(true, Ordering::Release);
        self.output_changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    /// Serializes an explicit upload with Stop and natural finalization. The
    /// operation never creates or resumes an execution and may not send input.
    pub fn with_active<T>(&self, operation: impl FnOnce(&Session) -> Result<T>) -> Result<T> {
        let _lifecycle = self.lifecycle.lock().unwrap();
        self.check_upload_active()?;
        let meta = self.meta();
        operation(&meta)
    }

    pub fn check_upload_active(&self) -> Result<()> {
        if !self.meta().active() || self.stop.load(Ordering::Acquire) {
            bail!("Вложения доступны только для работающей сессии");
        }
        #[cfg(unix)]
        if let Some(pid) = self.process_id
            && exited_unreaped(pid).context("Не удалось проверить исполнение сессии")?
        {
            bail!("CLI уже завершился; вложение не сохранено");
        }
        Ok(())
    }

    pub fn working_root_identity(&self) -> Result<(u64, u64)> {
        #[cfg(unix)]
        {
            Ok(self.root_identity)
        }
        #[cfg(not(unix))]
        {
            bail!("Вложения пока доступны только на macOS/Linux")
        }
    }

    pub fn verify_root(&self, root_id: &str, root: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let current = root.metadata().context("Папка сессии недоступна")?;
            let expected = if root_id == self.meta.lock().unwrap().root_id
                && root.to_str() == Some(&self.root_path)
            {
                self.root_identity
            } else {
                self.extra_identities
                    .iter()
                    .find(|(id, path, _)| id == root_id && root.to_str() == Some(path))
                    .map(|(_, _, identity)| *identity)
                    .context("Папка не входила в доступ сессии при запуске")?
            };
            if (current.dev(), current.ino()) != expected {
                bail!("Папка заменена после запуска CLI; запустите новую сессию");
            }
        }
        Ok(())
    }

    pub fn input(&self, data: String) -> Result<()> {
        if data.len() > 16 * 1024 {
            bail!("Слишком большой ввод; отправьте до 16 KiB за раз");
        }
        let mut meta = self.meta.lock().unwrap();
        if !meta.active() || self.stop.load(Ordering::Acquire) {
            bail!("Сессия уже завершена");
        }
        let submitted = data.contains(['\r', '\n']);
        self.input
            .try_send(data.into_bytes())
            .context("Очередь ввода заполнена или закрыта")?;
        if meta.agent == "demo" && submitted {
            meta.status = "running".into();
        }
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        if !(20..=300).contains(&cols) || !(5..=150).contains(&rows) {
            bail!("Размер терминала вне диапазона");
        }
        self.resize
            .try_send(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Очередь resize заполнена или сессия закрыта")
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(unix)]
struct GroupGuard {
    pid: i32,
    signalled: bool,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

#[cfg(unix)]
impl GroupGuard {
    fn signal(&mut self) -> std::io::Result<()> {
        if self.signalled {
            return Ok(());
        }
        // Negative pid targets this still-owned session's original group.
        let result = unsafe { libc::kill(-self.pid, libc::SIGKILL) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            #[cfg(target_os = "macos")]
            if error.raw_os_error() == Some(libc::EPERM) && group_has_no_live_members(self.pid)? {
                self.signalled = true;
                return Ok(());
            }
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        self.signalled = true;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn group_has_no_live_members(pgid: i32) -> std::io::Result<bool> {
    // Darwin returns EPERM for zombie-only groups. Verify native run state;
    // a live or uninspectable member must never turn EPERM into success.
    let mut pids = vec![0_i32; 4096];
    let bytes = (pids.len() * std::mem::size_of::<i32>()) as i32;
    let count = unsafe { libc::proc_listpids(2, pgid as u32, pids.as_mut_ptr().cast(), bytes) };
    if count < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // The owned unreaped leader must still be enumerable. Zero is also an
    // API failure result, so it cannot prove that the group has drained.
    if count == 0 {
        return Ok(false);
    }
    if count >= bytes {
        return Ok(false);
    }
    for pid in pids
        .into_iter()
        .take(count as usize / std::mem::size_of::<i32>())
        .filter(|pid| *pid > 0)
    {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&info) as i32;
        let result = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if result == 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            continue;
        }
        if result != size || info.pbi_status != libc::SZOMB {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(unix)]
impl Drop for GroupGuard {
    fn drop(&mut self) {
        if self.signal().is_ok() {
            let _ = self.child.wait();
        }
    }
}

#[cfg(unix)]
fn exited_unreaped(pid: u32) -> std::io::Result<bool> {
    // No other thread can reap this portable-pty std::process::Child.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    #[cfg(target_os = "macos")]
    let observed = info.si_pid;
    #[cfg(not(target_os = "macos"))]
    let observed = unsafe { info.si_pid() };
    Ok(observed != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_title_waits_for_completion_promotes_once_and_persists() {
        let root = std::env::temp_dir().join(format!("mpac-auto-title-{}", crate::model::id()));
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(&root.join("state.sqlite3")).unwrap();
        store
            .project(&crate::model::Project {
                id: "project".into(),
                name: "fixture".into(),
                roots: vec![],
            })
            .unwrap();
        let mut meta: Session = serde_json::from_value(serde_json::json!({
            "id":"session", "project_id":"project", "root_id":"root", "agent":"codex",
            "title":"codex", "status":"running", "started_at":1, "last_activity":1,
            "exit_code":null, "error":null, "seq":0, "mode":"interactive_pty"
        }))
        .unwrap();
        // Old records must never become eligible merely by being deserialized.
        assert!(!meta.auto_title_eligible);
        assert_eq!(meta.title_source, crate::model::SessionTitleSource::Manual);
        meta.auto_title_eligible = true;
        meta.title_source = crate::model::SessionTitleSource::PendingFirstAnswer;
        meta.cli_title = Some("Implement launch defaults".into());
        store.session(&meta).unwrap();
        assert!(!promote_auto_title(&mut meta, false, &root, &store).unwrap());
        assert_eq!(meta.title, "codex");
        assert!(promote_auto_title(&mut meta, true, &root, &store).unwrap());
        assert_eq!(meta.title, "Implement launch defaults");
        assert_eq!(meta.title_source, crate::model::SessionTitleSource::Harness);
        assert!(!meta.auto_title_eligible);
        meta.cli_title = Some("Second answer title must not replace name".into());
        assert!(!promote_auto_title(&mut meta, true, &root, &store).unwrap());
        let stored = store.session_entry(&meta.id).unwrap().0;
        assert_eq!(stored.title, "Implement launch defaults");
        assert!(!stored.auto_title_eligible);
        meta.auto_title_eligible = true;
        meta.status = "stopped".into();
        assert!(!promote_auto_title(&mut meta, true, &root, &store).unwrap());
        meta.status = "running".into();
        meta.title_source = crate::model::SessionTitleSource::PendingFirstAnswer;
        store.session(&meta).unwrap();
        let title = meta.title.clone();
        meta = store
            .rename_session(&meta.id, &title, &title, Some(&meta))
            .unwrap();
        assert!(!meta.auto_title_eligible);
        assert_eq!(meta.title_source, crate::model::SessionTitleSource::Manual);
        assert!(!promote_auto_title(&mut meta, true, &root, &store).unwrap());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn manual_title_is_not_reverted_by_concurrent_finalization() {
        let root = std::env::temp_dir().join(format!("mpac-rename-race-{}", crate::model::id()));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let store = Arc::new(Store::open(&root.join("state.sqlite3")).unwrap());
        store
            .project(&crate::model::Project {
                id: "project".into(),
                name: "Fixture".into(),
                roots: vec![],
            })
            .unwrap();
        let meta = Session {
            id: crate::model::id(),
            project_id: "project".into(),
            root_id: "root".into(),
            agent: "demo".into(),
            title: "Original".into(),
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
            cli_title: Some("CLI candidate".into()),
            title_source: crate::model::SessionTitleSource::PendingFirstAnswer,
            auto_title_eligible: true,
            activity: Default::default(),
            environment: None,
            terminal_launch: None,
        };
        store.session(&meta).unwrap();
        let owner = LiveSession::test_fixture(meta.clone(), &root);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let final_owner = owner.clone();
        let final_store = store.clone();
        let final_barrier = barrier.clone();
        let finalize = thread::spawn(move || {
            final_barrier.wait();
            let _lifecycle = final_owner.lifecycle.lock().unwrap();
            let mut meta = final_owner.meta.lock().unwrap();
            meta.status = "stopped".into();
            final_store.session(&meta).unwrap();
        });
        let rename_owner = owner.clone();
        let rename_store = store.clone();
        let rename = thread::spawn(move || {
            barrier.wait();
            rename_owner
                .rename(&rename_store, "Manual name", "Original")
                .unwrap();
        });
        finalize.join().unwrap();
        rename.join().unwrap();
        let saved = store.session_entry(&meta.id).unwrap().0;
        assert_eq!(saved.title, "Manual name");
        assert_eq!(saved.status, "stopped");
        assert_eq!(saved.cli_title.as_deref(), Some("CLI candidate"));
        assert!(!saved.auto_title_eligible);
        assert_eq!(saved.title_source, crate::model::SessionTitleSource::Manual);
        assert_eq!(owner.meta().title, "Manual name");
        drop(owner);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn upload_gate_rejects_naturally_exited_cli_before_metadata_finalization() {
        let cwd = std::env::temp_dir().canonicalize().unwrap();
        let meta = Session {
            id: crate::model::id(),
            project_id: "fixture".into(),
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
        };
        let mut owner = LiveSession::test_fixture(meta, &cwd);
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        Arc::get_mut(&mut owner).unwrap().process_id = Some(child.id());
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !exited_unreaped(child.id()).unwrap() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert!(owner.with_active(|_| Ok(())).is_err());
        assert_eq!(owner.meta().status, "running");
        assert!(child.wait().unwrap().success());
    }

    #[cfg(unix)]
    #[test]
    fn natural_exit_is_observed_without_reaping_and_group_can_be_finalized() {
        use std::os::unix::process::CommandExt;
        let child = std::process::Command::new("/usr/bin/true")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        let mut guard = GroupGuard {
            pid: pid as i32,
            signalled: false,
            child: Box::new(child),
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !exited_unreaped(pid).unwrap() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        guard.signal().unwrap();
        assert!(guard.child.wait().unwrap().success());
        guard.signal().unwrap(); // never send a new signal after reaping
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn live_group_is_not_misclassified_as_zombie_only() {
        use std::os::unix::process::CommandExt;
        let child = std::process::Command::new("/bin/sleep")
            .arg("10")
            .process_group(0)
            .spawn()
            .unwrap();
        let guard = GroupGuard {
            pid: child.id() as i32,
            signalled: false,
            child: Box::new(child),
        };
        assert!(!group_has_no_live_members(guard.pid).unwrap());
    }

    #[test]
    fn ring_offsets_and_replay_are_bounded_and_preserve_raw_bytes() {
        let mut ring = OutputBuffer::default();
        ring.append(&[0xe2]);
        ring.append(&[0x82, 0xac]);
        assert_eq!(
            STANDARD.decode(ring.read(0).data_base64).unwrap(),
            "€".as_bytes()
        );
        ring.append(&vec![b'x'; OUTPUT_LIMIT + 5]);
        let stale = ring.read(0);
        assert!(stale.reset);
        assert_eq!(
            STANDARD.decode(stale.data_base64).unwrap().len(),
            CHUNK_LIMIT
        );
        assert!(stale.seq > CHUNK_LIMIT as u64);
        assert!(!ring.read(ring.seq()).reset);
        assert!(ring.read(ring.seq()).data_base64.is_empty());
        assert!(ring.read(u64::MAX).reset);
    }

    #[test]
    fn replay_preserves_geometry_at_byte_boundaries_and_idle_resizes() {
        let mut ring = OutputBuffer::default();
        ring.append(&[0xe2]);
        ring.resized(180, 50);
        ring.append(&[0x82, 0xac]);
        let output = ring.read(0);
        assert_eq!(
            output.geometry,
            TerminalSize {
                cols: 100,
                rows: 24
            }
        );
        assert_eq!(output.resizes[0].offset, 1);
        assert_eq!(
            output.final_geometry(),
            TerminalSize {
                cols: 180,
                rows: 50
            }
        );
        assert_eq!(STANDARD.decode(output.data_base64).unwrap(), "€".as_bytes());
        assert_eq!(
            ring.read(1).geometry,
            TerminalSize {
                cols: 180,
                rows: 50
            }
        );
        ring.resized(160, 40);
        let idle = ring.read(ring.seq());
        assert!(idle.data_base64.is_empty());
        assert_eq!(
            idle.geometry,
            TerminalSize {
                cols: 160,
                rows: 40
            }
        );
    }

    #[test]
    fn chunk_edges_and_evicted_history_keep_the_right_starting_geometry() {
        let mut ring = OutputBuffer::default();
        ring.append(&vec![b'a'; CHUNK_LIMIT]);
        ring.resized(160, 40);
        ring.append(&vec![b'b'; CHUNK_LIMIT]);
        let first = ring.read(0);
        assert_eq!(first.resizes[0].offset, CHUNK_LIMIT);
        assert_eq!(
            ring.read(first.seq).geometry,
            TerminalSize {
                cols: 160,
                rows: 40
            }
        );
        ring.append(&vec![b'c'; OUTPUT_LIMIT]);
        let reset = ring.read(0);
        assert!(reset.reset);
        assert_eq!(
            reset.geometry,
            TerminalSize {
                cols: 160,
                rows: 40
            }
        );
        assert!(reset.resizes.is_empty());
    }

    #[test]
    fn resize_metadata_is_bounded_even_without_output_or_with_adversarial_churn() {
        let mut ring = OutputBuffer::default();
        for i in 0..2000 {
            ring.resized(100 + i % 5, 24);
        }
        assert_eq!(ring.geometry.len(), 1);
        for i in 0..2000 {
            ring.append(b"x");
            ring.resized(150 + i % 5, 50);
        }
        assert!(ring.geometry.len() <= 512);
        assert!(ring.bytes.len() <= 512);
        assert!(ring.read(0).reset);
        assert!(ring.read(0).resizes.len() <= 511);
    }
}
