//! End-to-end tests of the library API: scan → plan → execute → journal → restore.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use tidy_up::{
    compare::{build_merge_plan, compare, scan_folders, validate_folders},
    dedupe::{NoProgress, build_dedupe_plan, delete_duplicates, find_duplicates},
    executor::execute,
    fsops::resolve_root,
    journal::{Journal, Operation},
    plan::{ProjectPolicy, build_organize_plan, organize_skip_dirs},
    restore::{ConflictPolicy, RestoreOptions, restore},
    rules::IgnoreRules,
    scan::{ScanOptions, scan},
};

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Snapshot of every file under `root` (excluding `.tidy-up`) as relative path → content.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == ".tidy-up") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                out.insert(rel, fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Every directory under `root` (excluding `.tidy-up`), to check restore removes what we created.
fn dirs(root: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() && path.file_name().is_none_or(|n| n != ".tidy-up") {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
                walk(root, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn messy_folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(r, "holiday.jpg", "jpeg-bytes");
    write(r, "thesis.pdf", "pdf-bytes");
    write(r, "notes.txt", "some notes");
    write(r, "part.stl", "solid");
    write(r, "setup.exe", "MZ");
    write(r, "mystery", "no extension");
    write(r, "Chrome.lnk", "shortcut");
    write(r, ".hidden", "secret");
    write(r, "sub/nested.png", "png-bytes");
    write(r, "sub/other/deep.md", "# deep");
    write(r, "myrepo/Cargo.toml", "[package]");
    write(r, "myrepo/src/main.rs", "fn main(){}");
    dir
}

fn organize_options(depth: usize) -> ScanOptions {
    ScanOptions {
        max_depth: depth,
        rules: IgnoreRules::new(),
        skip_root_dirs: organize_skip_dirs(),
    }
}

#[test]
fn organize_then_restore_returns_to_exact_original_state() {
    let dir = messy_folder();
    let root = resolve_root(dir.path()).unwrap();
    let before = snapshot(&root);
    let dirs_before = dirs(&root);

    let scanned = scan(&root, &organize_options(usize::MAX)).unwrap();
    let plan = build_organize_plan(&root, &scanned, ProjectPolicy::Move);
    let report = execute(&plan, Operation::Organize, |_| {}).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);

    let after = snapshot(&root);
    assert!(after.contains_key(Path::new("Images/holiday.jpg")));
    assert!(after.contains_key(Path::new("Images/nested.png")));
    assert!(after.contains_key(Path::new("3D Models/part.stl")));
    assert!(after.contains_key(Path::new("Text Files/deep.md")));
    assert!(after.contains_key(Path::new("Other/mystery")));
    assert!(after.contains_key(Path::new("Projects/myrepo/Cargo.toml")));
    // shortcuts and hidden files are untouched
    assert!(after.contains_key(Path::new("Chrome.lnk")));
    assert!(after.contains_key(Path::new(".hidden")));

    let mut journal = Journal::find(&root, &report.journal_id).unwrap();
    let restored = restore(&root, &mut journal, RestoreOptions::default(), |_, _| {}).unwrap();
    assert!(restored.is_clean(), "{restored:?}");

    assert_eq!(snapshot(&root), before, "file set and contents must match");
    assert_eq!(dirs(&root), dirs_before, "created folders must be cleaned up");
}

#[test]
fn organizing_twice_is_idempotent() {
    let dir = messy_folder();
    let root = resolve_root(dir.path()).unwrap();
    let plan = build_organize_plan(
        &root,
        &scan(&root, &organize_options(1)).unwrap(),
        ProjectPolicy::Keep,
    );
    execute(&plan, Operation::Organize, |_| {}).unwrap();

    let second = build_organize_plan(
        &root,
        &scan(&root, &organize_options(usize::MAX)).unwrap(),
        ProjectPolicy::Keep,
    );
    // Only the not-yet-touched sub/ contents remain; nothing already sorted moves again.
    assert!(second.moves.iter().all(|m| m.from.starts_with(root.join("sub"))));
}

#[test]
fn projects_stay_put_by_default() {
    let dir = messy_folder();
    let root = resolve_root(dir.path()).unwrap();
    let plan = build_organize_plan(
        &root,
        &scan(&root, &organize_options(usize::MAX)).unwrap(),
        ProjectPolicy::Keep,
    );
    execute(&plan, Operation::Organize, |_| {}).unwrap();
    assert!(root.join("myrepo/Cargo.toml").exists());
    assert!(root.join("myrepo/src/main.rs").exists());
}

#[test]
fn ignore_rules_protect_matching_files() {
    let dir = messy_folder();
    let root = resolve_root(dir.path()).unwrap();
    let mut options = organize_options(1);
    options.rules.add_extension("pdf");
    options.rules.add_entry("no*");
    let plan = build_organize_plan(&root, &scan(&root, &options).unwrap(), ProjectPolicy::Keep);
    execute(&plan, Operation::Organize, |_| {}).unwrap();
    assert!(root.join("thesis.pdf").exists());
    assert!(root.join("notes.txt").exists());
    assert!(!root.join("holiday.jpg").exists());
}

#[test]
fn dedupe_isolates_extra_copies_and_restore_brings_them_back() {
    let dir = tempfile::tempdir().unwrap();
    let root = resolve_root(dir.path()).unwrap();
    write(&root, "report.pdf", "identical");
    write(&root, "Downloads/report (1).pdf", "identical");
    write(&root, "Downloads/report (2).pdf", "identical");
    write(&root, "unique.txt", "only one");
    let before = snapshot(&root);

    let options = ScanOptions {
        max_depth: usize::MAX,
        rules: IgnoreRules::new(),
        skip_root_dirs: ["_duplicates".to_string()].into(),
    };
    let found = find_duplicates(&scan(&root, &options).unwrap().files, &NoProgress);
    assert_eq!(found.groups.len(), 1);
    assert_eq!(found.groups[0].keeper, root.join("report.pdf"));

    let plan = build_dedupe_plan(&root, &found.groups);
    let report = execute(&plan, Operation::Dedupe, |_| {}).unwrap();
    assert_eq!(report.moved, 2);
    let after = snapshot(&root);
    assert!(after.contains_key(Path::new("report.pdf")));
    assert!(after.contains_key(Path::new("unique.txt")));
    assert_eq!(after.keys().filter(|p| p.starts_with("_Duplicates")).count(), 2);

    // A second scan ignores the quarantine folder, so nothing is re-flagged.
    let again = find_duplicates(&scan(&root, &options).unwrap().files, &NoProgress);
    assert!(again.groups.is_empty());

    let mut journal = Journal::find(&root, &report.journal_id).unwrap();
    restore(&root, &mut journal, RestoreOptions::default(), |_, _| {}).unwrap();
    assert_eq!(snapshot(&root), before);
}

#[test]
fn stacked_runs_undo_in_reverse_order() {
    let dir = messy_folder();
    let root = resolve_root(dir.path()).unwrap();
    write(&root, "dup-a.txt", "same same");
    write(&root, "dup-b.txt", "same same");
    let before = snapshot(&root);

    // Run 1: dedupe.
    let options = ScanOptions {
        max_depth: 1,
        rules: IgnoreRules::new(),
        skip_root_dirs: ["_duplicates".to_string()].into(),
    };
    let dups = find_duplicates(&scan(&root, &options).unwrap().files, &NoProgress);
    let r1 = execute(&build_dedupe_plan(&root, &dups.groups), Operation::Dedupe, |_| {}).unwrap();

    // Run 2: organize.
    let plan = build_organize_plan(&root, &scan(&root, &organize_options(1)).unwrap(), ProjectPolicy::Keep);
    let r2 = execute(&plan, Operation::Organize, |_| {}).unwrap();
    assert_ne!(r1.journal_id, r2.journal_id);

    for id in [&r2.journal_id, &r1.journal_id] {
        let mut journal = Journal::find(&root, id).unwrap();
        let report = restore(&root, &mut journal, RestoreOptions::default(), |_, _| {}).unwrap();
        assert!(report.is_clean(), "{report:?}");
    }
    assert_eq!(snapshot(&root), before);
    assert!(Journal::load_all(&root).unwrap().iter().all(Journal::is_restored));
}

#[test]
fn restore_handles_user_changes_made_after_organizing() {
    let dir = tempfile::tempdir().unwrap();
    let root = resolve_root(dir.path()).unwrap();
    write(&root, "a.png", "original");
    let plan = build_organize_plan(&root, &scan(&root, &organize_options(1)).unwrap(), ProjectPolicy::Keep);
    let report = execute(&plan, Operation::Organize, |_| {}).unwrap();

    write(&root, "a.png", "someone made a new a.png meanwhile");
    let mut journal = Journal::find(&root, &report.journal_id).unwrap();
    let options = RestoreOptions { conflict: ConflictPolicy::Rename, dry_run: false };
    let outcome = restore(&root, &mut journal, options, |_, _| {}).unwrap();

    assert_eq!(outcome.renamed.len(), 1);
    assert_eq!(fs::read_to_string(root.join("a.png")).unwrap(), "someone made a new a.png meanwhile");
    assert_eq!(fs::read_to_string(root.join("a (1).png")).unwrap(), "original");
}

#[test]
fn name_collisions_across_folders_survive_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let root = resolve_root(dir.path()).unwrap();
    write(&root, "a/photo.jpg", "A");
    write(&root, "b/photo.jpg", "B");
    write(&root, "photo.jpg", "TOP");
    let before = snapshot(&root);

    let plan = build_organize_plan(&root, &scan(&root, &organize_options(usize::MAX)).unwrap(), ProjectPolicy::Keep);
    let report = execute(&plan, Operation::Organize, |_| {}).unwrap();
    assert_eq!(snapshot(&root).keys().filter(|p| p.starts_with("Images")).count(), 3);

    let mut journal = Journal::find(&root, &report.journal_id).unwrap();
    restore(&root, &mut journal, RestoreOptions::default(), |_, _| {}).unwrap();
    assert_eq!(snapshot(&root), before);
}

// ---------------------------------------------------------------- compare across folders

/// Three folders on "different drives" (separate temp dirs) with overlapping content.
struct Trio {
    _guards: Vec<tempfile::TempDir>,
    roots: Vec<PathBuf>,
}

fn trio() -> Trio {
    let guards: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let roots: Vec<PathBuf> = guards.iter().map(|d| resolve_root(d.path()).unwrap()).collect();
    // A (primary)
    write(&roots[0], "shared.txt", "shared content");
    write(&roots[0], "only-a.txt", "only in a");
    write(&roots[0], "everywhere.bin", "in all three");
    // B
    write(&roots[1], "shared-copy.txt", "shared content");
    write(&roots[1], "docs/only-b.txt", "only in b");
    write(&roots[1], "everywhere.bin", "in all three");
    // C
    write(&roots[2], "another-copy.txt", "shared content");
    write(&roots[2], "docs/only-b.txt", "different file, same name");
    write(&roots[2], "everywhere.bin", "in all three");
    Trio { _guards: guards, roots }
}

fn scan_trio(t: &Trio) -> (Vec<tidy_up::scan::FileEntry>, Vec<tidy_up::dedupe::DuplicateGroup>) {
    let files = scan_folders(&t.roots, &IgnoreRules::new(), usize::MAX, |_, _| {}).unwrap();
    let groups = find_duplicates(&files, &NoProgress).groups;
    (files, groups)
}

fn snapshots(t: &Trio) -> Vec<BTreeMap<PathBuf, String>> {
    t.roots.iter().map(|r| snapshot(r)).collect()
}

#[test]
fn compare_reports_how_folders_relate() {
    let t = trio();
    let (files, groups) = scan_trio(&t);
    let c = compare(&t.roots, &files, &groups);
    assert!(!c.all_identical);
    assert_eq!(groups.len(), 2, "shared.txt x3 and everywhere.bin x3");
    assert_eq!(c.folders[0].files, 3);
    assert_eq!(c.folders[1].shared, 2);
    assert_eq!(c.folders[1].unique, 1);
}

#[test]
fn compare_move_across_folders_then_restore_is_exact() {
    let t = trio();
    let before = snapshots(&t);
    let (_, groups) = scan_trio(&t);

    let plan = build_dedupe_plan(&t.roots[0], &groups);
    assert_eq!(plan.moves.len(), 4, "two extra copies in each of two groups");
    let report = execute(&plan, Operation::Compare, |_| {}).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);

    // primary keeps its copies; extras live in the primary's _Duplicates
    assert!(t.roots[0].join("shared.txt").exists());
    assert!(!t.roots[1].join("shared-copy.txt").exists());
    assert!(!t.roots[2].join("another-copy.txt").exists());
    assert_eq!(
        snapshot(&t.roots[0]).keys().filter(|p| p.starts_with("_Duplicates")).count(),
        4
    );
    // non-duplicates untouched
    assert!(t.roots[1].join("docs/only-b.txt").exists());

    // the journal lives in the primary; restoring it puts files back in the OTHER folders
    let mut journal = Journal::find(&t.roots[0], &report.journal_id).unwrap();
    let restored = restore(&t.roots[0], &mut journal, RestoreOptions::default(), |_, _| {}).unwrap();
    assert!(restored.is_clean(), "{restored:?}");
    assert_eq!(snapshots(&t), before);
}

#[test]
fn compare_merge_gathers_everything_into_the_primary_and_restore_undoes_it() {
    let t = trio();
    let before = snapshots(&t);
    let (files, groups) = scan_trio(&t);

    let plan = build_merge_plan(&t.roots, &files, &groups);
    let report = execute(&plan, Operation::Compare, |_| {}).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);

    let a = snapshot(&t.roots[0]);
    assert_eq!(a[Path::new("only-a.txt")], "only in a");
    assert_eq!(a[Path::new("docs/only-b.txt")], "only in b", "B's unique file gathered");
    assert_eq!(
        a[Path::new("docs/only-b (1).txt")],
        "different file, same name",
        "C's clashing file is renamed, never overwritten"
    );
    // B and C hold no real files now: everything moved into A (or A's _Duplicates)
    assert!(snapshot(&t.roots[1]).is_empty(), "{:?}", snapshot(&t.roots[1]));
    assert!(snapshot(&t.roots[2]).is_empty(), "{:?}", snapshot(&t.roots[2]));
    // no content was lost: every original content still exists somewhere under A
    let contents: Vec<&String> = a.values().collect();
    for original in before.iter().flat_map(|s| s.values()) {
        assert!(contents.contains(&original), "lost content: {original}");
    }

    let mut journal = Journal::find(&t.roots[0], &report.journal_id).unwrap();
    let restored = restore(&t.roots[0], &mut journal, RestoreOptions::default(), |_, _| {}).unwrap();
    assert!(restored.is_clean(), "{restored:?}");
    assert_eq!(snapshots(&t), before);
}

#[test]
fn merge_of_disjoint_folders_needs_no_duplicates() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    write(a.path(), "one.txt", "1");
    write(b.path(), "two.txt", "2");
    let roots = validate_folders(&[a.path().to_path_buf(), b.path().to_path_buf()]).unwrap();
    let files = scan_folders(&roots, &IgnoreRules::new(), usize::MAX, |_, _| {}).unwrap();
    let groups = find_duplicates(&files, &NoProgress).groups;
    assert!(groups.is_empty());
    let plan = build_merge_plan(&roots, &files, &groups);
    execute(&plan, Operation::Compare, |_| {}).unwrap();
    assert!(roots[0].join("one.txt").exists() && roots[0].join("two.txt").exists());
}

#[test]
fn delete_keeps_exactly_one_copy_and_verifies_before_deleting() {
    let t = trio();
    let (_, groups) = scan_trio(&t);
    let report = delete_duplicates(&groups, |_, _| {});
    assert_eq!(report.deleted, 4);
    assert!(report.skipped.is_empty());
    assert!(t.roots[0].join("shared.txt").exists(), "primary copy survives");
    assert!(!t.roots[1].join("shared-copy.txt").exists());
    assert!(t.roots[1].join("docs/only-b.txt").exists());
}

#[test]
fn delete_skips_copies_that_changed_after_the_scan() {
    let t = trio();
    let (_, groups) = scan_trio(&t);
    // someone edits one extra copy, and the kept copy of the other group, after the scan
    fs::write(t.roots[1].join("shared-copy.txt"), "edited since scan!!").unwrap();
    fs::write(t.roots[0].join("everywhere.bin"), "kept copy was damaged").unwrap();

    let report = delete_duplicates(&groups, |_, _| {});
    assert!(t.roots[1].join("shared-copy.txt").exists(), "changed copy must survive");
    assert!(t.roots[1].join("everywhere.bin").exists(), "unverifiable keeper keeps extras");
    assert!(t.roots[2].join("everywhere.bin").exists());
    assert!(!t.roots[2].join("another-copy.txt").exists(), "verified copy was deleted");
    assert_eq!(report.deleted, 1);
    assert_eq!(report.skipped.len(), 3);
}
