//! `tidy-up analyze`

use std::path::Path;

use anyhow::Result;
use console::style;

use crate::{
    analyze::{Analysis, AnalyzeOptions, DuplicateSummary, analyze},
    cli::AnalyzeArgs,
    dedupe::find_duplicates,
    disk::SystemDisks,
    fsops::resolve_root,
    plan::DUPLICATES_DIR,
    rules::IgnoreRules,
    scan::{ScanOptions, scan},
    timefmt::format_utc,
    ui,
};

const BAR_WIDTH: usize = 24;

/// Analyzes each folder and prints a report (or JSON with `--json`).
pub fn run(args: &AnalyzeArgs) -> Result<()> {
    let options = AnalyzeOptions {
        top: args.top as usize,
        ..Default::default()
    };
    let mut reports = Vec::new();
    for path in &args.paths {
        let root = resolve_root(path)?;
        let spinner = ui::spinner("Analyzing");
        let result = analyze(&root, &options, &SystemDisks, &mut |files| {
            spinner.set_message(format!(
                "Analyzing, {} so far",
                ui::plural(files as usize, "file")
            ));
        });
        spinner.finish_and_clear();
        let mut analysis = result?;
        if args.duplicates {
            analysis.duplicates = Some(duplicate_summary(&root)?);
        }
        reports.push(analysis);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        for report in &reports {
            print_report(report);
        }
    }
    Ok(())
}

/// Runs the duplicate pipeline over `root` and summarises the waste.
fn duplicate_summary(root: &Path) -> Result<DuplicateSummary> {
    let options = ScanOptions {
        max_depth: usize::MAX,
        rules: IgnoreRules::new(),
        skip_root_dirs: [DUPLICATES_DIR.to_lowercase()].into(),
    };
    let scanned = scan(root, &options)?;
    let bar = ui::HashBar::new();
    let report = find_duplicates(&scanned.files, &bar);
    bar.finish();
    Ok(DuplicateSummary::from_report(&report))
}

fn share(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

fn print_report(a: &Analysis) {
    let root = Path::new(&a.root);
    ui::heading(&format!("Analysis of {}", a.root));
    println!(
        "  {} in {} · {}",
        ui::plural(a.files as usize, "file"),
        ui::plural(a.folders as usize, "folder"),
        style(ui::format_size(a.bytes)).bold()
    );
    let mut notes = Vec::new();
    if a.empty_files > 0 {
        notes.push(ui::plural(a.empty_files as usize, "empty file"));
    }
    if a.empty_folders > 0 {
        notes.push(ui::plural(a.empty_folders as usize, "empty folder"));
    }
    if a.skipped > 0 {
        notes.push(format!(
            "{} not counted (links or unreadable)",
            ui::plural(a.skipped as usize, "item")
        ));
    }
    if !notes.is_empty() {
        ui::hint(&notes.join(", "));
    }
    if let Some(disk) = &a.disk {
        let used = share(disk.used, disk.total);
        println!(
            "  Partition  {} {} used · {} of {} · {} free",
            ui::bar(used, BAR_WIDTH),
            ui::percent(used),
            ui::format_size(disk.used),
            ui::format_size(disk.total),
            ui::format_size(disk.available)
        );
    }
    if a.files == 0 {
        ui::hint("No files found.");
        return;
    }

    ui::heading("By type");
    for c in &a.by_category {
        let part = share(c.bytes, a.bytes);
        println!(
            "  {:<14} {:>10}  {:>10}  {:>6}  {}",
            c.category,
            ui::plural(c.files as usize, "file"),
            ui::format_size(c.bytes),
            ui::percent(part),
            style(ui::bar(part, BAR_WIDTH)).cyan()
        );
    }

    if !a.largest_files.is_empty() {
        ui::heading("Largest files");
        for f in &a.largest_files {
            let date = f
                .modified
                .map(|m| format_utc(m)[..10].to_string())
                .unwrap_or_else(|| "unknown".into());
            println!(
                "  {:>10}  {}  {}",
                ui::format_size(f.bytes),
                style(date).dim(),
                ui::rel(root, Path::new(&f.path))
            );
        }
    }
    if !a.largest_folders.is_empty() {
        ui::heading("Largest folders");
        for f in &a.largest_folders {
            println!(
                "  {:>10}  {:>10}  {}",
                ui::format_size(f.bytes),
                ui::plural(f.files as usize, "file"),
                ui::rel(root, Path::new(&f.path))
            );
        }
    }
    if a.project_count > 0 {
        ui::heading("Code projects");
        println!(
            "  {} using {} ({} of the total)",
            ui::plural(a.project_count as usize, "project"),
            ui::format_size(a.project_bytes),
            ui::percent(share(a.project_bytes, a.bytes))
        );
        for p in &a.largest_projects {
            println!(
                "  {:>10}  {}",
                ui::format_size(p.bytes),
                ui::rel(root, Path::new(&p.path))
            );
        }
    }

    ui::heading("By age (last modified)");
    for bucket in &a.age {
        if bucket.files == 0 {
            continue;
        }
        println!(
            "  {:<20} {:>10}  {:>10}  {}",
            bucket.label,
            ui::plural(bucket.files as usize, "file"),
            ui::format_size(bucket.bytes),
            style(ui::bar(share(bucket.bytes, a.bytes), BAR_WIDTH)).cyan()
        );
    }

    if let Some(d) = &a.duplicates {
        ui::heading("Duplicates");
        if d.groups == 0 {
            ui::success("No duplicate content found.");
        } else {
            println!(
                "  {} in {} waste {}",
                ui::plural(d.extra_copies, "extra copy"),
                ui::plural(d.groups, "group"),
                style(ui::format_size(d.reclaimable_bytes)).bold()
            );
            ui::hint(&format!("Isolate them with: tidy-up dedupe \"{}\"", a.root));
        }
    } else {
        ui::hint(
            "Add --duplicates to measure space wasted by duplicate files (reads file contents).",
        );
    }
}
