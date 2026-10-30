//! What a build in the tool cache offers: `node.tool.check`.
//!
//! The system checks each build of an agent program once, when it is first kept and before any unit is given it:
//! this node runs the build so that it prints its interface, and answers which of the names the system asked about
//! it lacks. The build runs in a scratch directory of the cache that is its home and its working directory, with
//! nothing of the node's environment but `PATH`, for at most the time the system gives. Where the host lets a
//! process have a network of its own (`unshare --user --map-root-user --net`), it runs in one with no way out;
//! elsewhere every proxy is pointed at a closed port. It is asked only to print; what it printed is read within a
//! bound, and the scratch directory goes with the answer. It runs with the node's user and without a unit's walls:
//! the vendor's checksum, verified when the build entered the cache, is what admits it, as it admits it to units.
//!
//! Native behavior is checked separately through a driver's disposable networkless unit. That stage has no
//! host-process fallback and uses the shared native cases carried by the node's runner.
//!
//! **Where the names are looked for.** Help text ([`ToolInterface::Text`]): a name is there when it stands as a
//! whole word, so `--model` is not found in `--model-provider`. A JSON Schema bundle ([`ToolInterface::Schema`]):
//! `Type.member` is there when a definition `Type` has the property `member`, and any other name when a file holds
//! it as a string, which is how a protocol names its methods.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use orch_link_contract::commands::{NodeToolCheck, NodeToolCheckResult, ToolInterface};
use serde_json::Value as Json;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::tools::{ToolCache, ToolError};

/// Runs the second stage only through a driver that provides its networkless unit.
///
/// # Errors
/// The build is absent, or no loaded driver can run an isolated check.
pub async fn conformance(
    cache: &ToolCache,
    drivers: &crate::drivers::DriverHost,
    request: &orch_link_contract::commands::NodeToolConformance,
    cancel: &CancellationToken,
) -> Result<
    orch_link_contract::commands::NodeToolConformanceResult,
    orch_link_contract::commands::CommandError,
> {
    use orch_driver_contract::driver::DriverError;
    use orch_link_contract::commands::{CommandError, ErrorCode};
    if request.case_timeout_secs == 0 || request.timeout_secs == 0 {
        return Err(CommandError::new(
            ErrorCode::Unsupported,
            "native check deadlines must be positive",
        ));
    }
    if !cache.has(&request.name, &request.version) {
        return Err(CommandError::new(
            ErrorCode::NotFound,
            "the build is not in this node's tool cache",
        ));
    }
    let programs: Vec<_> = request.programs.iter().map(String::as_str).collect();
    let program = orch_connector_core::builds::find_program(
        &cache.dir().join(&request.name),
        &request.version,
        &programs,
    )
    .ok_or_else(|| {
        CommandError::new(
            ErrorCode::NotFound,
            "the cached build has no harness executable",
        )
    })?;
    let mut unavailable = Vec::new();
    for (name, driver) in drivers.all() {
        match driver
            .extras()
            .harness_conformance(request, &program, cancel)
            .await
        {
            Err(DriverError::Unsupported(why)) => unavailable.push(format!("{name}: {why}")),
            Err(error) => return Err(crate::drivers::failure(&error)),
            Ok(result) => return Ok(result),
        }
    }
    Err(CommandError::new(
        ErrorCode::Unsupported,
        format!(
            "no driver can run an isolated native check: {}",
            unavailable.join("; ")
        ),
    ))
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
const OUT: &str = "{out}";
/// How much of a program's standard output is read: help text is a few kilobytes.
const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;
/// How much of the files a program writes is read: Codex's protocol bundle is about two megabytes.
const MAX_SCHEMA_BYTES: u64 = 256 * 1024 * 1024;

/// Runs the build and says which of the names it lacks.
///
/// # Errors
/// The build is not in the cache, or has no such program; the program could not be run, ended badly, ran longer
/// than allowed or printed more than is read; the check was cancelled.
pub async fn check(
    cache: &ToolCache,
    request: &NodeToolCheck,
    cancel: &CancellationToken,
) -> Result<NodeToolCheckResult, ToolError> {
    if !cache.has(&request.name, &request.version) {
        return Err(ToolError::Invalid(format!(
            "{} {} is not in the tool cache of this host: node.tool.ensure comes first",
            request.name, request.version
        )));
    }
    let programs: Vec<&str> = request.programs.iter().map(String::as_str).collect();
    let program = orch_connector_core::builds::find_program(
        &cache.dir().join(&request.name),
        &request.version,
        &programs,
    )
    .ok_or_else(|| {
        ToolError::Shape(format!(
            "{} {} has none of the programs {}",
            request.name,
            request.version,
            request.programs.join(", ")
        ))
    })?;
    let scratch = tempfile::Builder::new()
        .prefix(".check-")
        .tempdir_in(cache.dir())
        .map_err(|source| ToolError::Io {
            what: "making the check's scratch directory".to_owned(),
            source,
        })?;
    let out = scratch.path().join("out");
    std::fs::create_dir(&out).map_err(|source| ToolError::Io {
        what: "making the check's output directory".to_owned(),
        source,
    })?;
    let printed = run(&program, request, scratch.path(), &out, cancel).await?;
    let missing = match request.printed {
        ToolInterface::Text => {
            let text = String::from_utf8_lossy(&printed);
            request
                .names
                .iter()
                .filter(|name| !stands_in(&text, name))
                .cloned()
                .collect()
        }
        ToolInterface::Schema => {
            let found = Schema::read(&out)?;
            request
                .names
                .iter()
                .filter(|name| !found.has(name))
                .cloned()
                .collect()
        }
    };
    Ok(NodeToolCheckResult { missing })
}

/// Runs the program as the check does and answers its standard output.
async fn run(
    program: &Path,
    request: &NodeToolCheck,
    home: &Path,
    out: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, ToolError> {
    let args = request.args.iter().map(|arg| {
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
    command
        .args(args)
        .current_dir(home)
        .env_clear()
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    for proxy in PROXIES {
        command.env(proxy, NO_NETWORK);
    }
    let mut child = command.spawn().map_err(|source| ToolError::Io {
        what: format!("starting {}", program.display()),
        source,
    })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::Shape("the program's output could not be read".to_owned()))?;
    let limit = Duration::from_secs(u64::from(request.timeout_secs.max(1)));
    let work = async {
        let mut printed = Vec::new();
        (&mut stdout)
            .take(MAX_TEXT_BYTES + 1)
            .read_to_end(&mut printed)
            .await
            .map_err(|source| ToolError::Io {
                what: "reading the program's output".to_owned(),
                source,
            })?;
        let status = child.wait().await.map_err(|source| ToolError::Io {
            what: "waiting for the program".to_owned(),
            source,
        })?;
        Ok::<_, ToolError>((status, printed))
    };
    let (status, printed) = tokio::select! {
        () = cancel.cancelled() => return Err(ToolError::Cancelled),
        answer = tokio::time::timeout(limit, work) => answer.map_err(|_| {
            ToolError::Shape(format!("the program did not finish within {} s", limit.as_secs()))
        })??,
    };
    if u64::try_from(printed.len()).unwrap_or(u64::MAX) > MAX_TEXT_BYTES {
        return Err(ToolError::Shape(format!(
            "the program printed more than {MAX_TEXT_BYTES} bytes"
        )));
    }
    if !status.success() {
        return Err(ToolError::Shape(format!(
            "the program ended with {status} when asked for its interface"
        )));
    }
    Ok(printed)
}

/// Whether this host lets a process have a network of its own, tried once: `unshare` is there and user namespaces
/// are allowed to an unprivileged user.
async fn isolation_works() -> bool {
    static WORKS: tokio::sync::OnceCell<bool> = tokio::sync::OnceCell::const_new();
    *WORKS
        .get_or_init(|| async {
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
    fn read(dir: &Path) -> Result<Self, ToolError> {
        let mut found = Self::default();
        let mut read = 0_u64;
        let mut pending = vec![dir.to_path_buf()];
        while let Some(next) = pending.pop() {
            let entries = std::fs::read_dir(&next).map_err(|source| ToolError::Io {
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
                    let bytes = std::fs::read(&path).map_err(|source| ToolError::Io {
                        what: format!("reading {}", path.display()),
                        source,
                    })?;
                    read += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                    if read > MAX_SCHEMA_BYTES {
                        return Err(ToolError::Shape(format!(
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
    fn asking(args: &[&str], timeout_secs: u32) -> NodeToolCheck {
        NodeToolCheck {
            name: "program".to_owned(),
            version: "1.0.0".to_owned(),
            programs: vec!["program".to_owned()],
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            printed: ToolInterface::Text,
            names: Vec::new(),
            timeout_secs,
        }
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
        let printed = run(
            &program,
            &asking(&[OUT], 10),
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
            matches!(failed, Err(ToolError::Shape(ref why)) if why.contains("ended")),
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
            matches!(stopped, Err(ToolError::Shape(ref why)) if why.contains("within")),
            "{stopped:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(20));
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
