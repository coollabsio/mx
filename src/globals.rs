//! Process-wide runtime settings derived from global CLI flags.
//!
//! Initialized once in [`crate::run`]; read anywhere via [`get`] or the accessor functions.
//! Before initialization (e.g. in unit tests) the accessors return defaults.

use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Debug, Clone, Default)]
pub struct Globals {
    pub json: bool,
    pub quiet: bool,
    /// Skip TLS certificate verification.
    pub insecure: bool,
    pub debug: bool,
    pub no_color: bool,
    pub disable_pager: bool,
    /// Extra request headers from `-H/--custom-header KEY:VALUE`.
    pub custom_headers: Vec<(String, String)>,
    /// Upload bandwidth limit in bytes per second.
    pub limit_upload: Option<u64>,
    /// Download bandwidth limit in bytes per second.
    pub limit_download: Option<u64>,
    /// `--config-dir` override (config file and `certs/CAs` live here).
    pub config_dir: Option<PathBuf>,
}

static GLOBALS: OnceLock<Globals> = OnceLock::new();
static DEFAULT: OnceLock<Globals> = OnceLock::new();

/// Stores the settings. Returns `false` if they were already initialized (the first call wins).
pub fn init(globals: Globals) -> bool {
    GLOBALS.set(globals).is_ok()
}

pub fn get() -> &'static Globals {
    GLOBALS
        .get()
        .unwrap_or_else(|| DEFAULT.get_or_init(Globals::default))
}

pub fn json() -> bool {
    get().json
}

/// `--json` in raw `argv` (or `MC_JSON`), for errors reported before the CLI is parsed.
pub fn json_requested(argv: &[String]) -> bool {
    argv.iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--json")
        || std::env::var("MC_JSON")
            .ok()
            .and_then(|value| crate::cli::parse_env_bool(&value).ok())
            .unwrap_or(false)
}

pub fn quiet() -> bool {
    get().quiet
}

pub fn insecure() -> bool {
    get().insecure
}

pub fn debug() -> bool {
    get().debug
}

pub fn no_color() -> bool {
    get().no_color
}

pub fn disable_pager() -> bool {
    get().disable_pager
}

pub fn custom_headers() -> &'static [(String, String)] {
    &get().custom_headers
}

pub fn limit_upload() -> Option<u64> {
    get().limit_upload
}

pub fn limit_download() -> Option<u64> {
    get().limit_download
}

pub fn config_dir() -> Option<&'static std::path::Path> {
    get().config_dir.as_deref()
}
