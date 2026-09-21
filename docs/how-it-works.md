# How it works

## Pipeline

```text
classify ─▶ scan ─▶ plan ─▶ execute ─▶ journal ─▶ restore
```

| Stage | Module | Responsibility |
|---|---|---|
| classify | `category`, `rules`, `projects` | What a file is; what to leave alone |
| scan | `scan` | Walk a folder into eligible files plus skipped entries with reasons |
| plan | `plan`, `dedupe` | Pure data: a list of `PlannedMove`s. No side effects |
| execute | `executor`, `fsops` | Perform moves without ever overwriting |
| journal | `journal` | Append-only undo log |
| restore | `restore` | Replay a journal backwards |
| present | `cli`, `commands`, `ui` | Arguments, flows, terminal output |

Because a `Plan` is plain data, `--dry-run`, previews and confirmation prompts
are free, and the engine can be embedded without any terminal code.

## The journal

Location: `<folder>/.tidy-up/journals/<id>.jsonl`. The id is `YYYYMMDD-HHMMSS`
(UTC), with `-2`, `-3`, … appended if several runs start in the same second.

The format is JSON Lines. Line 1 is a header; every later line is one event,
appended the moment it happens:

```jsonl
{"type":"header","version":1,"id":"20260920-140309","operation":"organize","root":"C:\\Users\\me\\Downloads","created_at":1789913789}
{"type":"dir_created","path":"Images"}
{"type":"move","from":"holiday.jpg","to":"Images/holiday.jpg","kind":"file"}
{"type":"restored","at":1789914001}
```

Design decisions:

- **Relative paths.** Moves are stored relative to the folder, so a drive that
  changes letter (or a folder that is renamed) keeps a valid history.
- **Append-only, flushed per record.** A crash mid-run leaves a complete record
  of everything already moved. A torn *final* line is tolerated when loading;
  corruption anywhere else is an error.
- **Journal failure is fatal.** If a move can't be recorded, the executor rolls
  that move back and stops. An unrecorded change is an un-undoable change.
- **`dir_created` precedes the moves that need it**, so restore, which runs in
  reverse, removes the folder only after its contents have gone back. Folders
  are removed only if empty, so anything you added since is safe.
- **Ordering.** Journals are ordered by creation time, then id length, never by
  filename (`x-2.jsonl` sorts before `x.jsonl` lexicographically).

## Restore semantics

Records are replayed newest-first. For each `move`:

| Situation | Behaviour |
|---|---|
| Item is at its recorded new location, original path free | Moved back |
| Original path is now occupied | `rename` (default): restore as `name (1).ext`. `skip`: leave it and report |
| Item no longer exists | Reported as missing; does not block completion |
| Move fails (permissions, in use) | Reported as failed; journal stays active so you can retry |

Only a run that ends with no conflicts and no failures is marked restored.

## Safety model

- `fsops::move_path` refuses to overwrite. It uses an atomic rename, falling
  back to copy → verify length → delete for files when a rename isn't possible
  (e.g. across volumes). A failed verification deletes the copy and leaves the original.
- Destinations are re-checked at execution time, so a file that appears after
  planning is never clobbered.
- Symlinks are never followed or moved. Hidden and system files are skipped
  unless requested. On Windows the HIDDEN and SYSTEM attributes count.
- Folders created by a previous run (category folders, `Projects`,
  `_Duplicates`) are skipped at the top level, which makes repeated runs idempotent.
- Names that aren't valid UTF-8 are skipped, since they can't be journaled faithfully.
- The tool makes no network connections and touches only the folder you give it.

## Duplicate detection

1. **Group by size.** Already known from the scan; unique sizes are dropped. Empty files are ignored.
2. **Partial hash.** BLAKE3 of the first 4 KiB of each size-mate. Most non-duplicates are eliminated here.
3. **Full hash.** BLAKE3 of the whole file for anything that still collides.

Stages 2 and 3 run on a scoped-thread pool (up to 8 workers) with results kept in
input order. Unreadable files are reported and excluded rather than aborting.

**Keeper choice:** shallowest path depth, then oldest modification time, then path order,
so results are deterministic. Duplicates land in `_Duplicates/Group-NNN/`, largest reclaimable
space first, with name collisions inside a group resolved as `name (1).ext`.

## Using it as a library

```rust
use tidy_up::{
    executor::execute,
    journal::Operation,
    plan::{build_organize_plan, organize_skip_dirs, ProjectPolicy},
    rules::IgnoreRules,
    scan::{scan, ScanOptions},
};

let root = std::path::Path::new("/home/me/Downloads");
let options = ScanOptions {
    max_depth: 1,
    rules: IgnoreRules::new(),
    skip_root_dirs: organize_skip_dirs(),
};
let plan = build_organize_plan(root, &scan(root, &options)?, ProjectPolicy::Keep);
let report = execute(&plan, Operation::Organize, |done, total| { /* progress */ })?;
```

Then `restore::restore(root, &mut journal, RestoreOptions::default(), |_, _| {})` to undo.

## Testing

```sh
cargo test
cargo clippy --all-targets
```

- **Unit tests** in every module (rules and globbing, categories, journal round trips and corruption, path collision handling, restore edge cases, hashing).
- **`tests/workflow.rs`**: real temp directories; organize → restore and dedupe → restore must reproduce the original tree exactly, including stacked runs undone in reverse order.
- **`tests/cli.rs`**: drives the compiled binary: flags, ignore files, dry runs, no-terminal behaviour, exit codes.
