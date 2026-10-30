//! Starting an agent's process and speaking `initialize` to it: the part of a session that comes before any session.
//!
//! [`start`] runs the program as the manifest says (its arguments, its variables, its directory, the files written
//! first), connects the streams, keeps the last lines of the program's error output, and watches for its end.
//! [`Launched::initialize`] says what Orch can do as a client and reads what the agent says it can do; where the
//! program has a companion plugin it also waits, for as long as the manifest says and no longer, for the plugin's
//! hello.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use orch_agent_contract::adapter::AdapterError;
use serde_json::json;
use tokio::sync::{Notify, watch};

use crate::extensions::{Extra, Hello};
use crate::process::{
    LaunchRequest, Launcher, ProcessControl, StderrTail, prepare_directory, write_private,
};
use crate::program::{AcpProgram, LaunchPlan};
use crate::rpc::{self, FrameTap, INITIALIZE_TIMEOUT, Incoming, Rpc, RpcError};
use crate::wire::{self, Initialized, PROTOCOL_VERSION};

/// How long a program that was asked to end by closing its input is given before it is ended: it has sessions to
/// put down.
pub const STOP_GRACE: Duration = Duration::from_secs(8);

/// How long an ended process is waited for after it was killed.
pub const KILL_GRACE: Duration = Duration::from_secs(5);

/// How long the program that prints its version is given.
pub const VERSION_TIMEOUT: Duration = Duration::from_secs(120);

/// A running program.
pub struct Launched {
    /// The connection.
    pub rpc: Rpc,
    /// What the agent says of its own accord.
    pub incoming: orch_agent_contract::flow::Receiver<Incoming>,
    /// The last lines it wrote on its error output.
    pub stderr: StderrTail,
    /// Set to why it ended, once it has.
    pub exit: watch::Receiver<Option<String>>,
    /// Ends it.
    pub kill: Arc<Notify>,
}

impl Launched {
    /// Ends the process now.
    pub fn end(&self) {
        self.kill.notify_one();
    }

    /// Waits until it has ended, for at most `grace`, and says whether it has.
    pub async fn ended_within(&mut self, grace: Duration) -> bool {
        tokio::time::timeout(grace, async {
            loop {
                if self.exit.borrow_and_update().is_some() {
                    return;
                }
                if self.exit.changed().await.is_err() {
                    return;
                }
            }
        })
        .await
        .is_ok()
    }

    /// How it ended (its exit code, or the signal that ended it), once it has.
    #[must_use]
    pub fn why_it_ended(&self) -> Option<String> {
        self.exit.borrow().clone()
    }
}

/// Ends a process when it is dropped, unless it was handed on: a session that did not come to be leaves nothing
/// running.
pub struct KillGuard {
    kill: Option<Arc<Notify>>,
}

impl KillGuard {
    /// A guard over the process `launched`.
    #[must_use]
    pub fn over(launched: &Launched) -> Self {
        Self {
            kill: Some(Arc::clone(&launched.kill)),
        }
    }

    /// The process is handed on and lives.
    pub fn disarm(mut self) {
        self.kill = None;
    }
}

impl Drop for KillGuard {
    fn drop(&mut self) {
        if let Some(kill) = self.kill.take() {
            kill.notify_one();
        }
    }
}

/// Writes the files of a plan, each readable by this user alone; a file that is to be kept as it is stays.
pub fn write_files(plan: &LaunchPlan) -> Result<(), AdapterError> {
    for directory in &plan.directories {
        prepare_directory(directory).map_err(|error| {
            AdapterError::HarnessUnavailable(format!(
                "{} could not be made: {error}",
                directory.display()
            ))
        })?;
    }
    for file in &plan.files {
        if !file.rewrite && file.path.exists() {
            continue;
        }
        write_private(&file.path, &file.content).map_err(|error| {
            AdapterError::HarnessUnavailable(format!(
                "{} could not be written: {error}",
                file.path.display()
            ))
        })?;
    }
    Ok(())
}

/// Makes the directories a launch needs: the home, the directory the program starts in, and its shared agent
/// files.
pub fn prepare_home(home: &Path) -> Result<(), AdapterError> {
    for directory in [
        home.to_path_buf(),
        home.join("neutral"),
        home.join("agents"),
    ] {
        prepare_directory(&directory).map_err(|error| {
            AdapterError::HarnessUnavailable(format!(
                "{} could not be made: {error}",
                directory.display()
            ))
        })?;
    }
    Ok(())
}

/// Starts the program of `program` as `plan` says, in `cwd`.
pub async fn start(
    launcher: &dyn Launcher,
    binary: &str,
    plan: &LaunchPlan,
    cwd: PathBuf,
    tap: Option<FrameTap>,
) -> Result<Launched, AdapterError> {
    write_files(plan)?;
    let spawned = launcher
        .launch(LaunchRequest {
            program: binary.to_owned(),
            args: plan.args.clone(),
            env: plan.env.clone(),
            cwd: Some(cwd),
        })
        .await
        .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
    let (rpc, incoming) = rpc::connect(spawned.stdout, spawned.stdin, tap);
    let stderr = StderrTail::default();
    if let Some(stream) = spawned.stderr {
        tokio::spawn(stderr.clone().follow(stream));
    }
    let (exit_sender, exit) = watch::channel(None::<String>);
    let kill = Arc::new(Notify::new());
    {
        let kill = Arc::clone(&kill);
        let mut control = spawned.control;
        tokio::spawn(async move {
            let ended = tokio::select! {
                ended = control.wait() => ended,
                () = kill.notified() => {
                    control.kill();
                    control.wait().await
                }
            };
            // How it ended and nothing else: what it wrote on its error output is added by whoever tells a person
            // why it ended, from what it wrote since the session was up and not from what it said while it started.
            exit_sender.send_replace(Some(format!("the program {ended}")));
        });
    }
    Ok(Launched {
        rpc,
        incoming,
        stderr,
        exit,
        kill,
    })
}

/// What Orch tells the agent it can do as a client.
#[must_use]
pub fn client_capabilities(client_methods: bool) -> serde_json::Value {
    json!({
        "fs": { "readTextFile": client_methods, "writeTextFile": client_methods },
        "terminal": client_methods,
        "elicitation": { "form": {} },
        "session": { "compaction": {} },
        "auth": { "terminal": false },
    })
}

/// What `initialize` learned.
pub struct Initialization {
    /// What the agent said of itself.
    pub initialized: Initialized,
    /// The hello of its companion plugin, where it said one.
    pub hello: Option<Hello>,
}

impl Launched {
    /// Speaks `initialize`; where the plugin was given to the program (`plugin_loaded`) and its hello has not
    /// come with the answer, waits for it as long as the manifest says.
    pub async fn initialize(
        &mut self,
        program: &AcpProgram,
        client_methods: bool,
        plugin_loaded: bool,
    ) -> Result<Initialization, AdapterError> {
        let answer = self
            .rpc
            .call_within(
                wire::agent::INITIALIZE,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientCapabilities": client_capabilities(client_methods),
                    "clientInfo": { "name": "orch", "title": "Orch", "version": env!("CARGO_PKG_VERSION") },
                }),
                INITIALIZE_TIMEOUT,
            )
            .await
            .map_err(|error| self.explain(error))?;
        let initialized: Initialized = serde_json::from_value(answer).map_err(|error| {
            AdapterError::HarnessUnavailable(format!(
                "its answer to initialize is not readable: {error}"
            ))
        })?;
        if initialized.protocol_version != PROTOCOL_VERSION {
            return Err(AdapterError::HarnessUnavailable(format!(
                "the program speaks version {} of the Agent Client Protocol, and this adapter drives version {PROTOCOL_VERSION}",
                initialized.protocol_version
            )));
        }
        let mut found = initialized
            .meta
            .as_ref()
            .and_then(|meta| meta.get("orch"))
            .and_then(Hello::read);
        if found.is_none()
            && plugin_loaded
            && let Some(companion) = &program.companion
        {
            let wait = Duration::from_millis(companion.hello_wait_ms);
            found = self.hello_said();
            if found.is_none() {
                // The plugin announces itself after the answer to `initialize`, and answers the same when it is asked.
                found = match self
                    .rpc
                    .call_within(wire::orch::HELLO, json!({}), wait)
                    .await
                {
                    Ok(answer) => Hello::read(&answer),
                    Err(error) => {
                        tracing::debug!(%error, "the companion plugin did not answer when asked for its hello");
                        None
                    }
                };
            }
            if found.is_none() {
                found = self.hello_said();
            }
        }
        Ok(Initialization {
            initialized,
            hello: found,
        })
    }

    /// The hello the plugin has already said, if it has: what was said before is read, and anything else only logged.
    fn hello_said(&mut self) -> Option<Hello> {
        while let Ok(message) = self.incoming.try_recv() {
            match message {
                Incoming::Notification { method, params } if method == wire::orch::HELLO => {
                    if let Some(hello) = Hello::read(&params) {
                        return Some(hello);
                    }
                }
                other => tracing::debug!(?other, "said before the plugin's hello"),
            }
        }
        None
    }

    /// A refusal carries the agent's own cause. Error output explains a lost process, not an RPC refusal.
    pub fn explain(&self, error: RpcError) -> AdapterError {
        match error {
            RpcError::Timeout { .. } => AdapterError::Timeout,
            RpcError::Remote { message, .. } => AdapterError::HarnessUnavailable(message),
            other => {
                let said = self.stderr.text();
                AdapterError::HarnessUnavailable(if said.is_empty() {
                    other.to_string()
                } else {
                    format!("{other}: {said}")
                })
            }
        }
    }
}

/// The extras of a hello that are on and that the manifest accepts, and the note about the plugin the descriptor
/// carries: who it is, which extras work, and why each of the others does not.
#[must_use]
pub fn plugin_note(program: &AcpProgram, hello: Option<&Hello>) -> (Vec<Extra>, serde_json::Value) {
    let Some(companion) = &program.companion else {
        return (Vec::new(), serde_json::Value::Null);
    };
    match hello {
        None => (
            Vec::new(),
            json!({ "plugin": null, "extras": [], "off": [], "missing": true }),
        ),
        Some(hello) => {
            let accepted = hello.accepted(&companion.extras);
            let note = json!({
                "plugin": { "name": hello.plugin.name, "version": hello.plugin.version },
                "protocol": hello.protocol,
                "host": { "version": hello.host.version, "tested": hello.host.tested },
                "extras": accepted.on.iter().map(|extra| extra.name()).collect::<Vec<_>>(),
                "off": accepted
                    .off
                    .iter()
                    .map(|(name, why)| json!({ "extra": name, "reason": why }))
                    .collect::<Vec<_>>(),
                "missing": false,
            });
            (accepted.on, note)
        }
    }
}

/// The program's version, from what it prints for the arguments the manifest names: the last word that is a
/// version.
pub async fn version(
    launcher: &dyn Launcher,
    binary: &str,
    program: &AcpProgram,
    env: &BTreeMap<String, String>,
) -> Result<String, AdapterError> {
    use tokio::io::AsyncReadExt;
    let spawned = launcher
        .launch(LaunchRequest {
            program: binary.to_owned(),
            args: program.version_args.clone(),
            env: env.clone(),
            cwd: None,
        })
        .await
        .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
    let mut control: Box<dyn ProcessControl> = spawned.control;
    let mut stdout = spawned.stdout;
    let mut printed = String::new();
    let read = tokio::time::timeout(VERSION_TIMEOUT, async {
        let mut buffer = vec![0_u8; 4096];
        loop {
            let read = stdout.read(&mut buffer).await.unwrap_or(0);
            if read == 0 || printed.len() > 16 * 1024 {
                break;
            }
            printed.push_str(&String::from_utf8_lossy(&buffer[..read]));
        }
    })
    .await;
    control.kill();
    let _ = tokio::time::timeout(KILL_GRACE, control.wait()).await;
    if read.is_err() {
        return Err(AdapterError::Timeout);
    }
    printed
        .split_whitespace()
        .rev()
        .find_map(|word| {
            semver::Version::parse(word.trim_start_matches('v'))
                .ok()
                .map(|version| version.to_string())
        })
        .ok_or_else(|| {
            AdapterError::HarnessUnavailable(format!(
                "the program did not print a version for {:?}: {}",
                program.version_args,
                printed.trim()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_rpc_refusal_keeps_the_cause_and_does_not_blame_an_ordinary_stderr_note() {
        let (read, write) = tokio::io::duplex(4096);
        let (rpc, incoming) = rpc::connect(read, write, None);
        let stderr = StderrTail::default();
        stderr
            .clone()
            .follow(Box::new(&b"an optional backend is off\n"[..]))
            .await;
        let (_exit, receiver) = watch::channel(None);
        let launched = Launched {
            rpc,
            incoming,
            stderr,
            exit: receiver,
            kill: Arc::new(Notify::new()),
        };
        let error = launched.explain(RpcError::Remote {
            method: "session/new".to_owned(),
            code: -32603,
            message: "the provider has no compatible protocol".to_owned(),
            data: Some(Box::new(
                json!({"details": "the provider has no compatible protocol"}),
            )),
        });
        assert_eq!(error.code(), "harness_unavailable");
        assert_eq!(error.detail(), "the provider has no compatible protocol");
    }
}
