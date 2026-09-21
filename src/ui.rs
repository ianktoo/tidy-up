//! Terminal output and prompts. All colour/progress/interaction lives here so the
//! rest of the crate stays testable and presentation-free.

use std::{
    collections::BTreeMap,
    io::IsTerminal,
    path::Path,
    time::Duration,
};

use anyhow::{Result, bail};
use console::{Term, style};
use indicatif::{ProgressBar, ProgressStyle};

use crate::{dedupe::HashProgress, executor::Progress, plan::Plan, scan::Skipped};

/// How many example lines to show per section unless `--verbose` is given.
const PREVIEW_LIMIT: usize = 8;

/// Formats a byte count as `1.5 MiB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Renders `path` relative to `root` when possible.
pub fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}

/// `1 file` / `2 files` / `2 copies` (consonant + `y` becomes `ies`).
pub fn plural(count: usize, singular: &str) -> String {
    if count == 1 {
        return format!("{count} {singular}");
    }
    let mut chars = singular.chars().rev();
    match (chars.next(), chars.next()) {
        (Some('y'), Some(prev)) if !"aeiou".contains(prev) => {
            format!("{count} {}ies", &singular[..singular.len() - 1])
        }
        _ => format!("{count} {singular}s"),
    }
}

/// Whether stdin and stdout are attached to a terminal.
pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && Term::stdout().is_term()
}

/// Prints the program banner.
pub fn banner() {
    println!(
        "{} {}",
        style("tidy-up").cyan().bold(),
        style(format!("v{}", env!("CARGO_PKG_VERSION"))).dim()
    );
    println!("{}\n", style("Organize folders by file type, and undo it any time.").dim());
}

/// Section heading.
pub fn heading(text: &str) {
    println!("\n{}", style(text).bold().underlined());
}

/// Success line.
pub fn success(text: &str) {
    println!("{} {text}", style("✔").green().bold());
}

/// Neutral information line.
pub fn info(text: &str) {
    println!("{} {text}", style("•").cyan());
}

/// Warning line.
pub fn warn(text: &str) {
    println!("{} {text}", style("!").yellow().bold());
}

/// Dimmed hint line.
pub fn hint(text: &str) {
    println!("  {}", style(text).dim());
}

/// Spinner for work of unknown length; call `finish_and_clear` when done.
pub fn spinner(message: &str) -> ProgressBar {
    let bar = ProgressBar::new_spinner();
    bar.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg} {pos} ")
            .expect("valid template")
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );
    bar.set_message(message.to_string());
    bar.enable_steady_tick(Duration::from_millis(80));
    bar
}

/// Determinate progress bar.
pub fn progress_bar(len: usize, message: &str) -> ProgressBar {
    let bar = ProgressBar::new(len as u64);
    bar.set_style(
        ProgressStyle::with_template("{msg} [{bar:40.cyan/blue}] {pos}/{len}")
            .expect("valid template")
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    bar.set_message(message.to_string());
    bar
}

const BYTE_BAR: &str =
    "{prefix:.bold} [{bar:30.cyan/blue}] {bytes}/{total_bytes} {binary_bytes_per_sec} eta {eta}";

fn byte_bar(template: &str) -> ProgressBar {
    let bar = ProgressBar::new(1);
    bar.set_style(
        ProgressStyle::with_template(template)
            .expect("valid template")
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    bar
}

/// Byte-based progress for moving files: shows throughput, ETA, and the current file.
///
/// Byte progress (rather than item counts) keeps the bar honest when one item is a
/// multi-gigabyte video being copied between drives.
pub struct TransferBar(ProgressBar);

impl TransferBar {
    /// Creates a bar labelled `label`. It stays hidden when stderr is not a terminal.
    pub fn new(label: &str) -> Self {
        let bar = byte_bar(&format!("{BYTE_BAR}  {{wide_msg}}"));
        bar.set_prefix(label.to_string());
        Self(bar)
    }

    /// Reflects an [`executor::Progress`](crate::executor::Progress) snapshot.
    pub fn update(&self, progress: &Progress) {
        self.0.set_length(progress.bytes_total.max(1));
        self.0.set_position(progress.bytes_done);
        let name = progress
            .current
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.0.set_message(format!("{}/{} {name}", progress.done, progress.total));
    }

    /// Removes the bar from the screen.
    pub fn finish(&self) {
        self.0.finish_and_clear();
    }
}

/// Progress for content hashing; one bar that restarts for each pass.
pub struct HashBar(ProgressBar);

impl HashBar {
    /// Creates the bar (hidden until the first pass starts, and when not on a terminal).
    pub fn new() -> Self {
        Self(byte_bar(BYTE_BAR))
    }

    /// Removes the bar from the screen.
    pub fn finish(&self) {
        self.0.finish_and_clear();
    }
}

impl Default for HashBar {
    fn default() -> Self {
        Self::new()
    }
}

impl HashProgress for HashBar {
    fn stage(&self, label: &'static str, files: usize, bytes: u64) {
        self.0.reset();
        self.0.set_length(bytes.max(1));
        self.0.set_position(0);
        self.0.set_prefix(format!("{label} ({})", plural(files, "file")));
    }

    fn advance(&self, bytes: u64) {
        self.0.inc(bytes);
    }
}

/// Asks a yes/no question. `assume_yes` short-circuits; without a terminal the
/// answer is an error rather than a silent "no" so scripts fail loudly.
pub fn confirm(prompt: &str, default: bool, assume_yes: bool) -> Result<bool> {
    if assume_yes {
        return Ok(true);
    }
    if !is_interactive() {
        bail!("cannot ask \"{prompt}\" without a terminal; re-run with --yes to proceed");
    }
    Ok(dialoguer::Confirm::new()
        .with_prompt(prompt)
        .default(default)
        .interact()?)
}

/// Prints a summary of a plan: per-folder totals, then (optionally) every move.
pub fn print_plan(plan: &Plan, verbose: bool) {
    heading("Plan");
    for (folder, (count, bytes)) in plan.by_folder() {
        let label = if folder.is_empty() { "(folder root)".to_string() } else { format!("{folder}/") };
        println!(
            "  {:<16} {:>6}  {:>10}",
            style(label).green(),
            count,
            style(format_size(bytes)).dim()
        );
    }
    println!(
        "  {}",
        style(format!(
            "{} · {} total",
            plural(plan.moves.len(), "item"),
            format_size(plan.total_bytes())
        ))
        .bold()
    );

    let shown = if verbose { plan.moves.len() } else { PREVIEW_LIMIT.min(plan.moves.len()) };
    if shown > 0 {
        println!();
    }
    for m in &plan.moves[..shown] {
        println!(
            "  {} {} {}",
            rel(&plan.root, &m.from),
            style("→").dim(),
            style(rel(&plan.root, &m.to)).cyan()
        );
    }
    if shown < plan.moves.len() {
        hint(&format!(
            "… and {} more (use --verbose to list everything)",
            plan.moves.len() - shown
        ));
    }
}

/// Prints skipped entries grouped by reason.
pub fn print_skipped(root: &Path, skipped: &[Skipped], verbose: bool) {
    if skipped.is_empty() {
        return;
    }
    let mut groups: BTreeMap<&'static str, Vec<&Skipped>> = BTreeMap::new();
    for s in skipped {
        groups.entry(s.reason.label()).or_default().push(s);
    }
    heading("Left alone");
    for (label, entries) in groups {
        println!("  {} {}", style(plural(entries.len(), "item")).yellow(), label);
        if verbose {
            for s in entries {
                hint(&format!("{}  ({})", rel(root, &s.path), s.reason));
            }
        }
    }
}

/// Prints an error in the house style (used for non-fatal errors in interactive mode).
pub fn error(err: &anyhow::Error) {
    eprintln!("{} {err:#}", style("error:").red().bold());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_format_humanly() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KiB");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(5 * 1024 * 1024 * 1024), "5.0 GiB");
        assert_eq!(format_size(u64::MAX), "16777216.0 TiB");
    }

    #[test]
    fn plural_handles_one_and_many() {
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(0, "file"), "0 files");
        assert_eq!(plural(3, "file"), "3 files");
        assert_eq!(plural(2, "copy"), "2 copies");
        assert_eq!(plural(1, "copy"), "1 copy");
        assert_eq!(plural(2, "key"), "2 keys");
    }

    #[test]
    fn rel_strips_root_when_possible() {
        assert_eq!(rel(Path::new("/r"), Path::new("/r/a/b.txt")), Path::new("a/b.txt").display().to_string());
        assert_eq!(rel(Path::new("/r"), Path::new("/x/y")), Path::new("/x/y").display().to_string());
    }

    #[test]
    fn confirm_short_circuits_with_assume_yes() {
        assert!(confirm("sure?", false, true).unwrap());
    }

    #[test]
    fn confirm_without_terminal_errors_instead_of_guessing() {
        if !is_interactive() {
            assert!(confirm("sure?", true, false).is_err());
        }
    }
}
