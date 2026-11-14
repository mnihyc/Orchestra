//! Configured patterns and nested .orch_ignore files.
//!
//! Rules apply from configured patterns through each containing directory, with the deepest
//! matching rule winning. An excluded parent cannot be re-included. Orch's own directory is
//! always available. Invalid or unreadable rules are an explicit coverage failure, never an
//! empty set that could expose files the person intended to exclude.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::io::Read;
use std::sync::{Arc, Mutex, PoisonError};

use ::ignore::Match;
use ::ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::paths::{Locate, ORCH_DIR, RelPath, Workspace};
use crate::platform::{self, Dir, Identity};

/// The local exclusion file, always opened without following a link.
pub const IGNORE_FILE: &str = ".orch_ignore";
/// The default parsing allowance; served environments supply their resolved setting.
pub const IGNORE_FILE_MAX_BYTES: u64 = 1024 * 1024;
/// Cached source bytes and record metadata, with least recently used eviction.
pub const IGNORE_CACHE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleProblem {
    /// Relative file path, or the configured setting key.
    pub path: String,
    /// One-based source line or configured item.
    pub line: Option<usize>,
    /// A safe diagnostic code, never pattern contents or an absolute path.
    pub reason: &'static str,
}

impl RuleProblem {
    pub(crate) fn message(&self) -> String {
        let line = self
            .line
            .map_or_else(String::new, |line| format!(" at line {line}"));
        let reason = match self.reason {
            "linked" => "the rules file is a link",
            "not_file" => "the rules path is not a regular file",
            "too_large" => "the rules exceed the parsing allowance",
            "invalid_encoding" => "the rules are not valid UTF-8",
            "invalid_pattern" => "a pattern is invalid",
            "changed" => "the rules changed while being read",
            _ => "the rules could not be read",
        };
        format!(
            "The exclusion rules in {}{line} could not be applied: {reason}.",
            self.path
        )
    }
}

type Patterns = Result<Option<Arc<Gitignore>>, RuleProblem>;

#[derive(Debug, Clone)]
struct Loaded {
    version: Identity,
    patterns: Patterns,
    age: u64,
    bytes: u64,
}

#[derive(Debug, Default)]
struct Cache {
    files: HashMap<RelPath, Loaded>,
    order: BTreeMap<u64, RelPath>,
    age: u64,
    bytes: u64,
}

impl Cache {
    fn next_age(&mut self) -> u64 {
        if let Some(age) = self.age.checked_add(1) {
            self.age = age;
        } else {
            self.files.clear();
            self.order.clear();
            self.bytes = 0;
            self.age = 1;
        }
        self.age
    }
    fn remove(&mut self, path: &RelPath) {
        if let Some(loaded) = self.files.remove(path) {
            self.order.remove(&loaded.age);
            self.bytes = self.bytes.saturating_sub(loaded.bytes);
        }
    }
    fn get(&mut self, path: &RelPath, version: &Identity) -> Option<Patterns> {
        let age = self.next_age();
        let loaded = self.files.get_mut(path)?;
        if &loaded.version != version {
            return None;
        }
        let result = loaded.patterns.clone();
        self.order.remove(&loaded.age);
        loaded.age = age;
        self.order.insert(age, path.clone());
        Some(result)
    }
    fn keep(
        &mut self,
        path: &RelPath,
        version: Identity,
        patterns: Patterns,
        source_bytes: u64,
        budget: u64,
    ) {
        self.remove(path);
        // Both indices own a path; the fixed record is counted separately from source text.
        let bytes = source_bytes
            .saturating_add((2 * path.as_str().len() + std::mem::size_of::<Loaded>()) as u64);
        if bytes > budget {
            return;
        }
        while self.bytes.saturating_add(bytes) > budget {
            let Some((_, old)) = self.order.pop_first() else {
                break;
            };
            self.remove(&old);
        }
        let age = self.next_age();
        self.bytes = self.bytes.saturating_add(bytes);
        self.order.insert(age, path.clone());
        self.files.insert(
            path.clone(),
            Loaded {
                version,
                patterns,
                age,
                bytes,
            },
        );
    }
}

#[derive(Debug)]
pub struct IgnoreRules {
    configured_input: Vec<String>,
    configured: Option<Arc<Gitignore>>,
    configured_problem: Option<RuleProblem>,
    rejected: Vec<String>,
    case_sensitive: bool,
    max_file_bytes: u64,
    cache_bytes: u64,
    files: Mutex<Cache>,
}

fn builder(case_sensitive: bool) -> GitignoreBuilder {
    let mut builder = GitignoreBuilder::new(".");
    let _ = builder.case_insensitive(!case_sensitive);
    builder
}

impl IgnoreRules {
    #[must_use]
    pub fn new(patterns: &[String], case_sensitive: bool) -> Self {
        Self::with_limits(
            patterns,
            case_sensitive,
            IGNORE_FILE_MAX_BYTES,
            IGNORE_CACHE_BYTES,
        )
    }

    #[must_use]
    pub fn with_limits(
        patterns: &[String],
        case_sensitive: bool,
        max_file_bytes: u64,
        cache_bytes: u64,
    ) -> Self {
        let mut builder = builder(case_sensitive);
        let mut rejected = Vec::new();
        let mut configured_problem = None;
        for (line, pattern) in patterns.iter().enumerate() {
            if builder.add_line(None, pattern).is_err() {
                rejected.push(pattern.clone());
                configured_problem.get_or_insert(RuleProblem {
                    path: "workspace.ignore".to_owned(),
                    line: Some(line + 1),
                    reason: "invalid_pattern",
                });
            }
        }
        let configured = match builder.build() {
            Ok(patterns) => (!patterns.is_empty()).then(|| Arc::new(patterns)),
            Err(_) => {
                configured_problem.get_or_insert(RuleProblem {
                    path: "workspace.ignore".to_owned(),
                    line: None,
                    reason: "invalid_pattern",
                });
                None
            }
        };
        Self {
            configured_input: patterns.to_vec(),
            configured,
            configured_problem,
            rejected,
            case_sensitive,
            max_file_bytes,
            cache_bytes,
            files: Mutex::new(Cache::default()),
        }
    }

    #[must_use]
    pub fn rejected(&self) -> &[String] {
        &self.rejected
    }
    #[must_use]
    pub fn configured_patterns(&self) -> &[String] {
        &self.configured_input
    }
    #[must_use]
    pub(crate) fn max_file_bytes(&self) -> u64 {
        self.max_file_bytes
    }

    pub fn forget(&self, directory: &RelPath) {
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(directory);
    }
    pub(crate) fn forget_all(&self) {
        *self.files.lock().unwrap_or_else(PoisonError::into_inner) = Cache::default();
    }

    fn problem(directory: &RelPath, reason: &'static str, line: Option<usize>) -> RuleProblem {
        RuleProblem {
            path: if directory.is_root() {
                IGNORE_FILE.to_owned()
            } else {
                format!("{directory}/{IGNORE_FILE}")
            },
            reason,
            line,
        }
    }

    fn load(&self, directory: &RelPath, dir: &Dir) -> Patterns {
        let meta = match dir.lstat(OsStr::new(IGNORE_FILE)) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.forget(directory);
                return Ok(None);
            }
            Err(_) => {
                self.forget(directory);
                return Err(Self::problem(directory, "unreadable", None));
            }
        };
        let version = meta.identity();
        if let Some(patterns) = self
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(directory, &version)
        {
            return patterns;
        }
        let patterns = if meta.is_symlink() {
            Err(Self::problem(directory, "linked", None))
        } else if !meta.is_file() {
            Err(Self::problem(directory, "not_file", None))
        } else if meta.size > self.max_file_bytes {
            Err(Self::problem(directory, "too_large", None))
        } else {
            self.read(directory, dir, &version)
                .map(|patterns| patterns.map(Arc::new))
        };
        let source_bytes = if patterns.is_ok() { meta.size } else { 0 };
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keep(
                directory,
                version,
                patterns.clone(),
                source_bytes,
                self.cache_bytes,
            );
        patterns
    }

    fn read(
        &self,
        directory: &RelPath,
        dir: &Dir,
        version: &Identity,
    ) -> Result<Option<Gitignore>, RuleProblem> {
        let mut file = dir
            .open_read(OsStr::new(IGNORE_FILE))
            .map_err(|_| Self::problem(directory, "unreadable", None))?;
        let before =
            platform::file_meta(&file).map_err(|_| Self::problem(directory, "unreadable", None))?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(self.max_file_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| Self::problem(directory, "unreadable", None))?;
        let after =
            platform::file_meta(&file).map_err(|_| Self::problem(directory, "unreadable", None))?;
        if &before.identity() != version || before.identity() != after.identity() {
            return Err(Self::problem(directory, "changed", None));
        }
        if bytes.len() as u64 > self.max_file_bytes {
            return Err(Self::problem(directory, "too_large", None));
        }
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            Self::problem(
                directory,
                "invalid_encoding",
                Some(
                    bytes[..error.valid_up_to()]
                        .iter()
                        .filter(|byte| **byte == b'\n')
                        .count()
                        + 1,
                ),
            )
        })?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut builder = builder(self.case_sensitive);
        for (line, pattern) in text.lines().enumerate() {
            builder
                .add_line(None, pattern)
                .map_err(|_| Self::problem(directory, "invalid_pattern", Some(line + 1)))?;
        }
        let patterns = builder
            .build()
            .map_err(|_| Self::problem(directory, "invalid_pattern", None))?;
        Ok((!patterns.is_empty()).then_some(patterns))
    }

    /// A coverage problem has its own result; boolean consumers conservatively leave the scope out.
    pub fn check(
        &self,
        workspace: &Workspace,
        path: &RelPath,
        is_dir: bool,
    ) -> Result<bool, RuleProblem> {
        if path.has_component(orch_agent_contract::session::INSTRUCTION_ROOT_MARKER) {
            return Ok(true);
        }
        if path.has_component(ORCH_DIR) {
            return Ok(false);
        }
        if let Some(problem) = &self.configured_problem {
            return Err(problem.clone());
        }
        let names: Vec<&str> = path.components().collect();
        let count = names.len();
        let mut bases = Vec::new();
        let mut directory = RelPath::root();
        for depth in 0..count.max(1) {
            if depth > 0 {
                directory = directory
                    .join(names[depth - 1])
                    .map_err(|_| Self::problem(&directory, "unreadable", None))?;
            }
            match self.patterns_of(workspace, &directory) {
                Ok(Some(patterns)) => bases.push((depth, patterns)),
                Ok(None) => {}
                Err(problem) if !is_dir && path.as_str() == problem.path => return Ok(false),
                Err(problem) => return Err(problem),
            }
            if count > 0 && self.decide(&names, depth + 1, depth + 1 < count || is_dir, &bases) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[must_use]
    pub fn is_ignored(&self, workspace: &Workspace, path: &RelPath, is_dir: bool) -> bool {
        self.check(workspace, path, is_dir).unwrap_or(true)
    }

    pub(crate) fn root_problem(&self, workspace: &Workspace) -> Option<RuleProblem> {
        self.check(workspace, &RelPath::root(), true).err()
    }

    fn patterns_of(&self, workspace: &Workspace, directory: &RelPath) -> Patterns {
        let located = match workspace.locate(directory, Locate::THROUGH) {
            Ok(located) => located,
            Err(crate::error::HostError::NotFound(_)) => return Ok(None),
            Err(_) => return Err(Self::problem(directory, "unreadable", None)),
        };
        let dir = workspace
            .open_dir(&located)
            .map_err(|_| Self::problem(directory, "unreadable", None))?;
        self.load(directory, &dir)
    }

    fn decide(
        &self,
        names: &[&str],
        upto: usize,
        is_dir: bool,
        bases: &[(usize, Arc<Gitignore>)],
    ) -> bool {
        let mut decision = None;
        let consider =
            |patterns: &Gitignore, relative: &str| match patterns.matched(relative, is_dir) {
                Match::Ignore(_) => Some(true),
                Match::Whitelist(_) => Some(false),
                Match::None => None,
            };
        if let Some(configured) = &self.configured
            && let Some(found) = consider(configured, &names[..upto].join("/"))
        {
            decision = Some(found);
        }
        for (base, patterns) in bases {
            if *base < upto
                && let Some(found) = consider(patterns, &names[*base..upto].join("/"))
            {
                decision = Some(found);
            }
        }
        decision == Some(true)
    }

    #[must_use]
    pub fn scope(self: &Arc<Self>, root: &Dir) -> Scope {
        let (patterns, problem) = match self.load(&RelPath::root(), root) {
            Ok(patterns) => (patterns, None),
            Err(problem) => (None, Some(problem)),
        };
        Scope {
            rules: Arc::clone(self),
            bases: patterns
                .map(|patterns| vec![(RelPath::root(), patterns)])
                .unwrap_or_default(),
            problem: self.configured_problem.clone().or(problem),
        }
    }

    pub fn scope_at(self: &Arc<Self>, workspace: &Workspace, directory: &RelPath) -> Option<Scope> {
        if directory.has_component(ORCH_DIR) {
            return Some(Scope {
                rules: Arc::clone(self),
                bases: Vec::new(),
                problem: None,
            });
        }
        let mut scope = self.scope(&workspace.root_dir().ok()?);
        let mut here = RelPath::root();
        for name in directory.components() {
            if scope.problem.is_some() {
                return Some(scope);
            }
            here = here.join(name).ok()?;
            if scope.is_ignored(&here, true) {
                return None;
            }
            let located = workspace.locate(&here, Locate::ENTRY).ok()?;
            let dir = workspace.open_dir(&located).ok()?;
            scope = scope.enter(&here, &dir);
        }
        Some(scope)
    }
}

#[derive(Debug, Clone)]
pub struct Scope {
    rules: Arc<IgnoreRules>,
    bases: Vec<(RelPath, Arc<Gitignore>)>,
    problem: Option<RuleProblem>,
}

impl Scope {
    #[must_use]
    pub fn enter(&self, child: &RelPath, dir: &Dir) -> Scope {
        if child.has_component(ORCH_DIR) {
            return Scope {
                rules: Arc::clone(&self.rules),
                bases: Vec::new(),
                problem: None,
            };
        }
        let mut next = self.clone();
        if next.problem.is_none() {
            match self.rules.load(child, dir) {
                Ok(Some(patterns)) => next.bases.push((child.clone(), patterns)),
                Ok(None) => {}
                Err(problem) => next.problem = Some(problem),
            }
        }
        next
    }
    #[must_use]
    pub fn problem(&self) -> Option<&RuleProblem> {
        self.problem.as_ref()
    }
    #[must_use]
    pub fn is_ignored(&self, path: &RelPath, is_dir: bool) -> bool {
        if path.has_component(orch_agent_contract::session::INSTRUCTION_ROOT_MARKER) {
            return true;
        }
        if path.has_component(ORCH_DIR) {
            return false;
        }
        if self.problem.is_some() {
            return true;
        }
        let mut decision = None;
        let consider =
            |patterns: &Gitignore, relative: &str| match patterns.matched(relative, is_dir) {
                Match::Ignore(_) => Some(true),
                Match::Whitelist(_) => Some(false),
                Match::None => None,
            };
        if let Some(configured) = &self.rules.configured
            && let Some(found) = consider(configured, path.as_str())
        {
            decision = Some(found);
        }
        for (base, patterns) in &self.bases {
            if let Some(relative) = path.strip_prefix(base)
                && let Some(found) = consider(patterns, relative.as_str())
            {
                decision = Some(found);
            }
        }
        decision == Some(true)
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::paths::PathStyle;
    use std::path::Path;

    fn workspace(root: &Path, case_sensitive: bool) -> Workspace {
        Workspace::open(root, &[], PathStyle::Posix, case_sensitive).unwrap()
    }

    fn path(text: &str) -> RelPath {
        RelPath::parse(text, PathStyle::Posix).unwrap()
    }

    fn rules(patterns: &[&str], case_sensitive: bool) -> IgnoreRules {
        IgnoreRules::new(
            &patterns.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
            case_sensitive,
        )
    }

    #[test]
    fn configured_patterns_leave_out_what_they_match_and_what_is_below_it() {
        let root = tempfile::tempdir().unwrap();
        let ws = workspace(root.path(), true);
        let rules = rules(&["*.log", "build/", "!important.log"], true);
        assert!(rules.is_ignored(&ws, &path("a.log"), false));
        assert!(rules.is_ignored(&ws, &path("deep/er/b.log"), false));
        assert!(
            !rules.is_ignored(&ws, &path("important.log"), false),
            "a later pattern brings it back"
        );
        assert!(rules.is_ignored(&ws, &path("build"), true));
        assert!(
            rules.is_ignored(&ws, &path("build/out/x.o"), false),
            "everything below a directory that is left out"
        );
        assert!(
            !rules.is_ignored(&ws, &path("build"), false),
            "build/ names directories only"
        );
        assert!(!rules.is_ignored(&ws, &path("src/main.rs"), false));
        assert!(!rules.is_ignored(&ws, &RelPath::root(), true));
    }

    #[test]
    fn a_nested_file_overrides_the_ones_above_it_for_its_own_directory_and_below() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("sub/deep")).unwrap();
        std::fs::write(root.path().join(".orch_ignore"), "*.tmp\n").unwrap();
        std::fs::write(
            root.path().join("sub/.orch_ignore"),
            "!keep.tmp\n/only-here\n*.cache\n",
        )
        .unwrap();
        let ws = workspace(root.path(), true);
        let rules = rules(&["*.log"], true);
        assert!(
            rules.is_ignored(&ws, &path("x.tmp"), false),
            "the file at the root"
        );
        assert!(rules.is_ignored(&ws, &path("sub/x.tmp"), false));
        assert!(
            !rules.is_ignored(&ws, &path("sub/keep.tmp"), false),
            "the deeper file wins"
        );
        assert!(
            !rules.is_ignored(&ws, &path("sub/deep/keep.tmp"), false),
            "and its scope reaches below"
        );
        assert!(rules.is_ignored(&ws, &path("sub/only-here"), false));
        assert!(
            !rules.is_ignored(&ws, &path("sub/deep/only-here"), false),
            "a pattern with a slash is anchored to its file"
        );
        assert!(rules.is_ignored(&ws, &path("sub/deep/z.cache"), false));
        assert!(
            !rules.is_ignored(&ws, &path("z.cache"), false),
            "and does not reach above its file"
        );
        assert!(
            rules.is_ignored(&ws, &path("sub/deep/y.log"), false),
            "the configured patterns still apply"
        );
    }

    #[test]
    fn nothing_inside_a_directory_that_is_left_out_can_be_brought_back() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("vendor")).unwrap();
        std::fs::write(root.path().join("vendor/.orch_ignore"), "!wanted\n").unwrap();
        let ws = workspace(root.path(), true);
        let rules = rules(&["vendor/"], true);
        assert!(rules.is_ignored(&ws, &path("vendor/wanted"), false));
    }

    #[test]
    fn the_directory_orch_keeps_in_a_threads_directory_is_never_left_out() {
        let root = tempfile::tempdir().unwrap();
        let ws = workspace(root.path(), true);
        let rules = rules(&["*", ".*", ".orch"], true);
        assert!(rules.is_ignored(&ws, &path("notes.txt"), false));
        assert!(!rules.is_ignored(&ws, &path(".orch"), true));
        assert!(!rules.is_ignored(&ws, &path("web/.orch/memory/MEMORY.md"), false));
    }

    #[test]
    fn upper_and_lower_case_are_the_same_where_the_file_system_says_so() {
        let root = tempfile::tempdir().unwrap();
        let ws = workspace(root.path(), false);
        let insensitive = rules(&["secret/", "*.KEY"], false);
        assert!(
            insensitive.is_ignored(&ws, &path("SECRET/token"), false),
            "a case trick does not get past it"
        );
        assert!(insensitive.is_ignored(&ws, &path("a/b.key"), false));
        let sensitive = rules(&["secret/"], true);
        assert!(!sensitive.is_ignored(&workspace(root.path(), true), &path("SECRET/token"), false));
    }

    #[test]
    fn a_file_of_patterns_is_read_again_when_it_changes_and_never_through_a_link() {
        let root = tempfile::tempdir().unwrap();
        let ws = workspace(root.path(), true);
        let rules = rules(&[], true);
        assert!(!rules.is_ignored(&ws, &path("x.out"), false));
        std::fs::write(root.path().join(".orch_ignore"), "*.out\n").unwrap();
        assert!(
            rules.is_ignored(&ws, &path("x.out"), false),
            "a new rule takes effect at once"
        );
        std::fs::write(root.path().join(".orch_ignore"), "*.other\n").unwrap();
        assert!(
            !rules.is_ignored(&ws, &path("x.out"), false),
            "and so does a rule that was taken away"
        );
        std::fs::remove_file(root.path().join(".orch_ignore")).unwrap();
        std::fs::write(root.path().join("patterns-elsewhere"), "*.out\n").unwrap();
        std::os::unix::fs::symlink("patterns-elsewhere", root.path().join(".orch_ignore")).unwrap();
        assert!(
            rules.is_ignored(&ws, &path("x.out"), false),
            "a linked rule file leaves coverage unavailable until corrected"
        );
    }

    #[test]
    fn a_walk_sees_the_same_rules_as_a_single_question() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("sub")).unwrap();
        std::fs::write(root.path().join("sub/.orch_ignore"), "!keep.log\n").unwrap();
        let ws = workspace(root.path(), true);
        let rules = Arc::new(rules(&["*.log"], true));
        let at_root = rules.scope(&ws.root_dir().unwrap());
        let sub_dir = ws
            .root_dir()
            .unwrap()
            .open_child(OsStr::new("sub"))
            .unwrap();
        let in_sub = at_root.enter(&path("sub"), &sub_dir);
        assert!(at_root.is_ignored(&path("a.log"), false));
        assert!(in_sub.is_ignored(&path("sub/other.log"), false));
        assert!(!in_sub.is_ignored(&path("sub/keep.log"), false));
        for (probe, is_dir) in [
            ("a.log", false),
            ("sub/other.log", false),
            ("sub/keep.log", false),
        ] {
            assert_eq!(
                rules.is_ignored(&ws, &path(probe), is_dir),
                if probe.starts_with("sub") {
                    in_sub.is_ignored(&path(probe), is_dir)
                } else {
                    at_root.is_ignored(&path(probe), is_dir)
                }
            );
        }
    }

    #[test]
    fn an_unclosed_character_class_keeps_its_git_literal_meaning() {
        let found = rules(&["*.log", "[unclosed", "ok/"], true);
        // An unclosed class is read literally, as git does; the rest are unaffected either way.
        let root = tempfile::tempdir().unwrap();
        let ws = workspace(root.path(), true);
        assert!(found.is_ignored(&ws, &path("a.log"), false));
        assert!(found.is_ignored(&ws, &path("ok"), true));
    }
    #[test]
    fn an_invalid_exclusion_line_is_named_and_its_scope_never_exposes_content() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(IGNORE_FILE), "*.private\n[z-a]\n").unwrap();
        let ws = workspace(root.path(), true);
        let rules = Arc::new(rules(&[], true));
        let problem = rules.check(&ws, &path("visible.txt"), false).unwrap_err();
        assert_eq!(problem.path, IGNORE_FILE);
        assert_eq!(problem.line, Some(2));
        assert_eq!(problem.reason, "invalid_pattern");
        assert!(rules.is_ignored(&ws, &path("visible.txt"), false));
        let scope = rules.scope(&ws.root_dir().unwrap());
        assert_eq!(scope.problem(), Some(&problem));
        assert!(scope.is_ignored(&path("visible.txt"), false));
        assert!(
            !rules.check(&ws, &path(IGNORE_FILE), false).unwrap(),
            "the broken control file can still be corrected"
        );
    }

    #[test]
    fn invalid_configured_exclusions_are_not_partially_applied_as_if_every_rule_was_valid() {
        let root = tempfile::tempdir().unwrap();
        let ws = workspace(root.path(), true);
        let rules = rules(&["*.private", "[z-a]"], true);
        let problem = rules.check(&ws, &path("visible.txt"), false).unwrap_err();
        assert_eq!(problem.path, "workspace.ignore");
        assert_eq!(problem.line, Some(2));
        assert!(rules.is_ignored(&ws, &path("visible.txt"), false));
    }

    #[test]
    fn an_exclusion_parse_budget_is_reported_and_a_larger_chosen_budget_applies_the_complete_file()
    {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(IGNORE_FILE), "hidden.txt\n").unwrap();
        let ws = workspace(root.path(), true);
        let small = IgnoreRules::with_limits(&[], true, 4, 1024);
        let problem = small.check(&ws, &path("hidden.txt"), false).unwrap_err();
        assert_eq!(problem.reason, "too_large");
        let chosen = IgnoreRules::with_limits(&[], true, 100, 1024);
        assert!(chosen.check(&ws, &path("hidden.txt"), false).unwrap());
    }

    #[test]
    fn invalid_text_and_linked_exclusion_files_are_reported_until_the_actual_rules_are_readable() {
        let root = tempfile::tempdir().unwrap();
        let ws = workspace(root.path(), true);
        let rules = rules(&[], true);
        std::fs::write(root.path().join(IGNORE_FILE), [255u8]).unwrap();
        assert_eq!(
            rules
                .check(&ws, &path("visible.txt"), false)
                .unwrap_err()
                .reason,
            "invalid_encoding"
        );
        std::fs::remove_file(root.path().join(IGNORE_FILE)).unwrap();
        std::fs::write(root.path().join("actual-rules"), "*.private\n").unwrap();
        std::os::unix::fs::symlink("actual-rules", root.path().join(IGNORE_FILE)).unwrap();
        assert_eq!(
            rules
                .check(&ws, &path("visible.txt"), false)
                .unwrap_err()
                .reason,
            "linked"
        );
        std::fs::remove_file(root.path().join(IGNORE_FILE)).unwrap();
        std::fs::write(root.path().join(IGNORE_FILE), "*.private\n").unwrap();
        assert!(!rules.check(&ws, &path("visible.txt"), false).unwrap());
        assert!(rules.check(&ws, &path("hidden.private"), false).unwrap());
    }
}
