//! What a search looks through and how a match is found.
//!
//! The searches of both tool sets: the plain `search`, and the `Grep` and `Glob` of the set that has the
//! names of Claude Code's own tools. They walk a directory the way a programmer expects a search to: files
//! that a `.gitignore` (or an `.ignore`) leaves out are skipped, and so are hidden files and binary ones,
//! unless the path given names the file itself. The regular expressions are those of the harness's own
//! tool, the syntax of the `regex` crate. Like the file tools they block, and they are meant to run on a
//! thread of their own. Every search is bounded in time, in the number of results and in the size of what it
//! answers, and says when it stopped before it was done.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use ignore::types::TypesBuilder;
use regex::RegexBuilder;

use crate::files::SNIFF_BYTES;
use crate::tools::{ToolFault, ToolOutput};

/// How long one search runs at most before it answers with what it has: the figure the environment's own
/// search of the files of a workspace uses.
pub const SEARCH_TIME: Duration = Duration::from_secs(20);

/// The largest file that is searched: a larger one is a dump or a data file that no one reads for lines.
pub const SEARCH_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// How many characters of a matching line a result shows.
pub const SEARCH_LINE_CHARS: usize = 500;

/// How much a search answers at most, as a read does: 64 KiB.
pub const SEARCH_RESULT_BYTES: usize = 64 * 1024;

/// How many results a plain search gives when the call does not say.
pub const DEFAULT_MAX_RESULTS: usize = 200;

/// The most results a plain search gives, whatever the call asks.
pub const MAX_RESULTS_CEILING: usize = 2000;

/// How many output lines `Grep` gives when the call does not say; the harness's own tool gives this many.
pub const GREP_DEFAULT_HEAD_LIMIT: usize = 250;

/// How many lines `Grep` collects before it stops looking, so that a pattern that matches everything does
/// not fill the memory.
const GREP_COLLECT_MAX: usize = 100_000;

/// How many paths `Glob` gives; the harness's own tool gives this many.
pub const GLOB_LIMIT: usize = 100;

/// How many matching paths `Glob` collects before it stops looking, which is also how many it can sort by
/// the time of change.
const GLOB_COLLECT_MAX: usize = 20_000;

/// Where a search answers the paths from: the place it looks in, as the caller wrote it, and the directory
/// the caller's relative paths start from.
#[derive(Debug, Clone)]
pub struct Where<'a> {
    /// The place to look in, where it really is.
    pub real: &'a Path,
    /// The place to look in, as an absolute path the caller recognises.
    pub shown: &'a Path,
    /// The directory the caller's relative paths start from.
    pub base: &'a Path,
}

impl Where<'_> {
    /// How `file`, a file found below the place to look in, is written in an answer: relative to the
    /// caller's base when it lies below it, else absolute.
    fn name(&self, file: &Path) -> String {
        let below = file.strip_prefix(self.real).unwrap_or(Path::new(""));
        let shown = if below.as_os_str().is_empty() {
            self.shown.to_path_buf()
        } else {
            self.shown.join(below)
        };
        shown
            .strip_prefix(self.base)
            .map(|relative| {
                if relative.as_os_str().is_empty() {
                    ".".to_owned()
                } else {
                    relative.display().to_string()
                }
            })
            .unwrap_or_else(|_| shown.display().to_string())
    }
}

/// The files below a place, in path order, as a programmer's search sees them.
fn walker(
    real: &Path,
    glob: Option<&str>,
    file_type: Option<&str>,
) -> Result<ignore::Walk, ToolFault> {
    let mut builder = WalkBuilder::new(real);
    builder
        .hidden(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        // The user's own global ignore file is a fact of one machine and not of the project.
        .git_global(false)
        .parents(true)
        // A `.gitignore` counts in a directory that is not a repository too.
        .require_git(false)
        .follow_links(false)
        .sort_by_file_path(|left, right| left.cmp(right));
    if let Some(glob) = glob {
        let base = if real.is_dir() {
            real
        } else {
            real.parent().unwrap_or(real)
        };
        let mut overrides = OverrideBuilder::new(base);
        overrides.add(glob).map_err(|error| {
            ToolFault::InvalidInput(format!("the glob {glob:?} is not valid: {error}"))
        })?;
        let overrides = overrides.build().map_err(|error| {
            ToolFault::InvalidInput(format!("the glob {glob:?} is not valid: {error}"))
        })?;
        builder.overrides(overrides);
    }
    if let Some(name) = file_type {
        let mut types = TypesBuilder::new();
        types.add_defaults();
        types.select(name);
        let types = types.build().map_err(|_| {
            ToolFault::InvalidInput(format!(
                "{name:?} is not a file type: use one such as js, py, rust, go, java, ts or md"
            ))
        })?;
        builder.types(types);
    }
    Ok(builder.build())
}

/// A compiled regular expression and how it is applied.
struct Pattern {
    regex: regex::Regex,
    multiline: bool,
}

impl Pattern {
    fn new(pattern: &str, ignore_case: bool, multiline: bool) -> Result<Self, ToolFault> {
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .multi_line(multiline)
            .dot_matches_new_line(multiline)
            .build()
            .map_err(|error| {
                ToolFault::InvalidInput(format!(
                    "the pattern {pattern:?} is not a regular expression: {error}"
                ))
            })?;
        Ok(Self { regex, multiline })
    }
}

/// What one file holds of a pattern.
struct Scan {
    /// The lines of the file, when they were asked for.
    lines: Vec<String>,
    /// The numbers (from 0) of the lines that match, in order, each once.
    hits: Vec<usize>,
}

/// Looks through one file. A file that is not text, or is too large, holds nothing for a search.
fn scan(pattern: &Pattern, path: &Path, keep_lines: bool) -> Option<Scan> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > SEARCH_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    let hits = if pattern.multiline {
        let mut starts = vec![0usize];
        starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        let line_of = |offset: usize| starts.partition_point(|start| *start <= offset) - 1;
        let mut hits: Vec<usize> = Vec::new();
        for found in pattern.regex.find_iter(&text) {
            let first = line_of(found.start());
            let last = line_of(found.end().saturating_sub(1).max(found.start()));
            for line in first..=last {
                if hits.last().is_none_or(|previous| *previous < line) {
                    hits.push(line);
                }
            }
        }
        hits
    } else {
        text.lines()
            .enumerate()
            .filter(|(_, line)| pattern.regex.is_match(line))
            .map(|(number, _)| number)
            .collect()
    };
    let lines = if keep_lines {
        text.lines().map(str::to_owned).collect()
    } else {
        Vec::new()
    };
    Some(Scan { lines, hits })
}

/// A line of text as a result shows it.
fn shown_line(line: &str) -> String {
    if line.chars().count() > SEARCH_LINE_CHARS {
        let cut: String = line.chars().take(SEARCH_LINE_CHARS).collect();
        format!("{cut} [line cut]")
    } else {
        line.to_owned()
    }
}

/// A plain search: the lines that match, as `path:line: text`.
#[derive(Debug, Clone)]
pub struct PlainSearch<'a> {
    /// The regular expression.
    pub pattern: &'a str,
    /// Where to look.
    pub at: Where<'a>,
    /// Which files, by a glob such as `*.rs`.
    pub glob: Option<&'a str>,
    /// How many matching lines at most.
    pub max_results: usize,
}

/// Runs a plain search.
pub fn plain(search: &PlainSearch<'_>) -> Result<ToolOutput, ToolFault> {
    let pattern = Pattern::new(search.pattern, false, false)?;
    let started = Instant::now();
    let mut out = String::new();
    let mut found = 0usize;
    let mut more = false;
    let mut timed_out = false;
    'files: for entry in walker(search.at.real, search.glob, None)?.flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if started.elapsed() > SEARCH_TIME {
            timed_out = true;
            break;
        }
        let Some(scanned) = scan(&pattern, entry.path(), true) else {
            continue;
        };
        let name = search.at.name(entry.path());
        for hit in scanned.hits {
            if found == search.max_results {
                more = true;
                break 'files;
            }
            let line = scanned.lines.get(hit).map_or("", String::as_str);
            let _ = writeln!(out, "{name}:{}: {}", hit + 1, shown_line(line));
            found += 1;
        }
    }
    if found == 0 && !timed_out {
        return Ok(ToolOutput::Text("No lines match.".to_owned()));
    }
    if more {
        let _ = writeln!(
            out,
            "[More lines match: this is the first {found}. Narrow the pattern or the path, or ask for more with max_results.]"
        );
    }
    if timed_out {
        let _ = writeln!(
            out,
            "[The search stopped after {} seconds, before it had looked at every file. Narrow the path.]",
            SEARCH_TIME.as_secs()
        );
    }
    Ok(ToolOutput::Text(bounded(out)))
}

/// What `Grep` answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrepMode {
    /// The matching lines.
    Content,
    /// The files that have a matching line.
    FilesWithMatches,
    /// How many lines match in each file.
    Count,
}

/// A search as `Grep` of the harness's own tools is asked.
#[derive(Debug, Clone)]
pub struct GrepSearch<'a> {
    /// The regular expression.
    pub pattern: &'a str,
    /// Where to look.
    pub at: Where<'a>,
    /// Which files, by a glob such as `*.rs`.
    pub glob: Option<&'a str>,
    /// Which files, by a type such as `rust`.
    pub file_type: Option<&'a str>,
    /// What to answer.
    pub mode: GrepMode,
    /// Whether upper and lower case are the same.
    pub ignore_case: bool,
    /// Whether the lines have their numbers (content mode).
    pub line_numbers: bool,
    /// How many lines to show before each match (content mode).
    pub before: usize,
    /// How many lines to show after each match (content mode).
    pub after: usize,
    /// How many lines or entries at most; 0 for all.
    pub head_limit: usize,
    /// How many lines or entries to skip first.
    pub offset: usize,
    /// Whether a pattern may span lines.
    pub multiline: bool,
}

/// Runs the search of `Grep`.
pub fn grep(search: &GrepSearch<'_>) -> Result<ToolOutput, ToolFault> {
    let pattern = Pattern::new(search.pattern, search.ignore_case, search.multiline)?;
    let started = Instant::now();
    let keep_lines = search.mode == GrepMode::Content;
    let mut lines: Vec<String> = Vec::new();
    let mut files = 0usize;
    let mut total_lines_matching = 0usize;
    let mut timed_out = false;
    let mut collected_all = true;
    for entry in walker(search.at.real, search.glob, search.file_type)?.flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if started.elapsed() > SEARCH_TIME {
            timed_out = true;
            break;
        }
        let Some(scanned) = scan(&pattern, entry.path(), keep_lines) else {
            continue;
        };
        if scanned.hits.is_empty() {
            continue;
        }
        files += 1;
        total_lines_matching += scanned.hits.len();
        let name = search.at.name(entry.path());
        match search.mode {
            GrepMode::FilesWithMatches => lines.push(name),
            GrepMode::Count => lines.push(format!("{name}:{}", scanned.hits.len())),
            GrepMode::Content => {
                if !content_lines(search, &name, &scanned, &mut lines) {
                    collected_all = false;
                    break;
                }
            }
        }
        if lines.len() >= GREP_COLLECT_MAX {
            collected_all = false;
            break;
        }
    }
    if lines.is_empty() {
        return Ok(ToolOutput::Text(match search.mode {
            GrepMode::FilesWithMatches => "No files found".to_owned(),
            _ => "No matches found".to_owned(),
        }));
    }
    let skipped = search.offset.min(lines.len());
    let rest = &lines[skipped..];
    let limit = if search.head_limit == 0 {
        rest.len()
    } else {
        search.head_limit.min(rest.len())
    };
    let shown = &rest[..limit];
    let mut out = String::new();
    if search.mode == GrepMode::FilesWithMatches {
        let _ = writeln!(
            out,
            "Found {files} file{}",
            if files == 1 { "" } else { "s" }
        );
    }
    for line in shown {
        out.push_str(line);
        out.push('\n');
    }
    if search.mode == GrepMode::Count {
        let _ = writeln!(
            out,
            "\nFound {total_lines_matching} matching line{} across {files} file{}.",
            if total_lines_matching == 1 { "" } else { "s" },
            if files == 1 { "" } else { "s" }
        );
    }
    if limit < rest.len() {
        let _ = writeln!(
            out,
            "[Showing {} to {} of {}{}. Ask for the rest with offset {}.]",
            skipped + 1,
            skipped + limit,
            lines.len(),
            if collected_all { "" } else { " or more" },
            skipped + limit
        );
    } else if !collected_all {
        let _ = writeln!(
            out,
            "[The search stopped after {} lines, before it had looked at every file. Narrow the pattern or the path.]",
            lines.len()
        );
    }
    if timed_out {
        let _ = writeln!(
            out,
            "[The search stopped after {} seconds, before it had looked at every file. Narrow the path.]",
            SEARCH_TIME.as_secs()
        );
    }
    Ok(ToolOutput::Text(bounded(out)))
}

/// Adds the lines of one file for the content mode: each match with the lines of context around it,
/// groups apart from each other marked by `--`. False when the limit of lines collected was reached.
fn content_lines(
    search: &GrepSearch<'_>,
    name: &str,
    scanned: &Scan,
    out: &mut Vec<String>,
) -> bool {
    let last = scanned.lines.len().saturating_sub(1);
    let mut next_unshown = 0usize;
    let mut first_group = true;
    let hits: std::collections::BTreeSet<usize> = scanned.hits.iter().copied().collect();
    for hit in &scanned.hits {
        let from = hit.saturating_sub(search.before).max(next_unshown);
        let to = (hit + search.after).min(last);
        if from > to {
            continue;
        }
        // The groups of lines are told apart only when there are lines of context to make groups of.
        if !first_group && from > next_unshown && (search.before > 0 || search.after > 0) {
            out.push("--".to_owned());
        }
        first_group = false;
        for number in from..=to {
            let separator = if hits.contains(&number) { ':' } else { '-' };
            let text = shown_line(scanned.lines.get(number).map_or("", String::as_str));
            if search.line_numbers {
                out.push(format!("{name}{separator}{}{separator}{text}", number + 1));
            } else {
                out.push(format!("{name}{separator}{text}"));
            }
        }
        next_unshown = to + 1;
        if out.len() >= GREP_COLLECT_MAX {
            return false;
        }
    }
    true
}

/// A search for the paths that match a glob, as `Glob` of the harness's own tools is asked.
#[derive(Debug, Clone)]
pub struct GlobSearch<'a> {
    /// The glob, matched against the path below the place to look in.
    pub pattern: &'a str,
    /// Where to look.
    pub at: Where<'a>,
}

/// Runs the search of `Glob`: the paths that match, newest first, at most [`GLOB_LIMIT`].
pub fn glob(search: &GlobSearch<'_>) -> Result<ToolOutput, ToolFault> {
    if Path::new(search.pattern).is_absolute() {
        return Err(ToolFault::InvalidInput(
            "the pattern is relative to the place searched, which the path argument names; give the directory there"
                .to_owned(),
        ));
    }
    let matcher = globset::GlobBuilder::new(search.pattern)
        .literal_separator(true)
        .build()
        .map_err(|error| {
            ToolFault::InvalidInput(format!(
                "the pattern {:?} is not a glob: {error}",
                search.pattern
            ))
        })?
        .compile_matcher();
    let started = Instant::now();
    let mut matches: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut collected_all = true;
    let mut timed_out = false;
    for entry in walker(search.at.real, None, None)?.flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if started.elapsed() > SEARCH_TIME {
            timed_out = true;
            break;
        }
        let below = entry
            .path()
            .strip_prefix(search.at.real)
            .unwrap_or_else(|_| entry.path());
        if !matcher.is_match(below) {
            continue;
        }
        let changed = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        matches.push((changed, entry.path().to_path_buf()));
        if matches.len() >= GLOB_COLLECT_MAX {
            collected_all = false;
            break;
        }
    }
    if matches.is_empty() {
        return Ok(ToolOutput::Text("No files found".to_owned()));
    }
    matches.sort_by(|(left_time, left), (right_time, right)| {
        right_time.cmp(left_time).then_with(|| left.cmp(right))
    });
    let total = matches.len();
    let mut out = String::new();
    for (_, path) in matches.iter().take(GLOB_LIMIT) {
        out.push_str(&search.at.name(path));
        out.push('\n');
    }
    if total > GLOB_LIMIT || !collected_all {
        let _ = writeln!(
            out,
            "[There are more: this is the newest {GLOB_LIMIT} of {total}{}. Use a more specific path or pattern.]",
            if collected_all { "" } else { " or more" }
        );
    }
    if timed_out {
        let _ = writeln!(
            out,
            "[The search stopped after {} seconds, before it had looked at every file. Narrow the path.]",
            SEARCH_TIME.as_secs()
        );
    }
    Ok(ToolOutput::Text(bounded(out)))
}

/// `text` within the size of a result; what does not fit is left out at a line end, and the answer says so.
fn bounded(text: String) -> String {
    if text.len() <= SEARCH_RESULT_BYTES {
        return text;
    }
    let mut end = SEARCH_RESULT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = text[..end].rfind('\n').map_or(end, |at| at + 1);
    format!(
        "{}[The answer reached {SEARCH_RESULT_BYTES} bytes and the rest is left out. Narrow the search.]\n",
        &text[..cut]
    )
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    struct Tree {
        dir: tempfile::TempDir,
    }

    impl Tree {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("a directory");
            let root = std::fs::canonicalize(dir.path()).expect("path");
            std::fs::create_dir_all(root.join("src/deep")).expect("dirs");
            std::fs::create_dir_all(root.join("target")).expect("dirs");
            std::fs::create_dir_all(root.join(".hidden")).expect("dirs");
            std::fs::write(
                root.join("src/main.rs"),
                "fn main() {\n    println!(\"hello\");\n}\n",
            )
            .expect("file");
            std::fs::write(
                root.join("src/lib.rs"),
                "pub fn hello() {}\npub fn bye() {}\n",
            )
            .expect("file");
            std::fs::write(
                root.join("src/deep/mod.rs"),
                "// Hello again\nfn deep() {}\n",
            )
            .expect("file");
            std::fs::write(root.join("notes.md"), "# Notes\nhello from the notes\n").expect("file");
            std::fs::write(root.join("target/out.txt"), "hello from a build product\n")
                .expect("file");
            std::fs::write(root.join(".gitignore"), "target/\n").expect("file");
            std::fs::write(root.join(".hidden/secret.txt"), "hello hidden\n").expect("file");
            std::fs::write(root.join("blob.bin"), b"hello\0binary").expect("file");
            Self { dir }
        }

        fn root(&self) -> PathBuf {
            std::fs::canonicalize(self.dir.path()).expect("path")
        }
    }

    fn at<'a>(root: &'a Path) -> Where<'a> {
        Where {
            real: root,
            shown: root,
            base: root,
        }
    }

    fn text(output: ToolOutput) -> String {
        match output {
            ToolOutput::Text(text) => text,
            other => panic!("text was expected, and {other:?} came"),
        }
    }

    #[test]
    fn a_plain_search_gives_path_line_and_text_and_skips_what_a_search_should_skip() {
        let tree = Tree::new();
        let root = tree.root();
        let answer = text(
            plain(&PlainSearch {
                pattern: "hello",
                at: at(&root),
                glob: None,
                max_results: DEFAULT_MAX_RESULTS,
            })
            .expect("search"),
        );
        let lines: Vec<&str> = answer.lines().collect();
        assert_eq!(
            lines,
            [
                "notes.md:2: hello from the notes",
                "src/lib.rs:1: pub fn hello() {}",
                "src/main.rs:2:     println!(\"hello\");",
            ],
            "ignored, hidden and binary files are not searched, and Hello is not hello"
        );
    }

    #[test]
    fn a_glob_limits_the_files_and_a_named_file_is_searched_whatever_the_ignore_rules_say() {
        let tree = Tree::new();
        let root = tree.root();
        let only_rust = text(
            plain(&PlainSearch {
                pattern: "hello",
                at: at(&root),
                glob: Some("*.rs"),
                max_results: DEFAULT_MAX_RESULTS,
            })
            .expect("search"),
        );
        assert!(!only_rust.contains("notes.md") && only_rust.contains("src/lib.rs"));
        let named = root.join("target/out.txt");
        let one_file = text(
            plain(&PlainSearch {
                pattern: "hello",
                at: Where {
                    real: &named,
                    shown: &named,
                    base: &root,
                },
                glob: None,
                max_results: DEFAULT_MAX_RESULTS,
            })
            .expect("search"),
        );
        assert_eq!(
            one_file.trim_end(),
            "target/out.txt:1: hello from a build product"
        );
    }

    #[test]
    fn a_plain_search_stops_at_the_most_results_and_says_there_are_more() {
        let tree = Tree::new();
        let root = tree.root();
        let answer = text(
            plain(&PlainSearch {
                pattern: "hello",
                at: at(&root),
                glob: None,
                max_results: 2,
            })
            .expect("search"),
        );
        assert_eq!(answer.lines().filter(|line| line.contains(": ")).count(), 3);
        assert!(answer.contains("More lines match"), "{answer}");
        assert_eq!(
            answer
                .lines()
                .filter(|l| l.starts_with("notes.md") || l.starts_with("src/"))
                .count(),
            2
        );
    }

    #[test]
    fn a_pattern_that_is_not_a_regular_expression_is_refused_with_the_reason() {
        let tree = Tree::new();
        let root = tree.root();
        let fault = plain(&PlainSearch {
            pattern: "(unclosed",
            at: at(&root),
            glob: None,
            max_results: 10,
        })
        .expect_err("not a regex");
        assert_eq!(fault.code(), "invalid_input");
        assert!(fault.to_string().contains("regular expression"));
    }

    fn grep_of<'a>(root: &'a Path, pattern: &'a str, mode: GrepMode) -> GrepSearch<'a> {
        GrepSearch {
            pattern,
            at: at(root),
            glob: None,
            file_type: None,
            mode,
            ignore_case: false,
            line_numbers: true,
            before: 0,
            after: 0,
            head_limit: GREP_DEFAULT_HEAD_LIMIT,
            offset: 0,
            multiline: false,
        }
    }

    #[test]
    fn grep_names_the_files_that_match_when_no_mode_is_given_and_counts_lines_when_asked() {
        let tree = Tree::new();
        let root = tree.root();
        let files = text(grep(&grep_of(&root, "hello", GrepMode::FilesWithMatches)).expect("grep"));
        assert_eq!(files, "Found 3 files\nnotes.md\nsrc/lib.rs\nsrc/main.rs\n");
        let counts = text(grep(&grep_of(&root, "fn ", GrepMode::Count)).expect("grep"));
        assert!(
            counts.contains("src/lib.rs:2\n") && counts.contains("src/main.rs:1\n"),
            "{counts}"
        );
        assert!(
            counts.contains("Found 4 matching lines across 3 files."),
            "{counts}"
        );
        let none = text(grep(&grep_of(&root, "zzz", GrepMode::FilesWithMatches)).expect("grep"));
        assert_eq!(none, "No files found");
    }

    #[test]
    fn grep_shows_lines_with_their_numbers_and_the_context_around_them() {
        let tree = Tree::new();
        let root = tree.root();
        std::fs::write(
            root.join("ctx.txt"),
            "a\nb\nneedle one\nc\nd\ne\nf\nneedle two\ng\n",
        )
        .expect("file");
        let mut search = grep_of(&root, "needle", GrepMode::Content);
        search.glob = Some("ctx.txt");
        search.before = 1;
        search.after = 1;
        let answer = text(grep(&search).expect("grep"));
        assert_eq!(
            answer,
            "ctx.txt-2-b\nctx.txt:3:needle one\nctx.txt-4-c\n--\nctx.txt-7-f\nctx.txt:8:needle two\nctx.txt-9-g\n"
        );
        search.line_numbers = false;
        search.before = 0;
        search.after = 0;
        let plain_lines = text(grep(&search).expect("grep"));
        assert_eq!(plain_lines, "ctx.txt:needle one\nctx.txt:needle two\n");
    }

    #[test]
    fn grep_honours_case_types_head_limit_offset_and_multiline() {
        let tree = Tree::new();
        let root = tree.root();
        let mut search = grep_of(&root, "HELLO", GrepMode::FilesWithMatches);
        assert_eq!(text(grep(&search).expect("grep")), "No files found");
        search.ignore_case = true;
        assert!(text(grep(&search).expect("grep")).starts_with("Found 4 files"));
        search.file_type = Some("rust");
        assert_eq!(
            text(grep(&search).expect("grep")),
            "Found 3 files\nsrc/deep/mod.rs\nsrc/lib.rs\nsrc/main.rs\n"
        );
        search.head_limit = 1;
        search.offset = 1;
        let paged = text(grep(&search).expect("grep"));
        assert!(
            paged.contains("src/lib.rs") && !paged.contains("src/main.rs"),
            "{paged}"
        );
        assert!(paged.contains("Ask for the rest with offset 2"), "{paged}");
        let mut multi = grep_of(&root, "fn main\\(\\) \\{.*hello", GrepMode::Content);
        assert_eq!(text(grep(&multi).expect("grep")), "No matches found");
        multi.multiline = true;
        let spanning = text(grep(&multi).expect("grep"));
        assert!(
            spanning.contains("src/main.rs:1:fn main() {") && spanning.contains("src/main.rs:2:"),
            "{spanning}"
        );
        search.file_type = Some("nonsense-type");
        assert_eq!(
            grep(&search).expect_err("unknown type").code(),
            "invalid_input"
        );
    }

    #[test]
    fn glob_gives_the_newest_paths_first_and_says_when_there_are_more() {
        let tree = Tree::new();
        let root = tree.root();
        let old = root.join("src/main.rs");
        let new = root.join("src/lib.rs");
        let then = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .expect("open")
            .set_modified(then)
            .expect("set time");
        let found = text(
            glob(&GlobSearch {
                pattern: "src/**/*.rs",
                at: at(&root),
            })
            .expect("glob"),
        );
        let paths: Vec<&str> = found.lines().collect();
        assert_eq!(paths.len(), 3, "{found}");
        assert_eq!(paths[2], "src/main.rs", "the oldest comes last: {found}");
        assert!(new.exists());
        let top_only = text(
            glob(&GlobSearch {
                pattern: "*.md",
                at: at(&root),
            })
            .expect("glob"),
        );
        assert_eq!(top_only.trim_end(), "notes.md");
        let nothing = text(
            glob(&GlobSearch {
                pattern: "*.rs",
                at: at(&root),
            })
            .expect("glob"),
        );
        assert_eq!(nothing, "No files found", "* does not cross a slash");
        for n in 0..(GLOB_LIMIT + 7) {
            std::fs::write(root.join(format!("many{n:03}.dat")), "x").expect("file");
        }
        let many = text(
            glob(&GlobSearch {
                pattern: "*.dat",
                at: at(&root),
            })
            .expect("glob"),
        );
        assert_eq!(
            many.lines().filter(|l| l.ends_with(".dat")).count(),
            GLOB_LIMIT
        );
        assert!(many.contains("There are more"), "{many}");
        assert_eq!(
            glob(&GlobSearch {
                pattern: "/etc/*",
                at: at(&root),
            })
            .expect_err("absolute")
            .code(),
            "invalid_input"
        );
    }

    #[test]
    fn a_long_answer_is_cut_at_a_line_and_says_so() {
        let tree = Tree::new();
        let root = tree.root();
        let mut many = String::new();
        for n in 0..4000 {
            let _ = writeln!(many, "needle number {n} {}", "x".repeat(40));
        }
        std::fs::write(root.join("many.txt"), many).expect("file");
        let mut search = grep_of(&root, "needle", GrepMode::Content);
        search.glob = Some("many.txt");
        search.head_limit = 0;
        let answer = text(grep(&search).expect("grep"));
        assert!(
            answer.len() <= SEARCH_RESULT_BYTES + 200,
            "{}",
            answer.len()
        );
        assert!(
            answer.contains("the rest is left out"),
            "{}",
            &answer[answer.len() - 100..]
        );
    }
}
