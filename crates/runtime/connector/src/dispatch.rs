//! Role commands and durable event production.
//!
//! [`Dispatcher`] is what carries the system's commands out. The link has already written a command
//! down before it said it had it, queues it behind the commands with the same serial key, refuses one
//! whose deadline has passed, and will answer it again from the journal if it is sent again; the
//! dispatcher is what runs when its turn comes:
//!
//! 1. **The machine's own commands** (`machine.probe`, `machine.policy.set`) are answered here.
//! 2. **Local policy** is applied to every other command, whoever sent it and whatever role it is for
//!    ([`crate::policy::gate`]): a command the policy does not allow is refused with the policy's own
//!    error before any role has seen it, and a sandbox mode wider than the ceiling is lowered to it.
//! 3. The command goes to the **role** that owns its kind ([`RoleHandler`]), and no longer than the
//!    kind's budget: a role that hangs fails its command with `timeout` instead of holding up the
//!    commands behind it.
//! 4. A kind **no registered role takes** is answered `unsupported`, naming the command this
//!    connector's role composition cannot take. Every command receives an answer.
//!
//! One kind is taken by one role, except the kinds the catalogue addresses to either of two roles (the models
//! of a provider, asked of the machine that holds its sign-in or of the one that holds its key): both roles
//! may take them, and the one that claims the command ([`RoleHandler::claims`]) answers it; where both
//! claim, the provider role does, since it keeps the provider's key and a harness beside it only runs on it.
//!
//! The event half of a role (what an adapter reports) does not pass through here: roles publish to the
//! spool themselves, in the order things happened.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orch_link::commands::{CommandContext, CommandExecution, CommandHandler};
use orch_link_contract::commands::{
    Addressee, Command, CommandError, CommandKind, CommandResult, ErrorCode, MachinePolicySetResult,
};
use orch_link_contract::policy::LocalPolicy;
use orch_link_contract::session::MachineFacts;

use crate::policy::PolicyStore;
use crate::roles::{Call, RoleHandler};

fn record_cancelled() -> CommandError {
    CommandError::new(ErrorCode::Conflict, "the recording was cancelled")
}

fn record_timeout() -> CommandError {
    CommandError::new(
        ErrorCode::Timeout,
        "recording the files exceeded its allowed time",
    )
    .retryable()
}

fn stopped_capture(
    joined: Result<CommandExecution, tokio::task::JoinError>,
    cause: CommandError,
) -> CommandExecution {
    match joined {
        // Cancellation can race the last local step. Preserve any lease already acquired,
        // even though this publication must fail, until the server persists that failure.
        Ok(CommandExecution::Captured { hold, .. }) => CommandExecution::Captured {
            finish: Box::pin(std::future::ready(Err(cause))),
            hold,
        },
        _ => CommandExecution::ready(Err(cause)),
    }
}

/// Makes the facts of the machine as they are now.
pub type FactsNow = dyn Fn() -> MachineFacts + Send + Sync;

/// Two roles take one kind of command.
#[derive(Debug, thiserror::Error)]
#[error("two roles take commands of the kind {0}")]
pub struct DuplicateKind(pub CommandKind);

/// Carries the commands of the system out.
pub struct Dispatcher {
    clock: Arc<dyn orch_types::Clock>,
    roles: HashMap<CommandKind, Vec<Arc<dyn RoleHandler>>>,
    answer_roles: Vec<Arc<dyn RoleHandler>>,
    policy: Arc<PolicyStore>,
    facts: Arc<FactsNow>,
    local_harnesses: Option<(
        Arc<crate::local_harnesses::Discovery>,
        Arc<dyn crate::events::Publisher>,
    )>,
    turn_admission: Arc<tokio::sync::RwLock<()>>,
    updates: Arc<crate::automatic::Requests>,
    programs: Option<(
        Arc<dyn crate::programs::Handler>,
        Arc<crate::roles::Services>,
    )>,
    role_changes: Option<(
        Arc<crate::role_changes::Requests>,
        Arc<crate::roles::Services>,
    )>,
    gateway_control: Option<Arc<crate::gateway_control::Requests>>,
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher")
            .field("kinds", &self.roles.len())
            .finish_non_exhaustive()
    }
}

impl Dispatcher {
    fn role_for(&self, command: &Command) -> Result<Arc<dyn RoleHandler>, CommandError> {
        let kind = command.kind();
        let takers = self.roles.get(&kind).ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                format!("this connector holds no role that takes {kind}"),
            )
        })?;
        let claiming: Vec<_> = takers.iter().filter(|role| role.claims(command)).collect();
        claiming
            .iter()
            .find(|role| role.role() == orch_link_contract::session::Role::Provider)
            .or_else(|| claiming.first())
            .map(|role| Arc::clone(role))
            .ok_or_else(|| {
                let driver = match command {
                    Command::NodeVolumeEnsure(body) => Some(body.driver.as_str()),
                    Command::NodeVolumeRemove(body) => Some(body.driver.as_str()),
                    _ => None,
                };
                if let Some(driver) = driver {
                    return CommandError::new(
                        ErrorCode::Unsupported,
                        format!("this machine has no loaded storage driver {driver:?} for {kind}"),
                    );
                }
                CommandError::new(
                    ErrorCode::NotFound,
                    format!("nothing on this machine holds what {kind} names"),
                )
            })
    }

    async fn begin_record(&self, context: CommandContext, command: Command) -> CommandExecution {
        let started = tokio::time::Instant::now();
        let kind = CommandKind::FsRecord;
        let due = started + Duration::from_secs(u64::from(kind.deadline_secs()));
        let gated = match self.policy.gate(command) {
            Ok(gated) => gated,
            Err(error) => return CommandExecution::ready(Err(error)),
        };
        let role = match self.role_for(&gated.command) {
            Ok(role) => role,
            Err(error) => return CommandExecution::ready(Err(error)),
        };
        let cancel = context.cancel.clone();
        let call = Call {
            work_deadline: None,
            context,
            clamps: gated.clamps,
        };
        let mut capture = tokio::spawn(async move { role.begin(call, gated.command).await });
        let execution = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                // Do not drop a local publishing closure when cancellation arrives.
                return stopped_capture(capture.await, record_cancelled());
            }
            () = tokio::time::sleep_until(due) => {
                cancel.cancel();
                return stopped_capture(capture.await, record_timeout());
            }
            outcome = &mut capture => match outcome {
                Ok(execution) => execution,
                Err(_) => CommandExecution::ready(Err(CommandError::new(ErrorCode::Internal, "the recording handler failed"))),
            }
        };
        match execution {
            CommandExecution::Complete(outcome) => CommandExecution::Complete(outcome),
            CommandExecution::Captured { finish, hold } => CommandExecution::Captured {
                finish: Box::pin(async move {
                    tokio::select! {
                        biased;
                        () = cancel.cancelled() => Err(record_cancelled()),
                        () = tokio::time::sleep_until(due) => { cancel.cancel(); Err(record_timeout()) },
                        outcome = finish => outcome,
                    }
                }),
                hold,
            },
        }
    }
    /// A dispatcher that holds commands to `policy` and answers `machine.probe` with `facts`.
    #[must_use]
    pub fn new(policy: Arc<PolicyStore>, facts: Arc<FactsNow>) -> Self {
        Self {
            clock: Arc::new(orch_types::SystemClock),
            roles: HashMap::new(),
            answer_roles: Vec::new(),
            policy,
            facts,
            local_harnesses: None,
            turn_admission: Arc::new(tokio::sync::RwLock::new(())),
            updates: Arc::new(crate::automatic::Requests::new(
                false,
                crate::update::Program::Connector,
                String::new(),
            )),
            programs: None,
            role_changes: None,
            gateway_control: None,
        }
    }

    /// Use the same wall clock as the connector's native event suppliers.
    pub fn clock(&mut self, clock: Arc<dyn orch_types::Clock>) {
        self.clock = clock;
    }

    pub(crate) fn local_harnesses(
        &mut self,
        discovery: Arc<crate::local_harnesses::Discovery>,
        publisher: Arc<dyn crate::events::Publisher>,
    ) {
        self.local_harnesses = Some((discovery, publisher));
    }

    async fn report_local_programs(
        &self,
        publisher: &dyn crate::events::Publisher,
    ) -> MachineFacts {
        let facts = (self.facts)();
        if let Err(error) = publisher
            .publish(
                orch_link_contract::streams::Stream::Machine,
                orch_link_contract::events::Event::MachineFacts(facts.clone()),
            )
            .await
        {
            tracing::warn!(%error, "local program facts could not be queued");
        }
        facts
    }

    /// Gives the common machine command the physical program's supervisor queue.
    pub fn updates(&mut self, updates: Arc<crate::automatic::Requests>) {
        self.updates = updates;
    }

    /// The physical composition's one existing external-program supervisor.
    pub fn programs(
        &mut self,
        handler: Arc<dyn crate::programs::Handler>,
        services: Arc<crate::roles::Services>,
    ) {
        self.programs = Some((handler, services));
    }

    /// The physical connector's journal-aware restart queue and actual hosted-turn state.
    pub fn role_changes(
        &mut self,
        requests: Arc<crate::role_changes::Requests>,
        services: Arc<crate::roles::Services>,
    ) {
        self.role_changes = Some((requests, services));
    }

    pub fn gateway_control(&mut self, requests: Arc<crate::gateway_control::Requests>) {
        self.gateway_control = Some(requests);
    }

    /// Hands the commands a role takes to it. A kind that another role takes already is refused, unless the
    /// catalogue addresses it to either of two roles.
    pub fn add(&mut self, role: &Arc<dyn RoleHandler>) -> Result<(), DuplicateKind> {
        for kind in role.takes() {
            let taken = self
                .roles
                .get(kind)
                .is_some_and(|takers| !takers.is_empty());
            if taken
                && !matches!(
                    kind.addressee(),
                    Addressee::Either(_, _) | Addressee::Roles(_)
                )
            {
                return Err(DuplicateKind(*kind));
            }
        }
        for kind in role.takes() {
            self.roles.entry(*kind).or_default().push(Arc::clone(role));
        }
        self.answer_roles.push(Arc::clone(role));
        Ok(())
    }

    /// Whether a role takes commands of this kind.
    #[must_use]
    pub fn takes(&self, kind: CommandKind) -> bool {
        self.roles.contains_key(&kind)
    }

    /// Gate shared by turn starts and the connector's idle-only replacement of its own program.
    #[must_use]
    pub fn turn_admission(&self) -> Arc<tokio::sync::RwLock<()>> {
        Arc::clone(&self.turn_admission)
    }

    fn machine_roles_set(
        &self,
        context: &CommandContext,
        body: orch_link_contract::commands::MachineRolesSet,
    ) -> Result<CommandResult, CommandError> {
        let Some((requests, services)) = &self.role_changes else {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "This connector cannot change its roles while running. Update it first.",
            ));
        };
        let admission=Arc::clone(&self.turn_admission).try_write_owned().map_err(|_|CommandError::new(ErrorCode::Conflict,"A turn or program change is starting. Wait for it to finish before changing roles."))?;
        if services.agents.get().is_some_and(|hosting| {
            hosting
                .recorded()
                .iter()
                .flat_map(|harness| harness.sessions.iter())
                .any(|session| session.running_turn.is_some())
        }) {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "Wait for this machine's active turns to finish before changing its roles.",
            ));
        }
        self.policy
            .reload_from_file()
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        requests
            .request(context.id, &body, &self.policy, admission)
            .map(CommandResult::MachineRolesSet)
    }

    fn machine_policy_set(
        &self,
        values: &std::collections::BTreeMap<String, serde_json::Value>,
    ) -> Result<CommandResult, CommandError> {
        let (change, policy) = self
            .policy
            .apply_from_system(values)
            .map_err(|e| CommandError::new(ErrorCode::Internal, e.to_string()))?;
        let policy: LocalPolicy = policy.ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                "a machine the system made has no local policy to change",
            )
        })?;
        Ok(CommandResult::MachinePolicySet(MachinePolicySetResult {
            applied: change.applied,
            refused: change.refused,
            policy,
        }))
    }
}

#[async_trait]
impl CommandHandler for Dispatcher {
    fn tool_session_restrict(&self, update: &orch_link_contract::tool_sessions::ToolSessionUpdate) {
        for role in &self.answer_roles {
            role.tool_session_restrict(update);
        }
    }
    fn tool_session_refresh(&self) {
        for role in &self.answer_roles {
            role.tool_session_refresh();
        }
    }
    fn tool_session_update(&self, update: &orch_link_contract::tool_sessions::ToolSessionUpdate) {
        for role in &self.answer_roles {
            role.tool_session_update(update);
        }
    }
    fn tool_session_control(&self, available: bool) {
        for role in &self.answer_roles {
            role.tool_session_control(available);
        }
    }
    async fn interrupted(&self, id: orch_types::Id, command: &Command) -> Result<(), CommandError> {
        self.role_for(command)?.interrupted(id, command).await
    }
    async fn begin(&self, context: CommandContext, command: Command) -> CommandExecution {
        if command.kind() == CommandKind::FsRecord {
            self.begin_record(context, command).await
        } else {
            CommandExecution::ready(self.handle(context, command).await)
        }
    }
    fn answered(&self, id: orch_types::Id) {
        for role in &self.answer_roles {
            role.answered(id);
        }
        if let Some(requests) = &self.gateway_control {
            requests.answered(id);
        }
        if let Some((requests, _)) = &self.role_changes {
            requests.answered(id);
        }
    }

    async fn handle(
        &self,
        context: CommandContext,
        command: Command,
    ) -> Result<CommandResult, CommandError> {
        if command.kind() == CommandKind::FsRecord {
            return self.begin_record(context, command).await.complete().await;
        }
        let kind = command.kind();
        let _admission = if matches!(kind, CommandKind::TurnStart | CommandKind::TurnStartFenced) {
            Some(self.turn_admission.read().await)
        } else {
            None
        };
        match command {
            Command::MachineDirectoryList(body) => {
                self.policy.reload_from_file().map_err(|_| {
                    CommandError::new(
                        ErrorCode::Internal,
                        "Reading the machine's directory policy failed",
                    )
                })?;
                let policy = self.policy.policy().ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::PolicyRootNotAllowed,
                        "environment.roots: this machine offers no directory roots",
                    )
                })?;
                let roots = policy.environment_roots.clone();
                let cancel = context.cancel.clone();
                let due = std::time::Instant::now()
                    + Duration::from_secs(u64::from(kind.deadline_secs()));
                let result = tokio::task::spawn_blocking(move || {
                    crate::directory_listing::read(&body, &policy, &cancel, due)
                })
                .await
                .map_err(|_| {
                    CommandError::new(ErrorCode::Internal, "Reading the machine directory failed")
                })??;
                self.policy.reload_from_file().map_err(|_| {
                    CommandError::new(
                        ErrorCode::Internal,
                        "Reading the machine's directory policy failed",
                    )
                })?;
                if self
                    .policy
                    .policy()
                    .is_none_or(|policy| policy.environment_roots != roots)
                {
                    return Err(CommandError::new(
                        ErrorCode::PolicyRootNotAllowed,
                        "environment.roots: the machine's allowed roots changed while reading",
                    ));
                }
                return Ok(CommandResult::MachineDirectoryList(result));
            }
            Command::MachineProbe(_) => {
                if let Some((discovery, _)) = &self.local_harnesses {
                    discovery.refresh(&self.policy).await;
                }
                return Ok(CommandResult::MachineProbe((self.facts)()));
            }
            Command::MachineRolesSet(body) => return self.machine_roles_set(&context, body),
            Command::MachineGatewaySet(body) => {
                if context.cancel.is_cancelled() {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "The gateway change was cancelled.",
                    ));
                }
                let requests = self.gateway_control.as_ref().ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::Unsupported,
                        "This composition has no persisted gateway control.",
                    )
                })?;
                let saved = requests.request(context.id, &body.choice)?;
                return Ok(CommandResult::MachineGatewaySet(
                    orch_link_contract::commands::MachineGatewaySetResult {
                        choice: saved.choice,
                        changed: saved.changed,
                    },
                ));
            }
            Command::MachinePrograms(native) => {
                use orch_api_contract::machine_programs::MachineProgramsCommand;
                if !native.coherent() {
                    return Err(CommandError::new(
                        ErrorCode::Unsupported,
                        "the native program decision has no valid original manager attribution",
                    ));
                }
                let confirmation = native.confirmation;
                let request = native.request;
                let harness = match &request {
                    MachineProgramsCommand::ConfirmHarness { kind, path } => {
                        Some((kind.as_str(), path.as_str(), true))
                    }
                    MachineProgramsCommand::RemoveHarness { kind, path } => {
                        Some((kind.as_str(), path.as_str(), false))
                    }
                    _ => None,
                };
                if let Some((kind, path, confirm)) = harness {
                    let Some((discovery, publisher)) = &self.local_harnesses else {
                        return Err(CommandError::new(
                            ErrorCode::Unsupported,
                            "this connector cannot inspect local harnesses",
                        ));
                    };
                    let selected = self.policy.harness_selection(kind)?;
                    if !discovery.supports(kind) {
                        return Err(CommandError::new(
                            ErrorCode::Unsupported,
                            "this connector did not start with that harness kind",
                        ));
                    }
                    if confirm {
                        let program = std::path::Path::new(path);
                        if !program.is_absolute() || !program.is_file() {
                            return Err(CommandError::new(
                                ErrorCode::Unsupported,
                                "the harness program must be an absolute path to an existing file",
                            ));
                        }
                    } else if !std::path::Path::new(path).is_absolute()
                        || !selected
                            .as_deref()
                            .is_some_and(|selected| discovery.matches_selected_path(selected, path))
                    {
                        return Err(CommandError::new(
                            ErrorCode::Conflict,
                            "the harness selection changed; refresh the machine and try again",
                        ));
                    }
                    self.policy.change_harness(
                        kind,
                        selected.as_deref(),
                        confirm.then_some(path),
                        confirmation,
                    )?;
                    discovery.refresh(&self.policy).await;
                    let facts = self.report_local_programs(publisher.as_ref()).await;
                    return Ok(CommandResult::MachinePrograms(facts));
                }
                if matches!(&request, MachineProgramsCommand::Reload)
                    && self.policy.policy().is_some()
                    && let Some((discovery, publisher)) = &self.local_harnesses
                {
                    discovery.refresh(&self.policy).await;
                    let facts = self.report_local_programs(publisher.as_ref()).await;
                    if self.programs.is_none() {
                        return Ok(CommandResult::MachinePrograms(facts));
                    }
                }
                let Some((handler, services)) = &self.programs else {
                    return Err(CommandError::new(
                        ErrorCode::Unsupported,
                        "This program has no runtime program registry. Update the connector first.",
                    ));
                };
                // No admitted start races removal/reload; existing turns must finish first.
                let _program_admission = self.turn_admission.write().await;
                if !matches!(
                    &request,
                    orch_api_contract::machine_programs::MachineProgramsCommand::Test(_)
                ) && services.agents.get().is_some_and(|hosting| {
                    hosting
                        .recorded()
                        .iter()
                        .flat_map(|harness| harness.sessions.iter())
                        .any(|session| session.running_turn.is_some())
                }) {
                    return Err(CommandError::new(
                        ErrorCode::Unsupported,
                        "Wait for this machine's active turns to finish before changing its programs.",
                    ));
                }
                if let orch_api_contract::machine_programs::MachineProgramsCommand::Register(body) =
                    &request
                {
                    crate::programs::require_registration(&self.policy, body.kind)?;
                }
                handler.execute(request, services).await?;
                let facts = (self.facts)();
                if let Err(error) = services
                    .publisher
                    .publish(
                        orch_link_contract::streams::Stream::Machine,
                        orch_link_contract::events::Event::MachineFacts(facts.clone()),
                    )
                    .await
                {
                    tracing::warn!(%error,"programs changed but their machine report could not be queued");
                }
                return Ok(CommandResult::MachinePrograms(facts));
            }
            Command::MachinePolicySet(body) => {
                let reply = self.machine_policy_set(&body.values)?;
                if body
                    .values
                    .contains_key(orch_link_contract::policy::PolicyKey::ExecPlatform.as_str())
                {
                    for role in &self.answer_roles {
                        role.review_system().await?;
                    }
                }
                return Ok(reply);
            }
            Command::MachineUpdateNow(body) => {
                let _ = self.policy.reload_from_file();
                return self
                    .updates
                    .request(
                        &body,
                        self.policy
                            .policy()
                            .is_some_and(|policy| policy.update_auto),
                    )
                    .map(CommandResult::MachineUpdateNow);
            }
            _ => {}
        }
        let gated = self.policy.gate(command)?;
        let role = self.role_for(&gated.command)?;
        let reported = match &gated.command {
            Command::FsRelayPrepare(body) => body.work.as_ref(),
            Command::FsRelayCommit(body) => body.work.as_ref(),
            Command::FsWrite(body) => body.work.as_ref(),
            Command::FsSnapshot(body) => body.work.as_ref(),
            Command::FsMaterialize(body) => body.work.as_ref(),
            Command::FsMaterializeIfAbsent(body) => body.work.as_ref(),
            Command::FsDataMeasure(body) => body.work.as_ref(),
            Command::FsDataExport(body) => body.work.as_ref(),
            Command::FsDataImport(body) => body.work.as_ref(),
            Command::FsRestore(body) => body.work.as_ref(),
            Command::NodeRuntimeVolumesMeasure(body) => body.work.as_ref(),
            Command::NodeRuntimeVolumesExport(body) => body.work.as_ref(),
            Command::NodeRuntimeVolumeImport(body) => body.work.as_ref(),
            Command::NodeMountMeasure(body) => body.work.as_ref(),
            Command::NodeMountFingerprint(body) => body.work.as_ref(),
            Command::NodeMountExport(body) => body.work.as_ref(),
            Command::NodeMountImport(body) => body.work.as_ref(),
            _ => None,
        };
        let allowance = match &gated.command {
            Command::NodeServerUpdate(body) => Some(body.timeout_secs),
            _ => None,
        };
        let budget = Duration::from_secs(
            allowance
                .or_else(|| reported.map(|work| work.timeout_secs))
                .unwrap_or(u64::from(kind.deadline_secs())),
        );
        let cooperative = reported.is_some()
            || matches!(
                kind,
                CommandKind::ProviderGenerate
                    | CommandKind::FsSnapshot
                    | CommandKind::FsMaterialize
                    | CommandKind::FsMaterializeIfAbsent
                    | CommandKind::FsDataExport
                    | CommandKind::FsDataImport
                    | CommandKind::FsRestore
                    | CommandKind::FsFetch
                    | CommandKind::FsWrite
                    | CommandKind::FsCopy
                    | CommandKind::FsMeasure
                    | CommandKind::FsManifest
                    | CommandKind::NodeMountMeasure
                    | CommandKind::NodeMountExport
                    | CommandKind::NodeMountImport
                    | CommandKind::NodeRuntimeVolumesMeasure
                    | CommandKind::NodeRuntimeVolumesExport
                    | CommandKind::NodeRuntimeVolumeImport
            );
        let cancel = context.cancel.clone();
        let accepted_at = self.clock.now();
        let due = tokio::time::Instant::now()
            .checked_add(budget)
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::Unsupported,
                    "the operation's allowance is outside the native timer range",
                )
            })?;
        let work_deadline = reported
            .map(|report| {
                let span = chrono::Duration::from_std(budget).map_err(|_| {
                    CommandError::new(
                        ErrorCode::Unsupported,
                        "the operation's allowance is outside the native clock range",
                    )
                })?;
                let at = accepted_at.checked_add_signed(span).ok_or_else(|| {
                    CommandError::new(
                        ErrorCode::Unsupported,
                        "the operation's deadline is outside the native clock range",
                    )
                })?;
                Ok::<_, CommandError>(orch_api_contract::workspace::JobWorkDeadline {
                    command: context.id,
                    phase: report.phase.clone(),
                    accepted_at: orch_types::Time::new(accepted_at),
                    at: orch_types::Time::new(at),
                    seconds: budget.as_secs(),
                    policy: report.timeout_policy,
                })
            })
            .transpose()?;
        let call = Call {
            work_deadline,
            context,
            clamps: gated.clamps,
        };
        let operation = role.handle(call, gated.command);
        tokio::pin!(operation);
        if cooperative {
            return tokio::select! {
                outcome = &mut operation => outcome,
                () = tokio::time::sleep_until(due) => {
                    cancel.cancel();
                    // Keep owning the future until its helper, streams and blocking workers have joined.
                    let _ = operation.await;
                    Err(CommandError::new(ErrorCode::Timeout, "the operation exceeded its configured allowance"))
                }
            };
        }
        match tokio::time::timeout_at(due, operation).await {
            Ok(outcome) => outcome,
            Err(_) => Err(CommandError::new(
                ErrorCode::Timeout,
                format!(
                    "{kind} took longer than the {} seconds it is allowed",
                    budget.as_secs()
                ),
            )
            .retryable()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{FactsDraft, Host, machine_facts};
    use orch_config::ConnectorConfig;
    use orch_link_contract::commands::{
        Empty, ProviderRefresh, SessionClose, SessionOpen, TurnInterrupt,
    };
    use orch_link_contract::session::{MachineFactsRates, Role};
    use orch_types::{Time, new_id};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_util::sync::CancellationToken;

    struct Counting {
        kinds: &'static [CommandKind],
        seen: AtomicUsize,
        wait: Duration,
    }

    #[async_trait]
    impl RoleHandler for Counting {
        fn role(&self) -> Role {
            Role::Agent
        }
        fn takes(&self) -> &'static [CommandKind] {
            self.kinds
        }
        async fn handle(&self, _: Call, _: Command) -> Result<CommandResult, CommandError> {
            self.seen.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.wait).await;
            Ok(CommandResult::SessionClose(Empty {}))
        }
    }

    fn context() -> CommandContext {
        CommandContext {
            authority_deadline: None,
            native_members: None,
            native_revision: None,

            native_fences: None,
            native_target: None,
            id: new_id(),
            key: None,
            deadline: Time::new(chrono::Utc::now() + chrono::Duration::minutes(5)),
            cancel: CancellationToken::new(),
        }
    }

    fn dispatcher(config: ConnectorConfig) -> Dispatcher {
        let policy = Arc::new(PolicyStore::connected(config, None).unwrap());
        let facts_policy = Arc::clone(&policy);
        Dispatcher::new(
            policy,
            Arc::new(move || {
                machine_facts(
                    &Host::detect(),
                    FactsDraft::default(),
                    facts_policy.policy(),
                    MachineFactsRates::default(),
                    0,
                )
            }),
        )
    }

    #[cfg(unix)]
    #[derive(Default)]
    struct LocalProgramReports(Mutex<Vec<MachineFacts>>);

    #[cfg(unix)]
    #[async_trait]
    impl crate::events::Publisher for LocalProgramReports {
        async fn publish_from(
            &self,
            native_opening_id: Option<orch_types::Id>,
            stream: orch_link_contract::streams::Stream,
            event: orch_link_contract::events::Event,
        ) -> Result<(), orch_link::streams::PublishError> {
            assert!(
                native_opening_id.is_none(),
                "machine facts have no physical session"
            );
            assert_eq!(stream, orch_link_contract::streams::Stream::Machine);
            let orch_link_contract::events::Event::MachineFacts(facts) = event else {
                panic!("only local program facts are published")
            };
            self.0.lock().unwrap().push(facts);
            Ok(())
        }
    }

    #[cfg(unix)]
    fn local_program(path: &std::path::Path, text: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, format!("#!/bin/sh\n{text}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    fn program_dispatcher(directory: &std::path::Path) -> (Dispatcher, Arc<LocalProgramReports>) {
        programs_dispatcher(directory, &["codex"])
    }

    #[cfg(unix)]
    fn programs_dispatcher(
        directory: &std::path::Path,
        kinds: &[&'static str],
    ) -> (Dispatcher, Arc<LocalProgramReports>) {
        use crate::harnesses::{AdapterRegistration, AdapterRegistry, HomeRule};
        let mut registry = AdapterRegistry::new();
        for kind in kinds {
            registry
                .register(AdapterRegistration {
                    kind: *kind,
                    adapter_version: 1,
                    builtin: true,
                    needs_binary: *kind != "fake",
                    home_rule: HomeRule::Connector,
                    factory: Arc::new(|_| {
                        panic!("program discovery never starts a harness adapter")
                    }),
                })
                .unwrap();
        }
        let discovery = Arc::new(crate::local_harnesses::Discovery::with_test_search(
            Arc::new(registry),
            directory.to_owned(),
        ));
        let mut config = ConnectorConfig::default();
        config.connector.roles = vec!["environment".into()];
        let mut dispatcher = dispatcher(config);
        let prior = Arc::clone(&dispatcher.facts);
        let found = Arc::clone(&discovery);
        dispatcher.facts = Arc::new(move || {
            let mut facts = prior();
            found.apply(&mut facts);
            facts
        });
        let reports = Arc::new(LocalProgramReports::default());
        dispatcher.local_harnesses(discovery, reports.clone());
        (dispatcher, reports)
    }

    #[cfg(unix)]
    fn program_confirmation() -> orch_agent_programs::confirmation::Confirmation {
        orch_agent_programs::confirmation::Confirmation {
            by: orch_types::new_id(),
            at: "2026-10-31T00:00:00.000Z".parse().unwrap(),
        }
    }

    #[cfg(unix)]
    fn harness_action(action: &str, kind: &str, path: &std::path::Path) -> Command {
        Command::MachinePrograms(orch_link_contract::commands::MachineProgramsRequest {
            request: serde_json::from_value(serde_json::json!({
                "action":action,"kind":kind,"path":path.to_string_lossy()
            }))
            .unwrap(),
            confirmation: (action == "confirm_harness").then(program_confirmation),
            interface_timeout_secs: (action == "check_harness").then_some(30),
        })
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_selected_program_is_checked_on_its_machine_and_what_it_lacks_is_reported() {
        use orch_api_contract::admin::HarnessBuildStatus;
        let directory = tempfile::tempdir().unwrap();
        let claude = directory.path().join("claude");
        let needs: Vec<String> = orch_agent_programs::Programs::builtin()
            .program("claude-code")
            .and_then(|program| program.checks.interface.as_ref())
            .unwrap()
            .needs
            .iter()
            .map(|need| need.name.clone())
            .collect();
        // Claude Code as installed: its version, and its help with every flag its adapter needs but those left out.
        let installed = |without: &[&str]| {
            let flags: Vec<&str> = needs
                .iter()
                .map(String::as_str)
                .filter(|flag| !without.contains(flag))
                .collect();
            format!(
                "if [ \"$1\" = --version ]; then echo '2.1.322 (Claude Code)'; else printf '  %s\\n' {}; fi",
                flags.join(" ")
            )
        };
        local_program(&claude, &installed(&["--fork-session"]));
        let (dispatcher, reports) = programs_dispatcher(directory.path(), &["claude-code", "fake"]);
        dispatcher
            .policy
            .delegate(&["agent.harnesses".into()])
            .unwrap();
        for (kind, path) in [
            ("claude-code", claude.display().to_string()),
            ("fake", "/usr/local/bin/orch-connector".to_owned()),
        ] {
            dispatcher
                .policy
                .change_harness(kind, None, Some(&path), Some(program_confirmation()))
                .unwrap();
        }
        let checked = |kind: &str, path: &std::path::Path| {
            let command = harness_action("check_harness", kind, path);
            let dispatcher = &dispatcher;
            async move {
                let result = tokio::time::timeout(
                    Duration::from_secs(30),
                    dispatcher.handle(context(), command),
                )
                .await
                .unwrap()
                .unwrap();
                let CommandResult::MachinePrograms(facts) = result else {
                    panic!("program acknowledgement")
                };
                facts
            }
        };
        let facts = checked("claude-code", &claude).await;
        let check = facts
            .program_checks
            .iter()
            .find(|check| check.kind == "claude-code")
            .unwrap();
        assert_eq!(check.status, HarnessBuildStatus::Checked);
        assert_eq!(check.version.as_deref(), Some("2.1.322"));
        assert_eq!(check.missing, ["--fork-session"]);
        assert_eq!(check.off, ["fork_native"]);
        assert_eq!(check.path, claude.display().to_string());
        assert!(!check.builtin);
        assert!(check.sha256.as_ref().is_some_and(|sha| sha.len() == 64));
        assert_eq!(
            reports.0.lock().unwrap().len(),
            1,
            "the machine reports what it found"
        );
        // A flag the adapter needs as a whole: the program fails its check, with the flag named.
        local_program(&claude, &installed(&["--input-format"]));
        let facts = checked("claude-code", &claude).await;
        let check = facts
            .program_checks
            .iter()
            .find(|check| check.kind == "claude-code")
            .unwrap();
        assert_eq!(check.status, HarnessBuildStatus::Failed);
        assert_eq!(check.missing, ["--input-format"]);
        // An adapter built into the connector has no program of its own to run.
        let facts = checked(
            "fake",
            std::path::Path::new("/usr/local/bin/orch-connector"),
        )
        .await;
        let example = facts
            .program_checks
            .iter()
            .find(|check| check.kind == "fake")
            .unwrap();
        assert!(example.builtin);
        assert_eq!(example.status, HarnessBuildStatus::Checked);
        // Only the selected program is run.
        let other = directory.path().join("other-claude");
        local_program(&other, &installed(&[]));
        let stale = dispatcher
            .handle(
                context(),
                harness_action("check_harness", "claude-code", &other),
            )
            .await
            .unwrap_err();
        assert_eq!(stale.code, "conflict");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_kept_harness_key_refuses_before_probing_or_writing_policy() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("candidate-was-probed");
        let candidate = directory.path().join("custom-codex");
        let quoted = format!("'{}'", marker.display().to_string().replace('\'', "'\\''"));
        local_program(
            &candidate,
            &format!("printf probed > {quoted}; printf '0.165.0\\n'"),
        );
        let (dispatcher, reports) = program_dispatcher(directory.path());
        let before = dispatcher.policy.policy();
        let error = dispatcher
            .handle(
                context(),
                harness_action("confirm_harness", "codex", &candidate),
            )
            .await
            .unwrap_err();
        assert_eq!(error.details.unwrap()["policy_key"], "agent.harnesses");
        assert_eq!(dispatcher.policy.policy(), before);
        assert!(!marker.exists());
        assert!(reports.0.lock().unwrap().is_empty());
        dispatcher
            .policy
            .delegate(&["agent.harnesses".into()])
            .unwrap();
        let delegated = dispatcher.policy.policy();
        for (kind, path) in [
            ("unknown-harness", candidate.clone()),
            ("codex", std::path::PathBuf::from("relative-program")),
            ("codex", directory.path().join("missing-program")),
        ] {
            assert!(
                dispatcher
                    .handle(context(), harness_action("confirm_harness", kind, &path))
                    .await
                    .is_err()
            );
        }
        assert_eq!(dispatcher.policy.policy(), delegated);
        assert!(!marker.exists());
        assert!(reports.0.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_harness_replacement_refresh_and_bare_remove_do_not_wait_for_external_turn_admission()
     {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("codex");
        let replacement = directory.path().join("custom-codex");
        local_program(&original, "printf '0.165.0\\n'");
        local_program(&replacement, "printf '0.166.0\\n'");
        let (dispatcher, reports) = program_dispatcher(directory.path());
        dispatcher
            .policy
            .delegate(&["agent.harnesses".into()])
            .unwrap();
        dispatcher
            .policy
            .change_harness(
                "claude-code",
                None,
                Some("/kept/claude"),
                Some(program_confirmation()),
            )
            .unwrap();
        // Reuse the existing dispatch tests' observation headroom, while the real admission writer stays held.
        let admission = Arc::clone(&dispatcher.turn_admission).write_owned().await;
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            dispatcher.handle(
                context(),
                harness_action("confirm_harness", "codex", &original),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let CommandResult::MachinePrograms(facts) = result else {
            panic!("program acknowledgement")
        };
        assert_eq!(facts.harnesses[0].version, "0.165.0");
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            dispatcher.handle(
                context(),
                harness_action("confirm_harness", "codex", &replacement),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let CommandResult::MachinePrograms(facts) = result else {
            panic!("program acknowledgement")
        };
        assert_eq!(facts.harnesses[0].path, replacement.display().to_string());
        assert_eq!(facts.harnesses[0].version, "0.166.0");
        let stale = dispatcher
            .handle(
                context(),
                harness_action("remove_harness", "codex", &original),
            )
            .await
            .unwrap_err();
        assert_eq!(stale.code, "conflict");
        assert_eq!(
            dispatcher.policy.harness("codex").unwrap().path,
            replacement.display().to_string()
        );
        local_program(&replacement, "printf '0.167.0\\n'");
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            dispatcher.handle(
                context(),
                Command::MachinePrograms(orch_link_contract::commands::MachineProgramsRequest {
                    request: orch_api_contract::machine_programs::MachineProgramsCommand::Reload,
                    confirmation: None,
                    interface_timeout_secs: None,
                }),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let CommandResult::MachinePrograms(facts) = result else {
            panic!("program acknowledgement")
        };
        assert_eq!(facts.harnesses[0].version, "0.167.0");
        dispatcher
            .policy
            .change_harness(
                "codex",
                replacement.to_str(),
                Some("codex"),
                Some(program_confirmation()),
            )
            .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            dispatcher.handle(
                context(),
                harness_action("remove_harness", "codex", &original),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        let CommandResult::MachinePrograms(facts) = result else {
            panic!("program acknowledgement")
        };
        assert!(facts.harnesses.is_empty());
        assert_eq!(
            facts.discovered_harnesses.unwrap()[0].path,
            original.display().to_string()
        );
        assert!(dispatcher.policy.harness("codex").is_none());
        assert_eq!(
            dispatcher.policy.harness("claude-code").unwrap().path,
            "/kept/claude"
        );
        assert_eq!(reports.0.lock().unwrap().len(), 4);
        drop(admission);
    }

    fn close() -> Command {
        Command::SessionClose(SessionClose {
            thread_id: new_id(),
        })
    }

    struct LocalCapture {
        gate: (Mutex<bool>, std::sync::Condvar),
        entered: tokio::sync::Semaphore,
        joined: std::sync::atomic::AtomicBool,
        saw_cancel: std::sync::atomic::AtomicBool,
        hold_dropped: std::sync::atomic::AtomicBool,
        panic: bool,
    }

    impl LocalCapture {
        fn new(panic: bool) -> Self {
            Self {
                gate: (Mutex::new(true), std::sync::Condvar::new()),
                entered: tokio::sync::Semaphore::new(0),
                joined: std::sync::atomic::AtomicBool::new(false),
                saw_cancel: std::sync::atomic::AtomicBool::new(false),
                hold_dropped: std::sync::atomic::AtomicBool::new(false),
                panic,
            }
        }
        fn release(&self) {
            *self
                .gate
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
            self.gate.1.notify_all();
        }
    }

    struct ReleaseCapture(Arc<LocalCapture>);
    impl Drop for ReleaseCapture {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    struct JoinedCapture(Arc<LocalCapture>);
    impl Drop for JoinedCapture {
        fn drop(&mut self) {
            self.0.joined.store(true, Ordering::SeqCst);
        }
    }
    struct CaptureHold(Arc<LocalCapture>);
    impl Drop for CaptureHold {
        fn drop(&mut self) {
            self.0.hold_dropped.store(true, Ordering::SeqCst);
        }
    }
    struct CaptureRole(Arc<LocalCapture>);

    #[async_trait]
    impl RoleHandler for CaptureRole {
        fn role(&self) -> Role {
            Role::Environment
        }
        fn takes(&self) -> &'static [CommandKind] {
            &[CommandKind::FsRecord]
        }
        async fn handle(&self, _: Call, _: Command) -> Result<CommandResult, CommandError> {
            Err(CommandError::new(
                ErrorCode::Unsupported,
                "this test role uses a captured phase",
            ))
        }
        async fn begin(&self, call: Call, _: Command) -> CommandExecution {
            let capture = Arc::clone(&self.0);
            let cancel = call.context.cancel;
            let local = tokio::task::spawn_blocking(move || {
                let _joined = JoinedCapture(Arc::clone(&capture));
                capture.entered.add_permits(1);
                let mut held = capture
                    .gate
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while *held {
                    held = capture
                        .gate
                        .1
                        .wait(held)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                capture
                    .saw_cancel
                    .store(cancel.is_cancelled(), Ordering::SeqCst);
                assert!(!capture.panic, "synthetic local capture panic");
            });
            if local.await.is_err() {
                return CommandExecution::ready(Err(CommandError::new(
                    ErrorCode::Internal,
                    "local capture failed",
                )));
            }
            CommandExecution::Captured {
                finish: Box::pin(std::future::pending()),
                hold: Box::new(CaptureHold(Arc::clone(&self.0))),
            }
        }
    }

    fn record() -> Command {
        Command::FsRecord(orch_link_contract::source::FsRecord {
            access: orch_link_contract::commands::FsAccess::roots(),
            environment_id: new_id(),
            snapshot_id: new_id(),
            directory: String::new(),
            wait_for_indexing: true,
            kind: orch_link_contract::source::RecordKind::Turn,
            record_days: 1,
            max_bytes: 1024,
        })
    }

    #[tokio::test]
    async fn recording_cancellation_and_timeout_signal_then_join_local_capture_before_returning()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for timeout in [false, true] {
            let mut dispatcher = dispatcher(ConnectorConfig::default());
            let local = Arc::new(LocalCapture::new(false));
            let _release = ReleaseCapture(Arc::clone(&local));
            let role: Arc<dyn RoleHandler> = Arc::new(CaptureRole(Arc::clone(&local)));
            dispatcher.add(&role)?;
            let context = context();
            let cancel = context.cancel.clone();
            let task =
                tokio::spawn(
                    async move { dispatcher.begin(context, record()).await.complete().await },
                );
            tokio::time::timeout(Duration::from_secs(30), local.entered.acquire())
                .await??
                .forget();
            if timeout {
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(u64::from(
                    CommandKind::FsRecord.deadline_secs(),
                )))
                .await;
                // Advancing does not guarantee the timer driver has processed the deadline.
                // Keep the watchdog live while the blocking capture waits for our release.
                tokio::time::resume();
                tokio::time::timeout(Duration::from_secs(30), cancel.cancelled()).await?;
            } else {
                cancel.cancel();
            }
            assert!(!local.joined.load(Ordering::SeqCst));
            assert!(
                !task.is_finished(),
                "the command cannot release capture while its local closure runs"
            );
            local.release();
            let outcome = tokio::time::timeout(Duration::from_secs(30), task).await??;
            let Err(error) = outcome else {
                return Err("cancelled capture succeeded".into());
            };
            assert_eq!(error.code, if timeout { "timeout" } else { "conflict" });
            assert!(local.joined.load(Ordering::SeqCst));
            assert!(local.saw_cancel.load(Ordering::SeqCst));
            assert!(local.hold_dropped.load(Ordering::SeqCst));
        }
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_at_capture_completion_returns_the_acquired_hold_with_the_failure()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut router = dispatcher(ConnectorConfig::default());
        let local = Arc::new(LocalCapture::new(false));
        let _release = ReleaseCapture(Arc::clone(&local));
        let role: Arc<dyn RoleHandler> = Arc::new(CaptureRole(Arc::clone(&local)));
        router.add(&role)?;
        let context = context();
        let cancel = context.cancel.clone();
        let task = tokio::spawn(async move { router.begin(context, record()).await });
        tokio::time::timeout(Duration::from_secs(30), local.entered.acquire())
            .await??
            .forget();
        cancel.cancel();
        local.release();
        let execution = tokio::time::timeout(Duration::from_secs(30), task).await??;
        assert!(local.joined.load(Ordering::SeqCst));
        assert!(matches!(&execution, CommandExecution::Captured { .. }));
        assert!(
            !local.hold_dropped.load(Ordering::SeqCst),
            "terminal persistence still owns the acquired lease"
        );
        let Err(error) = execution.complete().await else {
            return Err("cancelled capture succeeded".into());
        };
        assert_eq!(error.code, "conflict");
        assert!(local.hold_dropped.load(Ordering::SeqCst));
        Ok(())
    }

    #[tokio::test]
    async fn publication_uses_the_remaining_capture_budget_and_a_local_panic_is_joined()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut router = dispatcher(ConnectorConfig::default());
        let local = Arc::new(LocalCapture::new(false));
        let _release = ReleaseCapture(Arc::clone(&local));
        let role: Arc<dyn RoleHandler> = Arc::new(CaptureRole(Arc::clone(&local)));
        router.add(&role)?;
        let task = tokio::spawn(async move { router.begin(context(), record()).await });
        tokio::time::timeout(Duration::from_secs(30), local.entered.acquire())
            .await??
            .forget();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(
            u64::from(CommandKind::FsRecord.deadline_secs()) - 60,
        ))
        .await;
        tokio::time::resume();
        local.release();
        let execution = tokio::time::timeout(Duration::from_secs(30), task).await??;
        assert!(matches!(&execution, CommandExecution::Captured { .. }));
        let finish = tokio::spawn(execution.complete());
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::time::resume();
        let outcome = tokio::time::timeout(Duration::from_secs(30), finish).await??;
        let Err(error) = outcome else {
            return Err("publication reset its six-hour budget".into());
        };
        assert_eq!(error.code, "timeout");
        assert!(local.joined.load(Ordering::SeqCst));
        let mut router = dispatcher(ConnectorConfig::default());
        let panicking = Arc::new(LocalCapture::new(true));
        let _release = ReleaseCapture(Arc::clone(&panicking));
        let role: Arc<dyn RoleHandler> = Arc::new(CaptureRole(Arc::clone(&panicking)));
        router.add(&role)?;
        let task =
            tokio::spawn(async move { router.begin(context(), record()).await.complete().await });
        tokio::time::timeout(Duration::from_secs(30), panicking.entered.acquire())
            .await??
            .forget();
        panicking.release();
        let outcome = tokio::time::timeout(Duration::from_secs(30), task).await??;
        assert!(outcome.is_err());
        assert!(panicking.joined.load(Ordering::SeqCst));
        Ok(())
    }

    #[tokio::test]
    async fn a_command_goes_to_the_role_that_takes_its_kind() {
        let mut dispatcher = dispatcher(ConnectorConfig::default());
        let role = Arc::new(Counting {
            kinds: &[CommandKind::SessionClose],
            seen: AtomicUsize::new(0),
            wait: Duration::ZERO,
        });
        let handler: Arc<dyn RoleHandler> = role.clone();
        dispatcher.add(&handler).unwrap();
        let result = dispatcher.handle(context(), close()).await.unwrap();
        assert_eq!(result.kind(), CommandKind::SessionClose);
        assert_eq!(role.seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_kind_no_role_takes_is_answered_with_the_actual_missing_role() {
        let dispatcher = dispatcher(ConnectorConfig::default());
        let error = dispatcher.handle(context(), close()).await.unwrap_err();
        assert_eq!(error.code, "unsupported");
        assert_eq!(
            error.detail,
            "this connector holds no role that takes session.close"
        );
        let interrupt = Command::TurnInterrupt(TurnInterrupt {
            thread_id: new_id(),
            turn_id: new_id(),
        });
        let error = dispatcher.handle(context(), interrupt).await.unwrap_err();
        assert_eq!(error.code, "unsupported");
        assert_eq!(
            error.detail,
            "this connector holds no role that takes turn.interrupt"
        );
    }

    /// A role that claims every provider read it takes, and counts what it answered.
    struct Claiming {
        role: Role,
        seen: AtomicUsize,
    }

    #[async_trait]
    impl RoleHandler for Claiming {
        fn role(&self) -> Role {
            self.role
        }
        fn takes(&self) -> &'static [CommandKind] {
            &[CommandKind::ProviderRefresh]
        }
        async fn handle(&self, _: Call, _: Command) -> Result<CommandResult, CommandError> {
            self.seen.fetch_add(1, Ordering::SeqCst);
            Err(CommandError::new(ErrorCode::NotFound, "counted"))
        }
    }

    #[tokio::test]
    async fn where_both_roles_claim_a_providers_read_the_role_that_keeps_its_key_answers_it() {
        let mut both = dispatcher(ConnectorConfig::default());
        let mut alone = dispatcher(ConnectorConfig::default());
        let agent = Arc::new(Claiming {
            role: Role::Agent,
            seen: AtomicUsize::new(0),
        });
        let provider = Arc::new(Claiming {
            role: Role::Provider,
            seen: AtomicUsize::new(0),
        });
        // The agent role is registered first, as a connector of both roles registers it.
        let (first, second): (Arc<dyn RoleHandler>, Arc<dyn RoleHandler>) =
            (agent.clone(), provider.clone());
        both.add(&first).unwrap();
        both.add(&second).unwrap();
        let refresh = || {
            Command::ProviderRefresh(ProviderRefresh {
                provider_id: new_id(),
                retry: None,
                renew: None,
                facts: None,
            })
        };
        let _ = both.handle(context(), refresh()).await;
        assert_eq!(
            (
                agent.seen.load(Ordering::SeqCst),
                provider.seen.load(Ordering::SeqCst)
            ),
            (0, 1),
            "a harness running on a key provider never reads it as a sign-in"
        );
        // A machine where only the harness holds the provider answers through it.
        alone.add(&first).unwrap();
        let _ = alone.handle(context(), refresh()).await;
        assert_eq!(agent.seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn two_roles_cannot_take_one_kind() {
        let mut dispatcher = dispatcher(ConnectorConfig::default());
        let make = || -> Arc<dyn RoleHandler> {
            Arc::new(Counting {
                kinds: &[CommandKind::SessionClose],
                seen: AtomicUsize::new(0),
                wait: Duration::ZERO,
            })
        };
        dispatcher.add(&make()).unwrap();
        assert!(dispatcher.add(&make()).is_err());
    }

    #[tokio::test]
    async fn a_command_the_policy_forbids_is_refused_before_the_role_sees_it() {
        let mut dispatcher = dispatcher(ConnectorConfig::default());
        let role = Arc::new(Counting {
            kinds: &[CommandKind::SessionOpen],
            seen: AtomicUsize::new(0),
            wait: Duration::ZERO,
        });
        let handler: Arc<dyn RoleHandler> = role.clone();
        dispatcher.add(&handler).unwrap();
        let body: SessionOpen = serde_json::from_value(serde_json::json!({
            "agent_id": new_id(), "thread_id": new_id(), "segment_id": new_id(),
            "mode": "new", "native_session_ref": null, "archive": null, "history": null,
            "upto_turn_ordinal": null, "workspace_root": "/etc", "instruction_root_marker": false,
            "directory": "/etc", "cwd": "/etc", "mounts": [],
            "model": "m", "effort": null, "params": {}, "approval_mode": "on_request", "web_action_wait_secs": 120,
            "sandbox_mode": "workspace", "instructions": "", "memory": null,
            "arrangement": "together",
            "provider": { "path": "native", "provider_id": new_id() },
            "exec": { "executor": null, "environment_id": new_id(), "generation": 1 },
            "mcp": [], "orch_tools": [], "output_capture_bytes":67108864, "raw_events": false
        }))
        .unwrap();
        let error = dispatcher
            .handle(context(), Command::SessionOpen(body))
            .await
            .unwrap_err();
        assert_eq!(error.code, "policy.root_not_allowed");
        assert_eq!(role.seen.load(Ordering::SeqCst), 0, "the role never saw it");
    }

    struct QuietFileWork {
        observed: Mutex<Vec<serde_json::Value>>,
        joined: AtomicUsize,
    }
    #[async_trait]
    impl RoleHandler for QuietFileWork {
        fn role(&self) -> Role {
            Role::Node
        }
        fn takes(&self) -> &'static [CommandKind] {
            &[CommandKind::NodeMountMeasure]
        }
        async fn handle(&self, call: Call, _: Command) -> Result<CommandResult, CommandError> {
            self.observed
                .lock()
                .unwrap()
                .push(serde_json::to_value(call.work_deadline).unwrap());
            call.context.cancel.cancelled().await;
            self.joined.fetch_add(1, Ordering::SeqCst);
            Err(CommandError::new(
                ErrorCode::Conflict,
                "stopped after cleanup",
            ))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_quiet_filesystem_command_reports_the_same_captured_budget_that_cancels_and_joins_it()
    {
        use orch_link_contract::commands::{MountData, NodeMountMeasure, WorkReport};
        let mut config = ConnectorConfig::default();
        config.connector.roles = vec!["node".into()];
        let mut dispatcher = dispatcher(config);
        let at: chrono::DateTime<chrono::Utc> = "2026-11-15T10:00:00Z".parse().unwrap();
        let clock = Arc::new(orch_types::ManualClock::new(at));
        dispatcher.clock(clock.clone());
        let work = Arc::new(QuietFileWork {
            observed: Mutex::new(Vec::new()),
            joined: AtomicUsize::new(0),
        });
        let role: Arc<dyn RoleHandler> = work.clone();
        dispatcher.add(&role).unwrap();
        let mount = new_id();
        let error = dispatcher
            .handle(
                context(),
                Command::NodeMountMeasure(NodeMountMeasure {
                    driver: "test".into(),
                    mount_id: mount,
                    data: MountData::Volume {
                        name: format!("orch-mount-{mount}"),
                        storage: None,
                    },
                    work: Some(WorkReport {
                        files_total: None,
                        bytes_total: None,
                        job_id: new_id(),
                        phase: "measure".into(),
                        timeout_secs: 1,
                        timeout_policy: None,
                    }),
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "timeout");
        assert_eq!(
            work.joined.load(Ordering::SeqCst),
            1,
            "the captured deadline still waits for native cleanup"
        );
        let captured = work.observed.lock().unwrap()[0].clone();
        assert_eq!(
            captured["accepted_at"],
            serde_json::to_value(Time::new(at)).unwrap()
        );
        assert_eq!(
            captured["at"],
            serde_json::to_value(Time::new(at + chrono::Duration::seconds(1))).unwrap()
        );
        assert_eq!(captured["seconds"], 1);
        clock.advance(chrono::Duration::hours(1));
        assert_eq!(
            work.observed.lock().unwrap()[0],
            captured,
            "a later clock or setting cannot rewrite the accepted budget"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_role_that_hangs_fails_its_command_at_the_budget_of_its_kind() {
        let mut dispatcher = dispatcher(ConnectorConfig::default());
        let role: Arc<dyn RoleHandler> = Arc::new(Counting {
            kinds: &[CommandKind::SessionClose],
            seen: AtomicUsize::new(0),
            wait: Duration::from_secs(3_600),
        });
        dispatcher.add(&role).unwrap();
        let error = dispatcher.handle(context(), close()).await.unwrap_err();
        assert_eq!(error.code, "timeout");
        assert!(error.retryable);
    }

    #[tokio::test]
    async fn the_machine_answers_a_probe_with_its_facts_and_takes_a_change_of_a_key_that_was_handed_over()
     {
        let mut config = ConnectorConfig::default();
        config.policy.delegated = vec!["environment.roots".to_owned()];
        let dispatcher = dispatcher(config);
        let probe = dispatcher
            .handle(
                context(),
                Command::MachineProbe(orch_link_contract::commands::MachineProbe {}),
            )
            .await
            .unwrap();
        let CommandResult::MachineProbe(facts) = probe else {
            panic!()
        };
        assert!(facts.policy.is_some());
        let change = dispatcher
            .handle(
                context(),
                Command::MachinePolicySet(orch_link_contract::commands::MachinePolicySet {
                    values: [
                        ("environment.roots".to_owned(), serde_json::json!(["/srv"])),
                        ("environment.terminal".to_owned(), serde_json::json!(true)),
                    ]
                    .into(),
                }),
            )
            .await
            .unwrap();
        let CommandResult::MachinePolicySet(result) = change else {
            panic!()
        };
        assert_eq!(result.applied, ["environment.roots"]);
        assert_eq!(result.refused, ["environment.terminal"]);
        assert_eq!(result.policy.environment_roots, ["/srv"]);
    }

    struct ServerPreparation {
        wait: Duration,
    }
    #[async_trait]
    impl RoleHandler for ServerPreparation {
        fn role(&self) -> Role {
            Role::Node
        }
        fn takes(&self) -> &'static [CommandKind] {
            &[CommandKind::NodeServerUpdate]
        }
        async fn handle(&self, _: Call, command: Command) -> Result<CommandResult, CommandError> {
            let Command::NodeServerUpdate(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "this fixture prepares only the server updater",
                ));
            };
            tokio::time::sleep(self.wait).await;
            Ok(CommandResult::NodeServerUpdate(
                orch_api_contract::admin::ServerUpdateObservation {
                    stage: orch_api_contract::admin::ServerUpdateStage::Queued,
                    reason: None,
                    from_version: None,
                    to_version: body.release,
                },
            ))
        }
    }
    #[tokio::test]
    async fn a_server_update_uses_its_accepted_allowance_after_the_command_default_has_elapsed() {
        let nominal = u64::from(CommandKind::NodeServerUpdate.deadline_secs());
        let mut dispatcher = dispatcher(ConnectorConfig::default());
        let role: Arc<dyn RoleHandler> = Arc::new(ServerPreparation {
            wait: Duration::from_secs(nominal + 1),
        });
        dispatcher.add(&role).unwrap();
        let result = dispatcher
            .handle(
                context(),
                Command::NodeServerUpdate(orch_link_contract::commands::NodeServerUpdate {
                    job_id: new_id(),
                    release: "1.2.3".into(),
                    timeout_secs: nominal * 2,
                }),
            )
            .await
            .unwrap();
        assert!(matches!(result, CommandResult::NodeServerUpdate(_)));
    }
}
