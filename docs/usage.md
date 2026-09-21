# CLI reference

```text
tidy-up [COMMAND]
```

With no command, the interactive menu starts (requires a terminal).
Every command takes a folder as its first positional argument; the default is
the current directory. Aliases: `o` organize, `d` dedupe, `c` compare, `a` analyze, `x` distribute, `r` restore, `h` history.

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
| `--json` | off | Print JSON (an array, one object per folder) instead of a report |
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

Plus the ignore options from `organize`. Sources and destinations must be separate folders. Undo with
`tidy-up restore <first destination>`.

## `purge`

Permanently delete `_Duplicates/`. Shows the file count and size, defaults to
"no", and cannot be undone by `restore`.

```sh
tidy-up purge [PATH] [-y]
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

A run that finishes with no conflicts or failures is marked restored and won't
be applied twice. Files deleted since the run are reported as missing and don't
block completion.

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

`0` success (including "nothing to do" and cancelled), `1` runtime error,
`2` invalid arguments.
