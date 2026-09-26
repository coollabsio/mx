//! Records the git commit and rustc version for `mx --version`.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    println!("cargo:rerun-if-env-changed=MX_COMMIT_ID");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");

    let commit = std::env::var("MX_COMMIT_ID")
        .or_else(|_| std::env::var("GITHUB_SHA"))
        .ok()
        .filter(|sha| !sha.is_empty())
        .or_else(|| command_output("git", &["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=MX_COMMIT_ID={commit}");

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    // `rustc 1.85.0 (4d91de4e4 2025-02-17)` -> `rustc1.85.0`, shaped like Go's `go1.24.0`.
    let version = command_output(&rustc, &["--version"])
        .and_then(|text| text.split_whitespace().nth(1).map(|v| format!("rustc{v}")))
        .unwrap_or_else(|| "rustc".to_string());
    println!("cargo:rustc-env=MX_RUSTC_VERSION={version}");
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}
