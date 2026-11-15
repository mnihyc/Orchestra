//! A manual Host update's exact request, original actor and physical program across drain waits.

use crate::{
    EngineError,
    commands::Outgoing,
    operation_scope::{
        continuing::{self, Error, Source},
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
    host_actions::{self, Source as Role},
    operation_sources::Declaration,
    resources::HostUpdateNow,
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{
        component_updates as rows,
        link::{CommandRow, NewCommand},
    },
};
use orch_link_contract::commands::{CommandKind, MachineUpdateNow, SerialKey};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

/// Original actual manual request; no field is a reusable permission or model lease.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    origin: CapturedOperation,
    request: Id,
    host: Id,
    machine: Id,
    recommended: String,
    session: Option<Id>,
    until: Option<Time>,
}
impl std::fmt::Debug for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ComponentUpdateOrigin([private])")
    }
}

/// Absence is only pre-existing/system work; damaged known provenance is explicit refusal.
#[derive(Debug)]
pub enum StoredOrigin {
    /// Existing independent automatic or legacy request.
    Legacy,
    /// One exactly recorded manual operation.
    Bound(Origin),
    /// Known provenance cannot become an automatic request.
    Invalid,
}

impl Origin {
    /// Capture only the newly filed native Host request on its actual guarded connection.
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        host: Id,
        machine: Id,
        session: Option<Id>,
        until: Option<Time>,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        if origin.operation() != host_actions::UPDATE_NOW.operation
            || session.is_some() == caller.grant().is_some()
            || session.is_some() && until.is_none()
            || !orch_db::queries::transaction_guard::in_transaction(conn)
        {
            return Err(EngineError::unreadable(
                "the physical update has another request origin",
            ));
        }
        let row = rows::origin(conn, machine)
            .await?
            .ok_or_else(|| EngineError::unreadable("the native update request is absent"))?;
        if row.requested_by != Some(caller.user())
            || row.origin_required
            || row.origin.is_some()
            || !matches!(row.state.as_str(), "queued" | "refused")
        {
            return Err(EngineError::unreadable(
                "the update request already belongs to another act",
            ));
        }
        let host_row = orch_db::queries::resources::routes::hosts(conn, &[host], caller.user())
            .await?
            .pop()
            .filter(|row| row.machine_id == machine)
            .ok_or_else(|| EngineError::unreadable("the Host has another physical program"))?;
        let target = targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Host,
                id: host_row.id,
            },
        )
        .await
        .map_err(EngineError::unreadable)?;
        let facts = [Facts::one(
            Role::Host,
            KeyTargetKind::Object(ObjectKind::Host),
            target,
        )];
        let required =
            sources::bind_required(HostUpdateNow::ROUTE, &host_actions::UPDATE_NOW, &facts)
                .map_err(EngineError::unreadable)?
                .required_facets()
                .map_err(EngineError::unreadable)?;
        let admitted = caller.admitted_facets().unwrap_or(&[]);
        if !required.iter().all(|fact| admitted.contains(fact)) || !caller.permits_facets(admitted)
        {
            return Err(EngineError::unreadable(
                "the physical update was not admitted at its actual Host",
            ));
        }
        Ok(Self {
            origin,
            request: row.request_id,
            host,
            machine,
            recommended: row.recommended_version,
            session,
            until,
        })
    }

    /// Read and classify the actual private ledger, never infer a new session from its account.
    pub async fn load(conn: &mut Conn, machine: Id) -> Result<StoredOrigin, DbError> {
        let Some(row) = rows::origin(conn, machine).await? else {
            return Ok(StoredOrigin::Invalid);
        };
        match (row.origin_required, row.origin) {
            (false, None) => Ok(StoredOrigin::Legacy),
            (true, Some(value)) => {
                let Ok(origin) = serde_json::from_value::<Self>(value) else {
                    return Ok(StoredOrigin::Invalid);
                };
                Ok(
                    if origin.machine == row.machine_id
                        && origin.request == row.request_id
                        && origin.recommended == row.recommended_version
                        && Some(origin.origin.user()) == row.requested_by
                        && origin.origin.operation() == host_actions::UPDATE_NOW.operation
                        && !origin.request.is_nil()
                    {
                        StoredOrigin::Bound(origin)
                    } else {
                        StoredOrigin::Invalid
                    },
                )
            }
            _ => Ok(StoredOrigin::Invalid),
        }
    }

    /// Persist only on the fresh actual request, within the same accepted SQL transaction.
    pub async fn store(&self, conn: &mut Conn) -> Result<(), EngineError> {
        if !rows::bind_origin(
            conn,
            self.request,
            self.machine,
            &serde_json::to_value(self)?,
        )
        .await?
        {
            return Err(EngineError::unreadable(
                "the fresh physical request no longer belongs to this act",
            ));
        }
        Ok(())
    }

    /// Current original authority before a deferred grant, without network or model IO.
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<(Caller, Option<Time>)>, EngineError> {
        self.current_for(conn, clock, None).await
    }
    async fn current_for(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        command: Option<Id>,
    ) -> Result<Option<(Caller, Option<Time>)>, EngineError> {
        let Some(session) = crate::commands::authority::session_state(
            conn,
            &self.origin,
            self.session,
            clock.time(),
        )
        .await?
        else {
            return Ok(None);
        };
        let Some(caller) = self.origin.restore_current(conn, clock).await? else {
            return Ok(None);
        };
        if self.session.is_some() == caller.grant().is_some()
            || self.session.is_some() && self.until.is_none()
        {
            return Ok(None);
        }
        let caller = caller
            .clone()
            .with_admin_mode(caller.as_admin() && session.is_some_and(|(admin, _)| admin));
        let frame = match continuing::prepare_with_caller::<HostRequest>(
            conn,
            clock,
            &self.origin,
            &caller,
            Input {
                origin: self.clone(),
                command,
            },
        )
        .await
        {
            Ok(frame) => frame,
            Err(Error::Refused | Error::Binding(_)) => return Ok(None),
            Err(Error::Native(error)) => return Err(error),
        };
        let caller = frame.principal().caller().clone();
        if frame.into_parts().first_effect.check(clock).is_err() {
            return Ok(None);
        }
        let until = match (self.until, session.map(|(_, until)| until)) {
            (Some(original), Some(current)) => Some(original.min(current)),
            (original, current) => original.or(current),
        };
        Ok((caller.key_deadline_allows(clock)
            && until.is_none_or(|until| until.get() > clock.now()))
        .then_some((caller, until)))
    }

    /// The exact native body retained after the drain granted its real command identity.
    pub fn command(&self, command: &Outgoing) -> Result<UpdateCommand, EngineError> {
        if command.kind != CommandKind::MachineUpdateNow
            || command.machine != self.machine
            || command.key != SerialKey::Machine
            || command.body
                != serde_json::to_value(MachineUpdateNow {
                    recommended: self.recommended.clone(),
                    automatic: false,
                })?
        {
            return Err(EngineError::unreadable(
                "this is not the original physical update command",
            ));
        }
        Ok(UpdateCommand {
            origin: self.clone(),
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }
}

struct Input {
    origin: Origin,
    command: Option<Id>,
}
#[derive(PartialEq, Eq)]
struct Association {
    request: Id,
    host: Id,
    machine: Id,
    recommended: String,
}
struct HostRequest;
impl Source for HostRequest {
    type Role = Role;
    type Input = Input;
    type Value = Association;
    const ROUTE: Route = HostUpdateNow::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &host_actions::UPDATE_NOW;
    fn prepare<'a>(
        conn: &'a mut Conn,
        caller: &'a Caller,
        input: &'a Input,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Association, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let Some(row) = rows::origin(conn, input.origin.machine).await? else {
                return Err(Error::Refused);
            };
            if row.request_id != input.origin.request
                || row.requested_by != Some(caller.user())
                || !row.origin_required
                || row.origin
                    != Some(serde_json::to_value(&input.origin).map_err(EngineError::from)?)
                || row.recommended_version != input.origin.recommended
                || !matches!(row.state.as_str(), "queued" | "updating")
                || input
                    .command
                    .is_some_and(|command| row.command_id != Some(command))
            {
                return Err(Error::Refused);
            }
            let host = orch_db::queries::resources::routes::hosts(
                conn,
                &[input.origin.host],
                caller.user(),
            )
            .await?
            .pop()
            .filter(|row| row.machine_id == input.origin.machine)
            .ok_or(Error::Refused)?;
            if orch_db::queries::resources::machine(conn, input.origin.machine)
                .await?
                .is_none_or(|row| !row.is_active() || row.runtime_id.is_some())
            {
                return Err(Error::Refused);
            }
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: host.id,
                },
            )
            .await
            .map_err(EngineError::unreadable)?;
            Ok((
                Association {
                    request: row.request_id,
                    host: host.id,
                    machine: host.machine_id,
                    recommended: row.recommended_version,
                },
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
        _value: &'a Association,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: input.origin.host,
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
        _principal: &'a Principal,
        input: &'a Input,
        _value: &'a Association,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            rows::lock(conn, input.origin.machine).await?;
            let _ = sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR SHARE")
                .bind(input.origin.host)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let _ = sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                .bind(input.origin.machine)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let _ = sqlx::query(
                "SELECT machine_id FROM component_updates WHERE machine_id=$1 FOR SHARE",
            )
            .bind(input.origin.machine)
            .fetch_optional(conn)
            .await
            .map_err(DbError::from)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Input,
        opened: &'a Association,
        fresh: &'a Association,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}

/// Actual original request beside a canonical native update, with no authority on the wire.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateCommand {
    origin: Origin,
    canonical: Json,
}
impl std::fmt::Debug for UpdateCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ComponentUpdateCommand([private])")
    }
}
impl UpdateCommand {
    /// Take the same physical drain fence before locking its canonical command row.
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        rows::lock(conn, self.origin.machine).await?;
        Ok(())
    }

    /// Rechecks the same native grant after queue preparation and before its first insert.
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != CommandKind::MachineUpdateNow.as_str()
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
            .origin
            .current_for(conn, clock, Some(command.id))
            .await?
            .map(|(caller, until)| vec![(Some(caller), until)]))
    }
    /// The same original/current gate at actual unaccepted native delivery.
    pub async fn current_callers(
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
