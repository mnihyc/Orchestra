//! The running connector's local socket: what its console and its commands ask it, on this machine only.
//!
//! A connector that runs, from its service or by hand, listens in its data directory. On Unix that is a socket
//! file, `console.sock`, which only its user can open: the data directory is that user's alone and the socket is
//! made `0600`. On Windows, where such a file is not open to every build, it is the loopback, with the port and a
//! token of 32 random bytes kept in `console.token` there, which only that user can read; a connection that does
//! not give the token first is closed. Each request is one line of JSON, and so is its answer.
//!
//! What can be asked is what the console shows and what `orch-connector mcp` reads: the session, the environments
//! registered here, the tool servers the harnesses here have configured and which of them have a copy, and what
//! the system asked of the machine lately. The owner's limits are not changed here: they are the configuration
//! file's, which the running connector reads within seconds, so the socket can do nothing the file cannot.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::enrollment::DataDir;
use async_trait::async_trait;
use orch_link::commands::{CommandContext, CommandExecution, CommandHandler, CommandOutcome};
use orch_link::machine::{LinkStatus, MachineLink};
use orch_link_contract::commands::{Command, CommandError, CommandResult};
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio_util::sync::CancellationToken;

/// The socket file on Unix.
pub const SOCKET_FILE: &str = "console.sock";
/// The file that holds the loopback port and its token on Windows.
pub const TOKEN_FILE: &str = "console.token";
/// How many requests' answers may be in one line: enough for a few hundred rows.
const LINE_BYTES: usize = 1024 * 1024;
/// How long a client waits for the connector to answer.
pub const ANSWER_LIMIT: Duration = Duration::from_secs(15);
/// How many of the system's latest requests the activity keeps: several screens of them.
const ACTIVITY_KEPT: usize = 200;

/// What a client asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Adopts a same-machine replacement identity after its token has been checked and files saved.
    ReloadIdentity,
    /// The session and who this machine is.
    Status,
    /// The environments registered here.
    Environments,
    /// The tool servers the harnesses here have configured, and which of them have a copy.
    McpList,
    /// Imports through the system, which files and audits the copy as this machine's current owner.
    McpImport {
        /// The configured server's name.
        name: String,
        /// The harness to disambiguate, where supplied.
        from: Option<String>,
        /// The console's selected place; the command uses the name and harness alone.
        place: Option<String>,
    },
    /// Deletes the copy of the tool server called `name`, from the harness `from` where several have one.
    McpForget {
        /// The server's name in the harness's configuration.
        name: String,
        /// The harness kind.
        from: Option<String>,
    },
    /// What the system asked of the machine lately, newest first.
    Activity {
        /// How many at most.
        limit: Option<usize>,
    },
}

/// The session as the connector has it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// Whether a session stands.
    pub connected: bool,
    /// The address of the gateway it stands through.
    pub endpoint: Option<String>,
    /// The link's state in a few words.
    pub link: String,
    /// The connector's version.
    pub version: String,
    /// How many turns run on the harnesses here now.
    pub turns_running: usize,
    /// Latest authenticated recommendation, retained through a reconnect.
    #[serde(default)]
    pub recommended: Option<String>,
    /// Whether the owner's current local configuration allows automatic recommended updates.
    #[serde(default)]
    pub update_auto: bool,
}

/// One environment registered here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentRow {
    /// Its identifier.
    pub id: Id,
    /// The directory it works in.
    pub root: String,
}

/// One tool server a harness here has configured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpRow {
    /// Its name there.
    pub name: String,
    /// The harness kind.
    pub harness: String,
    /// Where it is configured: `user` or a directory.
    pub place: String,
    /// How it is reached.
    pub transport: String,
    /// Whether a copy of this very definition is kept for Orch.
    pub imported: bool,
}

/// One thing the system asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityRow {
    /// When it was answered.
    pub at: Time,
    /// The command's kind.
    pub kind: String,
    /// `done`, or the code it was refused or failed with.
    pub outcome: String,
    /// Why, where it was not done.
    pub detail: Option<String>,
}

/// What the connector answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    /// The session.
    Status(Status),
    /// The environments.
    Environments {
        /// Each one.
        environments: Vec<EnvironmentRow>,
    },
    /// The tool servers.
    Mcp {
        /// Each one.
        servers: Vec<McpRow>,
    },
    /// The activity.
    Activity {
        /// Each request, newest first.
        rows: Vec<ActivityRow>,
    },
    /// A request was carried out.
    Done {
        /// What came of it.
        said: String,
    },
    /// A request could not be carried out.
    Error {
        /// Why.
        message: String,
    },
}

/// What the system asked lately, kept in memory only.
#[derive(Debug, Default)]
pub struct Activity {
    rows: Mutex<VecDeque<ActivityRow>>,
}

impl Activity {
    fn push(&self, row: ActivityRow) {
        let mut rows = self.rows.lock().unwrap_or_else(PoisonError::into_inner);
        rows.push_front(row);
        rows.truncate(ACTIVITY_KEPT);
    }

    /// The latest rows, newest first, at most `limit`.
    #[must_use]
    pub fn latest(&self, limit: usize) -> Vec<ActivityRow> {
        self.rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }
}

/// A command handler that keeps what it was asked and how it answered in [`Activity`].
pub struct Recording {
    inner: Arc<dyn CommandHandler>,
    activity: Arc<Activity>,
    clock: Arc<dyn Clock>,
}

impl Recording {
    /// Records what `inner` is asked.
    #[must_use]
    pub fn new(
        inner: Arc<dyn CommandHandler>,
        activity: Arc<Activity>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            inner,
            activity,
            clock,
        }
    }

    fn record_result(
        activity: &Activity,
        clock: &dyn Clock,
        kind: String,
        answer: &CommandOutcome,
    ) {
        let (outcome, detail) = match answer {
            Ok(_) => ("done".to_owned(), None),
            Err(error) => (error.code.clone(), Some(error.detail.clone())),
        };
        activity.push(ActivityRow {
            at: clock.time(),
            kind,
            outcome,
            detail,
        });
    }
}

#[async_trait]
impl CommandHandler for Recording {
    fn answered(&self, id: Id) {
        self.inner.answered(id);
    }

    async fn begin(&self, context: CommandContext, command: Command) -> CommandExecution {
        let kind = command.kind().to_string();
        match self.inner.begin(context, command).await {
            CommandExecution::Complete(answer) => {
                Self::record_result(self.activity.as_ref(), self.clock.as_ref(), kind, &answer);
                CommandExecution::Complete(answer)
            }
            CommandExecution::Captured { finish, hold } => {
                let activity = Arc::clone(&self.activity);
                let clock = Arc::clone(&self.clock);
                CommandExecution::Captured {
                    finish: Box::pin(async move {
                        let answer = finish.await;
                        Self::record_result(activity.as_ref(), clock.as_ref(), kind, &answer);
                        answer
                    }),
                    hold,
                }
            }
        }
    }

    async fn handle(
        &self,
        context: CommandContext,
        command: Command,
    ) -> Result<CommandResult, CommandError> {
        self.begin(context, command).await.complete().await
    }
}

/// What the socket answers from.
pub struct LocalState {
    /// The physical supervisor's replacement request, absent on an unsupervised role host.
    pub identity_reload: Option<Arc<tokio::sync::Notify>>,
    /// The certificate this running link actually holds.
    pub identity_fingerprint: String,
    /// The link to the gateway.
    pub link: MachineLink,
    /// The connector's version.
    pub version: String,
    /// The data directory.
    pub data_dir: PathBuf,
    /// The environment role, where the machine plays it.
    pub environments: Option<Arc<crate::roles::environment::EnvironmentRole>>,
    /// The harnesses hosted here, where the machine plays the agent role.
    pub hosting: Option<Arc<crate::harnesses::Hosting>>,
    /// What the system asked lately.
    pub activity: Arc<Activity>,
}

impl LocalState {
    async fn answer(&self, request: Request) -> Reply {
        match request {
            Request::ReloadIdentity => self.reload_identity(),
            Request::Status => Reply::Status(self.status()),
            Request::Environments => Reply::Environments {
                environments: self
                    .environments
                    .as_ref()
                    .map(|role| {
                        role.host()
                            .environments()
                            .iter()
                            .map(|environment| EnvironmentRow {
                                id: environment.id(),
                                root: environment.facts().workspace_root,
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            Request::McpList => Reply::Mcp {
                servers: self.servers().await,
            },
            Request::McpImport { name, from, place } => {
                self.import(&name, from.as_deref(), place.as_deref()).await
            }
            Request::McpForget { name, from } => self.forget(&name, from.as_deref()).await,
            Request::Activity { limit } => Reply::Activity {
                rows: self.activity.latest(limit.unwrap_or(ACTIVITY_KEPT)),
            },
        }
    }

    fn reload_identity(&self) -> Reply {
        let Some(reload) = &self.identity_reload else {
            return Reply::Error {
                message: "restart this runner to use the saved identity".into(),
            };
        };
        match DataDir::open(&self.data_dir).and_then(|data| data.identity()) {
            Ok(identity) => {
                if identity.certificate.fingerprint() != self.identity_fingerprint {
                    reload.notify_one();
                }
                Reply::Done {
                    said: "the replacement identity will be used when this runner is idle".into(),
                }
            }
            Err(error) => Reply::Error {
                message: error.to_string(),
            },
        }
    }

    fn status(&self) -> Status {
        let state = self.link.status().borrow().clone();
        let (connected, endpoint, link) = match state {
            LinkStatus::Connected { endpoint, .. } => {
                (true, Some(endpoint), "connected".to_owned())
            }
            LinkStatus::Starting => (false, None, "starting".to_owned()),
            LinkStatus::Connecting { failures } => (
                false,
                None,
                format!("connecting ({failures} attempts failed before)"),
            ),
            LinkStatus::Waiting {
                failures,
                retry_in,
                why,
            } => (
                false,
                None,
                format!(
                    "waiting {} s before trying again ({failures} attempts failed): {why}",
                    retry_in.as_secs()
                ),
            ),
            LinkStatus::Refused { code, detail, .. } => (
                false,
                None,
                format!("refused by the system ({code:?}): {detail}"),
            ),
            LinkStatus::Stopped => (false, None, "stopped".to_owned()),
        };
        Status {
            connected,
            endpoint,
            link,
            version: self.version.clone(),
            recommended: self.link.versions().map(|versions| versions.recommended),
            update_auto: DataDir::open(&self.data_dir)
                .and_then(|data| data.config(|name| std::env::var(name).ok()))
                .is_ok_and(|config| config.update.auto),
            turns_running: self.hosting.as_ref().map_or(0, |hosting| {
                hosting
                    .recorded()
                    .iter()
                    .flat_map(|harness| harness.sessions.iter())
                    .filter(|session| session.running_turn.is_some())
                    .count()
            }),
        }
    }

    /// The servers the harnesses here have configured, in their users' own configuration and in the directories
    /// of the environments registered here, each marked imported where a copy of the same definition is kept.
    async fn servers(&self) -> Vec<McpRow> {
        let Some(role) = &self.environments else {
            return Vec::new();
        };
        let roots: Vec<String> = role
            .host()
            .environments()
            .iter()
            .map(|environment| environment.facts().workspace_root)
            .collect();
        let copies = copies(&self.data_dir);
        role.imports()
            .found(&roots)
            .await
            .into_iter()
            .map(|found| McpRow {
                imported: copies.iter().any(|(_, digest)| *digest == found.digest),
                place: found.directory.clone().unwrap_or_else(|| "user".to_owned()),
                transport: serde_json::to_value(found.transport)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                harness: found.source,
                name: found.name,
            })
            .collect()
    }

    async fn import(&self, name: &str, from: Option<&str>, place: Option<&str>) -> Reply {
        let Some(role) = &self.environments else {
            return Reply::Error {
                message:
                    "this machine plays no environment role, so it has no MCP discovery context"
                        .to_owned(),
            };
        };
        let allowed = DataDir::open(&self.data_dir)
            .and_then(|data| data.config(|name| std::env::var(name).ok()))
            .is_ok_and(|config| config.mcp.import);
        if !allowed {
            return Reply::Error {
                message: "policy.mcp_import_disabled: this machine permits no MCP imports"
                    .to_owned(),
            };
        }
        let mut candidates = Vec::new();
        for environment in role.host().environments() {
            if !environment.mcp().import_allowed() {
                continue;
            }
            let root = environment.facts().workspace_root;
            for found in role.imports().found(&[root]).await {
                candidates.push((environment.id(), found));
            }
        }
        let (environment, found) = match choose_import(candidates, name, from, place) {
            Ok(found) => found,
            Err(message) => return Reply::Error { message },
        };
        let request = orch_link_contract::mcp_import::McpImportAsk {
            request_id: orch_types::new_id(),
            environment_id: environment,
            source: found.source,
            name: found.name,
            scope: match found.scope {
                orch_agent_contract::discovery::McpScope::User => {
                    orch_api_contract::environment::McpScope::User
                }
                orch_agent_contract::discovery::McpScope::Directory => {
                    orch_api_contract::environment::McpScope::Project
                }
            },
            directory: found.directory,
        };
        match self.link.import_mcp(request).await {
            Ok(answer) if answer.code.is_none() && answer.server_id.is_some() => Reply::Done {
                said: answer.detail,
            },
            Ok(answer) => Reply::Error {
                message: format!(
                    "{}: {}",
                    answer.code.as_deref().unwrap_or("unconfirmed"),
                    answer.detail
                ),
            },
            Err(error) => Reply::Error {
                message: error.to_string(),
            },
        }
    }

    async fn forget(&self, name: &str, from: Option<&str>) -> Reply {
        let Some(role) = &self.environments else {
            return Reply::Error {
                message: "this machine plays no environment role, so it keeps no tool servers"
                    .to_owned(),
            };
        };
        let roots: Vec<String> = role
            .host()
            .environments()
            .iter()
            .map(|environment| environment.facts().workspace_root)
            .collect();
        let found: Vec<_> = role
            .imports()
            .found(&roots)
            .await
            .into_iter()
            .filter(|found| found.name == name && from.is_none_or(|from| found.source == from))
            .collect();
        let copies = copies(&self.data_dir);
        let mut forgotten = 0;
        for server in &found {
            for (id, digest) in &copies {
                if *digest == server.digest && role.host().mcp_forget(*id).is_ok() {
                    forgotten += 1;
                }
            }
        }
        if forgotten == 0 {
            Reply::Error {
                message: format!("no copy of a tool server called {name} is kept here"),
            }
        } else {
            Reply::Done {
                said: format!("The copy of {name} is forgotten; Orch no longer starts it here."),
            }
        }
    }
}

/// User configuration may appear in several registered environments. Identical origins are
/// one choice; different harnesses/directories/definitions must never be chosen by accident.
fn choose_import(
    mut candidates: Vec<(Id, orch_agent_contract::discovery::McpFound)>,
    name: &str,
    from: Option<&str>,
    place: Option<&str>,
) -> Result<(Id, orch_agent_contract::discovery::McpFound), String> {
    candidates.sort_by_key(|(environment, _)| *environment);
    let mut choices = Vec::new();
    for (environment, found) in candidates.into_iter().filter(|(_, found)| {
        found.name == name
            && from.is_none_or(|from| found.source == from)
            && place.is_none_or(|place| found.directory.as_deref().unwrap_or("user") == place)
    }) {
        if !choices.iter().any(|(_, previous)| previous == &found) {
            choices.push((environment, found));
        }
    }
    if choices.len() > 1 {
        return Err(format!(
            "several definitions called {name} were found; select a harness with --from, or choose the exact directory on the environment's Tools page"
        ));
    }
    choices.pop().ok_or_else(|| {
        format!("no importable tool server called {name} was found in a registered environment")
    })
}

/// The copies of tool servers kept in the data directory, each with the digest of its definition.
fn copies(data_dir: &Path) -> Vec<(Id, String)> {
    let Ok(entries) = std::fs::read_dir(data_dir.join("mcp")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let id: Id = path.file_stem()?.to_str()?.parse().ok()?;
            let bytes = std::fs::read(&path).ok()?;
            let definition: orch_agent_contract::discovery::McpDefinition =
                serde_json::from_slice(&bytes).ok()?;
            Some((id, definition.digest().to_string()))
        })
        .collect()
}

/// Why a client did not get its answer.
#[derive(Debug, thiserror::Error)]
pub enum LocalError {
    /// No connector runs on this data directory, or it does not listen.
    #[error(
        "no connector runs on {0}: start it (`orch-connector run`, or its service) and ask again"
    )]
    NotRunning(String),
    /// The exchange failed.
    #[error("the connector did not answer: {0}")]
    Exchange(String),
}

/// Reads a line without allowing an unterminated line to grow past the limit.
async fn bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    // Permit a CRLF after the longest valid line without buffering more than that.
    let bound = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(2);
    let count = reader.take(bound).read_until(b'\n', &mut bytes).await?;
    if count == 0 {
        return Ok(None);
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    if bytes.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the line is too long",
        ));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Answers one connection: lines of requests, each with a line of answer.
async fn converse<S>(state: Arc<LocalState>, stream: S, token: Option<String>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    if let Some(token) = token {
        match bounded_line(&mut reader, token.len()).await {
            Ok(Some(given)) if bool::from(given.as_bytes().ct_eq(token.as_bytes())) => {}
            _ => return,
        }
    }
    while let Ok(Some(line)) = bounded_line(&mut reader, LINE_BYTES).await {
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(request) => state.answer(request).await,
            Err(error) => Reply::Error {
                message: format!("the request does not read: {error}"),
            },
        };
        let Ok(mut text) = serde_json::to_string(&reply) else {
            return;
        };
        text.push('\n');
        if writer.write_all(text.as_bytes()).await.is_err() {
            return;
        }
    }
}

/// Listens until `stop` on the socket of `state`'s data directory.
pub async fn serve(state: Arc<LocalState>, stop: CancellationToken) {
    // Enrollment can finish while startup is still composing the local listener.
    if state.identity_reload.is_some() {
        let _ = state.reload_identity();
    }
    listen(state, stop).await;
}

#[cfg(unix)]
async fn listen(state: Arc<LocalState>, stop: CancellationToken) {
    use std::os::unix::fs::PermissionsExt;
    let path = state.data_dir.join(SOCKET_FILE);
    // A socket file a connector that stopped left behind is no other connector's: two do not share a directory.
    let _ = std::fs::remove_file(&path);
    let listener = match tokio::net::UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "the console's socket could not be opened");
            return;
        }
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    loop {
        let accepted = tokio::select! {
            () = stop.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        if let Ok((stream, _)) = accepted {
            tokio::spawn(converse(Arc::clone(&state), stream, None));
        }
    }
    let _ = std::fs::remove_file(&path);
}

#[cfg(not(unix))]
async fn listen(state: Arc<LocalState>, stop: CancellationToken) {
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!(%error, "the console's socket could not be opened");
            return;
        }
    };
    let Ok(address) = listener.local_addr() else {
        return;
    };
    let token = hex::encode(rand::random::<[u8; 32]>());
    let file = state.data_dir.join(TOKEN_FILE);
    if let Err(error) =
        orch_pki::certificates::write_private_file(&file, &format!("{}\n{token}\n", address.port()))
    {
        tracing::warn!(%error, "the console's token could not be kept");
        return;
    }
    loop {
        let accepted = tokio::select! {
            () = stop.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        if let Ok((stream, _)) = accepted {
            tokio::spawn(converse(Arc::clone(&state), stream, Some(token.clone())));
        }
    }
    let _ = std::fs::remove_file(&file);
}

/// Sends one request over a connection and reads its answer.
async fn exchange<S>(
    stream: S,
    token: Option<String>,
    request: &Request,
) -> Result<Reply, LocalError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let failed = |error: std::io::Error| LocalError::Exchange(error.to_string());
    let (reader, mut writer) = tokio::io::split(stream);
    let mut text = String::new();
    if let Some(token) = token {
        text.push_str(&token);
        text.push('\n');
    }
    text.push_str(
        &serde_json::to_string(request).map_err(|error| LocalError::Exchange(error.to_string()))?,
    );
    text.push('\n');
    writer.write_all(text.as_bytes()).await.map_err(failed)?;
    let mut reader = BufReader::new(reader);
    let line = bounded_line(&mut reader, LINE_BYTES)
        .await
        .map_err(failed)?
        .ok_or_else(|| {
            LocalError::Exchange("the connection closed without an answer".to_owned())
        })?;
    serde_json::from_str(&line).map_err(|error| LocalError::Exchange(error.to_string()))
}

/// Asks the connector running on `data_dir` one thing.
pub async fn ask(data_dir: &Path, request: &Request) -> Result<Reply, LocalError> {
    let limit = if matches!(request, Request::McpImport { .. }) {
        Duration::from_secs(orch_link_contract::mcp_import::ANSWER_WITHIN_SECS) + ANSWER_LIMIT
    } else {
        ANSWER_LIMIT
    };
    tokio::time::timeout(limit, connect_and_ask(data_dir, request))
        .await
        .map_err(|_| LocalError::Exchange(format!("no answer within {} s", limit.as_secs())))?
}

#[cfg(unix)]
async fn connect_and_ask(data_dir: &Path, request: &Request) -> Result<Reply, LocalError> {
    let stream = tokio::net::UnixStream::connect(data_dir.join(SOCKET_FILE))
        .await
        .map_err(|_| LocalError::NotRunning(data_dir.display().to_string()))?;
    exchange(stream, None, request).await
}

#[cfg(not(unix))]
async fn connect_and_ask(data_dir: &Path, request: &Request) -> Result<Reply, LocalError> {
    let not_running = || LocalError::NotRunning(data_dir.display().to_string());
    let kept = std::fs::read_to_string(data_dir.join(TOKEN_FILE)).map_err(|_| not_running())?;
    let mut parts = kept.lines();
    let port: u16 = parts
        .next()
        .and_then(|port| port.trim().parse().ok())
        .ok_or_else(not_running)?;
    let token = parts.next().map(str::trim).unwrap_or_default().to_owned();
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|_| not_running())?;
    exchange(stream, Some(token), request).await
}

#[cfg(test)]
mod recording_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_local_console_refuses_an_oversized_unfinished_line_without_waiting_for_a_newline() {
        let (mut sender, receiver) = tokio::io::duplex(32);
        sender.write_all(b"123456").await.unwrap();
        let mut reader = BufReader::new(receiver);
        let error = tokio::time::timeout(ANSWER_LIMIT, bounded_line(&mut reader, 4))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        drop(sender);
    }

    #[tokio::test]
    async fn a_local_console_reads_complete_lines_at_the_limit() {
        let (mut sender, receiver) = tokio::io::duplex(32);
        sender.write_all(b"1234\r\nnext\n").await.unwrap();
        let mut reader = BufReader::new(receiver);
        assert_eq!(
            bounded_line(&mut reader, 4).await.unwrap().as_deref(),
            Some("1234")
        );
        assert_eq!(
            bounded_line(&mut reader, 4).await.unwrap().as_deref(),
            Some("next")
        );
    }

    fn found(source: &str, directory: Option<&str>) -> orch_agent_contract::discovery::McpFound {
        serde_json::from_value(serde_json::json!({"name":"files","source":source,
            "scope":if directory.is_some(){"directory"}else{"user"},"directory":directory,"transport":"stdio","digest":"one"})).unwrap()
    }

    #[test]
    fn local_import_selects_a_unique_origin_and_never_a_different_harness_or_directory_silently() {
        let first = Id::from_u128(1);
        let second = Id::from_u128(2);
        let one = found("codex", None);
        let selected = choose_import(
            vec![(second, one.clone()), (first, one.clone())],
            "files",
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            selected,
            (first, one.clone()),
            "one user definition seen in two environments is one choice"
        );
        let different = found("claude-code", None);
        assert!(
            choose_import(
                vec![(first, one.clone()), (second, different.clone())],
                "files",
                None,
                None
            )
            .is_err()
        );
        assert_eq!(
            choose_import(
                vec![(first, one.clone()), (second, different)],
                "files",
                Some("codex"),
                None
            )
            .unwrap()
            .1,
            one
        );
        let directory = found("codex", Some("tools"));
        assert!(
            choose_import(
                vec![(first, one.clone()), (second, directory.clone())],
                "files",
                Some("codex"),
                None
            )
            .is_err()
        );
        assert_eq!(
            choose_import(
                vec![(first, one), (second, directory.clone())],
                "files",
                Some("codex"),
                Some("tools")
            )
            .unwrap()
            .1,
            directory
        );
        assert!(choose_import(Vec::new(), "missing", None, None).is_err());
    }

    #[test]
    fn requests_and_answers_are_one_line_of_json_each() {
        let request = Request::McpForget {
            name: "files".to_owned(),
            from: Some("codex".to_owned()),
        };
        let text = serde_json::to_string(&request).unwrap();
        assert_eq!(text, r#"{"op":"mcp_forget","name":"files","from":"codex"}"#);
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), request);
        let reply = Reply::Done {
            said: "ok".to_owned(),
        };
        let text = serde_json::to_string(&reply).unwrap();
        assert!(!text.contains('\n'));
        assert_eq!(serde_json::from_str::<Reply>(&text).unwrap(), reply);
    }

    #[test]
    fn the_activity_keeps_the_newest_first_and_no_more_than_it_holds() {
        let activity = Activity::default();
        for index in 0..(ACTIVITY_KEPT + 5) {
            activity.push(ActivityRow {
                at: Time::new(
                    chrono::DateTime::from_timestamp(i64::try_from(index).unwrap(), 0).unwrap(),
                ),
                kind: format!("kind-{index}"),
                outcome: "done".to_owned(),
                detail: None,
            });
        }
        let latest = activity.latest(usize::MAX);
        assert_eq!(latest.len(), ACTIVITY_KEPT);
        assert_eq!(latest[0].kind, format!("kind-{}", ACTIVITY_KEPT + 4));
        assert_eq!(activity.latest(3).len(), 3);
    }
}
