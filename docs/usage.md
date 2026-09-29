# CLI reference

```text
tidy-up [COMMAND]
```

With no command, the interactive menu starts (requires a terminal).
Every command takes a folder as its first positional argument; the default is
the current directory. Aliases: `o` organize, `ro` reorganize, `d` dedupe, `c` compare,
`a` analyze, `x` distribute, `r` restore, `h` history.

## Global options

These work with any command, before or after it.

| Option | Default | Description |
|---|---|---|
| `--json` | off | Print one JSON object describing the run instead of the usual output. See [Machine output](#machine-output) |
| `--log` | off | Write a JSON Lines record of the run to `<folder>/.tidy-up/logs/`. `TIDY_UP_LOG=1` does the same |
| `--strict` | off | Exit non-zero if anything had to be skipped. Without it, a run that skipped items still succeeds |

`TIDY_UP_UTC_OFFSET` (for example `+03:00`) shifts the dates used by
`reorganize --by year|month|day`, which are otherwise UTC.

## Machine output

`--json` prints exactly one JSON object on stdout and nothing else, whatever
happened. Everything the terminal would have shown is silenced, including
progress bars, so the output is always parseable.

```console
$ tidy-up organize D:\Downloads --dry-run --json
{"tidy_up":"0.3.0","format":1,"command":"organize","status":"ok","outcome":{...}}
```

| Field | Meaning |
|---|---|
| `tidy_up` | Version of the tool |
| `format` | Version of this format. Bumped if anything here changes meaning |
| `command` | Sub-command that ran |
| `status` | `ok` or `error` |
| `outcome` | What it did. Present on success |
| `error` | `{code, message, path}`. Present on failure |

Inside `outcome`, every field is optional, because commands differ: `root` and
`roots`, `dry_run`, `plan`, `execution`, `restore`, `detail` (command-specific,
such as an analysis), `skipped` (counts by reason), `skipped_items` (only with
`--verbose`) and `problems`.

Confirmation prompts need a terminal, so pair `--json` with `--yes` or
`--dry-run`.

### Error codes

`error.code` is stable; `error.message` is prose and may be reworded.

| Code | Meaning | Exit |
|---|---|---|
| `system_folder` | Refused by the guard | 3 |
| `not_writable` | Refused: cannot write there, and no flag overrides it | 3 |
| `cancelled` | You declined a confirmation | 0 |
| `not_found` | The path does not exist | 1 |
| `not_a_folder` | The path is not a folder | 2 |
| `denied` | Permission denied | 1 |
| `overlap` | Folders overlap when they must be separate | 1 |
| `invalid_plan` | A plan file could not be read, or describes something unsafe | 1 |
| `invalid_journal` | A journal could not be read | 1 |
| `invalid` | The arguments did not make sense | 2 |
| `io` | Any other I/O failure | 1 |
| `internal` | A bug. Worth reporting | 1 |

## `apply`

Carry out a plan that an earlier `--dry-run --json` produced. Propose, review,
then apply: nothing is re-decided in between.

```sh
tidy-up organize D:\Downloads --dry-run --json > plan.json
# read plan.json, or hand it to someone who will
tidy-up apply plan.json --yes
```

| Option | Default | Description |
|---|---|---|
| `-n, --dry-run` | off | Show what the plan would do; change nothing |
| `-y, --yes` | off | Skip confirmation |
| `-v, --verbose` | off | List every move in the plan |
| `--allow-system-folder` | off | As for any other command |

The file may be a bare plan or a whole `--json` envelope with one inside.

A plan file is a **trust boundary**: it is JSON on disk, so it may have been
edited by hand or produced by something acting on instructions from elsewhere.
Before anything moves, tidy-up checks that every path in it is absolute and
inside the plan's own root, that the counts match, and that the root still
passes the system-folder guard. A plan that reaches outside its root is
refused. Files that moved or vanished since the plan was written are reported
as problems, and the rest of the plan still runs.

## The system-folder guard

Before any command changes anything, it checks the folder you named. Folders the
operating system manages are refused; see
[Platforms](platforms.md#system-folders) for the list per platform and the exact
policy. In short:

- Refused outright: `C:\Windows`, `Program Files`, `ProgramData`, `C:\Users`,
  `AppData`, `/usr`, `/etc`, `/System`, `/Library`, `~/Library`, `/home`, `/root`,
  drive roots, network share roots, and your own home folder.
- Warned and confirmed: `/usr/local`, `/opt`, `/srv`, the root of a mounted disk.
- `analyze` and `history` only read, so they warn and carry on.
- `--yes` answers a confirmation; it is never an override.
- `--allow-system-folder` gets you to a confirmation that defaults to no. A folder
  tidy-up cannot write to is refused even with it.

## Options in the interactive menu

If you started tidy-up by double-clicking it, or just ran `tidy-up` with nothing after
it, there is no command line to put flags on. **Change options** in the menu sets the
same things, and they apply to every action until you change them again:

| Menu option | Same as |
|---|---|
| Preview only, change nothing | `--dry-run` |
| Folders to look through | `--depth` |
| Include hidden files and folders | `--include-hidden` |
| Include shortcuts | `--include-shortcuts` |
| Extensions to leave alone | `--ignore-ext` |
| Names or patterns to leave alone | `--ignore` |
| Code projects | `--projects keep\|move` |
| List every item, not a sample | `--verbose` |
| Write a run log | `--log` |
| Allow folders the system manages | `--allow-system-folder` |

Whatever is not at its default is listed above the menu, so you can see at a glance
what the next action will do:

```text
  Options: preview only · all sub-folders · including hidden
```

Two notes. Turning on **Allow folders the system manages** asks you to confirm, and
the guard still asks again before anything is changed, so it is two deliberate steps
rather than one. And these are remembered for as long as the program is open and no
longer: tidy-up writes no configuration file, so there is nothing to clean up and
nothing to carry a surprise into the next run.

## `organize`

Sort files into category folders.

```sh
tidy-up organize [PATH] [OPTIONS]
```

| Option | Default | Description |
|---|---|---|
| `-d, --depth <N>` | `1` | Folder levels to scan. `1` = only files directly inside `PATH`. Must be ≥ 1 |
| `--projects <keep\|move>` | `keep` | Leave detected projects in place, or move them to `Projects/` |
| `-x, --ignore-ext <EXT>` | | Extensions to ignore; comma-separated or repeated |
| `-i, --ignore <PATTERN>` | | Names or globs to ignore; comma-separated or repeated |
| `-f, --ignore-file <FILE>` | | Ignore file; repeatable |
| `--include-shortcuts` | off | Also process `.lnk`, `.url`, `.webloc`, `.desktop` |
| `--include-hidden` | off | Also process hidden files and folders |
| `-n, --dry-run` | off | Show the plan; change nothing |
| `-y, --yes` | off | Skip confirmation |
| `-v, --verbose` | off | List every move and skipped item |
| `--allow-system-folder` | off | Proceed on a folder the system manages. You are still asked to confirm; `--yes` alone is not enough |

## `reorganize`

Unpack a folder that is organized badly and re-file everything. Where `organize`
sorts loose files and leaves its own output alone, this descends into everything,
including folders a previous run created, and deletes the folders it empties.

```sh
tidy-up reorganize [PATH] [OPTIONS]
```

| Option | Default | Description |
|---|---|---|
| `-b, --by <KEY,KEY,...>` | `type` | Grouping keys, outermost first. See below |
| `-d, --depth <N>` | all | Folder levels to unpack |
| `--keep-empty-dirs` | off | Leave folders that end up empty instead of deleting them |
| `--projects <keep\|move>` | `keep` | Leave detected projects in place, or move them to `Projects/` |
| `-x, --ignore-ext <EXT>` | | Extensions to ignore |
| `-i, --ignore <PATTERN>` | | Names or globs to ignore |
| `-f, --ignore-file <FILE>` | | Ignore file; repeatable |
| `--include-shortcuts` | off | Also process `.lnk`, `.url`, `.webloc`, `.desktop` |
| `--include-hidden` | off | Also process hidden files and folders |
| `-n, --dry-run` | off | Show the plan; change nothing |
| `-y, --yes` | off | Skip confirmation |
| `-v, --verbose` | off | List every move and skipped item |
| `--allow-system-folder` | off | Proceed on a folder the system manages. You are still asked to confirm; `--yes` alone is not enough |

### Grouping keys

Each key becomes one level of nesting, in the order given.

| Key | Folder | From |
|---|---|---|
| `type` | `Images`, `Documents`, `3D Models` | The same categories `organize` uses |
| `ext` | `PDF`, `JPG`, `No extension` | The extension, upper-cased |
| `year` | `2024`, `Unknown date` | Last modified |
| `month` | `03-March` | Last modified; the number keeps folders in calendar order |
| `day` | `2024-03-17` | Last modified |
| `size` | `Small (under 10 MiB)` | Five bands, powers of 1024 |
| `alpha` | `A`, `0-9`, `Other` | First character of the name |

```sh
tidy-up reorganize D:\Archive --by year,type   # 2024/Images/photo.jpg
tidy-up reorganize D:\Archive --by type,ext    # Images/PNG/photo.png
```

Every key is total, so no file is ever left unplaced. Running the same command
twice does nothing the second time. `restore` puts the tree back exactly, including
folders that held nothing.

## `dedupe`

Find files with identical contents and move the extra copies into
`_Duplicates/Group-NNN/`. The shallowest, then oldest, copy stays in place.
Empty files are never treated as duplicates. Writes a report to
`.tidy-up/reports/<run-id>-duplicates.txt`.

```sh
tidy-up dedupe [PATH] [OPTIONS]
```

Accepts the same ignore options as `organize`, plus:

| Option | Default | Description |
|---|---|---|
| `-d, --depth <N>` | unlimited | Folder levels to search |
| `-n, --dry-run` | off | Report duplicates; move nothing |
| `-y, --yes` | off | Skip confirmation |
| `-v, --verbose` | off | List every group |
| `--allow-system-folder` | off | Proceed on a folder the system manages. You are still asked to confirm; `--yes` alone is not enough |

Project folders are never searched.

## `compare`

Compare two or more folders by content and act on the overlap.

```sh
tidy-up compare <FOLDER>... [OPTIONS]
```

The **first folder is the primary**: when content exists in several places, its copy is
kept, moved copies land in its `_Duplicates/`, and merges gather everything into it.
Folders must not be inside one another.

The report shows, per folder, how many files are `shared` (content also in another
folder), `unique` (content found nowhere else) and `repeated inside` (repeated only
within that folder), then how each pair relates: identical, one contained in the other,
overlapping, or nothing in common.

| Option | Default | Description |
|---|---|---|
| `-a, --action <leave\|move\|merge\|delete>` | ask | What to do with duplicated content. Without a terminal and without this flag, `leave` is assumed (report only) |
| `-d, --depth <N>` | unlimited | Folder levels to search inside each folder |
| `-n, --dry-run` | off | Show the plan; change nothing |
| `-y, --yes` | off | Skip confirmation |
| `-v, --verbose` | off | List every duplicate group |
| `--allow-system-folder` | off | Proceed on a folder the system manages. You are still asked to confirm; `--yes` alone is not enough |

Plus the ignore options from `organize` (`-x`, `-i`, `-f`, `--include-shortcuts`, `--include-hidden`).

| Action | Effect | Undoable |
|---|---|---|
| `leave` | Report only | n/a |
| `move` | Extra copies go to `<primary>/_Duplicates/Group-NNN/` | yes, `tidy-up restore <primary>` |
| `merge` | Files that exist only outside the primary (and kept copies that live outside it) are moved into the primary at the same relative path, clashes renamed `name (1).ext`; extra copies go to `_Duplicates/` | yes, `tidy-up restore <primary>` |
| `delete` | Extra copies are deleted. Each is re-hashed first and skipped if it, or the kept copy, changed since the scan | **no** |

## `analyze`

Read-only report of where the space goes. Reads metadata only (never file contents unless
`--duplicates` is given), keeps memory bounded, and writes nothing.

```sh
tidy-up analyze [PATH]... [OPTIONS]
```

| Option | Default | Description |
|---|---|---|
| `-t, --top <N>` | `10` | Entries in each "largest" list (`0` to hide them) |
| `--duplicates` | off | Also measure space wasted by duplicate files; reads file contents, so it is slower |

The report has: file, folder and byte totals; usage of the partition holding the folder; a breakdown by
[file type](file-types.md); the largest files and folders (folder sizes include everything below them);
code projects (outermost only, so `node_modules` inside a project is part of it); data by last-modified
age (under 30 days, under 1 year, under 3 years, older); empty files and folders; and duplicate waste.
Sizes are apparent sizes (the file length), not disk allocation. Symbolic links are never followed.

## `distribute`

Move files from full folders into other places (usually other partitions), controlling how much,
how it is split, and how full each destination may get. Full guide: [distribute.md](distribute.md).

```sh
tidy-up distribute --from <FOLDER>... --to <FOLDER>... [OPTIONS]
```

| Option | Default | Description |
|---|---|---|
| `--from <FOLDER>...` | required | Sources to move files out of |
| `--to <FOLDER>...` | required | Destinations (created if missing). The undo journal lives in the first |
| `--ratio <N,N,...>` | | Split the moved data in these proportions, one number per destination |
| `--strategy <fill\|free\|even>` | `fill` | Without a ratio: equalise fill %, proportional to free space, or equal shares |
| `--limit <SIZE>` | none | Move at most this much (`200GiB`, `500M`; units are binary) |
| `--max-fill <PERCENT>` | `90` | Never fill a destination's partition beyond this |
| `--min-free <SIZE>` | `0` | Always keep at least this much free on every destination |
| `--prefer <largest\|oldest>` | `largest` | What to move first when a limit applies |
| `--layout <keep\|organize>` | `keep` | Keep the folder structure, or sort into category folders |
| `--granularity <item\|file>` | `item` | Move whole top-level items (folders stay together) or single files |
| `-n`, `-y`, `-v` | | Dry run, skip confirmation, list every item |
| `--allow-system-folder` | off | Proceed on a folder the system manages. You are still asked to confirm; `--yes` alone is not enough |

Plus the ignore options from `organize`. Sources and destinations must be separate folders. Undo with
`tidy-up restore <first destination>`.

## `purge`

Permanently delete `_Duplicates/`. Shows the file count and size, defaults to
"no", and cannot be undone by `restore`.

```sh
tidy-up purge [PATH] [-y] [--allow-system-folder]
```

## `restore`

Undo recorded runs.

```sh
tidy-up restore [PATH] [OPTIONS]
```

| Option | Default | Description |
|---|---|---|
| `--id <ID>` | latest active | Restore a specific run (unique prefix accepted) |
| `--all` | off | Restore every active run, newest first. Conflicts with `--id` |
| `--on-conflict <rename\|skip>` | `rename` | When the original name is taken again: restore as `name (1).ext`, or leave the file where it is |
| `-n, --dry-run` | off | Report what would happen |
| `-y, --yes` | off | Skip confirmation |
| `--allow-system-folder` | off | Proceed on a folder the system manages. You are still asked to confirm; `--yes` alone is not enough |

A run that finishes with no conflicts or failures is marked restored and won't
be applied twice. Files deleted since the run are reported as missing and don't
block completion.

## `show`

What one run actually did, and whether it can still be undone. `history` lists
runs; this explains one.

```sh
tidy-up show [PATH] [ID] [-v]
```

With no id, the most recent run. An id may be a unique prefix.

| Option | Default | Description |
|---|---|---|
| `-v, --verbose` | off | List every move rather than a sample |

```console
$ tidy-up show D:\Downloads

Run 20260929-014441
- organize - 2026-09-29 01:44:41 UTC - 5 moves

What it moved
  ? a.png -> Images.png
    b.pdf -> Documents.pdf
    ...
- 5 folders created

What it left alone
  hidden items: 1

Can it still be undone?
! Partly. 1 of 5 have since been moved, renamed or deleted, and will be
  reported as missing.
  tidy-up restore "D:\Downloads" --id 20260929-014441
```

A `?` marks an item that is no longer where the run put it. The journal cannot
know that, because the folder carries on changing after a run, so `show`
checks the disk as it reports.

**What the run left alone** is only available when the run used `--log`: a
journal records changes, so it can never say what did not change. Without a
log the rest of the review still works.

Read-only. Reviewing never modifies the folder.

## `history`

List recorded runs for a folder, newest first, with operation, time (UTC), move
count and whether each is `active` or `restored`.

## Ignore rules

Sources, all combined: `-x`, `-i`, and `-f` files.

Ignore-file syntax, one entry per line:

| Entry | Meaning |
|---|---|
| `# text` / blank | Comment / skipped |
| `.iso` | Extension (case-insensitive) |
| `.tar.gz` | Multi-part extension (becomes the glob `*.tar.gz`) |
| `*.tmp`, `draft-??.txt` | Glob: `*` any run of characters, `?` any one character |
| `notes.txt` | Exact name (case-insensitive) |

Matching is case-insensitive. Extension rules apply to files only; name and glob
rules apply to files and folders.

Always skipped regardless of flags: OS files (`desktop.ini`, `Thumbs.db`,
`.DS_Store`, Office `~$` lock files), symlinks, the `.tidy-up/` state folder,
and names that aren't valid UTF-8.

## Categories

| Folder | Examples |
|---|---|
| Images | jpg png gif webp heic svg raw dng |
| Videos | mp4 mkv mov avi webm |
| Audio | mp3 wav flac m4a ogg |
| Documents | pdf doc docx odt rtf tex |
| Text Files | txt md log rst |
| Spreadsheets | xls xlsx ods csv tsv |
| Presentations | ppt pptx odp key |
| Ebooks | epub mobi azw3 |
| Archives | zip rar 7z tar gz zst |
| Code | rs py js ts java c cpp go html css sh ps1 ipynb … |
| 3D Models | stl obj fbx blend gltf glb 3mf step |
| Design | psd ai xd fig sketch eps |
| Fonts | ttf otf woff woff2 |
| Installers | exe msi dmg pkg deb iso |
| Data | json xml yaml toml sql db sqlite |
| Other | anything else, including files with no extension |

The table lives in `src/category.rs`; a test guarantees no extension is listed
twice.

## Project detection

A folder is a project if it directly contains any of: `.git`, `.hg`, `.svn`,
`Cargo.toml`, `package.json`, `pyproject.toml`, `setup.py`, `go.mod`, `pom.xml`,
`build.gradle(.kts)`, `CMakeLists.txt`, `composer.json`, `Gemfile`,
`pubspec.yaml`, `mix.exs`, `deno.json`, `Package.swift`, `project.godot`, or a
`*.sln`, `*.csproj`, `*.fsproj`, `*.vcxproj`, `*.uproject`, `*.xcodeproj`.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success, including "nothing to do", a cancelled confirmation, and a run that skipped items it could not process |
| `1` | Something went wrong and stopped the run |
| `2` | The arguments did not make sense |
| `3` | Refused deliberately: the system-folder guard, or a permission wall |
| `4` | Finished, but items were skipped and `--strict` was given |

`3` exists so a caller can tell a refusal from a malfunction. A refusal is an
answer and should not be retried; a failure might be worth retrying or
reporting. Before 0.3.0 both were `1`.

A run that could not process some items still exits `0`, because it did the work it
could; the items are listed at the end, grouped by cause, and appear under
`problems` in `--json`. Pass `--strict` to exit `4` instead.
