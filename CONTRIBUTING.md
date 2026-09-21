# Contributing to tidy-up

Thanks for helping. tidy-up moves people's files, so the bar is "boring, safe and well tested".
This guide covers how to get set up, how the project is organised, and how changes land.

## Ground rules

1. **Safety first.** A change must never make it easier to lose data. Nothing may overwrite a
   file, and every change that moves files must be journaled so `tidy-up restore` can undo it.
   The only intentionally irreversible actions are `purge` and `compare --action delete`.
2. **Every change has tests.** Bug fixes come with a test that fails without the fix.
3. **Keep it cross-platform.** tidy-up supports Windows, macOS and Linux. Avoid assumptions about
   path separators, case sensitivity or drive letters, and put OS-specific code behind `cfg`.
4. **Be gentle on the machine.** No unbounded threads or memory; prefer streaming to loading.
5. **Be kind.** Be respectful in issues and reviews. Assume good faith.

## Getting set up

You need a recent stable Rust toolchain (Rust 1.85 or newer, edition 2024).

```sh
git clone https://github.com/ianktoo/tidy-up.git
cd tidy-up
git checkout dev            # integration branch
cargo build
cargo test
```

Before you push, the same checks CI runs:

```sh
cargo fmt --all --check
cargo clippy --all-targets
cargo test
cargo doc --no-deps
```

If you change anything under `.github/`, also lint the workflows. A single YAML mistake makes GitHub
reject a whole workflow file with no useful message (an unquoted `: ` inside a step name is enough):

```sh
pip install actionlint-py && actionlint      # or: docker run --rm -v "$PWD:/repo" -w /repo rhysd/actionlint
```

## Branches and pull requests

| Branch | Purpose |
|---|---|
| `main` | Stable. Every release is tagged from here. Protected by CI. |
| `dev` | Integration branch. Feature work lands here first. |
| `feature/<name>`, `fix/<name>` | Your work, branched from `dev`. |

1. Branch from `dev`: `git checkout -b feature/my-change dev`.
2. Make focused commits with clear messages (imperative mood, e.g. "Add ratio option to distribute").
3. Open a pull request **into `dev`**. Describe what changed and why, and how you tested it.
4. CI must pass on Linux, macOS and Windows.
5. Maintainers batch `dev` into `main` with a pull request when cutting a release.

## Project map

```text
src/
  category.rs     extension table and folder names        (docs/file-types.md must match)
  rules.rs        ignore rules: extensions, globs, shortcuts
  projects.rs     code-project detection
  scan.rs         directory walk into eligible files + skipped entries
  plan.rs         organize plan: pure data, no side effects
  dedupe.rs       duplicate detection, quarantine plan, verified delete
  compare.rs      multi-folder comparison, merge planning
  analyze.rs      read-only space analysis (metadata only, bounded memory)
  distribute.rs   spreading files across destinations: capacity, ratios, fill levels
  disk.rs         partition free space, size and percent parsing
  executor.rs     performs a plan, journaling every change
  fsops.rs        safe move / unique-name helpers
  journal.rs      append-only JSON Lines undo log
  restore.rs      replays a journal backwards
  cli.rs          clap definitions (parsing only)
  commands/       command flows: glue between CLI, engine and UI
  ui.rs           colours, prompts, progress bars
tests/
  workflow.rs     end-to-end library round trips on real temp dirs
  cli.rs          black-box tests of the compiled binary
docs/             user documentation
```

The pipeline is `classify -> scan -> plan -> execute -> journal -> restore`. Plans are plain data,
which is what makes dry runs, previews and tests cheap. Read [docs/how-it-works.md](docs/how-it-works.md)
before touching the executor, journal or restore code.

## Common tasks

### Add or change a file type

1. Add the lowercase extension (no dot) to the right category in `src/category.rs`.
2. Add it to [docs/file-types.md](docs/file-types.md).
3. `cargo test`: tests fail if an extension is duplicated, not lowercase, or missing from the docs.

### Add a command

1. Define its arguments in `src/cli.rs` and a variant in `Command`.
2. Put the engine logic in its own module (pure and testable) and the terminal flow in `src/commands/`.
3. If it moves files, build a `Plan` and run it through `executor::execute` so it is journaled.
4. Add unit tests for the engine, an end-to-end test in `tests/workflow.rs` that proves
   *action then restore returns the exact original state*, and CLI tests in `tests/cli.rs`.
5. Document it in `docs/usage.md`.

### Work on distribute or analyze

Both keep their logic pure so it can be tested without real disks: `distribute::allocate` takes
partition sizes as plain data, and `disk::StaticDisks` fakes the operating system's free-space
answers. Never assert on live free-space numbers in a test; they change between two calls. Assert
on totals, volume ids and behaviour instead.

### Change the journal format

Bump the format version in `journal.rs`, keep reading old versions, and add a test that loads a
journal written by the previous version.

## How platforms are tested

You do not need three computers. Each kind of platform behaviour is covered the cheapest way that is
still honest:

| What | How | Where it runs |
|---|---|---|
| Logic that depends on partition sizes (ratios, fill levels, limits) | Pure functions plus `disk::StaticDisks`, which fakes any layout, such as three nearly full drives | Everywhere, including your laptop |
| A rename that fails between partitions | `fsops::move_path_with` takes the rename as a parameter, so a test injects the failure | Everywhere |
| Case-sensitive versus case-insensitive file systems | The test asks the file system what it does and asserts the property, not a fixed answer | Everywhere |
| Windows attributes, Unix permissions, non-UTF-8 names, dotfiles | `#[cfg(windows)]` and `#[cfg(unix)]` tests | The matching OS |
| Moving between two **real** volumes | `tests/cross_volume.rs`, which needs a folder on another volume in `TIDY_UP_OTHER_VOLUME` | CI on every OS, and locally if you have a second volume |
| The whole suite on each OS and CPU | CI matrix: Linux x64 and arm64, macOS Intel and Apple Silicon, Windows x64 and arm64, static musl, and the minimum Rust version (1.85) | CI |

To run the real cross-volume tests yourself, point the variable at any folder on a different partition,
and set the second variable to make a missing volume an error instead of a skip:

```sh
# Linux or WSL: /tmp and /dev/shm or your home directory are usually different volumes
TIDY_UP_OTHER_VOLUME="$HOME/scratch" TIDY_UP_REQUIRE_CROSS_VOLUME=1 cargo test --test cross_volume
```

```powershell
# Windows, with a second drive
$env:TIDY_UP_OTHER_VOLUME = "D:\"; $env:TIDY_UP_REQUIRE_CROSS_VOLUME = "1"; cargo test --test cross_volume
```

On Windows you can also run the whole suite on real Linux with WSL, or in a container, without any CI
round trip.

## Testing expectations

- Unit tests live next to the code they cover.
- Anything that touches the filesystem uses `tempfile` directories, never real user folders.
- Tests must pass on all three platforms. Avoid hard-coded separators (use `Path::join`), and do not
  rely on file ordering, timestamps having sub-second precision, or case-sensitive names.
- The CLI tests run the real binary with no terminal attached, so they also verify that nothing
  prompts without `--yes`.

## Style

- `cargo fmt` and a clean `cargo clippy --all-targets` (CI enforces both).
- Document every public item; explain *why*, not what the code already says.
- Errors carry the path that caused them (`IoContext::at`); user-facing messages say what happened
  and what to do next.
- Use plain punctuation in docs, comments and messages (commas, colons, periods, hyphens), and avoid
  long dashes.

## Reporting bugs and ideas

Open an issue with your OS, `tidy-up --version`, the exact command, and what you expected. For
anything that could lose data or escape the target folder, **do not open a public issue**; follow
[SECURITY.md](SECURITY.md).

## Releasing (maintainers)

1. On `dev`, bump `version` in `Cargo.toml` (and let `Cargo.lock` update), and update docs if needed.
2. Open a pull request `dev` into `main` and wait for CI to pass.
3. After merging, tag the merge commit and push the tag:

   ```sh
   git checkout main && git pull
   git tag -a v0.2.0 -m "v0.2.0"
   git push origin v0.2.0
   ```

4. The **Release** workflow verifies that the tag matches `Cargo.toml`, runs the tests on all three
   platforms, builds binaries for Windows (x64, arm64), macOS (Intel, Apple Silicon) and Linux
   (x64, x64 static, arm64), and attaches them with `SHA256SUMS.txt` to a GitHub release. Creating a
   release with a new tag in the GitHub UI has the same effect.

Every push to `main` also builds all platforms and keeps the binaries as workflow artifacts for 14 days.

## Licensing of contributions

By contributing you agree that your contribution is licensed under the [MIT License](LICENSE), the
same as the rest of the source. Official binaries are additionally covered by the [EULA](EULA.md).
