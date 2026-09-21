# Supported file types

tidy-up sorts files by **extension**. The table below is the complete list; it is the same table the program uses (`src/category.rs`), and a test fails if this page and the code ever disagree.

**174 extensions in 15 categories**, plus `Other` for everything else.

## How classification works

- Matching is **case-insensitive**: `HOLIDAY.JPG` and `holiday.jpg` are both images.
- Only the **last** extension counts: `backup.tar.gz` is a `gz` file, so it goes to Archives.
- File contents are never inspected. A PNG renamed to `.txt` is a text file to tidy-up.
- Files with **no extension**, dotfiles such as `.gitignore`, and unknown extensions go to `Other`.
- Every extension belongs to exactly one category (a test enforces this). Ambiguous ones are resolved by the most common use, for example `ts` is Code, not Video.

## Categories

| Folder | What it holds | Extensions |
|---|---|---|
| `Images/` | Photos, raster and vector pictures, camera RAW | `jpg` `jpeg` `png` `gif` `bmp` `webp` `tiff` `tif` `heic` `heif` `svg` `ico` `raw` `cr2` `nef` `arw` `dng` `avif` |
| `Videos/` | Video clips and movies | `mp4` `mkv` `mov` `avi` `wmv` `flv` `webm` `m4v` `mpg` `mpeg` `3gp` |
| `Audio/` | Music, recordings, samples, MIDI | `mp3` `wav` `flac` `aac` `ogg` `m4a` `wma` `aiff` `opus` `mid` `midi` |
| `Documents/` | PDFs and word-processor documents | `pdf` `doc` `docx` `odt` `rtf` `pages` `tex` |
| `Text Files/` | Plain text, notes, logs | `txt` `md` `markdown` `log` `rst` `nfo` |
| `Spreadsheets/` | Spreadsheets and delimited tables | `xls` `xlsx` `ods` `csv` `tsv` `numbers` |
| `Presentations/` | Slide decks | `ppt` `pptx` `odp` `key` |
| `Ebooks/` | E-books | `epub` `mobi` `azw` `azw3` `djvu` |
| `Archives/` | Compressed archives | `zip` `rar` `7z` `tar` `gz` `bz2` `xz` `tgz` `zst` `cab` |
| `Code/` | Source files, scripts, web files, notebooks | `rs` `py` `js` `mjs` `ts` `tsx` `jsx` `java` `c` `h` `cpp` `hpp` `cc` `cs` `go` `rb` `php` `swift` `kt` `kts` `sh` `bash` `bat` `cmd` `ps1` `html` `htm` `css` `scss` `sass` `lua` `dart` `scala` `r` `vue` `ipynb` |
| `3D Models/` | 3D models and scenes (printing, CAD, DCC) | `stl` `obj` `fbx` `blend` `gltf` `glb` `3mf` `dae` `ply` `step` `stp` `3ds` `skp` `iges` `igs` `usdz` `max` `c4d` `ma` `mb` |
| `Design/` | Design-tool source files | `psd` `ai` `xd` `fig` `sketch` `indd` `eps` `afdesign` `afphoto` `kra` `xcf` |
| `Fonts/` | Font files | `ttf` `otf` `woff` `woff2` `fon` |
| `Installers/` | Installers, packages, disk images | `exe` `msi` `dmg` `pkg` `deb` `rpm` `apk` `appimage` `iso` `img` `msix` |
| `Data/` | Structured data, configs, databases | `json` `xml` `yaml` `yml` `toml` `ini` `sql` `db` `sqlite` `sqlite3` `dat` `conf` `cfg` |
| `Other/` | Anything not listed above, including files with no extension | (any) |

## Things that are never sorted like files

**Code projects.** A folder that directly contains any of these is treated as a project and kept intact:

`.git` `.hg` `.svn` `Cargo.toml` `package.json` `pyproject.toml` `setup.py` `go.mod` `pom.xml` `build.gradle` `build.gradle.kts` `CMakeLists.txt` `composer.json` `Gemfile` `pubspec.yaml` `mix.exs` `deno.json` `Package.swift` `project.godot`, or any `*.sln` `*.csproj` `*.fsproj` `*.vcxproj` `*.uproject` `*.xcodeproj`.

**Skipped by default** (opt in with `--include-shortcuts` / `--include-hidden`):

- Shortcuts: `lnk` `url` `webloc` `desktop`
- Hidden files and folders: names starting with `.`, and on Windows anything with the HIDDEN or SYSTEM attribute

**Always skipped:** OS bookkeeping files (`desktop.ini`, `Thumbs.db`, `ehthumbs.db`, `.DS_Store`), Office lock files (`~$*`), symbolic links, the `.tidy-up/` state folder, and names that are not valid UTF-8.

## Adding or changing a type

1. Add the lowercase extension (no dot) to the right category in `src/category.rs`.
2. Add it to this page in the same category row.
3. Run `cargo test`. The tests fail if an extension is listed twice, is not lowercase, or is missing from this page.

See [CONTRIBUTING.md](../CONTRIBUTING.md).
