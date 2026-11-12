//! Original HTTP issuer and immutable native replacement-token association at redemption.

use crate::{
    EngineError,
    operation_scope::{
        continuing::{self, Error, Source},
        request_origin::{self, Credential},
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
    machine_sources::{self as declarations, Source as Role},
    operation_sources::Declaration,
    resources::MachineEnrollmentAgainTokenCreate,
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{Conn, DbError, queries::resources};
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};

/// Factual reservation fields from the existing native one-use token ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgainToken {
    /// Actual retained physical machine, never a generated child.
    pub machine: Id,
    /// Native secret digest, not the token itself.
    pub hash: Vec<u8>,
    /// Actual stored issuer.
    pub issuer: Id,
    /// Native host/connected purpose.
    pub purpose: String,
    /// Actual reservation deadline.
    pub expires: Time,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Physical {
    machine: Id,
    owner: Option<Id>,
    host: Option<Id>,
    runtime: Option<Id>,
    facade: Option<Id>,
}
async fn physical(conn: &mut Conn, machine: Id) -> Result<Physical, Error> {
    let row = resources::machine(conn, machine)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.lost_at.is_none())
        .ok_or(Error::Refused)?;
    Ok(Physical {
        machine: row.id,
        owner: row.owner_id,
        host: row.host_id,
        runtime: row.runtime_id,
        facade: resources::reenrollment::host_of(conn, machine).await?,
    })
}
struct Replacement;
impl Source for Replacement {
    type Role = Role;
    type Input = Physical;
    type Value = Physical;
    const ROUTE: Route = MachineEnrollmentAgainTokenCreate::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::AGAIN;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a Physical,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Physical, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let current = physical(conn, input.machine).await?;
            if &current != input {
                return Err(Error::Refused);
            }
            let target = targets::machine_subject(conn, input.machine)
                .await
                .map_err(EngineError::unreadable)?;
            Ok((
                current,
                vec![Facts::one(
                    Role::Governor,
                    KeyTargetKind::Object(ObjectKind::Machine),
                    target,
                )],
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
            let allowed = match input.facade {
                Some(host) => orch_authz::resources::check(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Host,
                        id: host,
                    },
                    Permission::HostManage,
                    Policy::default(),
                )
                .await?
                .is_allowed(),
                None => orch_authz::resources::check_machine(
                    conn,
                    principal,
                    input.machine,
                    Permission::AccessOwn,
                    Policy::default(),
                )
                .await?
                .is_allowed(),
            };
            if !allowed {
                return Err(Error::Refused);
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
            sqlx::query("SELECT id FROM machines WHERE id=$1 FOR UPDATE")
                .bind(input.machine)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM hosts WHERE machine_id=$1 FOR SHARE")
                .bind(input.machine)
                .fetch_all(conn)
                .await
                .map_err(DbError::from)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Physical,
        old: &'a Physical,
        current: &'a Physical,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(old == current) })
    }
}

/// Private issuer packet. It restores actual raw/current authority; no serialized flag admits work.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgainOrigin {
    origin: CapturedOperation,
    credential: Credential,
    until: Option<Time>,
    token: AgainToken,
    physical: Physical,
}
impl std::fmt::Debug for AgainOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AgainOrigin([private])")
    }
}
impl AgainOrigin {
    /// Capture real native reservation facts under this actual original/current HTTP actor.
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        credential: Credential,
        until: Option<Time>,
        token: AgainToken,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        if matches!(&credential, Credential::Native)
            || !credential.matches(caller)
            || token.issuer != caller.user()
            || caller
                .admitted_operation()
                .is_some_and(|operation| operation != Replacement::ROUTE.name)
        {
            return Err(EngineError::unreadable(
                "the replacement token has another issuer origin",
            ));
        }
        let actual = physical(conn, token.machine)
            .await
            .map_err(EngineError::unreadable)?;
        if token.purpose
            != if actual.facade.is_some() {
                "host"
            } else {
                "connected"
            }
        {
            return Err(EngineError::unreadable(
                "the replacement token purpose changed",
            ));
        }
        let (_, facts) = Replacement::prepare(conn, caller, &actual, clock.time())
            .await
            .map_err(EngineError::unreadable)?;
        let caller = sources::bind_required(Replacement::ROUTE, Replacement::DECLARATION, &facts)
            .map_err(EngineError::unreadable)?
            .admit(caller, false)
            .map_err(EngineError::unreadable)?;
        let result = Self {
            origin: CapturedOperation::capture(&caller).map_err(EngineError::unreadable)?,
            credential,
            until,
            token,
            physical: actual,
        };
        if result.current(conn, clock).await?.is_none() {
            return Err(EngineError::unreadable(
                "the replacement token issuer is no longer current",
            ));
        }
        Ok(result)
    }
    /// Match the actual locked ledger record, never the enrolling client's subject.
    pub fn matches(&self, token: &AgainToken) -> bool {
        self.token.machine == token.machine
            && self.token.hash == token.hash
            && self.token.issuer == token.issuer
            && self.token.purpose == token.purpose
            && self.token.expires.get().timestamp_micros() == token.expires.get().timestamp_micros()
            && self.origin.user() == token.issuer
            && self.physical.machine == token.machine
    }
    /// Current exact original issuer, ordinary ownership and key/raw lifetime on this transaction.
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<(Caller, Option<Time>)>, EngineError> {
        if matches!(&self.credential, Credential::Native) || self.token.expires.get() <= clock.now()
        {
            return Ok(None);
        }
        let (caller, until) = match request_origin::current(
            conn,
            clock,
            &self.origin,
            &self.credential,
            self.until,
            Replacement::ROUTE.name,
        )
        .await
        {
            Ok(value) => value,
            Err(Error::Refused | Error::Binding(_)) => return Ok(None),
            Err(Error::Native(error)) => return Err(error),
        };
        let frame = match continuing::prepare_with_caller::<Replacement>(
            conn,
            clock,
            &self.origin,
            &caller,
            self.physical.clone(),
        )
        .await
        {
            Ok(value) => value,
            Err(Error::Refused | Error::Binding(_)) => return Ok(None),
            Err(Error::Native(error)) => return Err(error),
        };
        let caller = frame.principal().caller().clone();
        let until = Some(until.map_or(self.token.expires, |until| until.min(self.token.expires)));
        Ok((caller.key_deadline_allows(clock)
            && until.is_none_or(|until| until.get() > clock.now()))
        .then_some((caller, until)))
    }
}
