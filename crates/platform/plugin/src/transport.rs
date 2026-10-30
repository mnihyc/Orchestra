//! JSONL over spawned stdio or local socket attachment.
//!
//! A plugin is reached in one of two ways and speaks the same lines over both: the host starts the program and
//! talks over its standard input and output, or it attaches to a program that is already running, over a socket
//! in the file system or on the loopback address. This module opens either, and hands back the same four things:
//! a reader of lines, a writer, the started program (if the host started one) and the program's log.
//!
//! A line is what ends in a newline. The reader never lets a program make the host grow without bound: a line
//! longer than [`MAX_LINE_BYTES`] is read to its end and thrown away, and reported as [`Line::Oversized`] so that
//! it is counted like any other malformed line. What a started program writes to its standard error is its log:
//! [`forward_log`] writes it, line by line and apart from the host's own, and always reads it, because a program
//! that is not read blocks the moment the pipe is full.

use std::io;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

use bytes::BytesMut;
use orch_plugin_contract::registration::{ConnectAddress, PluginKind, PluginStart};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

/// The longest line the host reads from a plugin: 64 MiB. A connector forwards model requests of up to 32 MiB,
/// and the same request comes back translated, written as the text of a JSON string, which only grows by what
/// escaping adds; a line longer than twice the largest request is a program that has lost its way.
pub const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// The longest line of a plugin's log that is kept whole: 16 KiB. A longer one is cut, so that a program that
/// writes without ever ending a line costs the host a bounded amount of memory.
pub const MAX_LOG_LINE_BYTES: usize = 16 * 1024;

/// How many lines of a plugin's log are written to the host's log in one second. A program that floods its
/// standard error must not flood the file that the operator reads; two hundred a second is more than a person can
/// read and far less than a program can write. The lines over it are counted, and the count is written once a
/// second.
pub const LOG_LINES_PER_SECOND: u32 = 200;

/// The stretch of time in which [`LOG_LINES_PER_SECOND`] lines of a plugin's log are written: one second.
const LOG_WINDOW: Duration = Duration::from_secs(1);

/// How much is asked of the operating system at one read: 64 KiB, the size of a pipe's buffer.
const READ_CHUNK: usize = 64 * 1024;

/// Why a plugin could not be reached.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The program could not be started.
    #[error("the program {program:?} could not be started: {source}")]
    Spawn {
        /// The program.
        program: String,
        /// What the system said.
        source: io::Error,
    },
    /// The command names no program.
    #[error("the command names no program")]
    NoProgram,
    /// The started program has no standard input or output to talk over.
    #[error("the program {0:?} has no standard streams to talk over")]
    NoStreams(String),
    /// A running program could not be attached to.
    #[error("{address} could not be attached to: {source}")]
    Attach {
        /// The address.
        address: String,
        /// What the system said.
        source: io::Error,
    },
}

/// What one line read from a plugin turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A line of text, without its end.
    Text(String),
    /// A line longer than the bound, which was read to its end and dropped.
    Oversized,
    /// A line that is not UTF-8, and so cannot be JSON.
    NotUtf8,
}

/// Reads lines from a plugin, each at most a bound long.
///
/// Reading is cancel safe: what has been read stays in the reader when the future of [`LineReader::next`] is
/// dropped, so it can be raced against timers and signals.
pub struct LineReader {
    source: Box<dyn AsyncRead + Send + Unpin>,
    buffer: BytesMut,
    scanned: usize,
    skipping: bool,
    limit: usize,
}

impl LineReader {
    /// A reader of the lines of `source`, none longer than `limit` bytes.
    #[must_use]
    pub fn new(source: impl AsyncRead + Send + Unpin + 'static, limit: usize) -> Self {
        Self {
            source: Box::new(source),
            buffer: BytesMut::with_capacity(READ_CHUNK),
            scanned: 0,
            skipping: false,
            limit,
        }
    }

    /// The next line, or `None` at the end of the input. A last line that has no end is still a line.
    pub async fn next(&mut self) -> io::Result<Option<Line>> {
        loop {
            let ending = self.buffer[self.scanned..]
                .iter()
                .position(|byte| *byte == b'\n');
            if let Some(offset) = ending {
                let end = self.scanned + offset;
                let mut line = self.buffer.split_to(end + 1);
                self.scanned = 0;
                if std::mem::take(&mut self.skipping) || end > self.limit {
                    return Ok(Some(Line::Oversized));
                }
                line.truncate(end);
                return Ok(Some(decode(&line)));
            }
            self.scanned = self.buffer.len();
            if self.buffer.len() > self.limit {
                // No end in sight within the bound: forget what there is, and go on forgetting until the end.
                self.buffer.clear();
                self.scanned = 0;
                self.skipping = true;
            }
            if self.source.read_buf(&mut self.buffer).await? == 0 {
                let skipping = std::mem::take(&mut self.skipping);
                if self.buffer.is_empty() {
                    return Ok(skipping.then_some(Line::Oversized));
                }
                let rest = self.buffer.split();
                self.scanned = 0;
                return Ok(Some(if skipping {
                    Line::Oversized
                } else {
                    decode(&rest)
                }));
            }
        }
    }
}

/// A line of bytes, without its newline, as text; a carriage return before the newline is not part of it.
fn decode(bytes: &[u8]) -> Line {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => Line::Text(text.to_owned()),
        Err(_) => Line::NotUtf8,
    }
}

/// A program the host started.
///
/// It is ended when this value is dropped, so that a host that stops, or a test that fails, leaves nothing
/// running.
#[derive(Debug)]
pub struct Process {
    child: Child,
}

impl Process {
    /// Waits until the program has ended.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }

    /// Ends the program at once, and waits until the system has taken it away.
    pub async fn kill(&mut self) {
        // A program that is already gone cannot be killed again; there is nothing more to do for it.
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }

    /// The program's process number, while it runs.
    #[must_use]
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }
}

/// A plugin that has been started or attached to, before anything is said to it.
pub struct OpenLink {
    /// What the plugin writes, line by line.
    pub reader: LineReader,
    /// Where lines to the plugin are written.
    pub writer: Box<dyn AsyncWrite + Send + Unpin>,
    /// The program, when the host started it.
    pub process: Option<Process>,
    /// The program's standard error, when the host started it.
    pub log: Option<Box<dyn AsyncRead + Send + Unpin>>,
}

impl std::fmt::Debug for OpenLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenLink")
            .field("process", &self.process)
            .finish_non_exhaustive()
    }
}

/// Starts the program or attaches to the one that is running.
pub async fn open(start: &PluginStart) -> Result<OpenLink, TransportError> {
    match start {
        PluginStart::Command { argv, env, .. } => {
            let (program, arguments) = argv.split_first().ok_or(TransportError::NoProgram)?;
            let mut child = Command::new(program)
                .args(arguments)
                .envs(env)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|source| TransportError::Spawn {
                    program: program.clone(),
                    source,
                })?;
            let streams = (child.stdin.take(), child.stdout.take(), child.stderr.take());
            let (Some(stdin), Some(stdout), Some(stderr)) = streams else {
                return Err(TransportError::NoStreams(program.clone()));
            };
            Ok(OpenLink {
                reader: LineReader::new(stdout, MAX_LINE_BYTES),
                writer: Box::new(stdin),
                process: Some(Process { child }),
                log: Some(Box::new(stderr)),
            })
        }
        PluginStart::Attach(address) => {
            let attach_error = |source| TransportError::Attach {
                address: address.to_string(),
                source,
            };
            match address {
                ConnectAddress::Unix(path) => {
                    #[cfg(not(unix))]
                    {
                        let _ = path;
                        Err(attach_error(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "Unix plugin sockets are unavailable on this platform; use loopback TCP or a started program",
                        )))
                    }
                    #[cfg(unix)]
                    {
                        let stream = UnixStream::connect(path).await.map_err(attach_error)?;
                        let (read, write) = stream.into_split();
                        Ok(OpenLink {
                            reader: LineReader::new(read, MAX_LINE_BYTES),
                            writer: Box::new(write),
                            process: None,
                            log: None,
                        })
                    }
                }
                ConnectAddress::Tcp(port) => {
                    let stream = TcpStream::connect(("127.0.0.1", *port))
                        .await
                        .map_err(attach_error)?;
                    let (read, write) = stream.into_split();
                    Ok(OpenLink {
                        reader: LineReader::new(read, MAX_LINE_BYTES),
                        writer: Box::new(write),
                        process: None,
                        log: None,
                    })
                }
            }
        }
    }
}

/// What a program wrote to its log is text for people; anything that could move a terminal's cursor or change
/// its colours is shown as a replacement character, so that a plugin cannot write over the host's own lines.
fn tidy(line: &str) -> String {
    line.chars()
        .map(|c| {
            if c.is_control() && c != '\t' {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

/// Reads a started program's standard error for as long as it is open and writes each line to the host's log,
/// apart from the host's own lines: its target is `orch_plugin::log`, with the plugin's kind and name beside it.
/// At most [`LOG_LINES_PER_SECOND`] lines a second are written; the rest are counted and the count is written.
#[must_use]
pub fn forward_log(
    source: Box<dyn AsyncRead + Send + Unpin>,
    kind: PluginKind,
    name: String,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = LineReader::new(source, MAX_LOG_LINE_BYTES);
        let mut window = Instant::now();
        let mut written = 0u32;
        let mut dropped = 0u64;
        loop {
            let line = match reader.next().await {
                Ok(Some(line)) => line,
                Ok(None) | Err(_) => break,
            };
            if window.elapsed() >= LOG_WINDOW {
                if dropped > 0 {
                    tracing::warn!(
                        target: "orch_plugin::log", %kind, plugin = %name, dropped,
                        "lines of the plugin's log were not written: it writes too fast"
                    );
                }
                window = Instant::now();
                written = 0;
                dropped = 0;
            }
            if written >= LOG_LINES_PER_SECOND {
                dropped += 1;
                continue;
            }
            written += 1;
            match line {
                Line::Text(text) => {
                    tracing::info!(target: "orch_plugin::log", %kind, plugin = %name, "{}", tidy(&text));
                }
                Line::Oversized => {
                    tracing::info!(
                        target: "orch_plugin::log", %kind, plugin = %name,
                        "(a line of the plugin's log was too long and was cut)"
                    );
                }
                Line::NotUtf8 => {
                    tracing::info!(
                        target: "orch_plugin::log", %kind, plugin = %name,
                        "(a line of the plugin's log is not text)"
                    );
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn lines(input: &[u8], limit: usize) -> Vec<Line> {
        let mut reader = LineReader::new(std::io::Cursor::new(input.to_vec()), limit);
        let mut read = Vec::new();
        while let Some(line) = reader.next().await.unwrap() {
            read.push(line);
        }
        read
    }

    fn text(line: &str) -> Line {
        Line::Text(line.to_owned())
    }

    #[tokio::test]
    async fn lines_are_cut_at_the_newline_and_a_carriage_return_before_it_is_not_part_of_the_line()
    {
        assert_eq!(
            lines(b"one\ntwo\r\n\nthree", 100).await,
            [text("one"), text("two"), text(""), text("three")],
            "a last line without an end is still a line"
        );
        assert!(lines(b"", 100).await.is_empty());
    }

    #[tokio::test]
    async fn a_line_longer_than_the_bound_is_dropped_whole_and_the_next_one_is_read() {
        let mut input = vec![b'x'; 5000];
        input.extend_from_slice(b"\nok\n");
        assert_eq!(lines(&input, 1000).await, [Line::Oversized, text("ok")]);
        let mut unfinished = b"ok\n".to_vec();
        unfinished.extend(vec![b'y'; 5000]);
        assert_eq!(
            lines(&unfinished, 1000).await,
            [text("ok"), Line::Oversized],
            "an oversized line that the input ends in is dropped as well"
        );
    }

    #[tokio::test]
    async fn a_line_that_is_not_text_is_reported_and_reading_goes_on() {
        assert_eq!(
            lines(b"\xff\xfe\n{}\n", 100).await,
            [Line::NotUtf8, text("{}")]
        );
    }

    #[tokio::test]
    async fn a_line_that_arrives_in_pieces_is_read_whole() {
        let (mut near, far) = tokio::io::duplex(8);
        let mut reader = LineReader::new(far, 100);
        let writing = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            for piece in [&b"{\"id\""[..], b":1,\"res", b"ult\":{}}\n", b"next\n"] {
                near.write_all(piece).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        assert_eq!(
            reader.next().await.unwrap(),
            Some(text("{\"id\":1,\"result\":{}}"))
        );
        assert_eq!(reader.next().await.unwrap(), Some(text("next")));
        writing.await.unwrap();
        assert_eq!(reader.next().await.unwrap(), None);
    }

    #[test]
    fn the_log_of_a_plugin_cannot_move_the_cursor_of_the_host() {
        assert_eq!(tidy("\u{1b}[2Jhello\tworld"), "\u{fffd}[2Jhello\tworld");
    }
}
