use std::{io, process::ExitStatus, process::Stdio, time::Duration};

use anyhow::{Context, Result};
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{ChildStderr, ChildStdin, ChildStdout, Command},
    time::timeout,
};

const STOP_BUDGET: Duration = Duration::from_secs(5);

/// Owns one protocol process group (Unix) or Job Object (Windows).
///
/// No raw child, PID attachment, or wait/try_wait is exposed. In particular the
/// leader remains unreaped until the group has been signalled, even if it exits
/// early. This avoids sending a late signal to a recycled group ID. A process
/// must be explicitly stopped; dropping its owner initiates hard termination.
/// Unix descendants that deliberately leave their group are outside this scope.
pub struct ProtocolProcess {
    child: Box<dyn ChildWrapper>,
    kill_requested: bool,
    status: Option<ExitStatus>,
}

pub struct ProtocolIo {
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

impl ProtocolProcess {
    /// Runs the supplied executable directly, without adding a shell. The
    /// adapter sets args, cwd and environment on Command before handing it over.
    /// This boundary configures stdio and process ownership itself.
    pub fn spawn(mut command: Command) -> Result<(Self, ProtocolIo)> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut wrapped = CommandWrap::from(command);
        wrapped.wrap(KillOnDrop);
        #[cfg(unix)]
        wrapped.wrap(process_wrap::tokio::ProcessGroup::leader());
        #[cfg(windows)]
        wrapped.wrap(process_wrap::tokio::JobObject);
        #[cfg(not(any(unix, windows)))]
        anyhow::bail!("process_platform_unsupported");

        let child = wrapped.spawn().context("spawn_failed")?;
        let mut owner = Self {
            child,
            kill_requested: false,
            status: None,
        };
        let io = ProtocolIo {
            stdin: owner.child.stdin().take().context("stdin_missing")?,
            stdout: owner.child.stdout().take().context("stdout_missing")?,
            stderr: owner.child.stderr().take().context("stderr_missing")?,
        };
        Ok((owner, io))
    }

    /// Force-stop the owned execution and reap its leader. Protocol-level
    /// interrupt is a separate adapter operation, not a substitute for stop.
    /// Safe to call again, including after cancellation of a previous stop.
    pub async fn stop(&mut self) -> Result<ExitStatus> {
        if let Some(status) = self.status {
            return Ok(status);
        }
        self.request_kill().context("process_stop_failed")?;
        let status = timeout(STOP_BUDGET, self.child.wait())
            .await
            .context("process_reap_timeout")?
            .context("process_reap_failed")?;
        self.status = Some(status);
        Ok(status)
    }

    fn request_kill(&mut self) -> io::Result<()> {
        if self.kill_requested {
            return Ok(());
        }
        // Querying the still-owned child does not reap it. Its zombie pins the
        // original group ID until the signal decision below is complete.
        #[cfg(target_os = "macos")]
        let owned_pid = self.child.id();
        match self.child.start_kill() {
            Ok(()) => self.kill_requested = true,
            #[cfg(target_os = "macos")]
            Err(error) if error.raw_os_error() == Some(libc::EPERM) => {
                let drained = owned_pid
                    .and_then(|pid| i32::try_from(pid).ok())
                    .filter(|pid| *pid > 0)
                    .map(group_has_no_live_members)
                    .transpose()?
                    .unwrap_or(false);
                if !drained {
                    return Err(error);
                }
                self.kill_requested = true;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.kill_requested = true;
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn group_has_no_live_members(pgid: i32) -> io::Result<bool> {
    // Darwin returns EPERM for zombie-only groups. This exception is safe only
    // while the owned leader remains unreaped and every group member is known
    // to be a zombie. Permission failures and incomplete discovery fail closed.
    let mut exit: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let observed = unsafe {
        libc::waitid(
            libc::P_PID,
            pgid as libc::id_t,
            &mut exit,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if observed != 0 {
        return Err(io::Error::last_os_error());
    }
    if exit.si_pid != pgid {
        return Ok(false);
    }
    let mut pids = vec![0_i32; 4096];
    let bytes = (pids.len() * std::mem::size_of::<i32>()) as i32;
    let count = unsafe { libc::proc_listpids(2, pgid as u32, pids.as_mut_ptr().cast(), bytes) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    if count == 0 || count >= bytes || !(count as usize).is_multiple_of(std::mem::size_of::<i32>())
    {
        return Ok(false);
    }
    let mut saw_owned_leader = false;
    let mut verified_zombies = Vec::new();
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
        if result == 0
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            && pid == pgid
        {
            // Darwin's detailed process API reports ESRCH for an owned zombie
            // that proc_listpids still enumerates. WNOWAIT above proved this
            // exact unreaped leader exited; no other PID gets this exception.
            saw_owned_leader = true;
            verified_zombies.push(pid);
            continue;
        }
        if result != size || info.pbi_status != libc::SZOMB || info.pbi_pgid != pgid as u32 {
            return Ok(false);
        }
        saw_owned_leader |= pid == pgid;
        verified_zombies.push(pid);
    }
    if !saw_owned_leader {
        return Ok(false);
    }
    // A member may have forked and exited while the first list was inspected.
    // Reject any newly observed member rather than overlooking a live child.
    let mut current = vec![0_i32; 4096];
    let count = unsafe { libc::proc_listpids(2, pgid as u32, current.as_mut_ptr().cast(), bytes) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    if count == 0 || count >= bytes || !(count as usize).is_multiple_of(std::mem::size_of::<i32>())
    {
        return Ok(false);
    }
    let current = &current[..count as usize / std::mem::size_of::<i32>()];
    Ok(current.contains(&pgid)
        && current
            .iter()
            .filter(|pid| **pid > 0)
            .all(|pid| verified_zombies.contains(pid)))
}

impl Drop for ProtocolProcess {
    fn drop(&mut self) {
        // Never re-signal after wait could have reaped the leader. KillOnDrop
        // on the native child also arranges Tokio's background leader reaping.
        let _ = self.request_kill();
    }
}

/// Drain a pipe without retaining arbitrary output or stopping at a size cap.
/// Consumers that need data use their own bounded framing/capture policy.
pub async fn discard_output(mut reader: impl AsyncRead + Unpin) -> io::Result<()> {
    let mut chunk = [0_u8; 8192];
    while reader.read(&mut chunk).await? != 0 {}
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn already_exited_owned_root_can_be_stopped_and_reaped_idempotently() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 0"]);
        let (mut owner, io) = ProtocolProcess::spawn(command).unwrap();
        let pid = i32::try_from(owner.child.id().unwrap()).unwrap();
        drop(io.stdin);
        let (stdout, stderr) = tokio::join!(discard_output(io.stdout), discard_output(io.stderr));
        stdout.unwrap();
        stderr.unwrap();
        timeout(Duration::from_secs(2), async {
            while !group_has_no_live_members(pid).unwrap() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("owned leader never became observable as a zombie");
        let first = owner.stop().await.unwrap();
        assert!(first.success());
        assert_eq!(owner.stop().await.unwrap(), first);
    }

    #[tokio::test]
    async fn a_live_owned_member_never_qualifies_for_the_eperm_exception() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "while :; do sleep 1; done"]);
        let (mut owner, io) = ProtocolProcess::spawn(command).unwrap();
        let pid = i32::try_from(owner.child.id().unwrap()).unwrap();
        drop(io);
        assert!(!group_has_no_live_members(pid).unwrap());
        owner.stop().await.unwrap();
    }
}
