//! Probe which harness sandbox modes the environment can enforce.
//!
//! Codex's sandbox is carried out by helper programs inside the vendor's own binary, and whether it can start is
//! a fact of the place: on Linux it needs unprivileged user namespaces and a permission to mount, which a
//! stock container does not give. So the executor does not claim the modes the harness declares; it asks the
//! binary to run a command under each profile, here, and reports the modes for which what it saw was what the
//! mode promises. The mode `open` applies no sandbox and always works.
//!
//! Starting is not enough: a command is run to prove the sandbox does what it says. Under `read_only` a write
//! has to fail. Under `workspace` a write in the working directory has to work and a write beside it has to
//! fail. A profile under which the command ran but the writes went wrong is not enforced, and is reported as
//! not working, which is the safe answer for an executor that must not claim a confinement it does not have.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use orch_agent_contract::descriptor::SandboxMode;
use tokio::process::Command;

/// The built-in profiles of the vendor's `codex sandbox -P`, by the mode they enforce.
const PROFILES: [(SandboxMode, &str); 2] = [
    (SandboxMode::ReadOnly, ":read-only"),
    (SandboxMode::Workspace, ":workspace"),
];

/// How much of what a failed probe printed is kept as the reason.
const REASON_CHARS: usize = 200;

/// What a probe is run with.
#[derive(Debug, Clone)]
pub struct Probe<'a> {
    /// The vendor's program.
    pub program: &'a Path,
    /// The environment's shell, which runs the commands of the probe.
    pub shell: &'a str,
    /// A directory that is the executor's own, in which the probe works.
    pub scratch: &'a Path,
    /// The environment the program is started with and nothing else.
    pub environment: &'a [(OsString, OsString)],
    /// How long one command of the probe may take.
    pub timeout: Duration,
}

/// What the probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enforceable {
    /// The modes that work, from the strictest to the widest; `open` is always among them.
    pub modes: Vec<SandboxMode>,
    /// The modes that do not, and why not.
    pub refused: Vec<(SandboxMode, String)>,
}

/// The sandbox modes the environment can enforce through the vendor's program.
pub async fn probe(probe: &Probe<'_>) -> Enforceable {
    let mut modes = Vec::new();
    let mut refused = Vec::new();
    if let Err(error) = std::fs::create_dir_all(probe.scratch) {
        let reason = format!("the probe has no directory to work in: {error}");
        refused.extend(PROFILES.iter().map(|(mode, _)| (*mode, reason.clone())));
    } else {
        for (mode, profile) in PROFILES {
            match enforce(probe, mode, profile).await {
                Ok(()) => modes.push(mode),
                Err(reason) => refused.push((mode, reason)),
            }
        }
    }
    modes.push(SandboxMode::Open);
    Enforceable { modes, refused }
}

/// Whether the profile does what its mode promises, or what it did instead.
async fn enforce(probe: &Probe<'_>, mode: SandboxMode, profile: &str) -> Result<(), String> {
    run(probe, profile, "exit 0")
        .await
        .map_err(|why| format!("a command could not be started under {profile}: {why}"))?;
    let inside = probe.scratch.join(".probe-inside");
    let beside = probe
        .scratch
        .parent()
        .unwrap_or(probe.scratch)
        .join(".orch-probe-beside");
    for file in [&inside, &beside] {
        let _ = std::fs::remove_file(file);
    }
    // Whether the writes worked is told by the files; the commands' own results do not matter.
    let _ = run(probe, profile, &format!(": > {}", quote(&inside))).await;
    let _ = run(probe, profile, &format!(": > {}", quote(&beside))).await;
    let wrote_inside = inside.exists();
    let wrote_beside = beside.exists();
    for file in [&inside, &beside] {
        let _ = std::fs::remove_file(file);
    }
    match mode {
        SandboxMode::ReadOnly if wrote_inside || wrote_beside => Err(format!(
            "a command wrote a file under {profile}, which is not a read-only sandbox here"
        )),
        SandboxMode::Workspace if !wrote_inside => Err(format!(
            "a command could not write in its working directory under {profile}"
        )),
        SandboxMode::Workspace if wrote_beside => Err(format!(
            "a command wrote beside its working directory under {profile}, which is not confined here"
        )),
        _ => Ok(()),
    }
}

/// Runs `script` with the shell under the profile; why not, if it did not end well.
async fn run(probe: &Probe<'_>, profile: &str, script: &str) -> Result<(), String> {
    let mut command = Command::new(probe.program);
    command
        .arg("sandbox")
        .args(["-P", profile, "-C"])
        .arg(probe.scratch)
        .arg("--")
        .arg(probe.shell)
        .args(["-c", script])
        .env_clear()
        .envs(probe.environment.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command
        .spawn()
        .map_err(|error| format!("{} did not start: {error}", probe.program.display()))?;
    let output = tokio::time::timeout(probe.timeout, child.wait_with_output())
        .await
        .map_err(|_| format!("no answer within {} seconds", probe.timeout.as_secs()))?
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let first = stderr.lines().find(|line| !line.trim().is_empty());
    Err(first.map_or_else(
        || format!("it ended with {}", output.status),
        |line| line.trim().chars().take(REASON_CHARS).collect(),
    ))
}

/// `path` as one word for a POSIX shell.
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

/// The directory in which the probe works, below the executor's own directory.
#[must_use]
pub fn scratch_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sandbox-probe")
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    /// A program that stands for `codex`: `sandbox -P PROFILE -C DIR -- COMMAND...` is run by `body`.
    fn fake(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("codex");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("mode");
        path
    }

    fn environment() -> Vec<(OsString, OsString)> {
        vec![(
            OsString::from("PATH"),
            OsString::from("/usr/local/bin:/usr/bin:/bin"),
        )]
    }

    async fn probed(program: &Path, scratch: &Path) -> Enforceable {
        let environment = environment();
        probe(&Probe {
            program,
            shell: "/bin/sh",
            scratch,
            environment: &environment,
            timeout: Duration::from_secs(10),
        })
        .await
    }

    // Drops `sandbox -P PROFILE -C`, goes to the directory, drops it and `--`, and runs the command: a
    // sandbox that confines nothing.
    const UNCONFINED: &str = r#"shift; shift; shift; shift; cd "$1"; shift; shift; exec "$@""#;

    #[tokio::test]
    async fn a_sandbox_that_cannot_start_leaves_only_open_and_says_why() {
        let dir = tempfile::tempdir().expect("a directory");
        let program = fake(
            dir.path(),
            "echo 'bwrap: No permissions to create a new namespace' >&2; exit 1",
        );
        let found = probed(&program, &dir.path().join("work/probe")).await;
        assert_eq!(found.modes, [SandboxMode::Open]);
        assert_eq!(found.refused.len(), 2);
        assert!(
            found.refused[0]
                .1
                .contains("No permissions to create a new namespace"),
            "{:?}",
            found.refused
        );
    }

    #[tokio::test]
    async fn a_sandbox_that_runs_the_command_but_confines_nothing_is_not_taken_for_one() {
        let dir = tempfile::tempdir().expect("a directory");
        let program = fake(dir.path(), UNCONFINED);
        let found = probed(&program, &dir.path().join("work/probe")).await;
        assert_eq!(found.modes, [SandboxMode::Open]);
        let reasons: Vec<&str> = found.refused.iter().map(|(_, why)| why.as_str()).collect();
        assert!(
            reasons[0].contains("wrote a file under :read-only"),
            "{reasons:?}"
        );
        assert!(
            reasons[1].contains("wrote beside its working directory"),
            "{reasons:?}"
        );
        assert!(
            !dir.path().join("work/.orch-probe-beside").exists()
                && !dir.path().join("work/probe/.probe-inside").exists(),
            "the probe leaves nothing behind"
        );
    }

    #[tokio::test]
    async fn a_program_that_is_not_there_leaves_only_open() {
        let dir = tempfile::tempdir().expect("a directory");
        let found = probed(
            &dir.path().join("no-such-program"),
            &dir.path().join("probe"),
        )
        .await;
        assert_eq!(found.modes, [SandboxMode::Open]);
        assert!(
            found.refused[0].1.contains("did not start"),
            "{:?}",
            found.refused
        );
    }

    #[tokio::test]
    async fn a_sandbox_that_does_not_answer_in_time_is_not_waited_for() {
        let dir = tempfile::tempdir().expect("a directory");
        let program = fake(dir.path(), "sleep 30");
        let environment = environment();
        let began = std::time::Instant::now();
        let found = probe(&Probe {
            program: &program,
            shell: "/bin/sh",
            scratch: &dir.path().join("probe"),
            environment: &environment,
            timeout: Duration::from_millis(300),
        })
        .await;
        assert_eq!(found.modes, [SandboxMode::Open]);
        assert!(began.elapsed() < Duration::from_secs(10));
        assert!(
            found.refused[0].1.contains("no answer"),
            "{:?}",
            found.refused
        );
    }

    #[test]
    fn a_path_with_a_quote_in_it_is_one_word_for_the_shell() {
        assert_eq!(quote(Path::new("/a b/it's")), r"'/a b/it'\''s'");
    }
}
