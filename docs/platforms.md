# Platforms and installing

tidy-up runs on **Windows, macOS and Linux**. The test suite runs on all three in CI on every
change, and release binaries are built for each.

| Platform | Architectures | Release archive |
|---|---|---|
| Windows 10 / 11 | x64, arm64 | `tidy-up-<version>-x86_64-pc-windows-msvc.zip`, `...aarch64-pc-windows-msvc.zip` |
| macOS 11+ | Intel, Apple Silicon | `...x86_64-apple-darwin.tar.gz`, `...aarch64-apple-darwin.tar.gz` |
| Linux | x64 (glibc), x64 (static musl), arm64 | `...x86_64-unknown-linux-gnu.tar.gz`, `...x86_64-unknown-linux-musl.tar.gz`, `...aarch64-unknown-linux-gnu.tar.gz` |

The static musl build has no dependencies and runs on any x64 Linux, including minimal containers.

## Installing from a release

1. Download the archive for your platform from the project's **Releases** page, along with
   `SHA256SUMS.txt`.
2. Verify it:

   ```sh
   sha256sum -c SHA256SUMS.txt --ignore-missing      # Linux
   shasum -a 256 -c SHA256SUMS.txt --ignore-missing  # macOS
   Get-FileHash .\tidy-up-*.zip -Algorithm SHA256     # Windows PowerShell, compare by eye
   ```

3. Extract it and put the `tidy-up` executable in a folder that is on your `PATH`, so you can run
   `tidy-up` from any terminal. Step by step instructions for every platform are in
   [Adding tidy-up to your PATH](#adding-tidy-up-to-your-path) below.

4. Check it: `tidy-up --version`.

**macOS:** the binaries are not notarized. If Gatekeeper blocks the first run, clear the quarantine
flag once: `xattr -d com.apple.quarantine /usr/local/bin/tidy-up`.
**Windows:** SmartScreen may warn about an unrecognised app; choose "More info", then "Run anyway".

## Adding tidy-up to your PATH

The archive contains a single executable. To run `tidy-up` from any folder, keep it in a directory
that is on your `PATH`. These steps install for **your user only** and need no administrator rights.
Replace the version and target in the file names with the ones you downloaded.

### Windows (PowerShell)

```powershell
$dir = "$env:LOCALAPPDATA\Programs\tidy-up"
New-Item -ItemType Directory -Force $dir | Out-Null
Expand-Archive .\tidy-up-0.1.1-x86_64-pc-windows-msvc.zip -DestinationPath "$env:TEMP\tidy-up-x" -Force
Copy-Item "$env:TEMP\tidy-up-x\tidy-up-*\tidy-up.exe" $dir -Force

# add the folder to your user PATH once
$user = [Environment]::GetEnvironmentVariable("Path", "User")
if (($user -split ';') -notcontains $dir) {
    [Environment]::SetEnvironmentVariable("Path", "$user;$dir".TrimStart(';'), "User")
}
```

Then **open a new terminal** (an already open one keeps the old PATH) and run `tidy-up --version`.

Prefer the settings dialog? Press `Win+R`, run `sysdm.cpl`, choose **Advanced**, then **Environment
Variables**, select **Path** under your user variables, click **Edit**, then **New**, and add the folder.
Avoid the `setx PATH` command: it silently truncates long PATH values.

### macOS and Linux

```sh
mkdir -p ~/.local/bin
tar xzf tidy-up-0.1.1-x86_64-unknown-linux-gnu.tar.gz        # use the file you downloaded
install -m 755 tidy-up-0.1.1-*/tidy-up ~/.local/bin/tidy-up
```

If `~/.local/bin` is not already on your PATH (check with `echo "$PATH"`), add it for your shell:

```sh
# zsh (the default on macOS)
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
# bash (Linux: ~/.bashrc, macOS: ~/.bash_profile)
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
# fish
fish_add_path ~/.local/bin
```

Then open a new terminal, or run `source ~/.zshrc` (or your shell's file).

To make it available to **every user** instead, install into a system folder that is already on the PATH:

```sh
sudo install -m 755 tidy-up-0.1.1-*/tidy-up /usr/local/bin/tidy-up
```

### Check it worked

```sh
tidy-up --version          # prints: tidy-up 0.1.1
command -v tidy-up         # macOS and Linux: where it was found
Get-Command tidy-up        # Windows PowerShell
```

If you see `command not found` or `not recognized`: open a **new** terminal, confirm the folder really
contains the executable, and confirm it is listed in `echo "$PATH"` (or `$env:Path` on Windows).

## Upgrading

Nothing happens automatically: tidy-up never checks for updates and never uses the network. To upgrade,
install the new version over the old one.

1. Read the release notes on the [Releases page](https://github.com/ianktoo/tidy-up/releases). To be told
   about new versions, use **Watch, Custom, Releases** on the GitHub repository.
2. Download the archive for your platform and verify it against `SHA256SUMS.txt`.
3. Replace the executable in the same folder you installed it to, using the same steps as above. The PATH
   entry is already there, so there is nothing more to configure. On Windows, close any running `tidy-up`
   first, because a running program cannot be overwritten.
4. Check it: `tidy-up --version`.

If you installed with Cargo, run `cargo install --git https://github.com/ianktoo/tidy-up --locked --force`
(or `cargo install --path . --force` from an updated checkout).

**Is it safe to upgrade?** tidy-up keeps no settings, cache or database outside the folders it processed,
so there is nothing to migrate. The undo journals are versioned:

- A newer tidy-up reads every older journal, so you can still `restore` runs made by earlier versions.
- An older tidy-up that meets a journal written by a newer version stops with a clear message
  (`written by a newer tidy-up`) instead of guessing. If you ever downgrade, upgrade again before restoring.
- While the version starts with `0.`, command options can change between minor versions. The release notes
  list any change, and `tidy-up --help` always shows what your installed version accepts.

Several versions can live side by side in different folders, which is handy for trying a new release.

## Uninstalling

The program is one executable. Beyond that, tidy-up only ever leaves these behind:

| What | Where | Notes |
|---|---|---|
| The executable | Where you put it | Delete it |
| `.tidy-up/` folders | Inside each folder you processed (the first folder for `compare`, the first destination for `distribute`) | Undo journals and duplicate reports. Small. Deleting one only removes the ability to undo that folder's runs |
| Folders created by runs | Inside processed folders: `Images/`, `Documents/`, `Projects/`, `_Duplicates/`, ... | These hold **your files**. `restore` puts them back and removes the empty folders |

It does **not** write to the registry, to AppData or `~/.config`, to a cache, or to any service or
scheduled task, so there is nothing hidden to clean up.

### Step by step

1. **Undo what you want undone first.** Journals are what make undo possible, so do this before deleting
   anything else.

   ```sh
   tidy-up history "C:\path\to\folder"           # what was done here
   tidy-up restore "C:\path\to\folder" --all     # put everything back, newest run first
   tidy-up purge   "C:\path\to\folder"           # or delete quarantined duplicates for good
   ```

2. **Remove the program.**

   ```powershell
   Remove-Item "$env:LOCALAPPDATA\Programs\tidy-up" -Recurse          # Windows
   ```

   ```sh
   rm ~/.local/bin/tidy-up          # macOS and Linux (or /usr/local/bin/tidy-up with sudo)
   cargo uninstall tidy-up          # if you installed with Cargo
   ```

3. **Remove the PATH entry** (only if you added it just for tidy-up).

   ```powershell
   $dir = "$env:LOCALAPPDATA\Programs\tidy-up"
   $user = [Environment]::GetEnvironmentVariable("Path", "User")
   [Environment]::SetEnvironmentVariable("Path", (($user -split ';') | Where-Object { $_ -and $_ -ne $dir }) -join ';', "User")
   ```

   On macOS and Linux, delete the `export PATH=...` line you added to your shell file. Leave it if other
   programs also install into `~/.local/bin`, which is common.

4. **Optional: remove the leftover state folders.** Do this last, and only if you no longer want to undo
   anything. List them first, then delete:

   ```powershell
   Get-ChildItem $HOME -Recurse -Directory -Force -Filter .tidy-up -ErrorAction SilentlyContinue | Select-Object FullName
   # then, once you have reviewed the list:
   Get-ChildItem $HOME -Recurse -Directory -Force -Filter .tidy-up -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force
   ```

   ```sh
   find ~ -type d -name .tidy-up -prune -print                        # review the list
   find ~ -type d -name .tidy-up -prune -exec rm -rf {} +             # then delete
   ```

   Your files are not touched by this. Only the ability to undo those runs goes away.

## Building from source

Any platform with Rust 1.85 or newer:

```sh
cargo install --path .
```

## How each platform is handled

| Topic | Windows | macOS | Linux |
|---|---|---|---|
| Hidden files | Names starting with `.`, plus the HIDDEN and SYSTEM attributes | Names starting with `.` | Names starting with `.` |
| Shortcuts skipped | `lnk`, `url` | `webloc` | `desktop` |
| OS files skipped | `desktop.ini`, `Thumbs.db` | `.DS_Store` | (none) |
| Case sensitivity | Insensitive: `A.TXT` and `a.txt` clash | Insensitive by default | Sensitive |
| Which partition | Drive letter | Device number | Device number |
| Free space | Available to your user, from the volume | Same | Same |
| Symbolic links | Never followed or moved | Same | Same |

Name clashes are handled by content and by unique names on every platform, so behaviour is the same
whether the file system is case sensitive or not.

### Details worth knowing

- **Paths** are shown with your platform's separator. Undo journals store paths relative to the folder
  you processed, so a folder (or external drive) that is re-lettered or mounted elsewhere keeps a valid
  history. Paths outside that folder, used when several folders are compared or distributed, are stored
  absolute.
- **Moving between partitions** is a copy, a length check, then a delete. The modified time and
  permissions are preserved; ownership and some extended attributes may not carry over.
- **Network and removable drives** work, but hashing and copying run at the speed of the link, and a
  network share reports the share's free space.
- **Windows partition detection** uses the drive letter, so a folder mounted from another volume into a
  drive is treated as that drive. If it matters, point `distribute` at the mounted folder's real drive.
- **File names that are not valid UTF-8** (possible on Linux) are skipped, because they cannot be
  recorded faithfully in the undo journal.
- **Terminals:** colours and progress bars are used when attached to a terminal and disabled when output
  is piped. `NO_COLOR=1` turns colours off.
