//! Whether the harness's own shell sandbox can confine commands where the harness runs.
//!
//! Claude Code confines its shell's commands with the operating system's sandbox: Seatbelt on macOS, bubblewrap
//! on Linux and WSL2 with socat relaying the network to its proxy, and nothing on Windows, where it runs commands
//! unconfined. Whether that sandbox starts is a fact of the place, not of the build: on Linux both programs have
//! to be installed, and bubblewrap has to be allowed to make the namespaces it works in, which a stock container
//! does not allow (its system-call filter refuses a new user namespace, its access profile refuses the mounts).
//! So, as the Codex executor proves its profiles before it reports them, the adapter declares Workspace only
//! where bubblewrap ran a command here the way the harness's sandbox runs one. Where only a fresh `/proc` cannot
//! be mounted (a container that is its own boundary and keeps the runtime's `/proc`), the harness's weaker nested
//! sandbox still holds writes to the workspace and the network to its proxy; Workspace is declared, and the
//! session is told to use that sandbox. Everywhere else the reason goes into the descriptor, so that a person reads
//! it before choosing Open, and no turn starts with a sandbox that cannot start.

use std::collections::BTreeMap;
use std::time::Duration;

use orch_agent_contract::descriptor::SandboxMode;

use crate::process::{LaunchSpec, Launcher, RunError, run_to_end};

/// How long one command of the probe may take. A sandbox that starts does so in milliseconds.
const PROBE_COMMAND: Duration = Duration::from_secs(10);

/// How much of what a failed start printed is kept in the reason.
const REASON_CHARS: usize = 200;

/// The program that applies Seatbelt profiles on macOS.
const SEATBELT: &str = "/usr/bin/sandbox-exec";

/// A command started as the harness's sandbox starts one: a session of its own, the network, process and mount
/// namespaces of its own, the file system read-only, and a fresh `/proc`.
const FULL: [&str; 15] = [
    "--new-session",
    "--die-with-parent",
    "--unshare-net",
    "--ro-bind",
    "/",
    "/",
    "--dev",
    "/dev",
    "--unshare-pid",
    "--proc",
    "/proc",
    "--",
    "/bin/sh",
    "-c",
    ":",
];

/// The same command as the weaker nested sandbox starts it: the `/proc` of the place is kept.
const WEAKER: [&str; 12] = [
    "--new-session",
    "--die-with-parent",
    "--unshare-net",
    "--ro-bind",
    "/",
    "/",
    "--dev",
    "/dev",
    "--",
    "/bin/sh",
    "-c",
    ":",
];

/// What the harness's sandbox does where the harness runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxSupport {
    /// Whether it confines commands to the workspace here: what Workspace asks of it.
    pub workspace: bool,
    /// Whether it does so only as the harness's weaker nested sandbox, which keeps the place's `/proc`.
    pub weaker_nested: bool,
    /// Why it does not, in a sentence a person reads before choosing Open; none where it does.
    pub reason: Option<String>,
}

impl SandboxSupport {
    /// What the build supports where nothing was looked at: the programs of a launcher that does not run them here.
    #[must_use]
    pub const fn declared() -> Self {
        Self {
            workspace: true,
            weaker_nested: false,
            reason: None,
        }
    }

    fn confines(weaker_nested: bool) -> Self {
        Self {
            workspace: true,
            weaker_nested,
            reason: None,
        }
    }

    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            workspace: false,
            weaker_nested: false,
            reason: Some(reason.into()),
        }
    }

    /// The sandbox modes the harness's own tools enforce here; `open` applies no sandbox and always works.
    #[must_use]
    pub fn modes(&self) -> Vec<SandboxMode> {
        if self.workspace {
            vec![SandboxMode::Workspace, SandboxMode::Open]
        } else {
            vec![SandboxMode::Open]
        }
    }
}

impl Default for SandboxSupport {
    fn default() -> Self {
        Self::declared()
    }
}

/// What the harness's sandbox does where `launcher` runs the harness, with the harness's own environment; what
/// the build supports when the launcher does not run programs there.
pub async fn probe(
    launcher: &dyn Launcher,
    environment: &BTreeMap<String, String>,
) -> SandboxSupport {
    if !launcher.runs_here() {
        return SandboxSupport::declared();
    }
    match std::env::consts::OS {
        "linux" => linux(launcher, environment).await,
        "macos" if std::path::Path::new(SEATBELT).exists() => SandboxSupport::confines(false),
        "macos" => SandboxSupport::unavailable(format!(
            "Claude Code's sandbox needs macOS's {SEATBELT}, which is missing on this machine."
        )),
        "windows" => SandboxSupport::unavailable(
            "Claude Code runs commands without a sandbox on Windows; its sandbox needs WSL2.",
        ),
        other => SandboxSupport::unavailable(format!("Claude Code has no sandbox on {other}.")),
    }
}

/// On Linux: both programs are there, and bubblewrap starts a command as the sandbox does, or as the weaker nested
/// sandbox does.
async fn linux(launcher: &dyn Launcher, environment: &BTreeMap<String, String>) -> SandboxSupport {
    let mut missing = Vec::new();
    for (program, argument, name) in [
        ("bwrap", "--version", "bubblewrap (bwrap)"),
        ("socat", "-V", "socat"),
    ] {
        if let Err(Started::Not) = run(launcher, environment, program, &[argument]).await {
            missing.push(name);
        }
    }
    if !missing.is_empty() {
        return SandboxSupport::unavailable(missing_programs(&missing));
    }
    match run(launcher, environment, "bwrap", &FULL).await {
        Ok(()) => SandboxSupport::confines(false),
        Err(full) => match run(launcher, environment, "bwrap", &WEAKER).await {
            Ok(()) => SandboxSupport::confines(true),
            Err(_) => SandboxSupport::unavailable(format!(
                "Claude Code's sandbox cannot start on this machine (bubblewrap: {}).",
                full.said()
            )),
        },
    }
}

/// The reason of a sandbox whose programs are missing, naming them as the distribution's packages are named.
fn missing_programs(missing: &[&str]) -> String {
    let names = match missing {
        [one] => (*one).to_owned(),
        [first, second] => format!("{first} and {second}"),
        _ => missing.join(", "),
    };
    let verb = if missing.len() == 1 { "is" } else { "are" };
    format!("Claude Code's sandbox needs {names}, which {verb} not installed on this machine.")
}

/// How a probe command went, where it did not end well.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Started {
    /// The program could not be started: it is not installed, or not on the harness's search path.
    Not,
    /// It started and failed, with the first line it wrote to its standard error.
    Failed(String),
}

impl Started {
    /// The first clause of what bubblewrap said, without its name and the advice after it: the part that says why.
    fn said(&self) -> String {
        match self {
            Self::Not => "it could not be started".to_owned(),
            Self::Failed(line) => {
                let line = line.strip_prefix("bwrap:").unwrap_or(line).trim();
                line.split([',', ';'])
                    .next()
                    .unwrap_or(line)
                    .trim_end_matches('.')
                    .trim()
                    .to_owned()
            }
        }
    }
}

async fn run(
    launcher: &dyn Launcher,
    environment: &BTreeMap<String, String>,
    program: &str,
    args: &[&str],
) -> Result<(), Started> {
    let spec = LaunchSpec {
        program: program.to_owned(),
        args: args.iter().map(|argument| (*argument).to_owned()).collect(),
        env: environment.clone(),
        cwd: std::env::temp_dir(),
        label: "sandbox".to_owned(),
    };
    match run_to_end(launcher, &spec, PROBE_COMMAND).await {
        Ok(finished) if finished.exit.code == Some(0) => Ok(()),
        Ok(finished) => Err(Started::Failed(
            finished
                .exit
                .stderr_tail
                .iter()
                .map(|line| line.trim())
                .find(|line| !line.is_empty())
                .map_or_else(|| finished.exit.describe(), str::to_owned)
                .chars()
                .take(REASON_CHARS)
                .collect(),
        )),
        Err(RunError::Spawn(_)) => Err(Started::Not),
        Err(other) => Err(Started::Failed(other.to_string())),
    }
}

/// What Claude Code said when it refused to start because its sandbox could not, in plain words: the cause, and
/// the two ways on. None for anything else it said.
#[must_use]
pub fn refusal_in_words(said: &str) -> Option<String> {
    if !said.contains("sandbox required but unavailable") {
        return None;
    }
    let mut missing = Vec::new();
    if said.contains("bwrap) not installed") || said.contains("bubblewrap not installed") {
        missing.push("bubblewrap (bwrap)");
    }
    if said.contains("socat not installed") {
        missing.push("socat");
    }
    let open = "Choose Open in this thread's tool access to run without it";
    Some(if missing.is_empty() {
        format!(
            "Claude Code did not start because its sandbox cannot start on this machine. {open}."
        )
    } else {
        let names = missing.join(" and ");
        let verb = if missing.len() == 1 { "is" } else { "are" };
        format!(
            "Claude Code did not start because its sandbox cannot start on this machine: {names} {verb} not installed. {open}, or install bubblewrap and socat where Claude Code runs."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use crate::process::DirectLauncher;
    #[cfg(target_os = "linux")]
    use std::os::unix::fs::PermissionsExt;

    /// A directory holding stand-ins for `bwrap` and `socat` that end as told, and the harness's search path with it
    /// first, so that the real programs of the machine that runs the test are never found.
    #[cfg(target_os = "linux")]
    fn place(bwrap: Option<&str>, socat: bool) -> (tempfile::TempDir, BTreeMap<String, String>) {
        let directory = tempfile::tempdir().unwrap();
        let write = |name: &str, body: &str| {
            let path = directory.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        if let Some(body) = bwrap {
            write("bwrap", body);
        }
        if socat {
            write("socat", "exit 0");
        }
        let environment =
            BTreeMap::from([("PATH".to_owned(), directory.path().display().to_string())]);
        (directory, environment)
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn workspace_is_declared_only_where_bubblewrap_starts_the_sandbox_here() {
        let (_kept, environment) = place(Some("exit 0"), true);
        assert_eq!(
            probe(&DirectLauncher, &environment).await,
            SandboxSupport::confines(false)
        );
        // A container that keeps its runtime's /proc: only the weaker nested sandbox starts.
        let (_kept, environment) = place(
            Some(
                r#"for a in "$@"; do [ "$a" = --proc ] && { echo "bwrap: Can't mount proc on /newroot/proc: Operation not permitted" >&2; exit 1; }; done; exit 0"#,
            ),
            true,
        );
        let weaker = probe(&DirectLauncher, &environment).await;
        assert!(weaker.workspace && weaker.weaker_nested);
        assert_eq!(weaker.modes(), [SandboxMode::Workspace, SandboxMode::Open]);
        // A stock container: no namespace at all. Its words are kept up to the advice that follows them.
        let (_kept, environment) = place(
            Some(
                r#"[ "$1" = --version ] && exit 0; echo "bwrap: No permissions to create new namespace, likely because the kernel does not allow non-privileged user namespaces. See <https://deb.li/bubblewrap>." >&2; exit 1"#,
            ),
            true,
        );
        let refused = probe(&DirectLauncher, &environment).await;
        assert!(!refused.workspace);
        assert_eq!(refused.modes(), [SandboxMode::Open]);
        assert_eq!(
            refused.reason.as_deref(),
            Some(
                "Claude Code's sandbox cannot start on this machine (bubblewrap: No permissions to create new namespace)."
            )
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn missing_programs_are_named_as_their_packages_are() {
        let (_kept, environment) = place(None, false);
        let neither = probe(&DirectLauncher, &environment).await;
        assert_eq!(neither.modes(), [SandboxMode::Open]);
        assert_eq!(
            neither.reason.as_deref(),
            Some(
                "Claude Code's sandbox needs bubblewrap (bwrap) and socat, which are not installed on this machine."
            )
        );
        let (_kept, environment) = place(Some("exit 0"), false);
        assert_eq!(
            probe(&DirectLauncher, &environment).await.reason.as_deref(),
            Some("Claude Code's sandbox needs socat, which is not installed on this machine.")
        );
    }

    #[tokio::test]
    async fn a_launcher_that_does_not_run_the_harness_here_declares_what_the_build_supports() {
        #[derive(Debug)]
        struct Elsewhere;
        impl Launcher for Elsewhere {
            fn command(&self, _: &LaunchSpec) -> tokio::process::Command {
                panic!("nothing is started to learn what the build declares");
            }
        }
        assert_eq!(
            probe(&Elsewhere, &BTreeMap::new()).await,
            SandboxSupport::declared()
        );
    }

    #[test]
    fn a_start_refused_for_the_sandbox_is_said_in_plain_words_with_the_ways_on() {
        let said = "exited with status 1: claude.ai connectors are disabled because ANTHROPIC_API_KEY is set | Error: sandbox required but unavailable: bubblewrap (bwrap) not installed, socat not installed. sandbox.failIfUnavailable is set";
        let words = refusal_in_words(said).unwrap();
        assert_eq!(
            words,
            "Claude Code did not start because its sandbox cannot start on this machine: bubblewrap (bwrap) and socat are not installed. Choose Open in this thread's tool access to run without it, or install bubblewrap and socat where Claude Code runs."
        );
        assert!(!words.contains("ANTHROPIC_API_KEY"));
        assert!(refusal_in_words("exited with status 1: No conversation found").is_none());
        // A sandbox that is installed and cannot start is not said to be missing.
        assert_eq!(
            refusal_in_words("Error: sandbox required but unavailable: bwrap: No permissions to create new namespace").unwrap(),
            "Claude Code did not start because its sandbox cannot start on this machine. Choose Open in this thread's tool access to run without it."
        );
    }
}
