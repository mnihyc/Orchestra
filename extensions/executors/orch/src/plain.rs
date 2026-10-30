//! The eight plain tools: shell with its output and stop, read, write, edit, list and search.
//!
//! The tools every harness's own tools have in common, few and plain on purpose, so that an adapter can map
//! its harness's calls to them or simply offer them under these names. Each takes an object and refuses a
//! member it does not list. Paths are absolute, or relative to the thread's working directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rmcp::model::{JsonObject, Tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::commands::{ExitInfo, Launch, MAX_OUTPUT_WAIT, Reason};
use crate::files::{self, ReadRequest};
use crate::sandbox::Need;
use crate::search::{self, DEFAULT_MAX_RESULTS, MAX_RESULTS_CEILING, PlainSearch, Where};
use crate::tools::{
    Call, ToolFault, ToolOutput, arguments, blocking, changes_things, reads_only, schema,
};

/// The most a foreground command may be asked to run: an hour, which is the bound of the harness's own
/// tools, so that a model's sense of how long a call may take holds.
pub const TIMEOUT_MAX_MS: i64 = 3_600_000;

/// What `shell` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShellArgs {
    /// The command, as text for the environment's shell.
    command: String,
    /// The directory to run it in; the thread's working directory when left out.
    cwd: Option<String>,
    /// How long a foreground command may run, in milliseconds: 120000 when left out, at most 3600000. With
    /// `background` it ends the command then.
    timeout_ms: Option<i64>,
    /// Run the command in the background: the call returns at once with a `process_id`, and the command runs
    /// until it ends or is stopped.
    background: Option<bool>,
}

/// What `shell_output` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShellOutputArgs {
    /// The `process_id` that `shell` gave.
    process_id: String,
    /// How long to wait, in milliseconds, for new output or for the end of the command: nothing when left
    /// out, at most 30000.
    wait_ms: Option<i64>,
}

/// What `shell_stop` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShellStopArgs {
    /// The `process_id` that `shell` gave.
    process_id: String,
}

/// What `read` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    /// The file: an absolute path, or relative to the working directory.
    path: String,
    /// The line to start at, counted from 1.
    offset: Option<i64>,
    /// How many lines at most; 2000 when left out.
    limit: Option<i64>,
}

/// What `write` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    /// The file: an absolute path, or relative to the working directory.
    path: String,
    /// The whole content of the file.
    content: String,
}

/// What `edit` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    /// The file: an absolute path, or relative to the working directory.
    path: String,
    /// The text to replace, exactly as it is in the file, with its white space.
    old: String,
    /// The text to put in its place.
    new: String,
    /// Replace every occurrence; without it `old` has to occur exactly once.
    all: Option<bool>,
}

/// What `list` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// The directory: an absolute path, or relative to the working directory.
    path: String,
    /// How many levels to list: 1 when left out, at most 4.
    depth: Option<i64>,
}

/// What `search` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    /// A regular expression, in the syntax of the Rust `regex` crate.
    pattern: String,
    /// The file or directory to search; the working directory when left out.
    path: Option<String>,
    /// Which files to search, by a glob such as `*.rs`.
    glob: Option<String>,
    /// How many matching lines at most; 200 when left out.
    max_results: Option<i64>,
}

/// The eight tools.
pub(crate) fn definitions() -> Vec<Tool> {
    vec![
        Tool::new(
            "shell",
            "Run a shell command in the thread's environment and wait for it. The answer has the exit code, \
             the standard output and the standard error. A foreground command may run for two minutes unless \
             `timeout_ms` says otherwise (at most 3600000); anything longer belongs in the background with \
             `background: true`, which answers at once with a `process_id` for `shell_output` and `shell_stop`. \
             Long output is cut in the middle, and the whole of it is kept in the file `output_file` names.",
            schema::<ShellArgs>(),
        )
        .with_title("Run a command")
        .annotate(changes_things("Run a command", true, false)),
        Tool::new(
            "shell_output",
            "Read what a background command has written since the last call, and its exit code once it has \
             ended. `wait_ms` waits that long for new output or for the end.",
            schema::<ShellOutputArgs>(),
        )
        .with_title("Read a background command's output")
        .annotate(reads_only("Read a background command's output")),
        Tool::new(
            "shell_stop",
            "Stop a background command: TERM to it and what it started, then KILL ten seconds later if \
             anything is left. The answer has its exit code.",
            schema::<ShellStopArgs>(),
        )
        .with_title("Stop a background command")
        .annotate(changes_things("Stop a background command", true, true)),
        Tool::new(
            "read",
            "Read a text file with line numbers, from line `offset` (counted from 1), at most `limit` lines. \
             An image file is returned as an image.",
            schema::<ReadArgs>(),
        )
        .with_title("Read a file")
        .annotate(reads_only("Read a file")),
        Tool::new(
            "write",
            "Write a file with the given content, replacing it if it exists. Parent directories are created.",
            schema::<WriteArgs>(),
        )
        .with_title("Write a file")
        .annotate(changes_things("Write a file", true, true)),
        Tool::new(
            "edit",
            "Replace exact text in a file. `old` has to occur exactly once unless `all` is true.",
            schema::<EditArgs>(),
        )
        .with_title("Edit a file")
        .annotate(changes_things("Edit a file", true, false)),
        Tool::new(
            "list",
            "List a directory: entries with their kind and size, directories first. `depth` is 1 by default, \
             at most 4.",
            schema::<ListArgs>(),
        )
        .with_title("List a directory")
        .annotate(reads_only("List a directory")),
        Tool::new(
            "search",
            "Search the contents of files by regular expression. The lines that match are returned as \
             `path:line: text`, at most `max_results`. Hidden files, files that a .gitignore leaves out and \
             binary files are skipped unless the path names the file.",
            schema::<SearchArgs>(),
        )
        .with_title("Search files")
        .annotate(reads_only("Search files")),
    ]
}

/// Carries out the tool `name` of the set.
pub(crate) async fn call(
    name: &str,
    given: JsonObject,
    ctx: &Call<'_>,
) -> Result<ToolOutput, ToolFault> {
    match name {
        "shell" => shell(arguments(given)?, ctx).await,
        "shell_output" => shell_output(arguments(given)?, ctx).await,
        "shell_stop" => shell_stop(arguments(given)?, ctx).await,
        "read" => read(arguments(given)?, ctx).await,
        "write" => write(arguments(given)?, ctx).await,
        "edit" => edit(arguments(given)?, ctx).await,
        "list" => list(arguments(given)?, ctx).await,
        "search" => search_files(arguments(given)?, ctx).await,
        other => Err(ToolFault::NotFound(format!(
            "the set plain has no tool called {other}"
        ))),
    }
}

/// Where relative paths start from.
fn base(ctx: &Call<'_>) -> PathBuf {
    PathBuf::from(ctx.scope.start_directory())
}

async fn shell(args: ShellArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    ctx.access.permit_commands()?;
    let start = base(ctx);
    let cwd = match args.cwd.as_deref() {
        None => start,
        Some(given) => files::place(ctx.access, &start, given, Need::Read)?.real,
    };
    let timeout = match args.timeout_ms {
        None => None,
        Some(ms) if (1..=TIMEOUT_MAX_MS).contains(&ms) => {
            Some(Duration::from_millis(ms.unsigned_abs()))
        }
        Some(ms) => {
            return Err(ToolFault::InvalidInput(format!(
                "timeout_ms is from 1 to {TIMEOUT_MAX_MS}, and {ms} is not; put a longer command in the background"
            )));
        }
    };
    let launch = Launch {
        thread_id: &ctx.scope.thread_id,
        call_id: ctx.call_id,
        command: &args.command,
        cwd: &cwd,
        timeout,
        track_directory: false,
    };
    if args.background == Some(true) {
        let started = ctx.commands.start_background(&launch)?;
        return Ok(ToolOutput::Json(
            json!({ "process_id": started.process_id }),
        ));
    }
    let finished = ctx.commands.run(&launch, ctx.cancel).await?;
    let truncated = finished.stdout.omitted > 0 || finished.stderr.omitted > 0;
    let mut answer = json!({
        "exit_code": finished.exit.code,
        "stdout": finished.stdout.text,
        "stderr": finished.stderr.text,
        "truncated": truncated,
    });
    if truncated && let Some(file) = &finished.output_file {
        answer["output_file"] = Value::String(file.display().to_string());
    }
    if let Some(note) = how_it_ended(&finished.exit, timeout) {
        answer["note"] = Value::String(note);
    }
    Ok(ToolOutput::Json(answer))
}

/// A sentence about a command that did not end by itself.
fn how_it_ended(exit: &ExitInfo, timeout: Option<Duration>) -> Option<String> {
    match exit.reason {
        Reason::Exited => None,
        Reason::TimedOut => {
            let allowed = timeout.unwrap_or(crate::commands::DEFAULT_TIMEOUT);
            Some(format!(
                "The command was stopped when the {} ms it was allowed had passed.",
                allowed.as_millis()
            ))
        }
        Reason::Stopped => Some("The command was stopped.".to_owned()),
        Reason::Cancelled => Some("The call was cancelled and the command was stopped.".to_owned()),
        Reason::Shutdown => Some("The executor stopped and ended the command.".to_owned()),
    }
}

async fn shell_output(args: ShellOutputArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let longest = i64::try_from(MAX_OUTPUT_WAIT.as_millis()).unwrap_or(i64::MAX);
    let wait = match args.wait_ms {
        None => Duration::ZERO,
        Some(ms) if (0..=longest).contains(&ms) => Duration::from_millis(ms.unsigned_abs()),
        Some(ms) => {
            return Err(ToolFault::InvalidInput(format!(
                "wait_ms is from 0 to {longest}, and {ms} is not"
            )));
        }
    };
    let progress = ctx
        .commands
        .output(&ctx.scope.thread_id, &args.process_id, wait)
        .await?;
    let mut answer = json!({
        "output": progress.output.text,
        "running": progress.exit.is_none(),
    });
    if let Some(exit) = progress.exit {
        answer["exit_code"] = json!(exit.code);
    }
    if progress.output.omitted > 0 {
        answer["truncated"] = Value::Bool(true);
        answer["output_file"] = Value::String(progress.output_file.display().to_string());
    }
    Ok(ToolOutput::Json(answer))
}

async fn shell_stop(args: ShellStopArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let exit = ctx
        .commands
        .stop(&ctx.scope.thread_id, &args.process_id)
        .await?;
    Ok(ToolOutput::Json(json!({ "exit_code": exit.code })))
}

async fn read(args: ReadArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let (access, base) = (ctx.access.clone(), base(ctx));
    blocking(move || {
        files::read(
            &access,
            &base,
            &ReadRequest {
                path: &args.path,
                offset: args.offset,
                limit: args.limit,
                directory_advice: "Use the list tool to see what it holds.",
            },
        )
    })
    .await
}

async fn write(args: WriteArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let (access, base) = (ctx.access.clone(), base(ctx));
    let (_, written) =
        blocking(move || files::write(&access, &base, &args.path, &args.content)).await?;
    Ok(ToolOutput::Json(json!({ "bytes": written.bytes })))
}

async fn edit(args: EditArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let (access, base) = (ctx.access.clone(), base(ctx));
    let (_, replaced) = blocking(move || {
        files::edit(
            &access,
            &base,
            &args.path,
            &args.old,
            &args.new,
            args.all == Some(true),
        )
    })
    .await?;
    Ok(ToolOutput::Json(json!({ "replaced": replaced })))
}

async fn list(args: ListArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let (access, base) = (ctx.access.clone(), base(ctx));
    let listing = blocking(move || files::list(&access, &base, &args.path, args.depth)).await?;
    Ok(listing.into_output())
}

async fn search_files(args: SearchArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let (access, base) = (ctx.access.clone(), base(ctx));
    blocking(move || {
        let max_results = match args.max_results {
            None => DEFAULT_MAX_RESULTS,
            Some(n)
                if (1..=i64::try_from(MAX_RESULTS_CEILING).unwrap_or(i64::MAX)).contains(&n) =>
            {
                usize::try_from(n).unwrap_or(DEFAULT_MAX_RESULTS)
            }
            Some(n) => {
                return Err(ToolFault::InvalidInput(format!(
                    "max_results is from 1 to {MAX_RESULTS_CEILING}, and {n} is not"
                )));
            }
        };
        let found = files::place(
            &access,
            &base,
            args.path.as_deref().unwrap_or("."),
            Need::Read,
        )?;
        if !Path::new(&found.real).exists() {
            return Err(ToolFault::NotFound(format!(
                "{} does not exist",
                found.shown.display()
            )));
        }
        search::plain(&PlainSearch {
            pattern: &args.pattern,
            at: Where {
                real: &found.real,
                shown: &found.shown,
                base: &base,
            },
            glob: args.glob.as_deref(),
            max_results,
        })
    })
    .await
}
