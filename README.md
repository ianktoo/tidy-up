<div align="center">

# tidy-up

**Your Downloads folder is a crime scene. Clean it up, and undo it if you don't like the result.**

A fast, single-binary Rust CLI that sorts messy folders by file type, quarantines duplicates, and journals every move so nothing is ever a one-way door.

[Get started](docs/getting-started.md) · [CLI reference](docs/usage.md) · [How it works](docs/how-it-works.md)

</div>

---

## The problem

You write code, you study, you do creative work, you make 3D prints. Your Desktop and Downloads hold all of it at once: `final_v2_REAL.pdf`, six copies of the same installer, a half-finished `.stl`, and a git repo you cloned "just for a minute" in March.

Existing organizers are either a cron job that yolo-moves files, or a GUI you have to babysit. Neither answers the only question that matters:

> *"What if it moves something I needed?"*

## What makes tidy-up different

### 1. Every run is reversible

Each move is written to an append-only journal *as it happens*. `tidy-up restore` walks it backwards and puts the folder back as it was, including removing the folders it created. We test this by diffing the directory tree before and after a full organize → dedupe → restore cycle.

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

### 2. It knows what a repo is

Folders containing `.git`, `Cargo.toml`, `package.json`, `*.sln` and friends are detected as projects and **left exactly where they are**. Moving a repo breaks IDE configs, symlinks and absolute paths. If you want them gathered anyway: `--projects move`.

### 3. Duplicates are quarantined, not deleted

Files are compared by content, never by name. Extra copies move into `_Duplicates/Group-001/`, and the copy you'd want (shallowest, oldest) stays put. You review, then `tidy-up purge`, or `restore` to put it all back. Deletion is always a separate, explicit, confirmed step.

```console
$ tidy-up dedupe
Duplicates
  Group-001 2 copies × 5 B
    keep Text Files\a.txt
    move Text Files\a-copy.txt
```

Under the hood: group by size → hash the first 4 KiB → full BLAKE3 hash, spread over a worker pool. Most files are never fully read.

### 4. It stays out of your way

- **Shortcuts and hidden files skipped by default** (`.lnk`, `.url`, dotfiles, `desktop.ini`); opt in with a flag.
- **Ignore anything** by extension, glob or exact name, from flags or a text file you commit alongside your dotfiles.
- **Never overwrites.** Name clash? You get `name (1).ext`. Symlinks are never followed.
- **Preview first.** Confirmation by default, `--dry-run` when you're curious, `--yes` when you're scripting.

## Try it in 30 seconds

```sh
cargo install --path .
tidy-up organize ~/Downloads --dry-run   # look, don't touch
tidy-up organize ~/Downloads             # do it
tidy-up restore  ~/Downloads             # change your mind
```

Or run `tidy-up` with no arguments for the interactive menu.

## Built like a library, shipped like a tool

The CLI is a thin layer over a reusable crate. Each stage is a pure, independently testable piece:

```text
classify ─▶ scan ─▶ plan ─▶ execute ─▶ journal ─▶ restore
```

- A `Plan` is plain data, so previews, dry runs and confirmation prompts cost nothing.
- The executor treats a failed journal write as fatal and rolls back the move in flight, because a change you can't record is a change you can't undo.
- 100+ tests: unit tests per module, end-to-end round trips on real temp directories, and black-box tests that drive the compiled binary.
- Small dependency tree, no network access, no telemetry. It only ever touches the folder you point it at.

Want to embed it? Everything is `pub` and documented (`cargo doc --open`).

## Documentation

| | |
|---|---|
| [Getting started](docs/getting-started.md) | Install, first run, common recipes |
| [CLI reference](docs/usage.md) | Every command, flag and the ignore-file syntax |
| [How it works](docs/how-it-works.md) | Journal format, dedupe pipeline, architecture, safety model |

## License

Source code is [MIT](LICENSE). Official binaries are additionally covered by the [EULA](EULA.md) — free forever.

Built by [Ian Too](https://iantoo.space) · [hello@iantoo.space](mailto:hello@iantoo.space) · security reports: see [SECURITY.md](SECURITY.md)
