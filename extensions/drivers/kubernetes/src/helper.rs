//! Trusted init, guard and retained workload supervisor entry points.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use futures::future::BoxFuture;
use orch_driver_contract::driver::DriverError;
use rustix::process::Pid;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::time::Instant;

use crate::authenticated_control::GuardKey;
use crate::control_protocol::{GuardPhase, GuardReply, GuardRequest};
use crate::encrypted_transport::{read_envelope, write_envelope};
use crate::guard_dispatch::{GuardDispatcher, GuardOperations};
use crate::guard_link::{PinnedPeer, SupervisorChannel};
use crate::guard_stop::{self, ContainerScope, ProcessScope, SupervisorLink};
use crate::launch::LaunchFiles;
use crate::launch_store::LaunchStore;
use crate::private_environment::PrivateEnvironment;
use crate::run_files::RunDirectory;
use crate::subreaper;
use crate::supervisor_protocol::{self, ExitWire, SupervisorReply, SupervisorRequest};
use crate::supervisor_state::{Phase, SupervisorState};

const CONTROL: &str = "/run/orch-control";
const SOCKET: &str = "/run/orch-control/guard.sock";
const BOOTSTRAP: &str = "/run/orch-control/bootstrap.sock";
const RENDEZVOUS: &str = "/run/orch-rendezvous/supervisor.sock";
const RUN: &str = "/run/orch";
/// Where the connector keeps the record of its enrollment on the unit's state claim.
const IDENTITY_RECORD: &str = "/var/lib/orch/identity/enrollment.json";
// An idle service rechecks its channel every thirty seconds. Each active frame
// and control operation has a separate absolute deadline: a stop is given the node's
// whole grace and then its signals, and the node holds the exchange to 90 seconds.
const IDLE: Duration = Duration::from_secs(30);
const OPERATION: Duration = Duration::from_secs(80);
// How long the staging container waits for the tree, a guard for its first command and a supervisor for its
// guard: the transfer of the connector and of the harness builds a template asks for (hundreds of megabytes
// through the cluster's exec stream), on a loaded cluster, has to fit inside the node's 600 seconds for a create.
// 240 seconds ended the staging of the connector alone on a busy machine, and of a Codex build beside it every time.
const INITIALIZE: Duration = Duration::from_secs(540);
// How long the init container lets an exec session finish its acknowledgement before it exits.
const SETTLE: Duration = Duration::from_secs(10);

fn unavailable() -> DriverError {
    DriverError::Unavailable("the retained Kubernetes helper is unavailable".into())
}

fn permission(path: &str, mode: u32) -> Result<(), DriverError> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|_| unavailable())
}

fn listener(path: &str, mode: u32) -> Result<UnixListener, DriverError> {
    let socket = UnixListener::bind(path).map_err(|_| unavailable())?;
    permission(path, mode)?;
    Ok(socket)
}

async fn accept(socket: &UnixListener, deadline: Instant) -> Result<UnixStream, DriverError> {
    tokio::time::timeout_at(deadline, socket.accept())
        .await
        .map_err(|_| DriverError::Timeout)?
        .map(|(stream, _)| stream)
        .map_err(|_| unavailable())
}

struct Operations {
    rendezvous: Option<UnixListener>,
    supervisor: Option<SupervisorChannel<PinnedPeer>>,
    container: Option<String>,
    launch: LaunchStore,
    run: RunDirectory,
    phase: GuardPhase,
}

impl Operations {
    async fn attach(&mut self, container: String, deadline: Instant) -> Result<(), DriverError> {
        if self.supervisor.is_some() || self.container.is_some() {
            return Err(unavailable());
        }
        let socket = self.rendezvous.take().ok_or_else(unavailable)?;
        let stream = accept(&socket, deadline).await?;
        let mut channel = SupervisorChannel::pin(stream, &container)?;
        // No future workload can reconnect or impersonate this established peer.
        drop(socket);
        fs::remove_file(RENDEZVOUS).map_err(|_| unavailable())?;
        channel.prepare(deadline).await?;
        let scope = ContainerScope::new(&container, channel.supervisor_pid())?;
        if !scope.members(deadline)?.is_empty() || !channel.children(deadline).await?.none {
            return Err(unavailable());
        }
        self.supervisor = Some(channel);
        self.container = Some(container);
        self.phase = GuardPhase::Stopped;
        Ok(())
    }

    async fn observe(&mut self, deadline: Instant) -> Result<(), DriverError> {
        let Some(channel) = &mut self.supervisor else {
            if self.phase == GuardPhase::Unprepared {
                return Ok(());
            }
            return Err(unavailable());
        };
        channel.check_live()?;
        let children = channel.children(deadline).await?;
        let container = self.container.as_deref().ok_or_else(unavailable)?;
        let scope = ContainerScope::new(container, channel.supervisor_pid())?;
        match self.phase {
            GuardPhase::Stopped if !children.none || !scope.members(deadline)?.is_empty() => {
                return Err(unavailable());
            }
            GuardPhase::Running if children.connector_exit.is_some() || children.none => {
                // Failure is safe for succession only after every remaining writer is fenced.
                guard_stop::stop(channel, &scope, Duration::ZERO, deadline).await?;
                let (uid, gid) = channel.identity();
                self.launch.clear_stopped_run(&self.run, uid, gid)?;
                self.phase = GuardPhase::Failed;
            }
            _ => {}
        }
        Ok(())
    }
}

impl GuardOperations for Operations {
    fn execute(
        &mut self,
        command: GuardRequest,
        deadline: Instant,
    ) -> BoxFuture<'_, Result<GuardReply, DriverError>> {
        Box::pin(async move {
            match command {
                GuardRequest::Attach { container_id, .. } => {
                    self.attach(container_id, deadline).await?
                }
                GuardRequest::Prepare {
                    ca,
                    enrollment,
                    environment,
                    mounts,
                    ..
                } => {
                    self.observe(deadline).await?;
                    if self.phase != GuardPhase::Stopped {
                        return Err(unavailable());
                    }
                    self.launch.prepare(LaunchFiles {
                        ca,
                        enrollment,
                        environment,
                        mounts,
                    })?;
                }
                GuardRequest::Start { .. } => {
                    self.observe(deadline).await?;
                    if self.phase != GuardPhase::Stopped {
                        return Err(DriverError::Busy("the unit is not stopped".into()));
                    }
                    let channel = self.supervisor.as_mut().ok_or_else(unavailable)?;
                    let (uid, gid) = channel.identity();
                    self.run.prepare(self.launch.files()?, uid, gid)?;
                    channel.start(deadline).await?;
                    self.phase = GuardPhase::Running;
                    self.observe(deadline).await?;
                }
                GuardRequest::EnrollmentConfirmed { .. } => {
                    self.observe(deadline).await?;
                    let (uid, gid) = self.supervisor.as_ref().ok_or_else(unavailable)?.identity();
                    self.launch.confirm_enrollment(&self.run, uid, gid)?;
                }
                GuardRequest::Stop { grace_millis, .. } => {
                    let channel = self.supervisor.as_mut().ok_or_else(unavailable)?;
                    let container = self.container.as_deref().ok_or_else(unavailable)?;
                    let scope = ContainerScope::new(container, channel.supervisor_pid())?;
                    guard_stop::stop(
                        channel,
                        &scope,
                        Duration::from_millis(grace_millis),
                        deadline,
                    )
                    .await?;
                    let (uid, gid) = channel.identity();
                    self.launch.clear_stopped_run(&self.run, uid, gid)?;
                    self.phase = GuardPhase::Stopped;
                }
                GuardRequest::Inspect { .. } => self.observe(deadline).await?,
            }
            if Instant::now() >= deadline {
                return Err(DriverError::Timeout);
            }
            Ok(GuardReply::State { phase: self.phase })
        })
    }
}

async fn guard(pod_uid: &str) -> Result<(), DriverError> {
    subreaper::protect()?;
    permission(CONTROL, 0o700)?;
    // Only the original supervisor exists when this socket is visible. Its UID
    // is selected by the workload image and authenticated through SO_PEERPIDFD.
    permission("/run/orch-rendezvous", 0o755)?;
    let rendezvous = listener(RENDEZVOUS, 0o666)?;
    let bootstrap = listener(BOOTSTRAP, 0o600)?;
    let deadline = Instant::now() + INITIALIZE;
    let mut initial = accept(&bootstrap, deadline).await?;
    if initial.peer_cred().map_err(|_| unavailable())?.uid() != 0 {
        return Err(unavailable());
    }
    let mut key = [0_u8; 32];
    tokio::time::timeout_at(deadline, initial.read_exact(&mut key))
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| unavailable())?;
    let mut dispatch = GuardDispatcher::new(pod_uid, GuardKey::from_bytes(key))?;
    let mut operations = Operations {
        rendezvous: Some(rendezvous),
        supervisor: None,
        container: None,
        launch: LaunchStore::default(),
        run: RunDirectory::open(Path::new(RUN))?,
        phase: GuardPhase::Unprepared,
    };
    // Install the ordinary endpoint before acknowledging bootstrap. It carries
    // ciphertext only and has no operation for changing the key or pinned peer.
    let control = listener(SOCKET, 0o600)?;
    drop(bootstrap);
    fs::remove_file(BOOTSTRAP).map_err(|_| unavailable())?;
    tokio::time::timeout_at(deadline, initial.write_all(b"ready"))
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| unavailable())?;
    drop(initial);
    loop {
        let mut stream = match accept(&control, Instant::now() + IDLE).await {
            Ok(stream) => stream,
            Err(DriverError::Timeout) => continue,
            Err(error) => return Err(error),
        };
        let deadline = Instant::now() + OPERATION;
        let Ok((sequence, request)) = read_envelope(&mut stream, deadline).await else {
            continue;
        };
        let Ok(reply) = dispatch
            .handle(sequence, &request, &mut operations, deadline)
            .await
        else {
            continue;
        };
        // Losing a relay response does not restart or abandon the settled operation.
        let _ = write_envelope(&mut stream, sequence, &reply, deadline).await;
    }
}

async fn bootstrap() -> Result<(), DriverError> {
    let deadline = Instant::now() + OPERATION;
    let mut key = [0_u8; 32];
    tokio::time::timeout_at(deadline, tokio::io::stdin().read_exact(&mut key))
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| unavailable())?;
    let mut socket = tokio::time::timeout_at(deadline, UnixStream::connect(BOOTSTRAP))
        .await
        .map_err(|_| DriverError::Timeout)?
        .map_err(|_| unavailable())?;
    tokio::time::timeout_at(deadline, async {
        socket.write_all(&key).await?;
        let mut ready = [0_u8; 5];
        socket.read_exact(&mut ready).await?;
        if &ready != b"ready" {
            return Err(std::io::Error::other("bootstrap refused"));
        }
        tokio::io::stdout().write_all(&ready).await
    })
    .await
    .map_err(|_| DriverError::Timeout)?
    .map_err(|_| unavailable())
}

fn children(state: &mut SupervisorState) -> Result<subreaper::Children, DriverError> {
    let children = subreaper::reap(state.connector(), std::time::Instant::now() + OPERATION)?;
    state.observe(children)?;
    Ok(children)
}

fn spawn_connector(run: &RunDirectory) -> Result<Pid, DriverError> {
    let environment = PrivateEnvironment::read(run)?;
    let mut command = Command::new(crate::launch::CONNECTOR);
    command.args(["run", "--managed"]).stdin(Stdio::null());
    environment.apply(&mut command);
    let child = command.spawn().map_err(|_| unavailable())?;
    Pid::from_raw(i32::try_from(child.id()).map_err(|_| unavailable())?).ok_or_else(unavailable)
}

async fn supervisor() -> Result<(), DriverError> {
    subreaper::protect()?;
    let deadline = Instant::now() + INITIALIZE;
    let mut stream = loop {
        match UnixStream::connect(RENDEZVOUS).await {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep_until(
                    (Instant::now() + Duration::from_millis(100)).min(deadline),
                )
                .await
            }
            Err(_) => return Err(unavailable()),
        }
    };
    // The immutable trusted image owns the guard endpoint. The workload cannot
    // reconnect after the guard pins this original, non-dumpable process.
    if stream.peer_cred().map_err(|_| unavailable())?.uid() != 0 {
        return Err(unavailable());
    }
    let run = RunDirectory::open(Path::new(RUN))?;
    let mut state = SupervisorState::default();
    loop {
        // A node that stays away for longer than an operation takes (it restarts, or is held up) must not end the
        // unit: when no message begins within the idle time, the supervisor looks at its children and waits again.
        // It waits for the first byte of a message and not for the connection's readiness, which stays set after
        // a message read to its last byte and would hand the wait over to a read that ends the unit at its deadline.
        let Some(first) = supervisor_protocol::begins(&mut stream, IDLE).await? else {
            let _ = children(&mut state)?;
            continue;
        };
        let deadline = Instant::now() + OPERATION;
        let request = supervisor_protocol::read_from(&mut stream, Some(first), deadline).await?;
        let reply = match request {
            SupervisorRequest::Prepare => {
                let observed = subreaper::reap(None, deadline.into_std())?;
                state.prepare(observed)?;
                SupervisorReply::Prepared
            }
            SupervisorRequest::Start => {
                let observed = children(&mut state)?;
                state.start(observed, || spawn_connector(&run))?;
                SupervisorReply::Started
            }
            SupervisorRequest::Freeze => {
                state.begin_stop()?;
                SupervisorReply::Frozen
            }
            SupervisorRequest::Children => {
                let observed = children(&mut state)?;
                let exit = observed.connector_exit.or_else(|| {
                    if state.phase() == Phase::Failed {
                        state.exit()
                    } else {
                        None
                    }
                });
                SupervisorReply::Children {
                    none: observed.none,
                    connector_exit: exit.map(|exit| ExitWire {
                        code: exit.code,
                        signal: exit.signal,
                    }),
                }
            }
        };
        supervisor_protocol::write(&mut stream, &reply, deadline).await?;
    }
}

/// The receiving exec session acknowledges after it publishes the tree, and a container that ends takes the
/// processes of its exec sessions with it. The init container therefore waits, for a bounded time, until no
/// process other than itself and the Pod's pause process is left in the shared process space.
async fn settle(deadline: Instant) {
    let own = std::process::id();
    while Instant::now() < deadline {
        let busy = fs::read_dir("/proc").is_ok_and(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
                .any(|pid| pid != own && pid != 1)
        });
        if !busy {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Says on standard output whether the connector has stored its identity on the state claim. The node's
/// enrollment watch asks it through the workload container; it reads one name and changes nothing.
fn enrolled() -> Result<(), DriverError> {
    use std::io::Write;
    let found = fs::symlink_metadata(IDENTITY_RECORD).is_ok_and(|meta| meta.is_file());
    std::io::stdout()
        .write_all(if found { b"1" } else { b"0" })
        .map_err(|_| unavailable())
}

/// Runs the trusted helper; the supervisor is the same static binary staged under its own name.
pub async fn main() -> Result<(), DriverError> {
    let mut args = std::env::args();
    if args
        .next()
        .as_deref()
        .is_some_and(|name| name.ends_with("orch-kube-supervisor"))
    {
        return match args.next().as_deref() {
            None => supervisor().await,
            Some("enrolled") if args.next().is_none() => enrolled(),
            Some(_) => Err(unavailable()),
        };
    }
    let operation = args.next().ok_or_else(unavailable)?;
    if args.next().is_some() {
        return Err(unavailable());
    }
    match operation.as_str() {
        "stage" => {
            let pod_uid = std::env::var("ORCH_POD_UID").map_err(|_| unavailable())?;
            crate::stage_worker::prepare(Path::new("/stage"))?;
            crate::stage_worker::wait_ready(
                Path::new("/stage"),
                &pod_uid,
                Instant::now() + INITIALIZE,
            )
            .await?;
            settle(Instant::now() + SETTLE).await;
            Ok(())
        }
        "receive" => {
            let pod_uid = std::env::var("ORCH_POD_UID").map_err(|_| unavailable())?;
            crate::stage_exchange::receive(
                Path::new("/stage"),
                Path::new("/usr/local/bin/orch-kube-helper"),
                &pod_uid,
                &mut tokio::io::stdin(),
                &mut tokio::io::stdout(),
                OPERATION,
                Instant::now() + INITIALIZE,
            )
            .await?;
            Ok(())
        }
        "guard" => guard(&std::env::var("ORCH_POD_UID").map_err(|_| unavailable())?).await,
        "bootstrap" => bootstrap().await,
        "relay" => {
            crate::relay::relay(
                Path::new(SOCKET),
                &mut tokio::io::stdin(),
                &mut tokio::io::stdout(),
                Instant::now() + OPERATION,
            )
            .await
        }
        _ => Err(unavailable()),
    }
}
