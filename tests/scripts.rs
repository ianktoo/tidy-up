//! Tests for the install helper scripts in `scripts/`.
//!
//! Unix: the `.sh` scripts run under `sh` (dash on Debian and Ubuntu, bash 3.2 in POSIX mode on
//! macOS), in a throw-away `$HOME`, so the real shell files are never touched.
//! Windows: the `.ps1` scripts run in Windows PowerShell against a scratch registry key instead of
//! the real user PATH.

use std::path::{Path, PathBuf};

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts")
}

#[cfg(unix)]
mod unix {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        process::{Command, Output},
    };

    use tempfile::TempDir;

    use super::scripts_dir;

    /// A fake user: an empty home directory and a stand-in tidy-up executable.
    struct Sandbox {
        home: TempDir,
        binary: PathBuf,
    }

    fn sandbox() -> Sandbox {
        let home = tempfile::tempdir().unwrap();
        let binary = home.path().join("download").join("tidy-up");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, "#!/bin/sh\necho tidy-up 9.9.9\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        Sandbox { home, binary }
    }

    impl Sandbox {
        fn path(&self) -> &Path {
            self.home.path()
        }

        fn run(&self, script: &str, shell: &str, path_env: &str, args: &[&str]) -> Output {
            Command::new("sh")
                .arg(scripts_dir().join(script))
                .args(args)
                .env_clear()
                .env("HOME", self.path())
                .env("SHELL", shell)
                .env("PATH", path_env)
                .env("TMPDIR", self.path())
                .output()
                .expect("sh runs")
        }

        /// `add-to-path.sh` with a minimal PATH (tidy-up is not on it yet).
        fn add(&self, shell: &str, extra: &[&str]) -> Output {
            let binary = self.binary.to_str().unwrap();
            let mut args = vec!["--binary", binary];
            args.extend_from_slice(extra);
            self.run("add-to-path.sh", shell, "/usr/bin:/bin", &args)
        }

        fn remove(&self, extra: &[&str]) -> Output {
            self.run("remove-from-path.sh", "/bin/sh", "/usr/bin:/bin", extra)
        }

        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.path().join(rel)).unwrap_or_default()
        }

        fn installed(&self) -> PathBuf {
            self.path().join(".local/bin/tidy-up")
        }
    }

    fn ok(out: &Output) -> String {
        assert!(
            out.status.success(),
            "exit {:?}\nstdout: {}\nstderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    const BEGIN: &str = "# >>> tidy-up >>>";

    #[test]
    fn add_installs_the_binary_and_writes_one_marked_block() {
        let s = sandbox();
        let out = ok(&s.add("/bin/zsh", &[]));
        assert!(out.contains("Open a NEW terminal"));

        assert!(s.installed().exists());
        assert_eq!(
            fs::metadata(s.installed()).unwrap().permissions().mode() & 0o111,
            0o111,
            "the installed binary is executable"
        );
        let rc = s.read(".zshrc");
        assert_eq!(rc.matches(BEGIN).count(), 1);
        assert!(
            rc.contains("export PATH=\"$HOME/.local/bin:$PATH\""),
            "{rc}"
        );
        assert!(rc.contains("# <<< tidy-up <<<"));
    }

    #[test]
    fn running_add_twice_changes_nothing_the_second_time() {
        let s = sandbox();
        ok(&s.add("/bin/zsh", &[]));
        let once = s.read(".zshrc");
        let out = ok(&s.add("/bin/zsh", &[]));
        assert!(out.contains("already set up"), "{out}");
        assert_eq!(s.read(".zshrc"), once, "no duplicate block");
    }

    #[test]
    fn remove_undoes_add_exactly_and_keeps_the_users_own_lines() {
        let s = sandbox();
        let original = "export EDITOR=vim\nalias ll='ls -l'\n";
        fs::write(s.path().join(".zshrc"), original).unwrap();

        ok(&s.add("/bin/zsh", &[]));
        assert!(s.read(".zshrc").starts_with(original));
        ok(&s.remove(&[]));

        assert_eq!(s.read(".zshrc"), original, "byte for byte what it was");
        assert!(!s.installed().exists(), "the installed binary is removed");
    }

    #[test]
    fn a_shell_file_without_a_trailing_newline_is_not_corrupted() {
        let s = sandbox();
        fs::write(s.path().join(".zshrc"), "export KEEP=1").unwrap(); // no final newline
        ok(&s.add("/bin/zsh", &[]));
        let rc = s.read(".zshrc");
        assert!(rc.starts_with("export KEEP=1\n"), "{rc:?}");
        ok(&s.remove(&[]));
        assert_eq!(s.read(".zshrc").trim_end(), "export KEEP=1");
    }

    #[test]
    fn each_shell_gets_its_own_file_and_syntax() {
        let s = sandbox();
        ok(&s.add("/bin/bash", &[]));
        assert!(s.read(".bashrc").contains(BEGIN));
        assert!(!s.path().join(".zshrc").exists());
        ok(&s.remove(&[]));

        ok(&s.add("/usr/bin/fish", &[]));
        let fish = s.read(".config/fish/conf.d/tidy-up.fish");
        assert!(
            fish.contains("set -gx PATH \"$HOME/.local/bin\" $PATH"),
            "{fish}"
        );
        assert!(!s.path().join(".profile").exists());
        ok(&s.remove(&[]));
        assert!(
            !s.path().join(".config/fish/conf.d/tidy-up.fish").exists(),
            "a fish file we created is deleted when empty"
        );

        ok(&s.add("/bin/dash", &[])); // anything unknown falls back to ~/.profile
        assert!(s.read(".profile").contains(BEGIN));
    }

    #[test]
    fn the_written_line_really_puts_tidy_up_on_the_path() {
        let s = sandbox();
        ok(&s.add("/bin/sh", &[]));
        // A fresh shell that only knows the startup file finds and runs the installed binary.
        let out = Command::new("sh")
            .arg("-c")
            .arg(". \"$HOME/.profile\"; command -v tidy-up && tidy-up --version")
            .env_clear()
            .env("HOME", s.path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        let text = ok(&out);
        assert!(text.contains(".local/bin/tidy-up"), "{text}");
        assert!(text.contains("tidy-up 9.9.9"), "{text}");
    }

    #[test]
    fn dry_run_changes_nothing_for_both_scripts() {
        let s = sandbox();
        let out = ok(&s.add("/bin/zsh", &["--dry-run"]));
        assert!(
            out.contains("would copy") && out.contains("would add to"),
            "{out}"
        );
        assert!(!s.installed().exists() && !s.path().join(".zshrc").exists());

        ok(&s.add("/bin/zsh", &[]));
        let (rc, bin_exists) = (s.read(".zshrc"), s.installed().exists());
        let out = ok(&s.remove(&["--dry-run"]));
        assert!(
            out.contains("would remove from") && out.contains("would delete"),
            "{out}"
        );
        assert_eq!((s.read(".zshrc"), s.installed().exists()), (rc, bin_exists));
    }

    #[test]
    fn nothing_is_edited_when_the_folder_is_already_on_the_path() {
        let s = sandbox();
        let with_dir = format!("{}/.local/bin:/usr/bin:/bin", s.path().display());
        let binary = s.binary.to_str().unwrap();
        let out = ok(&s.run(
            "add-to-path.sh",
            "/bin/zsh",
            &with_dir,
            &["--binary", binary],
        ));
        assert!(out.contains("already on your PATH"), "{out}");
        assert!(s.installed().exists());
        assert!(
            !s.path().join(".zshrc").exists(),
            "no shell file was needed"
        );
    }

    #[test]
    fn no_rc_installs_only_and_keep_binary_removes_only_the_path_setup() {
        let s = sandbox();
        ok(&s.add("/bin/zsh", &["--no-rc"]));
        assert!(s.installed().exists() && !s.path().join(".zshrc").exists());

        ok(&s.add("/bin/zsh", &[]));
        ok(&s.remove(&["--keep-binary"]));
        assert!(!s.read(".zshrc").contains(BEGIN));
        assert!(
            s.installed().exists(),
            "--keep-binary leaves the executable"
        );
    }

    #[test]
    fn a_custom_folder_outside_home_is_written_as_an_absolute_path() {
        let s = sandbox();
        let elsewhere = tempfile::tempdir().unwrap();
        let dir = elsewhere.path().join("tools");
        let out = ok(&s.add("/bin/zsh", &["--dir", dir.to_str().unwrap()]));
        assert!(dir.join("tidy-up").exists(), "{out}");
        let expected = format!("export PATH=\"{}:$PATH\"", dir.display());
        assert!(s.read(".zshrc").contains(&expected), "{}", s.read(".zshrc"));
        ok(&s.remove(&["--dir", dir.to_str().unwrap()]));
        assert!(!dir.join("tidy-up").exists());
        assert!(!s.read(".zshrc").contains(BEGIN));
    }

    #[test]
    fn removing_when_nothing_was_installed_is_harmless() {
        let s = sandbox();
        fs::write(s.path().join(".profile"), "export A=1\n").unwrap();
        let out = ok(&s.remove(&[]));
        assert!(out.contains("no executable"), "{out}");
        assert_eq!(s.read(".profile"), "export A=1\n");
    }

    #[test]
    fn other_programs_in_the_same_folder_are_left_alone() {
        let s = sandbox();
        ok(&s.add("/bin/zsh", &[]));
        fs::write(s.path().join(".local/bin/other-tool"), "x").unwrap();
        ok(&s.remove(&[]));
        assert!(s.path().join(".local/bin/other-tool").exists());
    }

    #[test]
    fn bad_usage_fails_clearly_without_changing_anything() {
        let s = sandbox();
        let out = s.run(
            "add-to-path.sh",
            "/bin/zsh",
            "/usr/bin:/bin",
            &["--binary", "/no/such/file"],
        );
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("cannot find the tidy-up executable")
        );

        let out = s.run("add-to-path.sh", "/bin/zsh", "/usr/bin:/bin", &["--wat"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("unknown option"));

        let out = s.run(
            "remove-from-path.sh",
            "/bin/zsh",
            "/usr/bin:/bin",
            &["--dir"],
        );
        assert!(!out.status.success());
        assert!(!s.path().join(".zshrc").exists() && !s.installed().exists());
    }

    #[test]
    fn help_works_for_both_scripts() {
        let s = sandbox();
        for script in ["add-to-path.sh", "remove-from-path.sh"] {
            let out = ok(&s.run(script, "/bin/zsh", "/usr/bin:/bin", &["--help"]));
            assert!(
                out.contains("Usage:") && out.contains("--dry-run"),
                "{script}: {out}"
            );
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::{fs, process::Command};

    use super::scripts_dir;

    /// Runs the PowerShell scripts end to end against a scratch registry key.
    #[test]
    fn add_and_remove_round_trip_through_the_registry_without_touching_the_real_path() {
        let scripts = scripts_dir().display().to_string().replace('\'', "''");
        let test = format!(
            r#"
$ErrorActionPreference = 'Stop'
$scripts = '{scripts}'
$work = Join-Path ([IO.Path]::GetTempPath()) ('tidyup-ps-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $work | Out-Null
$bin = Join-Path $work 'tidy-up.exe'
Set-Content -LiteralPath $bin -Value 'fake exe'
$inst = Join-Path $work 'Programs\tidy-up'
$key = 'Software\TidyUpScriptTest\' + [guid]::NewGuid().ToString('N')
function Fail($m) {{ throw "FAIL: $m" }}
function Raw {{
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($key)
    if (-not $k) {{ return '' }}
    try {{ [string]$k.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) }} finally {{ $k.Close() }}
}}
try {{
    # a user PATH that contains an unexpanded variable: it must survive both scripts untouched
    $seed = '%SystemRoot%\system32;C:\Existing'
    $k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($key)
    $k.SetValue('Path', $seed, [Microsoft.Win32.RegistryValueKind]::ExpandString); $k.Close()

    & "$scripts\add-to-path.ps1" -Binary $bin -InstallDir $inst -EnvironmentKey $key | Out-Null
    if (-not (Test-Path -LiteralPath "$inst\tidy-up.exe")) {{ Fail 'the binary was not copied' }}
    $after = Raw
    if ($after -ne "$seed;$inst") {{ Fail "unexpected PATH after add: $after" }}
    $kind = (Get-Item "HKCU:\$key").GetValueKind('Path')
    if ($kind -ne 'ExpandString') {{ Fail "PATH kind changed to $kind" }}

    & "$scripts\add-to-path.ps1" -Binary $bin -InstallDir $inst -EnvironmentKey $key | Out-Null
    if ((Raw) -ne $after) {{ Fail 'running add twice changed the PATH' }}

    & "$scripts\remove-from-path.ps1" -InstallDir $inst -EnvironmentKey $key -WhatIf | Out-Null
    if ((Raw) -ne $after -or -not (Test-Path -LiteralPath "$inst\tidy-up.exe")) {{ Fail '-WhatIf changed something' }}

    & "$scripts\remove-from-path.ps1" -InstallDir $inst -EnvironmentKey $key | Out-Null
    if ((Raw) -ne $seed) {{ Fail "PATH was not restored exactly: $(Raw)" }}
    if (Test-Path -LiteralPath $inst) {{ Fail 'the emptied install folder should be deleted' }}

    # -KeepFiles removes only the PATH entry; another file in the folder is never deleted
    & "$scripts\add-to-path.ps1" -Binary $bin -InstallDir $inst -EnvironmentKey $key | Out-Null
    Set-Content -LiteralPath "$inst\other.txt" -Value 'not ours'
    & "$scripts\remove-from-path.ps1" -InstallDir $inst -EnvironmentKey $key | Out-Null
    if (-not (Test-Path -LiteralPath "$inst\other.txt")) {{ Fail 'removed a file that is not ours' }}
    if (Test-Path -LiteralPath "$inst\tidy-up.exe") {{ Fail 'tidy-up.exe should be deleted' }}
    if ((Raw) -ne $seed) {{ Fail 'PATH not restored after the second cycle' }}

    # Process scope leaves the registry alone and affects only this session
    $before = $env:Path
    & "$scripts\add-to-path.ps1" -Binary $bin -InstallDir "$work\p" -Scope Process | Out-Null
    if (($env:Path -split ';') -notcontains "$work\p") {{ Fail 'process PATH was not updated' }}
    & "$scripts\remove-from-path.ps1" -InstallDir "$work\p" -Scope Process | Out-Null
    if ($env:Path -ne $before) {{ Fail 'process PATH was not restored' }}

    # removing something that was never added is harmless
    & "$scripts\remove-from-path.ps1" -InstallDir "$work\never" -EnvironmentKey $key | Out-Null
    if ((Raw) -ne $seed) {{ Fail 'removing an unknown folder changed the PATH' }}

    Write-Output 'PS-SCRIPT-TEST-OK'
}}
finally {{
    Remove-Item -LiteralPath "HKCU:\$key" -Recurse -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}}
"#
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.ps1");
        fs::write(&file, test).unwrap();
        let out = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&file)
            .output()
            .expect("Windows PowerShell is part of Windows");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("PS-SCRIPT-TEST-OK"),
            "stdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn the_scripts_are_ascii_only_so_windows_powershell_5_reads_them_correctly() {
        for name in [
            "add-to-path.ps1",
            "remove-from-path.ps1",
            "_path-helpers.ps1",
        ] {
            let bytes = fs::read(scripts_dir().join(name)).unwrap();
            assert!(bytes.is_ascii(), "{name} contains non-ASCII characters");
        }
    }
}

#[test]
fn every_script_has_a_counterpart_and_a_shared_helper() {
    for name in [
        "add-to-path.sh",
        "remove-from-path.sh",
        "_path-lib.sh",
        "add-to-path.ps1",
        "remove-from-path.ps1",
        "_path-helpers.ps1",
    ] {
        assert!(
            scripts_dir().join(name).is_file(),
            "scripts/{name} is missing"
        );
    }
}
