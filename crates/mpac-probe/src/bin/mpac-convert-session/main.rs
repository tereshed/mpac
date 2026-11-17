//! Native session format spike. Writes only to an explicitly selected test profile.
mod transcript;

use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use mpac_probe::{rpc, version_number};
use serde_json::{Value, json};
use tokio::{io::BufReader, process::Command, time::timeout};
use transcript::{Harness, Transcript};
use uuid::Uuid;

struct Staging(PathBuf);

impl Staging {
    fn new(profile: &Path) -> Result<Self> {
        let path = profile.join(format!(".mpac-conversion-{}", Uuid::new_v4()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&path)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn write(&self, name: &str, rows: &[Value]) -> Result<PathBuf> {
        let path = self.0.join(name);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        for row in rows {
            serde_json::to_writer(&mut file, row)?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        Ok(path)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn isolated_command(program: &str, profile: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear()
        .env("PATH", env::var_os("PATH").unwrap_or_default())
        .env("HOME", profile)
        .env("USERPROFILE", profile)
        .env("CLAUDE_CONFIG_DIR", profile)
        .env("CODEX_HOME", profile)
        .current_dir(cwd)
        .stderr(Stdio::null())
        .kill_on_drop(true);
    cmd
}

async fn verify_version(target: Harness, profile: &Path, cwd: &Path) -> Result<()> {
    let output = timeout(
        Duration::from_secs(5),
        isolated_command(target.program(), profile, cwd)
            .arg("--version")
            .output(),
    )
    .await
    .context("CLI version check timed out")??;
    ensure!(
        output.status.success()
            && version_number(&String::from_utf8_lossy(&output.stdout)).as_deref()
                == Some(target.version()),
        "prototype requires {} {}",
        target.program(),
        target.version()
    );
    Ok(())
}

fn write_claude(
    source: &Transcript,
    staging: &Staging,
    profile: &Path,
    cwd: &str,
    title: &str,
) -> Result<String> {
    ensure!(
        cwd.is_ascii() && cwd.len() <= 180,
        "unsupported Claude workspace path"
    );
    let id = Uuid::new_v4().to_string();
    let rows = source.claude_rows(&id, cwd, title);
    let staged = staging.write("session.jsonl", &rows)?;
    let projects = profile.join("projects");
    let workspace = cwd.replace(|c: char| !c.is_ascii_alphanumeric(), "-");
    let directory = projects.join(workspace);
    for dir in [&projects, &directory] {
        if !dir.exists() {
            fs::create_dir(dir)?;
        }
        ensure!(
            dir.symlink_metadata()?.file_type().is_dir(),
            "Claude profile contains a redirected history directory"
        );
    }
    // A hard link publishes the completed file atomically and cannot overwrite.
    fs::hard_link(staged, directory.join(format!("{id}.jsonl")))?;
    Ok(id)
}

async fn import_codex(
    source: &Transcript,
    staging: &Staging,
    profile: &Path,
    cwd: &Path,
    title: &str,
) -> Result<String> {
    let staging_id = Uuid::new_v4().to_string();
    let stamp = source.first_timestamp()[..19].replace(':', "-");
    let path = staging.write(
        &format!("rollout-{stamp}-{staging_id}.jsonl"),
        &source.codex_rows(&staging_id, cwd),
    )?;
    let mut child = isolated_command("codex", profile, cwd)
        .args([
            "app-server",
            "--stdio",
            "--disable",
            "shell_snapshot",
            "--disable",
            "daemon_auto_start",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().context("missing Codex input")?;
    let mut stdout = BufReader::new(child.stdout.take().context("missing Codex output")?);
    let result = timeout(Duration::from_secs(15), async {
        rpc::send(
            &mut stdin,
            &json!({"id":1,"method":"initialize","params":{
                "clientInfo":{"name":"mpac-converter","version":"0.1"},
                "capabilities":{"experimentalApi":true}
            }}),
        )
        .await?;
        let init = rpc::response(&mut stdout, 1).await?;
        ensure!(
            init["codexHome"].as_str().map(Path::new) == Some(profile),
            "Codex App Server selected a different profile"
        );
        rpc::send(&mut stdin, &json!({"method":"initialized"})).await?;
        rpc::send(
            &mut stdin,
            &json!({"id":2,"method":"thread/fork","params":{
                "threadId":staging_id,"path":path,"cwd":cwd,"excludeTurns":true
            }}),
        )
        .await?;
        let fork = rpc::response(&mut stdout, 2)
            .await
            .context("Codex could not register the imported session")?;
        let id = fork["thread"]["id"]
            .as_str()
            .context("missing native session ID")?;
        ensure!(
            Uuid::parse_str(id).is_ok() && id != staging_id,
            "invalid Codex session ID"
        );
        ensure!(
            fork["thread"]["path"]
                .as_str()
                .map(Path::new)
                .is_some_and(|path| path.starts_with(profile.join("sessions")) && path.is_file()),
            "Codex did not persist the new session in the selected profile"
        );
        rpc::send(
            &mut stdin,
            &json!({"id":3,"method":"thread/name/set","params":{"threadId":id,"name":title}}),
        )
        .await?;
        rpc::response(&mut stdout, 3)
            .await
            .with_context(|| format!("session {id} was registered but could not be named"))?;
        Ok(id.to_owned())
    })
    .await;
    drop(stdin);
    if timeout(Duration::from_secs(3), child.wait()).await.is_err() {
        child.kill().await?;
    }
    result.context("Codex import timed out; inspect the test profile before retrying")?
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let [source, input, target, profile, project, title] = args.as_slice() else {
        bail!(
            "usage: mpac-convert-session <claude|codex> <source.jsonl> <claude|codex> <test-profile-dir> <project-dir> <title>"
        );
    };
    let source_kind = Harness::parse(source)?;
    let target = Harness::parse(target)?;
    ensure!(source_kind != target, "select two different harnesses");
    let input = Path::new(input);
    let profile = Path::new(profile)
        .canonicalize()
        .context("test profile directory must exist")?;
    let project = Path::new(project)
        .canonicalize()
        .context("project directory must exist")?;
    ensure!(project.is_dir() && profile.is_dir(), "expected directory");
    // The spike deliberately refuses ordinary profiles. The smoke tool owns both
    // the marker and the unprivileged fixture configuration.
    ensure!(
        profile.join(".mpac-conversion-test-profile").is_file(),
        "use a dedicated test profile marked .mpac-conversion-test-profile"
    );
    let title = title.to_str().context("title must be UTF-8")?.trim();
    ensure!(
        !title.is_empty() && title.len() <= 160 && !title.chars().any(char::is_control),
        "expected a single-line title of at most 160 bytes"
    );
    let cwd = project.to_str().context("project path must be UTF-8")?;
    let transcript = Transcript::read(input, source_kind)?;
    verify_version(target, &profile, &project).await?;
    let staging = Staging::new(&profile)?;
    let id = match target {
        Harness::Claude => write_claude(&transcript, &staging, &profile, cwd, title)?,
        Harness::Codex => import_codex(&transcript, &staging, &profile, &project, title).await?,
    };
    println!(
        "{}",
        json!({"id":id,"target":target.program(),"messages":transcript.message_count(),
               "non_text_omitted":transcript.omitted,"incomplete_messages":transcript.incomplete})
    );
    Ok(())
}
