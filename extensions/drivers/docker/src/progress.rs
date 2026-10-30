//! What a long operation tells while it runs, and the end of its log kept for a failure.
//!
//! Pulling, building and loading an image take minutes and say what they are doing as they go. The operations of
//! the driver report each line to a [`ProgressSink`] (the node shows it as the progress of the phase), and keep the
//! last [`LOG_TAIL_BYTES`] of the text in a [`LogTail`] so that the error of a failure carries what led to it.

use std::sync::Mutex;

/// How much of the end of a log is kept for the error of a failed operation: 64 KiB, which is hundreds of lines
/// of a build and more than anyone reads.
pub const LOG_TAIL_BYTES: usize = 64 * 1024;

/// Which operation a line of progress is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// A pull from a registry.
    Pull,
    /// A build of a Dockerfile.
    Build,
    /// A load of an image archive.
    Load,
}

/// Whoever wants to know how an operation is going.
pub trait ProgressSink: Send + Sync {
    /// One line of progress, without its line break.
    fn line(&self, phase: Phase, text: &str);
}

/// A sink that wants to hear nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Silent;

impl ProgressSink for Silent {
    fn line(&self, _: Phase, _: &str) {}
}

/// A sink that keeps the lines it is given, for a test or for a caller that reports them later.
#[derive(Debug, Default)]
pub struct Collected(Mutex<Vec<(Phase, String)>>);

impl Collected {
    /// The lines so far.
    #[must_use]
    pub fn lines(&self) -> Vec<(Phase, String)> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl ProgressSink for Collected {
    fn line(&self, phase: Phase, text: &str) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((phase, text.to_owned()));
    }
}

/// The end of a log: at most [`LOG_TAIL_BYTES`] of the text written to it, cut at a line where it can be.
#[derive(Debug, Default)]
pub struct LogTail {
    text: String,
}

impl LogTail {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a piece of the log.
    pub fn push(&mut self, piece: &str) {
        self.text.push_str(piece);
        if self.text.len() > LOG_TAIL_BYTES {
            let mut cut = self.text.len() - LOG_TAIL_BYTES;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            // Begin at a whole line when there is one near.
            if let Some(newline) = self.text[cut..].find('\n') {
                cut += newline + 1;
            }
            self.text.drain(..cut);
        }
    }

    /// The end of the log.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_end_of_a_long_log_is_kept_and_it_begins_at_a_line() {
        let mut tail = LogTail::new();
        for number in 0..20_000 {
            tail.push(&format!(
                "step {number}: doing something that takes a line\n"
            ));
        }
        assert!(tail.as_str().len() <= LOG_TAIL_BYTES);
        assert!(tail.as_str().len() > LOG_TAIL_BYTES - 200);
        assert!(tail.as_str().starts_with("step "));
        assert!(
            tail.as_str()
                .ends_with("step 19999: doing something that takes a line\n")
        );
    }

    #[test]
    fn a_short_log_is_kept_whole_and_the_cut_never_splits_a_character() {
        let mut tail = LogTail::new();
        tail.push("one\n");
        tail.push("two\n");
        assert_eq!(tail.as_str(), "one\ntwo\n");
        let mut wide = LogTail::new();
        wide.push(&"é".repeat(LOG_TAIL_BYTES));
        assert!(wide.as_str().len() <= LOG_TAIL_BYTES);
        assert!(wide.as_str().chars().all(|c| c == 'é'));
    }

    #[test]
    fn collected_lines_are_kept_in_order() {
        let sink = Collected::default();
        sink.line(Phase::Build, "a");
        sink.line(Phase::Pull, "b");
        assert_eq!(
            sink.lines(),
            vec![
                (Phase::Build, "a".to_owned()),
                (Phase::Pull, "b".to_owned())
            ]
        );
    }
}
