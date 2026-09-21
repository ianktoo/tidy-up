//! Real cross-volume tests.
//!
//! Moving files between partitions is the riskiest thing tidy-up does: `rename` fails across
//! volumes, so a move becomes copy, verify, delete. That path cannot be exercised on a single
//! volume, so these tests need a writable folder on a **different** volume than the temp dir.
//!
//! Set `TIDY_UP_OTHER_VOLUME` to such a folder. CI does on every OS (a tmpfs on Linux, a RAM
//! disk on macOS, the `D:` drive on Windows) and also sets `TIDY_UP_REQUIRE_CROSS_VOLUME=1` so
//! a missing volume fails the build instead of silently skipping these tests.
//!
//! Locally, on Linux or WSL: `TIDY_UP_OTHER_VOLUME=/dev/shm cargo test --test cross_volume`.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use tempfile::TempDir;
use tidy_up::{
    disk::{DiskProbe, SystemDisks, volume_id},
    distribute::{
        CollectOptions, Granularity, Layout, Limits, Prefer, Strategy, allocate, build_plan,
        collect_units, probe_destinations, source_impact, validate_locations,
    },
    executor::execute,
    fsops::{move_path, resolve_root},
    journal::{Journal, Operation},
    restore::{RestoreOptions, restore},
    rules::IgnoreRules,
};

/// Two temp folders that live on different volumes, or `None` if the environment has no
/// second volume (in which case the test is skipped, unless CI requires it).
struct TwoVolumes {
    _local: TempDir,
    _other: TempDir,
    local: PathBuf,
    other: PathBuf,
}

fn two_volumes() -> Option<TwoVolumes> {
    let Some(other_root) = std::env::var_os("TIDY_UP_OTHER_VOLUME") else {
        assert!(
            std::env::var_os("TIDY_UP_REQUIRE_CROSS_VOLUME").is_none(),
            "TIDY_UP_REQUIRE_CROSS_VOLUME is set but TIDY_UP_OTHER_VOLUME is not"
        );
        eprintln!("skipped: set TIDY_UP_OTHER_VOLUME to a folder on another volume to run this");
        return None;
    };
    let local = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir_in(&other_root).unwrap();
    let (l, o) = (
        resolve_root(local.path()).unwrap(),
        resolve_root(other.path()).unwrap(),
    );
    assert_ne!(
        volume_id(&l).unwrap(),
        volume_id(&o).unwrap(),
        "TIDY_UP_OTHER_VOLUME ({}) is on the same volume as the temp dir ({})",
        o.display(),
        l.display()
    );
    Some(TwoVolumes {
        _local: local,
        _other: other,
        local: l,
        other: o,
    })
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
        let Ok(read) = fs::read_dir(dir) else { return };
        for entry in read.flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == ".tidy-up") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read_to_string(&path).unwrap_or_default(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn the_probe_sees_two_different_partitions() {
    let Some(v) = two_volumes() else { return };
    let (a, b) = (
        SystemDisks.space(&v.local).unwrap(),
        SystemDisks.space(&v.other).unwrap(),
    );
    assert_ne!(a.volume, b.volume);
    assert!(a.total > 0 && b.total > 0);
    assert!(a.available <= a.total && b.available <= b.total);
}

#[test]
fn a_plain_rename_really_fails_across_these_volumes() {
    // This is the precondition that makes the tests below meaningful on every OS.
    let Some(v) = two_volumes() else { return };
    write(&v.local, "probe.txt", "x");
    assert!(
        fs::rename(v.local.join("probe.txt"), v.other.join("probe.txt")).is_err(),
        "rename unexpectedly succeeded: the folders are not on separate volumes"
    );
}

#[test]
fn moving_a_file_across_volumes_copies_verifies_deletes_and_keeps_its_date() {
    let Some(v) = two_volumes() else { return };
    write(&v.local, "report.bin", &"data".repeat(50_000));
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);
    fs::File::options()
        .write(true)
        .open(v.local.join("report.bin"))
        .unwrap()
        .set_modified(old)
        .unwrap();

    move_path(&v.local.join("report.bin"), &v.other.join("out/report.bin")).unwrap();

    assert!(!v.local.join("report.bin").exists());
    assert_eq!(
        fs::read_to_string(v.other.join("out/report.bin")).unwrap(),
        "data".repeat(50_000)
    );
    let drift = fs::metadata(v.other.join("out/report.bin"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(old)
        .unwrap_or_else(|e| e.duration())
        .as_secs();
    assert!(
        drift <= 2,
        "modified date must survive a cross-volume move ({drift}s off)"
    );
}

#[test]
fn a_cross_volume_move_never_overwrites() {
    let Some(v) = two_volumes() else { return };
    write(&v.local, "a.txt", "new");
    write(&v.other, "a.txt", "existing");
    assert!(move_path(&v.local.join("a.txt"), &v.other.join("a.txt")).is_err());
    assert_eq!(
        fs::read_to_string(v.other.join("a.txt")).unwrap(),
        "existing"
    );
    assert!(v.local.join("a.txt").exists());
}

#[test]
fn distribute_across_real_volumes_frees_the_source_partition_and_restores_exactly() {
    let Some(v) = two_volumes() else { return };
    let src = v.local.join("src");
    for i in 0..8 {
        write(&src, &format!("album/{i}.dat"), &"x".repeat(2048));
    }
    write(&src, "loose.txt", "hello");
    write(&src, "repo/.git/HEAD", "ref: refs/heads/main");
    write(&src, "repo/Cargo.toml", "[package]");
    let before = snapshot(&src);
    let total: u64 = 8 * 2048 + 5 + 20 + 9;

    let (sources, dests) =
        validate_locations(std::slice::from_ref(&src), &[v.other.join("archive")]).unwrap();
    let destinations = probe_destinations(&dests, &SystemDisks).unwrap();
    assert_ne!(
        destinations[0].space.volume,
        SystemDisks.space(&sources[0]).unwrap().volume
    );
    let rules = IgnoreRules::new();
    let collected = collect_units(
        &sources,
        &CollectOptions {
            layout: Layout::Keep,
            granularity: Granularity::Item,
            rules: &rules,
        },
    )
    .unwrap();
    let limits = Limits {
        max_fill: 1.0,
        ..Default::default()
    };
    let allocation = allocate(
        &collected.units,
        &destinations,
        &Strategy::Fill,
        &limits,
        Prefer::Largest,
    )
    .unwrap();
    assert!(
        allocation.unplaced.is_empty(),
        "the second volume must have room for the test data"
    );
    let plan = build_plan(&dests[0], &collected.units, &destinations, &allocation);

    let impact = source_impact(&sources, &plan, &destinations, &SystemDisks);
    assert_eq!(
        impact[0].freed, total,
        "everything leaves the source's partition"
    );

    let report = execute(&plan, Operation::Distribute, |_| {}).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(snapshot(&src).len(), 0, "the source is emptied");
    assert!(
        dests[0].join("album/0.dat").exists(),
        "the album moved into the destination"
    );
    assert!(
        dests[0].join("repo/.git/HEAD").exists(),
        "the .git folder travelled with its project"
    );

    let mut journal = Journal::find(&dests[0], &report.journal_id).unwrap();
    let restored = restore(
        &dests[0],
        &mut journal,
        RestoreOptions::default(),
        |_, _| {},
    )
    .unwrap();
    assert!(restored.is_clean(), "{restored:?}");
    assert_eq!(
        snapshot(&src),
        before,
        "restore across volumes returns the exact original tree"
    );
}
