//! File tools, held to the thread's directory and the attached mounts by the agent's sandbox mode.
//!
//! Reading a file as text with line numbers (or as an image), writing it, replacing exact text in it, and
//! listing a directory. Every function takes the session's [`Access`] and judges the path by it before it
//! touches anything: a path outside the thread's directory and the attached mounts is refused in the modes
//! `workspace` and `read_only`, and a write is refused where the mode or a read-only mount forbids it. The
//! functions block on the file system and are meant to run on a thread of their own.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde_json::json;

use crate::sandbox::{Access, Need};
use crate::tools::{ToolFault, ToolOutput};

/// How many lines a read gives when the call does not say: a screenful of source, and the figure the
/// harness's own read tool uses.
pub const READ_DEFAULT_LINES: usize = 2000;

/// How many characters of one line a read shows: a line longer than this is a minified file or a data
/// file, and its end is no use in a result.
pub const READ_LINE_CHARS: usize = 2000;

/// How much text one read returns at most: 64 KiB is about sixteen thousand tokens, below the
/// twenty-five thousand that a harness may allow a tool's result before it refuses it.
pub const READ_RESULT_BYTES: usize = 64 * 1024;

/// How much of a file is scanned to find the line a read starts at: past this the file is no text to
/// read from the middle, and a search or a command is the better tool.
pub const READ_SCAN_BYTES: u64 = 512 * 1024 * 1024;

/// How much of the beginning of a file is looked at to tell text from other data: a NUL byte there means
/// the file is not text. The same length is used by the tools that tell a file apart by its start.
pub const SNIFF_BYTES: usize = 8 * 1024;

/// The largest image a read returns: the largest a model's interface takes.
pub const IMAGE_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// The largest file an edit works on, since it holds the whole file in memory twice.
pub const EDIT_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// How many entries a listing returns at most.
pub const LIST_MAX_ENTRIES: usize = 1000;

/// The deepest a listing goes.
pub const LIST_MAX_DEPTH: u32 = 4;

/// A path the caller named and where it really is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The path as an absolute path, the way the caller will recognise it.
    pub shown: PathBuf,
    /// The path to use: with every link resolved where the mode asks for it.
    pub real: PathBuf,
}

/// Judges `path`, given relative to `base`, for `need`.
pub fn place(access: &Access, base: &Path, path: &str, need: Need) -> Result<Place, ToolFault> {
    if path.is_empty() {
        return Err(ToolFault::InvalidInput("the path is empty".to_owned()));
    }
    if path.contains('\0') {
        return Err(ToolFault::InvalidInput(
            "the path has a NUL character".to_owned(),
        ));
    }
    let shown = Access::absolute(path, base);
    let real = access.permit(&shown, need)?;
    Ok(Place { shown, real })
}

/// What a read is asked for.
#[derive(Debug, Clone, Copy)]
pub struct ReadRequest<'a> {
    /// The path.
    pub path: &'a str,
    /// The line to start at, counted from 1.
    pub offset: Option<i64>,
    /// How many lines at most.
    pub limit: Option<i64>,
    /// What to say when the path is a directory.
    pub directory_advice: &'a str,
}

/// The file's text with line numbers, from line `offset`, at most `limit` lines; an image as an image.
pub fn read(
    access: &Access,
    base: &Path,
    request: &ReadRequest<'_>,
) -> Result<ToolOutput, ToolFault> {
    let start = match request.offset {
        None => 1,
        Some(offset) if offset >= 1 => offset.unsigned_abs() as usize,
        Some(0) => 1,
        Some(offset) => {
            return Err(ToolFault::InvalidInput(format!(
                "offset is a line number counted from 1, and {offset} is not"
            )));
        }
    };
    let limit = match request.limit {
        None => READ_DEFAULT_LINES,
        Some(limit) if limit >= 1 => limit.unsigned_abs() as usize,
        Some(limit) => {
            return Err(ToolFault::InvalidInput(format!(
                "limit is a number of lines, at least 1, and {limit} is not"
            )));
        }
    };
    let found = place(access, base, request.path, Need::Read)?;
    let metadata = std::fs::metadata(&found.real)
        .map_err(|error| ToolFault::io("read", &found.shown, error))?;
    if metadata.is_dir() {
        return Err(ToolFault::IsDirectory(format!(
            "{} is a directory. {}",
            found.shown.display(),
            request.directory_advice
        )));
    }
    if !metadata.is_file() {
        // A pipe or a device is not read from: opening one can wait for ever.
        return Err(ToolFault::NotText(format!(
            "{} is not a regular file",
            found.shown.display()
        )));
    }
    let mut file =
        File::open(&found.real).map_err(|error| ToolFault::io("read", &found.shown, error))?;
    let mut sniffed = vec![0u8; SNIFF_BYTES];
    let sniffed_len = read_up_to(&mut file, &mut sniffed)
        .map_err(|error| ToolFault::io("read", &found.shown, error))?;
    sniffed.truncate(sniffed_len);
    if let Some(media_type) = image_type(&sniffed) {
        return read_image(&mut file, &found, metadata.len(), media_type, sniffed);
    }
    if sniffed.contains(&0) {
        return Err(ToolFault::NotText(format!(
            "{} is not a text file (it holds NUL bytes)",
            found.shown.display()
        )));
    }
    let mut reader = BufReader::new(
        File::open(&found.real).map_err(|error| ToolFault::io("read", &found.shown, error))?,
    );
    read_lines(&mut reader, &found, start, limit)
}

/// Reads into `buffer` until it is full or the file ends; how many bytes came.
fn read_up_to(file: &mut File, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let got = file.read(&mut buffer[filled..])?;
        if got == 0 {
            break;
        }
        filled += got;
    }
    Ok(filled)
}

/// The media type of an image file by the bytes it begins with.
fn image_type(start: &[u8]) -> Option<&'static str> {
    if start.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if start.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if start.starts_with(b"GIF87a") || start.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if start.len() >= 12 && &start[..4] == b"RIFF" && &start[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The image the file holds, base64, when it is small enough.
fn read_image(
    file: &mut File,
    found: &Place,
    size: u64,
    media_type: &str,
    already_read: Vec<u8>,
) -> Result<ToolOutput, ToolFault> {
    if size > IMAGE_MAX_BYTES {
        return Err(ToolFault::TooLarge(format!(
            "{} is an image of {size} bytes, and an image of more than {IMAGE_MAX_BYTES} bytes is not returned",
            found.shown.display()
        )));
    }
    let mut bytes = already_read;
    file.read_to_end(&mut bytes)
        .map_err(|error| ToolFault::io("read", &found.shown, error))?;
    Ok(ToolOutput::Image {
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        media_type: media_type.to_owned(),
    })
}

/// One line of a file, as far as `cap` bytes of it, and whether the file ended with it.
struct Line {
    bytes: Vec<u8>,
    /// How many bytes the line had, without its end.
    length: usize,
    /// Whether the line ended with the end of the file and not with a line end.
    last: bool,
}

/// Reads the next line, keeping at most `cap` bytes of it in memory.
fn next_line(reader: &mut impl BufRead, cap: usize) -> std::io::Result<Option<Line>> {
    let mut bytes = Vec::new();
    let mut length = 0usize;
    let mut saw_any = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if !saw_any {
                return Ok(None);
            }
            return Ok(Some(Line {
                bytes,
                length,
                last: true,
            }));
        }
        saw_any = true;
        match available.iter().position(|byte| *byte == b'\n') {
            Some(end) => {
                keep(&mut bytes, &available[..end], cap);
                length += end;
                reader.consume(end + 1);
                // A line that is whole loses the carriage return of a DOS line end.
                if length <= cap && bytes.last() == Some(&b'\r') {
                    bytes.pop();
                }
                return Ok(Some(Line {
                    bytes,
                    length,
                    last: false,
                }));
            }
            None => {
                let used = available.len();
                keep(&mut bytes, available, cap);
                length += used;
                reader.consume(used);
            }
        }
    }
}

/// Appends as much of `piece` to `kept` as `cap` leaves room for.
fn keep(kept: &mut Vec<u8>, piece: &[u8], cap: usize) {
    let room = cap.saturating_sub(kept.len());
    kept.extend_from_slice(&piece[..room.min(piece.len())]);
}

/// The text of lines `start` onward with their numbers, within the bounds of a result.
fn read_lines(
    reader: &mut BufReader<File>,
    found: &Place,
    start: usize,
    limit: usize,
) -> Result<ToolOutput, ToolFault> {
    // A character is at most four bytes, so this many bytes of a line hold the characters a line shows.
    let line_cap = READ_LINE_CHARS * 4;
    let mut number = 0usize;
    let mut scanned = 0u64;
    let mut out = String::new();
    let mut shown = 0usize;
    let mut stopped_by_size = false;
    let mut more = false;
    while let Some(line) =
        next_line(reader, line_cap).map_err(|error| ToolFault::io("read", &found.shown, error))?
    {
        number += 1;
        scanned += line.length as u64 + 1;
        if number < start {
            if scanned > READ_SCAN_BYTES {
                return Err(ToolFault::TooLarge(format!(
                    "{} is too large to start at line {start}; search it or use a command",
                    found.shown.display()
                )));
            }
            continue;
        }
        if shown == limit {
            more = true;
            break;
        }
        let text = String::from_utf8_lossy(&line.bytes);
        let rendered = if text.chars().count() > READ_LINE_CHARS || line.length > line_cap {
            let cut: String = text.chars().take(READ_LINE_CHARS).collect();
            format!("{cut} [line cut: it is longer than {READ_LINE_CHARS} characters]")
        } else {
            text.into_owned()
        };
        let formatted = format!("{number:>6}\u{2192}{rendered}\n");
        if out.len() + formatted.len() > READ_RESULT_BYTES {
            stopped_by_size = true;
            more = true;
            break;
        }
        out.push_str(&formatted);
        shown += 1;
        if line.last {
            break;
        }
    }
    if shown == 0 && !stopped_by_size {
        return Ok(ToolOutput::Text(if number == 0 {
            format!("{} is empty.", found.shown.display())
        } else {
            format!(
                "{} has {number} lines, and the read was to start at line {start}.",
                found.shown.display()
            )
        }));
    }
    if more {
        let next = start + shown;
        let reason = if stopped_by_size {
            format!("the result reached {READ_RESULT_BYTES} bytes")
        } else {
            format!("the limit of {limit} lines was reached")
        };
        out.push_str(&format!(
            "[The file goes on: {reason}. Continue with offset {next}.]\n"
        ));
    }
    Ok(ToolOutput::Text(out))
}

/// What a write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written {
    /// How many bytes the file holds now.
    pub bytes: u64,
    /// Whether the file was made by this write.
    pub created: bool,
}

/// Creates the file or replaces all of it; parent directories are created.
pub fn write(
    access: &Access,
    base: &Path,
    path: &str,
    content: &str,
) -> Result<(Place, Written), ToolFault> {
    let found = place(access, base, path, Need::Write)?;
    let existing = match std::fs::symlink_metadata(&found.real) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(ToolFault::io("write", &found.shown, error)),
    };
    if existing.as_ref().is_some_and(std::fs::Metadata::is_dir) {
        return Err(ToolFault::IsDirectory(format!(
            "{} is a directory, not a file",
            found.shown.display()
        )));
    }
    replace_file(&found, content.as_bytes())?;
    Ok((
        found,
        Written {
            bytes: content.len() as u64,
            created: existing.is_none(),
        },
    ))
}

/// Makes the file hold `bytes`: written beside it and renamed over it, so that a write that is cut off
/// leaves the old file whole; written in place where the file cannot be replaced (one that is itself a
/// mount point, say).
fn replace_file(found: &Place, bytes: &[u8]) -> Result<(), ToolFault> {
    let Some(parent) = found.real.parent() else {
        return Err(ToolFault::InvalidInput(format!(
            "{} has no parent directory",
            found.shown.display()
        )));
    };
    let io = |error| ToolFault::io("write", &found.shown, error);
    std::fs::create_dir_all(parent).map_err(io)?;
    let existing = std::fs::metadata(&found.real).ok();
    if existing
        .as_ref()
        .is_some_and(|metadata| !metadata.is_file())
    {
        return Err(ToolFault::NotText(format!(
            "{} is not a regular file, and only a regular file is replaced",
            found.shown.display()
        )));
    }
    if existing.is_some() {
        // The file's own permissions decide, as they would for any program: a file nobody may write is
        // not replaced by renaming something over it.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&found.real)
            .map_err(io)?;
    }
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(io)?;
    staged.write_all(bytes).map_err(io)?;
    if let Some(metadata) = &existing {
        staged
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(io)?;
    }
    if staged.persist(&found.real).is_err() {
        // The staged file is gone with the failed rename; write where the file is.
        std::fs::write(&found.real, bytes).map_err(io)?;
    }
    Ok(())
}

/// Replaces exact text: `old` has to occur exactly once unless `all` is set. How many places changed.
pub fn edit(
    access: &Access,
    base: &Path,
    path: &str,
    old: &str,
    new: &str,
    all: bool,
) -> Result<(Place, usize), ToolFault> {
    if old.is_empty() {
        return Err(ToolFault::InvalidInput(
            "the text to replace is empty".to_owned(),
        ));
    }
    if old == new {
        return Err(ToolFault::InvalidInput(
            "the new text is the old text: nothing would change".to_owned(),
        ));
    }
    let found = place(access, base, path, Need::Write)?;
    let metadata = std::fs::metadata(&found.real)
        .map_err(|error| ToolFault::io("edit", &found.shown, error))?;
    if metadata.is_dir() {
        return Err(ToolFault::IsDirectory(format!(
            "{} is a directory, not a file",
            found.shown.display()
        )));
    }
    if !metadata.is_file() {
        return Err(ToolFault::NotText(format!(
            "{} is not a regular file",
            found.shown.display()
        )));
    }
    if metadata.len() > EDIT_MAX_BYTES {
        return Err(ToolFault::TooLarge(format!(
            "{} is {} bytes, and a file of more than {EDIT_MAX_BYTES} bytes is not edited",
            found.shown.display(),
            metadata.len()
        )));
    }
    let bytes =
        std::fs::read(&found.real).map_err(|error| ToolFault::io("edit", &found.shown, error))?;
    let text = String::from_utf8(bytes).map_err(|_| {
        ToolFault::NotText(format!(
            "{} is not UTF-8 text, and only text is edited",
            found.shown.display()
        ))
    })?;
    let count = text.matches(old).count();
    if count == 0 {
        return Err(ToolFault::NoMatch(format!(
            "the text to replace is not in {}; it has to match exactly, with its white space",
            found.shown.display()
        )));
    }
    if count > 1 && !all {
        return Err(ToolFault::Ambiguous(format!(
            "the text to replace occurs {count} times in {}; give more of the text around it so that it occurs once, or set all",
            found.shown.display()
        )));
    }
    let replaced = text.replace(old, new);
    replace_file(&found, replaced.as_bytes())?;
    Ok((found, count))
}

/// One entry of a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The path relative to the directory listed, with a `/` at the end of a directory's.
    pub path: String,
    /// `dir`, `file`, `symlink` or `other`.
    pub kind: &'static str,
    /// The size of a file.
    pub size: Option<u64>,
}

/// A directory's entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// The directory, as an absolute path.
    pub path: PathBuf,
    /// Its entries: directories first at every level, then by name.
    pub entries: Vec<Entry>,
    /// Whether there were more than [`LIST_MAX_ENTRIES`].
    pub truncated: bool,
}

impl Listing {
    /// The listing as the result of a tool.
    #[must_use]
    pub fn into_output(self) -> ToolOutput {
        let entries: Vec<serde_json::Value> = self
            .entries
            .iter()
            .map(|entry| match entry.size {
                Some(size) => json!({ "path": entry.path, "kind": entry.kind, "size": size }),
                None => json!({ "path": entry.path, "kind": entry.kind }),
            })
            .collect();
        ToolOutput::Json(json!({
            "path": self.path.display().to_string(),
            "entries": entries,
            "truncated": self.truncated,
        }))
    }
}

/// The entries of the directory `path`, to `depth` levels.
pub fn list(
    access: &Access,
    base: &Path,
    path: &str,
    depth: Option<i64>,
) -> Result<Listing, ToolFault> {
    let depth = match depth {
        None => 1,
        Some(depth) if (1..=i64::from(LIST_MAX_DEPTH)).contains(&depth) => {
            u32::try_from(depth).unwrap_or(1)
        }
        Some(depth) => {
            return Err(ToolFault::InvalidInput(format!(
                "depth is from 1 to {LIST_MAX_DEPTH}, and {depth} is not"
            )));
        }
    };
    let found = place(access, base, path, Need::Read)?;
    let metadata = std::fs::metadata(&found.real)
        .map_err(|error| ToolFault::io("list", &found.shown, error))?;
    if !metadata.is_dir() {
        return Err(ToolFault::InvalidInput(format!(
            "{} is a file, not a directory",
            found.shown.display()
        )));
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    walk(&found.real, "", depth, &mut entries, &mut truncated)
        .map_err(|error| ToolFault::io("list", &found.shown, error))?;
    Ok(Listing {
        path: found.shown,
        entries,
        truncated,
    })
}

/// Adds the entries of `directory` to `entries`, directories first, then those of the directories.
fn walk(
    directory: &Path,
    prefix: &str,
    depth: u32,
    entries: &mut Vec<Entry>,
    truncated: &mut bool,
) -> std::io::Result<()> {
    let mut found: Vec<(String, std::fs::Metadata)> = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        // A link is shown as a link and never followed: it may lead out of the places this session reaches.
        let metadata = entry.metadata()?;
        found.push((entry.file_name().to_string_lossy().into_owned(), metadata));
    }
    found.sort_by(|(left_name, left), (right_name, right)| {
        right
            .is_dir()
            .cmp(&left.is_dir())
            .then_with(|| left_name.cmp(right_name))
    });
    for (name, metadata) in found {
        if entries.len() >= LIST_MAX_ENTRIES {
            *truncated = true;
            return Ok(());
        }
        let file_type = metadata.file_type();
        let (kind, size) = if file_type.is_symlink() {
            ("symlink", None)
        } else if file_type.is_dir() {
            ("dir", None)
        } else if file_type.is_file() {
            ("file", Some(metadata.len()))
        } else {
            ("other", None)
        };
        let shown = if kind == "dir" {
            format!("{prefix}{name}/")
        } else {
            format!("{prefix}{name}")
        };
        entries.push(Entry {
            path: shown.clone(),
            kind,
            size,
        });
        if kind == "dir" && depth > 1 {
            walk(
                &directory.join(&name),
                &shown,
                depth - 1,
                entries,
                truncated,
            )?;
            if *truncated {
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::os::unix::fs::symlink;

    use orch_agent_contract::descriptor::SandboxMode;

    use super::*;
    use crate::scope::{Mount, SessionScope};
    use crate::tools::ToolSet;

    struct Rig {
        _outer: tempfile::TempDir,
        work: PathBuf,
        elsewhere: PathBuf,
    }

    fn rig() -> Rig {
        let outer = tempfile::tempdir().expect("a directory");
        let work = outer.path().join("work");
        let elsewhere = outer.path().join("elsewhere");
        std::fs::create_dir_all(&work).expect("work");
        std::fs::create_dir_all(&elsewhere).expect("elsewhere");
        Rig {
            work: std::fs::canonicalize(&work).expect("work path"),
            elsewhere: std::fs::canonicalize(&elsewhere).expect("elsewhere path"),
            _outer: outer,
        }
    }

    fn access(rig: &Rig, mode: SandboxMode, mounts: Vec<Mount>) -> Access {
        Access::new(
            &SessionScope {
                thread_id: "t-1".to_owned(),
                tool_set: ToolSet::Plain,
                directory: rig.work.display().to_string(),
                cwd: None,
                sandbox_mode: mode,
                mounts,
            },
            &[],
        )
    }

    fn text_of(output: &ToolOutput) -> &str {
        match output {
            ToolOutput::Text(text) => text,
            other => panic!("text was expected, and {other:?} came"),
        }
    }

    fn read_request(path: &str) -> ReadRequest<'_> {
        ReadRequest {
            path,
            offset: None,
            limit: None,
            directory_advice: "List it with the list tool.",
        }
    }

    #[test]
    fn a_file_is_read_with_line_numbers_from_the_line_asked_for() {
        let rig = rig();
        std::fs::write(rig.work.join("a.txt"), "one\ntwo\nthree\nfour\n").expect("file");
        let access = access(&rig, SandboxMode::Workspace, vec![]);
        let all = read(&access, &rig.work, &read_request("a.txt")).expect("read");
        assert_eq!(
            text_of(&all),
            "     1\u{2192}one\n     2\u{2192}two\n     3\u{2192}three\n     4\u{2192}four\n"
        );
        let part = read(
            &access,
            &rig.work,
            &ReadRequest {
                offset: Some(2),
                limit: Some(2),
                ..read_request("a.txt")
            },
        )
        .expect("read part");
        let text = text_of(&part);
        assert!(
            text.starts_with("     2\u{2192}two\n     3\u{2192}three\n"),
            "{text}"
        );
        assert!(text.contains("Continue with offset 4"), "{text}");
    }

    #[test]
    fn a_read_past_the_end_and_a_read_of_an_empty_file_say_so() {
        let rig = rig();
        std::fs::write(rig.work.join("empty.txt"), "").expect("file");
        std::fs::write(rig.work.join("two.txt"), "a\nb").expect("file");
        let access = access(&rig, SandboxMode::Open, vec![]);
        let empty = read(&access, &rig.work, &read_request("empty.txt")).expect("read");
        assert!(text_of(&empty).contains("is empty"));
        let past = read(
            &access,
            &rig.work,
            &ReadRequest {
                offset: Some(9),
                ..read_request("two.txt")
            },
        )
        .expect("read");
        assert!(text_of(&past).contains("has 2 lines"), "{}", text_of(&past));
        let last_without_newline =
            read(&access, &rig.work, &read_request("two.txt")).expect("read");
        assert_eq!(
            text_of(&last_without_newline),
            "     1\u{2192}a\n     2\u{2192}b\n"
        );
    }

    #[test]
    fn a_very_long_line_is_cut_and_a_very_long_result_stops_with_the_way_on() {
        let rig = rig();
        let long = "x".repeat(READ_LINE_CHARS + 500);
        std::fs::write(rig.work.join("long.txt"), format!("{long}\nshort\n")).expect("file");
        let many: String = (0..5000)
            .map(|n| format!("line number {n:05} {}\n", "y".repeat(40)))
            .collect();
        std::fs::write(rig.work.join("many.txt"), many).expect("file");
        let access = access(&rig, SandboxMode::Open, vec![]);
        let cut = read(&access, &rig.work, &read_request("long.txt")).expect("read");
        assert!(
            text_of(&cut).contains("[line cut"),
            "{}",
            &text_of(&cut)[..60]
        );
        let big = read(&access, &rig.work, &read_request("many.txt")).expect("read");
        let text = text_of(&big);
        assert!(text.len() <= READ_RESULT_BYTES + 200, "{}", text.len());
        assert!(
            text.contains("the result reached"),
            "{}",
            &text[text.len() - 120..]
        );
        assert!(text.contains("Continue with offset"));
    }

    #[test]
    fn a_directory_a_binary_file_and_an_image_are_each_answered_as_what_they_are() {
        let rig = rig();
        std::fs::create_dir_all(rig.work.join("sub")).expect("dir");
        std::fs::write(rig.work.join("blob.bin"), b"abc\0def").expect("file");
        let png: Vec<u8> = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        std::fs::write(rig.work.join("pic.png"), &png).expect("file");
        let access = access(&rig, SandboxMode::Workspace, vec![]);
        let dir = read(&access, &rig.work, &read_request("sub")).expect_err("a directory");
        assert_eq!(dir.code(), "is_directory");
        assert!(dir.to_string().contains("List it with the list tool."));
        let bin = read(&access, &rig.work, &read_request("blob.bin")).expect_err("binary");
        assert_eq!(bin.code(), "not_text");
        match read(&access, &rig.work, &read_request("pic.png")).expect("image") {
            ToolOutput::Image { data, media_type } => {
                assert_eq!(media_type, "image/png");
                let back = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .expect("base64");
                assert_eq!(back, png);
            }
            other => panic!("an image was expected, and {other:?} came"),
        }
    }

    #[test]
    fn a_read_outside_the_directory_is_refused_in_workspace_and_read_only_and_allowed_in_open() {
        let rig = rig();
        let secret = rig.elsewhere.join("secret.txt");
        std::fs::write(&secret, "hidden").expect("file");
        let path = secret.display().to_string();
        for mode in [SandboxMode::Workspace, SandboxMode::ReadOnly] {
            let refused = read(&access(&rig, mode, vec![]), &rig.work, &read_request(&path))
                .expect_err("outside");
            assert_eq!(refused.code(), "outside_sandbox", "{mode:?}");
            let by_dots = read(
                &access(&rig, mode, vec![]),
                &rig.work,
                &read_request("../elsewhere/secret.txt"),
            )
            .expect_err("outside by ..");
            assert_eq!(by_dots.code(), "outside_sandbox", "{mode:?}");
        }
        let allowed = read(
            &access(&rig, SandboxMode::Open, vec![]),
            &rig.work,
            &read_request(&path),
        )
        .expect("open");
        assert!(text_of(&allowed).contains("hidden"));
    }

    #[test]
    fn a_write_creates_parents_replaces_content_and_keeps_the_permissions_of_the_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let rig = rig();
        let access = access(&rig, SandboxMode::Workspace, vec![]);
        let (placed, written) =
            write(&access, &rig.work, "deep/er/file.txt", "hello").expect("write");
        assert_eq!(
            written,
            Written {
                bytes: 5,
                created: true
            }
        );
        assert_eq!(
            std::fs::read_to_string(&placed.real).expect("read back"),
            "hello"
        );
        let script = rig.work.join("run.sh");
        std::fs::write(&script, "old").expect("file");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("mode");
        let (_, replaced) = write(&access, &rig.work, "run.sh", "new content").expect("write");
        assert_eq!(
            replaced,
            Written {
                bytes: 11,
                created: false
            }
        );
        assert_eq!(
            std::fs::read_to_string(&script).expect("read back"),
            "new content"
        );
        assert_eq!(
            std::fs::metadata(&script)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    #[test]
    fn a_write_is_refused_where_the_mode_a_read_only_mount_or_a_link_out_forbids_it() {
        let rig = rig();
        let mounts = vec![Mount {
            name: "src".to_owned(),
            path: rig.elsewhere.display().to_string(),
            read_only: true,
        }];
        symlink(&rig.elsewhere, rig.work.join("door")).expect("link");
        let access_workspace = access(&rig, SandboxMode::Workspace, vec![]);
        let outside =
            write(&access_workspace, &rig.work, "../elsewhere/x.txt", "x").expect_err("outside");
        assert_eq!(outside.code(), "outside_sandbox");
        let through_link = write(&access_workspace, &rig.work, "door/x.txt", "x")
            .expect_err("a link that leads out");
        assert_eq!(through_link.code(), "outside_sandbox");
        assert!(!rig.elsewhere.join("x.txt").exists());
        let in_mount = write(
            &access(&rig, SandboxMode::Workspace, mounts.clone()),
            &rig.work,
            "door/x.txt",
            "x",
        )
        .expect_err("read-only mount");
        assert_eq!(in_mount.code(), "read_only");
        let read_only_mode = write(
            &access(&rig, SandboxMode::ReadOnly, mounts),
            &rig.work,
            "new.txt",
            "x",
        )
        .expect_err("read_only");
        assert_eq!(read_only_mode.code(), "read_only");
        assert!(!rig.work.join("new.txt").exists());
    }

    #[test]
    fn an_edit_replaces_exact_text_once_unless_all_is_asked_for() {
        let rig = rig();
        let path = rig.work.join("e.txt");
        std::fs::write(&path, "alpha beta alpha").expect("file");
        let access = access(&rig, SandboxMode::Workspace, vec![]);
        let ambiguous =
            edit(&access, &rig.work, "e.txt", "alpha", "omega", false).expect_err("twice");
        assert_eq!(ambiguous.code(), "ambiguous");
        assert_eq!(
            std::fs::read_to_string(&path).expect("unchanged"),
            "alpha beta alpha"
        );
        let missing =
            edit(&access, &rig.work, "e.txt", "gamma", "omega", false).expect_err("not there");
        assert_eq!(missing.code(), "no_match");
        let (_, once) = edit(&access, &rig.work, "e.txt", "beta", "BETA", false).expect("once");
        assert_eq!(once, 1);
        let (_, all) = edit(&access, &rig.work, "e.txt", "alpha", "omega", true).expect("all");
        assert_eq!(all, 2);
        assert_eq!(
            std::fs::read_to_string(&path).expect("edited"),
            "omega BETA omega"
        );
        assert_eq!(
            edit(&access, &rig.work, "e.txt", "", "x", false)
                .expect_err("empty")
                .code(),
            "invalid_input"
        );
        assert_eq!(
            edit(&access, &rig.work, "e.txt", "x", "x", false)
                .expect_err("same")
                .code(),
            "invalid_input"
        );
    }

    #[test]
    fn a_listing_has_directories_first_then_names_with_sizes_and_goes_as_deep_as_asked() {
        let rig = rig();
        std::fs::create_dir_all(rig.work.join("src/inner")).expect("dirs");
        std::fs::create_dir_all(rig.work.join("a_dir")).expect("dirs");
        std::fs::write(rig.work.join("b.txt"), "12345").expect("file");
        std::fs::write(rig.work.join("a.txt"), "1").expect("file");
        std::fs::write(rig.work.join("src/lib.rs"), "fn main() {}").expect("file");
        symlink(&rig.elsewhere, rig.work.join("door")).expect("link");
        let access = access(&rig, SandboxMode::Workspace, vec![]);
        let shallow = list(&access, &rig.work, ".", None).expect("list");
        let names: Vec<&str> = shallow.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, ["a_dir/", "src/", "a.txt", "b.txt", "door"]);
        assert_eq!(shallow.entries[3].size, Some(5));
        assert_eq!(shallow.entries[4].kind, "symlink");
        let deep = list(&access, &rig.work, ".", Some(3)).expect("list");
        let names: Vec<&str> = deep.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(
            names.contains(&"src/inner/") && names.contains(&"src/lib.rs"),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|n| n.starts_with("door/")),
            "a link is not followed"
        );
        assert_eq!(
            list(&access, &rig.work, ".", Some(5))
                .expect_err("too deep")
                .code(),
            "invalid_input"
        );
        assert_eq!(
            list(&access, &rig.work, "../elsewhere", None)
                .expect_err("outside")
                .code(),
            "outside_sandbox"
        );
    }

    #[test]
    fn a_pipe_is_not_opened_by_any_file_tool() {
        let rig = rig();
        let fifo = rig.work.join("pipe");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if !made.is_ok_and(|status| status.success()) {
            return;
        }
        let access = access(&rig, SandboxMode::Open, vec![]);
        assert_eq!(
            read(&access, &rig.work, &read_request("pipe"))
                .expect_err("a pipe")
                .code(),
            "not_text"
        );
        assert_eq!(
            write(&access, &rig.work, "pipe", "x")
                .expect_err("a pipe")
                .code(),
            "not_text"
        );
        assert_eq!(
            edit(&access, &rig.work, "pipe", "a", "b", false)
                .expect_err("a pipe")
                .code(),
            "not_text"
        );
    }

    #[test]
    fn a_listing_is_cut_at_the_most_entries_it_returns_and_says_so() {
        let rig = rig();
        for n in 0..(LIST_MAX_ENTRIES + 5) {
            std::fs::write(rig.work.join(format!("f{n:05}")), "").expect("file");
        }
        let access = access(&rig, SandboxMode::Open, vec![]);
        let listing = list(&access, &rig.work, ".", None).expect("list");
        assert_eq!(listing.entries.len(), LIST_MAX_ENTRIES);
        assert!(listing.truncated);
    }
}
