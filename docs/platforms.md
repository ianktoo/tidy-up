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

3. Extract it and put the `tidy-up` executable somewhere on your `PATH`.

   ```sh
   tar xzf tidy-up-0.2.0-x86_64-unknown-linux-gnu.tar.gz
   sudo install tidy-up-0.2.0-x86_64-unknown-linux-gnu/tidy-up /usr/local/bin/
   ```

4. Check it: `tidy-up --version`.

**macOS:** the binaries are not notarized. If Gatekeeper blocks the first run, clear the quarantine
flag once: `xattr -d com.apple.quarantine /usr/local/bin/tidy-up`.
**Windows:** SmartScreen may warn about an unrecognised app; choose "More info", then "Run anyway".

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
