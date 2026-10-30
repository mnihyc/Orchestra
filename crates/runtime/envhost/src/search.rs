//! The search inside files: the lines of text files under a directory that hold a word or match a bounded pattern.
//!
//! The search a person starts in the files panel and the one an agent's file tools have are one
//! search. It walks the directory the way every tree is walked here (never into a link, nothing
//! the ignore rules leave out) and skips what a `.gitignore` leaves out, unless a glob names the
//! file. It reads text files no larger than the preview size, as UTF-8, and reports each line that
//! holds the query with where in the line it does.
//!
//! It is held to bounds on every side and says when it stopped short: at most the number of
//! matches asked for (never more than a thousand), within twenty seconds, with a pattern whose
//! compiled form is bounded (the pattern language has no backtracking, so no pattern can take
//! longer than the text it is run on), and only the first 400 characters around a match of a long
//! line are returned.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::rc::Rc;
use std::time::Instant;

use ::ignore::Match;
use ::ignore::gitignore::{Gitignore, GitignoreBuilder};
use globset::{GlobBuilder, GlobMatcher};
use orch_api_contract::workspace::{FILE_MATCH_TEXT_CHARS, FILE_SEARCH_LIMIT_MAX, FileMatch};
use regex::{Regex, RegexBuilder};

use crate::error::{HostError, Result};
use crate::files::Files;
use crate::media::{SNIFF_BYTES, is_text};
use crate::paths::{Locate, RelPath, Workspace};
use crate::platform::{Dir, read_at};
use crate::walk::{Entry, Flow, walk};

/// The longest query or glob that is accepted, in characters. A word or a pattern typed by a
/// person is a line at most; a megabyte of pattern is an attack on the time of the search.
pub const QUERY_MAX_CHARS: usize = 1000;

/// The most memory a compiled pattern may take. A mebibyte holds any pattern a person writes and
/// refuses the ones that expand into millions of states.
const PATTERN_SIZE_LIMIT_BYTES: usize = 1024 * 1024;

/// The most memory the matching engine may build lazily while it runs.
const PATTERN_CACHE_LIMIT_BYTES: usize = 4 * 1024 * 1024;

/// How deeply a pattern may nest groups.
const PATTERN_NEST_LIMIT: u32 = 64;

/// The most bytes of a file of `.gitignore` patterns that are read.
const GITIGNORE_MAX_BYTES: u64 = 1024 * 1024;

/// What to look for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    /// The word, or the pattern when `regex` is set.
    pub query: String,
    /// Whether the query is a regular expression.
    pub regex: bool,
    /// Whether upper and lower case differ.
    pub case_sensitive: bool,
    /// Which files to look in: a glob on the file name, or on the path below the directory when
    /// it holds a `/`.
    pub glob: Option<String>,
    /// The most matches to return.
    pub limit: usize,
}

/// What a search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    /// The lines that hold the query, in the order of the files.
    pub matches: Vec<FileMatch>,
    /// Whether the search stopped before it had seen everything.
    pub truncated: bool,
    /// How many files it looked in.
    pub searched: i64,
}

fn compile(request: &SearchRequest) -> Result<Regex> {
    if request.query.is_empty() {
        return Err(HostError::Unsupported(
            "a search needs something to look for".to_owned(),
        ));
    }
    if request.query.chars().count() > QUERY_MAX_CHARS {
        return Err(HostError::Unsupported(format!(
            "a query has at most {QUERY_MAX_CHARS} characters"
        )));
    }
    let pattern = if request.regex {
        request.query.clone()
    } else {
        regex::escape(&request.query)
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!request.case_sensitive)
        .size_limit(PATTERN_SIZE_LIMIT_BYTES)
        .dfa_size_limit(PATTERN_CACHE_LIMIT_BYTES)
        .nest_limit(PATTERN_NEST_LIMIT)
        .build()
        .map_err(|error| HostError::Unsupported(format!("that pattern is not accepted: {error}")))
}

struct Glob {
    matcher: GlobMatcher,
    on_path: bool,
}

fn compile_glob(text: &str) -> Result<Glob> {
    if text.chars().count() > QUERY_MAX_CHARS {
        return Err(HostError::Unsupported(format!(
            "a glob has at most {QUERY_MAX_CHARS} characters"
        )));
    }
    let glob = GlobBuilder::new(text)
        .literal_separator(true)
        .build()
        .map_err(|error| HostError::Unsupported(format!("that glob is not accepted: {error}")))?;
    Ok(Glob {
        matcher: glob.compile_matcher(),
        on_path: text.contains('/'),
    })
}

/// The `.gitignore` files that apply at each directory of the search, from the workspace root down.
struct GitIgnores<'a> {
    workspace: &'a Workspace,
    chains: HashMap<RelPath, Rc<Vec<(RelPath, Gitignore)>>>,
}

impl<'a> GitIgnores<'a> {
    fn new(workspace: &'a Workspace) -> Self {
        Self {
            workspace,
            chains: HashMap::new(),
        }
    }

    fn load(&self, dir: &Dir) -> Option<Gitignore> {
        let mut file = dir.open_read(OsStr::new(".gitignore")).ok()?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(GITIGNORE_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > GITIGNORE_MAX_BYTES {
            return None;
        }
        let mut builder = GitignoreBuilder::new(".");
        let _ = builder.case_insensitive(!self.workspace.case_sensitive());
        for line in String::from_utf8_lossy(&bytes).lines() {
            let _ = builder.add_line(None, line);
        }
        builder.build().ok().filter(|patterns| !patterns.is_empty())
    }

    /// The files that apply inside `directory`. `held` is the directory open, when the walk has it.
    fn chain(&mut self, directory: &RelPath, held: Option<&Dir>) -> Rc<Vec<(RelPath, Gitignore)>> {
        if let Some(chain) = self.chains.get(directory) {
            return Rc::clone(chain);
        }
        let mut chain: Vec<(RelPath, Gitignore)> = match directory.parent() {
            Some(parent) => self.chain(&parent, None).as_ref().clone(),
            None => Vec::new(),
        };
        let own = match held {
            Some(dir) => self.load(dir),
            None => self
                .workspace
                .locate(directory, Locate::ENTRY)
                .ok()
                .and_then(|located| self.workspace.open_dir(&located).ok())
                .and_then(|dir| self.load(&dir)),
        };
        if let Some(patterns) = own {
            chain.push((directory.clone(), patterns));
        }
        let chain = Rc::new(chain);
        self.chains.insert(directory.clone(), Rc::clone(&chain));
        chain
    }

    /// Whether a `.gitignore` leaves out `path`, which lies directly in `dir`.
    fn leaves_out(&mut self, path: &RelPath, is_dir: bool, dir: &Dir) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        let chain = self.chain(&parent, Some(dir));
        let mut left_out = false;
        for (base, patterns) in chain.iter() {
            if let Some(relative) = path.strip_prefix(base) {
                match patterns.matched(relative.as_str(), is_dir) {
                    Match::Ignore(_) => left_out = true,
                    Match::Whitelist(_) => left_out = false,
                    Match::None => {}
                }
            }
        }
        left_out
    }
}

/// The part of a long line that is returned, and where the matches lie in it, counted in characters.
fn excerpt(line: &str, found: &[(usize, usize)]) -> (String, Vec<(u32, u32)>) {
    let total = line.chars().count();
    let to_char = |byte: usize| line[..byte].chars().count();
    let spans: Vec<(usize, usize)> = found
        .iter()
        .map(|&(start, end)| (to_char(start), to_char(end)))
        .collect();
    let limit = FILE_MATCH_TEXT_CHARS as usize;
    if total <= limit {
        return (
            line.to_owned(),
            spans.iter().map(|&(s, e)| (s as u32, e as u32)).collect(),
        );
    }
    // A window of the limit's size, placed so that the first match lies in the middle of it.
    let (first_start, first_end) = spans.first().copied().unwrap_or((0, 0));
    let middle = usize::midpoint(first_start, first_end);
    let begin = middle.saturating_sub(limit / 2).min(total - limit);
    let end = begin + limit;
    let text: String = line.chars().skip(begin).take(limit).collect();
    let ranges = spans
        .iter()
        .filter(|&&(s, e)| s >= begin && e <= end)
        .map(|&(s, e)| ((s - begin) as u32, (e - begin) as u32))
        .collect();
    (text, ranges)
}

struct Run<'a> {
    pattern: &'a Regex,
    glob: Option<&'a Glob>,
    start: &'a RelPath,
    limit: usize,
    preview_bytes: u64,
    deadline: Instant,
    matches: Vec<FileMatch>,
    truncated: bool,
    searched: i64,
}

impl Run<'_> {
    fn matches_glob(&self, entry: &Entry<'_>) -> bool {
        match self.glob {
            None => true,
            Some(glob) if glob.on_path => entry
                .path
                .strip_prefix(self.start)
                .is_some_and(|below| glob.matcher.is_match(below.as_str())),
            Some(glob) => glob.matcher.is_match(entry.name),
        }
    }

    /// Looks into one regular file; says whether the search is over.
    fn look_into(&mut self, entry: &Entry<'_>) -> bool {
        if entry.meta.size > self.preview_bytes {
            return false;
        }
        let Ok(file) = entry.parent.open_read(entry.name) else {
            return false;
        };
        let mut head = vec![0u8; SNIFF_BYTES];
        let Ok(count) = read_at(&file, &mut head, 0) else {
            return false;
        };
        if !is_text(&head[..count]) {
            return false;
        }
        let mut bytes = Vec::new();
        let mut reader = &file;
        if Read::by_ref(&mut reader)
            .take(self.preview_bytes + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return false;
        }
        // The head was read by position and this read starts at the cursor, which is at the start.
        if bytes.len() as u64 > self.preview_bytes {
            return false;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            return false;
        };
        self.searched += 1;
        for (number, line) in text.lines().enumerate() {
            let found: Vec<(usize, usize)> = self
                .pattern
                .find_iter(line)
                .map(|m| (m.start(), m.end()))
                .collect();
            if found.is_empty() {
                continue;
            }
            if self.matches.len() >= self.limit {
                // One more than was asked for exists: the result is cut and says so.
                self.truncated = true;
                return true;
            }
            let (text, ranges) = excerpt(line, &found);
            self.matches.push(FileMatch {
                path: entry.path.to_string(),
                line: i64::try_from(number + 1).unwrap_or(i64::MAX),
                text,
                ranges,
            });
        }
        false
    }
}

impl Files {
    /// Searches the text files at or under `path` for the query. The search stops at its limit of
    /// matches and at its time, and says so.
    pub fn search(&self, path: &RelPath, request: &SearchRequest) -> Result<SearchResult> {
        let pattern = compile(request)?;
        let glob = request.glob.as_deref().map(compile_glob).transpose()?;
        let limit = request.limit.clamp(1, FILE_SEARCH_LIMIT_MAX as usize);
        let workspace = self.root();
        let located = workspace.locate(path, Locate::THROUGH)?;
        let meta = located
            .meta
            .ok_or_else(|| HostError::NotFound(format!("{path} is not there")))?;
        self.check_visible(path, &located, meta.is_dir())?;
        let mut run = Run {
            pattern: &pattern,
            glob: glob.as_ref(),
            start: &located.real,
            limit,
            preview_bytes: self.limits().preview_bytes,
            deadline: Instant::now() + self.limits().search_time,
            matches: Vec::new(),
            truncated: false,
            searched: 0,
        };
        if meta.is_file() {
            let name = located.name()?;
            let entry = Entry {
                path: located.real.clone(),
                name,
                parent: &located.parent,
                meta,
                link_target: None,
            };
            if run.matches_glob(&entry) {
                run.look_into(&entry);
            }
        } else if meta.is_dir() {
            let mut git = GitIgnores::new(workspace);
            walk(workspace, self.ignore(), &located.real, &mut |entry| {
                if Instant::now() >= run.deadline {
                    run.truncated = true;
                    return Flow::Stop;
                }
                if entry.name == ".git" {
                    return Flow::SkipDir;
                }
                let is_dir = entry.meta.is_dir();
                if git.leaves_out(&entry.path, is_dir, entry.parent) {
                    // A glob that names a file brings it back; a directory that is left out stays out.
                    if is_dir || !(run.glob.is_some() && run.matches_glob(entry)) {
                        return if is_dir {
                            Flow::SkipDir
                        } else {
                            Flow::Continue
                        };
                    }
                }
                if !entry.meta.is_file() || !run.matches_glob(entry) {
                    return Flow::Continue;
                }
                if run.look_into(entry) {
                    Flow::Stop
                } else {
                    Flow::Continue
                }
            })?;
        } else {
            return Err(HostError::Unsupported(format!(
                "{path} is neither a file nor a directory"
            )));
        }
        Ok(SearchResult {
            matches: run.matches,
            truncated: run.truncated,
            searched: run.searched,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{GateState, Gates, Limits};
    use crate::ignore::IgnoreRules;
    use crate::index::HashCache;
    use crate::paths::PathStyle;

    fn native_style() -> PathStyle {
        if cfg!(windows) {
            PathStyle::Windows
        } else {
            PathStyle::Posix
        }
    }
    use std::sync::Arc;
    use std::time::Duration;

    fn files_with(root: &std::path::Path, patterns: &[&str], limits: Limits) -> Files {
        let ws = Workspace::open(root, &[], native_style(), true).unwrap();
        let rules = Arc::new(IgnoreRules::new(
            &patterns.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
            true,
        ));
        Files::workspace(
            ws,
            rules,
            Arc::new(HashCache::new()),
            limits,
            Arc::new(GateState::new(Gates::OPEN)),
        )
    }

    fn request(query: &str) -> SearchRequest {
        SearchRequest {
            query: query.to_owned(),
            regex: false,
            case_sensitive: false,
            glob: None,
            limit: 200,
        }
    }

    fn path(text: &str) -> RelPath {
        RelPath::parse(text, native_style()).unwrap()
    }

    fn tree() -> (tempfile::TempDir, Files) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src/deep")).unwrap();
        std::fs::create_dir_all(root.path().join("target")).unwrap();
        std::fs::create_dir_all(root.path().join(".git")).unwrap();
        std::fs::write(
            root.path().join("src/main.rs"),
            "fn main() {\n    println!(\"Hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("src/deep/lib.rs"),
            "// hello again\npub fn hello() {}\n",
        )
        .unwrap();
        std::fs::write(root.path().join("src/notes.txt"), "say HELLO to all\n").unwrap();
        std::fs::write(root.path().join("target/out.txt"), "hello from a build\n").unwrap();
        std::fs::write(root.path().join(".git/config"), "hello inside git\n").unwrap();
        std::fs::write(root.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::write(
            root.path().join("src/blob.bin"),
            [b'h', b'e', b'l', b'l', b'o', 0, 1, 2],
        )
        .unwrap();
        let files = files_with(root.path(), &[], Limits::default());
        (root, files)
    }

    #[test]
    fn a_word_is_found_in_text_files_with_its_line_and_where_in_the_line() {
        let (_root, files) = tree();
        let found = files.search(&RelPath::root(), &request("hello")).unwrap();
        let places: Vec<(String, i64)> = found
            .matches
            .iter()
            .map(|m| (m.path.clone(), m.line))
            .collect();
        assert_eq!(
            places,
            [
                ("src/deep/lib.rs".to_owned(), 1),
                ("src/deep/lib.rs".to_owned(), 2),
                ("src/main.rs".to_owned(), 2),
                ("src/notes.txt".to_owned(), 1)
            ],
            "case is ignored unless asked for; a binary file, .git and what a .gitignore leaves out are not searched"
        );
        assert!(!found.truncated);
        assert_eq!(
            found.searched, 4,
            "the three sources and the .gitignore, which is text too"
        );
        let line = found
            .matches
            .iter()
            .find(|m| m.path == "src/main.rs")
            .unwrap();
        assert_eq!(line.text, "    println!(\"Hello\");");
        assert_eq!(line.ranges, vec![(14, 19)]);
        let exact = SearchRequest {
            case_sensitive: true,
            ..request("hello")
        };
        assert_eq!(
            files
                .search(&RelPath::root(), &exact)
                .unwrap()
                .matches
                .len(),
            2,
            "only the lower-case ones"
        );
    }

    #[test]
    fn a_pattern_a_glob_and_a_directory_narrow_the_search() {
        let (_root, files) = tree();
        let pattern = SearchRequest {
            regex: true,
            ..request(r"fn\s+\w+\(")
        };
        let found = files.search(&RelPath::root(), &pattern).unwrap();
        assert_eq!(found.matches.len(), 2);
        let only_rs = SearchRequest {
            glob: Some("*.rs".to_owned()),
            ..request("hello")
        };
        let found = files.search(&RelPath::root(), &only_rs).unwrap();
        assert!(found.matches.iter().all(|m| m.path.ends_with(".rs")));
        let by_path = SearchRequest {
            glob: Some("deep/*.rs".to_owned()),
            ..request("hello")
        };
        assert_eq!(
            files.search(&path("src"), &by_path).unwrap().matches.len(),
            2
        );
        let under = files.search(&path("src/deep"), &request("hello")).unwrap();
        assert_eq!(under.matches.len(), 2);
        // A glob that names a file a .gitignore leaves out brings it back.
        let named = SearchRequest {
            glob: Some("out.txt".to_owned()),
            ..request("hello")
        };
        assert_eq!(
            files
                .search(&RelPath::root(), &named)
                .unwrap()
                .matches
                .len(),
            0,
            "its directory is still left out"
        );
        let file = files
            .search(&path("src/main.rs"), &request("println"))
            .unwrap();
        assert_eq!(file.matches.len(), 1, "a path to a file searches that file");
    }

    #[test]
    fn what_the_ignore_rules_leave_out_is_not_searched_at_all() {
        let (root, _) = tree();
        let files = files_with(root.path(), &["notes.txt", "deep/"], Limits::default());
        let found = files.search(&RelPath::root(), &request("hello")).unwrap();
        let paths: Vec<&str> = found.matches.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths, ["src/main.rs"]);
        assert!(matches!(
            files.search(&path("src/deep"), &request("hello")),
            Err(HostError::Ignored(_))
        ));
    }

    #[test]
    fn a_search_stops_at_its_limit_and_says_so() {
        let root = tempfile::tempdir().unwrap();
        let lines = "match\n".repeat(50);
        std::fs::write(root.path().join("many.txt"), lines).unwrap();
        let files = files_with(root.path(), &[], Limits::default());
        let limited = SearchRequest {
            limit: 10,
            ..request("match")
        };
        let found = files.search(&RelPath::root(), &limited).unwrap();
        assert_eq!(found.matches.len(), 10);
        assert!(found.truncated);
        let exact = SearchRequest {
            limit: 50,
            ..request("match")
        };
        let all = files.search(&RelPath::root(), &exact).unwrap();
        assert_eq!(all.matches.len(), 50);
        assert!(!all.truncated, "everything was seen, so nothing was cut");
        let too_many = SearchRequest {
            limit: 100_000,
            ..request("match")
        };
        assert_eq!(
            files
                .search(&RelPath::root(), &too_many)
                .unwrap()
                .matches
                .len(),
            50
        );
    }

    #[test]
    fn a_search_stops_at_its_time_and_says_so() {
        let (root, _) = tree();
        let files = files_with(
            root.path(),
            &[],
            Limits {
                search_time: Duration::ZERO,
                ..Limits::default()
            },
        );
        let found = files.search(&RelPath::root(), &request("hello")).unwrap();
        assert!(found.truncated);
        assert_eq!(found.searched, 0);
    }

    #[test]
    fn files_larger_than_the_preview_size_and_text_that_is_not_utf8_are_not_searched() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("big.txt"), "needle ".repeat(100)).unwrap();
        std::fs::write(
            root.path().join("latin1.txt"),
            [b'n', b'e', b'e', b'd', b'l', b'e', b' ', 0xe9],
        )
        .unwrap();
        std::fs::write(root.path().join("small.txt"), "needle\n").unwrap();
        let files = files_with(
            root.path(),
            &[],
            Limits {
                preview_bytes: 100,
                ..Limits::default()
            },
        );
        let found = files.search(&RelPath::root(), &request("needle")).unwrap();
        let paths: Vec<&str> = found.matches.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths, ["small.txt"]);
    }

    #[test]
    fn a_long_line_is_cut_to_four_hundred_characters_around_the_match() {
        let root = tempfile::tempdir().unwrap();
        let line = format!("{}NEEDLE{}", "é".repeat(700), "x".repeat(700));
        std::fs::write(root.path().join("long.txt"), &line).unwrap();
        let files = files_with(root.path(), &[], Limits::default());
        let found = files.search(&RelPath::root(), &request("needle")).unwrap();
        let hit = &found.matches[0];
        assert_eq!(hit.text.chars().count(), 400);
        let (start, end) = hit.ranges[0];
        let shown: String = hit
            .text
            .chars()
            .skip(start as usize)
            .take((end - start) as usize)
            .collect();
        assert_eq!(shown, "NEEDLE");
    }

    #[test]
    fn a_pattern_that_is_too_large_or_wrong_is_refused_and_does_not_hang() {
        let (_root, files) = tree();
        let huge = SearchRequest {
            regex: true,
            ..request("(a{1000}){1000}")
        };
        assert!(matches!(
            files.search(&RelPath::root(), &huge),
            Err(HostError::Unsupported(_))
        ));
        let broken = SearchRequest {
            regex: true,
            ..request("(unclosed")
        };
        assert!(matches!(
            files.search(&RelPath::root(), &broken),
            Err(HostError::Unsupported(_))
        ));
        assert!(matches!(
            files.search(&RelPath::root(), &request("")),
            Err(HostError::Unsupported(_))
        ));
        // The classic pattern that makes a backtracking engine take forever is quick here.
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("a.txt"),
            format!("{}!\n", "a".repeat(5000)),
        )
        .unwrap();
        let files = files_with(root.path(), &[], Limits::default());
        let evil = SearchRequest {
            regex: true,
            ..request("(a+)+$")
        };
        // This is a hang guard for an unexpectedly backtracking engine, not a speed assertion.
        const HANG_GUARD: Duration = Duration::from_secs(300);
        let (sent, received) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            sent.send(files.search(&RelPath::root(), &evil)).unwrap();
        });
        let found = received
            .recv_timeout(HANG_GUARD)
            .expect("the bounded regex search completes")
            .unwrap();
        assert!(
            found.matches.is_empty(),
            "the input does not match the expression"
        );
        worker.join().unwrap();
    }
}
