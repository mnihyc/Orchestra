//! Starting the program of an ACP agent: the launcher, its streams, the end of its process, files written for its home.
//!
//! What runs the program is given to the adapter as a [`Launcher`]: it turns a [`LaunchRequest`] (a program, its
//! arguments, the variables it finds, a working directory) into a [`HarnessProcess`] with the three standard
//! streams and a handle to end it. The connector's launcher starts a plain program inside the runtime
//! ([`DirectLauncher`]); a test can start the program in a container, or play it from a pipe.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};

/// How many of the last lines the program wrote on its error output are kept, to say why it ended: the last
/// few lines of a failing start say what is wrong, and more only costs memory.
pub const STDERR_TAIL_LINES: usize = 12;

/// How much of one line of a program's error output is kept: a program that writes a line without an end does not
/// grow the adapter's memory (a default; the rest of such a line is dropped).
pub const STDERR_LINE_BYTES: usize = 8 * 1024;

/// What to run.
#[derive(Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    /// The program: a path, or a name found on the path.
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// The variables it finds beyond those of the connector's own environment. Some are secrets: the
    /// launcher passes them as the process's environment and never as an argument.
    pub env: BTreeMap<String, String>,
    /// Where it runs; the connector's own directory when none is named.
    pub cwd: Option<PathBuf>,
}

impl fmt::Debug for LaunchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaunchRequest")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("cwd", &self.cwd)
            .finish()
    }
}

/// How a process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessExit {
    /// The exit code, where it exited by itself.
    pub code: Option<i32>,
    /// What else is known of the end, such as the signal that ended it.
    pub detail: String,
}

impl fmt::Display for ProcessExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            Some(code) => write!(f, "exited with code {code}")?,
            None => f.write_str("ended without an exit code")?,
        }
        if !self.detail.is_empty() {
            write!(f, " ({})", self.detail)?;
        }
        Ok(())
    }
}

/// What ends or waits for a process the adapter started.
#[async_trait]
pub trait ProcessControl: Send {
    /// Waits until the process has ended. Cancel-safe.
    async fn wait(&mut self) -> ProcessExit;

    /// Ends the process now, if it still runs.
    fn kill(&mut self);
}

/// A running program (or a one-shot command of it) with its streams.
pub struct HarnessProcess {
    /// What the program reads.
    pub stdin: Box<dyn AsyncWrite + Send + Unpin>,
    /// What the program writes.
    pub stdout: Box<dyn AsyncRead + Send + Unpin>,
    /// What it writes for the log, if the launcher gives it.
    pub stderr: Option<Box<dyn AsyncRead + Send + Unpin>>,
    /// The handle that waits for it and ends it.
    pub control: Box<dyn ProcessControl>,
}

impl fmt::Debug for HarnessProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HarnessProcess").finish_non_exhaustive()
    }
}

/// Why a process could not be started.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    /// The program is not there or could not be run.
    #[error("{program} could not be started: {source}")]
    Spawn {
        /// The program.
        program: String,
        /// Why.
        source: io::Error,
    },
}

/// Starts the programs the adapter needs.
#[async_trait]
pub trait Launcher: Send + Sync + 'static {
    /// Starts what `request` describes.
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError>;
}

/// Starts the program as a process of the machine, in the connector's own environment with the request's
/// variables added.
#[derive(Debug, Clone, Copy, Default)]
pub struct DirectLauncher;

struct ChildControl(tokio::process::Child);

#[async_trait]
impl ProcessControl for ChildControl {
    async fn wait(&mut self) -> ProcessExit {
        match self.0.wait().await {
            Ok(status) => ProcessExit {
                code: status.code(),
                detail: if status.code().is_none() {
                    status.to_string()
                } else {
                    String::new()
                },
            },
            Err(error) => ProcessExit {
                code: None,
                detail: format!("its end could not be awaited: {error}"),
            },
        }
    }

    fn kill(&mut self) {
        let _ = self.0.start_kill();
    }
}

#[async_trait]
impl Launcher for DirectLauncher {
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        let mut command = tokio::process::Command::new(&request.program);
        // The program's variables are exactly the request's: nothing of the connector's own environment passes
        // unless the manifest names it.
        command
            .args(&request.args)
            .env_clear()
            .envs(&request.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &request.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(|source| LaunchError::Spawn {
            program: request.program.clone(),
            source,
        })?;
        let broken = |what: &str| LaunchError::Spawn {
            program: request.program.clone(),
            source: io::Error::other(format!("the {what} of the process is not piped")),
        };
        let stdin = child.stdin.take().ok_or_else(|| broken("input"))?;
        let stdout = child.stdout.take().ok_or_else(|| broken("output"))?;
        let stderr = child.stderr.take();
        Ok(HarnessProcess {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            stderr: stderr.map(|stream| Box::new(stream) as Box<dyn AsyncRead + Send + Unpin>),
            control: Box::new(ChildControl(child)),
        })
    }
}

/// What the tail of a program's error output holds.
#[derive(Debug, Default)]
struct Tail {
    /// The last lines, the oldest first.
    lines: Vec<String>,
    /// How many lines the program has written in all.
    kept: u64,
    /// How many it had written when [`StderrTail::mark`] was last called.
    marked: u64,
}

/// The last lines a program wrote on its error output, shared between the task that reads them and whoever
/// reports why the program ended.
#[derive(Debug, Clone, Default)]
pub struct StderrTail(Arc<Mutex<Tail>>);

impl StderrTail {
    /// Reads `stream` line by line until it ends, keeping the last lines and logging each at debug level.
    pub async fn follow(self, stream: Box<dyn AsyncRead + Send + Unpin>) {
        let mut reader = BufReader::new(stream);
        let mut line: Vec<u8> = Vec::new();
        loop {
            let (consumed, ended) = match reader.fill_buf().await {
                Ok(chunk) if !chunk.is_empty() => {
                    match chunk.iter().position(|byte| *byte == b'\n') {
                        Some(at) => {
                            Self::add(&mut line, &chunk[..at]);
                            (at + 1, true)
                        }
                        None => {
                            Self::add(&mut line, chunk);
                            (chunk.len(), false)
                        }
                    }
                }
                _ => break,
            };
            reader.consume(consumed);
            if ended {
                self.keep(&mut line);
            }
        }
        if !line.is_empty() {
            self.keep(&mut line);
        }
    }

    /// Adds to a line as much as a line may hold; the rest of a longer one is dropped.
    fn add(line: &mut Vec<u8>, piece: &[u8]) {
        let room = STDERR_LINE_BYTES.saturating_sub(line.len());
        line.extend_from_slice(&piece[..piece.len().min(room)]);
    }

    /// Keeps a finished line among the last ones.
    fn keep(&self, line: &mut Vec<u8>) {
        let text = String::from_utf8_lossy(line)
            .trim_end_matches('\r')
            .to_owned();
        line.clear();
        tracing::debug!(target: "orch_adapter_acp::agent", "{text}");
        let mut tail = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if tail.lines.len() == STDERR_TAIL_LINES {
            tail.lines.remove(0);
        }
        tail.lines.push(text);
        tail.kept += 1;
    }

    /// The last lines, one after the other, with the colour codes a terminal logger writes taken out.
    #[must_use]
    pub fn text(&self) -> String {
        Self::joined(&self.0.lock().unwrap_or_else(PoisonError::into_inner).lines)
    }

    /// Notes that the program is up: what it wrote until now was said while it started.
    pub fn mark(&self) {
        let mut tail = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        tail.marked = tail.kept;
    }

    /// What the program wrote since [`StderrTail::mark`], among its last lines: what can say why a program that
    /// was up has ended. The notes of its start are in [`StderrTail::text`], which a failure to start shows.
    #[must_use]
    pub fn recent(&self) -> String {
        let tail = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let newer = usize::try_from(tail.kept - tail.marked)
            .unwrap_or(usize::MAX)
            .min(tail.lines.len());
        Self::joined(&tail.lines[tail.lines.len() - newer..])
    }

    fn joined(lines: &[String]) -> String {
        lines
            .iter()
            .map(|line| strip_escapes(line))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A line without the `ESC [ ... m` sequences a logger colours it with.
fn strip_escapes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Writes `contents` to `path` through a temporary file beside it, so that a reader never sees half of it,
/// readable by the user of this process alone.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)
}

/// Makes a directory the program will be told to use, readable by the user of this process alone.
pub fn prepare_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn the_error_output_of_a_program_keeps_a_few_short_lines_however_much_it_writes() {
        let (mut writer, reader) = tokio::io::duplex(64 * 1024);
        let tail = StderrTail::default();
        let following = tokio::spawn(tail.clone().follow(Box::new(reader)));
        // A line of sixteen megabytes with no end, then a hundred thousand short lines.
        let long = vec![b'x'; 16 * 1024 * 1024];
        writer.write_all(&long).await.unwrap();
        writer.write_all(b"\n").await.unwrap();
        for number in 0..100_000 {
            writer
                .write_all(format!("line {number}\n").as_bytes())
                .await
                .unwrap();
        }
        writer
            .write_all(b"the last one, with no end")
            .await
            .unwrap();
        drop(writer);
        following.await.unwrap();
        let text = tail.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), STDERR_TAIL_LINES);
        assert_eq!(lines.last(), Some(&"the last one, with no end"));
        assert_eq!(lines[lines.len() - 2], "line 99999");
        assert!(text.len() <= STDERR_TAIL_LINES * STDERR_LINE_BYTES);
    }

    /// Waits until the follower of a tail has kept a line that holds `word`.
    async fn until_kept(tail: &StderrTail, word: &str) {
        for _ in 0..500 {
            if tail.text().contains(word) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("{word:?} was not read: {}", tail.text());
    }

    #[tokio::test]
    async fn what_a_program_said_while_it_started_is_not_what_it_has_said_since() {
        let (mut writer, reader) = tokio::io::duplex(4096);
        let tail = StderrTail::default();
        let following = tokio::spawn(tail.clone().follow(Box::new(reader)));
        writer
            .write_all(b"a part of the program is off\n")
            .await
            .unwrap();
        until_kept(&tail, "is off").await;
        tail.mark();
        assert_eq!(tail.recent(), "", "nothing was said since it was up");
        assert!(
            tail.text().contains("is off"),
            "a program that cannot start is explained by all it said"
        );
        writer
            .write_all(b"panicked at a.rs:1\nstack of the panic\n")
            .await
            .unwrap();
        until_kept(&tail, "stack of the panic").await;
        assert_eq!(tail.recent(), "panicked at a.rs:1\nstack of the panic");
        for number in 0..30 {
            writer
                .write_all(format!("line {number}\n").as_bytes())
                .await
                .unwrap();
        }
        until_kept(&tail, "line 29").await;
        let recent = tail.recent();
        assert_eq!(recent.lines().count(), STDERR_TAIL_LINES);
        assert!(
            !recent.contains("is off"),
            "what falls out of the tail is gone from both"
        );
        drop(writer);
        following.await.unwrap();
    }

    #[tokio::test]
    async fn one_long_line_is_cut_where_a_line_may_end_and_the_next_line_is_whole() {
        let (mut writer, reader) = tokio::io::duplex(1024);
        let tail = StderrTail::default();
        let following = tokio::spawn(tail.clone().follow(Box::new(reader)));
        let long = vec![b'y'; STDERR_LINE_BYTES * 3];
        writer.write_all(&long).await.unwrap();
        writer.write_all(b"\nshort\r\n").await.unwrap();
        drop(writer);
        following.await.unwrap();
        let text = tail.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), STDERR_LINE_BYTES);
        assert_eq!(lines[1], "short");
    }
}
