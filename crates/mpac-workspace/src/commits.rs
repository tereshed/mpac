//! Explicit whole-file commits. A private index isolates staging; the final
//! commit-msg hook verifies the reviewed tree and parent before Git publishes.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use process_wrap::std::CommandWrap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    branches::filter_overrides, git_strings, head, open_root, parse_status, read_bytes,
    relative_path,
};

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILES: usize = 256;
const MAX_INDEX: u64 = 32 * 1024 * 1024;
const READ_BUDGET: Duration = Duration::from_secs(15);
const WRITE_BUDGET: Duration = Duration::from_secs(90);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitFile {
    pub path: String,
    pub status: String,
    pub diff: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitPreview {
    pub head: String,
    pub branch: Option<String>,
    pub snapshot: String,
    pub paths: Vec<String>,
    pub files: Vec<CommitFile>,
    pub diff: String,
    pub blocked_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitResult {
    pub committed: bool,
    pub head: String,
    pub previous_head: String,
    pub paths: Vec<String>,
    pub warning: Option<String>,
    pub recovery_index: Option<String>,
}

struct CapturedFile {
    path: String,
    bytes: Option<Vec<u8>>,
    mode: String,
}

struct Capture {
    preview: CommitPreview,
    files: Vec<CapturedFile>,
    git_dir: PathBuf,
    index: Option<Vec<u8>>,
}

fn checked_paths(paths: &[String]) -> Result<Vec<String>> {
    ensure!(
        !paths.is_empty() && paths.len() <= MAX_FILES,
        "Выберите от 1 до 256 файлов"
    );
    let mut result = BTreeSet::new();
    for path in paths {
        ensure!(
            !path.is_empty() && path.len() <= 16384 && !path.contains('\0'),
            "Недопустимый путь файла"
        );
        let relative = relative_path(path)?;
        ensure!(
            relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "Укажите путь файла внутри репозитория"
        );
        ensure!(
            relative.components().all(|c| c.as_os_str() != ".git"),
            "Служебные файлы Git нельзя добавить в коммит"
        );
        ensure!(
            result.insert(path.clone()),
            "Файлы в коммите не должны повторяться"
        );
    }
    Ok(result.into_iter().collect())
}

fn output_text(root: &Path, args: &[&str], deadline: Instant) -> Result<String> {
    let output = git_strings(root, args, deadline)?;
    ensure!(output.success, "Не удалось прочитать состояние Git");
    let text = String::from_utf8(output.stdout)?;
    Ok(text.strip_suffix('\n').unwrap_or(&text).to_owned())
}

fn read_index(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= MAX_INDEX,
                "Git index недоступен или слишком большой"
            );
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            let mut file = options.open(path)?;
            let mut bytes = Vec::new();
            Read::by_ref(&mut file)
                .take(MAX_INDEX + 1)
                .read_to_end(&mut bytes)?;
            ensure!(bytes.len() as u64 <= MAX_INDEX, "Git index слишком большой");
            Ok(Some(bytes))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn identity(path: &Path) -> Result<Vec<u8>> {
    let metadata = open_root(path)?.dir_metadata()?;
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        Ok([metadata.dev().to_le_bytes(), metadata.ino().to_le_bytes()].concat())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(path.canonicalize()?.as_os_str().as_encoded_bytes().to_vec())
    }
}

fn frozen_diff(
    root: &Path,
    head: &str,
    path: &str,
    new: Option<&[u8]>,
    mode: &str,
    deadline: Instant,
) -> Result<String> {
    let (old, old_mode) = if head == "unborn" {
        (None, "100644".to_owned())
    } else {
        let spec = format!("{head}:{path}");
        let listed = git_strings(
            root,
            &["--literal-pathspecs", "ls-tree", "-z", head, "--", path],
            deadline,
        )?;
        ensure!(listed.success, "Не удалось прочитать исходный режим файла");
        if !listed.stdout.is_empty() {
            let blob = git_strings(root, &["cat-file", "blob", &spec], deadline)?;
            ensure!(blob.success, "Не удалось прочитать исходный файл HEAD");
            let old_mode = std::str::from_utf8(&listed.stdout)?
                .split(' ')
                .next()
                .unwrap_or("100644")
                .to_owned();
            (Some(blob.stdout), old_mode)
        } else {
            (None, "100644".to_owned())
        }
    };
    ensure!(
        old.is_some() || new.is_some(),
        "Файл отсутствует и в HEAD, и в рабочей папке"
    );
    let scratch = Work::new(&std::env::temp_dir())?;
    if let Some(bytes) = &old {
        fs::write(scratch.path.join("before"), bytes)?;
    }
    if let Some(bytes) = new {
        fs::write(scratch.path.join("after"), bytes)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for (name, exists, mode) in [
            ("before", old.is_some(), old_mode.as_str()),
            ("after", new.is_some(), mode),
        ] {
            if exists {
                fs::set_permissions(
                    scratch.path.join(name),
                    fs::Permissions::from_mode(if mode == "100755" { 0o700 } else { 0o600 }),
                )?;
            }
        }
    }
    let before = if old.is_some() { "before" } else { "/dev/null" };
    let after = if new.is_some() { "after" } else { "/dev/null" };
    let diff = git_strings(
        &scratch.path,
        &[
            "diff",
            "--no-index",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--",
            before,
            after,
        ],
        deadline,
    )?;
    ensure!(
        matches!(diff.exit_code, Some(0 | 1)),
        "Не удалось построить diff сохранённого снимка"
    );
    let text = String::from_utf8(diff.stdout).context("Git diff должен поддерживать UTF-8")?;
    let a = serde_json::to_string(&format!("a/{path}"))?;
    let b = serde_json::to_string(&format!("b/{path}"))?;
    Ok(text
        .split_inclusive('\n')
        .map(|line| {
            if line.starts_with("diff --git ") {
                format!("diff --git {a} {b}\n")
            } else if line.starts_with("--- ") {
                format!(
                    "--- {}\n",
                    if old.is_some() {
                        a.as_str()
                    } else {
                        "/dev/null"
                    }
                )
            } else if line.starts_with("+++ ") {
                format!(
                    "+++ {}\n",
                    if new.is_some() {
                        b.as_str()
                    } else {
                        "/dev/null"
                    }
                )
            } else if line.starts_with("Binary files ") {
                format!("Binary files {a} and {b} differ\n")
            } else {
                line.to_owned()
            }
        })
        .collect())
}

fn capture(root: &Path, requested: &[String]) -> Result<Capture> {
    let paths = checked_paths(requested)?;
    let deadline = Instant::now() + READ_BUDGET;
    let directory = open_root(root)?;
    let canonical = root.canonicalize()?;
    let top = output_text(root, &["rev-parse", "--show-toplevel"], deadline)?;
    ensure!(
        Path::new(&top).canonicalize()? == canonical,
        "Выберите корневую папку Git-репозитория"
    );
    let root_identity = identity(root)?;
    let git_dir = PathBuf::from(output_text(
        root,
        &["rev-parse", "--absolute-git-dir"],
        deadline,
    )?)
    .canonicalize()?;
    let git_identity = identity(&git_dir)?;
    let index = read_index(&git_dir.join("index"))?;
    let before_head = head(root, deadline)?;
    let symbolic = git_strings(
        root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        deadline,
    )?;
    let branch = if symbolic.success {
        Some(
            String::from_utf8(symbolic.stdout)?
                .trim_end_matches('\n')
                .to_owned(),
        )
    } else {
        None
    };
    let mut blocked_reason = None;
    if branch.is_none() {
        blocked_reason = Some("Для коммита выберите локальную ветку: сейчас HEAD отсоединён".into())
    }
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ] {
        if git_dir.join(name).exists() {
            blocked_reason =
                Some("Завершите текущую операцию merge/rebase/cherry-pick через Git".into())
        }
    }
    let shared = output_text(root, &["rev-parse", "--shared-index-path"], deadline)?;
    if !shared.is_empty() {
        blocked_reason = Some("Коммит со split index пока выполняется через Git".into())
    }
    let sparse = git_strings(root, &["config", "--bool", "core.sparseCheckout"], deadline)?;
    if sparse.success && sparse.stdout.starts_with(b"true") {
        blocked_reason = Some("Коммит sparse checkout пока выполняется через Git".into())
    }
    let listed = git_strings(root, &["ls-files", "--stage", "-z"], deadline)?;
    ensure!(listed.success, "Не удалось прочитать Git index");
    let mut entries = BTreeMap::new();
    for entry in listed.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let tab = entry
            .iter()
            .position(|b| *b == b'\t')
            .context("Некорректный Git index")?;
        let (meta, path) = (&entry[..tab], &entry[tab + 1..]);
        let meta = std::str::from_utf8(meta)?.split(' ').collect::<Vec<_>>();
        ensure!(meta.len() == 3, "Некорректный Git index");
        if meta[2] != "0" {
            blocked_reason = Some("Сначала разрешите конфликты Git".into())
        }
        entries.insert(std::str::from_utf8(path)?.to_owned(), meta[0].to_owned());
    }
    let mut status_args = filter_overrides(root, deadline)?;
    status_args.extend(args(&[
        "-c",
        "status.renames=false",
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=all",
    ]));
    let status_refs = status_args.iter().map(String::as_str).collect::<Vec<_>>();
    let status = git_strings(root, &status_refs, deadline)?;
    ensure!(status.success, "Не удалось прочитать изменённые файлы");
    let all_files = parse_status(&status.stdout)?;
    let filemode = git_strings(root, &["config", "--bool", "core.filemode"], deadline)?;
    let honor_mode = !filemode.success || !filemode.stdout.starts_with(b"false");
    let mut selected = Vec::new();
    let mut displayed = Vec::new();
    let mut total = 0;
    let mut digest = Sha256::new();
    for bytes in [
        b"mpac-commit-v1".as_slice(),
        canonical.as_os_str().as_encoded_bytes(),
        &root_identity,
        &git_identity,
        before_head.as_bytes(),
        branch.as_deref().unwrap_or("").as_bytes(),
        index.as_deref().unwrap_or(&[]),
    ] {
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    for path in &paths {
        let status = all_files
            .iter()
            .find(|f| &f.path == path)
            .context("Файл больше не входит в изменённые: обновите список")?
            .status
            .clone();
        if status != "??" && status.as_bytes()[0] != b' ' && status.as_bytes()[1] != b' ' {
            blocked_reason = Some(format!(
                "Файл «{path}» подготовлен частично. Завершите staging через Git или выберите другой файл"
            ));
        }
        let relative = relative_path(path)?;
        for ancestor in relative.ancestors().filter(|p| !p.as_os_str().is_empty()) {
            if let Ok(meta) = directory.symlink_metadata(ancestor) {
                ensure!(
                    !meta.file_type().is_symlink(),
                    "Коммит путей с symlink пока выполняется через Git"
                );
            }
        }
        let (bytes, mut mode) = match directory.symlink_metadata(&relative) {
            Ok(meta) => {
                ensure!(
                    meta.is_file(),
                    "В коммит можно добавить только обычные файлы"
                );
                let bytes = read_bytes(&directory, &relative, super::MAX_FILE_BYTES)?;
                total += bytes.len();
                ensure!(
                    total <= MAX_BYTES,
                    "Слишком большой состав коммита: выберите меньше файлов"
                );
                let mut mode = entries
                    .get(path)
                    .cloned()
                    .unwrap_or_else(|| "100644".into());
                #[cfg(unix)]
                if honor_mode {
                    use cap_std::fs::PermissionsExt;
                    mode = if meta.permissions().mode() & 0o111 != 0 {
                        "100755"
                    } else {
                        "100644"
                    }
                    .into();
                }
                (Some(bytes), mode)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (
                None,
                entries
                    .get(path)
                    .cloned()
                    .unwrap_or_else(|| "100644".into()),
            ),
            Err(e) => return Err(e.into()),
        };
        if !matches!(mode.as_str(), "100644" | "100755") {
            blocked_reason = Some("Коммит submodule или symlink пока выполняется через Git".into());
            mode = "100644".into()
        }
        digest.update((path.len() as u64).to_le_bytes());
        digest.update(path.as_bytes());
        digest.update(mode.as_bytes());
        digest.update([u8::from(bytes.is_some())]);
        if let Some(bytes) = &bytes {
            digest.update((bytes.len() as u64).to_le_bytes());
            digest.update(bytes)
        }
        let diff = frozen_diff(root, &before_head, path, bytes.as_deref(), &mode, deadline)?;
        digest.update((diff.len() as u64).to_le_bytes());
        digest.update(diff.as_bytes());
        displayed.push(CommitFile {
            path: path.clone(),
            status,
            diff,
        });
        selected.push(CapturedFile {
            path: path.clone(),
            bytes,
            mode,
        });
    }
    ensure!(
        identity(root)? == root_identity && identity(&git_dir)? == git_identity,
        "Папка Git изменилась во время чтения"
    );
    ensure!(
        head(root, deadline)? == before_head && read_index(&git_dir.join("index"))? == index,
        "Git уже изменился: обновите состав коммита"
    );
    let diff = displayed
        .iter()
        .map(|f| f.diff.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    ensure!(
        diff.len() <= 2 * 1024 * 1024,
        "Предпросмотр слишком большой: выберите меньше файлов"
    );
    Ok(Capture {
        preview: CommitPreview {
            head: before_head,
            branch,
            snapshot: format!("{:x}", digest.finalize()),
            paths,
            files: displayed,
            diff,
            blocked_reason,
        },
        files: selected,
        git_dir,
        index,
    })
}

pub fn commit_preview(root: &Path, paths: &[String]) -> Result<CommitPreview> {
    Ok(capture(root, paths)?.preview)
}

struct Work {
    path: PathBuf,
    keep: bool,
}
impl Drop for Work {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
impl Work {
    fn new(directory: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let name = format!(
            ".mpac-commit-{}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = directory.join(name);
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { path, keep: false })
    }
}
struct IndexLock {
    path: PathBuf,
    file: Option<File>,
    identity: fs::Metadata,
}
impl IndexLock {
    fn acquire(index: &Path) -> Result<Self> {
        let path = index.with_file_name("index.lock");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .context("Git index занят другой операцией. Повторите позже")?;
        let identity = file.metadata()?;
        Ok(Self {
            path,
            file: Some(file),
            identity,
        })
    }
    fn owned(&self) -> bool {
        let Ok(meta) = fs::symlink_metadata(&self.path) else {
            return false;
        };
        if !meta.is_file() || meta.file_type().is_symlink() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            meta.dev() == self.identity.dev() && meta.ino() == self.identity.ino()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
    fn publish(&mut self, bytes: &[u8], index: &Path) -> Result<()> {
        ensure!(self.owned(), "Git index lock заменён другой операцией");
        let file = self.file.as_mut().context("Git index lock недоступен")?;
        if let Ok(meta) = fs::metadata(index) {
            file.set_permissions(meta.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        ensure!(self.owned(), "Git index lock заменён другой операцией");
        self.file.take();
        fs::rename(&self.path, index)?;
        Ok(())
    }
}
impl Drop for IndexLock {
    fn drop(&mut self) {
        self.file.take();
        if self.owned() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct Run {
    success: bool,
    stdout: Vec<u8>,
    stderr: String,
}
fn run(
    root: &Path,
    args: &[String],
    env: &[(String, String)],
    input: Option<Vec<u8>>,
    deadline: Instant,
) -> Result<Run> {
    ensure!(Instant::now() < deadline, "Git не завершился вовремя");
    let cwd = open_root(root)?;
    let mut command = Command::new("git");
    command
        .args([
            "--no-pager",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "core.pager=",
            "-c",
            "color.ui=false",
        ])
        .args(args)
        .current_dir(root);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_PAGER", "")
        .env("GIT_EDITOR", ":")
        .env("LC_ALL", "C")
        .envs(env.iter().cloned());
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::{os::fd::AsRawFd, os::unix::process::CommandExt};
        let fd = cwd.as_raw_fd();
        unsafe {
            command.pre_exec(move || {
                if libc::fchdir(fd) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut wrapped = CommandWrap::from(command);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::std::ProcessGroup::leader());
    #[cfg(windows)]
    wrapped.wrap(process_wrap::std::JobObject);
    let mut child = wrapped.spawn().context("Не удалось запустить Git")?;
    drop(cwd);
    if let Some(input) = input {
        let mut stdin = child.stdin().take().context("Git stdin недоступен")?;
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let stdout = child.stdout().take().context("Git stdout недоступен")?;
    let stderr = child.stderr().take().context("Git stderr недоступен")?;
    let (tx, rx) = mpsc::channel();
    let out = tx.clone();
    thread::spawn(move || {
        let _ = out.send((true, super::read_limited(stdout, MAX_BYTES)));
    });
    thread::spawn(move || {
        let _ = tx.send((false, super::read_limited(stderr, MAX_BYTES)));
    });
    let result = (|| -> Result<Run> {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        for _ in 0..2 {
            let (is_out, bytes) = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .context("Git не завершился вовремя")?;
            let bytes = bytes.context("Слишком большой вывод Git")?;
            if is_out {
                stdout = bytes
            } else {
                stderr = bytes
            }
        }
        loop {
            if let Some(status) = child.inner_mut().try_wait()? {
                return Ok(Run {
                    success: status.success(),
                    stdout,
                    stderr: String::from_utf8_lossy(&stderr)
                        .chars()
                        .filter(|c| !c.is_control() || *c == '\n')
                        .take(4000)
                        .collect(),
                });
            }
            ensure!(Instant::now() < deadline, "Git не завершился вовремя");
            thread::sleep(Duration::from_millis(5));
        }
    })();
    if result.is_err() {
        let _ = child.start_kill();
        thread::spawn(move || {
            let _ = child.inner_mut().wait();
        });
    }
    result
}
fn ok(
    root: &Path,
    args: &[String],
    env: &[(String, String)],
    input: Option<Vec<u8>>,
    deadline: Instant,
) -> Result<Vec<u8>> {
    let output = run(root, args, env, input, deadline)?;
    ensure!(output.success, "Git: {}", output.stderr);
    Ok(output.stdout)
}
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).to_owned()).collect()
}

#[cfg(unix)]
fn hook_wrappers(
    root: &Path,
    work: &Path,
    expected_tree: &str,
    expected_head: &str,
    deadline: Instant,
) -> Result<Vec<(String, String)>> {
    use std::os::unix::fs::PermissionsExt;
    // The shared read helper intentionally overrides hooksPath=/dev/null.
    // This read-only builtin must inspect the actual configured hook location.
    let hook_output = ok(
        root,
        &args(&["rev-parse", "--git-path", "hooks"]),
        &[],
        None,
        deadline,
    )?;
    let hook_text = String::from_utf8(hook_output)?;
    let hook_path = hook_text
        .strip_suffix('\n')
        .unwrap_or(&hook_text)
        .to_owned();
    let hooks = if Path::new(&hook_path).is_absolute() {
        PathBuf::from(hook_path)
    } else {
        root.join(hook_path)
    };
    let wrappers = work.join("hooks");
    fs::create_dir(&wrappers)?;
    let script = "#!/bin/sh\nname=${0##*/}\nif test \"$name\" = post-commit; then\n git rev-parse --verify HEAD >\"$MPAC_COMMIT_RECEIPT\" || exit 1\nfi\nif test -x \"$MPAC_REAL_HOOKS/$name\"; then\n \"$MPAC_REAL_HOOKS/$name\" \"$@\"\n result=$?\n if test \"$result\" != 0; then\n  if test \"$name\" = post-commit; then printf \"%s\" \"$result\" >\"$MPAC_POST_HOOK_ERROR\"; fi\n  exit \"$result\"\n fi\nfi\ncase \"$name\" in\n post-commit) exit 0 ;;\nesac\nactual=$(git -c core.fsmonitor=false write-tree) || exit 1\nif test \"$actual\" != \"$MPAC_EXPECTED_TREE\"; then\n echo 'MPAC: выбранные изменения изменились в Git hook; обновите предпросмотр' >&2\n exit 1\nfi\ncurrent=$(git rev-parse --verify HEAD 2>/dev/null) || current=unborn\nif test \"$current\" != \"$MPAC_EXPECTED_HEAD\"; then\n echo 'MPAC: HEAD изменился во время коммита; обновите предпросмотр' >&2\n exit 1\nfi\n";
    for name in [
        "pre-commit",
        "prepare-commit-msg",
        "commit-msg",
        "post-commit",
    ] {
        let file = wrappers.join(name);
        fs::write(&file, script)?;
        fs::set_permissions(file, fs::Permissions::from_mode(0o700))?;
    }
    Ok(vec![
        (
            "MPAC_REAL_HOOKS".into(),
            hooks
                .to_str()
                .context("Путь hooks должен поддерживать UTF-8")?
                .into(),
        ),
        ("MPAC_EXPECTED_TREE".into(), expected_tree.into()),
        ("MPAC_EXPECTED_HEAD".into(), expected_head.into()),
        (
            "MPAC_COMMIT_RECEIPT".into(),
            work.join("created-head")
                .to_str()
                .context("Путь должен поддерживать UTF-8")?
                .into(),
        ),
        (
            "MPAC_POST_HOOK_ERROR".into(),
            work.join("post-hook-error")
                .to_str()
                .context("Путь должен поддерживать UTF-8")?
                .into(),
        ),
    ])
}

pub fn commit_selected(
    root: &Path,
    paths: &[String],
    message: &str,
    expected_snapshot: &str,
) -> Result<CommitResult> {
    ensure!(
        !message.trim().is_empty() && message.len() <= 16000 && !message.contains('\0'),
        "Укажите сообщение коммита до 16000 байт"
    );
    let before = capture(root, paths)?;
    ensure!(
        before.preview.snapshot == expected_snapshot,
        "Состав коммита уже изменился. Обновите предпросмотр"
    );
    ensure!(
        before.preview.blocked_reason.is_none(),
        "{}",
        before
            .preview
            .blocked_reason
            .as_deref()
            .unwrap_or("Коммит недоступен")
    );
    #[cfg(not(unix))]
    {
        bail!("Коммиты через интерфейс пока доступны на macOS/Linux");
    }
    #[cfg(unix)]
    {
        let deadline = Instant::now() + WRITE_BUDGET;
        let index_path = before.git_dir.join("index");
        let mut lock = IndexLock::acquire(&index_path)?;
        ensure!(
            capture(root, paths)?.preview.snapshot == expected_snapshot,
            "Git изменился: обновите предпросмотр"
        );
        let mut work = Work::new(&before.git_dir)?;
        let private = work.path.join("index");
        let expected = work.path.join("expected-index");
        let pathspec = work.path.join("paths");
        if let Some(bytes) = &before.index {
            fs::write(&private, bytes)?
        }
        let env_index = |path: &Path| -> Result<Vec<(String, String)>> {
            Ok(vec![(
                "GIT_INDEX_FILE".into(),
                path.to_str()
                    .context("Путь index должен поддерживать UTF-8")?
                    .into(),
            )])
        };
        let private_env = env_index(&private)?;
        let expected_env = env_index(&expected)?;
        let initialize = if before.preview.head == "unborn" {
            args(&["read-tree", "--empty"])
        } else {
            args(&["read-tree", &before.preview.head])
        };
        ok(root, &initialize, &expected_env, None, deadline)?;
        if before.index.is_none() {
            ok(
                root,
                &args(&["read-tree", "--empty"]),
                &private_env,
                None,
                deadline,
            )?;
        }
        let safe_filters = filter_overrides(root, deadline)?;
        let object_format = output_text(root, &["rev-parse", "--show-object-format"], deadline)?;
        let object_id_len = if object_format == "sha256" { 64 } else { 40 };
        for file in &before.files {
            let line = if let Some(bytes) = &file.bytes {
                let mut command = safe_filters.clone();
                command.extend(args(&["hash-object", "-w", "--stdin"]));
                command.push(format!("--path={}", file.path));
                let oid = String::from_utf8(ok(
                    root,
                    &command,
                    &expected_env,
                    Some(bytes.clone()),
                    deadline,
                )?)?
                .trim()
                .to_owned();
                ensure!(
                    matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit()),
                    "Git вернул некорректный object ID"
                );
                format!("{} {}\t{}\0", file.mode, oid, file.path)
            } else {
                format!("0 {}\t{}\0", "0".repeat(object_id_len), file.path)
            };
            ok(
                root,
                &args(&["update-index", "-z", "--index-info"]),
                &expected_env,
                Some(line.as_bytes().to_vec()),
                deadline,
            )?;
            // Install the reviewed blob with an empty stat cache. Merely
            // copying an index with a fresh mtime can hide equal-length writes
            // from Git's racy-clean check; native --only must reread this path.
            ok(
                root,
                &args(&["update-index", "-z", "--index-info"]),
                &private_env,
                Some(line.into_bytes()),
                deadline,
            )?;
        }
        let tree = String::from_utf8(ok(
            root,
            &args(&["write-tree"]),
            &expected_env,
            None,
            deadline,
        )?)?
        .trim()
        .to_owned();
        let mut payload = Vec::new();
        for path in &before.preview.paths {
            payload.extend_from_slice(path.as_bytes());
            payload.push(0)
        }
        fs::write(&pathspec, payload)?;
        let pathspec_arg = format!(
            "--pathspec-from-file={}",
            pathspec
                .to_str()
                .context("Путь должен поддерживать UTF-8")?
        );
        ensure!(
            capture(root, paths)?.preview.snapshot == expected_snapshot,
            "Выбранные файлы изменились: обновите предпросмотр"
        );
        let mut env = private_env;
        env.extend(hook_wrappers(
            root,
            &work.path,
            &tree,
            &before.preview.head,
            deadline,
        )?);
        let wrappers = work.path.join("hooks");
        let mut command = args(&[
            "-c",
            &format!(
                "core.hooksPath={}",
                wrappers
                    .to_str()
                    .context("Путь hooks должен поддерживать UTF-8")?
            ),
            "commit",
            "--only",
            "--no-edit",
            "--pathspec-file-nul",
        ]);
        command.push(pathspec_arg);
        command.extend(args(&["-m", message]));
        let outcome = run(root, &command, &env, None, deadline);
        let current = head(root, Instant::now() + READ_BUDGET).ok();
        let receipt = fs::read_to_string(work.path.join("created-head"))
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit()));
        if receipt.is_none()
            && current.as_deref() == Some(before.preview.head.as_str())
            && !outcome.as_ref().is_ok_and(|o| o.success)
        {
            let output = outcome?;
            bail!("Git не создал коммит: {}", output.stderr)
        }
        // From here onward never return a retryable error: a commit may exist.
        let committed = receipt.is_some() || outcome.as_ref().is_ok_and(|o| o.success);
        let new_head = receipt
            .or_else(|| current.clone())
            .unwrap_or_else(|| before.preview.head.clone());
        let mut result = CommitResult {
            committed,
            head: new_head.clone(),
            previous_head: before.preview.head.clone(),
            paths: before.preview.paths.clone(),
            warning: None,
            recovery_index: None,
        };
        let post_hook_failed = work.path.join("post-hook-error").is_file();
        work.keep = true;
        let verify = (|| -> Result<()> {
            ensure!(
                committed,
                "Исход коммита не подтверждён; проверьте историю Git перед повтором"
            );
            ensure!(
                current.as_deref() == Some(new_head.as_str()),
                "HEAD изменён после создания коммита"
            );
            let parent = output_text(
                root,
                &["rev-list", "--parents", "-n", "1", &new_head],
                Instant::now() + READ_BUDGET,
            )?;
            let parts = parent.split_whitespace().collect::<Vec<_>>();
            ensure!(
                parts.first().copied() == Some(new_head.as_str())
                    && (if before.preview.head == "unborn" {
                        parts.len() == 1
                    } else {
                        parts.len() == 2 && parts[1] == before.preview.head
                    }),
                "HEAD изменён другой операцией; проверьте историю коммитов"
            );
            let actual_tree = output_text(
                root,
                &["rev-parse", &format!("{new_head}^{{tree}}")],
                Instant::now() + READ_BUDGET,
            )?;
            ensure!(
                actual_tree == tree,
                "Состав созданного коммита отличается от предпросмотра"
            );
            ensure!(
                read_index(&index_path)? == before.index,
                "Git index изменён внешней операцией"
            );
            let bytes =
                read_index(&private)?.context("Не удалось прочитать подготовленный index")?;
            lock.publish(&bytes, &index_path)?;
            Ok(())
        })();
        if let Err(error) = verify {
            result.warning = Some(format!(
                "HEAD изменён; не повторяйте коммит автоматически. {error}. Исходный index сохранён; доступен index для восстановления"
            ));
            result.recovery_index = Some(private.to_string_lossy().into_owned());
        } else {
            work.keep = false;
            if post_hook_failed {
                result.warning=Some("Коммит создан, но post-commit hook завершился с ошибкой. Повторять коммит не нужно".into());
            }
            match outcome {
                Ok(output) if !output.success => {
                    result.warning = Some(format!(
                        "Коммит создан, но Git сообщил ошибку после создания: {}",
                        output.stderr
                    ))
                }
                Err(error) => {
                    result.warning = Some(format!(
                        "Коммит создан; завершение Git не подтверждено: {error}"
                    ))
                }
                _ => {}
            }
        }
        Ok(result)
    }
}
