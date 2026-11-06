//! Exact caller and physical source evidence for the existing credential-return workflow.
//! This is private continuation provenance, never a grant or a machine protocol field.

use super::handover::ReturnError;
use crate::error::EngineError;
use crate::operation_scope::{BindingError, Prepared, targets};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::Conn;
use orch_db::queries::environments::thread_engine as environments;
use orch_db::queries::resources::{self, provider_runtime as rows};
use orch_link_contract::commands::{
    AgentCredentialReturn, MachineKey, ProviderCredentialAccept, SealedCredential, SerialKey,
};
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};

const PROVIDER: &str = "ProviderHandoverReturn.provider";
const HOLDER: &str = "ProviderHandoverReturn.holding_environment";

/// The accepted operation and the existing custody incarnation it acted on.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoverAuthority {
    origin: CapturedOperation,
    provider: Id,
    holder: Id,
    since: Time,
    generation: Option<i32>,
    interrupt: bool,
    lost: bool,
}

impl std::fmt::Debug for HandoverAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HandoverAuthority([private])")
    }
}

impl HandoverAuthority {
    /// Capture only the operation admitted by the request's current guarded producer.
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        provider: Id,
        interrupt: bool,
        lost: bool,
        now: Time,
    ) -> Result<Self, ReturnError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        if !matches!(origin.operation(), PROVIDER | HOLDER)
            || (lost && origin.operation() == HOLDER)
        {
            return Err(refused());
        }
        let state = rows::lock(conn, provider)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or(ReturnError::NotHeld)?;
        let holder = state.held_by_environment_id.ok_or(ReturnError::NotHeld)?;
        let since = rows::held_since(conn, provider)
            .await?
            .ok_or(ReturnError::NotHeld)?;
        let generation = environments::environment(conn, holder)
            .await?
            .map(|row| row.generation);
        let authority = Self {
            origin,
            provider,
            holder,
            since: Time::new(since),
            generation,
            interrupt,
            lost,
        };
        if !authority.authorized(conn, now).await? {
            return Err(refused());
        }
        Ok(authority)
    }

    #[must_use]
    pub fn provider(&self) -> Id {
        self.provider
    }
    #[must_use]
    pub fn holder(&self) -> Id {
        self.holder
    }
    #[must_use]
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    pub(crate) fn matches(
        &self,
        provider: Id,
        caller: &Caller,
        interrupt: bool,
        lost: bool,
    ) -> bool {
        self.provider == provider
            && CapturedOperation::capture(caller).ok().as_ref() == Some(&self.origin)
            && self.interrupt == interrupt
            && self.lost == lost
    }

    /// Check the original and current ceilings against fresh targets and the same ordinary branch.
    async fn principal(
        &self,
        conn: &mut Conn,
        now: Time,
        completed: bool,
    ) -> Result<Option<Principal>, EngineError> {
        let Some(state) = rows::state(conn, self.provider)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(None);
        };
        let current_since = rows::held_since(conn, self.provider).await?;
        let same = state.held_by_environment_id == Some(self.holder)
            && current_since == Some(self.since.get());
        if !same && !(completed && state.held_by_environment_id.is_none()) {
            return Ok(None);
        }
        let environment = environments::environment(conn, self.holder).await?;
        if !completed && environment.as_ref().map(|row| row.generation) != self.generation {
            return Ok(None);
        }
        let Some(caller) = self.origin.restore(conn, now).await? else {
            return Ok(None);
        };
        if crate::preflight::second_step_missing(conn, self.origin.user(), state.home_id).await? {
            return Ok(None);
        }
        let mut plan =
            Prepared::for_selection(self.origin.operation()).map_err(EngineError::unreadable)?;
        let provider = match targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: self.provider,
            },
        )
        .await
        {
            Ok(target) => target,
            Err(targets::TargetError::Missing) => return Ok(None),
            Err(error) => return Err(EngineError::unreadable(error)),
        };
        let response_target = provider.clone();
        let (object, permission) = match self.origin.operation() {
            PROVIDER => {
                plan.bind(0, provider.object.kind, &[provider])
                    .map_err(EngineError::unreadable)?;
                (
                    ObjectRef {
                        kind: ObjectKind::Provider,
                        id: self.provider,
                    },
                    Permission::ProviderManage,
                )
            }
            HOLDER if !self.lost => {
                let holder = match targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: self.holder,
                    },
                )
                .await
                {
                    Ok(target) => target,
                    Err(targets::TargetError::Missing) => return Ok(None),
                    Err(error) => return Err(EngineError::unreadable(error)),
                };
                plan.bind(0, holder.object.kind, &[holder])
                    .map_err(EngineError::unreadable)?;
                plan.bind(1, provider.object.kind, &[provider])
                    .map_err(EngineError::unreadable)?;
                (
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: self.holder,
                    },
                    Permission::EnvironmentManage,
                )
            }
            _ => return Ok(None),
        };
        match plan.compatible(&caller) {
            Ok(()) => {}
            Err(BindingError::Refused) => return Ok(None),
            Err(error) => return Err(EngineError::unreadable(error)),
        }
        let caller = if completed {
            // Completed effects are not admitted again. The same operation's current Read
            // bounds the response before its ordinary selected-branch check.
            let metadata = plan
                .variant()
                .facets
                .iter()
                .find(|facet| {
                    facet.usage == orch_api_contract::operations::FacetUsage::ResponseContent
                        && facet.resources == [response_target.object.kind]
                })
                .ok_or_else(|| {
                    EngineError::unreadable("the return response has no Provider Read")
                })?;
            let facts = crate::operation_scope::facets(
                self.origin.operation(),
                metadata,
                response_target.object.kind,
                &[response_target],
            )
            .map_err(EngineError::unreadable)?;
            match caller.admit_facets(&facts) {
                Ok(caller) => caller.into_caller(),
                Err(_) => return Ok(None),
            }
        } else {
            match plan.admit(&caller, false) {
                Ok(caller) => caller,
                Err(BindingError::Refused) => return Ok(None),
                Err(error) => return Err(EngineError::unreadable(error)),
            }
        };
        let principal = match Principal::load(conn, caller, now).await {
            Ok(principal) => principal,
            Err(orch_authz::Error::NoSuchUser(_) | orch_authz::Error::Suspended { .. }) => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        match orch_authz::resources::check(conn, &principal, object, permission, Policy::default())
            .await
        {
            Ok(decision) if decision.is_allowed() => Ok(Some(principal)),
            Ok(_)
            | Err(
                orch_authz::Error::NotFound(_)
                | orch_authz::Error::NoSuchUser(_)
                | orch_authz::Error::Suspended { .. }
                | orch_authz::Error::Archived { .. },
            ) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn authorized(&self, conn: &mut Conn, now: Time) -> Result<bool, EngineError> {
        Ok(self.principal(conn, now, false).await?.is_some())
    }

    /// Successful return clears custody. Its accepted holder remains the response authority anchor.
    pub async fn authorize_response(
        &self,
        conn: &mut Conn,
        now: Time,
    ) -> Result<Option<Principal>, EngineError> {
        self.principal(conn, now, true).await
    }

    pub(crate) async fn require(&self, conn: &mut Conn, now: Time) -> Result<(), ReturnError> {
        if self.authorized(conn, now).await? {
            Ok(())
        } else {
            Err(refused())
        }
    }
}

pub(crate) fn refused() -> ReturnError {
    ReturnError::Failed(
        "the accepted credential return is no longer authorized for its current source".into(),
    )
}

/// Existing physical facts binding an unfinished ciphertext to its actual source and recipient.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReturnSource {
    pub(super) provider: Id,
    pub(super) holder: Id,
    pub(super) since: Time,
    pub(super) holder_machine: Id,
    pub(super) generation: i32,
    pub(super) agent: Id,
    pub(super) custodian: MachineKey,
    provider_machine: Option<Id>,
    key_machine: Option<Id>,
    pub(super) runtime: Option<(Id, i32)>,
    pub(super) lease: Option<Id>,
    pub(super) return_command: Id,
}

impl std::fmt::Debug for ReturnSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReturnSource([private])")
    }
}

impl ReturnSource {
    pub(super) async fn capture(
        conn: &mut Conn,
        provider: Id,
        holder: Id,
        since: Time,
        agent: Id,
        custodian: MachineKey,
        command: Id,
    ) -> Result<Self, ReturnError> {
        let environment = environments::environment(conn, holder)
            .await?
            .ok_or_else(refused)?;
        let holder_machine = environment.machine_id.ok_or_else(refused)?;
        let state = rows::state(conn, provider).await?.ok_or_else(refused)?;
        let runtime = rows::live_runtime(conn, provider)
            .await?
            .filter(|row| row.machine_id == Some(custodian.machine_id))
            .map(|row| (row.id, row.generation));
        let lease = sqlx::query_scalar("SELECT id FROM leases WHERE provider_id=$1 AND mode='handover' AND recipient_machine_id=$2 AND opened_at >= $3 ORDER BY opened_at DESC,id DESC LIMIT 1")
            .bind(provider).bind(holder_machine).bind(since.get()).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
        let source = Self {
            provider,
            holder,
            since,
            holder_machine,
            generation: environment.generation,
            agent,
            custodian,
            provider_machine: state.machine_id,
            key_machine: state.key_machine_id,
            runtime,
            lease,
            return_command: command,
        };
        if !source.current(conn, true).await? {
            return Err(refused());
        }
        Ok(source)
    }

    pub(super) async fn current(
        &self,
        conn: &mut Conn,
        issuing_return: bool,
    ) -> Result<bool, EngineError> {
        let Some(state) = rows::state(conn, self.provider)
            .await?
            .filter(|row| row.deleted_at.is_none())
        else {
            return Ok(false);
        };
        if state.held_by_environment_id != Some(self.holder)
            || rows::held_since(conn, self.provider).await? != Some(self.since.get())
        {
            return Ok(false);
        }
        let Some(environment) = environments::environment(conn, self.holder).await? else {
            return Ok(false);
        };
        if environment.machine_id != Some(self.holder_machine)
            || environment.generation != self.generation
        {
            return Ok(false);
        }
        if state.key_machine_id != self.key_machine
            || (Some(self.custodian.machine_id) != self.key_machine
                && state.machine_id != self.provider_machine)
            || (Some(self.custodian.machine_id) != self.provider_machine
                && Some(self.custodian.machine_id) != self.key_machine)
        {
            return Ok(false);
        }
        let Some(keeper) = resources::machine(conn, self.custodian.machine_id).await? else {
            return Ok(false);
        };
        if keeper.link().public_key != self.custodian.public_key {
            return Ok(false);
        }
        let runtime = rows::live_runtime(conn, self.provider)
            .await?
            .filter(|row| row.machine_id == Some(self.custodian.machine_id))
            .map(|row| (row.id, row.generation));
        if runtime != self.runtime {
            return Ok(false);
        }
        if issuing_return
            && rows::harness_agent_on(conn, self.provider, self.holder_machine)
                .await?
                .or(state.agent_id)
                != Some(self.agent)
        {
            return Ok(false);
        }
        if let Some(lease) = self.lease {
            let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM leases WHERE id=$1 AND provider_id=$2 AND mode='handover' AND recipient_machine_id=$3 AND opened_at >= $4)")
                .bind(lease).bind(self.provider).bind(self.holder_machine).bind(self.since.get()).fetch_one(conn).await.map_err(orch_db::DbError::from)?;
            if !valid {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn return_body(&self) -> AgentCredentialReturn {
        AgentCredentialReturn {
            provider_id: self.provider,
            agent_id: self.agent,
            custodian: self.custodian.clone(),
        }
    }
}

/// Stored beside the canonical durable command, never included in its machine body.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoverCommandAuthority {
    origin: Option<HandoverAuthority>,
    source: ReturnSource,
    command: Id,
    returned: Option<Hash>,
}

impl std::fmt::Debug for HandoverCommandAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HandoverCommandAuthority([private])")
    }
}

pub(super) fn fingerprint(value: &SealedCredential) -> Result<Hash, EngineError> {
    Ok(Hash::of(&serde_json::to_vec(value)?))
}

impl HandoverCommandAuthority {
    pub(super) fn returning(origin: Option<&HandoverAuthority>, source: &ReturnSource) -> Self {
        Self {
            origin: origin.cloned(),
            source: source.clone(),
            command: source.return_command,
            returned: None,
        }
    }
    pub(super) fn accepting(
        origin: Option<&HandoverAuthority>,
        source: &ReturnSource,
        command: Id,
        sealed: &SealedCredential,
    ) -> Result<Self, EngineError> {
        Ok(Self {
            origin: origin.cloned(),
            source: source.clone(),
            command,
            returned: Some(fingerprint(sealed)?),
        })
    }
    #[must_use]
    pub fn provider(&self) -> Id {
        self.source.provider
    }
    #[must_use]
    pub fn source(&self) -> &ReturnSource {
        &self.source
    }
    #[must_use]
    pub fn command_kind(&self) -> &'static str {
        if self.returned.is_some() {
            "provider.credential.accept"
        } else {
            "agent.credential.return"
        }
    }

    /// Identity/deadline metadata only, after this exact canonical first-send check.
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let Some(origin) = &self.origin else {
            return Ok(Some(Vec::new()));
        };
        Ok(origin
            .origin
            .restore_current(conn, clock)
            .await?
            .map(|caller| vec![(Some(caller), None)]))
    }

    /// Check canonical identity and actual source result at the existing unacknowledged wire gate.
    #[expect(
        clippy::too_many_arguments,
        reason = "the canonical command identity and actual wire body must be checked together"
    )]
    pub async fn permits(
        &self,
        conn: &mut Conn,
        command: Id,
        machine: Id,
        kind: &str,
        key: Option<&str>,
        body: &Json,
        now: Time,
    ) -> Result<bool, EngineError> {
        if command != self.command
            || kind != self.command_kind()
            || !self.source.current(conn, self.returned.is_none()).await?
        {
            return Ok(false);
        }
        if let Some(origin) = &self.origin {
            if origin.lost
                || origin.provider != self.source.provider
                || origin.holder != self.source.holder
                || origin.since != self.source.since
                || !origin.authorized(conn, now).await?
            {
                return Ok(false);
            }
        }
        if let Some(expected) = self.returned {
            if machine != self.source.custodian.machine_id
                || key != SerialKey::Provider(self.source.provider).wire().as_deref()
            {
                return Ok(false);
            }
            let accept: ProviderCredentialAccept = match serde_json::from_value(body.clone()) {
                Ok(value) => value,
                Err(_) => return Ok(false),
            };
            if accept.provider_id != self.source.provider {
                return Ok(false);
            }
            let sealed = SealedCredential {
                form: accept.form,
                credential: accept.credential,
                expires_at: accept.expires_at,
            };
            if fingerprint(&sealed)? != expected {
                return Ok(false);
            }
            let Some(row) =
                orch_db::queries::link::command(conn, self.source.return_command).await?
            else {
                return Ok(false);
            };
            Ok(row.machine_id == self.source.holder_machine
                && row.kind == "agent.credential.return"
                && row.serial_key
                    == SerialKey::Harness {
                        provider_id: self.source.provider,
                        agent_id: self.source.agent,
                    }
                    .wire()
                && row.body == serde_json::to_value(self.source.return_body())?
                && row.state == "succeeded"
                && row.result.as_ref().is_some_and(|result| {
                    serde_json::from_value::<SealedCredential>(result.clone())
                        .ok()
                        .as_ref()
                        == Some(&sealed)
                }))
        } else {
            Ok(machine == self.source.holder_machine
                && key
                    == SerialKey::Harness {
                        provider_id: self.source.provider,
                        agent_id: self.source.agent,
                    }
                    .wire()
                    .as_deref()
                && *body == serde_json::to_value(self.source.return_body())?)
        }
    }
}
