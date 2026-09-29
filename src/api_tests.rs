//! Tests for the machine-facing contract in [`crate::api`].
//!
//! These are deliberately picky about field names and string values. Outside
//! code parses this; rewording a label is a breaking change, and a test that
//! only checked "it is valid JSON" would not notice.

use super::*;
use crate::plan::MoveKind;

fn root() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\work")
    } else {
        PathBuf::from("/work")
    }
}

fn mv(from: &str, to: &str, size: u64) -> PlannedMove {
    PlannedMove {
        from: root().join(from),
        to: root().join(to),
        kind: MoveKind::File,
        size,
    }
}

fn sample_plan() -> Plan {
    Plan {
        root: root(),
        moves: vec![
            mv("a.png", "Images/a.png", 10),
            mv("b.pdf", "Documents/b.pdf", 20),
        ],
        skipped: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Exit codes
// ---------------------------------------------------------------------------

/// The distinction the exit codes exist for: a refusal is an answer, a crash
/// is not, and a caller has to be able to tell them apart.
#[test]
fn a_refusal_and_a_failure_exit_differently() {
    assert_eq!(ErrorCode::SystemFolder.exit(), Exit::Refused);
    assert_eq!(ErrorCode::NotWritable.exit(), Exit::Refused);
    assert_eq!(ErrorCode::Io.exit(), Exit::Error);
    assert_eq!(ErrorCode::Internal.exit(), Exit::Error);
    assert_ne!(Exit::Refused as u8, Exit::Error as u8);
}

/// Declining a confirmation is the user getting exactly what they asked for.
#[test]
fn cancelling_is_not_a_failure() {
    assert_eq!(ErrorCode::Cancelled.exit(), Exit::Ok);
    assert_eq!(Exit::Ok as u8, 0);
}

#[test]
fn bad_arguments_use_the_code_clap_already_uses() {
    assert_eq!(Exit::Usage as u8, 2);
    assert_eq!(ErrorCode::Invalid.exit(), Exit::Usage);
    assert_eq!(ErrorCode::NotAFolder.exit(), Exit::Usage);
}

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

/// A typed refusal must survive the trip through `anyhow` with its code, or
/// the exit boundary is back to matching on prose.
#[test]
fn a_typed_refusal_keeps_its_code_through_anyhow() {
    let err = anyhow::Error::new(Refused::at(
        ErrorCode::SystemFolder,
        Path::new("/usr"),
        "refusing to change /usr",
    ));
    let info = classify(&err);
    assert_eq!(info.code, ErrorCode::SystemFolder);
    assert_eq!(info.path.as_deref(), Some("/usr"));
    assert!(info.message.contains("refusing"));
    assert_eq!(info.code.exit(), Exit::Refused);
}

#[test]
fn library_errors_are_classified_by_kind_not_by_message() {
    let denied = anyhow::Error::new(Error::Denied {
        path: PathBuf::from("/srv/data"),
    });
    assert_eq!(classify(&denied).code, ErrorCode::Denied);

    let missing = anyhow::Error::new(Error::Io {
        path: PathBuf::from("/nope"),
        source: std::io::ErrorKind::NotFound.into(),
    });
    assert_eq!(classify(&missing).code, ErrorCode::NotFound);

    let invalid = anyhow::Error::new(Error::Invalid("not a folder".into()));
    assert_eq!(classify(&invalid).code, ErrorCode::Invalid);
}

#[test]
fn an_unrecognised_error_is_internal_rather_than_silently_fine() {
    let err = anyhow::anyhow!("something nobody typed a code for");
    let info = classify(&err);
    assert_eq!(info.code, ErrorCode::Internal);
    assert_eq!(info.code.exit(), Exit::Error);
}

#[test]
fn every_error_code_serializes_to_a_distinct_snake_case_token() {
    let all = [
        ErrorCode::SystemFolder,
        ErrorCode::NotWritable,
        ErrorCode::Cancelled,
        ErrorCode::NotFound,
        ErrorCode::NotAFolder,
        ErrorCode::Denied,
        ErrorCode::Overlap,
        ErrorCode::InvalidPlan,
        ErrorCode::InvalidJournal,
        ErrorCode::Invalid,
        ErrorCode::Io,
        ErrorCode::Internal,
    ];
    let mut tokens: Vec<String> = all
        .iter()
        .map(|c| {
            serde_json::to_value(c)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert!(
        tokens
            .iter()
            .all(|t| t == &t.to_lowercase() && !t.contains(' '))
    );
    let total = tokens.len();
    tokens.sort();
    tokens.dedup();
    assert_eq!(tokens.len(), total, "codes must not collide");
    assert!(tokens.contains(&"system_folder".to_string()));
}

// ---------------------------------------------------------------------------
// The envelope
// ---------------------------------------------------------------------------

/// A caller parses the output without knowing which command ran, so the outer
/// shape has to be the same every time.
#[test]
fn the_envelope_has_the_same_shape_whatever_happened() {
    let good = Envelope::ok("organize", Outcome::at(&root(), true));
    let bad = Envelope::from_error("organize", &anyhow::anyhow!("boom"));
    for envelope in [&good, &bad] {
        let v = serde_json::to_value(envelope).unwrap();
        for key in ["tidy_up", "format", "command", "status"] {
            assert!(v.get(key).is_some(), "{key} missing from {v}");
        }
        assert_eq!(v["command"], "organize");
        assert_eq!(v["format"], FORMAT_VERSION);
    }
    assert_eq!(serde_json::to_value(&good).unwrap()["status"], "ok");
    assert_eq!(serde_json::to_value(&bad).unwrap()["status"], "error");
}

#[test]
fn a_successful_envelope_carries_no_error_and_the_reverse() {
    let good = serde_json::to_value(Envelope::ok("dedupe", Outcome::default())).unwrap();
    assert!(good.get("error").is_none());
    assert!(good.get("outcome").is_some());

    let bad =
        serde_json::to_value(Envelope::failed("dedupe", ErrorCode::Io, "disk fell over")).unwrap();
    assert!(bad.get("outcome").is_none());
    assert_eq!(bad["error"]["code"], "io");
    assert_eq!(bad["error"]["message"], "disk fell over");
}

/// Empty collections are omitted rather than serialized as `[]` and `{}`, so a
/// quiet run produces a small object.
#[test]
fn an_outcome_only_reports_what_happened() {
    let v = serde_json::to_value(Outcome::at(&root(), false)).unwrap();
    assert!(v.get("plan").is_none());
    assert!(v.get("execution").is_none());
    assert!(v.get("skipped").is_none());
    assert!(v.get("problems").is_none());
    assert_eq!(v["dry_run"], false);
    assert!(v["root"].as_str().unwrap().contains("work"));
}

// ---------------------------------------------------------------------------
// Plan files
// ---------------------------------------------------------------------------

#[test]
fn a_plan_survives_a_round_trip_through_json() {
    let plan = sample_plan();
    let file = PlanFile::of(&plan, Operation::Organize);
    assert_eq!(file.moves, 2);
    assert_eq!(file.bytes, 30);

    let text = serde_json::to_string(&file).unwrap();
    let back = PlanFile::parse(&text).unwrap().into_plan().unwrap();
    assert_eq!(back.root, plan.root);
    assert_eq!(back.moves, plan.moves);
}

/// A dry run with `--json` prints an envelope, and that whole envelope is the
/// most natural thing for someone to save and hand back to `apply`.
#[test]
fn a_plan_can_be_read_back_out_of_a_whole_envelope() {
    let mut outcome = Outcome::at(&root(), true);
    outcome.plan = Some(PlanFile::of(&sample_plan(), Operation::Reorganize));
    let text = serde_json::to_string(&Envelope::ok("reorganize", outcome)).unwrap();

    let file = PlanFile::parse(&text).unwrap();
    assert_eq!(file.operation, Operation::Reorganize);
    assert_eq!(file.into_plan().unwrap().moves.len(), 2);
}

// ---------------------------------------------------------------------------
// The trust boundary
// ---------------------------------------------------------------------------

/// A plan file is JSON on disk. It may have been hand-edited, or produced by an
/// agent acting on something it read. Applying one unchecked would make `apply`
/// a way to move any file anywhere, so every path must be inside the plan root.
#[test]
fn a_plan_that_reaches_outside_its_own_root_is_refused() {
    let escape = if cfg!(windows) {
        PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
    } else {
        PathBuf::from("/etc/passwd")
    };

    let mut file = PlanFile::of(&sample_plan(), Operation::Organize);
    file.items[0].to = escape.clone();
    let err = file.into_plan().unwrap_err().to_string();
    assert!(err.contains("outside its own root"), "{err}");

    let mut file = PlanFile::of(&sample_plan(), Operation::Organize);
    file.items[0].from = escape;
    let err = file.into_plan().unwrap_err().to_string();
    assert!(err.contains("outside its own root"), "{err}");
}

#[test]
fn a_plan_with_relative_paths_is_refused() {
    let mut file = PlanFile::of(&sample_plan(), Operation::Organize);
    file.items[0].to = PathBuf::from("Images/a.png");
    let err = file.into_plan().unwrap_err().to_string();
    assert!(err.contains("relative path"), "{err}");

    let mut file = PlanFile::of(&sample_plan(), Operation::Organize);
    file.root = PathBuf::from("work");
    let err = file.into_plan().unwrap_err().to_string();
    assert!(err.contains("absolute"), "{err}");
}

/// The count is what a reader checks before parsing thousands of moves, so a
/// mismatch means the file is not what it claims to be.
#[test]
fn a_plan_whose_count_disagrees_with_its_contents_is_refused() {
    let mut file = PlanFile::of(&sample_plan(), Operation::Organize);
    file.moves = 99;
    let err = file.into_plan().unwrap_err().to_string();
    assert!(err.contains("99"), "{err}");
}

/// Same courtesy the journal already extends: say so plainly rather than
/// failing to parse a field that did not exist yet.
#[test]
fn a_plan_from_a_newer_tidy_up_says_so() {
    let mut file = PlanFile::of(&sample_plan(), Operation::Organize);
    file.format = FORMAT_VERSION + 1;
    let err = file.into_plan().unwrap_err().to_string();
    assert!(err.contains("newer tidy-up"), "{err}");
}

#[test]
fn nonsense_in_a_plan_file_is_an_error_not_a_panic() {
    assert!(PlanFile::parse("not json at all").is_err());
    assert!(PlanFile::parse("{}").is_err());
    assert!(PlanFile::parse(r#"{"format":1}"#).is_err());
}
