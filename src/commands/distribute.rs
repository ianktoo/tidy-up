//! `tidy-up distribute`

use std::path::Path;

use anyhow::Result;
use console::style;

use crate::{
    cli::DistributeArgs,
    commands::print_execution,
    disk::SystemDisks,
    distribute::{
        Allocation, CollectOptions, Destination, Layout, Limits, SourceImpact, Strategy, Unit,
        allocate, build_plan, collect_units, probe_destinations, projected_fill, source_impact,
        validate_locations,
    },
    executor::execute,
    journal::Operation,
    ui,
};

/// Scans the sources, decides the split, previews it, and moves the files.
pub fn run(args: &DistributeArgs) -> Result<()> {
    let (sources, dests) = validate_locations(&args.from, &args.to)?;
    let rules = args.filter.to_rules()?;
    let destinations = probe_destinations(&dests, &SystemDisks)?;
    let strategy = if args.ratio.is_empty() {
        Strategy::from(args.strategy)
    } else {
        Strategy::Ratio(args.ratio.clone())
    };
    let limits = Limits {
        max_fill: args.max_fill,
        min_free: args.min_free,
        max_bytes: args.limit,
    };

    let spinner = ui::spinner("Scanning");
    let collected = collect_units(
        &sources,
        &CollectOptions {
            layout: args.layout,
            granularity: args.granularity,
            rules: &rules,
        },
    );
    spinner.finish_and_clear();
    let collected = collected?;
    let total: u64 = collected.units.iter().map(|u| u.size).sum();
    ui::info(&format!(
        "Found {} ({}) in {}",
        ui::plural(collected.units.len(), "movable item"),
        ui::format_size(total),
        ui::plural(sources.len(), "source folder")
    ));
    if args.layout == Layout::Organize {
        ui::hint("Organize sorts files one by one, so folders are not kept together.");
    }
    if collected.units.is_empty() {
        ui::print_skipped(Path::new(""), &collected.skipped, args.verbose);
        ui::success("Nothing to move.");
        return Ok(());
    }

    let allocation = allocate(
        &collected.units,
        &destinations,
        &strategy,
        &limits,
        args.prefer,
    )?;
    print_destinations(&destinations, &allocation, &limits);
    print_left_out(&collected.units, &allocation);
    if args.verbose {
        print_items(&collected.units, &destinations, &allocation);
    }
    ui::print_skipped(Path::new(""), &collected.skipped, args.verbose);
    if allocation.unit_count() == 0 {
        println!();
        ui::warn(
            "Nothing can be moved with these limits. Try a higher --max-fill or a different --to.",
        );
        return Ok(());
    }

    let journal_root = &destinations[0].path;
    let plan = build_plan(journal_root, &collected.units, &destinations, &allocation);
    print_sources(&source_impact(&sources, &plan, &destinations, &SystemDisks));

    if args.dry_run {
        println!();
        ui::warn("Dry run: nothing was changed.");
        return Ok(());
    }
    println!();
    let prompt = format!(
        "Move {} ({}) to {}?",
        ui::plural(plan.moves.len(), "file"),
        ui::format_size(plan.total_bytes()),
        ui::plural(destinations.len(), "destination")
    );
    if !ui::confirm(&prompt, true, args.yes)? {
        ui::info("Cancelled. Nothing was changed.");
        return Ok(());
    }

    let bar = ui::TransferBar::new("Distributing");
    let executed = execute(&plan, Operation::Distribute, |p| bar.update(p));
    bar.finish();
    print_execution(journal_root, &executed?);
    ui::hint("Emptied source folders are left in place; delete them if you no longer need them.");
    Ok(())
}

fn print_destinations(dests: &[Destination], allocation: &Allocation, limits: &Limits) {
    ui::heading("Destinations");
    let after = projected_fill(dests, allocation);
    for (i, dest) in dests.iter().enumerate() {
        println!(
            "  {}  {}",
            style(format!("#{}", i + 1)).cyan().bold(),
            dest.path.display()
        );
        println!(
            "      {} -> {} full  {}  ({}), room for {}",
            ui::percent(dest.space.used_fraction()),
            style(ui::percent(after[i])).bold(),
            style(format!("+{}", ui::format_size(allocation.bytes[i]))).green(),
            ui::plural(allocation.per_dest[i].len(), "item"),
            ui::format_size(allocation.capacity[i])
        );
    }
    let cap = format!("never above {} full", ui::percent(limits.max_fill));
    if limits.min_free > 0 {
        ui::hint(&format!(
            "{cap}, at least {} kept free",
            ui::format_size(limits.min_free)
        ));
    } else {
        ui::hint(&cap);
    }
    println!(
        "  {}",
        style(format!(
            "{} · {} in total",
            ui::plural(allocation.unit_count(), "item"),
            ui::format_size(allocation.total_bytes())
        ))
        .bold()
    );
}

fn print_left_out(units: &[Unit], allocation: &Allocation) {
    let sum = |indexes: &[usize]| indexes.iter().map(|&i| units[i].size).sum::<u64>();
    if !allocation.unplaced.is_empty() {
        ui::warn(&format!(
            "{} ({}) do not fit on any destination within the limits",
            ui::plural(allocation.unplaced.len(), "item"),
            ui::format_size(sum(&allocation.unplaced))
        ));
    }
    if !allocation.over_limit.is_empty() {
        ui::info(&format!(
            "{} ({}) left out by --limit",
            ui::plural(allocation.over_limit.len(), "item"),
            ui::format_size(sum(&allocation.over_limit))
        ));
    }
}

fn print_items(units: &[Unit], dests: &[Destination], allocation: &Allocation) {
    ui::heading("Items");
    for (i, dest) in dests.iter().enumerate() {
        println!(
            "  {}  {}",
            style(format!("#{}", i + 1)).cyan().bold(),
            dest.path.display()
        );
        for &index in &allocation.per_dest[i] {
            let unit = &units[index];
            ui::hint(&format!("{}  ({})", unit.name, ui::format_size(unit.size)));
        }
    }
}

fn print_sources(impacts: &[SourceImpact]) {
    if impacts.is_empty() {
        return;
    }
    ui::heading("Sources");
    for s in impacts {
        let note = if s.freed == 0 {
            "same partition as the destinations, so no space is freed".to_string()
        } else {
            format!("frees {}", style(ui::format_size(s.freed)).green())
        };
        println!(
            "  {}\n      {} -> {} full  ({note})",
            s.path.display(),
            ui::percent(s.before),
            style(ui::percent(s.after)).bold()
        );
    }
}
