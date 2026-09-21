# Distributing files across partitions

`tidy-up distribute` moves data out of full folders or drives into other places, and lets you
control **how much** moves, **how it is split**, and **how full** each destination may get. It is
built for the situation where you have two or three partitions that are all nearly full.

```sh
tidy-up distribute --from D:\Media --to E:\Archive F:\Archive --ratio 60,40 --limit 200GiB --max-fill 90
```

A single destination is a limited merge ("move up to 200 GiB from here to there"). Several
destinations are a distribution. Everything is previewed first, journaled, and undoable with
`tidy-up restore`.

## The three questions it answers

### 1. How much moves? `--limit`

`--limit 200GiB` moves at most that much in total (units are binary: 1 GB = 1 GiB). Without it,
everything that fits is moved. When the limit applies, `--prefer` decides what goes first:

| `--prefer` | Picks | Good for |
|---|---|---|
| `largest` (default) | Biggest items first | Freeing the most space with the fewest moves |
| `oldest` | Items not touched for longest | Archiving what you no longer use |

An item that would push the total over the limit is skipped and the search continues with smaller
ones, so the limit is filled as fully as possible.

### 2. How is it split? `--ratio` or `--strategy`

| Option | Meaning |
|---|---|
| `--ratio 50,30,20` | Split the moved data 50%, 30%, 20% (one number per destination, any scale) |
| `--strategy fill` (default) | Equalise how full the destinations end up, by percentage, filling the emptiest first |
| `--strategy free` | In proportion to each destination's free space |
| `--strategy even` | Equal shares |

The ratio applies to the data **being moved**, not to what is already there. A destination with
weight `0` receives nothing. If a destination cannot take its share (it is nearly full), the rest
is redistributed among the others, keeping their proportions.

Worked example of `fill`: destination A is 50% full, B is 10% full, both 1000 GiB. Moving 800 GiB
puts 200 GiB on A and 600 GiB on B, so **both end at 70%**. Moving only 300 GiB sends all of it to
B (it ends at 40%, A stays at 50%) because that is what keeps them closest to level.

### 3. How full may a destination get? `--max-fill` and `--min-free`

| Option | Default | Meaning |
|---|---|---|
| `--max-fill PERCENT` | `90` | Never fill a destination's partition beyond this |
| `--min-free SIZE` | `0` | Always keep at least this much free (the larger of the two limits wins) |

The room a destination offers is `free space - reserve`, where the reserve is the larger of
`min-free` and the space needed to stay under `max-fill`. A destination that is already past its
limit receives nothing, and the report says so.

## What is moved: units

Data is assigned to destinations in **units**, so related files stay together.

| `--granularity` | Unit | Effect |
|---|---|---|
| `item` (default) | Each top-level file or folder of a source | A folder (an album, a code project) moves whole and is never split across destinations. Hidden files and `.git` go with it |
| `file` | Every individual file | Best packing, but folders may be split; code projects are left alone |

A folder that contains a symbolic link, an unreadable entry, or a name that is not valid UTF-8 is
skipped entirely rather than half-moved.

## Where it lands: `--layout`

| `--layout` | Result |
|---|---|
| `keep` (default) | The original structure: `E:\Archive\Photos\...`. A name clash becomes `name (1).ext` |
| `organize` | Sorted into category folders (`Images/`, `Documents/`, `3D Models/`, ...). Works file by file |

This is how you combine distributing with organizing in one step.

## Several partitions, one pool

If two destinations live on the **same partition**, they share that partition's free space (a
"pool"), so the tool cannot promise the same gigabytes twice. On Linux and macOS a partition is
identified by its device number; on Windows by its drive letter (a folder mounted from another
volume into a drive is treated as that drive).

## Safety

- **Preview and confirm.** You see each destination's fill level before and after, and what each
  source partition gets back. `--dry-run` shows it without changing anything.
- **Never overwrites.** Clashes are renamed.
- **Copy, verify, then delete.** Between partitions a move is a copy that is length-checked before
  the original is removed, and modified times are preserved. Only one file at a time is in flight, so
  a failure never leaves a file missing.
- **Journaled.** The undo journal lives in the **first destination**. `tidy-up restore <first
  destination>` moves every file back to its original place, even across partitions, and removes the
  folders the run created if they are empty.
- **Empty source folders are left behind** (they are not part of the journal's undo); delete them
  yourself if you no longer need them.
- **Space you get back** only counts data that actually leaves the source's partition. Moving to a
  folder on the same partition frees nothing, and the report says so.

## Examples

Free a nearly-full drive by archiving its oldest 100 GiB to two other drives, evenly filled:

```sh
tidy-up distribute --from D:\ --to E:\Archive F:\Archive --limit 100GiB --prefer oldest --strategy fill
```

Split a media library 50/30/20 across three disks, never above 85% full and keeping 50 GiB free:

```sh
tidy-up distribute --from D:\Media --to E:\M F:\M G:\M --ratio 50,30,20 --max-fill 85 --min-free 50GiB
```

Merge two folders into one destination and sort the result:

```sh
tidy-up distribute --from ~/Downloads ~/Desktop --to /mnt/big/inbox --layout organize
```

Look before you leap:

```sh
tidy-up analyze D:\ E:\ F:\                    # which partitions are full, and with what
tidy-up distribute --from D:\ --to E:\ F:\ -n  # preview the plan
```

## Options at a glance

| Option | Default | |
|---|---|---|
| `--from FOLDER...` | required | Sources to move files out of |
| `--to FOLDER...` | required | Destinations (created if missing) |
| `--ratio N,N,...` | | Explicit split, one number per destination |
| `--strategy fill\|free\|even` | `fill` | Split rule when no ratio is given |
| `--limit SIZE` | none | Move at most this much |
| `--max-fill PERCENT` | `90` | Never fill a destination beyond this |
| `--min-free SIZE` | `0` | Always keep this much free |
| `--prefer largest\|oldest` | `largest` | What to move first under a limit |
| `--layout keep\|organize` | `keep` | Where files land |
| `--granularity item\|file` | `item` | Unit of moving |
| `-x`, `-i`, `-f`, `--include-shortcuts`, `--include-hidden` | | Ignore rules for top-level entries (same as `organize`) |
| `-n`, `-y`, `-v` | | Dry run, skip confirmation, verbose |

Sources and destinations must be separate: no folder may be inside another, or the same one.

## For developers

`src/distribute.rs` is deliberately split so the interesting part is pure. `collect_units` reads the
sources, `allocate` decides who gets what from plain numbers (no disk access), and `build_plan`
produces a normal `Plan` that the shared executor runs and journals. Partition space comes through the
`DiskProbe` trait, so tests use `StaticDisks` to simulate any combination of full drives.
