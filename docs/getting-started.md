# Getting started

## Install

Requires a recent stable Rust toolchain (edition 2024, Rust 1.85+).

```sh
git clone https://github.com/ianktoo/tidy-up.git tidy-up && cd tidy-up
cargo install --path .
tidy-up --version
```

Or build without installing: `cargo build --release` (binary at `target/release/tidy-up`).

## Your first run

Always preview first:

```sh
tidy-up organize ~/Downloads --dry-run
```

You'll see a per-folder summary, a sample of planned moves, and a list of what was
left alone and why. Nothing is changed and no state folder is created.

When it looks right:

```sh
tidy-up organize ~/Downloads
```

Confirm the prompt. The output ends with the run id and the exact undo command.

Changed your mind?

```sh
tidy-up restore ~/Downloads
```

## Interactive mode

Run `tidy-up` with no arguments. It offers your Downloads, Desktop and Documents
folders (or a typed path), then a menu: organize, find duplicates, restore,
history, purge. It asks about ignored extensions, shortcuts, subfolders and
projects as it goes.

## Recipes

**Desktop, but leave shortcuts and my installers alone** (shortcuts are skipped by default):

```sh
tidy-up organize ~/Desktop -x exe,msi
```

**Sort a whole drive, including subfolders, keeping repos intact:**

```sh
tidy-up organize D:\ --depth 6 --dry-run
```

**A reusable ignore list:**

```text
# ~/.tidy-ignore
.iso
*.tmp
thesis-final.docx
```

```sh
tidy-up organize ~/Downloads -f ~/.tidy-ignore
```

**Free up space safely:**

```sh
tidy-up dedupe ~/Downloads       # isolates extra copies into _Duplicates/
# ...look around, decide...
tidy-up purge ~/Downloads        # permanently deletes them (asks first)
```

**Gather all repos into one folder:**

```sh
tidy-up organize ~/Downloads --projects move
```

**Use in scripts** (no terminal, so confirmation must be explicit):

```sh
tidy-up organize ~/Downloads --yes
```

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `cannot ask "..." without a terminal; re-run with --yes` | stdin isn't a TTY; pass `--yes` |
| A file wasn't moved | Check the "Left alone" section (or `--verbose`): shortcut, hidden, ignored, project, or beyond `--depth` |
| Restore says "missing (deleted or purged)" | The file was deleted after the run; nothing to put back |
| Restore renamed a file to `name (1).ext` | Something new now occupies the original name; use `--on-conflict skip` to leave it instead |
| Second `organize` run does nothing | Category folders at the root are skipped on purpose, so runs are idempotent |
