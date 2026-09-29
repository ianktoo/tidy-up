<div align="center">

# tidy-up

**Your Downloads folder is a crime scene. Clean it up, and undo it if you don't like the result.**

A fast, single-binary Rust CLI for Windows, macOS and Linux that sorts messy folders by file type, re-files badly organized ones, quarantines duplicates, compares whole drives, spreads data across full partitions, and journals every move so nothing is ever a one-way door. It also refuses to reorganize the folders your operating system needs.

[Get started](docs/getting-started.md) · [CLI reference](docs/usage.md) · [How it works](docs/how-it-works.md) · [Contributing](CONTRIBUTING.md)

</div>

---

## The problem

You write code, you study, you do creative work, you make 3D prints. Your Desktop and Downloads hold all of it at once: `final_v2_REAL.pdf`, six copies of the same installer, a half-finished `.stl`, and a git repo you cloned "just for a minute" in March. And your drives are 90% full.

Existing organizers are either a cron job that yolo-moves files, or a GUI you have to babysit. Neither answers the only question that matters:

> *"What if it moves something I needed?"*

## What makes tidy-up different

### 1. It refuses to reorganize your operating system

Every command checks the folder you named before it moves anything. `C:\Windows`,
`/usr`, `/System`, a drive root, a network share root and your home folder are
refused outright, on all three platforms:

```console
$ tidy-up organize C:\Windows --yes
Careful
! C:\Windows is a folder the system manages.
  It holds the Windows installation, which other programs expect to find in place.
error: refusing to change C:\Windows: it looks like a folder the system manages.
       If you are certain, re-run with --allow-system-folder
```

**`--yes` is not an override.** A script that passes `--yes` keeps working on
ordinary folders and starts failing loudly on system ones. `--allow-system-folder`
gets you to a confirmation that defaults to no; it does not skip it. A folder
tidy-up genuinely cannot write to is refused even with the flag, because no flag
grants permission the operating system refused.

Folders worth a second thought rather than a refusal, such as `/usr/local`, `/opt`
or the root of a mounted disk, warn and ask. `analyze` and `history` only look, so
they warn and carry on.

The rules are the boring part; not firing on `D:\Downloads` is the hard part. There
is a test whose only job is to assert that two dozen perfectly ordinary folders stay
silent, because a guard that cries wolf is a guard people learn to bypass.

### 2. Every run is reversible

Each move is written to an append-only journal *as it happens*. `tidy-up restore` walks it backwards and puts the folder back as it was, including removing the folders it created. We test this by diffing the directory tree before and after every kind of run.

```console
$ tidy-up organize ~/Downloads
Plan
  3D Models/            1         6 B
  Documents/            1         4 B
  Images/               2         8 B
  Text Files/           2        10 B
  6 items · 28 B total

Left alone
  1 item code projects
  1 item shortcuts

? Move 6 items into category folders? [Y/n]

✔ Moved 6 items (28 B)
  Undo with: tidy-up restore "C:\Users\you\Downloads"
```

### 3. It shows you where the space went

`tidy-up analyze` is read-only and answers "why is this drive full?" in one screen: partition usage, breakdown by file type, the biggest files and folders, how stale the data is, how much your code projects weigh, and (optionally) how much space duplicates waste. `--json` for scripts.

```console
$ tidy-up analyze ~/Downloads --duplicates
Analysis of C:\Users\you\Downloads
  371 files in 20 folders · 2.1 GiB
  Partition  ██████████████████████░░ 90.9% used · 414.1 GiB of 455.7 GiB · 41.7 GiB free

By type
  Installers      4 files     2.0 GiB   94.3%  ███████████████████████░
  Archives       15 files    57.9 MiB    2.7%  █░░░░░░░░░░░░░░░░░░░░░░░
  ...
Duplicates
  14 extra copies in 14 groups waste 4.3 MiB
```

### 4. It spreads data across partitions, by the numbers

Three partitions, all nearly full? `tidy-up distribute` moves data out of them into other places and you set the rules: **how much** (`--limit 200GiB`), **how it's split** (`--ratio 50,30,20`, or automatically by free space, or by equalising how full they end up), and **how full** each destination may get (`--max-fill 90`, `--min-free 20GiB`). It can sort into category folders as it goes, and it keeps folders and code projects together.

```console
$ tidy-up distribute --from D:\Media --to E:\Archive F:\Archive --ratio 60,40 --limit 200GiB --max-fill 90 -n
Destinations
  #1  E:\Archive
      71.0% -> 83.4% full  +120.0 GiB  (14 items), room for 93.0 GiB
  #2  F:\Archive
      55.0% -> 68.2% full  +80.0 GiB   (9 items), room for 210.0 GiB
Sources
  D:\Media
      96.0% -> 86.3% full  (frees 200.0 GiB)
```

### 5. It knows what a repo is

Folders containing `.git`, `Cargo.toml`, `package.json`, `*.sln` and friends are detected as projects and **left exactly where they are** when organizing. Moving a repo breaks IDE configs, symlinks and absolute paths. When you *do* move one (`distribute`), it travels whole, `.git` and all. Gather them with `--projects move`.

### 6. Duplicates are quarantined, not deleted

Files are compared by content, never by name. Extra copies move into `_Duplicates/Group-001/`, and the copy you'd want (shallowest, oldest) stays put. You review, then `tidy-up purge`, or `restore` to put it all back. Deletion is always a separate, explicit, confirmed step. [How dedup works](docs/deduplication.md).

```console
$ tidy-up dedupe
Duplicates
  Group-001 2 copies x 5 B
    keep Text Files\a.txt
    move Text Files\a-copy.txt
```

### 7. It compares whole folders, not just files

Point it at two, three or ten folders (across drives, backups, old laptop dumps) and it tells you how they relate, by content, not by name:

```console
$ tidy-up compare D:\Photos E:\Backup F:\OldLaptop
How they compare
  * #1 and #2 have identical content
  * everything in #3 is also in #1 (#1 has more)
```

Then you decide: **leave** it as a report, **move** the extra copies aside, **merge** everything into one folder, or **delete** the extras. The first folder you list is the one whose copies win. Move and merge are fully undoable, and delete re-hashes every file right before removing it.

### 8. It re-files folders that were organized badly

`organize` sorts loose files and deliberately leaves its own output alone.
`reorganize` is for the folder somebody organized badly years ago: it unpacks the
whole tree, including folders a previous run created, and re-files everything by
keys you choose, in the order you choose.

```console
$ tidy-up reorganize D:\Archive --by year,type
Move 4,812 items and re-file by year, then type? 137 folders will be left empty
and deleted.
```

Keys are `type`, `ext`, `year`, `month`, `day`, `size` and `alpha`, combined in any
order: `--by year,month` gives `2024/03-March/`, `--by type,ext` gives `Images/PNG/`.

Running it twice does nothing the second time. Folders it empties are deleted, and
`restore` puts the tree back exactly as it was, **including the folders that held
nothing** (a journal of moves alone cannot express that, so the journal records
removals too).

Dates are UTC, because the standard library has no time zone API and guessing badly
on one platform is worse than being explicit. Set `TIDY_UP_UTC_OFFSET=+03:00` to
group by your own clock.

### 9. You can check what it did afterwards

`tidy-up show` explains one run: what moved, what it left alone, and whether
undoing it will still be clean. That last part is the one the journal cannot
answer on its own, because a folder carries on changing after a run, so `show`
checks the disk as it reports.

```console
$ tidy-up show D:\Downloads
Can it still be undone?
! Partly. 1 of 5 have since been moved, renamed or deleted, and will be
  reported as missing.
  tidy-up restore "D:\Downloads" --id 20260929-014441
```

This matters most when something other than a person did the work. An undo
journal is only worth having if somebody can read it.

### 10. It is built to be driven by something other than a person

Every command takes `--json` and prints exactly one object, whatever happened.
Errors carry a stable code, and the exit code says whether tidy-up refused
(`3`) or broke (`1`), which a caller needs in order to know whether retrying
makes any sense.

Because a plan is plain data, the two halves come apart: one process proposes,
a person or another process reviews, a third carries it out.

```console
$ tidy-up organize D:\Downloads --dry-run --json > plan.json
$ tidy-up apply plan.json --yes
```

A plan file is a trust boundary, not just a convenience. It is JSON on disk, so
before anything moves tidy-up checks that every path in it sits inside the
plan's own root and that the root still passes the guard. A plan that reaches
outside itself is refused rather than carried out.

### 11. One bad file never ends the run

A locked file, a folder you lack permission to read, a name the filesystem rejects:
each is skipped, classified and reported at the end, grouped by cause with something
you can act on.

```console
!  23 items could not be processed:
   Permission denied               18  (pick a folder you own, or run as an administrator)
   In use by another program        4  (close the programs holding these files and run again)
   No longer there                  1
```

A run that skipped things still exits `0`, because it did the work it could. Pass
`--strict` if you would rather it failed. The one exception is a journal write
failure, which is fatal on purpose and rolls back the move in flight: a change that
cannot be recorded is a change you cannot undo.

### 12. It stays out of your way

- **Shortcuts and hidden files skipped by default** (`.lnk`, `.url`, dotfiles, `desktop.ini`); opt in with a flag.
- **Ignore anything** by extension, glob or exact name, from flags or a text file you commit alongside your dotfiles.
- **Never overwrites.** Name clash? You get `name (1).ext`. Symlinks are never followed.
- **Gentle on your machine.** Hashing uses at most half your cores (never more than 4 threads), and files are only fully read when a cheaper check says they might match. Live progress bars show throughput, ETA and the current file.
- **Preview first.** Confirmation by default, `--dry-run` when you're curious, `--yes` when you're scripting (on ordinary folders; see #1).
- **Options without a command line.** Run `tidy-up` with no arguments and **Change options** in the menu sets what the flags set: preview, depth, hidden files, ignore rules, logging and the rest. Nothing is written to disk.
- **An optional run log.** `--log` writes a JSON Lines record of a run beside its undo journal: what the guard decided, what was skipped and why, what failed, how long each stage took. Off by default.

## Try it in 30 seconds

Grab a binary for your platform from the [Releases page](https://github.com/ianktoo/tidy-up/releases) ([install guide](docs/platforms.md)), or build from source:

```sh
cargo install --path .
tidy-up analyze ~/Downloads                      # where did the space go?
tidy-up organize ~/Downloads --dry-run           # look, don't touch
tidy-up organize ~/Downloads                     # do it
tidy-up reorganize ~/Archive --by year,type -n   # re-file an old mess, preview only
tidy-up restore  ~/Downloads                     # change your mind
```

Driving it from a script or an agent? Add `--json` to any command.
[Machine output](docs/usage.md#machine-output).

Every release archive includes scripts that install `tidy-up`, put it on your PATH, and undo that again (`scripts/add-to-path.sh`, `scripts/add-to-path.ps1` and their `remove-from-path` counterparts).

Or run `tidy-up` with no arguments for the interactive menu.

## Platforms

| | x64 | arm64 |
|---|:-:|:-:|
| **Windows** | yes | yes |
| **macOS** | yes | yes (Apple Silicon) |
| **Linux** | yes (glibc and static musl) | yes |

CI runs the full test suite on all six OS and CPU combinations (plus static musl, and the minimum Rust version) for every change, including real moves between two separate volumes on each OS. Every release ships prebuilt binaries with checksums. [Install, add to PATH, upgrade and uninstall](docs/platforms.md).

## Built like a library, shipped like a tool

The CLI is a thin layer over a reusable crate. Each stage is a pure, independently testable piece:

```text
guard -> classify -> scan -> plan -> execute -> journal -> restore
```

- The guard is a pure function of the path, the environment and a few probed facts, and it takes the platform as an *argument*. All three rule tables are therefore tested from any one machine, which is how the macOS and Linux rules are verified on a Windows laptop.
- A `Plan` is plain data, so previews, dry runs and confirmation prompts cost nothing.
- The distribution planner is pure too: it takes partition sizes as numbers, so every "what if my drives look like this" case is a unit test.
- The executor treats a failed journal write as fatal and rolls back the move in flight, because a change you can't record is a change you can't undo.
- Failures are classified once, in one place, and collected rather than propagated, so a bulk run survives the awkward corners of a real disk.
- 350+ tests: unit tests per module, end-to-end round trips compared on both file contents and directory structure, and black-box tests that drive the compiled binary. Run on six OS and CPU combinations plus the minimum supported Rust version.
- Small dependency tree, no network access, no telemetry. It only ever touches the folders you point it at.

Want to embed it? Everything is `pub` and documented (`cargo doc --open`).

## Documentation

| | |
|---|---|
| [Getting started](docs/getting-started.md) | Install, first run, common recipes |
| [CLI reference](docs/usage.md) | Every command, flag, the menu options and the ignore-file syntax |
| [Distributing across partitions](docs/distribute.md) | Ratios, limits, fill levels, safety, examples |
| [How deduplication works](docs/deduplication.md) | What counts as a duplicate, the hashing pipeline, keeper choice, limits |
| [Supported file types](docs/file-types.md) | All 174 extensions in 15 categories, and how classification works |
| [Platforms and installing](docs/platforms.md) | Windows, macOS, Linux: binaries, checksums, platform notes |
| [How it works](docs/how-it-works.md) | Journal format, architecture, the system-folder guard, safety model |
| [The design paper](paper/) | Why the guard, the failure taxonomy and the re-grouping algorithm work the way they do |
| [Contributing](CONTRIBUTING.md) | Setup, branches, tests, releasing |

## License

Source code is [MIT](LICENSE). Official binaries are additionally covered by the [EULA](EULA.md), free forever.

Built by [Ian Too](https://iantoo.space) · [hello@iantoo.space](mailto:hello@iantoo.space) · security reports: see [SECURITY.md](SECURITY.md)
