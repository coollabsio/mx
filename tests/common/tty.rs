//! Runs a command on a pseudo-terminal (util-linux `script`) and types an answer, for
//! interactive flows (TLS trust prompt, `replicate backlog` view).
#![allow(dead_code)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// True when `program --version` runs.
pub fn have(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok()
}

/// Runs `program args` (shell words) on a pseudo-terminal with HOME=`home`, typing `answer`
/// after 1.5 s. Returns the terminal output and whether the command succeeded.
pub fn run(program: &Path, home: &Path, args: &str, answer: &str) -> (String, bool) {
    let command = format!("{} {args}", program.display());
    let mut child = Command::new("script")
        .args(["-qec", &command, "/dev/null"])
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("script");
    let mut stdin = child.stdin.take().expect("stdin");
    std::thread::sleep(Duration::from_millis(1500));
    let _ = stdin.write_all(answer.as_bytes());
    let _ = stdin.flush();
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().expect("wait").is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("`{args}` did not finish on the tty");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(stdin);
    let out = child.wait_with_output().expect("output");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.success(),
    )
}
