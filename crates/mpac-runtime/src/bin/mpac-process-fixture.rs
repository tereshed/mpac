//! Fixed local fixtures for process-ownership tests; never launches an agent.
use std::{
    io::{self, BufRead, Write},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

fn marker(role: &str) {
    let line = format!("{role} {}\n", std::process::id());
    io::stdout().write_all(line.as_bytes()).unwrap();
    io::stdout().flush().unwrap();
}

fn child(mode: &str) {
    // Children intentionally outlive an early-exiting parent. Each fixture has
    // its own expiry as a final backstop for an interrupted test harness.
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg(mode)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    thread::spawn(move || {
        let _ = child.wait();
    });
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "tree" | "orphan" => {
            marker("root");
            child("branch");
            if mode == "orphan" {
                return;
            }
        }
        "branch" => {
            marker("branch");
            child("leaf");
        }
        "leaf" => marker("leaf"),
        "echo" => {
            let mut line = String::new();
            io::stdin().lock().read_line(&mut line).unwrap();
            if line == "ping\n" {
                io::stdout().write_all(b"pong\n").unwrap();
                io::stdout().flush().unwrap();
                io::stderr().write_all(b"fixture-stderr\n").unwrap();
                io::stderr().flush().unwrap();
            }
        }
        "flood" => {
            let mut stderr = io::stderr().lock();
            let chunk = [b'x'; 8192];
            for _ in 0..1280 {
                stderr.write_all(&chunk).unwrap();
            }
            stderr.flush().unwrap();
            marker("drained");
        }
        _ => {
            eprintln!("fixed fixture mode required");
            std::process::exit(2);
        }
    }
    thread::sleep(Duration::from_secs(20));
}
