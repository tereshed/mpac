//! Read-only project artifacts and Git reviews for the local prototype.
//!
//! Reads use a directory capability; relative paths cannot escape it through
//! traversal or symlinks. Limits: 512 directory entries, 1 MiB text file, 256
//! changed paths, 8 MiB review content, 2 MiB displayed diff, 8 seconds Git work.
//! Truncated reviews are readable but cannot pass validate_review. This is not
//! an atomic filesystem snapshot; dispatch must revalidate immediately before use.
//! On Unix final-component root symlinks are rejected and each Git command pins
//! its working directory via fchdir. Replacement/rename of ancestor directories
//! across separate operations still needs a future persistent root handle.

use std::{
    ffi::OsStr,
    io::{Read, Result as IoResult},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use cap_std::fs::Dir;
use process_wrap::std::CommandWrap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod branches;
mod commits;
pub use commits::{CommitFile, CommitPreview, CommitResult, commit_preview, commit_selected};
pub mod review_lines;
pub use branches::{Branch, BranchState, branch_state, repository_root, switch_branch};

pub const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 512;
const MAX_CHANGED_FILES: usize = 256;
const MAX_REVIEW_CONTENT: usize = 8 * 1024 * 1024;
const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;
const MAX_GIT_OUTPUT: usize = 8 * 1024 * 1024;
const GIT_BUDGET: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Root {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub head: String,
    pub snapshot: String,
    pub diff: String,
    pub files: Vec<ChangedFile>,
    pub truncated: bool,
}

fn relative_path(relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    ensure!(
        !path.is_absolute()
            && !path.components().any(|c| matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )),
        "path_must_stay_inside_root"
    );
    Ok(if relative.is_empty() {
        PathBuf::from(".")
    } else {
        path.to_path_buf()
    })
}

fn open_root(root: &Path) -> Result<Dir> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NOFOLLOW protects the root itself after the API's canonical-path
        // check. cap-std then confines all subsequent relative resolution.
        let normalized: PathBuf = root.components().collect();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(normalized)
            .context("project_root_unavailable")?;
        Ok(Dir::from_std_file(file))
    }
    #[cfg(not(unix))]
    Dir::open_ambient_dir(root, cap_std::ambient_authority()).context("project_root_unavailable")
}

pub fn list_files(root: &Path, relative: &str) -> Result<Vec<FileEntry>> {
    let relative = relative_path(relative)?;
    let dir = open_root(root)?;
    let entries = dir.read_dir(&relative).context("directory_unavailable")?;
    let mut result = Vec::new();
    for (index, entry) in entries.enumerate() {
        ensure!(index < MAX_DIRECTORY_ENTRIES, "directory_entry_limit");
        let entry = entry.context("directory_entry_unavailable")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name == ".git" {
            continue;
        }
        let kind = entry.file_type().context("directory_entry_unavailable")?;
        // Symlinks are intentionally omitted from the tree. Direct artifact
        // reads still resolve in-root relative symlinks through the capability.
        if !kind.is_file() && !kind.is_dir() {
            continue;
        }
        let metadata = entry.metadata().context("directory_entry_unavailable")?;
        let path = relative.join(name);
        let path = path.strip_prefix(".").unwrap_or(&path);
        result.push(FileEntry {
            path: path
                .components()
                .filter_map(|component| match component {
                    Component::Normal(name) => name.to_str(),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/"),
            is_dir: kind.is_dir(),
            size: metadata.len(),
        });
    }
    result.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.path.cmp(&b.path)));
    Ok(result)
}

fn read_bytes(dir: &Dir, relative: &Path, limit: usize) -> Result<Vec<u8>> {
    let metadata = dir.metadata(relative).context("file_unavailable")?;
    ensure!(metadata.is_file(), "not_regular_file");
    ensure!(metadata.len() <= limit as u64, "file_size_limit");
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        // A raced replacement with a FIFO must not block an HTTP worker.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = dir
        .open_with(relative, &options)
        .context("file_unavailable")?;
    ensure!(
        file.metadata().context("file_unavailable")?.is_file(),
        "not_regular_file"
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .context("file_read_failed")?;
    ensure!(bytes.len() <= limit, "file_size_limit");
    Ok(bytes)
}

pub fn read_text(root: &Path, relative: &str) -> Result<String> {
    let relative = relative_path(relative)?;
    let bytes = read_bytes(&open_root(root)?, &relative, MAX_FILE_BYTES)?;
    ensure!(!bytes.contains(&0), "file_not_utf8_text");
    String::from_utf8(bytes).context("file_not_utf8_text")
}

struct GitOutput {
    success: bool,
    exit_code: Option<i32>,
    stdout: Vec<u8>,
}

fn read_limited(reader: impl Read, max: usize) -> IoResult<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(max as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(std::io::Error::other("git_output_limit"));
    }
    Ok(bytes)
}

/// Fixed builtins only, plus the explicitly guarded local switch operation.
/// Both pipes are drained concurrently and the
/// process group is killed before reaping on timeout/overflow. Reader threads
/// are not joined while a descendant could still hold their pipe open.
fn git(root: &Path, args: &[&OsStr], deadline: Instant) -> Result<GitOutput> {
    ensure!(Instant::now() < deadline, "git_timeout");
    let cwd = open_root(root)?;
    let mut command = Command::new("git");
    command
        .args([
            "--no-pager",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.pager=",
            "-c",
            "diff.external=",
            "-c",
            "core.quotePath=false",
            "-c",
            "color.ui=false",
            "-c",
            "diff.submodule=short",
        ])
        .args(args)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::{os::fd::AsRawFd, os::unix::process::CommandExt};
        let cwd_fd = cwd.as_raw_fd();
        // cwd remains alive until spawn completes. Only async-signal-safe
        // fchdir runs between fork and exec, after Command's path-based chdir.
        unsafe {
            command.pre_exec(move || {
                if libc::fchdir(cwd_fd) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    // Do not inherit process-specific Git routing or command injection from the
    // launcher. Account/SSH configuration is unnecessary for these local reads.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    let mut wrapped = CommandWrap::from(command);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::std::ProcessGroup::leader());
    #[cfg(windows)]
    wrapped.wrap(process_wrap::std::JobObject);
    let mut child = wrapped.spawn().context("git_spawn_failed")?;
    drop(cwd);
    let stdout = child.stdout().take().context("git_stdout_missing")?;
    let stderr = child.stderr().take().context("git_stderr_missing")?;
    let (tx, rx) = mpsc::channel();
    let out_tx = tx.clone();
    thread::spawn(move || {
        let _ = out_tx.send((true, read_limited(stdout, MAX_GIT_OUTPUT)));
    });
    thread::spawn(move || {
        let _ = tx.send((false, read_limited(stderr, MAX_GIT_OUTPUT)));
    });
    let result = (|| -> Result<GitOutput> {
        let mut output = Vec::new();
        for _ in 0..2 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (is_stdout, bytes) = rx.recv_timeout(remaining).context("git_timeout")?;
            let bytes = bytes.context("git_output_limit")?;
            if is_stdout {
                output = bytes;
            }
        }
        loop {
            if let Some(status) = child.inner_mut().try_wait().context("git_wait_failed")? {
                return Ok(GitOutput {
                    success: status.success(),
                    exit_code: status.code(),
                    stdout: output,
                });
            }
            ensure!(Instant::now() < deadline, "git_timeout");
            thread::sleep(Duration::from_millis(5));
        }
    })();
    if result.is_err() {
        let _ = child.start_kill();
        let cleanup_deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < cleanup_deadline {
            match child.inner_mut().try_wait().ok().flatten() {
                Some(_) => return result,
                None => thread::sleep(Duration::from_millis(5)),
            }
        }
        // Never block the caller indefinitely, even on an exceptional native
        // reap failure. The group has been signalled; a background reaper owns
        // the remaining child and Job Object handles until it finishes.
        thread::spawn(move || {
            let _ = child.inner_mut().wait();
        });
    }
    result
}

fn git_strings(root: &Path, args: &[&str], deadline: Instant) -> Result<GitOutput> {
    if matches!(args.first(), Some(&"status" | &"diff")) {
        let mut configured = branches::filter_overrides(root, deadline)?;
        configured.extend(args.iter().map(|s| (*s).to_owned()));
        let configured: Vec<_> = configured.iter().map(OsStr::new).collect();
        return git(root, &configured, deadline);
    }
    let args: Vec<_> = args.iter().map(OsStr::new).collect();
    git(root, &args, deadline)
}

fn head(root: &Path, deadline: Instant) -> Result<String> {
    let output = git_strings(root, &["rev-parse", "--verify", "HEAD"], deadline)?;
    Ok(if output.success {
        String::from_utf8(output.stdout)
            .context("git_invalid_head")?
            .trim()
            .to_owned()
    } else {
        "unborn".to_owned()
    })
}

fn parse_status(bytes: &[u8]) -> Result<Vec<ChangedFile>> {
    let mut result = Vec::new();
    let mut records = bytes.split(|b| *b == 0).filter(|s| !s.is_empty());
    while let Some(record) = records.next() {
        ensure!(record.len() >= 4 && record[2] == b' ', "git_invalid_status");
        let status = std::str::from_utf8(&record[..2]).context("git_invalid_status")?;
        let path = std::str::from_utf8(&record[3..]).context("git_filename_not_utf8")?;
        relative_path(path)?;
        result.push(ChangedFile {
            path: path.to_owned(),
            status: status.to_owned(),
        });
        if status.contains('R') || status.contains('C') {
            ensure!(records.next().is_some(), "git_invalid_status");
        }
    }
    Ok(result)
}

fn push_diff(out: &mut Vec<u8>, bytes: &[u8], truncated: &mut bool) {
    let count = bytes.len().min(MAX_DIFF_BYTES.saturating_sub(out.len()));
    out.extend_from_slice(&bytes[..count]);
    *truncated |= count < bytes.len();
}

pub fn git_review(root: &Path) -> Result<Review> {
    // Reject a substituted final-root symlink before canonicalization could
    // erase that evidence. All Git operations repeat the pinned open.
    let dir = open_root(root)?;
    let root = root.canonicalize().context("project_root_unavailable")?;
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        let opened = dir.dir_metadata().context("project_root_unavailable")?;
        let current = open_root(&root)?
            .dir_metadata()
            .context("project_root_unavailable")?;
        ensure!(
            opened.dev() == current.dev() && opened.ino() == current.ino(),
            "project_root_changed"
        );
    }
    let deadline = Instant::now() + GIT_BUDGET;
    let top = git_strings(&root, &["rev-parse", "--show-toplevel"], deadline)?;
    ensure!(top.success, "not_git_repository");
    let top = String::from_utf8(top.stdout).context("git_root_not_utf8")?;
    let top = PathBuf::from(top.strip_suffix('\n').unwrap_or(&top));
    ensure!(
        top.canonicalize().context("git_root_unavailable")? == root,
        "root_must_be_repository_root"
    );
    let head = head(&root, deadline)?;
    let status_args = [
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=dirty",
    ];
    let status = git_strings(&root, &status_args, deadline)?;
    ensure!(status.success, "git_status_failed");
    let all_files = parse_status(&status.stdout)?;
    let staged = git_strings(
        &root,
        &[
            "diff",
            "--cached",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=dirty",
            "--",
        ],
        deadline,
    )?;
    let working = git_strings(
        &root,
        &[
            "diff",
            "--binary",
            "--full-index",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=dirty",
            "--",
        ],
        deadline,
    )?;
    ensure!(staged.success && working.success, "git_diff_failed");
    let mut hash = Sha256::new();
    hash.update(b"mpac-review-v1\0");
    let root_identity = root.as_os_str().as_encoded_bytes();
    hash.update((root_identity.len() as u64).to_le_bytes());
    hash.update(root_identity);
    for bytes in [
        head.as_bytes(),
        &status.stdout,
        &staged.stdout,
        &working.stdout,
    ] {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let mut truncated = all_files.len() > MAX_CHANGED_FILES;
    let files: Vec<_> = all_files.into_iter().take(MAX_CHANGED_FILES).collect();
    let mut diff = Vec::new();
    if !staged.stdout.is_empty() {
        push_diff(&mut diff, b"# Staged changes\n\n", &mut truncated);
        push_diff(&mut diff, &staged.stdout, &mut truncated);
    }
    if !working.stdout.is_empty() {
        push_diff(&mut diff, b"\n# Working tree changes\n\n", &mut truncated);
        push_diff(&mut diff, &working.stdout, &mut truncated);
    }
    let mut content_bytes = 0;
    for changed in &files {
        ensure!(Instant::now() < deadline, "git_timeout");
        let relative = relative_path(&changed.path)?;
        hash.update(changed.path.as_bytes());
        // A Git symlink is reviewed as a link, never by dereferencing its target.
        let metadata = dir.symlink_metadata(&relative).ok();
        #[cfg(unix)]
        let mut executable = false;
        #[cfg(not(unix))]
        let executable = false;
        if let Some(metadata) = &metadata {
            #[cfg(unix)]
            {
                use cap_std::fs::PermissionsExt;
                let mode = metadata.permissions().mode();
                hash.update(mode.to_le_bytes());
                executable = mode & 0o111 != 0;
            }
            #[cfg(not(unix))]
            hash.update([metadata.permissions().readonly() as u8]);
        }
        if metadata
            .as_ref()
            .is_some_and(|m| m.file_type().is_symlink())
        {
            let link = dir
                .read_link_contents(&relative)
                .context("review_link_unavailable")?;
            hash.update(link.as_os_str().as_encoded_bytes());
            if changed.status == "??" {
                let quoted = serde_json::to_string(&format!("b/{}", changed.path))?;
                let link = serde_json::to_string(&link.to_string_lossy())?;
                let header = format!(
                    "\ndiff --git {quoted} {quoted}\nnew file mode 120000\n--- /dev/null\n+++ {quoted}\n@@ -0,0 +1 @@\n+{link}\n"
                );
                push_diff(&mut diff, header.as_bytes(), &mut truncated);
            }
            continue;
        }
        if metadata.is_none() {
            continue;
        } // A deletion.
        if metadata.as_ref().is_some_and(|m| !m.is_file()) {
            if !metadata.as_ref().is_some_and(|m| m.is_dir()) {
                truncated = true;
            }
            continue;
        }
        let remaining = MAX_REVIEW_CONTENT.saturating_sub(content_bytes);
        let bytes = match read_bytes(&dir, &relative, MAX_FILE_BYTES.min(remaining)) {
            Ok(bytes) => bytes,
            Err(error) if error.to_string() == "file_size_limit" => {
                truncated = true;
                continue;
            }
            Err(_) => bail!("review_file_changed_or_unavailable"),
        };
        content_bytes += bytes.len();
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
        if changed.status != "??" {
            continue;
        }
        let quoted = serde_json::to_string(&format!("b/{}", changed.path))?;
        let mode = if executable { "100755" } else { "100644" };
        let header = format!(
            "\ndiff --git {quoted} {quoted}\nnew file mode {mode}\n--- /dev/null\n+++ {quoted}\n"
        );
        push_diff(&mut diff, header.as_bytes(), &mut truncated);
        match std::str::from_utf8(&bytes)
            .ok()
            .filter(|_| !bytes.contains(&0))
        {
            Some(text) => {
                let lines = text.lines().count();
                push_diff(
                    &mut diff,
                    format!("@@ -0,0 +1,{lines} @@\n").as_bytes(),
                    &mut truncated,
                );
                for line in text.lines() {
                    push_diff(&mut diff, b"+", &mut truncated);
                    push_diff(&mut diff, line.as_bytes(), &mut truncated);
                    push_diff(&mut diff, b"\n", &mut truncated);
                }
                if !text.is_empty() && !text.ends_with('\n') {
                    push_diff(&mut diff, b"\\ No newline at end of file\n", &mut truncated);
                }
            }
            None => push_diff(
                &mut diff,
                b"Binary file (content included in snapshot)\n",
                &mut truncated,
            ),
        }
    }
    let latest = git_strings(&root, &status_args, deadline)?;
    ensure!(
        latest.success && latest.stdout == status.stdout && self::head(&root, deadline)? == head,
        "review_changed_during_capture"
    );
    Ok(Review {
        head,
        snapshot: format!("{:x}", hash.finalize()),
        diff: String::from_utf8_lossy(&diff).into_owned(),
        files,
        truncated,
    })
}

pub fn validate_review(root: &Path, snapshot: &str) -> Result<()> {
    ensure!(
        snapshot.len() == 64 && snapshot.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid_review_snapshot"
    );
    let current = git_review(root)?;
    ensure!(!current.truncated, "truncated_review_cannot_dispatch");
    ensure!(current.snapshot == snapshot, "review_snapshot_stale");
    Ok(())
}
