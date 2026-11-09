use mpac_workspace::{branch_state, switch_branch};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture {
    directory: PathBuf,
    root: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "--no-pager",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=MPAC Test",
            "-c",
            "user.email=mpac@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let directory = std::env::temp_dir().join(format!(
            "mpac-branches-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let root = directory.join("repo");
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        git(&root, &["init", "--quiet", "-b", "main"]);
        fs::write(root.join("notes.md"), "before\n").unwrap();
        git(&root, &["add", "notes.md"]);
        git(&root, &["commit", "--quiet", "-m", "main"]);
        git(&root, &["checkout", "--quiet", "-b", "other"]);
        fs::write(root.join("notes.md"), "after!\n").unwrap();
        git(&root, &["commit", "--quiet", "-am", "other"]);
        git(&root, &["checkout", "--quiet", "main"]);
        Self { directory, root }
    }
    fn switch(&self, name: &str) -> anyhow::Result<mpac_workspace::BranchState> {
        let state = branch_state(&self.root)?;
        switch_branch(&self.root, name, &state.head, state.current.as_deref())
    }
}

#[test]
fn only_existing_local_branches_are_listed_and_clean_switch_updates_head_and_files() {
    let fixture = Fixture::new();
    git(
        &fixture.root,
        &["update-ref", "refs/remotes/origin/remote-only", "HEAD"],
    );
    let before = branch_state(&fixture.root).unwrap();
    assert!(before.is_repo);
    assert_eq!(before.current.as_deref(), Some("main"));
    assert!(!before.dirty);
    assert_eq!(before.branches.len(), 2);
    assert!(before.branches.iter().all(|b| b.name != "remote-only"));
    let after = fixture.switch("other").unwrap();
    assert_eq!(after.current.as_deref(), Some("other"));
    assert_ne!(after.head, before.head);
    assert_eq!(
        fs::read_to_string(fixture.root.join("notes.md")).unwrap(),
        "after!\n"
    );
    assert!(!after.dirty);
}

#[test]
fn stale_snapshots_dirty_files_remote_names_and_option_like_inputs_are_rejected() {
    let fixture = Fixture::new();
    let before = branch_state(&fixture.root).unwrap();
    assert!(switch_branch(&fixture.root, "other", "stale", before.current.as_deref()).is_err());
    assert!(switch_branch(&fixture.root, "other", &before.head, Some("stale")).is_err());
    for branch in ["--detach", "remote-only", "does-not-exist", ""] {
        assert!(fixture.switch(branch).is_err());
    }
    fs::write(fixture.root.join("untracked.md"), "preserve me").unwrap();
    let dirty = branch_state(&fixture.root).unwrap();
    assert!(dirty.dirty);
    assert!(dirty.blocked_reason.is_some());
    assert!(fixture.switch("other").is_err());
    assert_eq!(
        fs::read_to_string(fixture.root.join("untracked.md")).unwrap(),
        "preserve me"
    );
    assert_eq!(branch_state(&fixture.root).unwrap().current, before.current);
}

#[test]
fn nonrepo_nested_root_unborn_and_detached_states_are_explicit() {
    let fixture = Fixture::new();
    let plain = fixture.directory.join("plain");
    fs::create_dir(&plain).unwrap();
    assert!(!branch_state(&plain).unwrap().is_repo);
    let nested = fixture.root.join("nested");
    fs::create_dir(&nested).unwrap();
    assert!(!branch_state(&nested).unwrap().is_repo);
    git(&plain, &["init", "--quiet", "-b", "main"]);
    let unborn = branch_state(&plain).unwrap();
    assert_eq!(unborn.head, "unborn");
    assert!(unborn.blocked_reason.is_some());
    git(&fixture.root, &["checkout", "--quiet", "--detach", "HEAD"]);
    let detached = branch_state(&fixture.root).unwrap();
    assert!(detached.detached);
    assert!(detached.current.is_none());
    assert!(detached.blocked_reason.is_none());
    assert_eq!(
        fixture.switch("other").unwrap().current.as_deref(),
        Some("other")
    );
}

#[test]
fn another_worktree_using_target_branch_causes_refusal_without_force() {
    let fixture = Fixture::new();
    let worktree = fixture.directory.join("independent-worktree");
    git(
        &fixture.root,
        &[
            "worktree",
            "add",
            "--quiet",
            worktree.to_str().unwrap(),
            "other",
        ],
    );
    let error = fixture.switch("other").unwrap_err().to_string();
    assert!(error.contains("worktree"));
    assert_eq!(
        branch_state(&fixture.root).unwrap().current.as_deref(),
        Some("main")
    );
    assert_eq!(
        fs::read_to_string(worktree.join("notes.md")).unwrap(),
        "after!\n"
    );
}

#[test]
fn ignored_user_file_is_never_overwritten_by_branch_switch() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join(".gitignore"), "precious.local\n").unwrap();
    git(&fixture.root, &["add", ".gitignore"]);
    git(&fixture.root, &["commit", "--quiet", "-m", "ignore"]);
    git(&fixture.root, &["checkout", "--quiet", "-b", "with-local"]);
    fs::write(fixture.root.join("precious.local"), "tracked target").unwrap();
    git(&fixture.root, &["add", "-f", "precious.local"]);
    git(&fixture.root, &["commit", "--quiet", "-m", "target"]);
    git(&fixture.root, &["checkout", "--quiet", "main"]);
    fs::write(fixture.root.join("precious.local"), "user ignored data").unwrap();
    assert!(!branch_state(&fixture.root).unwrap().dirty);
    assert!(fixture.switch("with-local").is_err());
    assert_eq!(
        fs::read_to_string(fixture.root.join("precious.local")).unwrap(),
        "user ignored data"
    );
}

#[cfg(unix)]
#[test]
fn clean_smudge_process_filters_and_post_checkout_hooks_never_run_in_preflight_or_switch() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let marker = fixture.directory.join("external-ran");
    let script = fixture.directory.join("driver.sh");
    fs::write(
        &script,
        format!("#!/bin/sh\nprintf ran > '{}'\ncat\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(fixture.root.join(".gitattributes"), "*.md filter=proof\n").unwrap();
    git(&fixture.root, &["add", ".gitattributes"]);
    git(&fixture.root, &["commit", "--quiet", "-m", "attributes"]);
    for key in ["filter.proof.clean", "filter.proof.smudge"] {
        git(&fixture.root, &["config", key, script.to_str().unwrap()]);
    }
    git(&fixture.root, &["config", "filter.proof.required", "true"]);
    fs::write(fixture.root.join("notes.md"), "after!\n").unwrap();
    let file = fs::OpenOptions::new()
        .write(true)
        .open(fixture.root.join("notes.md"))
        .unwrap();
    file.set_times(
        fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(100000)),
    )
    .unwrap();
    assert!(branch_state(&fixture.root).unwrap().dirty);
    assert!(!marker.exists());
    assert!(
        mpac_workspace::git_review(&fixture.root)
            .unwrap()
            .diff
            .contains("after!")
    );
    assert!(!marker.exists());
    fs::write(fixture.root.join("notes.md"), "before\n").unwrap();
    git(
        &fixture.root,
        &["config", "filter.proof.process", script.to_str().unwrap()],
    );
    let hook = fixture.root.join(".git/hooks/post-checkout");
    fs::write(
        &hook,
        format!("#!/bin/sh\nprintf hook > '{}'\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        fixture.switch("other").unwrap().current.as_deref(),
        Some("other")
    );
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn tracked_submodule_blocks_switch_without_running_nested_filters() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let nested = fixture.root.join("nested");
    fs::create_dir(&nested).unwrap();
    git(&nested, &["init", "--quiet"]);
    fs::write(nested.join("note.txt"), "before\n").unwrap();
    git(&nested, &["add", "note.txt"]);
    git(&nested, &["commit", "--quiet", "-m", "nested"]);
    git(&fixture.root, &["add", "nested"]);
    git(&fixture.root, &["commit", "--quiet", "-m", "gitlink"]);
    let marker = fixture.directory.join("nested-ran");
    let driver = fixture.directory.join("nested-filter.sh");
    fs::write(
        &driver,
        format!("#!/bin/sh\nprintf ran > '{}'\ncat\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&driver, fs::Permissions::from_mode(0o700)).unwrap();
    git(
        &nested,
        &[
            "config",
            "filter.nestedproof.clean",
            driver.to_str().unwrap(),
        ],
    );
    fs::write(nested.join(".gitattributes"), "*.txt filter=nestedproof\n").unwrap();
    fs::write(nested.join("note.txt"), "after!\n").unwrap();
    let state = branch_state(&fixture.root).unwrap();
    assert!(state.blocked_reason.unwrap().contains("submodules"));
    assert!(fixture.switch("other").is_err());
    assert!(!marker.exists());
}
