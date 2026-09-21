# How it works

## Pipeline

```text
classify -> scan -> plan -> execute -> journal -> restore
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
  changes letter (or a folder that is renamed) keeps a valid history. When several
  folders are compared, the journal lives in the primary folder and paths outside
  it are stored absolute (joining an absolute path onto the root yields that path,
  which is how restore finds them).
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

Stages 2 and 3 run on a scoped-thread pool (at most 4 workers, and never more than half the cores) with results kept in
input order. Unreadable files are reported and excluded rather than aborting.

**Keeper choice:** shallowest path depth, then oldest modification time, then path order,
so results are deterministic. Duplicates land in `_Duplicates/Group-NNN/`, largest reclaimable
space first, with name collisions inside a group resolved as `name (1).ext`.

## Comparing folders

`compare` scans each folder, tags every file with the index of the folder it came from
(`FileEntry::source`), and runs the same duplicate pipeline over the combined list. Keeper
choice ranks `source` first, so the earliest-listed folder always wins, then depth, age and
path. A folder's content is modelled as the set of distinct contents it holds (each
duplicate group is one identity), which makes the pairwise relations (identical, subset,
overlapping, disjoint) simple set comparisons.

Merge is a plan like any other: extra copies to `_Duplicates/`, plus a move into the
primary for every file that exists only elsewhere and for every kept copy that lives
outside it. It is executed and journaled by the same executor, so it is undoable in the same way.

Delete is the one irreversible action and is deliberately not journaled. Right before
removing a file it is re-hashed and compared with the group hash, and so is the kept copy;
anything that changed since the scan is left alone and reported.

## Analysis

`analyze` is one metadata-only walk. Directory entries are collected per folder so project markers
can be spotted before descending (which is how nested projects, such as packages inside `node_modules`,
are attributed to the outermost project instead of being counted thousands of times). Symlinks are not
followed. Memory stays bounded however large the tree is: the "largest" lists are fixed-size heaps
ranked by `(size, path)`, so the result never depends on directory iteration order (which differs between
Windows, macOS and Linux), and a candidate's path is only built once it is known to make the list.

## Distribution

`distribute` is three pure-ish steps: `collect_units` (reads the sources), `allocate` (no I/O), and
`build_plan` (a normal `Plan` for the shared executor).

`allocate` works in this order:

1. **Capacity.** Each destination offers `available - reserve`, where the reserve is the larger of
   `--min-free` and the space needed to stay under `--max-fill`. Destinations on one partition share a pool.
2. **Selection.** Units are taken in `--prefer` order until `--limit` would be exceeded; a unit that does
   not fit the remaining budget is skipped and smaller ones are still considered.
3. **Targets.** A target number of bytes per destination comes from the strategy. Weighted strategies
   (`ratio`, `free`, `even`) are water-filled: a destination that cannot absorb its proportional share is
   capped and its overflow is shared by the rest. `fill` finds, by bisection, the common fill level `L` at
   which filling every destination up to `L` of its partition absorbs exactly the amount being moved.
4. **Placement.** Units are placed largest first, each on the eligible destination furthest below its
   target that still has room. Anything that fits nowhere is reported, never forced.

Because the allocator takes partition sizes as plain numbers, every "my three drives look like this"
scenario is a fast unit test (see `disk::StaticDisks`).

## Resource use and progress

- Hashing runs on at most `min(4, cores / 2)` threads, so the machine stays responsive.
- Passes are staged (size, then first 4 KiB, then full hash), so most files are never read in full.
- Progress is byte-based: `HashBar` restarts for each hashing pass, and `TransferBar` shows
  bytes moved, throughput, ETA and the current file while moving, which stays honest for
  multi-gigabyte files copied between drives. Both draw to stderr and stay hidden when it
  is not a terminal, so piping and scripting output is unaffected.
- The library exposes progress through the `HashProgress` trait and the `executor::Progress`
  struct, so any front end can render its own.

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
let report = execute(&plan, Operation::Organize, |p| { /* p.done, p.bytes_done, p.current */ })?;
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
