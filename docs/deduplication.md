# How deduplication works

This page explains exactly what tidy-up treats as a duplicate, how it finds them without
reading your whole disk, which copy it keeps, and what it does (and does not do) with the rest.
It applies to both `tidy-up dedupe` (one folder) and `tidy-up compare` (several folders).

## What counts as a duplicate

Two files are duplicates when their **bytes are identical**. Nothing else matters:

| Differs | Still a duplicate? |
|---|---|
| File name (`IMG_001.jpg` vs `copy.jpg`) | Yes |
| Folder or drive | Yes |
| Modified or created date | Yes |
| One byte of content | **No** |
| Same picture, re-saved or re-compressed | **No** (different bytes; tidy-up does no visual matching) |
| Same name and size, different content | **No** |
| Empty files | Never (they are ignored, not reported) |
| Symbolic links | Never (not followed, not moved) |

If you need near-duplicate or "looks similar" detection for photos, this is not that tool.

## The pipeline

```text
   every file in scope
          |
          v
 1. group by SIZE ------------- a file with a unique size cannot have a twin: dropped, never read
          |
          v
 2. QUICK hash of first 4 KiB -- BLAKE3 of the first 4096 bytes: drops most same-size strangers
          |
          v
 3. FULL hash of whole file ---- BLAKE3 of every byte, only for files that survived stages 1 and 2
          |
          v
   groups of identical files
```

Each stage exists to avoid work in the next one:

1. **Size.** The scan already knows every file's size for free. Files whose size is unique on
   disk cannot be duplicates and are never opened. On a typical folder this removes the large
   majority of files.
2. **Quick hash.** Two files of equal size usually differ near the start (headers, metadata).
   Reading one 4 KiB block is cheap, and it eliminates most look-alikes without reading a
   multi-gigabyte file to the end.
3. **Full hash.** Only files that still collide are read in full. A group is only reported
   when the whole-file hashes match.

### Why a hash instead of comparing bytes

The hash is BLAKE3 (256-bit), a fast cryptographic hash. The chance that two different files
share a hash by accident is astronomically small (on the order of 2^-128 for random data), far
below the chance of a disk error. tidy-up does not run a final byte-for-byte comparison,
because that would mean reading both files a second time. Instead, the one irreversible action,
`--action delete`, **re-hashes both the copy and the kept file immediately before removing
anything** and skips the file if either changed since the scan.

## Which copy is kept

Inside each group, the copies are ranked and the first one is the **keeper**:

1. the earliest-listed folder (only matters for `compare`: the first folder you list wins),
2. then the **shallowest** path (fewer folders deep),
3. then the **oldest** modification time,
4. then path order, so the result is always deterministic.

Example, one folder:

```text
Group-001  3 copies x 4.0 MiB
  keep   Photos/beach.jpg              (depth 2)
  extra  Photos/Old/beach.jpg          (depth 3)
  extra  Downloads/beach (1).jpg       (depth 2, newer)
```

Example, `tidy-up compare D:\Photos E:\Backup`: the copy on `D:` is kept even if the one on `E:`
is older, because `D:\Photos` was listed first.

## What happens to the extra copies

Nothing is deleted by default. The options, from safest to most final:

| Command | Result | Undoable |
|---|---|---|
| `tidy-up dedupe` | Extras move to `_Duplicates/Group-NNN/`; a report is written to `.tidy-up/reports/` | `tidy-up restore` |
| `tidy-up compare ... --action move` | Same, across several folders; extras go into the first folder's `_Duplicates/` | `tidy-up restore` |
| `tidy-up compare ... --action merge` | Extras aside as above, and every file that exists only outside the first folder is gathered into it | `tidy-up restore` |
| `tidy-up purge` | Permanently deletes `_Duplicates/` | No |
| `tidy-up compare ... --action delete` | Permanently deletes the extras, after re-verifying each | No |

Name clashes inside a group folder become `name (1).ext`; nothing is overwritten.

## Scope: what is searched

- Files inside **code projects** (folders with `.git`, `Cargo.toml`, `package.json`, ...) are
  not searched. Vendored dependencies and build output would otherwise drown the results.
- Shortcuts, hidden files and system files are skipped unless you pass `--include-shortcuts` /
  `--include-hidden`.
- Use `-x`, `-i` or `-f` to exclude extensions, names or globs, and `--depth N` to limit how far
  down to look.
- A folder named `_Duplicates` at the top of a scanned folder is skipped, so isolating duplicates
  and running again never re-flags the isolated copies.

## Resource use

- **CPU:** hashing runs on at most `min(4, cores / 2)` threads so your machine stays responsive.
- **Disk:** only files that survive the size and quick-hash stages are read in full. Metadata is
  read once during the scan.
- **Memory:** one small record per file, plus hashes for the candidate files only. It does not
  load file contents; hashing streams through a small buffer.
- **Progress:** a byte-based progress bar restarts for each hashing pass and shows throughput and
  ETA (it is hidden when output is not a terminal).

## Limitations worth knowing

- **Hard links.** Two paths to the *same* file on disk have identical bytes, so they are reported
  as duplicates, but deleting one frees **no space**. The "reclaimable" figure can therefore be
  optimistic on folders with many hard links (some backup and package tools create them).
- **Files that change during a scan** can be skipped or mis-grouped. Run it when nothing is
  writing to the folder. The delete action's re-verification protects against acting on stale results.
- **Unreadable files** (permissions, in use) are reported and excluded, not treated as unique or as duplicates.
- **Case-insensitive file systems** (Windows, default macOS) are handled by content, not name, so
  `A.TXT` and `a.txt` are only duplicates if their bytes match.
- **Different formats of the same content** (a `.png` and the `.jpg` made from it) are different
  files. Only bit-identical copies match.

## Checking a result yourself

Any BLAKE3 tool will agree with tidy-up:

```sh
b3sum "D:\Photos\beach.jpg" "E:\Backup\IMG_001.jpg"     # identical hashes = identical content
```

Or use your platform's checksum tool (`sha256sum`, `shasum -a 256`, `Get-FileHash`) and compare.

## For developers

The code lives in `src/dedupe.rs` (pipeline, keeper choice, quarantine plan, verified delete) and
`src/compare.rs` (multi-folder analysis and merge planning). Progress is reported through the
`HashProgress` trait so any front end can render it. The hashing stages are unit-tested for
same-size-different-content files, identical prefixes with different tails, empty files, unreadable
files, keeper ordering and group ordering. See [How it works](how-it-works.md) for the journal and
undo design.
