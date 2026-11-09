use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use mpac_workspace::{MAX_FILE_BYTES, git_review, list_files, read_text, validate_review};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "mpac-workspace-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(root: &Path, args: &[&str]) {
    let result = Command::new("git")
        .args([
            "-c",
            "core.hooksPath=.mpac-nonexistent-hooks",
            "-c",
            "user.name=MPAC Test",
            "-c",
            "user.email=mpac@example.invalid",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn repository() -> TempDir {
    let temp = TempDir::new();
    git(&temp.0, &["init", "--quiet"]);
    fs::write(temp.0.join("notes.md"), "# Original\n").unwrap();
    git(&temp.0, &["add", "--", "notes.md"]);
    git(&temp.0, &["commit", "--quiet", "-m", "fixture"]);
    temp
}

#[test]
fn text_reader_and_listing_preserve_unicode_paths() {
    let temp = TempDir::new();
    fs::create_dir(temp.0.join("документы")).unwrap();
    fs::create_dir(temp.0.join(".git")).unwrap();
    fs::write(temp.0.join("документы/план.md"), "# План 🦀\n").unwrap();
    fs::write(temp.0.join("a.md"), "plain").unwrap();
    let entries = list_files(&temp.0, "").unwrap();
    assert!(entries[0].is_dir);
    assert_eq!(entries[0].path, "документы");
    assert!(entries.iter().all(|entry| entry.path != ".git"));
    assert_eq!(
        list_files(&temp.0, "документы").unwrap()[0].path,
        "документы/план.md"
    );
    assert_eq!(
        read_text(&temp.0, "документы/план.md").unwrap(),
        "# План 🦀\n"
    );
}

#[test]
fn rejects_absolute_paths_parent_traversal_and_binary_or_large_files() {
    let temp = TempDir::new();
    for path in ["../outside", "nested/../../outside", "/etc/passwd"] {
        assert!(read_text(&temp.0, path).is_err());
        assert!(list_files(&temp.0, path).is_err());
    }
    fs::write(temp.0.join("binary"), [0, 1, 2]).unwrap();
    assert_eq!(
        read_text(&temp.0, "binary").unwrap_err().to_string(),
        "file_not_utf8_text"
    );
    fs::write(temp.0.join("large"), vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
    assert_eq!(
        read_text(&temp.0, "large").unwrap_err().to_string(),
        "file_size_limit"
    );
}

#[cfg(unix)]
#[test]
fn symlinks_cannot_escape_the_directory_capability() {
    use std::os::unix::fs::symlink;
    let temp = TempDir::new();
    let outside = TempDir::new();
    fs::write(outside.0.join("secret.md"), "private").unwrap();
    fs::write(temp.0.join("safe.md"), "safe").unwrap();
    symlink(outside.0.join("secret.md"), temp.0.join("escape.md")).unwrap();
    symlink(&outside.0, temp.0.join("escape-dir")).unwrap();
    symlink("safe.md", temp.0.join("local-link.md")).unwrap();
    assert!(read_text(&temp.0, "escape.md").is_err());
    assert!(read_text(&temp.0, "escape-dir/secret.md").is_err());
    assert!(list_files(&temp.0, "escape-dir").is_err());
    assert_eq!(read_text(&temp.0, "local-link.md").unwrap(), "safe");
    assert_eq!(list_files(&temp.0, "").unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn special_files_are_rejected_without_blocking() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let temp = TempDir::new();
    let path = CString::new(temp.0.join("pipe").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert_eq!(
        read_text(&temp.0, "pipe").unwrap_err().to_string(),
        "not_regular_file"
    );
}

#[test]
fn review_captures_index_worktree_and_untracked_content_and_detects_stale_snapshot() {
    let temp = repository();
    fs::write(temp.0.join("notes.md"), "# Staged\n").unwrap();
    git(&temp.0, &["add", "--", "notes.md"]);
    fs::write(temp.0.join("notes.md"), "# Working\n").unwrap();
    fs::write(temp.0.join("new.md"), "new content\n").unwrap();
    let review = git_review(&temp.0).unwrap();
    assert!(!review.truncated);
    assert!(review.diff.contains("# Staged"));
    assert!(review.diff.contains("# Working"));
    assert!(review.diff.contains("+new content"));
    assert_eq!(review.files.len(), 2);
    validate_review(&temp.0, &review.snapshot).unwrap();
    fs::write(temp.0.join("new.md"), "bad content\n").unwrap();
    assert_eq!(
        validate_review(&temp.0, &review.snapshot)
            .unwrap_err()
            .to_string(),
        "review_snapshot_stale"
    );
}

#[test]
fn unborn_repository_includes_staged_and_untracked_files() {
    let temp = TempDir::new();
    git(&temp.0, &["init", "--quiet"]);
    fs::write(temp.0.join("staged.md"), "staged new\n").unwrap();
    fs::write(temp.0.join("untracked.md"), "untracked new\n").unwrap();
    git(&temp.0, &["add", "--", "staged.md"]);
    let review = git_review(&temp.0).unwrap();
    assert_eq!(review.head, "unborn");
    assert_eq!(review.files.len(), 2);
    assert!(review.diff.contains("staged new"));
    assert!(review.diff.contains("untracked new"));
    validate_review(&temp.0, &review.snapshot).unwrap();
}

#[test]
fn binary_untracked_content_is_part_of_the_snapshot() {
    let temp = repository();
    fs::write(temp.0.join("binary"), [0, 1, 2]).unwrap();
    let review = git_review(&temp.0).unwrap();
    assert!(review.diff.contains("Binary file"));
    fs::write(temp.0.join("binary"), [0, 2, 2]).unwrap();
    assert_ne!(git_review(&temp.0).unwrap().snapshot, review.snapshot);
}

#[test]
fn rename_with_spaces_is_parsed_and_nested_roots_are_not_expanded_to_parent_repo() {
    let temp = repository();
    git(&temp.0, &["mv", "--", "notes.md", "renamed notes.md"]);
    let review = git_review(&temp.0).unwrap();
    assert_eq!(review.files.len(), 1);
    assert_eq!(review.files[0].path, "renamed notes.md");
    assert!(review.files[0].status.contains('R'));
    fs::create_dir(temp.0.join("nested")).unwrap();
    assert_eq!(
        git_review(&temp.0.join("nested")).unwrap_err().to_string(),
        "root_must_be_repository_root"
    );
}

#[test]
fn oversized_review_is_explicit_and_cannot_be_dispatched() {
    let temp = repository();
    fs::write(temp.0.join("large"), vec![b'x'; MAX_FILE_BYTES + 1]).unwrap();
    let review = git_review(&temp.0).unwrap();
    assert!(review.truncated);
    assert_eq!(
        validate_review(&temp.0, &review.snapshot)
            .unwrap_err()
            .to_string(),
        "truncated_review_cannot_dispatch"
    );
}

#[cfg(unix)]
#[test]
fn external_diff_textconv_and_fsmonitor_are_never_executed() {
    use std::os::unix::fs::PermissionsExt;
    let temp = repository();
    let outside = TempDir::new();
    let marker = outside.0.join("driver-was-run");
    let driver = outside.0.join("driver.sh");
    fs::write(
        &driver,
        format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&driver, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(temp.0.join(".gitattributes"), "*.md diff=attack\n").unwrap();
    fs::write(temp.0.join("notes.md"), "changed\n").unwrap();
    git(
        &temp.0,
        &["config", "diff.attack.command", driver.to_str().unwrap()],
    );
    git(
        &temp.0,
        &["config", "diff.attack.textconv", driver.to_str().unwrap()],
    );
    git(
        &temp.0,
        &["config", "core.fsmonitor", driver.to_str().unwrap()],
    );
    let review = git_review(&temp.0).unwrap();
    assert!(review.diff.contains("changed"));
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn untracked_links_are_reviewed_without_reading_their_targets_and_mode_changes_expire_review() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = repository();
    let outside = TempDir::new();
    fs::write(outside.0.join("secret"), "not part of review").unwrap();
    symlink(outside.0.join("secret"), temp.0.join("external-link")).unwrap();
    fs::write(temp.0.join("script"), "echo hello\n").unwrap();
    fs::set_permissions(temp.0.join("script"), fs::Permissions::from_mode(0o600)).unwrap();
    let review = git_review(&temp.0).unwrap();
    assert!(review.diff.contains("new file mode 120000"));
    assert!(!review.diff.contains("not part of review"));
    fs::set_permissions(temp.0.join("script"), fs::Permissions::from_mode(0o700)).unwrap();
    assert_ne!(git_review(&temp.0).unwrap().snapshot, review.snapshot);
}

#[cfg(unix)]
#[test]
fn unix_backslash_filenames_are_not_converted_into_directory_separators() {
    let temp = TempDir::new();
    fs::write(temp.0.join("literal\\name.md"), "literal name").unwrap();
    let entries = list_files(&temp.0, "").unwrap();
    assert_eq!(entries[0].path, "literal\\name.md");
    assert_eq!(
        read_text(&temp.0, &entries[0].path).unwrap(),
        "literal name"
    );
}

#[test]
fn a_snapshot_from_an_identical_clone_cannot_validate_in_another_root() {
    let original = repository();
    let clone = TempDir::new();
    git(
        &clone.0,
        &["clone", "--quiet", original.0.to_str().unwrap(), "."],
    );
    for root in [&original.0, &clone.0] {
        fs::write(root.join("notes.md"), "same change\n").unwrap();
    }
    let first = git_review(&original.0).unwrap();
    let second = git_review(&clone.0).unwrap();
    assert_eq!(first.head, second.head);
    assert_eq!(first.diff, second.diff);
    assert_ne!(first.snapshot, second.snapshot);
    assert_eq!(
        validate_review(&clone.0, &first.snapshot)
            .unwrap_err()
            .to_string(),
        "review_snapshot_stale"
    );
}

#[cfg(unix)]
#[test]
fn root_symlinks_are_rejected_before_capability_open_or_git_canonicalization() {
    use std::os::unix::fs::symlink;
    let parent = TempDir::new();
    let outside = repository();
    fs::write(outside.0.join("notes.md"), "outside content\n").unwrap();
    let link = parent.0.join("substituted-root");
    symlink(&outside.0, &link).unwrap();
    assert!(read_text(&link, "notes.md").is_err());
    assert!(list_files(&link, "").is_err());
    assert!(git_review(&link).is_err());
    assert!(read_text(&link.join(""), "notes.md").is_err());
}
