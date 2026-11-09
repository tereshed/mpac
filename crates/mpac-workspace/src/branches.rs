use super::{GIT_BUDGET, git_strings, head, open_root};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub current: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BranchState {
    pub is_repo: bool,
    pub current: Option<String>,
    pub head: String,
    pub detached: bool,
    pub dirty: bool,
    pub branches: Vec<Branch>,
    pub blocked_reason: Option<String>,
}

fn repo_root(root: &Path, deadline: Instant) -> Result<Option<PathBuf>> {
    let output = git_strings(root, &["rev-parse", "--show-toplevel"], deadline)?;
    if !output.success {
        return Ok(None);
    }
    let path = String::from_utf8(output.stdout).context("Путь Git должен поддерживать UTF-8")?;
    Ok(Some(
        PathBuf::from(path.strip_suffix('\n').unwrap_or(&path))
            .canonicalize()
            .context("Папка Git недоступна")?,
    ))
}

pub fn repository_root(root: &Path) -> Result<Option<PathBuf>> {
    let _directory = open_root(root)?;
    repo_root(root, Instant::now() + GIT_BUDGET)
}

fn state(root: &Path, deadline: Instant) -> Result<BranchState> {
    let _directory = open_root(root)?;
    let canonical = root.canonicalize().context("Папка проекта недоступна")?;
    if repo_root(root, deadline)?.as_ref() != Some(&canonical) {
        return Ok(BranchState {
            is_repo: false,
            current: None,
            head: "unborn".into(),
            detached: false,
            dirty: false,
            branches: vec![],
            blocked_reason: Some("Выберите корневую папку Git-репозитория".into()),
        });
    }
    let head = head(root, deadline)?;
    let symbolic = git_strings(
        root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        deadline,
    )?;
    let current = if symbolic.success {
        let text =
            String::from_utf8(symbolic.stdout).context("Имя ветки должно поддерживать UTF-8")?;
        Some(text.strip_suffix('\n').unwrap_or(&text).to_owned())
    } else {
        None
    };
    let index = git_strings(root, &["ls-files", "--stage", "-z"], deadline)?;
    ensure!(index.success, "Не удалось проверить Git index");
    let submodules = index
        .stdout
        .split(|b| *b == 0)
        .any(|entry| entry.starts_with(b"160000 "));
    let status = git_strings(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ],
        deadline,
    )?;
    ensure!(
        status.success,
        "Не удалось проверить изменения перед переключением ветки"
    );
    let dirty = !status.stdout.is_empty();
    let refs = git_strings(
        root,
        &[
            "for-each-ref",
            "--count=257",
            "--format=%(refname)%00",
            "refs/heads/",
        ],
        deadline,
    )?;
    ensure!(refs.success, "Не удалось прочитать локальные ветки");
    let mut branches = Vec::new();
    for bytes in refs
        .stdout
        .split(|b| *b == 0 || *b == b'\n')
        .filter(|s| !s.is_empty())
    {
        let name = std::str::from_utf8(bytes)
            .context("Имя ветки должно поддерживать UTF-8")?
            .strip_prefix("refs/heads/")
            .context("Недопустимая локальная ветка")?
            .to_owned();
        branches.push(Branch {
            current: current.as_ref() == Some(&name),
            name,
        });
    }
    let blocked_reason = if branches.len() > 256 {
        branches.truncate(256);
        Some("Слишком много локальных веток: в прототипе доступны первые 256; переключите ветку через Git".into())
    } else if submodules {
        Some("Переключение репозитория с submodules пока доступно только через Git; вложенные фильтры не запускаются".into())
    } else if head == "unborn" {
        Some("Сначала создайте первый коммит в репозитории".into())
    } else if dirty {
        Some(
            "Сохраните или закоммитьте изменения, включая новые файлы, перед переключением ветки"
                .into(),
        )
    } else {
        None
    };
    Ok(BranchState {
        is_repo: true,
        detached: current.is_none(),
        current,
        head,
        dirty,
        branches,
        blocked_reason,
    })
}

pub fn branch_state(root: &Path) -> Result<BranchState> {
    state(root, Instant::now() + GIT_BUDGET)
}

/// No fetch, force, stash, reset, branch creation or ignored-file overwrite.
/// Checkout filters and hooks do not execute; LFS data can remain pointer files.
pub fn switch_branch(
    root: &Path,
    branch: &str,
    expected_head: &str,
    expected_branch: Option<&str>,
) -> Result<BranchState> {
    ensure!(
        !branch.is_empty()
            && branch.len() <= 1024
            && !branch.starts_with('-')
            && !branch.chars().any(char::is_control),
        "Укажите существующую локальную ветку"
    );
    let deadline = Instant::now() + GIT_BUDGET;
    let before = state(root, deadline)?;
    ensure!(before.is_repo, "Папка не является корнем Git-репозитория");
    ensure!(
        before.head == expected_head && before.current.as_deref() == expected_branch,
        "Ветка уже изменилась. Обновите список перед переключением"
    );
    ensure!(
        before.blocked_reason.is_none(),
        "{}",
        before
            .blocked_reason
            .as_deref()
            .unwrap_or("Переключение недоступно")
    );
    ensure!(
        before.branches.iter().any(|b| b.name == branch),
        "Укажите существующую локальную ветку; создание и загрузка веток не выполняются"
    );
    if before.current.as_deref() == Some(branch) {
        return Ok(before);
    }
    let mut args = filter_overrides(root, deadline)?;
    args.extend(["-c".to_owned(), "submodule.recurse=false".to_owned()]);
    args.extend([
        "switch".into(),
        "--no-guess".into(),
        "--no-recurse-submodules".into(),
        "--no-overwrite-ignore".into(),
        "--".into(),
        branch.into(),
    ]);
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let switched = git_strings(root, &args, Instant::now() + Duration::from_secs(30))?;
    ensure!(
        switched.success,
        "Git не переключил ветку. Она может быть открыта в другом worktree; изменения и принудительное переключение не выполняются"
    );
    let after = state(root, Instant::now() + GIT_BUDGET)?;
    ensure!(
        after.current.as_deref() == Some(branch),
        "Состояние Git изменилось во время переключения. Обновите папку"
    );
    Ok(after)
}

/// Enumerate names only; command values are never retained or exposed.
/// Both preflight status/diff and checkout require these overrides because Git
/// can run a clean filter merely to compare an equal-length tracked edit.
pub(super) fn filter_overrides(root: &Path, deadline: Instant) -> Result<Vec<String>> {
    let filters = git_strings(
        root,
        &[
            "config",
            "--null",
            "--name-only",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process|required)$",
        ],
        deadline,
    )?;
    ensure!(
        filters.success || filters.exit_code == Some(1),
        "Не удалось безопасно прочитать конфигурацию Git filter"
    );
    let mut drivers = BTreeSet::new();
    if filters.success {
        for bytes in filters.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
            let key =
                std::str::from_utf8(bytes).context("Конфигурация Git filter не поддерживается")?;
            let driver = key
                .strip_prefix("filter.")
                .and_then(|s| s.rsplit_once('.'))
                .map(|(name, _)| name)
                .context("Конфигурация Git filter не поддерживается")?;
            ensure!(
                !driver.is_empty() && driver.len() <= 1024 && !driver.chars().any(char::is_control),
                "Конфигурация Git filter не поддерживается"
            );
            drivers.insert(driver.to_owned());
        }
    }
    ensure!(
        drivers.len() <= 64,
        "Слишком много Git filter: переключите ветку через Git"
    );
    let mut args = Vec::new();
    for driver in drivers {
        for (setting, value) in [
            ("clean", ""),
            ("smudge", ""),
            ("process", ""),
            ("required", "false"),
        ] {
            args.extend(["-c".into(), format!("filter.{driver}.{setting}={value}")]);
        }
    }
    Ok(args)
}
