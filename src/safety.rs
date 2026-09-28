//! Detects folders the operating system manages, so tidy-up never reorganizes
//! `C:\Windows`, `/usr`, a drive root or a home directory by accident.
//!
//! Classification is pure. [`classify`] takes the platform, the [`Environment`] and a
//! set of already-probed [`Facts`], and returns an [`Assessment`]. It reads no
//! filesystem, no environment and no clock, so every rule is unit-testable on every
//! operating system, and the Windows rules are exercised on the Linux CI runner.
//!
//! Only [`SystemProbe`] touches the real machine, and every one of its failures
//! degrades to `None` rather than to a false accusation.
//!
//! ```
//! use std::path::Path;
//! use tidy_up::safety::{classify, Environment, Facts, Platform, Risk};
//!
//! let env = Environment::fake(Platform::Linux).with_home("/home/me");
//! let facts = Facts::default();
//! assert_eq!(classify(Path::new("/usr/lib"), &env, &facts).risk, Risk::Dangerous);
//! assert_eq!(classify(Path::new("/home/me/Downloads"), &env, &facts).risk, Risk::Safe);
//! ```

use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
};

use serde::Serialize;

/// Bumped when the rules change, so a log says which version of the rules judged a run.
pub const DETECTOR_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Platform
// ---------------------------------------------------------------------------

/// Which operating system's rules to apply.
///
/// Defaults to the compile target. Tests set it freely, which is what lets one
/// machine check all three rule tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// Windows.
    Windows,
    /// macOS.
    MacOs,
    /// Linux and other Unixes.
    #[default]
    Linux,
    /// Anything else: only the platform-independent rules apply.
    Other,
}

impl Platform {
    /// The platform this binary was built for.
    pub fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(unix) {
            Platform::Linux
        } else {
            Platform::Other
        }
    }

    /// Whether path comparison ignores case. Windows always; macOS by default.
    fn case_insensitive(self) -> bool {
        matches!(self, Platform::Windows | Platform::MacOs)
    }
}

// ---------------------------------------------------------------------------
// Path splitting
// ---------------------------------------------------------------------------

/// A path split into a comparable prefix and components.
///
/// `std::path` parses paths for the *build* target, so it cannot be used here:
/// on Linux it sees `C:\Windows` as a single component. This splitter takes the
/// platform as an argument instead, which is what makes the rule tables testable
/// everywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Parts {
    /// `C:` for a drive, `\\server\share` for a UNC share, `/` for a Unix root,
    /// `None` for a relative path.
    prefix: Option<String>,
    /// Components, case-folded on Windows and macOS.
    comps: Vec<String>,
}

impl Parts {
    /// Whether this is the very top of a filesystem: a drive root or `/`.
    fn is_root(&self) -> bool {
        self.comps.is_empty() && self.prefix.is_some()
    }

    /// Whether the prefix is a UNC share rather than a drive letter.
    fn is_unc(&self) -> bool {
        self.prefix.as_deref().is_some_and(|p| p.starts_with(r"\\"))
    }

    /// Whether these components begin with `wanted`.
    fn starts_with(&self, wanted: &[&str]) -> bool {
        self.comps.len() >= wanted.len()
            && self
                .comps
                .iter()
                .zip(wanted)
                .all(|(have, want)| have == want)
    }

    /// Whether these components are exactly `wanted`.
    fn is_exactly(&self, wanted: &[&str]) -> bool {
        self.comps.len() == wanted.len() && self.starts_with(wanted)
    }

    /// Whether this path is `other` or lies inside it. Both must share a prefix.
    fn under(&self, other: &Parts) -> bool {
        self.prefix == other.prefix
            && self.comps.len() >= other.comps.len()
            && self
                .comps
                .iter()
                .zip(&other.comps)
                .all(|(have, want)| have == want)
    }

    /// Whether this path is exactly `other`.
    fn same_as(&self, other: &Parts) -> bool {
        self.prefix == other.prefix && self.comps == other.comps
    }
}

/// Splits `path` for `platform`: folds away verbatim prefixes, drops empty and `.`
/// components, resolves `..`, and case-folds where the platform does.
pub(crate) fn parts(path: &Path, platform: Platform) -> Parts {
    let text = path.to_string_lossy().into_owned();
    let (prefix, rest) = match platform {
        Platform::Windows => split_windows_prefix(&text),
        _ => {
            if let Some(rest) = text.strip_prefix('/') {
                (Some("/".to_string()), rest.to_string())
            } else {
                (None, text)
            }
        }
    };

    let mut comps: Vec<String> = Vec::new();
    for raw in rest.split(['/', '\\']) {
        match raw {
            "" | "." => {}
            ".." => {
                comps.pop();
            }
            name => {
                let name = name.trim_end_matches(['.', ' ']);
                let name = if name.is_empty() { raw } else { name };
                comps.push(if platform.case_insensitive() {
                    name.to_lowercase()
                } else {
                    name.to_string()
                });
            }
        }
    }
    Parts { prefix, comps }
}

/// Peels `\\?\`, `\\?\UNC\`, a UNC share or a drive letter off the front.
fn split_windows_prefix(text: &str) -> (Option<String>, String) {
    // `\\?\UNC\server\share\...` and `\\server\share\...` mean the same place.
    let unc_tail = text
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| text.strip_prefix(r"\\?\unc\"))
        .map(str::to_string)
        .or_else(|| {
            let bare = text
                .strip_prefix(r"\\")
                .or_else(|| text.strip_prefix("//"))?;
            // `\\?\C:\...` is a verbatim drive, not a share.
            (!bare.starts_with("?\\") && !bare.starts_with("?/")).then(|| bare.to_string())
        });
    if let Some(tail) = unc_tail {
        let mut walk = tail.split(['/', '\\']).filter(|s| !s.is_empty());
        let server = walk.next().unwrap_or_default().to_lowercase();
        let share = walk.next().unwrap_or_default().to_lowercase();
        let rest: Vec<&str> = walk.collect();
        return (Some(format!(r"\\{server}\{share}")), rest.join("\\"));
    }

    let text = text
        .strip_prefix(r"\\?\")
        .or_else(|| text.strip_prefix("//?/"))
        .unwrap_or(text);
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive = text[..2].to_uppercase();
        return (Some(drive), text[2..].to_string());
    }
    (None, text.to_string())
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

/// The environment the rules are read against.
///
/// [`Environment::from_env`] reads the process environment once. Tests build one
/// literally with [`Environment::fake`], so no test depends on the machine it runs on.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    /// Which rule table to use.
    pub platform: Platform,
    /// `%USERPROFILE%` or `$HOME`, canonicalized when possible.
    pub home: Option<PathBuf>,
    /// The system temporary directory, canonicalized.
    ///
    /// It lives inside a protected path on two platforms (`%LOCALAPPDATA%\Temp` on
    /// Windows, `/private/var/folders/...` on macOS), so paths strictly under it are
    /// carved out of the rules that would otherwise catch them.
    pub temp: Option<PathBuf>,
    /// Windows only: the variables that name system folders. Empty elsewhere.
    pub vars: BTreeMap<String, PathBuf>,
}

/// The Windows variables worth reading, in the order they are tried.
const WINDOWS_VARS: &[&str] = &[
    "SystemRoot",
    "windir",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "ProgramData",
    "SystemDrive",
    "PUBLIC",
    "LOCALAPPDATA",
    "APPDATA",
];

impl Environment {
    /// Reads the real environment.
    pub fn from_env() -> Self {
        let platform = Platform::current();
        let home = std::env::var_os("USERPROFILE")
            .filter(|v| !v.is_empty())
            .or_else(|| std::env::var_os("HOME"))
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .map(canonical_or_given);
        let temp = Some(canonical_or_given(std::env::temp_dir()));
        let mut vars = BTreeMap::new();
        if platform == Platform::Windows {
            for name in WINDOWS_VARS {
                if let Some(value) = std::env::var_os(name).filter(|v| !v.is_empty()) {
                    vars.insert((*name).to_string(), PathBuf::from(value));
                }
            }
        }
        Environment {
            platform,
            home,
            temp,
            vars,
        }
    }

    /// An environment with nothing in it but a platform, for tests.
    pub fn fake(platform: Platform) -> Self {
        Environment {
            platform,
            ..Default::default()
        }
    }

    /// Sets the home directory.
    pub fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(home.into());
        self
    }

    /// Sets the temporary directory.
    pub fn with_temp(mut self, temp: impl Into<PathBuf>) -> Self {
        self.temp = Some(temp.into());
        self
    }

    /// Sets one Windows variable.
    pub fn with_var(mut self, key: &str, value: impl Into<PathBuf>) -> Self {
        self.vars.insert(key.to_string(), value.into());
        self
    }

    fn var(&self, key: &str) -> Option<&Path> {
        self.vars.get(key).map(PathBuf::as_path)
    }
}

fn canonical_or_given(path: PathBuf) -> PathBuf {
    match std::fs::canonicalize(&path) {
        Ok(canonical) => strip_verbatim(canonical),
        Err(_) => path,
    }
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

// ---------------------------------------------------------------------------
// Facts
// ---------------------------------------------------------------------------

/// What a probe learned about the folder itself.
///
/// Every field is optional so an unsupported platform or a failed syscall becomes
/// "unknown", never a false accusation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// Windows `FILE_ATTRIBUTE_SYSTEM` is set on the directory.
    pub system_attribute: Option<bool>,
    /// Windows `FILE_ATTRIBUTE_REPARSE_POINT`: a junction or a volume mount.
    pub reparse_point: Option<bool>,
    /// Unix: the device number differs from the parent's, so this is a mount root.
    pub mount_point: Option<bool>,
    /// Unix: the folder is owned by the user running tidy-up.
    pub owned_by_me: Option<bool>,
    /// Whether this process can actually create an entry here.
    pub writable: Option<bool>,
    /// Linux: the filesystem type from `/proc/self/mountinfo`.
    pub fs_type: Option<String>,
}

/// Filesystems that exist only in kernel memory. Nothing a user keeps files in.
///
/// `tmpfs` is deliberately absent: `/tmp` and `/dev/shm` are tmpfs and `/tmp` is a
/// perfectly reasonable folder to tidy.
const PSEUDO_FILESYSTEMS: &[&str] = &[
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "cgroup",
    "cgroup2",
    "securityfs",
    "debugfs",
    "tracefs",
    "pstore",
    "bpf",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "efivarfs",
    "nsfs",
    "autofs",
    "binfmt_misc",
    "ramfs",
    "squashfs",
];

// ---------------------------------------------------------------------------
// Verdict
// ---------------------------------------------------------------------------

/// How much trouble reorganizing a folder would cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// An ordinary folder. Nothing is said.
    #[default]
    Safe,
    /// Worth a warning and an explicit confirmation.
    Caution,
    /// Refused unless the user passes the override flag.
    Dangerous,
}

impl Risk {
    /// Stable machine token for logs.
    pub fn key(self) -> &'static str {
        match self {
            Risk::Safe => "safe",
            Risk::Caution => "caution",
            Risk::Dangerous => "dangerous",
        }
    }
}

/// One machine-readable finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Reason {
    /// `/`, `C:\`, `D:\`: the top of a filesystem.
    FilesystemRoot,
    /// `\\server\share` with nothing below it.
    UncShareRoot,
    /// A folder on the platform's protected list.
    SystemFolder {
        /// Human description, used in the warning.
        label: &'static str,
        /// The rule that fired, for logs.
        matched: String,
        /// How bad this particular entry is.
        severity: Risk,
    },
    /// The folder that holds every user's home (`C:\Users`, `/home`, `/Users`).
    UserContainer,
    /// The current user's home directory itself.
    HomeRoot,
    /// Per-user application state (`AppData`, `~/Library`).
    ApplicationData,
    /// A kernel or virtual filesystem.
    PseudoFilesystem {
        /// The filesystem type reported by the kernel.
        fs_type: String,
    },
    /// Windows marks the folder itself as a system folder.
    SystemAttribute,
    /// The root of a mounted volume.
    MountPoint,
    /// Owned by another user.
    NotOwned,
    /// This process cannot write here. Never overridable: a flag cannot grant
    /// permission the operating system refused.
    NotWritable,
}

impl Reason {
    /// The risk this reason alone implies.
    pub fn risk(&self) -> Risk {
        match self {
            Reason::FilesystemRoot
            | Reason::UncShareRoot
            | Reason::UserContainer
            | Reason::HomeRoot
            | Reason::ApplicationData
            | Reason::PseudoFilesystem { .. }
            | Reason::SystemAttribute
            | Reason::NotWritable => Risk::Dangerous,
            Reason::SystemFolder { severity, .. } => *severity,
            Reason::MountPoint | Reason::NotOwned => Risk::Caution,
        }
    }

    /// Whether the override flag can unlock this. False only for a real permission wall.
    pub fn overridable(&self) -> bool {
        !matches!(self, Reason::NotWritable)
    }

    /// Stable machine token for logs.
    pub fn key(&self) -> &'static str {
        match self {
            Reason::FilesystemRoot => "filesystem_root",
            Reason::UncShareRoot => "unc_share_root",
            Reason::SystemFolder { .. } => "system_folder",
            Reason::UserContainer => "user_container",
            Reason::HomeRoot => "home_root",
            Reason::ApplicationData => "application_data",
            Reason::PseudoFilesystem { .. } => "pseudo_filesystem",
            Reason::SystemAttribute => "system_attribute",
            Reason::MountPoint => "mount_point",
            Reason::NotOwned => "not_owned",
            Reason::NotWritable => "not_writable",
        }
    }

    /// One sentence a person can act on.
    pub fn explain(&self) -> String {
        match self {
            Reason::FilesystemRoot => {
                "This is the top of a whole drive, not a folder of your files.".into()
            }
            Reason::UncShareRoot => {
                "This is the top of a network share, not a folder inside it.".into()
            }
            Reason::SystemFolder { label, .. } => {
                format!("It holds {label}, which other programs expect to find in place.")
            }
            Reason::UserContainer => {
                "This folder holds everybody's home folders, not one person's files.".into()
            }
            Reason::HomeRoot => {
                "This is your home folder. Tidying it would move your settings and every \
                 top-level folder you have."
                    .into()
            }
            Reason::ApplicationData => {
                "This holds application settings and state. Moving it breaks the programs \
                 that put it there."
                    .into()
            }
            Reason::PseudoFilesystem { fs_type } => {
                format!(
                    "This is a {fs_type} filesystem the kernel invents; the files in it are not real."
                )
            }
            Reason::SystemAttribute => "Windows marks this folder as one of its own.".into(),
            Reason::MountPoint => {
                "This is the root of a mounted volume, so it may hold more than you expect.".into()
            }
            Reason::NotOwned => "It belongs to another user account.".into(),
            Reason::NotWritable => "tidy-up cannot write here, so nothing could be moved.".into(),
        }
    }
}

/// The verdict for one folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Assessment {
    /// The path that was judged.
    pub path: PathBuf,
    /// The highest risk of any reason; `Safe` when there are none.
    pub risk: Risk,
    /// Every reason that fired, worst first.
    pub reasons: Vec<Reason>,
}

impl Assessment {
    /// A folder with no findings.
    pub fn safe(path: impl Into<PathBuf>) -> Self {
        Assessment {
            path: path.into(),
            risk: Risk::Safe,
            reasons: Vec::new(),
        }
    }

    /// Whether the override flag can unlock this folder.
    ///
    /// False when any reason is a real permission wall: no flag grants permission.
    pub fn overridable(&self) -> bool {
        self.reasons.iter().all(Reason::overridable)
    }

    /// Whether anything at all was found.
    pub fn is_safe(&self) -> bool {
        self.risk == Risk::Safe
    }

    /// One line for the top of a warning.
    pub fn headline(&self) -> String {
        let what = match self.reasons.first() {
            Some(Reason::FilesystemRoot) => "the top of a drive",
            Some(Reason::UncShareRoot) => "the top of a network share",
            Some(Reason::HomeRoot) => "your home folder",
            Some(Reason::UserContainer) => "the folder that holds everyone's home folders",
            Some(Reason::ApplicationData) => "application data",
            Some(Reason::PseudoFilesystem { .. }) => "a kernel filesystem",
            Some(Reason::NotWritable) => "a folder you cannot write to",
            Some(Reason::MountPoint) => "the root of a mounted volume",
            Some(Reason::NotOwned) => "a folder owned by someone else",
            _ => "a folder the system manages",
        };
        format!("{} is {what}.", self.path.display())
    }

    fn from_reasons(path: &Path, mut reasons: Vec<Reason>) -> Self {
        // Worst first: the first reason is the headline.
        reasons.sort_by_key(|reason| std::cmp::Reverse(reason.risk()));
        // Several rules routinely name the same folder (`C:\Windows` matches
        // %SystemRoot%, %windir% and the name rule). Say it once.
        let mut seen = Vec::new();
        reasons.retain(|reason| {
            let sentence = reason.explain();
            if seen.contains(&sentence) {
                return false;
            }
            seen.push(sentence);
            true
        });
        let risk = reasons.iter().map(Reason::risk).max().unwrap_or(Risk::Safe);
        Assessment {
            path: path.to_path_buf(),
            risk,
            reasons,
        }
    }
}

impl fmt::Display for Assessment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.headline())
    }
}

// ---------------------------------------------------------------------------
// Rule tables
// ---------------------------------------------------------------------------

/// One protected location, as already case-folded path components.
struct Entry {
    /// Components below the root, e.g. `["program files"]`.
    comps: &'static [&'static str],
    /// Match only this exact folder, not everything under it.
    exact: bool,
    /// How bad it is.
    severity: Risk,
    /// Human description, dropped into "It holds {label}".
    label: &'static str,
}

const fn dir(comps: &'static [&'static str], label: &'static str) -> Entry {
    Entry {
        comps,
        exact: false,
        severity: Risk::Dangerous,
        label,
    }
}

const fn only(comps: &'static [&'static str], label: &'static str) -> Entry {
    Entry {
        comps,
        exact: true,
        severity: Risk::Dangerous,
        label,
    }
}

const fn care(comps: &'static [&'static str], label: &'static str) -> Entry {
    Entry {
        comps,
        exact: false,
        severity: Risk::Caution,
        label,
    }
}

const fn care_only(comps: &'static [&'static str], label: &'static str) -> Entry {
    Entry {
        comps,
        exact: true,
        severity: Risk::Caution,
        label,
    }
}

/// Windows rules, relative to any drive root. Applied whether or not the matching
/// environment variable is set, so a scrubbed environment or a second Windows
/// install on another drive stays protected.
const WINDOWS_RULES: &[Entry] = &[
    dir(&["windows"], "the Windows installation"),
    dir(&["program files"], "installed programs"),
    dir(&["program files (x86)"], "installed programs"),
    dir(&["programdata"], "shared application data"),
    dir(&["$recycle.bin"], "the recycle bin"),
    dir(&["system volume information"], "system restore data"),
    dir(&["recovery"], "Windows recovery files"),
    dir(&["config.msi"], "installer state"),
    dir(&["perflogs"], "performance logs"),
    dir(&["$winreagent"], "Windows update state"),
    dir(&["msocache"], "Office installer state"),
    care(&["users", "public"], "the shared Public folder"),
];

/// macOS rules.
const MACOS_RULES: &[Entry] = &[
    dir(&["system"], "macOS itself"),
    dir(&["library"], "system-wide application support"),
    dir(&["applications"], "installed applications"),
    dir(&["usr"], "the operating system's own programs"),
    dir(&["bin"], "core command-line programs"),
    dir(&["sbin"], "system administration programs"),
    dir(&["etc"], "system configuration"),
    dir(&["var"], "system state and logs"),
    dir(&["private"], "system state"),
    dir(&["cores"], "crash dumps"),
    dir(&["network"], "network mounts"),
    dir(&[".vol"], "a filesystem control folder"),
    only(&["volumes"], "the list of mounted disks"),
    care(&["opt"], "optional software"),
];

/// Linux rules.
const LINUX_RULES: &[Entry] = &[
    dir(&["bin"], "core command-line programs"),
    dir(&["sbin"], "system administration programs"),
    dir(&["lib"], "system libraries"),
    dir(&["lib32"], "system libraries"),
    dir(&["lib64"], "system libraries"),
    dir(&["libx32"], "system libraries"),
    dir(&["usr"], "the operating system's own programs"),
    dir(&["etc"], "system configuration"),
    dir(&["boot"], "the files that start the computer"),
    dir(&["dev"], "device nodes"),
    dir(&["proc"], "kernel process information"),
    dir(&["sys"], "kernel settings"),
    dir(&["run"], "runtime state"),
    dir(&["snap"], "installed snap packages"),
    dir(&["nix"], "the Nix store"),
    dir(&["efi"], "the files that start the computer"),
    dir(&["lost+found"], "filesystem recovery data"),
    dir(&["var"], "system state and logs"),
    dir(&["root"], "the root account's home folder"),
    care(&["opt"], "optional software"),
    care(&["srv"], "served data"),
    care_only(&["media"], "the list of mounted disks"),
    care_only(&["mnt"], "the list of mounted disks"),
];

/// A subtree that overrides the rule tables.
enum Carve {
    /// Nothing from the path rules applies below here.
    Safe,
    /// Downgrade to caution with this description.
    Caution(&'static str),
}

/// macOS carve-outs, longest match wins.
fn macos_carve(p: &Parts) -> Option<Carve> {
    // `/usr/local` is a writable firmlink full of user-installed software.
    if p.comps.len() >= 2 && p.starts_with(&["usr", "local"]) {
        return Some(Carve::Caution("software you installed yourself"));
    }
    // A mounted disk, a user's own home, and the same home seen through the data
    // volume are all ordinary places to keep files.
    if p.comps.len() >= 2 && (p.starts_with(&["volumes"]) || p.starts_with(&["users"])) {
        return Some(Carve::Safe);
    }
    if p.comps.len() >= 4 && p.starts_with(&["system", "volumes", "data", "users"]) {
        return Some(Carve::Safe);
    }
    if p.starts_with(&["private", "tmp"]) || p.starts_with(&["tmp"]) {
        return Some(Carve::Safe);
    }
    None
}

/// Linux carve-outs, longest match wins.
fn linux_carve(p: &Parts) -> Option<Carve> {
    if p.comps.len() >= 2 && p.starts_with(&["usr", "local"]) {
        return Some(Carve::Caution("software you installed yourself"));
    }
    // Removable media. `/run/media` must be carved out of the `/run` rule, or every
    // USB stick a desktop mounts gets refused.
    if p.comps.len() >= 4 && p.starts_with(&["run", "media"]) {
        return Some(Carve::Safe);
    }
    if p.comps.len() >= 3 && p.starts_with(&["media"]) {
        return Some(Carve::Safe);
    }
    if p.comps.len() >= 2 && p.starts_with(&["mnt"]) {
        return Some(Carve::Safe);
    }
    if p.starts_with(&["tmp"]) || p.starts_with(&["var", "tmp"]) {
        return Some(Carve::Safe);
    }
    None
}

// ---------------------------------------------------------------------------
// The classifier
// ---------------------------------------------------------------------------

/// Judges `path` against `env` and `facts`.
///
/// Pure: no I/O, no environment reads, no clock. Never returns an error, because a
/// classifier that can fail is one callers are tempted to treat as "safe".
pub fn classify(path: &Path, env: &Environment, facts: &Facts) -> Assessment {
    let p = parts(path, env.platform);
    let mut reasons = Vec::new();

    // A path strictly inside the temporary directory is exempt from the path rules.
    // On Windows and macOS the temp directory lives inside a protected tree, and
    // refusing to tidy it would be both wrong and a self-inflicted wound.
    let in_temp = env
        .temp
        .as_deref()
        .map(|t| parts(t, env.platform))
        .is_some_and(|t| p.under(&t) && !p.same_as(&t));

    if p.is_root() {
        reasons.push(if p.is_unc() {
            Reason::UncShareRoot
        } else {
            Reason::FilesystemRoot
        });
    }

    if let Some(home) = env.home.as_deref() {
        let home = parts(home, env.platform);
        if p.same_as(&home) {
            reasons.push(Reason::HomeRoot);
        }
    }

    if !in_temp {
        reasons.extend(path_rules(&p, env));
    }

    if let Some(fs_type) = facts.fs_type.as_deref()
        && PSEUDO_FILESYSTEMS.contains(&fs_type)
    {
        reasons.push(Reason::PseudoFilesystem {
            fs_type: fs_type.to_string(),
        });
    }
    if facts.system_attribute == Some(true) {
        reasons.push(Reason::SystemAttribute);
    }
    if facts.writable == Some(false) {
        reasons.push(Reason::NotWritable);
    }
    if facts.owned_by_me == Some(false) {
        reasons.push(Reason::NotOwned);
    }
    // A mount point is only worth mentioning when nothing stronger was found:
    // every external drive is one, and they are the most ordinary target there is.
    let bare_mount = facts.mount_point == Some(true) || facts.reparse_point == Some(true);
    if bare_mount && reasons.is_empty() {
        reasons.push(Reason::MountPoint);
    }

    Assessment::from_reasons(path, reasons)
}

/// The per-platform path rules, after the temp carve-out.
fn path_rules(p: &Parts, env: &Environment) -> Vec<Reason> {
    match env.platform {
        Platform::Windows => windows_rules(p, env),
        Platform::MacOs => unix_rules(p, env, MACOS_RULES, macos_carve, &["users"]),
        Platform::Linux => unix_rules(p, env, LINUX_RULES, linux_carve, &["home"]),
        Platform::Other => Vec::new(),
    }
}

fn windows_rules(p: &Parts, env: &Environment) -> Vec<Reason> {
    let mut reasons = Vec::new();
    if p.prefix.is_none() {
        return reasons;
    }

    // Variables only ever add coverage: a folder pointed at by %SystemRoot% is
    // protected even when it is not called Windows and is not on the system drive.
    for name in [
        "SystemRoot",
        "windir",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "ProgramData",
    ] {
        if let Some(value) = env.var(name)
            && p.under(&parts(value, Platform::Windows))
        {
            reasons.push(Reason::SystemFolder {
                label: match name {
                    "ProgramFiles" | "ProgramFiles(x86)" | "ProgramW6432" => "installed programs",
                    "ProgramData" => "shared application data",
                    _ => "the Windows installation",
                },
                matched: format!("%{name}%"),
                severity: Risk::Dangerous,
            });
        }
    }

    for entry in WINDOWS_RULES {
        let hit = if entry.exact {
            p.is_exactly(entry.comps)
        } else {
            p.starts_with(entry.comps)
        };
        if hit {
            reasons.push(Reason::SystemFolder {
                matched: entry.comps.join("\\"),
                label: entry.label,
                severity: entry.severity,
            });
        }
    }

    if p.is_exactly(&["users"]) {
        reasons.push(Reason::UserContainer);
    }

    // AppData, but only where AppData really lives. A folder somebody named
    // `D:\Backup\AppData-2019` is not application state.
    let appdata_var = ["LOCALAPPDATA", "APPDATA"].iter().any(|name| {
        env.var(name)
            .is_some_and(|value| p.under(&parts(value, Platform::Windows)))
    });
    let appdata_shape = p.comps.len() >= 3 && p.comps[0] == "users" && p.comps[2] == "appdata";
    if appdata_var || appdata_shape {
        reasons.push(Reason::ApplicationData);
    }

    reasons
}

fn unix_rules(
    p: &Parts,
    env: &Environment,
    table: &'static [Entry],
    carve: fn(&Parts) -> Option<Carve>,
    user_container: &[&str],
) -> Vec<Reason> {
    let mut reasons = Vec::new();
    if p.prefix.is_none() {
        return reasons;
    }

    // `~/Library` on macOS is the equivalent of AppData and just as fragile, so it
    // is judged before the carve-out that makes the rest of a home folder ordinary.
    if env.platform == Platform::MacOs
        && let Some(home) = env.home.as_deref()
    {
        let home = parts(home, env.platform);
        let mut library = home.clone();
        library.comps.push("library".to_string());
        if p.under(&library) {
            reasons.push(Reason::ApplicationData);
            return reasons;
        }
    }

    match carve(p) {
        Some(Carve::Safe) => return reasons,
        Some(Carve::Caution(label)) => {
            reasons.push(Reason::SystemFolder {
                label,
                matched: p.comps.first().cloned().unwrap_or_default(),
                severity: Risk::Caution,
            });
            return reasons;
        }
        None => {}
    }

    for entry in table {
        let hit = if entry.exact {
            p.is_exactly(entry.comps)
        } else {
            p.starts_with(entry.comps)
        };
        if hit {
            reasons.push(Reason::SystemFolder {
                matched: format!("/{}", entry.comps.join("/")),
                label: entry.label,
                severity: entry.severity,
            });
        }
    }

    if p.is_exactly(user_container) {
        reasons.push(Reason::UserContainer);
    }

    reasons
}

/// Widens the verdict when hidden items are in scope.
///
/// `--include-hidden` changes the blast radius: a folder that merely warranted
/// caution becomes dangerous once dotfiles and attribute-hidden items are being
/// moved, because that is where configuration and operating-system state live.
/// A safe folder stays safe.
pub fn escalate_for_hidden(mut assessment: Assessment, include_hidden: bool) -> Assessment {
    if include_hidden && assessment.risk == Risk::Caution {
        assessment.risk = Risk::Dangerous;
    }
    assessment
}

// ---------------------------------------------------------------------------
// Probes
// ---------------------------------------------------------------------------

/// Something that can collect [`Facts`] about a folder.
pub trait Probe {
    /// Learns what it can about `path`. Never fails: unknowns stay `None`.
    fn facts(&self, path: &Path) -> Facts;
}

/// Asks the operating system.
///
/// Every failure degrades to `None`, so a folder is never accused because a
/// syscall did not work.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProbe {
    /// Whether to find out if the folder is writable by briefly creating a file.
    ///
    /// Only set for commands that are about to write. A reporting command such as
    /// `analyze` must not touch the disk, and neither must a dry run.
    pub write_probe: bool,
}

impl SystemProbe {
    /// A probe that only reads metadata.
    pub fn read_only() -> Self {
        SystemProbe { write_probe: false }
    }

    /// A probe that also checks writability, for commands about to move files.
    pub fn writing() -> Self {
        SystemProbe { write_probe: true }
    }
}

impl Probe for SystemProbe {
    fn facts(&self, path: &Path) -> Facts {
        let mut facts = Facts {
            writable: self.write_probe.then(|| probe_writable(path)).flatten(),
            ..Default::default()
        };
        platform_facts(path, &mut facts);
        facts
    }
}

/// Whether this process can actually create an entry in `dir`.
///
/// Mode bits, POSIX and NFSv4 ACLs, read-only mounts, macOS SIP and Windows ACLs
/// all disagree with each other, and `access(2)` is advisory and lies for root, so
/// the only honest answer is to try. A uniquely named empty file is created and
/// removed again.
///
/// Returns `None` when the attempt failed for a reason other than permission (the
/// disk is full, the folder vanished), so an unknown never reads as an accusation.
fn probe_writable(dir: &Path) -> Option<bool> {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as u64);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let probe = dir.join(format!(
        ".tidy-up-write-test-{}-{nanos}-{unique}",
        std::process::id()
    ));

    match std::fs::File::create_new(&probe) {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&probe);
            Some(true)
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            Some(false)
        }
        Err(_) => None,
    }
}

#[cfg(windows)]
fn platform_facts(path: &Path, facts: &mut Facts) {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    // FILE_ATTRIBUTE_READONLY is deliberately not consulted: on a directory it
    // means "has a customized view", not "read-only", and is set on ordinary
    // folders such as Documents.
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        let attributes = meta.file_attributes();
        facts.system_attribute = Some(attributes & FILE_ATTRIBUTE_SYSTEM != 0);
        facts.reparse_point = Some(attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0);
    }
}

#[cfg(unix)]
fn platform_facts(path: &Path, facts: &mut Facts) {
    use std::os::unix::fs::MetadataExt;
    if let Ok(meta) = std::fs::metadata(path) {
        // The root of a mounted filesystem has a different device number from the
        // directory it is mounted on.
        facts.mount_point = path
            .parent()
            .and_then(|parent| std::fs::metadata(parent).ok())
            .map(|parent| parent.dev() != meta.dev());
    }
    if cfg!(target_os = "linux")
        && let Ok(text) = std::fs::read_to_string("/proc/self/mountinfo")
    {
        facts.fs_type = fs_type_from_mountinfo(&text, path);
    }
}

#[cfg(not(any(unix, windows)))]
fn platform_facts(_path: &Path, _facts: &mut Facts) {}

/// Reads the filesystem type covering `path` out of the contents of
/// `/proc/self/mountinfo`.
///
/// The format is whitespace separated: field 5 is the mount point, then a variable
/// number of optional fields, then a literal `-`, then the filesystem type. Mount
/// points are escaped octally, so `\040` is a space. The longest mount point that
/// covers `path` wins, which is what makes nested mounts come out right.
///
/// Pure, so it is tested against fixtures rather than the live kernel.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn fs_type_from_mountinfo(text: &str, path: &Path) -> Option<String> {
    let target = parts(path, Platform::Linux);
    let mut best: Option<(usize, String)> = None;

    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(mount_point) = fields.get(4) else {
            continue;
        };
        // Everything after the `-` separator describes the filesystem itself.
        let Some(dash) = fields.iter().position(|f| *f == "-") else {
            continue;
        };
        let Some(fs_type) = fields.get(dash + 1) else {
            continue;
        };
        let mount_point = unescape_octal(mount_point);
        let mount = parts(Path::new(&mount_point), Platform::Linux);
        if target.under(&mount) {
            let depth = mount.comps.len();
            if best.as_ref().is_none_or(|(best, _)| depth >= *best) {
                best = Some((depth, (*fs_type).to_string()));
            }
        }
    }
    best.map(|(_, fs_type)| fs_type)
}

/// Turns `\040` and friends back into the characters they stand for.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn unescape_octal(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && let Ok(code) = u8::from_str_radix(&text[i + 1..i + 4], 8)
        {
            out.push(code as char);
            i += 4;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Fixed answers, for tests and simulations.
#[derive(Debug, Clone, Default)]
pub struct StaticFacts(BTreeMap<PathBuf, Facts>);

impl StaticFacts {
    /// An empty table; every path comes back with everything unknown.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares the facts for one path.
    pub fn with(mut self, path: impl Into<PathBuf>, facts: Facts) -> Self {
        self.0.insert(path.into(), facts);
        self
    }
}

impl Probe for StaticFacts {
    fn facts(&self, path: &Path) -> Facts {
        self.0.get(path).cloned().unwrap_or_default()
    }
}

/// Judges a real folder without touching it, for reporting commands.
pub fn assess(path: &Path) -> Assessment {
    assess_with(path, &SystemProbe::read_only())
}

/// Judges a real folder a command is about to write to, including whether it
/// actually can.
pub fn assess_for_writing(path: &Path) -> Assessment {
    assess_with(path, &SystemProbe::writing())
}

/// Judges `path` using `probe` and the real environment.
pub fn assess_with(path: &Path, probe: &dyn Probe) -> Assessment {
    classify(path, &Environment::from_env(), &probe.facts(path))
}

#[cfg(test)]
#[path = "safety_tests.rs"]
mod tests;
