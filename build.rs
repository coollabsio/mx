//! Records the git commit and rustc version for `mx --version`.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
    println!("cargo:rerun-if-env-changed=MX_COMMIT_ID");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-env-changed=MX_RELEASE");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    let commit = std::env::var("MX_COMMIT_ID")
        .or_else(|_| std::env::var("GITHUB_SHA"))
        .ok()
        .filter(|sha| !sha.is_empty())
        .or_else(|| command_output("git", &["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=MX_COMMIT_ID={commit}");

    // mc-style release tag `RELEASE.2026-09-26T16-30-04Z` from the commit time (or
    // SOURCE_DATE_EPOCH / build time); MX_RELEASE overrides it.
    let release = std::env::var("MX_RELEASE")
        .ok()
        .filter(|tag| !tag.is_empty())
        .unwrap_or_else(|| {
            let epoch = std::env::var("SOURCE_DATE_EPOCH")
                .ok()
                .or_else(|| command_output("git", &["log", "-1", "--format=%ct"]))
                .and_then(|value| value.trim().parse::<i64>().ok())
                .unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0)
                });
            release_tag(epoch)
        });
    println!("cargo:rustc-env=MX_RELEASE={release}");

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    // `rustc 1.85.0 (4d91de4e4 2025-02-17)` -> `rustc1.85.0`, shaped like Go's `go1.24.0`.
    let version = command_output(&rustc, &["--version"])
        .and_then(|text| text.split_whitespace().nth(1).map(|v| format!("rustc{v}")))
        .unwrap_or_else(|| "rustc".to_string());
    println!("cargo:rustc-env=MX_RUSTC_VERSION={version}");
}

/// Unix seconds -> `RELEASE.YYYY-MM-DDTHH-MM-SSZ` (UTC).
fn release_tag(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "RELEASE.{year:04}-{month:02}-{day:02}T{:02}-{:02}-{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}
