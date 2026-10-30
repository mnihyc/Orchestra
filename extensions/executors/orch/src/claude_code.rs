//! The set with the names and inputs of Claude Code's own shell and file tools, carried out here.
//!
//! `Bash`, `Read`, `Write`, `Edit`, `Glob`, `Grep` and `TaskStop`, with the inputs the harness's own tools
//! have, so that a model taught on them finds what it expects. They are a likeness and not the harness's own
//! tools: those run inside the harness's process and act on its machine. A member that is not in the tool's
//! input is refused as unknown. What is carried out here differs from the harness's own tools where an
//! environment cannot do what they do: no PDF pages and no notebooks in `Read`, no command outside a
//! sandbox by request, and no searching the web.
//!
//! The session has a current directory. Relative paths start from it, and a `cd` in a command moves it for
//! the session's next call as long as the shell is left in the thread's directory or an attached mount; where
//! it is left anywhere else, the directory is set back and the answer says so. Variables are not kept.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rmcp::model::{JsonObject, Tool};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::commands::{DEFAULT_TIMEOUT, ExitInfo, Launch, Reason};
use crate::files::{self, ReadRequest};
use crate::sandbox::Need;
use crate::search::{self, GREP_DEFAULT_HEAD_LIMIT, GlobSearch, GrepMode, GrepSearch, Where};
use crate::tools::{
    Call, ToolFault, ToolOutput, arguments, blocking, changes_things, reads_only, schema,
};

/// The most a foreground command may be asked to run: ten minutes, which is the bound of the harness's own
/// shell tool, so that a model's sense of how long a call may take holds.
pub const TIMEOUT_MAX_MS: i64 = 600_000;

/// What `Bash` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BashArgs {
    /// The command to execute.
    command: String,
    /// Optional timeout in milliseconds: 120000 when left out, at most 600000. With `run_in_background` it
    /// ends the command then.
    timeout: Option<i64>,
    /// A clear, concise description of what the command does, in active voice.
    description: Option<String>,
    /// Set to true to run the command in the background: the call answers at once with an id for the command
    /// and the path of the file its output is written to.
    run_in_background: Option<bool>,
}

/// What `Read` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    /// The path of the file to read: absolute, or relative to the current directory.
    file_path: String,
    /// The line number to start reading from, counted from 1. Only provide it if the file is too large to
    /// read at once.
    offset: Option<i64>,
    /// The number of lines to read. Only provide it if the file is too large to read at once.
    limit: Option<i64>,
}

/// What `Write` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    /// The path of the file to write: absolute, or relative to the current directory.
    file_path: String,
    /// The content to write to the file.
    content: String,
}

/// What `Edit` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    /// The path of the file to modify: absolute, or relative to the current directory.
    file_path: String,
    /// The text to replace.
    old_string: String,
    /// The text to replace it with (must be different from old_string).
    new_string: String,
    /// Replace all occurrences of old_string (default false).
    replace_all: Option<bool>,
}

/// What `Glob` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GlobArgs {
    /// The glob pattern to match files against, such as `**/*.rs`.
    pattern: String,
    /// The directory to search in. If not specified, the current directory is used. Omit this field to use
    /// the default; do not enter "undefined" or "null".
    path: Option<String>,
}

/// What `Grep` answers with.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum OutputMode {
    /// The matching lines.
    Content,
    /// The paths of the files that have a match.
    FilesWithMatches,
    /// The number of matching lines in each file.
    Count,
}

/// What `Grep` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GrepArgs {
    /// The regular expression pattern to search for in file contents.
    pattern: String,
    /// File or directory to search in. Defaults to the current directory.
    path: Option<String>,
    /// Glob pattern to filter files (e.g. "*.js", "*.{ts,tsx}").
    glob: Option<String>,
    /// File type to search (e.g. js, py, rust, go, java).
    #[serde(rename = "type")]
    file_type: Option<String>,
    /// Output mode: "content" shows matching lines (supports -A/-B/-C context, -n line numbers, head_limit),
    /// "files_with_matches" shows file paths (supports head_limit), "count" shows match counts (supports
    /// head_limit). Defaults to "files_with_matches".
    output_mode: Option<OutputMode>,
    /// Case insensitive search.
    #[serde(rename = "-i")]
    ignore_case: Option<bool>,
    /// Show line numbers in output. Requires output_mode "content". Defaults to true.
    #[serde(rename = "-n")]
    line_numbers: Option<bool>,
    /// Number of lines to show after each match. Requires output_mode "content".
    #[serde(rename = "-A")]
    after: Option<i64>,
    /// Number of lines to show before each match. Requires output_mode "content".
    #[serde(rename = "-B")]
    before: Option<i64>,
    /// Number of lines to show before and after each match. Requires output_mode "content".
    #[serde(rename = "-C")]
    around: Option<i64>,
    /// Limit output to the first N lines or entries. Defaults to 250 when unspecified; 0 is unlimited.
    head_limit: Option<i64>,
    /// Skip the first N lines or entries before applying head_limit. Defaults to 0.
    offset: Option<i64>,
    /// Enable multiline mode where . matches newlines and patterns can span lines. Default: false.
    multiline: Option<bool>,
}

/// What `TaskStop` takes.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaskStopArgs {
    /// The ID of the background command to stop, as `Bash` gave it.
    task_id: String,
}

/// The seven tools.
pub(crate) fn definitions() -> Vec<Tool> {
    vec![
        Tool::new(
            "Bash",
            "Runs a shell command in the environment and returns its output. The working directory carries \
             over to the next call while it stays inside the thread's directory or an attached mount; \
             environment variables do not carry over. A foreground command may run for 120000 ms unless \
             `timeout` says otherwise (at most 600000). With `run_in_background` the call answers at once with \
             an id and the path of the file the output is written to, which Read reads; TaskStop stops the \
             command. Long output is cut in the middle; the whole of it is in the file the answer names.",
            schema::<BashArgs>(),
        )
        .with_title("Run a command")
        .annotate(changes_things("Run a command", true, false)),
        Tool::new(
            "Read",
            "Reads a file from the environment. The file_path is absolute, or relative to the current \
             directory. By default it reads up to 2000 lines from the start of the file; offset (the line to \
             start at, counted from 1) and limit (how many lines) read a part. Lines come with their numbers. \
             An image file is returned as an image. A directory cannot be read: list it with a command such \
             as ls.",
            schema::<ReadArgs>(),
        )
        .with_title("Read a file")
        .annotate(reads_only("Read a file")),
        Tool::new(
            "Write",
            "Writes a file, creating it or replacing all of it, and creating parent directories. Prefer Edit \
             for a change to an existing file.",
            schema::<WriteArgs>(),
        )
        .with_title("Write a file")
        .annotate(changes_things("Write a file", true, true)),
        Tool::new(
            "Edit",
            "Performs an exact string replacement in a file. old_string has to occur exactly once in the \
             file unless replace_all is true; give enough of the text around it to make it unique.",
            schema::<EditArgs>(),
        )
        .with_title("Edit a file")
        .annotate(changes_things("Edit a file", true, false)),
        Tool::new(
            "Glob",
            "Fast file pattern matching. Finds files by a glob pattern such as \"**/*.js\" or \
             \"src/**/*.ts\" and returns their paths, newest first, at most 100, and says whether there are \
             more. Hidden files and files that a .gitignore leaves out are skipped.",
            schema::<GlobArgs>(),
        )
        .with_title("Find files by name")
        .annotate(reads_only("Find files by name")),
        Tool::new(
            "Grep",
            "A search of file contents by regular expression, in the syntax of the Rust regex crate. \
             output_mode \"files_with_matches\" (the default) lists the files, \"content\" shows the \
             matching lines, \"count\" gives the number of matching lines in each file. Hidden files, \
             files that a .gitignore leaves out and binary files are skipped unless the path names the file.",
            schema::<GrepArgs>(),
        )
        .with_title("Search file contents")
        .annotate(reads_only("Search file contents")),
        Tool::new(
            "TaskStop",
            "Stops a background command by the id Bash gave: TERM to it and what it started, then KILL ten \
             seconds later if anything is left.",
            schema::<TaskStopArgs>(),
        )
        .with_title("Stop a background command")
        .annotate(changes_things("Stop a background command", true, true)),
    ]
}

/// Carries out the tool `name` of the set.
pub(crate) async fn call(
    name: &str,
    given: JsonObject,
    ctx: &Call<'_>,
) -> Result<ToolOutput, ToolFault> {
    match name {
        "Bash" => bash(arguments(given)?, ctx).await,
        "Read" => read(arguments(given)?, ctx).await,
        "Write" => write(arguments(given)?, ctx).await,
        "Edit" => edit(arguments(given)?, ctx).await,
        "Glob" => glob(arguments(given)?, ctx).await,
        "Grep" => grep(arguments(given)?, ctx).await,
        "TaskStop" => task_stop(arguments(given)?, ctx).await,
        other => Err(ToolFault::NotFound(format!(
            "the set claude-code has no tool called {other}"
        ))),
    }
}

/// The session's current directory, where relative paths start from.
fn current(ctx: &Call<'_>) -> PathBuf {
    ctx.commands
        .directory(&ctx.scope.thread_id, ctx.scope.start_directory())
}

async fn bash(args: BashArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    // The description is the model's note for whoever approves the command; it changes nothing here.
    tracing::debug!(thread = %ctx.scope.thread_id, description = args.description.as_deref().unwrap_or(""), "a command is run");
    ctx.access.permit_commands()?;
    let directory = current(ctx);
    let timeout = match args.timeout {
        None => None,
        Some(ms) if (1..=TIMEOUT_MAX_MS).contains(&ms) => {
            Some(Duration::from_millis(ms.unsigned_abs()))
        }
        Some(ms) => {
            return Err(ToolFault::InvalidInput(format!(
                "timeout is from 1 to {TIMEOUT_MAX_MS} milliseconds, and {ms} is not; run a longer command with run_in_background"
            )));
        }
    };
    let cwd = ctx.access.permit(&directory, Need::Read)?;
    let background = args.run_in_background == Some(true);
    let launch = Launch {
        thread_id: &ctx.scope.thread_id,
        call_id: ctx.call_id,
        command: &args.command,
        cwd: &cwd,
        timeout,
        track_directory: !background,
    };
    if background {
        let started = ctx.commands.start_background(&launch)?;
        return Ok(ToolOutput::Text(format!(
            "Command running in the background with ID: {}. Its output is written to {}; read it with the Read tool, and stop the command with TaskStop.",
            started.process_id,
            started.output_file.display()
        )));
    }
    let finished = ctx.commands.run(&launch, ctx.cancel).await?;
    let mut text = String::new();
    if finished.exit.code != 0 {
        text.push_str(&format!("Exit code {}\n", finished.exit.code));
    }
    text.push_str(&finished.combined.text);
    if !text.ends_with('\n') && !text.is_empty() {
        text.push('\n');
    }
    if finished.combined.omitted > 0
        && let Some(file) = &finished.output_file
    {
        text.push_str(&format!(
            "[Output truncated: {} bytes were left out of the middle. The whole output is in {}; read it with the Read tool.]\n",
            finished.combined.omitted,
            file.display()
        ));
    }
    if let Some(note) = how_it_ended(&finished.exit, timeout) {
        text.push_str(&note);
        text.push('\n');
    }
    if let Some(left_in) = finished.final_directory {
        if ctx.access.in_reach_of_the_thread(&left_in) {
            ctx.commands.set_directory(&ctx.scope.thread_id, left_in);
        } else {
            text.push_str(&format!("Shell cwd was reset to {}\n", directory.display()));
        }
    }
    if text.is_empty() {
        text.push_str("(no output)\n");
    }
    Ok(ToolOutput::Text(text))
}

/// A sentence about a command that did not end by itself.
fn how_it_ended(exit: &ExitInfo, timeout: Option<Duration>) -> Option<String> {
    match exit.reason {
        Reason::Exited => None,
        Reason::TimedOut => Some(format!(
            "Command timed out after {} ms and was stopped.",
            timeout.unwrap_or(DEFAULT_TIMEOUT).as_millis()
        )),
        Reason::Stopped => Some("The command was stopped.".to_owned()),
        Reason::Cancelled => Some("The call was cancelled and the command was stopped.".to_owned()),
        Reason::Shutdown => Some("The executor stopped and ended the command.".to_owned()),
    }
}

async fn read(args: ReadArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let (access, base) = (ctx.access.clone(), current(ctx));
    blocking(move || {
        files::read(
            &access,
            &base,
            &ReadRequest {
                path: &args.file_path,
                offset: args.offset,
                limit: args.limit,
                directory_advice: "List it with a command, such as ls.",
            },
        )
    })
    .await
}

async fn write(args: WriteArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let (access, base) = (ctx.access.clone(), current(ctx));
    let (placed, written) =
        blocking(move || files::write(&access, &base, &args.file_path, &args.content)).await?;
    Ok(ToolOutput::Text(if written.created {
        format!("File created successfully at: {}", placed.shown.display())
    } else {
        format!("The file {} has been updated.", placed.shown.display())
    }))
}

async fn edit(args: EditArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let (access, base) = (ctx.access.clone(), current(ctx));
    let all = args.replace_all == Some(true);
    let (placed, replaced) = blocking(move || {
        files::edit(
            &access,
            &base,
            &args.file_path,
            &args.old_string,
            &args.new_string,
            all,
        )
    })
    .await?;
    Ok(ToolOutput::Text(if all && replaced > 1 {
        format!(
            "The file {} has been updated. All {replaced} occurrences were replaced.",
            placed.shown.display()
        )
    } else {
        format!("The file {} has been updated.", placed.shown.display())
    }))
}

/// The place a search starts at, or why not: the path given (or the current directory), judged for reading
/// and known to exist.
fn search_root(
    access: &crate::sandbox::Access,
    base: &Path,
    path: Option<&str>,
) -> Result<files::Place, ToolFault> {
    let found = files::place(access, base, path.unwrap_or("."), Need::Read)?;
    if !found.real.exists() {
        return Err(ToolFault::NotFound(format!(
            "{} does not exist",
            found.shown.display()
        )));
    }
    Ok(found)
}

async fn glob(args: GlobArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let (access, base) = (ctx.access.clone(), current(ctx));
    blocking(move || {
        let found = search_root(&access, &base, args.path.as_deref())?;
        if !found.real.is_dir() {
            return Err(ToolFault::InvalidInput(format!(
                "{} is not a directory; the path of Glob is the directory to search in",
                found.shown.display()
            )));
        }
        search::glob(&GlobSearch {
            pattern: &args.pattern,
            at: Where {
                real: &found.real,
                shown: &found.shown,
                base: &base,
            },
        })
    })
    .await
}

/// A count the call gave, or why not.
fn count(name: &str, given: Option<i64>, default: usize) -> Result<usize, ToolFault> {
    match given {
        None => Ok(default),
        Some(n) if n >= 0 => Ok(usize::try_from(n).unwrap_or(default)),
        Some(n) => Err(ToolFault::InvalidInput(format!(
            "{name} is a number of lines or entries, not less than 0, and {n} is not"
        ))),
    }
}

async fn grep(args: GrepArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let (access, base) = (ctx.access.clone(), current(ctx));
    blocking(move || {
        let mode = match args.output_mode.unwrap_or(OutputMode::FilesWithMatches) {
            OutputMode::Content => GrepMode::Content,
            OutputMode::FilesWithMatches => GrepMode::FilesWithMatches,
            OutputMode::Count => GrepMode::Count,
        };
        let around = count("-C", args.around, 0)?;
        let after = count("-A", args.after, around)?;
        let before = count("-B", args.before, around)?;
        let head_limit = count("head_limit", args.head_limit, GREP_DEFAULT_HEAD_LIMIT)?;
        let offset = count("offset", args.offset, 0)?;
        let found = search_root(&access, &base, args.path.as_deref())?;
        search::grep(&GrepSearch {
            pattern: &args.pattern,
            at: Where {
                real: &found.real,
                shown: &found.shown,
                base: &base,
            },
            glob: args.glob.as_deref(),
            file_type: args.file_type.as_deref(),
            mode,
            ignore_case: args.ignore_case == Some(true),
            line_numbers: args.line_numbers != Some(false),
            before,
            after,
            head_limit,
            offset,
            multiline: args.multiline == Some(true),
        })
    })
    .await
}

async fn task_stop(args: TaskStopArgs, ctx: &Call<'_>) -> Result<ToolOutput, ToolFault> {
    let exit = ctx
        .commands
        .stop(&ctx.scope.thread_id, &args.task_id)
        .await?;
    Ok(ToolOutput::Text(format!(
        "Stopped the background command {} (exit code {}).",
        args.task_id, exit.code
    )))
}
