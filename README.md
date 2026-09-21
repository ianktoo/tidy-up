<div align="center">

# tidy-up

**Your Downloads folder is a crime scene. Clean it up, and undo it if you don't like the result.**

A fast, single-binary Rust CLI for Windows, macOS and Linux that sorts messy folders by file type, quarantines duplicates, compares whole drives, spreads data across full partitions, and journals every move so nothing is ever a one-way door.

[Get started](docs/getting-started.md) · [CLI reference](docs/usage.md) · [How it works](docs/how-it-works.md) · [Contributing](CONTRIBUTING.md)

</div>

---

## The problem

You write code, you study, you do creative work, you make 3D prints. Your Desktop and Downloads hold all of it at once: `final_v2_REAL.pdf`, six copies of the same installer, a half-finished `.stl`, and a git repo you cloned "just for a minute" in March. And your drives are 90% full.

Existing organizers are either a cron job that yolo-moves files, or a GUI you have to babysit. Neither answers the only question that matters:

> *"What if it moves something I needed?"*

## What makes tidy-up different

### 1. Every run is reversible

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

### 2. It shows you where the space went

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

### 3. It spreads data across partitions, by the numbers

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

### 4. It knows what a repo is

Folders containing `.git`, `Cargo.toml`, `package.json`, `*.sln` and friends are detected as projects and **left exactly where they are** when organizing. Moving a repo breaks IDE configs, symlinks and absolute paths. When you *do* move one (`distribute`), it travels whole, `.git` and all. Gather them with `--projects move`.

### 5. Duplicates are quarantined, not deleted

Files are compared by content, never by name. Extra copies move into `_Duplicates/Group-001/`, and the copy you'd want (shallowest, oldest) stays put. You review, then `tidy-up purge`, or `restore` to put it all back. Deletion is always a separate, explicit, confirmed step. [How dedup works](docs/deduplication.md).

```console
$ tidy-up dedupe
Duplicates
  Group-001 2 copies x 5 B
    keep Text Files\a.txt
    move Text Files\a-copy.txt
```

### 6. It compares whole folders, not just files

Point it at two, three or ten folders (across drives, backups, old laptop dumps) and it tells you how they relate, by content, not by name:

```console
$ tidy-up compare D:\Photos E:\Backup F:\OldLaptop
How they compare
  * #1 and #2 have identical content
  * everything in #3 is also in #1 (#1 has more)
```

Then you decide: **leave** it as a report, **move** the extra copies aside, **merge** everything into one folder, or **delete** the extras. The first folder you list is the one whose copies win. Move and merge are fully undoable, and delete re-hashes every file right before removing it.

### 7. It stays out of your way

- **Shortcuts and hidden files skipped by default** (`.lnk`, `.url`, dotfiles, `desktop.ini`); opt in with a flag.
- **Ignore anything** by extension, glob or exact name, from flags or a text file you commit alongside your dotfiles.
- **Never overwrites.** Name clash? You get `name (1).ext`. Symlinks are never followed.
- **Gentle on your machine.** Hashing uses at most half your cores (never more than 4 threads), and files are only fully read when a cheaper check says they might match. Live progress bars show throughput, ETA and the current file.
- **Preview first.** Confirmation by default, `--dry-run` when you're curious, `--yes` when you're scripting.

## Try it in 30 seconds

Grab a binary for your platform from the [Releases page](https://github.com/ianktoo/tidy-up/releases) ([install guide](docs/platforms.md)), or build from source:

```sh
cargo install --path .
tidy-up analyze ~/Downloads              # where did the space go?
tidy-up organize ~/Downloads --dry-run   # look, don't touch
tidy-up organize ~/Downloads             # do it
tidy-up restore  ~/Downloads             # change your mind
```

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
classify -> scan -> plan -> execute -> journal -> restore
```

- A `Plan` is plain data, so previews, dry runs and confirmation prompts cost nothing.
- The distribution planner is pure too: it takes partition sizes as numbers, so every "what if my drives look like this" case is a unit test.
- The executor treats a failed journal write as fatal and rolls back the move in flight, because a change you can't record is a change you can't undo.
- 200+ tests: unit tests per module, end-to-end round trips on real temp directories, and black-box tests that drive the compiled binary.
- Small dependency tree, no network access, no telemetry. It only ever touches the folders you point it at.

Want to embed it? Everything is `pub` and documented (`cargo doc --open`).

## Documentation

| | |
|---|---|
| [Getting started](docs/getting-started.md) | Install, first run, common recipes |
| [CLI reference](docs/usage.md) | Every command, flag and the ignore-file syntax |
| [Distributing across partitions](docs/distribute.md) | Ratios, limits, fill levels, safety, examples |
| [How deduplication works](docs/deduplication.md) | What counts as a duplicate, the hashing pipeline, keeper choice, limits |
| [Supported file types](docs/file-types.md) | All 174 extensions in 15 categories, and how classification works |
| [Platforms and installing](docs/platforms.md) | Windows, macOS, Linux: binaries, checksums, platform notes |
| [How it works](docs/how-it-works.md) | Journal format, architecture, safety model |
| [Contributing](CONTRIBUTING.md) | Setup, branches, tests, releasing |

## License

Source code is [MIT](LICENSE). Official binaries are additionally covered by the [EULA](EULA.md), free forever.

Built by [Ian Too](https://iantoo.space) · [hello@iantoo.space](mailto:hello@iantoo.space) · security reports: see [SECURITY.md](SECURITY.md)
