//! What an agent program offers its adapter: it is run so that it prints its interface, and the names the adapter needs
//! that it lacks are answered. One probe for a host's check of a catalogue build (`node.tool.check`) and for a connected
//! machine's own check of the program installed on it.
//!
//! The program runs in a scratch directory that is its home and its working directory, with nothing of the caller's
//! environment but `PATH` (and, on Windows, what the system needs to start a program), for at most the time it is
//! given. Where the machine lets a process have a network of its own (`unshare --user --map-root-user --net`), it runs
//! in one with no way out; elsewhere every proxy is pointed at a closed port. It is asked only to print; what it printed
//! is read within a bound, and the scratch directory goes with the answer.
//!
//! **Where the names are looked for.** Help text ([`ToolInterface::Text`]): a name is there when it stands as a whole
//! word, so `--model` is not found in `--model-provider`. A JSON Schema bundle ([`ToolInterface::Schema`]):
//! `Type.member` is there when a definition `Type` has the property `member`, and any other name when a file holds it
//! as a string, which is how a protocol names its methods.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use orch_link_contract::commands::ToolInterface;
use serde_json::Value as Json;
use tokio_util::sync::CancellationToken;

/// What a program is asked to print, and what is looked for in it.
#[derive(Debug, Clone, Copy)]
pub struct Probe<'a> {
    /// The arguments that make it print its interface; [`OUT`] stands for the directory it writes into.
    pub args: &'a [String],
    /// Where it prints it.
    pub printed: ToolInterface,
    /// The names the adapter needs.
    pub names: &'a [String],
    /// How long it may take.
    pub timeout: Duration,
}

/// Why a probe answered nothing.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// Something on the disk failed, or the program could not be started.
    #[error("{what}: {source}")]
    Io {
        /// What was being done.
        what: String,
        /// What the system said.
        source: std::io::Error,
    },
    /// The program ended badly, ran longer than allowed or printed more than is read.
    #[error("{0}")]
    Shape(String),
    /// The probe was asked to stop.
    #[error("the check was cancelled")]
    Cancelled,
}

/// The address every proxy variable is given: a closed port, so that a request the program tries fails at once.
const NO_NETWORK: &str = "http://127.0.0.1:9";
/// The proxy variables programs read, in both spellings.
const PROXIES: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];
/// The argument that stands for the directory a program writes its interface into.
pub const OUT: &str = "{out}";
/// How much of a program's standard output is read: help text is a few kilobytes.
const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;
/// The file in the scratch home that takes a program's standard output. A file and not a pipe: a program that ends
/// before its writes to a pipe are flushed (a Node.js program does, Claude Code among them) loses what was still
/// waiting, and its help would seem to lack every name after its first write.
const PRINTED: &str = ".printed";
/// How often the output file of a running program is measured: a program that prints without end is stopped soon
/// after [`MAX_TEXT_BYTES`] rather than filling the disk until its deadline.
const MEASURE_EVERY: Duration = Duration::from_millis(100);
/// How much of the files a program writes is read: Codex's protocol bundle is about two megabytes.
const MAX_SCHEMA_BYTES: u64 = 256 * 1024 * 1024;

/// Runs `program` as `probe` asks, in a scratch directory made in `scratch_in`, and answers the names it lacks.
///
/// # Errors
/// The scratch directory could not be made; the program could not be run, ended badly, ran longer than allowed or
/// printed more than is read; the probe was cancelled.
pub async fn missing(
    program: &Path,
    probe: &Probe<'_>,
    scratch_in: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<String>, ProbeError> {
    let scratch = tempfile::Builder::new()
        .prefix(".check-")
        .tempdir_in(scratch_in)
        .map_err(|source| ProbeError::Io {
            what: "making the check's scratch directory".to_owned(),
            source,
        })?;
    let out = scratch.path().join("out");
    std::fs::create_dir(&out).map_err(|source| ProbeError::Io {
        what: "making the check's output directory".to_owned(),
        source,
    })?;
    let printed = run(program, probe, scratch.path(), &out, cancel).await?;
    Ok(match probe.printed {
        ToolInterface::Text => {
            let text = String::from_utf8_lossy(&printed);
            probe
                .names
                .iter()
                .filter(|name| !stands_in(&text, name))
                .cloned()
                .collect()
        }
        ToolInterface::Schema => {
            let found = Schema::read(&out)?;
            probe
                .names
                .iter()
                .filter(|name| !found.has(name))
                .cloned()
                .collect()
        }
    })
}

/// Runs the program as the check does and answers its standard output.
async fn run(
    program: &Path,
    probe: &Probe<'_>,
    home: &Path,
    out: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, ProbeError> {
    let args = probe.args.iter().map(|arg| {
        if arg == OUT {
            out.display().to_string()
        } else {
            arg.clone()
        }
    });
    let mut command = if isolation_works().await {
        let mut wrapped = tokio::process::Command::new("unshare");
        wrapped
            .args(["--user", "--map-root-user", "--net", "--"])
            .arg(program);
        wrapped
    } else {
        tokio::process::Command::new(program)
    };
    let printed_path = home.join(PRINTED);
    let printed = std::fs::File::create(&printed_path).map_err(|source| ProbeError::Io {
        what: "making the file the program's output goes to".to_owned(),
        source,
    })?;
    command
        .args(args)
        .current_dir(home)
        .env_clear()
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::from(printed))
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    // Windows starts no program without these; they come from the caller, never from an environment's secrets.
    #[cfg(windows)]
    for name in ["SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    for proxy in PROXIES {
        command.env(proxy, NO_NETWORK);
    }
    let mut child = command.spawn().map_err(|source| ProbeError::Io {
        what: format!("starting {}", program.display()),
        source,
    })?;
    let limit = probe.timeout.max(Duration::from_secs(1));
    let too_much = || {
        ProbeError::Shape(format!(
            "the program printed more than {MAX_TEXT_BYTES} bytes"
        ))
    };
    let work = async {
        let mut measure = tokio::time::interval(MEASURE_EVERY);
        loop {
            tokio::select! {
                status = child.wait() => {
                    return status.map_err(|source| ProbeError::Io {
                        what: "waiting for the program".to_owned(),
                        source,
                    });
                }
                _ = measure.tick() => {
                    if printed_len(&printed_path) > MAX_TEXT_BYTES {
                        return Err(too_much());
                    }
                }
            }
        }
    };
    let status = tokio::select! {
        () = cancel.cancelled() => return Err(ProbeError::Cancelled),
        answer = tokio::time::timeout(limit, work) => answer.map_err(|_| {
            ProbeError::Shape(format!("the program did not finish within {} s", limit.as_secs()))
        })??,
    };
    if printed_len(&printed_path) > MAX_TEXT_BYTES {
        return Err(too_much());
    }
    if !status.success() {
        return Err(ProbeError::Shape(format!(
            "the program ended with {status} when asked for its interface"
        )));
    }
    tokio::fs::read(&printed_path)
        .await
        .map_err(|source| ProbeError::Io {
            what: "reading the program's output".to_owned(),
            source,
        })
}

/// How long the output file is now; nothing yet when it cannot be measured.
fn printed_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

/// Whether this machine lets a process have a network of its own, tried once: `unshare` is there and user namespaces
/// are allowed to an unprivileged user.
async fn isolation_works() -> bool {
    static WORKS: tokio::sync::OnceCell<bool> = tokio::sync::OnceCell::const_new();
    *WORKS
        .get_or_init(|| async {
            if !cfg!(target_os = "linux") {
                return false;
            }
            let tried = tokio::process::Command::new("unshare")
                .args(["--user", "--map-root-user", "--net", "--", "true"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status();
            matches!(
                tokio::time::timeout(Duration::from_secs(5), tried).await,
                Ok(Ok(status)) if status.success()
            )
        })
        .await
}

/// Whether `name` stands in `text` as a whole word: what is before and after it is no letter, digit, `_` or `-`.
fn stands_in(text: &str, name: &str) -> bool {
    let part = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if name.is_empty() {
        return false;
    }
    text.match_indices(name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        !before.is_some_and(part) && !after.is_some_and(part)
    })
}

/// What a JSON Schema bundle holds: every string in it, and the properties of every definition by its name.
#[derive(Debug, Default)]
struct Schema {
    strings: BTreeSet<String>,
    properties: BTreeMap<String, BTreeSet<String>>,
}

impl Schema {
    /// Reads every `.json` file below `dir`.
    fn read(dir: &Path) -> Result<Self, ProbeError> {
        let mut found = Self::default();
        let mut read = 0_u64;
        let mut pending = vec![dir.to_path_buf()];
        while let Some(next) = pending.pop() {
            let entries = std::fs::read_dir(&next).map_err(|source| ProbeError::Io {
                what: format!("reading {}", next.display()),
                source,
            })?;
            for entry in entries.flatten() {
                let path: PathBuf = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    pending.push(path);
                } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "json") {
                    let bytes = std::fs::read(&path).map_err(|source| ProbeError::Io {
                        what: format!("reading {}", path.display()),
                        source,
                    })?;
                    read += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                    if read > MAX_SCHEMA_BYTES {
                        return Err(ProbeError::Shape(format!(
                            "the program wrote more than {MAX_SCHEMA_BYTES} bytes of schema"
                        )));
                    }
                    if let Ok(value) = serde_json::from_slice::<Json>(&bytes) {
                        found.take(&value);
                    }
                }
            }
        }
        Ok(found)
    }

    fn take(&mut self, value: &Json) {
        match value {
            Json::String(text) => {
                self.strings.insert(text.clone());
            }
            Json::Array(items) => items.iter().for_each(|item| self.take(item)),
            Json::Object(members) => {
                if let (Some(Json::String(title)), Some(Json::Object(properties))) =
                    (members.get("title"), members.get("properties"))
                {
                    self.properties
                        .entry(title.clone())
                        .or_default()
                        .extend(properties.keys().cloned());
                }
                for (key, member) in members {
                    self.strings.insert(key.clone());
                    if let Some(Json::Object(properties)) = member.get("properties") {
                        self.properties
                            .entry(key.clone())
                            .or_default()
                            .extend(properties.keys().cloned());
                    }
                    self.take(member);
                }
            }
            _ => {}
        }
    }

    fn has(&self, name: &str) -> bool {
        match name.split_once('.') {
            Some((definition, member)) if !name.contains('/') => self
                .properties
                .get(definition)
                .is_some_and(|properties| properties.contains(member)),
            _ => self.strings.contains(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_flag_is_found_only_as_a_whole_word() {
        let help = "  --model <model>   the model\n  --model-provider <id>\n  --resume, -r\n";
        assert!(stands_in(help, "--model"));
        assert!(stands_in(help, "--resume"));
        assert!(!stands_in(help, "--model-prov"));
        assert!(!stands_in(help, "--fork-session"));
        assert!(!stands_in("--allowedTools2", "--allowedTools"));
    }

    #[cfg(unix)]
    fn script(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let program = dir.join("program");
        std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        program
    }

    #[cfg(unix)]
    fn asking(args: &[String], timeout_secs: u64) -> Probe<'_> {
        Probe {
            args,
            printed: ToolInterface::Text,
            names: &[],
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    #[cfg(unix)]
    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_program_runs_in_its_scratch_home_and_what_it_prints_and_writes_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(
            dir.path(),
            r#"echo "--model --resume HOME=$HOME PROXY=$HTTPS_PROXY"
printf '{"title":"Params","properties":{"member":{}}}' > "$1/params.json""#,
        );
        let home = dir.path().join("home");
        let out = home.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let args = words(&[OUT]);
        let printed = run(
            &program,
            &asking(&args, 10),
            &home,
            &out,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let text = String::from_utf8(printed).unwrap();
        assert!(
            stands_in(&text, "--model") && stands_in(&text, "--resume"),
            "{text}"
        );
        assert!(text.contains(&format!("HOME={}", home.display())), "{text}");
        assert!(text.contains(&format!("PROXY={NO_NETWORK}")), "{text}");
        assert!(Schema::read(&out).unwrap().has("Params.member"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_program_that_fails_or_runs_too_long_says_so_and_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let failing = script(dir.path(), "exit 3");
        let failed = run(
            &failing,
            &asking(&[], 10),
            &home,
            &home,
            &CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(failed, Err(ProbeError::Shape(ref why)) if why.contains("ended")),
            "{failed:?}"
        );
        let slow = script(dir.path(), "sleep 30");
        let started = std::time::Instant::now();
        let stopped = run(
            &slow,
            &asking(&[], 1),
            &home,
            &home,
            &CancellationToken::new(),
        )
        .await;
        assert!(
            matches!(stopped, Err(ProbeError::Shape(ref why)) if why.contains("within")),
            "{stopped:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_whole_help_of_a_program_that_exits_before_its_piped_output_is_written_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        // Like Claude Code, a Node.js program: it ends before its writes to a pipe are flushed, so through a pipe only
        // the first 8,192 bytes of its help arrive, and into a file all of it does.
        let program = script(
            dir.path(),
            r#"help() {
  i=0
  while [ "$i" -lt 300 ]; do
    echo "  --padding-$i  an option that makes the help longer than one write to a pipe"
    i=$((i + 1))
  done
  echo "  --late-flag  the option after the first 8,192 bytes"
}
if [ -p /dev/stdout ]; then help | head -c 8192; else help; fi"#,
        );
        let args = words(&["--help"]);
        let printed = run(
            &program,
            &asking(&args, 10),
            &home,
            &home,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let text = String::from_utf8(printed).unwrap();
        assert!(stands_in(&text, "--padding-0"), "{text}");
        assert!(
            stands_in(&text, "--late-flag"),
            "only {} bytes of the help were read",
            text.len()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_names_a_program_lacks_are_answered_from_its_scratch_directory() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), r#"echo "  --model  --resume""#);
        let args = words(&["--help"]);
        let names = words(&["--model", "--fork-session"]);
        let probe = Probe {
            args: &args,
            printed: ToolInterface::Text,
            names: &names,
            timeout: Duration::from_secs(10),
        };
        let lacking = missing(&program, &probe, dir.path(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(lacking, ["--fork-session"]);
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".check-"))
            .collect();
        assert!(
            left.is_empty(),
            "the scratch directory goes with the answer"
        );
    }

    #[test]
    fn a_method_is_a_string_of_the_bundle_and_a_member_a_property_of_its_definition() {
        let mut schema = Schema::default();
        schema.take(&json!({
            "definitions": {
                "TurnStartParams": { "type": "object", "properties": { "threadId": {}, "collaborationMode": {} } },
                "ClientRequest": { "oneOf": [{ "properties": { "method": { "enum": ["turn/start"] } } }] }
            }
        }));
        schema.take(&json!({ "title": "ThreadStartParams", "properties": { "cwd": {} } }));
        assert!(schema.has("turn/start"));
        assert!(!schema.has("turn/steer"));
        assert!(schema.has("TurnStartParams.collaborationMode"));
        assert!(!schema.has("TurnStartParams.environments"));
        assert!(schema.has("ThreadStartParams.cwd"));
        assert!(!schema.has("ThreadStartParams.selectedCapabilityRoots"));
    }
}
