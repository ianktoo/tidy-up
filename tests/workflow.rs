//! End-to-end tests of the library API: scan → plan → execute → journal → restore.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use tidy_up::{
    dedupe::{build_dedupe_plan, find_duplicates},
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
    let report = execute(&plan, Operation::Organize, |_, _| {}).unwrap();
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
    execute(&plan, Operation::Organize, |_, _| {}).unwrap();

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
    execute(&plan, Operation::Organize, |_, _| {}).unwrap();
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
    execute(&plan, Operation::Organize, |_, _| {}).unwrap();
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
    let found = find_duplicates(&scan(&root, &options).unwrap().files, &|| {});
    assert_eq!(found.groups.len(), 1);
    assert_eq!(found.groups[0].keeper, root.join("report.pdf"));

    let plan = build_dedupe_plan(&root, &found.groups);
    let report = execute(&plan, Operation::Dedupe, |_, _| {}).unwrap();
    assert_eq!(report.moved, 2);
    let after = snapshot(&root);
    assert!(after.contains_key(Path::new("report.pdf")));
    assert!(after.contains_key(Path::new("unique.txt")));
    assert_eq!(after.keys().filter(|p| p.starts_with("_Duplicates")).count(), 2);

    // A second scan ignores the quarantine folder, so nothing is re-flagged.
    let again = find_duplicates(&scan(&root, &options).unwrap().files, &|| {});
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
    let dups = find_duplicates(&scan(&root, &options).unwrap().files, &|| {});
    let r1 = execute(&build_dedupe_plan(&root, &dups.groups), Operation::Dedupe, |_, _| {}).unwrap();

    // Run 2: organize.
    let plan = build_organize_plan(&root, &scan(&root, &organize_options(1)).unwrap(), ProjectPolicy::Keep);
    let r2 = execute(&plan, Operation::Organize, |_, _| {}).unwrap();
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
    let report = execute(&plan, Operation::Organize, |_, _| {}).unwrap();

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
    let report = execute(&plan, Operation::Organize, |_, _| {}).unwrap();
    assert_eq!(snapshot(&root).keys().filter(|p| p.starts_with("Images")).count(), 3);

    let mut journal = Journal::find(&root, &report.journal_id).unwrap();
    restore(&root, &mut journal, RestoreOptions::default(), |_, _| {}).unwrap();
    assert_eq!(snapshot(&root), before);
}
