//! Tests for the trust boundaries: places where data tidy-up was pointed at
//! could otherwise direct what tidy-up does.
//!
//! Each of these is an attack written out. They exist because the same shape
//! keeps recurring: a file inside the folder being processed claims authority
//! over what happens to it, or to somewhere else.

mod common;

use std::{fs, path::Path};

use common::{all_output, sandbox, tidy};

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Plants a journal in `victim` claiming one of its files belongs at `escape`.
fn plant_journal(victim: &Path, escape: &Path) {
    let journals = victim.join(".tidy-up").join("journals");
    fs::create_dir_all(&journals).unwrap();
    let header = serde_json::json!({
        "type": "header", "version": 1, "id": "20200101-000000",
        "operation": "organize", "root": victim, "created_at": 1_577_836_800u64
    });
    let mv = serde_json::json!({
        "type": "move", "from": escape, "to": "payload.txt", "kind": "file"
    });
    fs::write(
        journals.join("20200101-000000.jsonl"),
        format!("{header}\n{mv}\n"),
    )
    .unwrap();
}

/// A journal is a file inside the folder, so a folder obtained from somewhere
/// else can carry one claiming a file belongs anywhere on the disk. Restoring
/// it must not write outside the folder being restored.
#[test]
fn a_planted_journal_cannot_make_restore_write_outside_the_folder() {
    let dir = sandbox();
    let victim = dir.path().join("victim");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&victim).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(victim.join("payload.txt"), "payload").unwrap();

    let escape = outside.join("ESCAPED.txt");
    plant_journal(&victim, &escape);

    let out = tidy(&["restore", s(&victim), "--yes"]);
    let text = all_output(&out);

    assert!(
        !escape.exists(),
        "restore wrote outside the folder it was given:\n{text}"
    );
    assert!(
        victim.join("payload.txt").exists(),
        "and the file should still be where it was:\n{text}"
    );
    assert!(
        text.contains("outside") || text.contains("Careful"),
        "the refusal should be visible, not silent:\n{text}"
    );
}

/// `--yes` is agreement to restore a folder. It is not agreement to write
/// outside it, so it must not be enough on its own.
#[test]
fn yes_alone_does_not_permit_writing_outside_the_folder() {
    let dir = sandbox();
    let victim = dir.path().join("victim");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&victim).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(victim.join("payload.txt"), "payload").unwrap();

    let escape = outside.join("ESCAPED.txt");
    plant_journal(&victim, &escape);

    assert!(!escape.exists());
    tidy(&["restore", s(&victim), "--yes"]);
    assert!(!escape.exists(), "--yes must not be the deciding factor");
}
