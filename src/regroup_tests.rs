//! Tests for [`crate::regroup`].

use std::time::{Duration, UNIX_EPOCH};

use super::*;

/// 2024-03-17 09:00:00 UTC
const MARCH_2024: u64 = 1_710_666_000;

fn file(name: &str, size: u64, modified: Option<u64>) -> FileEntry {
    FileEntry {
        path: PathBuf::from("/root").join(name),
        size,
        modified: modified.map(|s| UNIX_EPOCH + Duration::from_secs(s)),
        depth: 1,
        source: 0,
    }
}

// ---------------------------------------------------------------------------
// Buckets
// ---------------------------------------------------------------------------

#[test]
fn every_key_names_the_bucket_you_would_expect() {
    let photo = file("holiday.JPG", 5 << 20, Some(MARCH_2024));
    assert_eq!(GroupBy::Type.bucket(&photo, 0), "Images");
    assert_eq!(GroupBy::Ext.bucket(&photo, 0), "JPG");
    assert_eq!(GroupBy::Year.bucket(&photo, 0), "2024");
    assert_eq!(GroupBy::Month.bucket(&photo, 0), "03-March");
    assert_eq!(GroupBy::Day.bucket(&photo, 0), "2024-03-17");
    assert_eq!(GroupBy::Size.bucket(&photo, 0), "Small (under 10 MiB)");
    assert_eq!(GroupBy::Alpha.bucket(&photo, 0), "H");
}

/// The categories must be the ones `organize` uses, or the two commands would
/// disagree about where a file belongs and fight each other.
#[test]
fn the_type_key_reuses_the_organize_categories() {
    for (name, want) in [
        ("a.pdf", "Documents"),
        ("a.stl", "3D Models"),
        ("a.rs", "Code"),
        ("a.unknown-extension", "Other"),
    ] {
        let entry = file(name, 1, None);
        assert_eq!(GroupBy::Type.bucket(&entry, 0), want);
        assert_eq!(
            GroupBy::Type.bucket(&entry, 0),
            Category::from_path(&entry.path).folder_name()
        );
    }
}

/// No file may ever be left without a bucket, or it would silently vanish from
/// the plan.
#[test]
fn every_key_is_total() {
    let awkward = [
        file("Makefile", 0, None),
        file(".hidden", 1, None),
        file("2024-report.pdf", 1, Some(0)),
        file("étude.mp3", 1, None),
        file("日本語.txt", 1, None),
        file("...", 1, None),
    ];
    let keys = [
        GroupBy::Type,
        GroupBy::Ext,
        GroupBy::Year,
        GroupBy::Month,
        GroupBy::Day,
        GroupBy::Size,
        GroupBy::Alpha,
    ];
    for entry in &awkward {
        for key in keys {
            let bucket = key.bucket(entry, 0);
            assert!(
                !bucket.is_empty(),
                "{key:?} produced nothing for {}",
                entry.path.display()
            );
            assert_eq!(bucket, sanitize(&bucket), "{key:?} produced an unsafe name");
        }
    }
}

#[test]
fn a_file_with_no_timestamp_gets_its_own_bucket_rather_than_a_guess() {
    let undated = file("mystery.bin", 1, None);
    for key in [GroupBy::Year, GroupBy::Month, GroupBy::Day] {
        assert_eq!(key.bucket(&undated, 0), "Unknown date");
    }
}

#[test]
fn a_file_with_no_extension_is_named_as_such() {
    assert_eq!(
        GroupBy::Ext.bucket(&file("Makefile", 1, None), 0),
        "No extension"
    );
    assert_eq!(
        GroupBy::Ext.bucket(&file("archive.tar.gz", 1, None), 0),
        "GZ"
    );
}

/// Off-by-one at a band edge is the classic bug here, so every boundary is
/// checked exactly.
#[test]
fn size_bands_are_checked_on_their_exact_boundaries() {
    let band = |size| GroupBy::Size.bucket(&file("x.bin", size, None), 0);
    assert_eq!(band(0), "Tiny (under 1 MiB)");
    assert_eq!(band((1 << 20) - 1), "Tiny (under 1 MiB)");
    assert_eq!(
        band(1 << 20),
        "Small (under 10 MiB)",
        "exactly 1 MiB is not tiny"
    );
    assert_eq!(band((10 << 20) - 1), "Small (under 10 MiB)");
    assert_eq!(band(10 << 20), "Medium (under 100 MiB)");
    assert_eq!(band((100 << 20) - 1), "Medium (under 100 MiB)");
    assert_eq!(band(100 << 20), "Large (under 1 GiB)");
    assert_eq!(band((1 << 30) - 1), "Large (under 1 GiB)");
    assert_eq!(band(1 << 30), "Huge (1 GiB and up)");
    assert_eq!(band(u64::MAX), "Huge (1 GiB and up)");
}

#[test]
fn the_alpha_key_separates_letters_digits_and_everything_else() {
    let bucket = |name| GroupBy::Alpha.bucket(&file(name, 1, None), 0);
    assert_eq!(bucket("apple.txt"), "A");
    assert_eq!(bucket("Apple.txt"), "A", "case does not split a bucket");
    assert_eq!(bucket("2024-notes.txt"), "0-9");
    assert_eq!(bucket("_scratch.txt"), "Other");
    assert_eq!(
        bucket("étude.mp3"),
        "É",
        "a non-ASCII letter keeps its identity"
    );
}

#[test]
fn month_folders_sort_in_calendar_order() {
    // 2024-01-15 and 2024-11-15
    let january = GroupBy::Month.bucket(&file("a", 1, Some(1_705_320_000)), 0);
    let november = GroupBy::Month.bucket(&file("b", 1, Some(1_731_672_000)), 0);
    assert_eq!(january, "01-January");
    assert_eq!(november, "11-November");
    assert!(
        january < november,
        "the numeric prefix is what makes this work"
    );
}

/// Grouping by date has to honour the offset, or a late-evening file lands in
/// the wrong year.
#[test]
fn date_buckets_respect_the_utc_offset() {
    let new_years_eve = file("party.jpg", 1, Some(1_704_061_800));
    assert_eq!(GroupBy::Year.bucket(&new_years_eve, 0), "2023");
    assert_eq!(GroupBy::Year.bucket(&new_years_eve, 3 * 3600), "2024");
}

// ---------------------------------------------------------------------------
// Sanitizing
// ---------------------------------------------------------------------------

/// Buckets come from file contents, so they can contain anything. A folder name
/// the file system rejects would fail every move into it.
#[test]
fn bucket_names_are_made_safe_for_every_platform() {
    assert_eq!(sanitize("a/b"), "a_b");
    assert_eq!(sanitize(r"a\b"), "a_b");
    assert_eq!(sanitize("a:b"), "a_b");
    assert_eq!(sanitize("a*b?c\"d<e>f|g"), "a_b_c_d_e_f_g");
    assert_eq!(sanitize("tab\there"), "tab_here");
    assert_eq!(
        sanitize("trailing."),
        "trailing",
        "Windows strips these silently"
    );
    assert_eq!(sanitize("trailing   "), "trailing");
    assert_eq!(sanitize("  padded  "), "padded");
    assert_eq!(sanitize(""), "Other");
    assert_eq!(sanitize("..."), "Other");
    assert_eq!(
        sanitize("日本語"),
        "日本語",
        "non-ASCII names are left alone"
    );
}

/// `con.txt` is an ordinary file name, and `CON` is a folder Windows will not
/// create. Grouping by extension turns the first into the second.
#[test]
fn reserved_device_names_are_made_creatable() {
    assert_eq!(GroupBy::Ext.bucket(&file("driver.con", 1, None), 0), "CON_");
    for reserved in ["con", "PRN", "aux", "NUL", "com1", "LPT9"] {
        let safe = sanitize(reserved);
        assert!(
            safe.ends_with('_'),
            "{reserved} must be escaped, got {safe}"
        );
    }
    assert_eq!(
        sanitize("CONFIG"),
        "CONFIG",
        "only the exact names are reserved"
    );
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

fn scanned(files: Vec<FileEntry>) -> ScanResult {
    ScanResult {
        files,
        projects: vec![],
        skipped: vec![],
    }
}

fn at(root: &Path, rel: &str, size: u64, modified: Option<u64>) -> FileEntry {
    FileEntry {
        path: root.join(rel),
        size,
        modified: modified.map(|s| UNIX_EPOCH + Duration::from_secs(s)),
        depth: rel.matches('/').count() + 1,
        source: 0,
    }
}

#[test]
fn keys_nest_in_the_order_they_are_given() {
    let root = Path::new("/root");
    let scan = scanned(vec![at(root, "old/mess/photo.jpg", 1, Some(MARCH_2024))]);
    let plan = build_reorganize_plan(
        root,
        &scan,
        &[GroupBy::Year, GroupBy::Month, GroupBy::Type],
        ProjectPolicy::Keep,
        0,
    );
    assert_eq!(
        plan.plan.moves[0].to,
        root.join("2024")
            .join("03-March")
            .join("Images")
            .join("photo.jpg")
    );

    // The same files, the other way round.
    let swapped = build_reorganize_plan(
        root,
        &scan,
        &[GroupBy::Type, GroupBy::Year],
        ProjectPolicy::Keep,
        0,
    );
    assert_eq!(
        swapped.plan.moves[0].to,
        root.join("Images").join("2024").join("photo.jpg")
    );
}

/// The property that makes the command safe to run twice, and the one
/// `organize` gets for free from its skip list.
#[test]
fn a_file_already_in_place_produces_no_move() {
    let root = Path::new("/root");
    let scan = scanned(vec![at(root, "Images/photo.jpg", 1, None)]);
    let plan = build_reorganize_plan(root, &scan, &[GroupBy::Type], ProjectPolicy::Keep, 0);
    assert!(
        plan.plan.moves.is_empty(),
        "expected no moves, got {:?}",
        plan.plan.moves
    );
    assert!(
        plan.emptied.is_empty(),
        "and nothing to clean up: {:?}",
        plan.emptied
    );
}

#[test]
fn reorganizing_the_result_of_a_reorganize_is_a_no_op() {
    let root = Path::new("/root");
    let first = build_reorganize_plan(
        root,
        &scanned(vec![
            at(root, "junk/a.png", 1, Some(MARCH_2024)),
            at(root, "junk/deeper/b.pdf", 2, Some(MARCH_2024)),
        ]),
        &[GroupBy::Year, GroupBy::Type],
        ProjectPolicy::Keep,
        0,
    );
    assert_eq!(first.plan.moves.len(), 2);

    // Feed the destinations back in, as a second run would see them.
    let after: Vec<FileEntry> = first
        .plan
        .moves
        .iter()
        .map(|m| FileEntry {
            path: m.to.clone(),
            size: m.size,
            modified: Some(UNIX_EPOCH + Duration::from_secs(MARCH_2024)),
            depth: 3,
            source: 0,
        })
        .collect();
    let second = build_reorganize_plan(
        root,
        &scanned(after),
        &[GroupBy::Year, GroupBy::Type],
        ProjectPolicy::Keep,
        0,
    );
    assert!(
        second.is_empty(),
        "second run planned {:?}",
        second.plan.moves
    );
}

#[test]
fn emptied_folders_are_listed_deepest_first() {
    let root = Path::new("/root");
    let scan = scanned(vec![at(root, "a/b/c/deep.png", 1, None)]);
    let plan = build_reorganize_plan(root, &scan, &[GroupBy::Type], ProjectPolicy::Keep, 0);
    assert_eq!(
        plan.emptied,
        vec![
            root.join("a").join("b").join("c"),
            root.join("a").join("b"),
            root.join("a"),
        ],
        "a child must be removable before its parent"
    );
}

/// A folder holding something that stays must survive, and so must every folder
/// above it, or the run would try to delete a folder that is not empty.
#[test]
fn a_folder_that_keeps_something_is_never_scheduled_for_removal() {
    let root = Path::new("/root");
    let mut scan = scanned(vec![at(root, "keep/moving.png", 1, None)]);
    scan.skipped.push(Skipped {
        path: root.join("keep").join("staying.iso"),
        reason: SkipReason::Ignored(".iso".into()),
    });
    let plan = build_reorganize_plan(root, &scan, &[GroupBy::Type], ProjectPolicy::Keep, 0);
    assert_eq!(plan.plan.moves.len(), 1);
    assert!(
        plan.emptied.is_empty(),
        "`keep` still holds the ignored file: {:?}",
        plan.emptied
    );
}

#[test]
fn the_tool_state_folder_is_never_removed() {
    let root = Path::new("/root");
    let scan = scanned(vec![at(root, ".tidy-up/journals/old.png", 1, None)]);
    let plan = build_reorganize_plan(root, &scan, &[GroupBy::Type], ProjectPolicy::Keep, 0);
    assert!(
        plan.emptied
            .iter()
            .all(|dir| !dir.starts_with(root.join(TOOL_DIR))),
        "the undo journals must survive: {:?}",
        plan.emptied
    );
}

#[test]
fn a_kept_project_stays_put_and_keeps_its_folder() {
    let root = Path::new("/root");
    let mut scan = scanned(vec![]);
    scan.projects.push(root.join("code").join("app"));
    let plan = build_reorganize_plan(root, &scan, &[GroupBy::Type], ProjectPolicy::Keep, 0);
    assert!(plan.plan.moves.is_empty());
    assert!(plan.emptied.is_empty(), "{:?}", plan.emptied);
    assert!(
        plan.plan
            .skipped
            .iter()
            .any(|s| s.reason == SkipReason::Project)
    );
}

#[test]
fn a_moved_project_travels_whole_into_the_projects_folder() {
    let root = Path::new("/root");
    let mut scan = scanned(vec![]);
    scan.projects.push(root.join("code").join("app"));
    let plan = build_reorganize_plan(root, &scan, &[GroupBy::Type], ProjectPolicy::Move, 0);
    assert_eq!(plan.plan.moves.len(), 1);
    assert_eq!(plan.plan.moves[0].kind, MoveKind::Dir);
    assert_eq!(plan.plan.moves[0].to, root.join("Projects").join("app"));
    assert_eq!(plan.emptied, vec![root.join("code")]);
}

/// Two files with the same name from different folders collapse into one bucket,
/// and neither may overwrite the other.
#[test]
fn identical_names_from_different_folders_are_kept_apart() {
    let root = Path::new("/root");
    let scan = scanned(vec![
        at(root, "one/report.pdf", 1, None),
        at(root, "two/report.pdf", 2, None),
    ]);
    let plan = build_reorganize_plan(root, &scan, &[GroupBy::Type], ProjectPolicy::Keep, 0);
    assert_eq!(plan.plan.moves.len(), 2);
    assert_ne!(
        plan.plan.moves[0].to, plan.plan.moves[1].to,
        "one would have silently replaced the other"
    );
    assert_eq!(plan.plan.moves[0].to, root.join("Documents/report.pdf"));
    assert_eq!(plan.plan.moves[1].to, root.join("Documents/report (1).pdf"));
}

#[test]
fn no_keys_at_all_flattens_everything_into_the_root() {
    let root = Path::new("/root");
    let scan = scanned(vec![at(root, "deep/nested/a.png", 1, None)]);
    let plan = build_reorganize_plan(root, &scan, &[], ProjectPolicy::Keep, 0);
    assert_eq!(plan.plan.moves[0].to, root.join("a.png"));
}

#[test]
fn the_grouping_is_described_in_words_for_the_prompt() {
    assert_eq!(describe(&[GroupBy::Type]), "type");
    assert_eq!(describe(&[GroupBy::Year, GroupBy::Type]), "year, then type");
    assert_eq!(
        describe(&[GroupBy::Year, GroupBy::Month, GroupBy::Type]),
        "year, then month, then type"
    );
    assert_eq!(describe(&[]), "nothing");
}
