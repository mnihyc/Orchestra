//! The harness of each provider and profile, as one process or one for each conversation, and session supervision.
//!
//! One harness process serves every thread bound to its provider and agent
//! profile on the machine where the arrangement places it. This module knows
//! which machine that is, whether the process is online, how to start it
//! (`agent.start`, which is idempotent on the machine), and what the process
//! says about itself (`harness.status`, `harness.descriptor`). It also keeps
//! the hint of which sessions this process itself saw opened, so that a turn
//! does not open a session that is open; a hint that is missing costs one
//! `session.open` of mode `resume`, which a session already open answers
//! with itself.

use crate::commands::{Outcome, Outgoing};
use crate::engine::{Engine, lock};
use crate::error::EngineError;
use crate::unit::Unit;
use orch_agent_contract::descriptor::AgentDescriptor;
use orch_db::queries::resources::binding as resources_binding;
use orch_db::queries::resources::thread_engine::{self as resources, AgentRow, ProviderRow};
use orch_link_contract::commands::{
    AgentDiscoverSessions, AgentDiscoverSessionsResult, AgentStart, AgentStartResult, AgentStop,
    CommandError, ErrorCode, SerialKey, UnitAfterUp, UnitAfterUpResult,
};
use orch_link_contract::events::HarnessStatus;
use orch_types::{Id, Json};
use std::collections::HashMap;

/// The `harnesses.status` of a process that is up.
pub const ONLINE: &str = "online";
/// The `harnesses.status` of a process that is not.
pub const OFFLINE: &str = "offline";

/// A harness process: a provider and an agent profile on a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HarnessKey {
    /// The provider it serves.
    pub provider: Id,
    /// The profile it runs.
    pub agent: Id,
    /// The machine it runs on.
    pub machine: Id,
}

/// The effective external MCP definitions, authority and native reports, without session tickets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpRevision(orch_types::Hash);

impl McpRevision {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self(orch_types::Hash::of(bytes))
    }
}

/// The ordered Orch tool catalogue, including descriptions and policy-narrowed schemas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrchToolsRevision(orch_types::Hash);

impl OrchToolsRevision {
    pub(crate) fn of(catalogue: &Json) -> Self {
        Self(orch_types::Hash::of(catalogue.to_string().as_bytes()))
    }
}

/// Session-only values which a turn cannot replace on an already open native conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfiguration {
    /// The resolved absolute start directory.
    pub cwd: String,
    /// The resolved notes directory/keeper, or disabled native and agent notes.
    pub memory: Option<orch_link_contract::commands::SessionOpenMemory>,
}

impl From<&orch_link_contract::commands::SessionOpen> for SessionConfiguration {
    fn from(open: &orch_link_contract::commands::SessionOpen) -> Self {
        Self {
            cwd: open.cwd.clone(),
            memory: open.memory.clone(),
        }
    }
}

/// The sessions this process saw opened, and in which harness process.
#[derive(Debug, Default)]
pub struct OpenSessions {
    by_segment: HashMap<Id, (Id, HarnessKey)>,
    requested_instructions: HashMap<Id, orch_types::Hash>,
    requested_mcp: HashMap<Id, McpRevision>,
    requested_orch_tools: HashMap<Id, OrchToolsRevision>,
    requested_subagent_limits: HashMap<Id, orch_agent_contract::session::SubagentLimits>,
    accepted_configuration: HashMap<Id, SessionConfiguration>,
    requested_skills: HashMap<Id, orch_types::Hash>,
}

impl OpenSessions {
    /// Whether the session of a segment is known to be open in `key`.
    #[must_use]
    pub fn is_open(&self, segment: Id, key: HarnessKey) -> bool {
        self.by_segment.get(&segment).map(|(_, held)| held) == Some(&key)
    }

    /// The session of a segment of `thread` was opened in `key`.
    pub fn opened(&mut self, segment: Id, thread: Id, key: HarnessKey) {
        self.by_segment.insert(segment, (thread, key));
    }

    /// Whether the effective cwd/notes plan differs from the last acknowledged opening.
    #[must_use]
    pub fn configuration_changed(&self, segment: Id, configuration: &SessionConfiguration) -> bool {
        self.accepted_configuration.get(&segment) != Some(configuration)
    }

    /// Records only an actual successful SessionOpen body, never a requested or refused change.
    pub fn configuration_accepted(&mut self, segment: Id, configuration: SessionConfiguration) {
        self.accepted_configuration.insert(segment, configuration);
    }

    /// Whether current text differs from the last accepted session-setup request.
    #[must_use]
    pub fn instructions_changed(&self, segment: Id, text: &str) -> bool {
        self.requested_instructions.get(&segment) != Some(&orch_types::Hash::of(text.as_bytes()))
    }

    /// Remembers the accepted request; the adapter separately reports any native retention.
    pub fn instructions_requested(&mut self, segment: Id, text: &str) {
        self.requested_instructions
            .insert(segment, orch_types::Hash::of(text.as_bytes()));
    }

    /// A place's limits take effect at a session boundary, before the next accepted turn.
    #[must_use]
    pub fn subagent_limits_changed(
        &self,
        segment: Id,
        limits: orch_agent_contract::session::SubagentLimits,
    ) -> bool {
        self.requested_subagent_limits.get(&segment) != Some(&limits)
    }

    /// Records what the accepted native opening was told to enforce.
    pub fn subagent_limits_requested(
        &mut self,
        segment: Id,
        limits: orch_agent_contract::session::SubagentLimits,
    ) {
        self.requested_subagent_limits.insert(segment, limits);
    }

    /// Whether the effective MCP set differs from the last accepted session request.
    #[must_use]
    pub fn mcp_changed(&self, segment: Id, revision: McpRevision) -> bool {
        self.requested_mcp.get(&segment) != Some(&revision)
    }

    /// Records external definitions accepted by an opening or successful live reload. A reload
    /// does not acknowledge a new Orch catalogue, whose independent revision stays unchanged.
    pub fn mcp_requested(&mut self, segment: Id, revision: McpRevision) {
        self.requested_mcp.insert(segment, revision);
    }

    /// Whether the ordered Orch catalogue differs from the last accepted session opening.
    #[must_use]
    pub fn orch_tools_changed(&self, segment: Id, revision: OrchToolsRevision) -> bool {
        self.requested_orch_tools.get(&segment) != Some(&revision)
    }

    /// A session opening accepted this exact Orch catalogue; an external reload cannot set it.
    pub fn orch_tools_requested(&mut self, segment: Id, revision: OrchToolsRevision) {
        self.requested_orch_tools.insert(segment, revision);
    }

    /// Whether the skill versions a thread is given differ from those its open session was opened with: a program
    /// takes its skills when its session opens, so a change reaches the thread by opening it anew.
    #[must_use]
    pub fn skills_changed(&self, segment: Id, fingerprint: &orch_types::Hash) -> bool {
        self.requested_skills.get(&segment) != Some(fingerprint)
    }

    /// Records the skill versions an accepted opening was given.
    pub fn skills_requested(&mut self, segment: Id, fingerprint: orch_types::Hash) {
        self.requested_skills.insert(segment, fingerprint);
    }

    /// No session of the thread is known to be open.
    pub fn forget_thread_sessions(&mut self, thread: Id) {
        self.by_segment.retain(|_, (held_by, _)| *held_by != thread);
        self.requested_instructions
            .retain(|id, _| self.by_segment.contains_key(id));
        self.requested_mcp
            .retain(|id, _| self.by_segment.contains_key(id));
        self.requested_orch_tools
            .retain(|id, _| self.by_segment.contains_key(id));
        self.requested_subagent_limits
            .retain(|id, _| self.by_segment.contains_key(id));
        self.accepted_configuration
            .retain(|id, _| self.by_segment.contains_key(id));
        self.requested_skills
            .retain(|id, _| self.by_segment.contains_key(id));
    }

    /// The session of a segment is not open any longer.
    pub fn closed(&mut self, segment: Id) {
        self.by_segment.remove(&segment);
        self.requested_instructions.remove(&segment);
        self.requested_mcp.remove(&segment);
        self.requested_orch_tools.remove(&segment);
        self.requested_subagent_limits.remove(&segment);
        self.accepted_configuration.remove(&segment);
        self.requested_skills.remove(&segment);
    }

    /// The harness process `key` was started anew or went away: none of the
    /// sessions it held is known to be open.
    pub fn restarted(&mut self, key: HarnessKey) {
        self.by_segment.retain(|_, (_, held)| *held != key);
        self.requested_instructions
            .retain(|id, _| self.by_segment.contains_key(id));
        self.requested_mcp
            .retain(|id, _| self.by_segment.contains_key(id));
        self.requested_orch_tools
            .retain(|id, _| self.by_segment.contains_key(id));
        self.accepted_configuration
            .retain(|id, _| self.by_segment.contains_key(id));
        self.requested_skills
            .retain(|id, _| self.by_segment.contains_key(id));
    }

    /// The sessions known to be open in harness processes of a provider, as (segment, thread, process): what
    /// the part that keeps providers closes before a credential leaves, or comes home.
    #[must_use]
    pub fn of_provider(&self, provider: Id) -> Vec<(Id, Id, HarnessKey)> {
        self.by_segment
            .iter()
            .filter(|(_, (_, key))| key.provider == provider)
            .map(|(segment, (thread, key))| (*segment, *thread, *key))
            .collect()
    }

    /// Every session on `machine` is gone from view: it went away.
    pub fn machine_lost(&mut self, machine: Id) {
        self.by_segment
            .retain(|_, (_, held)| held.machine != machine);
        self.requested_mcp
            .retain(|id, _| self.by_segment.contains_key(id));
        self.requested_orch_tools
            .retain(|id, _| self.by_segment.contains_key(id));
        self.accepted_configuration
            .retain(|id, _| self.by_segment.contains_key(id));
    }
}

/// The machine a harness runs on in an arrangement: with the provider for
/// `separated`, in the environment for every other type.
#[must_use]
pub fn machine_of(
    arrangement: Option<&str>,
    provider: &ProviderRow,
    environment_machine: Option<Id>,
) -> Option<Id> {
    if arrangement == Some("separated") {
        provider.machine_id
    } else {
        environment_machine
    }
}

/// The descriptor a profile's harness declares, with the system's overrides
/// merged over it. None for a profile whose harness never reported one.
pub fn descriptor_of(agent: &AgentRow) -> Result<Option<AgentDescriptor>, EngineError> {
    let declared = agent
        .descriptor
        .as_object()
        .is_some_and(|members| !members.is_empty());
    if !declared {
        return Ok(None);
    }
    let base: AgentDescriptor = serde_json::from_value(agent.descriptor.clone())?;
    Ok(Some(
        if agent
            .descriptor_overrides
            .as_object()
            .is_some_and(|o| !o.is_empty())
        {
            base.with_overrides(&agent.descriptor_overrides)?
        } else {
            base
        },
    ))
}

/// The selected runtime's declaration, or the profile's last declaration before that runtime reports.
/// A reported version (or an exact pin for an unstarted runtime) remains a fact after display overrides.
pub(crate) async fn descriptor_at(
    conn: &mut orch_db::Conn,
    agent: &AgentRow,
    provider: Id,
    machine: Option<Id>,
) -> Result<Option<AgentDescriptor>, EngineError> {
    let report = match machine {
        Some(machine) => resources::harness(conn, provider, agent.id, machine).await?,
        None => None,
    };
    let mut selected = agent.clone();
    if let Some(report) = &report
        && report
            .descriptor
            .as_object()
            .is_some_and(|members| !members.is_empty())
    {
        selected.descriptor.clone_from(&report.descriptor);
    }
    let mut descriptor = descriptor_of(&selected)?;
    let version = report
        .and_then(|report| report.harness_version)
        .or_else(|| {
            agent
                .profile
                .get("harness_version")
                .and_then(Json::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            selected
                .descriptor
                .pointer("/harness/version")
                .and_then(Json::as_str)
                .map(str::to_owned)
        });
    if let Some(descriptor) = &mut descriptor {
        descriptor.kind.clone_from(&agent.harness_kind);
        if let Some(version) = version {
            descriptor.harness.version = version;
        }
    }
    Ok(descriptor)
}

/// What the provider's owner configured about the harness as a whole (`providers.config`), as `agent.start` carries
/// it to the adapter; `{}` for a provider that set nothing or is gone.
pub(crate) async fn provider_config_of(
    conn: &mut orch_db::Conn,
    provider: Id,
) -> Result<Json, EngineError> {
    let config: Option<Json> = sqlx::query_scalar("SELECT config FROM providers WHERE id = $1")
        .bind(provider)
        .fetch_optional(conn)
        .await
        .map_err(orch_db::DbError::from)?;
    Ok(config.unwrap_or_else(|| serde_json::json!({})))
}

/// Why a harness process could not be had.
#[derive(Debug)]
pub enum Started {
    /// It is up (it was, or `agent.start` brought it up).
    Up,
    /// The command ended otherwise.
    Failed(Outcome),
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct AttachCommands {
    request: Option<AgentStart>,
    ready_before_start: bool,
    commands: Vec<orch_settings_contract::unit::NamedAfterUpCommand>,
    notice: Option<String>,
    refusal: Option<String>,
}

impl Engine {
    fn harness_serial(&self, key: HarnessKey) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        let mut locks = lock(&self.inner.harness_starts);
        locks.retain(|_, serial| serial.strong_count() > 0);
        if let Some(serial) = locks.get(&key).and_then(std::sync::Weak::upgrade) {
            return serial;
        }
        let serial = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(key, std::sync::Arc::downgrade(&serial));
        serial
    }

    /// Freeze the same scoped commands and fact/policy admission before starting a directory runtime.
    async fn provider_attach_plan(&self, key: HarnessKey) -> Result<AttachCommands, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let current: Option<Id> =
            sqlx::query_scalar("SELECT machine_id FROM providers WHERE id=$1")
                .bind(key.provider)
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?
                .flatten();
        if current != Some(key.machine) {
            return Ok(AttachCommands::default());
        }
        let Some(machine) =
            orch_db::queries::environment_facts::of_provider(&mut conn, key.provider).await?
        else {
            return Ok(AttachCommands::default());
        };
        if !machine.attaches() {
            return Ok(AttachCommands::default());
        }
        // A directory has no unit recipe. Only its scoped commands apply at attachment.
        let scoped: Option<Json> = orch_settings::get(
            &mut conn,
            orch_settings::Place::Provider(key.provider),
            orch_settings_contract::registry::keys::RUNTIME_AFTER_UP,
        )
        .await?;
        let commands = crate::jobs::after_up::commands(
            &serde_json::json!({}),
            &scoped.unwrap_or_else(|| serde_json::json!({})),
        )
        .map_err(EngineError::unreadable)?;
        if commands.is_empty() {
            return Ok(AttachCommands::default());
        }
        Ok(match crate::jobs::after_up::admission(&machine) {
            Ok(false) => AttachCommands {
                notice: Some(crate::jobs::after_up::MACHINE_POLICY_OFF.into()),
                ..Default::default()
            },
            Err(reason) => AttachCommands {
                refusal: Some(reason),
                ..Default::default()
            },
            Ok(true) => AttachCommands {
                commands,
                ..Default::default()
            },
        })
    }

    /// The marker and frozen plan survive a server restart; concurrent servers share the same run IDs.
    async fn begin_harness(
        &self,
        key: HarnessKey,
        command: Id,
        plan: &AttachCommands,
        configuration: &str,
    ) -> Result<Option<(Id, AttachCommands)>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let row:Option<(Id,Json)>=sqlx::query_as(
            "INSERT INTO harnesses(provider_id,agent_id,machine_id,status,start_command_id,after_up_plan,updated_at)
             VALUES($1,$2,$3,'starting',$4,$5,$6)
             ON CONFLICT(provider_id,agent_id,machine_id) DO UPDATE SET
               start_command_id=CASE WHEN harnesses.status='error' THEN EXCLUDED.start_command_id ELSE COALESCE(harnesses.start_command_id,EXCLUDED.start_command_id) END,
               after_up_plan=CASE WHEN harnesses.start_command_id IS NULL OR harnesses.status='error' THEN EXCLUDED.after_up_plan ELSE harnesses.after_up_plan END,
               status='starting',status_detail=NULL,updated_at=EXCLUDED.updated_at
             WHERE harnesses.status<>'online' OR harnesses.start_command_id IS NOT NULL OR harnesses.after_up_plan->>'configuration_hash' IS DISTINCT FROM $7::text
             RETURNING start_command_id,after_up_plan")
            .bind(key.provider).bind(key.agent).bind(key.machine).bind(command).bind(serde_json::to_value(plan)?)
            .bind(self.inner.clock.now()).bind(configuration).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
        row.map(|(run, plan)| Ok((run, serde_json::from_value(plan)?)))
            .transpose()
    }

    /// Directory provider runtimes use the unit command journal after their actual home attaches.
    async fn provider_attach_commands(
        &self,
        key: HarnessKey,
        run: Id,
        plan: &AttachCommands,
    ) -> Result<Result<Option<String>, Outcome>, EngineError> {
        if let Some(reason) = &plan.refusal {
            return Ok(Err(Outcome::Failed(CommandError::new(
                ErrorCode::Unsupported,
                reason,
            ))));
        }
        for (position, entry) in plan.commands.iter().enumerate() {
            let position = u32::try_from(position)
                .map_err(|_| EngineError::unreadable("too many commands after provider attach"))?;
            let body = UnitAfterUp {
                run_id: run,
                environment_id: None,
                provider_id: Some(key.provider),
                agent_id: Some(key.agent),
                position,
                name: entry.name.clone(),
                run: orch_settings_contract::unit::AfterUpCommand {
                    command: entry.command.clone(),
                    timeout_secs: entry.timeout_secs,
                },
            };
            let id =
                crate::ids::derived("unit.after_up", &[run.as_bytes(), &position.to_be_bytes()]);
            let sent = Outgoing::new(id, key.machine, SerialKey::Runtime(key.provider), &body)?;
            let outcome = crate::commands::run(self.inner.commands.as_ref(), sent).await?;
            if let Outcome::Failed(error) = &outcome
                && error
                    .details
                    .as_ref()
                    .and_then(|details| details.get("policy_key"))
                    .and_then(Json::as_str)
                    == Some("environment.after_up")
            {
                return Ok(Ok(Some(crate::jobs::after_up::MACHINE_POLICY_OFF.into())));
            }
            match outcome {
                Outcome::Succeeded(value) => {
                    let result: UnitAfterUpResult = serde_json::from_value(value)?;
                    if result.name != body.name {
                        return Err(EngineError::unreadable(
                            "the provider answered for a different after-start command",
                        ));
                    }
                    crate::jobs::after_up::record_direct(self, &body, Some(&result)).await?;
                    if !result.passed() {
                        return Ok(Err(Outcome::Failed(CommandError::new(
                            ErrorCode::Unsupported,
                            format!(
                                "after-start command {} failed: {}",
                                body.name, result.first_lines
                            ),
                        ))));
                    }
                }
                other => {
                    crate::jobs::after_up::record_direct(self, &body, None).await?;
                    return Ok(Err(other));
                }
            }
        }
        Ok(Ok(plan.notice.clone()))
    }
    /// Whether the harness process is online as far as the database says.
    pub(crate) async fn harness_is_online(&self, key: HarnessKey) -> Result<bool, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        Ok(sqlx::query_scalar::<_,bool>("SELECT status='online' AND start_command_id IS NULL FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
            .bind(key.provider).bind(key.agent).bind(key.machine).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?.unwrap_or(false))
    }

    /// Makes sure the harness process is up: `agent.start` where it is not, with the provider's own configuration.
    /// The command is the same for every thread and every turn that needs the
    /// process, and a process that is up answers it with itself.
    pub(crate) async fn ensure_harness(
        &self,
        key: HarnessKey,
        agent: &AgentRow,
        command: Id,
    ) -> Result<Started, EngineError> {
        let serial = self.harness_serial(key);
        let _held = serial.lock().await;
        let was_ready = self.harness_is_online(key).await?;
        if was_ready
            && let Some(pin) = agent.profile.get("harness_version").and_then(Json::as_str)
            && let Some(reported) = resources::harness(
                &mut *self.inner.db.acquire().await?,
                key.provider,
                key.agent,
                key.machine,
            )
            .await?
            && let Some(version) = reported.harness_version
            && version != pin
        {
            return Ok(Started::Failed(Outcome::Failed(CommandError::new(
                ErrorCode::Unsupported,
                format!("{} {} is running, but this profile requires {}. Update the runtime or choose a matching profile", agent.harness_kind, version, pin),
            ).retryable())));
        }
        let home = None;
        let provider_config =
            provider_config_of(&mut *self.inner.db.acquire().await?, key.provider).await?;
        let body = AgentStart {
            provider_id: key.provider,
            agent_id: key.agent,
            harness_kind: agent.harness_kind.clone(),
            profile: serde_json::from_value(agent.profile.clone())?,
            home,
            provider_config,
            runtime: crate::provider_runtime::harness_variables(
                &mut *self.inner.db.acquire().await?,
                key.provider,
                key.agent,
                key.machine,
            )
            .await?,
        };
        let wanted = orch_types::Hash::of(&serde_json::to_vec(&body)?).to_string();
        let prior_configuration=sqlx::query_scalar::<_,Option<String>>("SELECT after_up_plan->>'configuration_hash' FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3 AND start_command_id IS NULL")
            .bind(key.provider).bind(key.agent).bind(key.machine)
            .fetch_optional(&mut *self.inner.db.acquire().await?).await.map_err(orch_db::DbError::from)?.flatten();
        let configured = was_ready && prior_configuration.as_deref() == Some(wanted.as_str());
        if configured {
            return Ok(Started::Up);
        }
        let mut proposed = self.provider_attach_plan(key).await?;
        proposed.request = Some(body.clone());
        proposed.ready_before_start = was_ready || prior_configuration.is_some();
        let Some((command, plan)) = self.begin_harness(key, command, &proposed, &wanted).await?
        else {
            return Ok(Started::Up);
        };
        let frozen = plan.request.is_some();
        let body = plan.request.clone().unwrap_or(body);
        let configuration = orch_types::Hash::of(&serde_json::to_vec(&body)?);
        let outgoing = Outgoing::new(
            command,
            key.machine,
            SerialKey::Harness {
                provider_id: key.provider,
                agent_id: key.agent,
            },
            &body,
        )?;
        let outcome = crate::commands::run(self.inner.commands.as_ref(), outgoing).await?;
        let Outcome::Succeeded(result) = outcome else {
            let mut conn = self.inner.db.acquire().await?;
            sqlx::query("UPDATE harnesses SET status='error',status_detail='The harness could not start.' WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3 AND start_command_id=$4")
                .bind(key.provider).bind(key.agent).bind(key.machine).bind(command).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
            return Ok(Started::Failed(outcome));
        };
        let result: AgentStartResult = serde_json::from_value(result)?;
        // A profile excludes every other version, including an already installed program.
        if let Some(pin) = agent.profile.get("harness_version").and_then(Json::as_str)
            && pin != result.harness_version
        {
            let detail = format!(
                "{} {} is running, but this profile requires {}. Update the runtime or choose a matching profile",
                agent.harness_kind, result.harness_version, pin
            );
            let mut tx = self.inner.db.begin().await?;
            let refusal = serde_json::json!({"actual": &result.harness_version, "required": pin, "detail": &detail});
            let cleared = sqlx::query("UPDATE harnesses SET start_command_id=NULL,after_up_plan=jsonb_build_object('version_pin_refusal',$5::jsonb) WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3 AND start_command_id=$4")
                .bind(key.provider).bind(key.agent).bind(key.machine).bind(command).bind(&refusal).execute(&mut *tx).await.map_err(orch_db::DbError::from)?.rows_affected()>0;
            let mut waiting = Vec::new();
            if cleared {
                resources::set_harness_status(
                    &mut tx,
                    key.provider,
                    key.agent,
                    key.machine,
                    &resources::HarnessReport {
                        status: "error",
                        detail: Some(&detail),
                        version: Some(&result.harness_version),
                        sessions_open: 0,
                    },
                    self.inner.clock.now(),
                )
                .await?;
                // The report remains useful even though this profile excludes its build.
                let descriptor = crate::hosting::with_build_limits(
                    &mut tx,
                    serde_json::to_value(&result.descriptor)?,
                )
                .await?;
                resources::set_harness_descriptor(
                    &mut tx,
                    key.provider,
                    key.agent,
                    key.machine,
                    &descriptor,
                    self.inner.clock.now(),
                )
                .await?;
                waiting = orch_db::queries::threads::blocked_threads_of_provider(
                    &mut tx,
                    key.provider,
                    Some(key.agent),
                )
                .await?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            for thread in waiting {
                self.wake(thread);
            }
            // Keep the input queued; the new report makes the next preflight name the missing build.
            return Ok(Started::Failed(Outcome::Failed(
                CommandError::new(ErrorCode::Unsupported, detail).retryable(),
            )));
        }
        let commands = if result.reused && plan.ready_before_start {
            Ok(plan.notice.clone())
        } else {
            self.provider_attach_commands(key, command, &plan).await?
        };
        let notice = match commands {
            Ok(notice) => notice,
            Err(outcome) => {
                let detail = format!("provider commands after attach did not finish: {outcome:?}");
                let mut conn = self.inner.db.acquire().await?;
                resources::set_harness_status(
                    &mut conn,
                    key.provider,
                    key.agent,
                    key.machine,
                    &resources::HarnessReport {
                        status: "error",
                        detail: Some(&detail),
                        version: Some(&result.harness_version),
                        sessions_open: 0,
                    },
                    self.inner.clock.now(),
                )
                .await?;
                return Ok(Started::Failed(outcome));
            }
        };
        let notice = orch_agent_contract::facts::build_detail(
            agent.profile.get("harness_version").and_then(Json::as_str),
            &result.harness_version,
            notice,
        );
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let cleared=sqlx::query("UPDATE harnesses SET start_command_id=NULL,after_up_plan=jsonb_build_object('configuration_hash',$5::text) WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3 AND start_command_id=$4")
            .bind(key.provider).bind(key.agent).bind(key.machine).bind(command).bind(frozen.then(||configuration.to_string()))
            .execute(&mut *tx).await.map_err(orch_db::DbError::from)?.rows_affected()>0;
        if !cleared {
            let online = resources::harness(&mut tx, key.provider, key.agent, key.machine)
                .await?
                .is_some_and(|row| row.status == ONLINE);
            return if online {
                Ok(Started::Up)
            } else {
                Err(EngineError::unreadable(
                    "a newer harness start replaced this run",
                ))
            };
        }
        resources::set_harness_status(
            &mut tx,
            key.provider,
            key.agent,
            key.machine,
            &resources::HarnessReport {
                status: ONLINE,
                detail: notice.as_deref(),
                version: Some(&result.harness_version),
                sessions_open: 0,
            },
            now,
        )
        .await?;
        let descriptor =
            crate::hosting::with_build_limits(&mut tx, serde_json::to_value(&result.descriptor)?)
                .await?;
        resources::set_harness_descriptor(
            &mut tx,
            key.provider,
            key.agent,
            key.machine,
            &descriptor,
            now,
        )
        .await?;
        let waiting = orch_db::queries::threads::blocked_threads_of_provider(
            &mut tx,
            key.provider,
            Some(key.agent),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        for thread in waiting {
            self.wake(thread);
        }
        lock(&self.inner.sessions).restarted(key);
        Ok(Started::Up)
    }

    /// `harness.status`: the state of a harness process, as it reports it.
    /// A process that comes up frees the threads that were blocked for want
    /// of it; one that goes away loses the sessions it held.
    pub(crate) async fn apply_harness_status(
        &self,
        unit: &mut Unit<'_>,
        machine: Id,
        provider: Id,
        agent: Id,
        status: &HarnessStatus,
    ) -> Result<(), EngineError> {
        let key = HarnessKey {
            provider,
            agent,
            machine,
        };
        let sessions_open = i32::try_from(status.sessions_open).unwrap_or(i32::MAX);
        let now = unit.now();
        let pending:Option<(String,Option<Id>,Option<String>)>=sqlx::query_as("SELECT status,start_command_id,status_detail FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3 FOR UPDATE")
            .bind(provider).bind(agent).bind(machine).fetch_optional(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        if status.status == "starting" && pending.as_ref().is_some_and(|(_, run, _)| run.is_none())
        {
            // A supervised process restart has a new after-start phase. A mere link reconnect
            // reports presence/offline instead and keeps the completed configuration record.
            sqlx::query("UPDATE harnesses SET after_up_plan='{}' WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
                .bind(provider).bind(agent).bind(machine).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        }
        // Online means the process exists; readiness also requires the frozen after-start plan.
        // A supervised restart reports Starting first and must pass the same start machinery again.
        let effective = if status.status == ONLINE {
            match pending.as_ref() {
                Some((previous, Some(_), _)) if previous == "error" => "error",
                Some((_, Some(_), _)) => "starting",
                Some((previous, None, _)) if previous == "starting" => "starting",
                _ => ONLINE,
            }
        } else {
            status.status.as_str()
        };
        let detail = if effective == "error" && status.status == ONLINE {
            pending
                .as_ref()
                .and_then(|(_, _, detail)| detail.as_deref())
                .or(status.detail.as_deref())
        } else {
            status.detail.as_deref()
        };
        let before = resources::set_harness_status(
            &mut *unit.conn,
            provider,
            agent,
            machine,
            &resources::HarnessReport {
                status: effective,
                detail,
                version: status.harness_version.as_deref(),
                sessions_open,
            },
            now,
        )
        .await?;
        if status.status == "error" {
            sqlx::query("UPDATE harnesses SET last_error_at=$4,last_error_detail=$5 WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
                .bind(provider).bind(agent).bind(machine).bind(now).bind(status.detail.as_deref())
                .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        }
        if effective != ONLINE {
            lock(&self.inner.sessions).restarted(key);
        }
        if status.status == ONLINE && before.as_deref() != Some(ONLINE) {
            for thread in orch_db::queries::threads::blocked_threads_of_provider(
                &mut *unit.conn,
                provider,
                Some(agent),
            )
            .await?
            {
                unit.wake(thread);
            }
        }
        Ok(())
    }

    /// `harness.descriptor`: what the adapter declares about its harness.
    pub(crate) async fn apply_harness_descriptor(
        &self,
        unit: &mut Unit<'_>,
        machine: Id,
        provider: Id,
        agent: Id,
        descriptor: &Json,
    ) -> Result<(), EngineError> {
        let now = unit.now();
        let descriptor =
            crate::hosting::with_build_limits(&mut *unit.conn, descriptor.clone()).await?;
        resources::set_harness_descriptor(
            &mut *unit.conn,
            provider,
            agent,
            machine,
            &descriptor,
            now,
        )
        .await?;
        Ok(())
    }
}

/// The build's current proof with the profile's permitted overrides, for an act being planned now.
pub async fn current_descriptor(
    conn: &mut orch_db::Conn,
    agent: &AgentRow,
) -> Result<Option<AgentDescriptor>, EngineError> {
    let mut limited = agent.clone();
    limited.descriptor = crate::hosting::with_build_limits(conn, limited.descriptor).await?;
    descriptor_of(&limited)
}

/// What bringing a harness process up anew came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restarted {
    /// It was stopped where it ran and started again, on this many machines.
    Done(usize),
    /// There is no such provider or profile.
    Gone,
    /// The provider has no machine to run on and no process of the harness is known.
    NoMachine,
    /// No machine it runs on is there.
    Offline,
    /// A machine could not start it again, with this.
    Failed(String),
}

/// What listing the conversations a harness holds came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Discovered {
    /// The conversations, as the adapter lists them.
    Sessions(Vec<Json>),
    /// The harness cannot list them.
    Unsupported,
    /// There is no such provider or profile.
    Gone,
    /// The provider has no machine to run on and no process of the harness is known.
    NoMachine,
    /// The machine it runs on is not there.
    Offline,
    /// The machine could not do it, with this.
    Failed(String),
}

impl Engine {
    /// The machine a harness of `agent` for `provider` is asked on: the first of the places it is, or
    /// would be, run on that is there and online.
    pub(crate) async fn harness_machine(
        &self,
        provider: &ProviderRow,
        agent: Id,
    ) -> Result<Option<Id>, EngineError> {
        for machine in self.harness_places(provider, agent).await? {
            let mut conn = self.inner.db.acquire().await?;
            let place = resources::machine_place(&mut conn, machine)
                .await?
                .filter(|place| {
                    place.deleted_at.is_none() && place.status == "active" && place.online
                });
            drop(conn);
            if place.is_some() {
                return Ok(Some(machine));
            }
        }
        Ok(None)
    }

    /// The machines a harness of `agent` for `provider` is, or would be, run on first: those that hold a
    /// process of it, the one heard of last first, then the profile's declared machine and the
    /// provider's runtime. Credential custody is independent of where an agent process runs.
    async fn harness_places(
        &self,
        provider: &ProviderRow,
        agent: Id,
    ) -> Result<Vec<Id>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let mut places: Vec<Id> = resources_binding::harness_rows(&mut conn, provider.id, agent)
            .await?
            .into_iter()
            .map(|row| row.machine_id)
            .collect();
        let profile = resources::agent(&mut conn, agent).await?;
        for machine in [
            profile.and_then(|profile| profile.machine_id),
            provider.machine_id,
        ]
        .into_iter()
        .flatten()
        {
            if !places.contains(&machine) {
                places.push(machine);
            }
        }
        Ok(places)
    }

    /// Brings the harness process of a profile for a provider up anew, on every machine that holds one:
    /// stopped there, forgotten as far as the sessions it held go (the next turn opens each again), and
    /// started; so that what it declares is learned again. Where none is known it is started where the
    /// provider runs.
    pub async fn restart_harness(&self, provider: Id, agent: Id) -> Result<Restarted, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(keeper) = resources::provider(&mut conn, provider)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(Restarted::Gone);
        };
        let Some(profile) = resources::agent(&mut conn, agent)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(Restarted::Gone);
        };
        drop(conn);
        let places = self.harness_places(&keeper, agent).await?;
        if places.is_empty() {
            return Ok(Restarted::NoMachine);
        }
        let mut done = 0;
        for machine in places {
            let mut conn = self.inner.db.acquire().await?;
            let place = resources::machine_place(&mut conn, machine)
                .await?
                .filter(|place| {
                    place.deleted_at.is_none() && place.status == "active" && place.online
                });
            drop(conn);
            let Some(_) = place else {
                continue;
            };
            let key = HarnessKey {
                provider,
                agent,
                machine,
            };
            let stop = Outgoing::new(
                orch_types::new_id(),
                machine,
                SerialKey::Harness {
                    provider_id: provider,
                    agent_id: agent,
                },
                &AgentStop {
                    provider_id: provider,
                    agent_id: agent,
                    logout: None,
                },
            )?;
            // A process that is not there to be stopped is no obstacle to starting one.
            let _ = tokio::time::timeout(
                self.inner.config.start_timeout,
                crate::commands::run(self.inner.commands.as_ref(), stop),
            )
            .await;
            let mut tx = self.inner.db.begin().await?;
            resources::set_harness_status(
                &mut tx,
                provider,
                agent,
                machine,
                &resources::HarnessReport {
                    status: OFFLINE,
                    detail: None,
                    version: None,
                    sessions_open: 0,
                },
                self.inner.clock.now(),
            )
            .await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            lock(&self.inner.sessions).restarted(key);
            let command = orch_types::new_id();
            let started = tokio::time::timeout(
                self.inner.config.start_timeout,
                self.ensure_harness(key, &profile, command),
            )
            .await;
            match started {
                Err(_elapsed) => {
                    self.inner.commands.cancel(command).await?;
                    return Ok(Restarted::Failed(
                        "the harness did not start in time".to_owned(),
                    ));
                }
                Ok(Err(error)) => return Err(error),
                Ok(Ok(Started::Up)) => done += 1,
                Ok(Ok(Started::Failed(outcome))) => {
                    return Ok(Restarted::Failed(crate::provider_setup::failure_text(
                        &outcome,
                    )));
                }
            }
        }
        Ok(if done == 0 {
            Restarted::Offline
        } else {
            Restarted::Done(done)
        })
    }

    /// Stops the harness process of a profile for a provider wherever it runs, as far as its machine
    /// answers: for a harness that was started to look at a login that was not kept. Nothing is wrong if
    /// none runs.
    pub async fn stop_harness(&self, provider: Id, agent: Id) -> Result<(), EngineError> {
        self.stop_harness_at(provider, agent, None).await
    }

    /// Stops only the selected runtime; an independent credential keeper keeps its native process.
    pub(crate) async fn stop_harness_at(
        &self,
        provider: Id,
        agent: Id,
        machine: Option<Id>,
    ) -> Result<(), EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let rows = resources_binding::harness_rows(&mut conn, provider, agent).await?;
        drop(conn);
        for row in rows
            .into_iter()
            .filter(|row| machine.is_none_or(|machine| row.machine_id == machine))
        {
            let key = HarnessKey {
                provider,
                agent,
                machine: row.machine_id,
            };
            let stop = Outgoing::new(
                orch_types::new_id(),
                row.machine_id,
                SerialKey::Harness {
                    provider_id: provider,
                    agent_id: agent,
                },
                &AgentStop {
                    provider_id: provider,
                    agent_id: agent,
                    logout: None,
                },
            )?;
            let stopped = tokio::time::timeout(
                self.inner.config.start_timeout,
                crate::commands::run(self.inner.commands.as_ref(), stop),
            )
            .await;
            if machine.is_some() {
                if !matches!(stopped, Ok(Ok(Outcome::Succeeded(_)))) {
                    return Err(EngineError::unreadable(
                        "the runtime's machine did not confirm stopping its harness",
                    ));
                }
                let mut conn = self.inner.db.acquire().await?;
                resources::set_harness_status(
                    &mut conn,
                    provider,
                    agent,
                    row.machine_id,
                    &resources::HarnessReport {
                        status: OFFLINE,
                        detail: None,
                        version: None,
                        sessions_open: 0,
                    },
                    self.inner.clock.now(),
                )
                .await?;
            }
            lock(&self.inner.sessions).restarted(key);
        }
        Ok(())
    }

    /// Asks the harness of a profile for a provider which conversations it holds: where the adapter
    /// declares that it can say, on the machine that has a process of it (else where the provider runs,
    /// where it is started first).
    pub async fn discover_sessions(
        &self,
        provider: Id,
        agent: Id,
    ) -> Result<Discovered, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(keeper) = resources::provider(&mut conn, provider)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(Discovered::Gone);
        };
        let Some(profile) = resources::agent(&mut conn, agent)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(Discovered::Gone);
        };
        drop(conn);
        if descriptor_of(&profile)?
            .is_some_and(|descriptor| !descriptor.capabilities.discover_sessions)
        {
            return Ok(Discovered::Unsupported);
        }
        let places = self.harness_places(&keeper, agent).await?;
        let has_places = !places.is_empty();
        let mut found = None;
        for machine in places {
            let mut conn = self.inner.db.acquire().await?;
            let place = resources::machine_place(&mut conn, machine)
                .await?
                .filter(|place| {
                    place.deleted_at.is_none() && place.status == "active" && place.online
                });
            drop(conn);
            if let Some(place) = place {
                found = Some((machine, place));
                break;
            }
        }
        let Some((machine, _)) = found else {
            return Ok(if has_places {
                Discovered::Offline
            } else {
                Discovered::NoMachine
            });
        };
        let key = HarnessKey {
            provider,
            agent,
            machine,
        };
        let command = orch_types::new_id();
        let started = tokio::time::timeout(
            self.inner.config.start_timeout,
            self.ensure_harness(key, &profile, command),
        )
        .await;
        match started {
            Err(_elapsed) => {
                self.inner.commands.cancel(command).await?;
                return Ok(Discovered::Failed(
                    "the harness did not start in time".to_owned(),
                ));
            }
            Ok(Err(error)) => return Err(error),
            Ok(Ok(Started::Up)) => {}
            Ok(Ok(Started::Failed(outcome))) => {
                return Ok(Discovered::Failed(crate::provider_setup::failure_text(
                    &outcome,
                )));
            }
        }
        // What the harness declared by starting is what says whether it can list its conversations.
        let mut conn = self.inner.db.acquire().await?;
        let profile = resources::agent(&mut conn, agent).await?.unwrap_or(profile);
        drop(conn);
        let can = descriptor_of(&profile)?
            .is_some_and(|descriptor| descriptor.capabilities.discover_sessions);
        if !can {
            return Ok(Discovered::Unsupported);
        }
        let asked = orch_types::new_id();
        let command = Outgoing::new(
            asked,
            machine,
            SerialKey::Harness {
                provider_id: provider,
                agent_id: agent,
            },
            &AgentDiscoverSessions {
                provider_id: provider,
                agent_id: agent,
            },
        )?;
        let outcome = tokio::time::timeout(
            self.inner.config.start_timeout,
            crate::commands::run(self.inner.commands.as_ref(), command),
        )
        .await;
        match outcome {
            Err(_elapsed) => {
                self.inner.commands.cancel(asked).await?;
                Ok(Discovered::Failed(
                    "the machine did not answer in time".to_owned(),
                ))
            }
            Ok(answered) => match answered? {
                Outcome::Succeeded(result) => {
                    let result: AgentDiscoverSessionsResult = serde_json::from_value(result)?;
                    Ok(Discovered::Sessions(
                        result
                            .sessions
                            .into_iter()
                            .map(serde_json::to_value)
                            .collect::<Result<_, _>>()?,
                    ))
                }
                other => Ok(Discovered::Failed(crate::provider_setup::failure_text(
                    &other,
                ))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn key(machine: Id) -> HarnessKey {
        HarnessKey {
            provider: new_id(),
            agent: new_id(),
            machine,
        }
    }

    #[test]
    fn a_session_is_known_open_only_in_the_process_that_opened_it() {
        let (segment, machine) = (new_id(), new_id());
        let (first, second) = (key(machine), key(machine));
        let mut sessions = OpenSessions::default();
        sessions.opened(segment, new_id(), first);
        assert!(sessions.is_open(segment, first));
        assert!(!sessions.is_open(segment, second));
        sessions.restarted(first);
        assert!(
            !sessions.is_open(segment, first),
            "a restarted process holds none"
        );
    }

    #[test]
    fn accepted_session_configuration_expires_with_the_actual_process_or_lane() {
        let machine = new_id();
        let process = key(machine);
        let segment = new_id();
        let thread = new_id();
        let configuration = SessionConfiguration {
            cwd: "/workspace/child".into(),
            memory: None,
        };
        let mut sessions = OpenSessions::default();
        sessions.opened(segment, thread, process);
        assert!(
            sessions.configuration_changed(segment, &configuration),
            "an opened event does not acknowledge configuration bytes"
        );
        sessions.configuration_accepted(segment, configuration.clone());
        assert!(!sessions.configuration_changed(segment, &configuration));
        let notes = SessionConfiguration {
            memory: Some(orch_link_contract::commands::SessionOpenMemory {
                directory: "/workspace/.orch/memory".into(),
                kept_by: orch_link_contract::commands::MemoryKeeper::Agent,
            }),
            ..configuration.clone()
        };
        assert!(sessions.configuration_changed(segment, &notes));
        assert!(
            !sessions.configuration_changed(segment, &configuration),
            "a desired change is not an accepted opening"
        );
        for retirement in [0, 1, 2, 3] {
            sessions.opened(segment, thread, process);
            sessions.configuration_accepted(segment, configuration.clone());
            match retirement {
                0 => sessions.closed(segment),
                1 => sessions.forget_thread_sessions(thread),
                2 => sessions.restarted(process),
                _ => sessions.machine_lost(machine),
            }
            assert!(sessions.configuration_changed(segment, &configuration));
        }
    }

    #[test]
    fn a_machine_that_is_lost_takes_every_session_on_it() {
        let (one, other, machine) = (new_id(), new_id(), new_id());
        let (here, elsewhere) = (key(machine), key(new_id()));
        let mut sessions = OpenSessions::default();
        sessions.opened(one, new_id(), here);
        sessions.opened(other, new_id(), elsewhere);
        sessions.machine_lost(machine);
        assert!(!sessions.is_open(one, here));
        assert!(sessions.is_open(other, elsewhere));
    }

    #[test]
    fn a_live_external_reload_keeps_the_accepted_own_catalogue_and_cannot_acknowledge_an_unapplied_change()
     {
        let segment = new_id();
        let external_before = McpRevision::of(b"external revision one");
        let external_after = McpRevision::of(b"external revision two");
        let own_before =
            OrchToolsRevision::of(&serde_json::json!(["read_shared", "announce/topic"]));
        let own_after =
            OrchToolsRevision::of(&serde_json::json!(["read_shared", "announce/project"]));
        let mut sessions = OpenSessions::default();
        sessions.opened(segment, new_id(), key(new_id()));
        sessions.mcp_requested(segment, external_before);
        sessions.orch_tools_requested(segment, own_before);
        sessions.mcp_requested(segment, external_after);
        assert!(
            !sessions.mcp_changed(segment, external_after),
            "a successful reload and turn admission use the same revision type"
        );
        assert!(
            !sessions.orch_tools_changed(segment, own_before),
            "the accepted catalogue stays accepted across an external reload"
        );
        assert!(
            sessions.orch_tools_changed(segment, own_after),
            "a live reload never claims to apply a changed own-tool schema"
        );
        sessions.orch_tools_requested(segment, own_after);
        assert!(!sessions.orch_tools_changed(segment, own_after));
    }

    #[test]
    fn retiring_a_session_retires_both_tool_revision_slots() {
        for retire in 0..4 {
            let (segment, thread, machine) = (new_id(), new_id(), new_id());
            let key = key(machine);
            let external = McpRevision::of(b"external");
            let own = OrchToolsRevision::of(&serde_json::json!(["own"]));
            let mut sessions = OpenSessions::default();
            sessions.opened(segment, thread, key);
            sessions.mcp_requested(segment, external);
            sessions.orch_tools_requested(segment, own);
            match retire {
                0 => sessions.closed(segment),
                1 => sessions.forget_thread_sessions(thread),
                2 => sessions.restarted(key),
                _ => sessions.machine_lost(machine),
            }
            assert!(!sessions.is_open(segment, key));
            assert!(sessions.mcp_changed(segment, external));
            assert!(sessions.orch_tools_changed(segment, own));
        }
    }
}
