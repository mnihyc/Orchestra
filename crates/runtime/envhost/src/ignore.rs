//! Configured patterns and nested .orch_ignore files.
//!
//! What the workspace leaves out of what Orch shows is decided by gitignore patterns from two
//! places: the patterns the environment was attached with (`workspace.ignore`, written as if in a
//! file at the root) and every `.orch_ignore` file in the workspace, each scoping its own
//! directory and below. The rules are git's: the last pattern that matches decides, a pattern
//! with a leading `!` brings back what an earlier one left out, and nothing inside a directory
//! that is left out can be brought back.
//!
//! **Precedence** runs from the configured patterns, which lose to everything, through the file
//! at the root, down to the file in the entry's own directory, which wins; a deeper file
//! overrides a shallower one as git's do. A path is ignored when it, or any directory above it,
//! is. The directory Orch keeps inside a thread's directory (`.orch/`) is never ignored,
//! whatever a pattern says. Where names do not differ by case, neither do the patterns.
//!
//! An ignored path is absent from listings, search, manifests and snapshots, and cannot be read or
//! written through Orch. The agent itself is unaffected: these rules decide what Orch shows, not
//! what exists.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::sync::{Arc, Mutex, PoisonError};

use ::ignore::Match;
use ::ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::paths::{Locate, ORCH_DIR, RelPath, Workspace};
use crate::platform::{Dir, Identity};

/// The name of a file of patterns inside the workspace.
pub const IGNORE_FILE: &str = ".orch_ignore";

/// The most bytes of a file of patterns that are read.
///
/// A file of patterns is written by hand and a few kilobytes long; one of a mebibyte is a mistake
/// or an attack on the time every request spends reading it, and is not read at all.
pub const IGNORE_FILE_MAX_BYTES: u64 = 1024 * 1024;

/// What the patterns say of one name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Ignore,
    Keep,
    Silent,
}

/// The parsed patterns of one directory's file, and which version of the file they were read from.
#[derive(Debug, Clone)]
struct Loaded {
    version: Option<Identity>,
    patterns: Option<Arc<Gitignore>>,
}

/// The rules of one workspace.
#[derive(Debug)]
pub struct IgnoreRules {
    configured_input: Vec<String>,
    configured: Option<Arc<Gitignore>>,
    rejected: Vec<String>,
    case_sensitive: bool,
    files: Mutex<HashMap<RelPath, Loaded>>,
}

fn builder(case_sensitive: bool) -> GitignoreBuilder {
    // The root is `.`, and every path matched against it is written from the directory the
    // patterns belong to: the matcher then never strips anything from the path it is given.
    let mut builder = GitignoreBuilder::new(".");
    // Setting this cannot fail; the crate returns a result for an old reason.
    let _ = builder.case_insensitive(!case_sensitive);
    builder
}

impl IgnoreRules {
    /// The rules of a workspace whose environment was attached with `patterns`. A pattern that is
    /// not a pattern is left out and listed by [`IgnoreRules::rejected`].
    #[must_use]
    pub fn new(patterns: &[String], case_sensitive: bool) -> Self {
        let mut builder = builder(case_sensitive);
        let mut rejected = Vec::new();
        for pattern in patterns {
            if builder.add_line(None, pattern).is_err() {
                rejected.push(pattern.clone());
            }
        }
        let configured = builder
            .build()
            .ok()
            .filter(|set| !set.is_empty())
            .map(Arc::new);
        Self {
            configured_input: patterns.to_vec(),
            configured,
            rejected,
            case_sensitive,
            files: Mutex::new(HashMap::new()),
        }
    }

    /// The configured patterns that could not be read as patterns.
    #[must_use]
    pub fn rejected(&self) -> &[String] {
        &self.rejected
    }

    /// The frozen configured rules, for a file state's coverage fingerprint.
    #[must_use]
    pub fn configured_patterns(&self) -> &[String] {
        &self.configured_input
    }

    /// Forgets what was read from the file of `directory`, after somebody changed it.
    pub fn forget(&self, directory: &RelPath) {
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(directory);
    }

    /// Drops cached rule parses when native event loss makes their version evidence incomplete.
    pub(crate) fn forget_all(&self) {
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// The patterns of the file in `dir`, read again when the file is another version than the one
    /// that was parsed.
    fn load(&self, directory: &RelPath, dir: &Dir) -> Option<Arc<Gitignore>> {
        let name = OsStr::new(IGNORE_FILE);
        let version = match dir.lstat(name) {
            Ok(meta) if meta.is_file() && meta.size <= IGNORE_FILE_MAX_BYTES => {
                Some(meta.identity())
            }
            _ => None,
        };
        {
            let files = self.files.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(loaded) = files.get(directory)
                && loaded.version == version
            {
                return loaded.patterns.clone();
            }
        }
        let patterns = version.and_then(|_| self.read(dir)).map(Arc::new);
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                directory.clone(),
                Loaded {
                    version,
                    patterns: patterns.clone(),
                },
            );
        patterns
    }

    fn read(&self, dir: &Dir) -> Option<Gitignore> {
        // Opened without following a link: a link called .orch_ignore is not a file of patterns.
        let mut file = dir.open_read(OsStr::new(IGNORE_FILE)).ok()?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(IGNORE_FILE_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > IGNORE_FILE_MAX_BYTES {
            return None;
        }
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut builder = builder(self.case_sensitive);
        for line in text.lines() {
            // A line that is no pattern costs the file that line and nothing more.
            let _ = builder.add_line(None, line);
        }
        builder.build().ok().filter(|set| !set.is_empty())
    }

    /// Whether `path` is left out. It is, when it or a directory above it is; `is_dir` says what
    /// the path itself is, since a pattern may name directories only. `path` may be spelled with
    /// links in it, and the answer is for the path as it is spelled: ask for the real path of
    /// the entry too, to catch a link that leads to a place that is left out.
    #[must_use]
    pub fn is_ignored(&self, workspace: &Workspace, path: &RelPath, is_dir: bool) -> bool {
        if path.has_component(orch_agent_contract::session::INSTRUCTION_ROOT_MARKER) {
            return true;
        }
        if path.is_root() || path.has_component(ORCH_DIR) {
            return false;
        }
        let names: Vec<&str> = path.components().collect();
        let count = names.len();
        // The pattern files that apply, from the root down, found as the walk goes.
        let mut bases: Vec<(usize, Arc<Gitignore>)> = Vec::new();
        let mut directory = RelPath::root();
        for depth in 0..count {
            if depth > 0 {
                match directory.join(names[depth - 1]) {
                    Ok(next) => directory = next,
                    Err(_) => return true,
                }
            }
            if let Some(patterns) = self.patterns_of(workspace, &directory) {
                bases.push((depth, patterns));
            }
            let entry_is_dir = depth + 1 < count || is_dir;
            if self.decide(&names, depth + 1, entry_is_dir, &bases) == Decision::Ignore {
                return true;
            }
        }
        false
    }

    fn patterns_of(&self, workspace: &Workspace, directory: &RelPath) -> Option<Arc<Gitignore>> {
        let located = workspace.locate(directory, Locate::THROUGH).ok()?;
        let dir = workspace.open_dir(&located).ok()?;
        self.load(directory, &dir)
    }

    /// What the patterns say of the first `upto` names, given the pattern files above them.
    fn decide(
        &self,
        names: &[&str],
        upto: usize,
        is_dir: bool,
        bases: &[(usize, Arc<Gitignore>)],
    ) -> Decision {
        let mut decision = Decision::Silent;
        let from_root = names[..upto].join("/");
        let consider =
            |patterns: &Gitignore, relative: &str| match patterns.matched(relative, is_dir) {
                Match::Ignore(_) => Some(Decision::Ignore),
                Match::Whitelist(_) => Some(Decision::Keep),
                Match::None => None,
            };
        if let Some(configured) = &self.configured
            && let Some(found) = consider(configured, &from_root)
        {
            decision = found;
        }
        for (base, patterns) in bases {
            if *base >= upto {
                continue;
            }
            let relative = names[*base..upto].join("/");
            if let Some(found) = consider(patterns, &relative) {
                decision = found;
            }
        }
        decision
    }

    /// The rules as a walk of a tree sees them, starting at the root.
    #[must_use]
    pub fn scope(self: &Arc<Self>, root: &Dir) -> Scope {
        let patterns = self.load(&RelPath::root(), root);
        Scope {
            rules: Arc::clone(self),
            bases: patterns
                .map(|patterns| vec![(RelPath::root(), patterns)])
                .unwrap_or_default(),
        }
    }

    /// The rules inside `directory`, for a walk that starts there; `None` when the directory is
    /// left out itself, or something above it is, and the walk has nothing to see. `directory`
    /// is spelled as it really is, with no link in it.
    pub fn scope_at(self: &Arc<Self>, workspace: &Workspace, directory: &RelPath) -> Option<Scope> {
        let mut scope = self.scope(&workspace.root_dir().ok()?);
        let mut here = RelPath::root();
        // A path that runs through `.orch/` is Orch's own and is not left out, whatever lies above it.
        let exempt = directory.has_component(ORCH_DIR);
        for name in directory.components() {
            here = here.join(name).ok()?;
            if !exempt && scope.is_ignored(&here, true) {
                return None;
            }
            let located = workspace.locate(&here, Locate::ENTRY).ok()?;
            let dir = workspace.open_dir(&located).ok()?;
            scope = scope.enter(&here, &dir);
        }
        Some(scope)
    }
}

/// The rules in force inside one directory of a walk: the walk enters a directory, learns its
/// file of patterns once, and then asks about each name in it.
#[derive(Debug, Clone)]
pub struct Scope {
    rules: Arc<IgnoreRules>,
    bases: Vec<(RelPath, Arc<Gitignore>)>,
}

impl Scope {
    /// The rules inside `child`, a directory of this one that is held as `dir`.
    #[must_use]
    pub fn enter(&self, child: &RelPath, dir: &Dir) -> Scope {
        let mut bases = self.bases.clone();
        if let Some(patterns) = self.rules.load(child, dir) {
            bases.push((child.clone(), patterns));
        }
        Scope {
            rules: Arc::clone(&self.rules),
            bases,
        }
    }

    /// Whether the entry `path`, which lies directly in the directory this scope is for, is left
    /// out. The directory itself is known not to be, or the walk would not be here.
    #[must_use]
    pub fn is_ignored(&self, path: &RelPath, is_dir: bool) -> bool {
        if path.has_component(orch_agent_contract::session::INSTRUCTION_ROOT_MARKER) {
            return true;
        }
        if path.has_component(ORCH_DIR) {
            return false;
        }
        let consider =
            |patterns: &Gitignore, relative: &str| match patterns.matched(relative, is_dir) {
                Match::Ignore(_) => Some(Decision::Ignore),
                Match::Whitelist(_) => Some(Decision::Keep),
                Match::None => None,
            };
        let mut decision = Decision::Silent;
        if let Some(configured) = &self.rules.configured
            && let Some(found) = consider(configured, path.as_str())
        {
            decision = found;
        }
        for (base, patterns) in &self.bases {
            if let Some(relative) = path.strip_prefix(base)
                && let Some(found) = consider(patterns, relative.as_str())
            {
                decision = found;
            }
        }
        decision == Decision::Ignore
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
            !rules.is_ignored(&ws, &path("x.out"), false),
            "a link is not a file of patterns"
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
    fn a_pattern_that_is_no_pattern_is_set_aside_and_the_rest_still_apply() {
        let found = rules(&["*.log", "[unclosed", "ok/"], true);
        // An unclosed class is read literally, as git does; the rest are unaffected either way.
        let root = tempfile::tempdir().unwrap();
        let ws = workspace(root.path(), true);
        assert!(found.is_ignored(&ws, &path("a.log"), false));
        assert!(found.is_ignored(&ws, &path("ok"), true));
    }
}
