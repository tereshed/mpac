use mpac_workspace::{commit_preview, commit_selected};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture {
    root: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
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
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
fn text(root: &Path, args: &[&str]) -> String {
    String::from_utf8(git(root, args))
        .unwrap()
        .trim_end_matches('\n')
        .to_owned()
}
impl Fixture {
    fn new(initial: bool) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "mpac-commit-tests-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "MPAC Test"]);
        git(&root, &["config", "user.email", "test@localhost"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        git(
            &root,
            &[
                "config",
                "core.hooksPath",
                root.join(".git/hooks").to_str().unwrap(),
            ],
        );
        git(
            &root,
            &[
                "config",
                "core.excludesFile",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            ],
        );
        if initial {
            fs::write(root.join("a.txt"), "before a\n").unwrap();
            fs::write(root.join("b.txt"), "before b\n").unwrap();
            git(&root, &["add", "."]);
            git(&root, &["commit", "-q", "-m", "initial"]);
        }
        Self { root }
    }
    fn preview(&self, paths: &[&str]) -> mpac_workspace::CommitPreview {
        commit_preview(
            &self.root,
            &paths.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap()
    }
    fn commit(&self, paths: &[&str]) -> mpac_workspace::CommitResult {
        let preview = self.preview(paths);
        assert!(
            preview.blocked_reason.is_none(),
            "{:?}",
            preview.blocked_reason
        );
        commit_selected(
            &self.root,
            &preview.paths,
            "Own selected commit",
            &preview.snapshot,
        )
        .unwrap()
    }
    #[cfg(unix)]
    fn hook(&self, name: &str, script: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = self.root.join(".git/hooks").join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn selected_commit_preserves_unselected_staging_and_working_tree() {
    let f = Fixture::new(true);
    fs::write(f.root.join("a.txt"), "selected a\n").unwrap();
    fs::write(f.root.join("b.txt"), "staged b\n").unwrap();
    git(&f.root, &["add", "b.txt"]);
    fs::write(f.root.join("b.txt"), "new working b\n").unwrap();
    let staged = text(&f.root, &["show", ":b.txt"]);
    let working = fs::read(f.root.join("b.txt")).unwrap();
    let result = f.commit(&["a.txt"]);
    assert!(result.committed);
    assert!(result.warning.is_none(), "{:?}", result.warning);
    assert_eq!(text(&f.root, &["show", "HEAD:a.txt"]), "selected a");
    assert_eq!(text(&f.root, &["show", "HEAD:b.txt"]), "before b");
    assert_eq!(text(&f.root, &["show", ":b.txt"]), staged);
    assert_eq!(fs::read(f.root.join("b.txt")).unwrap(), working);
    assert!(!f.root.join(".git/index.lock").exists());
}

#[test]
fn untracked_deleted_and_literal_special_names_are_supported() {
    let f = Fixture::new(true);
    fs::remove_file(f.root.join("a.txt")).unwrap();
    let unusual = ":(glob)*\nnew file.txt";
    fs::write(f.root.join(unusual), "new bytes\n").unwrap();
    let result = f.commit(&["a.txt", unusual]);
    assert!(result.committed);
    assert!(result.warning.is_none(), "{:?}", result.warning);
    assert_eq!(
        text(&f.root, &["show", &format!("HEAD:{unusual}")]),
        "new bytes"
    );
    assert!(
        !text(&f.root, &["ls-tree", "--name-only", "HEAD"])
            .lines()
            .any(|p| p == "a.txt")
    );
    assert_eq!(text(&f.root, &["status", "--porcelain"]), "");
}

#[test]
fn first_commit_only_records_selected_new_files() {
    let f = Fixture::new(false);
    fs::write(f.root.join("first.txt"), "first\n").unwrap();
    fs::write(f.root.join("later.txt"), "later\n").unwrap();
    let result = f.commit(&["first.txt"]);
    assert!(result.committed);
    assert_eq!(result.previous_head, "unborn");
    assert!(result.warning.is_none(), "{:?}", result.warning);
    assert_eq!(
        text(&f.root, &["ls-tree", "--name-only", "HEAD"]),
        "first.txt"
    );
    assert_eq!(text(&f.root, &["status", "--porcelain"]), "?? later.txt");
}

#[test]
fn stale_content_index_and_partial_stage_are_rejected_without_mutation() {
    let f = Fixture::new(true);
    fs::write(f.root.join("a.txt"), "preview\n").unwrap();
    let preview = f.preview(&["a.txt"]);
    let old = text(&f.root, &["rev-parse", "HEAD"]);
    fs::write(f.root.join("a.txt"), "changed\n").unwrap();
    assert!(commit_selected(&f.root, &preview.paths, "stale", &preview.snapshot).is_err());
    assert_eq!(text(&f.root, &["rev-parse", "HEAD"]), old);
    let preview = f.preview(&["a.txt"]);
    fs::write(f.root.join("b.txt"), "staged elsewhere\n").unwrap();
    git(&f.root, &["add", "b.txt"]);
    assert!(commit_selected(&f.root, &preview.paths, "stale index", &preview.snapshot).is_err());
    git(&f.root, &["add", "a.txt"]);
    fs::write(f.root.join("a.txt"), "unstaged after partial stage\n").unwrap();
    let partial = f.preview(&["a.txt"]);
    assert!(partial.blocked_reason.is_some());
    let index = fs::read(f.root.join(".git/index")).unwrap();
    assert!(commit_selected(&f.root, &partial.paths, "partial", &partial.snapshot).is_err());
    assert_eq!(fs::read(f.root.join(".git/index")).unwrap(), index);
}

#[cfg(unix)]
#[test]
fn hooks_are_honored_and_mutating_index_hook_aborts_before_commit() {
    let f = Fixture::new(true);
    fs::write(f.root.join("a.txt"), "selected\n").unwrap();
    f.hook("pre-commit", "echo checked > hook-ran\nexit 1");
    let preview = f.preview(&["a.txt"]);
    assert!(commit_selected(&f.root, &preview.paths, "blocked hook", &preview.snapshot).is_err());
    assert!(f.root.join("hook-ran").exists());
    assert_eq!(text(&f.root, &["log", "-1", "--format=%s"]), "initial");
    f.hook(
        "pre-commit",
        "printf 'hook rewrite\\n' > a.txt\ngit add -- a.txt",
    );
    let preview = f.preview(&["a.txt"]);
    let index = fs::read(f.root.join(".git/index")).unwrap();
    assert!(commit_selected(&f.root, &preview.paths, "mutating hook", &preview.snapshot).is_err());
    assert_eq!(text(&f.root, &["log", "-1", "--format=%s"]), "initial");
    assert_eq!(fs::read(f.root.join(".git/index")).unwrap(), index);
    assert_eq!(
        fs::read_to_string(f.root.join("a.txt")).unwrap(),
        "hook rewrite\n"
    );
    let result = f.commit(&["a.txt"]);
    assert!(result.committed);
    assert!(result.warning.is_none(), "{:?}", result.warning);
}

#[cfg(unix)]
#[test]
fn post_commit_failure_or_ref_change_never_reports_retryable_failure() {
    let f = Fixture::new(true);
    fs::write(f.root.join("a.txt"), "selected\n").unwrap();
    f.hook("post-commit", "echo post-ran > post-ran\nexit 7");
    let result = f.commit(&["a.txt"]);
    assert!(result.committed);
    assert_eq!(text(&f.root, &["rev-parse", "HEAD"]), result.head);
    assert!(f.root.join("post-ran").exists());
    assert!(
        result
            .warning
            .as_deref()
            .is_some_and(|w| w.contains("post-commit"))
    );
    fs::write(f.root.join("a.txt"), "another selected\n").unwrap();
    let old = text(&f.root, &["rev-parse", "HEAD"]);
    f.hook("post-commit", "git update-ref refs/heads/main HEAD^");
    let result = f.commit(&["a.txt"]);
    assert!(result.committed);
    assert_ne!(result.head, old);
    assert!(result.warning.is_some());
    assert!(result.recovery_index.is_some());
    assert_eq!(text(&f.root, &["rev-parse", "HEAD"]), old);
    assert!(Path::new(result.recovery_index.as_ref().unwrap()).is_file());
}

#[cfg(unix)]
#[test]
fn preview_uses_frozen_contents_and_never_executes_clean_filters() {
    let f = Fixture::new(true);
    fs::write(f.root.join(".gitattributes"), "a.txt filter=danger\n").unwrap();
    git(
        &f.root,
        &["config", "filter.danger.clean", "touch filter-ran; cat"],
    );
    git(&f.root, &["config", "filter.danger.required", "true"]);
    fs::write(f.root.join("a.txt"), "a/before must remain literal\n").unwrap();
    let preview = f.preview(&["a.txt"]);
    assert!(!f.root.join("filter-ran").exists());
    assert!(preview.diff.contains("+a/before must remain literal"));
    assert!(preview.diff.contains("\"a/a.txt\""));
    let result = commit_selected(
        &f.root,
        &preview.paths,
        "filter explicit",
        &preview.snapshot,
    )
    .unwrap();
    assert!(result.committed);
    assert!(f.root.join("filter-ran").exists());
}

#[cfg(unix)]
#[test]
fn relative_custom_hooks_execute_all_stages_and_post_hook_staging_is_preserved() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new(true);
    let hooks = f.root.join("own-hooks");
    fs::create_dir(&hooks).unwrap();
    git(&f.root, &["config", "core.hooksPath", "own-hooks"]);
    for (name, body) in [
        ("pre-commit", "echo pre > pre-ran"),
        ("prepare-commit-msg", "echo prepared > prepare-ran"),
        ("commit-msg", "echo checked > message-ran"),
        (
            "post-commit",
            "echo post > post-ran\nprintf 'post staged\\n' > post-staged.txt\ngit add -- post-staged.txt",
        ),
    ] {
        let path = hooks.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(f.root.join("a.txt"), "selected a\n").unwrap();
    let result = f.commit(&["a.txt"]);
    assert!(result.committed);
    assert!(result.recovery_index.is_none(), "{:?}", result.warning);
    for name in ["pre-ran", "prepare-ran", "message-ran", "post-ran"] {
        assert!(f.root.join(name).is_file(), "{name}")
    }
    assert_eq!(text(&f.root, &["show", ":post-staged.txt"]), "post staged");
    assert!(!text(&f.root, &["ls-tree", "--name-only", "HEAD"]).contains("post-staged.txt"));
}

#[cfg(unix)]
#[test]
fn foreign_replacement_index_lock_is_never_published_or_removed() {
    let f = Fixture::new(true);
    fs::write(f.root.join("a.txt"), "selected a\n").unwrap();
    let original_index = fs::read(f.root.join(".git/index")).unwrap();
    f.hook(
        "post-commit",
        "rm .git/index.lock\nprintf foreign-lock > .git/index.lock",
    );
    let result = f.commit(&["a.txt"]);
    assert!(result.committed);
    assert!(result.warning.is_some());
    assert!(
        result
            .recovery_index
            .as_deref()
            .is_some_and(|path| Path::new(path).is_file())
    );
    assert_eq!(fs::read(f.root.join(".git/index")).unwrap(), original_index);
    assert_eq!(
        fs::read_to_string(f.root.join(".git/index.lock")).unwrap(),
        "foreign-lock"
    );
    assert_eq!(text(&f.root, &["rev-parse", "HEAD"]), result.head);
}

#[test]
fn physical_root_replacement_after_preview_cannot_commit_in_new_checkout() {
    let f = Fixture::new(true);
    fs::write(f.root.join("a.txt"), "selected a\n").unwrap();
    let preview = f.preview(&["a.txt"]);
    let original = f.root.with_extension("original");
    fs::rename(&f.root, &original).unwrap();
    let output = Command::new("git")
        .args(["clone", "--quiet", "--no-local", "--no-hardlinks"])
        .arg(&original)
        .arg(&f.root)
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
    fs::write(f.root.join("a.txt"), "selected a\n").unwrap();
    fs::write(
        f.root.join(".git/index"),
        fs::read(original.join(".git/index")).unwrap(),
    )
    .unwrap();
    let replacement_head = text(&f.root, &["rev-parse", "HEAD"]);
    let result = commit_selected(
        &f.root,
        &preview.paths,
        "must not reach replacement",
        &preview.snapshot,
    );
    assert!(result.is_err());
    assert_eq!(text(&f.root, &["rev-parse", "HEAD"]), replacement_head);
    assert!(!f.root.join(".git/index.lock").exists());
    fs::remove_dir_all(original).unwrap();
}

#[cfg(unix)]
#[test]
fn missing_promisor_blob_does_not_execute_remote_helpers_during_preview() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new(true);
    let remote = f.root.join("promisor-remote");
    git(
        &f.root,
        &[
            "clone",
            "--quiet",
            "--no-local",
            "--no-hardlinks",
            f.root.to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );
    let helper = f.root.join("upload-pack-fixture");
    fs::write(
        &helper,
        "#!/bin/sh\ntouch \"$0.ran\"\nexec git-upload-pack \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    git(
        &f.root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&f.root, &["config", "remote.origin.promisor", "true"]);
    git(
        &f.root,
        &[
            "config",
            "remote.origin.uploadpack",
            helper.to_str().unwrap(),
        ],
    );
    git(&f.root, &["config", "extensions.partialClone", "origin"]);
    let oid = text(&f.root, &["rev-parse", "HEAD:a.txt"]);
    let blob = f.root.join(".git/objects").join(&oid[..2]).join(&oid[2..]);
    fs::remove_file(&blob).unwrap();
    fs::write(f.root.join("a.txt"), "selected a\n").unwrap();
    let result = commit_preview(&f.root, &["a.txt".into()]);
    assert!(result.is_err());
    assert!(!f.root.join("upload-pack-fixture.ran").exists());
    assert!(!blob.exists());
}
