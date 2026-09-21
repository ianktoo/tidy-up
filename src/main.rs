//! `tidy-up` binary entry point.

use std::process::ExitCode;

use console::style;

fn main() -> ExitCode {
    match tidy_up::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{} {err:#}", style("error:").red().bold());
            ExitCode::FAILURE
        }
    }
}
