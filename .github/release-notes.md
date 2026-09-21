## Install

Download the archive for your platform below, check it against `SHA256SUMS.txt`, extract it, and put `tidy-up` in a folder on your PATH.

| Platform | Archive |
|---|---|
| Windows x64 / arm64 | `tidy-up-VERSION-x86_64-pc-windows-msvc.zip` / `...aarch64-pc-windows-msvc.zip` |
| macOS Apple Silicon / Intel | `tidy-up-VERSION-aarch64-apple-darwin.tar.gz` / `...x86_64-apple-darwin.tar.gz` |
| Linux x64 / static x64 / arm64 | `tidy-up-VERSION-x86_64-unknown-linux-gnu.tar.gz` / `...x86_64-unknown-linux-musl.tar.gz` / `...aarch64-unknown-linux-gnu.tar.gz` |

**Add to PATH: extract the archive and run the included script from that folder.** It installs for your user only, needs no admin rights, and can be undone.

```powershell
# Windows (PowerShell)
powershell -ExecutionPolicy Bypass -File .\scripts\add-to-path.ps1
```

```sh
# macOS and Linux
sh scripts/add-to-path.sh
```

To undo it, run `scripts\remove-from-path.ps1` or `scripts/remove-from-path.sh` the same way. Both scripts take a preview flag (`-WhatIf` / `--dry-run`).

Open a **new terminal** and run `tidy-up --version`.

**Upgrading:** replace the executable with the new one, nothing else to do. tidy-up never checks for updates. Undo journals from older versions keep working.
**Uninstalling:** delete the executable and the PATH entry. tidy-up keeps nothing outside the folders it processed (small `.tidy-up/` folders you can delete after undoing runs you care about).

Full details for every platform: [Platforms, PATH, upgrading and uninstalling](https://github.com/ianktoo/tidy-up/blob/main/docs/platforms.md). macOS and Windows may warn about an unsigned app on first run; the page explains how to continue.

---

