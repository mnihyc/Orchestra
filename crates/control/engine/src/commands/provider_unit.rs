//! The logical Provider Stop and its original native command in the shared HTTP family.

use crate::{
    EngineError,
    commands::{CommandAuthority, Outgoing},
    operation_scope::{
        continuing::{self, Error, Source},
        http_origin,
        sources::{self, Facts},
        targets,
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetUsage, TargetResolver},
    resources::ProviderStop,
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn,
    queries::{
        environments::hosting as envs,
        link::{CommandRow, NewCommand},
        resources::{hosting as hosts, provider_runtime as providers},
        runtime_controls::Intent,
        runtime_incarnations::{Stop, StopProducer},
    },
};
use orch_link_contract::{
    commands::{CommandKind, NodeRuntimeStop, SerialKey},
    native_target::{LifecycleSubject, LifecycleTarget, NativeTarget, RuntimeSource},
};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

fn closed() -> EngineError {
    EngineError::BeforeEffect(Box::new(Error::Refused))
}
fn mapped(error: Error) -> EngineError {
    match error {
        Error::Native(error) => error,
        error => EngineError::BeforeEffect(Box::new(error)),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Provider,
}
const DECLARATION: Declaration<Role> = Declaration {
    operation: "ProviderStop",
    bindings: &[Binding {
        source: Role::Provider,
        target: TargetResolver::PermissionObject,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

/// The actual Provider operation requires Provider Manage, never Host Manage or Use.
pub async fn admit(conn: &mut Conn, caller: &Caller, provider: Id) -> Result<Caller, EngineError> {
    if caller
        .admitted_operation()
        .is_some_and(|name| name != DECLARATION.operation)
    {
        return Err(closed());
    }
    let target = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    sources::bind_required(
        ProviderStop::ROUTE,
        &DECLARATION,
        &[Facts::one(
            Role::Provider,
            KeyTargetKind::Object(ObjectKind::Provider),
            target,
        )],
    )
    .map_err(|error| EngineError::BeforeEffect(Box::new(error)))?
    .admit(caller, false)
    .map_err(|error| EngineError::BeforeEffect(Box::new(error)))
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StopFacts {
    id: Id,
    source: RuntimeSource,
    control: Intent,
    revision: i64,
}
impl StopFacts {
    fn of(stop: &Stop) -> Self {
        Self {
            id: stop.id,
            source: stop.source,
            control: stop.control.clone(),
            revision: stop.revision,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    provider: Id,
    owner: Id,
    home: Id,
    org: Id,
    agent: Option<Id>,
    harness: Option<String>,
    machine: Option<Id>,
    kind: String,
    host: Id,
    runtime: Id,
    generation: i32,
    stop: StopFacts,
}

impl Input {
    async fn capture(conn: &mut Conn, provider: Id, stop: &Stop) -> Result<Self, EngineError> {
        let state = providers::state(conn, provider).await?.ok_or_else(closed)?;
        let runtime = envs::runtime(conn, stop.control.runtime())
            .await?
            .ok_or_else(closed)?;
        let input = Self {
            provider,
            owner: state.owner_id,
            home: state.home_id,
            org: state.org_id,
            agent: state.agent_id,
            harness: state.harness_kind,
            machine: state.machine_id,
            kind: state.kind,
            host: runtime.host_id,
            runtime: runtime.id,
            generation: runtime.generation,
            stop: StopFacts::of(stop),
        };
        if !input.matches(conn).await? {
            return Err(closed());
        }
        Ok(input)
    }

    async fn lock(&self, conn: &mut Conn) -> Result<(), EngineError> {
        providers::lock(conn, self.provider)
            .await?
            .ok_or_else(closed)?;
        sqlx::query("SELECT id FROM runtimes WHERE id=$1 FOR SHARE")
            .bind(self.runtime)
            .fetch_optional(conn)
            .await
            .map_err(orch_db::DbError::from)?
            .ok_or_else(closed)?;
        Ok(())
    }

    async fn matches(&self, conn: &mut Conn) -> Result<bool, EngineError> {
        let Some(state) = providers::state(conn, self.provider).await? else {
            return Ok(false);
        };
        let Some(runtime) = providers::live_runtime(conn, self.provider).await? else {
            return Ok(false);
        };
        let Some(host) = hosts::host(conn, self.host).await? else {
            return Ok(false);
        };
        let revision: Option<i64> =
            sqlx::query_scalar("SELECT native_control_revision FROM runtimes WHERE id=$1")
                .bind(self.runtime)
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
        let original_machine = state.machine_id == self.machine
            || (state.machine_id.is_none()
                && orch_db::queries::runtime_incarnations::terminal(
                    conn,
                    self.runtime,
                    self.stop.source,
                )
                .await?);
        Ok(state.deleted_at.is_none()
            && state.owner_id == self.owner
            && state.home_id == self.home
            && state.org_id == self.org
            && state.agent_id == self.agent
            && state.harness_kind == self.harness
            && state.kind == self.kind
            && original_machine
            && runtime.id == self.runtime
            && runtime.provider_id == Some(self.provider)
            && runtime.kind == "provider"
            && runtime.host_id == self.host
            && runtime.generation == self.generation
            && self.stop.control.host() == self.host
            && host.machine_id == self.stop.control.machine()
            && revision == Some(self.stop.revision)
            && orch_db::queries::runtime_controls::owns_current_source(conn, &self.stop.control)
                .await?
            && orch_db::queries::runtime_controls::is_current(conn, &self.stop.control).await?)
    }
}

struct Native;
impl Source for Native {
    type Role = Role;
    type Input = Input;
    type Value = Input;
    const ROUTE: Route = ProviderStop::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &DECLARATION;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a Input,
        _: Time,
    ) -> BoxFuture<'a, Result<(Input, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            if !input.matches(conn).await? {
                return Err(Error::Refused);
            }
            let target = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id: input.provider,
                },
            )
            .await
            .map_err(EngineError::unreadable)?;
            Ok((
                input.clone(),
                vec![Facts::one(
                    Role::Provider,
                    KeyTargetKind::Object(ObjectKind::Provider),
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        _: &'a Input,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Provider,
                    id: input.provider,
                },
                Permission::ProviderManage,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        _: &'a Input,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            input.lock(conn).await?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        old: &'a Input,
        fresh: &'a Input,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(old == fresh) })
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    origin: CapturedOperation,
    input: Input,
    canonical: Json,
    deadline: Option<Time>,
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderUnitStop([private])")
    }
}

fn target(stop: &Stop) -> NativeTarget {
    NativeTarget::Lifecycle(LifecycleTarget {
        pending: stop.control.pending().to_vec(),
        subject: LifecycleSubject::Runtime {
            runtime_id: stop.control.runtime(),
        },
        observed: stop.source.incarnation,
        opening: None,
        revision: stop.revision,
    })
}

/// Prepare once while the caller still holds its pending Stop transaction. The saved value
/// is the existing Outgoing/HttpCommand carrier and is replayed verbatim after a crash.
pub(crate) async fn pending(
    conn: &mut Conn,
    clock: &dyn Clock,
    provider: Id,
    stop: &Stop,
) -> Result<Option<Outgoing>, EngineError> {
    if stop.producer != StopProducer::Unbound {
        return restore(provider, stop).map(Some);
    }
    let Some(http) =
        http_origin::active().filter(|http| http.origin().operation() == DECLARATION.operation)
    else {
        return Ok(None);
    };
    let mut outgoing = Outgoing::new(
        stop.id,
        stop.control.machine(),
        SerialKey::Runtime(stop.control.runtime()),
        &NodeRuntimeStop {
            runtime_id: stop.control.runtime(),
            reason: None,
        },
    )?;
    outgoing.native_target = Some(target(stop));
    let input = Input::capture(conn, provider, stop).await?;
    let frame = http
        .prepare::<Native>(conn, clock, input.clone())
        .await
        .map_err(mapped)?;
    frame.check_deadline(clock).map_err(mapped)?;
    let source = Command {
        origin: CapturedOperation::capture(frame.principal().caller()).map_err(|_| closed())?,
        input,
        canonical: crate::jobs::command_identity::canonical(
            outgoing.id,
            outgoing.machine,
            outgoing.key.wire().as_deref(),
            outgoing.kind.as_str(),
            &outgoing.body,
        )?,
        // The ordinary issuer allocates its work budget only on the first queue insert;
        // the existing outer HTTP carrier then seals that actual deadline for every retry.
        deadline: outgoing.deadline,
    };
    outgoing.authority = Some(http.command_source(
        &outgoing,
        crate::operation_scope::http_command_source::CommandEvidence::ProviderUnitStop(Box::new(
            source,
        )),
    )?);
    Ok(Some(outgoing))
}

/// A known Provider continuation cannot silently become a bare or differently sourced Stop.
pub(crate) fn restore(provider: Id, stop: &Stop) -> Result<Outgoing, EngineError> {
    let outgoing: Outgoing = serde_json::from_value(stop.command.clone().ok_or_else(closed)?)?;
    let Some(CommandAuthority::Http(http)) = &outgoing.authority else {
        return Err(closed());
    };
    let Some(crate::operation_scope::http_command_source::CommandEvidence::ProviderUnitStop(
        source,
    )) = http.native_source()
    else {
        return Err(closed());
    };
    // The source contains immutable operands; the private pending carrier is not nested in itself.
    if stop.producer == StopProducer::Unbound
        || http.operation() != DECLARATION.operation
        || source.origin.operation() != DECLARATION.operation
        || source.input.provider != provider
        || source.input.stop.id != stop.id
        || source.input.stop.source != stop.source
        || source.input.stop.control != stop.control
        || source.input.stop.revision != stop.revision
        || outgoing.id != stop.id
        || outgoing.kind != CommandKind::NodeRuntimeStop
        || outgoing.machine != stop.control.machine()
        || outgoing.key != SerialKey::Runtime(stop.control.runtime())
        || outgoing.body
            != serde_json::to_value(NodeRuntimeStop {
                runtime_id: stop.control.runtime(),
                reason: None,
            })?
        || outgoing.native_target != Some(target(stop))
        || outgoing.deadline != source.deadline
        || source.canonical
            != crate::jobs::command_identity::canonical(
                outgoing.id,
                outgoing.machine,
                outgoing.key.wire().as_deref(),
                outgoing.kind.as_str(),
                &outgoing.body,
            )?
    {
        return Err(closed());
    }
    Ok(outgoing)
}

/// Join active original work or its confirmed result. A new authorized press can own a
/// fresh intent when an older command expired or its actual source no longer permits it.
pub(crate) async fn reusable(
    conn: &mut Conn,
    clock: &dyn Clock,
    provider: Id,
    stop: &Stop,
) -> Result<bool, EngineError> {
    if stop.producer == StopProducer::Unbound {
        return Ok(true);
    }
    let original = restore(provider, stop)?;
    if stop.producer == StopProducer::ProviderStopRefused {
        return Ok(false);
    }
    let row = orch_db::queries::link::command(conn, stop.id).await?;
    if let Some(row) = row {
        if row.machine_id != original.machine
            || row.kind != original.kind.as_str()
            || row.serial_key() != original.key.wire().as_deref()
            || row.body != original.body
        {
            return Err(closed());
        }
        if row.state == "succeeded" {
            return Ok(row.result.is_some());
        }
        if !orch_db::queries::link::OPEN_STATES.contains(&row.state.as_str())
            || (row.state != "accepted" && row.deadline_at <= clock.now())
        {
            return Ok(false);
        }
    }
    // No other actor's identity is locked while this accepting transaction holds its
    // Provider. The actual issuer restores the original raw/key minimum after commit.
    Ok(true)
}

impl Command {
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        self.input.lock(conn).await
    }
    pub async fn current_issue(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != "node.runtime.stop"
            || self.deadline.is_some_and(|deadline| {
                command.deadline_at != deadline.get() || clock.time() >= deadline
            })
            || crate::jobs::command_identity::canonical(
                command.id,
                command.machine_id,
                command.serial_key,
                command.kind,
                command.body,
            )? != self.canonical
        {
            return Ok(None);
        }
        let Some(pending) =
            orch_db::queries::runtime_incarnations::stop(conn, self.input.runtime).await?
        else {
            return Ok(None);
        };
        if pending.producer != StopProducer::ProviderStop
            || StopFacts::of(&pending) != self.input.stop
        {
            return Ok(None);
        }
        let retained = match restore(self.input.provider, &pending) {
            Ok(retained) => retained,
            Err(_) => return Ok(None),
        };
        let Some(CommandAuthority::Http(retained)) = retained.authority else {
            return Ok(None);
        };
        if !matches!(retained.native_source(),
            Some(crate::operation_scope::http_command_source::CommandEvidence::ProviderUnitStop(source)) if source.as_ref() == self)
        {
            return Ok(None);
        }
        let Some(http) = http_origin::active() else {
            return Ok(None);
        };
        if http.origin().operation() != self.origin.operation()
            || http.origin().user() != self.origin.user()
        {
            return Ok(None);
        }
        match continuing::prepare::<Native>(conn, clock, &self.origin, self.input.clone()).await {
            Ok(frame) => {
                frame.check_deadline(clock).map_err(mapped)?;
                Ok(Some(vec![(
                    Some(frame.principal().caller().clone()),
                    http.until(),
                )]))
            }
            Err(Error::Refused | Error::Binding(_)) => Ok(None),
            Err(error) => Err(mapped(error)),
        }
    }
    pub async fn current_stored(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_issue(
            conn,
            &NewCommand {
                native_target: None,
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.serial_key(),
                deadline_at: command.deadline_at,
            },
            clock,
        )
        .await
    }
}
