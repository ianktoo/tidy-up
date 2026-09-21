## Install

Download the archive for your platform below, check it against `SHA256SUMS.txt`, extract it, and put `tidy-up` in a folder on your PATH.

| Platform | Archive |
|---|---|
| Windows x64 / arm64 | `tidy-up-VERSION-x86_64-pc-windows-msvc.zip` / `...aarch64-pc-windows-msvc.zip` |
| macOS Apple Silicon / Intel | `tidy-up-VERSION-aarch64-apple-darwin.tar.gz` / `...x86_64-apple-darwin.tar.gz` |
| Linux x64 / static x64 / arm64 | `tidy-up-VERSION-x86_64-unknown-linux-gnu.tar.gz` / `...x86_64-unknown-linux-musl.tar.gz` / `...aarch64-unknown-linux-gnu.tar.gz` |

**Add to PATH, Windows (PowerShell):**

```powershell
$dir = "$env:LOCALAPPDATA\Programs\tidy-up"; New-Item -ItemType Directory -Force $dir | Out-Null
Expand-Archive .\tidy-up-VERSION-x86_64-pc-windows-msvc.zip -DestinationPath "$env:TEMP\tidy-up-x" -Force
Copy-Item "$env:TEMP\tidy-up-x\tidy-up-*\tidy-up.exe" $dir -Force
$user = [Environment]::GetEnvironmentVariable("Path", "User")
if (($user -split ';') -notcontains $dir) { [Environment]::SetEnvironmentVariable("Path", "$user;$dir".TrimStart(';'), "User") }
```

**Add to PATH, macOS and Linux:**

```sh
mkdir -p ~/.local/bin && tar xzf tidy-up-VERSION-*.tar.gz
install -m 755 tidy-up-VERSION-*/tidy-up ~/.local/bin/tidy-up
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc   # or ~/.bashrc; skip if already on PATH
```

Open a **new terminal** and run `tidy-up --version`.

**Upgrading:** replace the executable with the new one, nothing else to do. tidy-up never checks for updates. Undo journals from older versions keep working.
**Uninstalling:** delete the executable and the PATH entry. tidy-up keeps nothing outside the folders it processed (small `.tidy-up/` folders you can delete after undoing runs you care about).

Full details for every platform: [Platforms, PATH, upgrading and uninstalling](https://github.com/ianktoo/tidy-up/blob/main/docs/platforms.md). macOS and Windows may warn about an unsigned app on first run; the page explains how to continue.

---

