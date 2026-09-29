//! `tidy-up` binary entry point.
//!
//! Everything, including reporting and the exit code, is decided in
//! [`tidy_up::run`]; this only hands the code to the operating system.

use std::process::ExitCode;

fn main() -> ExitCode {
    tidy_up::run().into()
}
