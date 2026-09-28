//! End-to-end tests for `reorganize`, including the round trip that matters:
//! a badly organized tree, re-filed, then restored byte for byte.

mod common;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use common::{all_output, sandbox, tidy};

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Every file under `root`, as relative path to contents, excluding tidy-up state.
fn files(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == ".tidy-up") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy();
                out.insert(rel.replace('\\', "/"), fs::read_to_string(&path).unwrap());
            }
        }
    }
    out
}

/// Every folder under `root`, relative, excluding tidy-up state.
fn dirs(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_dir() || path.file_name().is_some_and(|n| n == ".tidy-up") {
                continue;
            }
            out.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
            stack.push(path);
        }
    }
    out.sort();
    out
}

/// A folder organized the way people actually leave them: category folders from
/// some earlier tool, nested junk, and an empty folder for good measure.
fn messy_tree(root: &Path) -> BTreeMap<String, String> {
    write(root, "Images/old/holiday.jpg", "jpeg bytes");
    write(root, "Images/screenshot.png", "png bytes");
    write(root, "My Documents/2019/taxes.pdf", "pdf bytes");
    write(root, "My Documents/notes.txt", "some notes");
    write(root, "random/deeply/nested/song.mp3", "mp3 bytes");
    write(root, "loose.csv", "a,b,c");
    fs::create_dir_all(root.join("already empty")).unwrap();
    files(root)
}

#[test]
fn a_messy_tree_is_unpacked_and_refiled_by_type() {
    let dir = sandbox();
    let root = dir.path();
    let before = messy_tree(root);

    let out = tidy(&["reorganize", s(root), "--yes"]);
    let text = all_output(&out);
    assert!(out.status.success(), "{text}");

    let after = files(root);
    assert_eq!(
        after.keys().count(),
        before.keys().count(),
        "no file may be lost:\n{text}"
    );
    assert_eq!(
        after.get("Images/holiday.jpg").map(String::as_str),
        Some("jpeg bytes")
    );
    assert_eq!(
        after.get("Documents/taxes.pdf").map(String::as_str),
        Some("pdf bytes")
    );
    assert_eq!(
        after.get("Audio/song.mp3").map(String::as_str),
        Some("mp3 bytes")
    );
    assert_eq!(
        after.get("Spreadsheets/loose.csv").map(String::as_str),
        Some("a,b,c")
    );

    // The folders it emptied are gone.
    let remaining = dirs(root);
    for gone in ["random", "random/deeply", "My Documents", "Images/old"] {
        assert!(
            !remaining.contains(&gone.to_string()),
            "{gone} should have been removed, still have {remaining:?}"
        );
    }
}

/// The test that matters most: after undo, the tree is exactly what it was,
/// including the folders the run deleted.
#[test]
fn a_reorganize_can_be_undone_exactly() {
    let dir = sandbox();
    let root = dir.path();
    let before_files = messy_tree(root);
    let before_dirs = dirs(root);

    assert!(
        tidy(&["reorganize", s(root), "--by", "year,type", "--yes"])
            .status
            .success()
    );
    assert_ne!(files(root), before_files, "something must have changed");

    let out = tidy(&["restore", s(root), "--all", "--yes"]);
    let text = all_output(&out);
    assert!(out.status.success(), "{text}");

    assert_eq!(files(root), before_files, "every file back, byte for byte");
    assert_eq!(
        dirs(root),
        before_dirs,
        "and every folder, including the ones that were deleted"
    );
}

/// `organize` gets this from its skip list. `reorganize` has no skip list, so
/// it has to compute it, and getting it wrong would mean endless churn.
#[test]
fn running_it_twice_changes_nothing_the_second_time() {
    let dir = sandbox();
    let root = dir.path();
    messy_tree(root);

    assert!(tidy(&["reorganize", s(root), "--yes"]).status.success());
    let after_first = files(root);
    let dirs_first = dirs(root);

    let out = tidy(&["reorganize", s(root), "--yes"]);
    let text = all_output(&out);
    assert!(out.status.success(), "{text}");
    assert!(
        text.contains("already grouped by type"),
        "the second run should report there is nothing to do:\n{text}"
    );
    assert_eq!(files(root), after_first);
    assert_eq!(dirs(root), dirs_first);
}

#[test]
fn keys_nest_in_the_order_given() {
    let dir = sandbox();
    let root = dir.path();
    write(root, "junk/a.png", "x");

    assert!(
        tidy(&["reorganize", s(root), "--by", "type,ext", "--yes"])
            .status
            .success()
    );
    assert!(
        root.join("Images").join("PNG").join("a.png").exists(),
        "got {:?}",
        dirs(root)
    );
}

#[test]
fn a_dry_run_changes_nothing_and_leaves_no_state() {
    let dir = sandbox();
    let root = dir.path();
    let before_files = messy_tree(root);
    let before_dirs = dirs(root);

    let out = tidy(&["reorganize", s(root), "--dry-run"]);
    let text = all_output(&out);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("Dry run"), "{text}");
    assert_eq!(files(root), before_files);
    assert_eq!(dirs(root), before_dirs);
    assert!(!root.join(".tidy-up").exists(), "no journal for a dry run");
}

#[test]
fn empty_folders_can_be_kept() {
    let dir = sandbox();
    let root = dir.path();
    write(root, "junk/a.png", "x");

    assert!(
        tidy(&["reorganize", s(root), "--keep-empty-dirs", "--yes"])
            .status
            .success()
    );
    assert!(root.join("Images").join("a.png").exists());
    assert!(
        root.join("junk").exists(),
        "the emptied folder was asked to stay"
    );
}

#[test]
fn an_unknown_grouping_key_is_rejected() {
    let dir = sandbox();
    let out = tidy(&["reorganize", s(dir.path()), "--by", "colour", "--yes"]);
    assert!(!out.status.success());
    assert!(
        all_output(&out).contains("colour"),
        "the error should name the bad key:\n{}",
        all_output(&out)
    );
}

/// The undo journal lives in the folder being reorganized, so the run must not
/// sweep it up along with everything else.
#[test]
fn the_undo_journal_survives_its_own_run() {
    let dir = sandbox();
    let root = dir.path();
    messy_tree(root);
    assert!(tidy(&["reorganize", s(root), "--yes"]).status.success());
    assert!(root.join(".tidy-up").is_dir());

    let history = tidy(&["history", s(root)]);
    let text = all_output(&history);
    assert!(text.contains("reorganize"), "{text}");
}

/// Files with the same name in different folders collapse into one bucket. One
/// must not silently replace the other.
#[test]
fn colliding_names_are_all_kept() {
    let dir = sandbox();
    let root = dir.path();
    write(root, "a/report.pdf", "first");
    write(root, "b/report.pdf", "second");
    write(root, "c/report.pdf", "third");

    assert!(tidy(&["reorganize", s(root), "--yes"]).status.success());
    let contents: Vec<String> = files(root).into_values().collect();
    assert_eq!(contents.len(), 3, "all three survive");
    for want in ["first", "second", "third"] {
        assert!(
            contents.iter().any(|c| c == want),
            "{want} was lost: {contents:?}"
        );
    }
}

/// A project directory is a unit. Unpacking it would scatter a git repository
/// across a dozen type folders.
#[test]
fn a_code_project_is_left_whole() {
    let dir = sandbox();
    let root = dir.path();
    write(root, "work/app/Cargo.toml", "[package]");
    write(root, "work/app/src/main.rs", "fn main() {}");
    write(root, "work/stray.png", "x");

    let out = tidy(&["reorganize", s(root), "--yes"]);
    assert!(out.status.success(), "{}", all_output(&out));
    assert!(
        root.join("work/app/src/main.rs").exists(),
        "the project stays put: {:?}",
        dirs(root)
    );
    assert!(root.join("Images/stray.png").exists());
    assert!(
        root.join("work").exists(),
        "and its parent survives, because the project is still in it"
    );
}

#[test]
fn grouping_by_size_puts_files_in_the_right_band() {
    let dir = sandbox();
    let root = dir.path();
    fs::write(root.join("small.bin"), vec![0u8; 16]).unwrap();
    fs::write(root.join("bigger.bin"), vec![0u8; 2 << 20]).unwrap();

    assert!(
        tidy(&["reorganize", s(root), "--by", "size", "--yes"])
            .status
            .success()
    );
    let found: Vec<PathBuf> = files(root).keys().map(PathBuf::from).collect();
    assert!(
        found.iter().any(|p| p.starts_with("Tiny (under 1 MiB)")),
        "{found:?}"
    );
    assert!(
        found.iter().any(|p| p.starts_with("Small (under 10 MiB)")),
        "{found:?}"
    );
}
