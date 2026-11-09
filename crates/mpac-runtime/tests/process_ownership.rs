use std::time::Duration;

use mpac_runtime::process::{ProtocolIo, ProtocolProcess, discard_output};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::{sleep, timeout},
};

const BUDGET: Duration = Duration::from_secs(4);

fn spawn(mode: &str) -> (ProtocolProcess, ProtocolIo) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mpac-process-fixture"));
    command.arg(mode);
    ProtocolProcess::spawn(command).unwrap()
}

async fn ready_tree(mode: &str) -> (ProtocolProcess, Vec<u32>) {
    let (owner, io) = spawn(mode);
    let mut stdout = BufReader::new(io.stdout);
    let pids = timeout(BUDGET, async {
        let mut pids = Vec::new();
        for role in ["root", "branch", "leaf"] {
            let mut line = String::new();
            assert!(stdout.read_line(&mut line).await.unwrap() > 0);
            let (actual_role, pid) = line.trim().split_once(' ').unwrap();
            assert_eq!(actual_role, role);
            pids.push(pid.parse::<u32>().unwrap());
        }
        pids
    })
    .await
    .expect("fixture readiness timed out");
    // Dropping the consumer pipes is independent from process ownership.
    drop(io.stdin);
    drop(io.stderr);
    (owner, pids)
}

#[cfg(unix)]
fn running(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    output.status.success() && !state.trim().is_empty() && !state.trim().starts_with('Z')
}

#[cfg(windows)]
fn running(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    // Only fixture PIDs are queried; no mutation or process-wide discovery.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0;
        let result = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        assert_ne!(result, 0);
        code == 259 // STILL_ACTIVE
    }
}

async fn assert_stopped(pids: &[u32]) {
    timeout(BUDGET, async {
        while pids.iter().copied().any(running) {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("owned fixture still running");
}

#[tokio::test]
async fn explicit_stop_terminates_children_and_grandchildren_and_is_idempotent() {
    let (mut owner, pids) = ready_tree("tree").await;
    assert!(pids.iter().copied().all(running));
    let first = owner.stop().await.unwrap();
    assert_eq!(owner.stop().await.unwrap(), first);
    assert_stopped(&pids).await;
}

#[tokio::test]
async fn root_exit_does_not_release_ownership_of_remaining_descendants() {
    let (mut owner, pids) = ready_tree("orphan").await;
    assert!(pids[1..].iter().copied().all(running));
    owner.stop().await.unwrap();
    assert_stopped(&pids).await;
}

#[tokio::test]
async fn dropping_owner_terminates_the_group() {
    let (owner, pids) = ready_tree("tree").await;
    drop(owner);
    assert_stopped(&pids).await;
}

#[tokio::test]
async fn aborting_supervisor_task_drops_its_owned_processes() {
    let (owner, pids) = ready_tree("tree").await;
    let task = tokio::spawn(async move {
        let _owner = owner;
        std::future::pending::<()>().await;
    });
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_stopped(&pids).await;
}

#[tokio::test]
async fn timed_out_work_is_followed_by_explicit_group_cleanup() {
    let (mut owner, pids) = ready_tree("tree").await;
    assert!(
        timeout(Duration::from_millis(30), std::future::pending::<()>())
            .await
            .is_err()
    );
    owner.stop().await.unwrap();
    assert_stopped(&pids).await;
}

#[tokio::test]
async fn stopping_one_session_does_not_stop_another() {
    let (mut first, first_pids) = ready_tree("tree").await;
    let (mut second, second_pids) = ready_tree("tree").await;
    first.stop().await.unwrap();
    assert_stopped(&first_pids).await;
    assert!(second_pids.iter().copied().all(running));
    second.stop().await.unwrap();
    assert_stopped(&second_pids).await;
}

#[tokio::test]
async fn protocol_io_keeps_stdin_stdout_and_stderr_separate() {
    let (mut owner, mut io) = spawn("echo");
    io.stdin.write_all(b"ping\n").await.unwrap();
    let mut stdout = BufReader::new(io.stdout);
    let mut stderr = BufReader::new(io.stderr);
    let (mut out, mut err) = (String::new(), String::new());
    timeout(BUDGET, async {
        stdout.read_line(&mut out).await.unwrap();
        stderr.read_line(&mut err).await.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(out, "pong\n");
    assert_eq!(err, "fixture-stderr\n");
    owner.stop().await.unwrap();
}

#[tokio::test]
async fn discarded_stderr_continues_draining_beyond_previous_capture_limit() {
    let (mut owner, io) = spawn("flood");
    let drain = tokio::spawn(discard_output(io.stderr));
    let mut stdout = BufReader::new(io.stdout);
    let mut line = String::new();
    timeout(BUDGET, stdout.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert!(line.starts_with("drained "));
    owner.stop().await.unwrap();
    timeout(BUDGET, drain).await.unwrap().unwrap().unwrap();
}

#[tokio::test]
async fn missing_executable_fails_without_starting_a_session() {
    let result = ProtocolProcess::spawn(Command::new("mpac-nonexistent-fixture-executable"));
    assert_eq!(result.err().unwrap().to_string(), "spawn_failed");
}
