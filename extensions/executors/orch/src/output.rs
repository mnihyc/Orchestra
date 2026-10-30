//! What a command's output is cut to in a result, and the file that keeps it.
//!
//! A command's whole output is kept in a file of its own, which the file tools can read; what a result
//! carries of it is bounded, so that one noisy command cannot fill the model's context. Both are cut the same
//! way: the beginning and the end are kept and the middle is left out, with a line that says how much,
//! because the start of a build log says what was run and the end says how it went.
//!
//! The figures are those of the command output the adapters keep: 16 KiB shown inline, and at most 64 MiB kept
//! for one command, of which the last quarter is the end.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// What a result shows of a command's output at most: the figure the adapters use for what they keep
/// inline. Half of it is the beginning and half the end.
pub const DEFAULT_INLINE_BYTES: usize = 16 * 1024;

/// What is kept of one command's output at most, in its file: the figure the adapters cap a command's
/// output at.
pub const DEFAULT_KEPT_BYTES: u64 = 64 * 1024 * 1024;

/// How much of the kept output is its end: a quarter, which is enough to see how a long run ended while the
/// beginning, which says what was run and with which settings, keeps three quarters.
pub const DEFAULT_TAIL_BYTES: u64 = DEFAULT_KEPT_BYTES / 4;

/// The bounds of a command's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputLimits {
    /// What a result shows of the output at most.
    pub inline_bytes: usize,
    /// What the file keeps of it at most.
    pub kept_bytes: u64,
    /// How much of what the file keeps is the end of the output.
    pub tail_bytes: u64,
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            inline_bytes: DEFAULT_INLINE_BYTES,
            kept_bytes: DEFAULT_KEPT_BYTES,
            tail_bytes: DEFAULT_TAIL_BYTES,
        }
    }
}

/// A bounded view of a stream of bytes: its beginning and its end.
#[derive(Debug, Clone)]
pub struct HeadTail {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    head_capacity: usize,
    tail_capacity: usize,
    total: u64,
}

/// A view as text, and how much was left out of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The text, with a line where the middle was left out.
    pub text: String,
    /// How many bytes were left out.
    pub omitted: u64,
}

impl HeadTail {
    /// A view of at most `capacity` bytes, half of them the beginning and half the end.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let head_capacity = capacity / 2;
        Self {
            head: Vec::new(),
            tail: VecDeque::new(),
            head_capacity,
            tail_capacity: capacity - head_capacity,
            total: 0,
        }
    }

    /// Takes in the next `bytes` of the stream.
    pub fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        let mut rest = bytes;
        if self.head.len() < self.head_capacity {
            let room = self.head_capacity - self.head.len();
            let take = room.min(rest.len());
            self.head.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
        }
        if !rest.is_empty() {
            // Only the last `tail_capacity` bytes of what is left can be in the view.
            let skip = rest.len().saturating_sub(self.tail_capacity);
            self.tail.extend(&rest[skip..]);
            let surplus = self.tail.len().saturating_sub(self.tail_capacity);
            self.tail.drain(..surplus);
        }
    }

    /// How many bytes the stream has had in all.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }

    /// Whether nothing has come yet.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// How many bytes are not in the view.
    #[must_use]
    pub fn omitted(&self) -> u64 {
        self.total - (self.head.len() + self.tail.len()) as u64
    }

    /// The view as text. Bytes that are not text become the replacement character, and a character the cut
    /// went through is left out whole.
    #[must_use]
    pub fn render(&self) -> Rendered {
        let omitted = self.omitted();
        let tail: Vec<u8> = self.tail.iter().copied().collect();
        if omitted == 0 {
            let mut whole = self.head.clone();
            whole.extend_from_slice(&tail);
            return Rendered {
                text: String::from_utf8_lossy(&whole).into_owned(),
                omitted,
            };
        }
        let head_text = String::from_utf8_lossy(utf8_prefix(&self.head));
        let tail_text = String::from_utf8_lossy(utf8_suffix(&tail));
        Rendered {
            text: format!("{head_text}{}{tail_text}", omitted_line(omitted)),
            omitted,
        }
    }

    /// Forgets everything that came, and starts counting again.
    pub fn clear(&mut self) {
        self.head.clear();
        self.tail.clear();
        self.total = 0;
    }
}

/// The line that stands where the middle was left out.
#[must_use]
pub fn omitted_line(omitted: u64) -> String {
    format!("\n[... {omitted} bytes omitted ...]\n")
}

/// The longest start of `bytes` that does not end inside a character.
fn utf8_prefix(bytes: &[u8]) -> &[u8] {
    match std::str::from_utf8(bytes) {
        Err(error) if error.error_len().is_none() => &bytes[..error.valid_up_to()],
        _ => bytes,
    }
}

/// `bytes` without the continuation bytes of a character that begins before them.
fn utf8_suffix(bytes: &[u8]) -> &[u8] {
    // A character has at most three continuation bytes.
    let skip = bytes
        .iter()
        .take(3)
        .take_while(|byte| (**byte & 0b1100_0000) == 0b1000_0000)
        .count();
    &bytes[skip..]
}

/// The file that keeps the whole output of one command, as far as the limits allow.
#[derive(Debug)]
pub struct Capture {
    file: Option<File>,
    path: PathBuf,
    head_capacity: u64,
    tail_capacity: usize,
    in_file: u64,
    total: u64,
    ring: VecDeque<u8>,
    finished: bool,
}

impl Capture {
    /// Starts the file at `path`; it is private to the user the executor runs as.
    pub fn create(path: PathBuf, limits: &OutputLimits) -> std::io::Result<Self> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        let tail_capacity = usize::try_from(limits.tail_bytes.min(limits.kept_bytes))
            .unwrap_or(usize::MAX)
            .min(isize::MAX as usize);
        Ok(Self {
            file: Some(file),
            path,
            head_capacity: limits.kept_bytes.saturating_sub(tail_capacity as u64),
            tail_capacity,
            in_file: 0,
            total: 0,
            ring: VecDeque::new(),
            finished: false,
        })
    }

    /// Where the file is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How many bytes the output has had in all.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }

    /// Takes in the next `bytes` of the output: written at once while the beginning is not full, and kept
    /// aside as the end otherwise.
    pub fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        let mut rest = bytes;
        if self.in_file < self.head_capacity {
            let room = usize::try_from(self.head_capacity - self.in_file).unwrap_or(usize::MAX);
            let take = room.min(rest.len());
            if let Some(file) = self.file.as_mut()
                && file.write_all(&rest[..take]).is_err()
            {
                // The disk is full or the file is gone: the result still has its inline view.
                self.file = None;
            }
            self.in_file += take as u64;
            rest = &rest[take..];
        }
        if !rest.is_empty() {
            let skip = rest.len().saturating_sub(self.tail_capacity);
            self.ring.extend(&rest[skip..]);
            let surplus = self.ring.len().saturating_sub(self.tail_capacity);
            self.ring.drain(..surplus);
        }
    }

    /// Completes the file once the command is done: where the middle was left out, a line says how much,
    /// and the end follows.
    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if self.total > self.in_file {
            let omitted = self.total - self.in_file - self.ring.len() as u64;
            let mut end = Vec::new();
            if omitted > 0 {
                end.extend_from_slice(omitted_line(omitted).as_bytes());
            }
            end.extend(self.ring.iter());
            // A failed write leaves the file as it is; nothing else can be done about it.
            let _ = file.write_all(&end);
        }
        let _ = file.flush();
        self.file = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_that_fits_is_shown_whole_however_it_arrives() {
        let mut view = HeadTail::new(16);
        for piece in ["abc", "defgh", "ijkl"] {
            view.push(piece.as_bytes());
        }
        let shown = view.render();
        assert_eq!(shown.text, "abcdefghijkl");
        assert_eq!(shown.omitted, 0);
    }

    #[test]
    fn output_that_does_not_fit_keeps_its_beginning_and_its_end_and_says_how_much_is_between() {
        let mut view = HeadTail::new(10);
        view.push(b"0123456789abcdefghijklmnopqrstuvwxyz");
        let shown = view.render();
        assert_eq!(shown.omitted, 26);
        assert!(shown.text.starts_with("01234"), "{}", shown.text);
        assert!(shown.text.ends_with("vwxyz"), "{}", shown.text);
        assert!(shown.text.contains("[... 26 bytes omitted ...]"));
        assert_eq!(view.total(), 36);
    }

    #[test]
    fn the_end_of_the_view_is_the_end_of_the_output_even_when_it_came_in_small_pieces() {
        let mut view = HeadTail::new(8);
        for byte in b"abcdefghijklmnop" {
            view.push(&[*byte]);
        }
        let shown = view.render();
        assert!(shown.text.starts_with("abcd") && shown.text.ends_with("mnop"));
        assert_eq!(shown.omitted, 8);
    }

    #[test]
    fn a_cut_never_leaves_half_a_character() {
        let mut view = HeadTail::new(8);
        // Each of these characters is three bytes: the cut through the middle of one is dropped whole.
        view.push("日本語日本語日本語".as_bytes());
        let shown = view.render();
        assert!(!shown.text.contains('\u{FFFD}'), "{}", shown.text);
        assert!(shown.omitted > 0);
    }

    #[test]
    fn bytes_that_are_not_text_do_not_stop_the_view() {
        let mut view = HeadTail::new(32);
        view.push(&[0x66, 0xff, 0x6f]);
        assert_eq!(view.render().text, "f\u{FFFD}o");
    }

    fn file_of(limits: OutputLimits) -> (tempfile::TempDir, Capture) {
        let dir = tempfile::tempdir().expect("a directory");
        let capture = Capture::create(dir.path().join("out.log"), &limits).expect("a file");
        (dir, capture)
    }

    #[test]
    fn the_file_keeps_everything_while_the_output_is_within_what_is_kept() {
        let (_dir, mut capture) = file_of(OutputLimits {
            inline_bytes: 8,
            kept_bytes: 100,
            tail_bytes: 25,
        });
        capture.push(b"hello ");
        capture.push(b"world\n");
        capture.finish();
        assert_eq!(
            std::fs::read_to_string(capture.path()).expect("the file"),
            "hello world\n"
        );
    }

    #[test]
    fn beyond_what_is_kept_the_middle_is_left_out_of_the_file_and_the_file_says_so() {
        let (_dir, mut capture) = file_of(OutputLimits {
            inline_bytes: 8,
            kept_bytes: 20,
            tail_bytes: 5,
        });
        for chunk in ["0123456789", "abcdefghij", "ABCDEFGHIJ", "KLMNOPQRST"] {
            capture.push(chunk.as_bytes());
        }
        capture.finish();
        let kept = std::fs::read_to_string(capture.path()).expect("the file");
        assert!(kept.starts_with("0123456789abcde"), "{kept}");
        assert!(kept.ends_with("PQRST"), "{kept}");
        assert!(kept.contains("[... 20 bytes omitted ...]"), "{kept}");
        assert_eq!(capture.total(), 40);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private_to_the_user_that_runs_the_executor() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_dir, capture) = file_of(OutputLimits::default());
        let mode = std::fs::metadata(capture.path())
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "{mode:o}");
    }
}
