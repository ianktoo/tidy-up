//! Shared helpers for the integration tests.
//!
//! Included per test file with `mod common;`, which is why it lives in a
//! subdirectory: Cargo compiles every top-level file in `tests/` as its own test
//! binary, but not the contents of a directory.

#![allow(dead_code)] // each test binary uses a different subset

use std::{fs, path::PathBuf, process::Output};

/// Name of the scratch base created inside the home directory.
///
/// Deliberately not dot-prefixed: a hidden name would interact with the
/// hidden-file rules and read confusingly next to `.tidy-up`.
const SCRATCH: &str = "tidy-up-tests";

/// A temporary folder under the user's home directory, deleted on drop.
///
/// The system temporary directory sits inside a protected path on two of the
/// three platforms (`%LOCALAPPDATA%\Temp` on Windows, `/private/var/folders/...`
/// on macOS). tidy-up carves those out precisely so ordinary use keeps working,
/// but a test of the guard itself should not lean on the carve-out, or it would
/// pass for the wrong reason. Home is used when it is available and writable;
/// otherwise the system temporary directory, which is correct on Linux and fine
/// in a container.
pub fn sandbox() -> tempfile::TempDir {
    if let Some(base) = sandbox_base() {
        if fs::create_dir_all(&base).is_ok() {
            if let Ok(dir) = tempfile::Builder::new().prefix("tidy-").tempdir_in(&base) {
                return dir;
            }
        }
    }
    tempfile::tempdir().expect("a temporary directory")
}

/// Where [`sandbox`] puts its folders, when a home directory can be found.
pub fn sandbox_base() -> Option<PathBuf> {
    std::env::var_os("TIDY_UP_TEST_HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .or_else(|| std::env::var_os("HOME"))
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(SCRATCH))
}

/// The user's home directory, which the guard treats as dangerous.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Runs the compiled binary with no terminal attached.
pub fn tidy(args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_tidy-up"))
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("binary runs")
}

/// Everything the run printed, on both streams.
pub fn all_output(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}
