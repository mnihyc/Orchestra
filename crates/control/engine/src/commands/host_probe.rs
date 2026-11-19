//! The native Host Get proposal, retaining its actual reader and physical Host association.

use super::{Outgoing, authority::session_state};
use crate::EngineError;
use crate::operation_scope::{
    continuing::{self, Error, Source},
    sources::{self, Facts},
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    operation_sources::Declaration,
    resource_reads::{self, Source as Role},
    resources::HostGet,
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::link::{CommandRow, NewCommand},
};
use orch_link_contract::{
    commands::{CommandKind, SerialKey},
    ports::HostPortsCheck,
};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Physical {
    host: Id,
    machine: Id,
}

struct Proposal;
impl Source for Proposal {
    type Role = Role;
    type Input = Physical;
    type Value = Physical;
    const ROUTE: Route = HostGet::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &resource_reads::HOST_GET;

    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Physical,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Physical, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let row =
                orch_db::queries::resources::routes::hosts(conn, &[input.host], caller.user())
                    .await?
                    .pop()
                    .ok_or(Error::Refused)?;
            if row.machine_id != input.machine {
                return Err(Error::Refused);
            }
            if orch_db::queries::resources::machine(conn, row.machine_id)
                .await?
                .is_none_or(|machine| !machine.is_active())
            {
                return Err(Error::Refused);
            }
            let target = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: row.id,
                },
            )
            .await
            .map_err(EngineError::unreadable)?;
            let mut facts = vec![Facts::one(
                Role::Primary,
                KeyTargetKind::Object(ObjectKind::Host),
                target,
            )];
            // This actual proposal command has no foreign content or named-place operand.
            // The HTTP body retains its own complete native source projection separately.
            facts.extend(
                Self::DECLARATION
                    .bindings
                    .iter()
                    .filter(|binding| binding.source != Role::Primary)
                    .map(|binding| Facts::collection(binding.source, Vec::new())),
            );
            Ok((
                Physical {
                    host: row.id,
                    machine: row.machine_id,
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Physical,
        _value: &'a Physical,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            for permission in [Permission::HostView, Permission::HostManage] {
                if !orch_authz::resources::check(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Host,
                        id: input.host,
                    },
                    permission,
                    Policy::default(),
                )
                .await?
                .is_allowed()
                {
                    return Err(Error::Refused);
                }
            }
            Ok(())
        })
    }

    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Physical,
        _value: &'a Physical,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let host = sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR SHARE")
                .bind(input.host)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let machine = sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                .bind(input.machine)
                .fetch_optional(conn)
                .await
                .map_err(DbError::from)?;
            if host.is_none() || machine.is_none() {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }

    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Physical,
        opened: &'a Physical,
        fresh: &'a Physical,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}

/// Private provenance of one native read-only proposal; never part of the connector body.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostPortsProbe {
    origin: CapturedOperation,
    session: Option<Id>,
    raw_deadline: Option<Time>,
    physical: Physical,
    canonical: Json,
}
impl std::fmt::Debug for HostPortsProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostPortsProbe([private])")
    }
}
impl HostPortsProbe {
    /// Prepare the same actual Host reader under the original/current HTTP ceiling.
    /// The caller owns this transaction; capture performs no native IO or queue mutation.
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        session: Option<Id>,
        raw_deadline: Option<Time>,
        host: Id,
        command: &Outgoing,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        if session.is_some() == caller.grant().is_some()
            || caller
                .admitted_operation()
                .is_some_and(|operation| operation != "HostGet")
        {
            return Err(EngineError::unreadable(
                "the Host reader has another credential origin",
            ));
        }
        if command.kind != CommandKind::HostPorts
            || command.key != SerialKey::Machine
            || command.body
                != serde_json::to_value(HostPortsCheck {
                    ports: None,
                    revision: 0,
                })?
        {
            return Err(EngineError::unreadable(
                "this is not the reader's native proposal command",
            ));
        }
        let physical = Physical {
            host,
            machine: command.machine,
        };
        let (_, facts) = Proposal::prepare(conn, caller, &physical, clock.time())
            .await
            .map_err(EngineError::unreadable)?;
        let caller = sources::bind_required(Proposal::ROUTE, Proposal::DECLARATION, &facts)
            .map_err(EngineError::unreadable)?
            .admit(caller, false)
            .map_err(EngineError::unreadable)?;
        let value = Self {
            origin: CapturedOperation::capture(&caller).map_err(EngineError::unreadable)?,
            session,
            raw_deadline,
            physical,
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        };
        if value.current(conn, clock).await?.is_none() {
            return Err(EngineError::unreadable(
                "the Host proposal reader is no longer current",
            ));
        }
        Ok(value)
    }

    /// Current native reader and raw session lifetime, also used before caching its answer.
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<(Caller, Option<Time>)>, EngineError> {
        let Some(session) = session_state(conn, &self.origin, self.session, clock.time()).await?
        else {
            return Ok(None);
        };
        if self.origin.operation() != "HostGet"
            || self.session.is_some() && self.raw_deadline.is_none()
        {
            return Ok(None);
        }
        let Some(caller) = self.origin.restore_current(conn, clock).await? else {
            return Ok(None);
        };
        if self.session.is_some() == caller.grant().is_some() {
            return Ok(None);
        }
        let enabled = caller.as_admin() && session.is_some_and(|(admin, _)| admin);
        let caller = caller.with_admin_mode(enabled);
        let frame = match continuing::prepare_with_caller::<Proposal>(
            conn,
            clock,
            &self.origin,
            &caller,
            self.physical.clone(),
        )
        .await
        {
            Ok(frame) => frame,
            Err(Error::Refused | Error::Binding(_)) => return Ok(None),
            Err(Error::Native(error)) => return Err(error),
        };
        let caller = frame.principal().caller().clone();
        let deadline = match (session.map(|(_, until)| until), self.raw_deadline) {
            (Some(session), Some(raw)) => Some(session.min(raw)),
            (session, raw) => session.or(raw),
        };
        Ok((caller.key_deadline_allows(clock)
            && deadline.is_none_or(|until| until.get() > clock.now()))
        .then_some((caller, deadline)))
    }

    pub(super) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != "host.ports"
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
        Ok(self
            .current(conn, clock)
            .await?
            .map(|(caller, until)| vec![(Some(caller), until)]))
    }

    pub(super) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_issue_callers(
            conn,
            &NewCommand {
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
