//! The exact Host unit Stop and its original HTTP operation in the existing command family.
use crate::{
    EngineError,
    commands::Outgoing,
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
    resources::HostRuntimeStop,
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
    },
};
use orch_link_contract::{
    commands::{CommandKind, NodeRuntimeStop, SerialKey},
    native_target::{
        LifecycleAction, LifecycleSubject, LifecycleTarget, NativeTarget, RuntimeSource,
    },
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
    Host,
}
const DECLARATION: Declaration<Role> = Declaration {
    operation: "HostRuntimeStop",
    bindings: &[Binding {
        source: Role::Host,
        target: TargetResolver::PermissionObject,
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};

/// Bind the actual Host operation, without introducing a Tenant Manage requirement.
pub async fn admit(conn: &mut Conn, caller: &Caller, host: Id) -> Result<Caller, EngineError> {
    if caller
        .admitted_operation()
        .is_some_and(|name| name != DECLARATION.operation)
    {
        return Err(closed());
    }
    let target = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Host,
            id: host,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    sources::bind_required(
        HostRuntimeStop::ROUTE,
        &DECLARATION,
        &[Facts::one(
            Role::Host,
            KeyTargetKind::Object(ObjectKind::Host),
            target,
        )],
    )
    .map_err(|error| EngineError::BeforeEffect(Box::new(error)))?
    .admit(caller, false)
    .map_err(|error| EngineError::BeforeEffect(Box::new(error)))
}

/// Original physical operands only; the factory supplies current permission separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    host: Id,
    machine: Id,
    owner: Option<Id>,
    home: Option<Id>,
    org: Option<Id>,
    runtime: Id,
    environment: Option<Id>,
    provider: Option<Id>,
    source: RuntimeSource,
    control: Intent,
    target: LifecycleTarget,
    registration: orch_db::queries::runtime_incarnations::Registration,
}
impl Input {
    /// The selected receiver and phase are captured on the accepting resource transaction.
    pub(crate) async fn capture(
        conn: &mut Conn,
        host: Id,
        runtime: Id,
        control: &Intent,
        target: &LifecycleTarget,
    ) -> Result<Self, EngineError> {
        let unit = envs::runtime(conn, runtime).await?.ok_or_else(closed)?;
        let node = hosts::host(conn, host).await?.ok_or_else(closed)?;
        if unit.host_id != host
            || control.host() != host
            || control.machine() != node.machine_id
            || control.runtime() != runtime
            || target.subject
                != (LifecycleSubject::Runtime {
                    runtime_id: runtime,
                })
            || target.observed != control.source().incarnation
            || !target.coherent_for(LifecycleAction::Stop)
            || !orch_db::queries::runtime_controls::is_current(conn, control).await?
        {
            return Err(closed());
        }
        Ok(Self {
            host,
            machine: control.machine(),
            owner: node.owner_id,
            home: node.home_id,
            org: node.org_id,
            runtime,
            environment: unit.environment_id,
            provider: unit.provider_id,
            source: control.source(),
            control: control.clone(),
            target: target.clone(),
            registration: orch_db::queries::runtime_incarnations::Registration::capture(
                conn, runtime,
            )
            .await?
            .ok_or_else(closed)?,
        })
    }
    async fn lock(&self, conn: &mut Conn) -> Result<(), EngineError> {
        if let Some(provider) = self.provider {
            providers::lock(conn, provider).await?.ok_or_else(closed)?;
        } else if let Some(environment) = self.environment {
            envs::lock_environment(conn, environment)
                .await?
                .ok_or_else(closed)?;
        } else {
            return Err(closed());
        }
        sqlx::query("SELECT id FROM runtimes WHERE id=$1 FOR SHARE")
            .bind(self.runtime)
            .fetch_optional(conn)
            .await
            .map_err(orch_db::DbError::from)?
            .ok_or_else(closed)?;
        Ok(())
    }
    async fn matches(&self, conn: &mut Conn) -> Result<bool, EngineError> {
        let Some(host) = hosts::host(conn, self.host).await? else {
            return Ok(false);
        };
        let Some(unit) = envs::runtime(conn, self.runtime).await? else {
            return Ok(false);
        };
        let order: Option<i64> =
            sqlx::query_scalar("SELECT native_control_revision FROM runtimes WHERE id=$1")
                .bind(self.runtime)
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
        Ok(host.machine_id == self.machine
            && host.owner_id == self.owner
            && host.home_id == self.home
            && host.org_id == self.org
            && unit.host_id == self.host
            && unit.environment_id == self.environment
            && unit.provider_id == self.provider
            && self.registration.matches(conn, &self.control).await?
            && order == Some(self.target.revision)
            && orch_db::queries::runtime_controls::owns_current_source(conn, &self.control).await?)
    }
}

/// A single accepted Host unit phase; its metadata grants no permission or shell.
pub(crate) struct Phase {
    command: Id,
    input: Input,
}
impl Phase {
    /// The caller holds the Environment or Provider parent before this native runtime phase.
    pub(crate) async fn capture(
        conn: &mut Conn,
        host: Id,
        runtime: Id,
    ) -> Result<Option<Self>, EngineError> {
        let Some(source) = orch_db::queries::runtime_incarnations::current(conn, runtime)
            .await?
            .filter(|source| source.incarnation.is_some())
        else {
            return Ok(None);
        };
        let control =
            orch_db::queries::runtime_controls::accept(conn, runtime, orch_types::new_id()).await?;
        if control.source() != source {
            return Ok(None);
        }
        let command = orch_types::new_id();
        let Some(revision) = orch_db::queries::runtime_controls::phase(
            conn,
            &control,
            command,
            &serde_json::json!({"kind":"node.runtime.stop","runtime_id":runtime,"source":source,
                "body":{"runtime_id":runtime}}),
        )
        .await?
        else {
            return Ok(None);
        };
        let target = LifecycleTarget {
            pending: control.pending().to_vec(),
            subject: LifecycleSubject::Runtime {
                runtime_id: runtime,
            },
            observed: source.incarnation,
            opening: None,
            revision,
        };
        let input = Input::capture(conn, host, runtime, &control, &target).await?;
        Ok(Some(Self { command, input }))
    }
    pub(crate) async fn is_current(&self, conn: &mut Conn) -> Result<bool, EngineError> {
        self.input.matches(conn).await
    }
    /// Uses only the original receiver and raw Host operation; timeout confirms no physical state.
    pub(crate) async fn run(
        &self,
        engine: &crate::Engine,
    ) -> Result<Option<crate::commands::Outcome>, EngineError> {
        let mut outgoing = Outgoing::new(
            self.command,
            self.input.machine,
            SerialKey::Runtime(self.input.runtime),
            &NodeRuntimeStop {
                runtime_id: self.input.runtime,
                reason: None,
            },
        )?
        .with_native_target(NativeTarget::Lifecycle(self.input.target.clone()))
        .by(Time::new(
            engine.inner.clock.now()
                + chrono::Duration::seconds(i64::from(
                    CommandKind::NodeRuntimeStop.deadline_secs(),
                )),
        ));
        let http = http_origin::active().ok_or_else(closed)?;
        let mut checked = engine.inner.db.begin().await?;
        let native = Command::capture(
            &mut checked,
            engine.inner.clock.as_ref(),
            self.input.clone(),
            &outgoing,
        )
        .await?;
        checked.commit().await.map_err(orch_db::DbError::from)?;
        outgoing.authority = Some(http.command_source(
            &outgoing,
            crate::operation_scope::http_command_source::CommandEvidence::HostUnitStop(Box::new(
                native,
            )),
        )?);
        match tokio::time::timeout(
            engine.inner.config.hosting.stop_wait,
            crate::commands::run(engine.inner.commands.as_ref(), outgoing),
        )
        .await
        {
            Ok(Ok(outcome)) => {
                if let crate::commands::Outcome::Succeeded(result) = &outcome {
                    let mut tx = engine.inner.db.begin().await?;
                    self.input.lock(&mut tx).await?;
                    let bound = orch_db::queries::runtime_controls::bind_stopped(
                        &mut tx,
                        &self.input.control,
                        self.command,
                        result,
                        None,
                    )
                    .await?;
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    if bound.is_none() {
                        return Err(closed());
                    }
                }
                Ok(Some(outcome))
            }
            Ok(Err(error)) => Err(error.into()),
            Err(_) => Ok(None),
        }
    }
}
/// Retain the host manager's original authority alongside its selected provider unit.
pub(crate) async fn pending(
    conn: &mut Conn,
    clock: &dyn Clock,
    stop: &orch_db::queries::runtime_incarnations::Stop,
    previous: Option<&Command>,
) -> Result<Outgoing, EngineError> {
    let NativeTarget::Lifecycle(target) = super::provider_unit::target(stop) else {
        return Err(closed());
    };
    let input = if let Some(previous) = previous {
        Input {
            target: target.clone(),
            ..previous.input.clone()
        }
    } else {
        Input::capture(
            conn,
            stop.control.host(),
            stop.control.runtime(),
            &stop.control,
            &target,
        )
        .await?
    };
    if input.registration != stop.registration || input.control != stop.control {
        return Err(closed());
    }
    let mut outgoing = Outgoing::new(
        stop.id,
        stop.control.machine(),
        SerialKey::Runtime(stop.control.runtime()),
        &NodeRuntimeStop {
            runtime_id: stop.control.runtime(),
            reason: None,
        },
    )?
    .with_native_target(NativeTarget::Lifecycle(target))
    .by(Time::new(
        clock.now()
            + chrono::Duration::seconds(i64::from(CommandKind::NodeRuntimeStop.deadline_secs())),
    ));
    let http = http_origin::active().ok_or_else(closed)?;
    let source = Command::capture(conn, clock, input, &outgoing).await?;
    outgoing.authority = Some(http.command_source(
        &outgoing,
        crate::operation_scope::http_command_source::CommandEvidence::HostUnitStop(Box::new(
            source,
        )),
    )?);
    Ok(outgoing)
}

struct Native;
impl Source for Native {
    type Role = Role;
    type Input = Input;
    type Value = Input;
    const ROUTE: Route = HostRuntimeStop::ROUTE;
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
                    kind: ObjectKind::Host,
                    id: input.host,
                },
            )
            .await
            .map_err(EngineError::unreadable)?;
            Ok((
                input.clone(),
                vec![Facts::one(
                    Role::Host,
                    KeyTargetKind::Object(ObjectKind::Host),
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
                    kind: ObjectKind::Host,
                    id: input.host,
                },
                Permission::HostManage,
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

/// Canonical native source inside the one original HTTP evidence channel.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    origin: CapturedOperation,
    input: Input,
    canonical: Json,
    deadline: Time,
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostUnitStop([private])")
    }
}
impl Command {
    pub(crate) async fn capture(
        conn: &mut Conn,
        clock: &dyn Clock,
        input: Input,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        let http = http_origin::active().ok_or_else(closed)?;
        if http.origin().operation() != DECLARATION.operation
            || command.authority.is_some()
            || command.kind != CommandKind::NodeRuntimeStop
            || command.machine != input.machine
            || command.key != SerialKey::Runtime(input.runtime)
            || command.body
                != serde_json::to_value(NodeRuntimeStop {
                    runtime_id: input.runtime,
                    reason: None,
                })?
            || command.native_target.as_ref()
                != Some(&NativeTarget::Lifecycle(input.target.clone()))
        {
            return Err(closed());
        }
        let frame = http
            .prepare::<Native>(conn, clock, input.clone())
            .await
            .map_err(mapped)?;
        frame.check_deadline(clock).map_err(mapped)?;
        Ok(Self {
            origin: CapturedOperation::capture(frame.principal().caller()).map_err(|_| closed())?,
            input,
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
            deadline: command.deadline.ok_or_else(closed)?,
        })
    }
    pub(super) fn matches_stop(
        &self,
        provider: Id,
        stop: &orch_db::queries::runtime_incarnations::Stop,
        outgoing: &Outgoing,
    ) -> Result<bool, EngineError> {
        Ok(self.input.provider == Some(provider)
            && self.input.control == stop.control
            && self.input.registration == stop.registration
            && self.input.target.revision == stop.revision
            && outgoing.id == stop.id
            && outgoing.machine == stop.control.machine()
            && outgoing.native_target == Some(super::provider_unit::target(stop))
            && outgoing.deadline == Some(self.deadline)
            && outgoing.kind == CommandKind::NodeRuntimeStop
            && outgoing.key == SerialKey::Runtime(stop.control.runtime())
            && self.canonical
                == crate::jobs::command_identity::canonical(
                    outgoing.id,
                    outgoing.machine,
                    outgoing.key.wire().as_deref(),
                    outgoing.kind.as_str(),
                    &outgoing.body,
                )?)
    }
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
            || command.deadline_at != self.deadline.get()
            || clock.time() >= self.deadline
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
                content_transit: command.content_transit,
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
