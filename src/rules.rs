//! User-configurable ignore rules (extensions, names, globs, shortcuts).

use std::{collections::BTreeSet, fs, path::Path};

use crate::{
    error::{IoContext, Result},
    scan::SkipReason,
};

/// Extensions treated as desktop shortcuts unless explicitly included.
pub const SHORTCUT_EXTENSIONS: &[&str] = &["lnk", "url", "webloc", "desktop"];

/// OS-generated files that are never worth moving.
const SYSTEM_FILES: &[&str] = &["desktop.ini", "thumbs.db", "ehthumbs.db", ".ds_store"];

/// Case-insensitive glob match supporting `*` (any run) and `?` (any one char).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<usize> = None;
    let mut mark = 0;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Decides which directory entries the tool must leave alone.
#[derive(Debug, Clone)]
pub struct IgnoreRules {
    extensions: BTreeSet<String>,
    patterns: Vec<String>,
    skip_shortcuts: bool,
    skip_hidden: bool,
}

impl Default for IgnoreRules {
    /// Skips shortcuts and hidden files, with no user rules.
    fn default() -> Self {
        Self {
            extensions: BTreeSet::new(),
            patterns: Vec::new(),
            skip_shortcuts: true,
            skip_hidden: true,
        }
    }
}

impl IgnoreRules {
    /// Creates the default rule set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Chooses whether shortcut files (`.lnk`, `.url`, ...) are processed.
    pub fn include_shortcuts(mut self, include: bool) -> Self {
        self.skip_shortcuts = !include;
        self
    }

    /// Chooses whether hidden files are processed.
    pub fn include_hidden(mut self, include: bool) -> Self {
        self.skip_hidden = !include;
        self
    }

    /// Whether hidden entries are being skipped.
    pub fn skips_hidden(&self) -> bool {
        self.skip_hidden
    }

    /// Ignores a file extension (`pdf`, `.PDF` and `Pdf` are equivalent).
    pub fn add_extension(&mut self, ext: &str) {
        let ext = ext.trim().trim_start_matches('.').to_lowercase();
        if !ext.is_empty() {
            self.extensions.insert(ext);
        }
    }

    /// Ignores anything whose name matches a glob (or exact name).
    pub fn add_pattern(&mut self, pattern: &str) {
        let pattern = pattern.trim();
        if !pattern.is_empty() {
            self.patterns.push(pattern.to_lowercase());
        }
    }

    /// Adds one line of an ignore file.
    ///
    /// * blank lines and `# comments` are skipped
    /// * `.ext` → extension rule (`.tar.gz` becomes the glob `*.tar.gz`)
    /// * anything else → glob/exact-name rule
    pub fn add_entry(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return;
        }
        match line.strip_prefix('.') {
            Some(rest) if !rest.is_empty() && !rest.contains(['.', '*', '?', '/', '\\']) => {
                self.add_extension(rest)
            }
            Some(rest) if !rest.is_empty() && !line.contains(['*', '?']) && rest.contains('.') => {
                self.add_pattern(&format!("*{line}"))
            }
            _ => self.add_pattern(line),
        }
    }

    /// Loads an ignore file (one entry per line); returns how many entries were read.
    pub fn load_file(&mut self, path: &Path) -> Result<usize> {
        let text = fs::read_to_string(path).at(path)?;
        let before = self.extensions.len() + self.patterns.len();
        text.lines().for_each(|line| self.add_entry(line));
        Ok(self.extensions.len() + self.patterns.len() - before)
    }

    /// Returns why an entry named `name` must be skipped, or `None` to process it.
    pub fn check(&self, name: &str, is_dir: bool) -> Option<SkipReason> {
        let lower = name.to_lowercase();
        if SYSTEM_FILES.contains(&lower.as_str()) || lower.starts_with("~$") {
            return Some(SkipReason::SystemFile);
        }
        if !is_dir {
            if let Some(ext) = Path::new(&lower).extension().and_then(|e| e.to_str()) {
                if self.skip_shortcuts && SHORTCUT_EXTENSIONS.contains(&ext) {
                    return Some(SkipReason::Shortcut);
                }
                if self.extensions.contains(ext) {
                    return Some(SkipReason::Ignored(format!(".{ext}")));
                }
            }
        }
        self.patterns
            .iter()
            .find(|p| glob_match(p, &lower))
            .map(|p| SkipReason::Ignored(p.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_basics() {
        assert!(glob_match("*.tmp", "Report.TMP"));
        assert!(glob_match("draft-??.txt", "draft-01.txt"));
        assert!(!glob_match("draft-??.txt", "draft-1.txt"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
        assert!(glob_match("exact.txt", "EXACT.txt"));
    }

    #[test]
    fn shortcuts_skipped_by_default_and_includable() {
        let rules = IgnoreRules::new();
        assert_eq!(rules.check("Chrome.lnk", false), Some(SkipReason::Shortcut));
        assert_eq!(rules.check("Game.URL", false), Some(SkipReason::Shortcut));
        let rules = IgnoreRules::new().include_shortcuts(true);
        assert_eq!(rules.check("Chrome.lnk", false), None);
    }

    #[test]
    fn system_files_always_skipped() {
        let rules = IgnoreRules::new().include_hidden(true).include_shortcuts(true);
        assert_eq!(rules.check("desktop.ini", false), Some(SkipReason::SystemFile));
        assert_eq!(rules.check("~$budget.xlsx", false), Some(SkipReason::SystemFile));
    }

    #[test]
    fn extension_rules_are_normalised_and_file_only() {
        let mut rules = IgnoreRules::new();
        rules.add_extension(".PDF");
        assert!(matches!(rules.check("a.pdf", false), Some(SkipReason::Ignored(_))));
        assert_eq!(rules.check("folder.pdf", true), None);
        assert_eq!(rules.check("a.doc", false), None);
    }

    #[test]
    fn ignore_file_entry_syntax() {
        let mut rules = IgnoreRules::new();
        for line in ["# comment", "", "  .iso ", "*.tmp", "notes.txt", ".tar.gz", "Build*"] {
            rules.add_entry(line);
        }
        assert!(rules.check("x.iso", false).is_some());
        assert!(rules.check("x.tmp", false).is_some());
        assert!(rules.check("NOTES.txt", false).is_some());
        assert!(rules.check("backup.tar.gz", false).is_some());
        assert!(rules.check("Build-output", true).is_some());
        assert!(rules.check("x.txt", false).is_none());
    }

    #[test]
    fn load_file_counts_entries() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ignore.txt");
        fs::write(&file, "# skip\n.iso\n*.tmp\n\n").unwrap();
        let mut rules = IgnoreRules::new();
        assert_eq!(rules.load_file(&file).unwrap(), 2);
        assert!(rules.load_file(&dir.path().join("missing.txt")).is_err());
    }
}
