//! Original explicit update request and exact native command, retained inside the existing drain fence.

use crate::{
    EngineError,
    commands::Outgoing,
    operation_scope::{
        continuing::{self, Error, Source},
        request_origin::{self, Credential},
        sources::{self, Facts},
        targets,
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    machine_sources::{self as declarations, Source as Role},
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetUsage, TargetResolver},
    resources::{HostUpdateNow, MachineUpdateNow},
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{
        component_updates as updates,
        link::{CommandRow, NewCommand},
        resources,
    },
};
use orch_link_contract::commands::{CommandKind, MachineUpdateNow as NativeUpdate, SerialKey};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

/// Immutable actual explicit request, distinct from a later click on the same machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    /// Newly allocated private request identity; never a permission target.
    pub id: Id,
    /// Actual physical program selected by the existing native helper.
    pub machine: Id,
    /// Server recommendation selected for this request.
    pub recommended: String,
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
        .filter(|row| row.is_active() && row.runtime_id.is_none())
        .ok_or(Error::Refused)?;
    Ok(Physical {
        machine: row.id,
        owner: row.owner_id,
        host: row.host_id,
        runtime: row.runtime_id,
        facade: resources::hosting::host_of_machine(conn, machine).await?,
    })
}
async fn locks(conn: &mut Conn, machine: Id) -> Result<(), EngineError> {
    updates::lock(conn, machine).await?;
    sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
        .bind(machine)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM hosts WHERE machine_id=$1 FOR SHARE")
        .bind(machine)
        .fetch_all(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}
trait Operation: Send + Sync + 'static {
    const ROUTE: Route;
    const DECLARATION: &'static Declaration<Role>;
    const FAMILY: KeyTargetKind;
}
struct Machine;
impl Operation for Machine {
    const ROUTE: Route = MachineUpdateNow::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::UPDATE_NOW;
    const FAMILY: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Machine);
}
const HOST: Declaration<Role> = Declaration {
    operation: "HostUpdateNow",
    bindings: &[Binding {
        source: Role::Governor,
        target: TargetResolver::Path("id"),
        usage: FacetUsage::Required,
        cardinality: Cardinality::One,
    }],
};
struct Host;
impl Operation for Host {
    const ROUTE: Route = HostUpdateNow::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &HOST;
    const FAMILY: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Host);
}
struct Native<F>(std::marker::PhantomData<F>);
impl<F: Operation> Source for Native<F> {
    type Role = Role;
    type Input = Physical;
    type Value = Physical;
    const ROUTE: Route = F::ROUTE;
    const DECLARATION: &'static Declaration<Role> = F::DECLARATION;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a Physical,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Physical, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let value = physical(conn, input.machine).await?;
            if &value != input
                || F::FAMILY == KeyTargetKind::Object(ObjectKind::Host) && value.facade.is_none()
            {
                return Err(Error::Refused);
            }
            let target = targets::machine_subject(conn, input.machine)
                .await
                .map_err(EngineError::unreadable)?;
            Ok((value, vec![Facts::one(Role::Governor, F::FAMILY, target)]))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Physical,
        _value: &'a Physical,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !orch_authz::resources::check_machine(
                conn,
                principal,
                input.machine,
                Permission::MachineManage,
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
        input: &'a Physical,
        _value: &'a Physical,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            locks(conn, input.machine).await?;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Purpose {
    Machine,
    Host,
}
impl Purpose {
    fn operation(self) -> &'static str {
        match self {
            Self::Machine => MachineUpdateNow::ROUTE.name,
            Self::Host => HostUpdateNow::ROUTE.name,
        }
    }
}

/// Private original explicit request. It contains no positive durable admission.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    origin: CapturedOperation,
    credential: Credential,
    until: Option<Time>,
    intent: Intent,
    physical: Physical,
    purpose: Purpose,
}
impl std::fmt::Debug for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ComponentUpdateOrigin([private])")
    }
}
impl Origin {
    /// The native row's actual private request identity, never a permission target.
    pub fn request_id(&self) -> Id {
        self.intent.id
    }
    /// Capture the actual Machine endpoint before its native request INSERT.
    pub async fn machine(
        conn: &mut Conn,
        caller: &Caller,
        credential: Credential,
        until: Option<Time>,
        intent: Intent,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        Self::capture::<Machine>(
            conn,
            caller,
            credential,
            until,
            intent,
            clock,
            Purpose::Machine,
        )
        .await
    }
    /// Capture the actual Host endpoint without relabeling its operation as a Machine act.
    pub async fn host(
        conn: &mut Conn,
        caller: &Caller,
        credential: Credential,
        until: Option<Time>,
        intent: Intent,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        Self::capture::<Host>(
            conn,
            caller,
            credential,
            until,
            intent,
            clock,
            Purpose::Host,
        )
        .await
    }
    async fn capture<F: Operation>(
        conn: &mut Conn,
        caller: &Caller,
        credential: Credential,
        until: Option<Time>,
        intent: Intent,
        clock: &dyn Clock,
        purpose: Purpose,
    ) -> Result<Self, EngineError> {
        if matches!(&credential, Credential::Native)
            || !credential.matches(caller)
            || caller
                .admitted_operation()
                .is_some_and(|operation| operation != F::ROUTE.name)
        {
            return Err(EngineError::unreadable(
                "the update request has another original issuer",
            ));
        }
        let physical = physical(conn, intent.machine)
            .await
            .map_err(EngineError::unreadable)?;
        let (_, facts) = Native::<F>::prepare(conn, caller, &physical, clock.time())
            .await
            .map_err(EngineError::unreadable)?;
        let caller = sources::bind_required(F::ROUTE, F::DECLARATION, &facts)
            .map_err(EngineError::unreadable)?
            .admit(caller, false)
            .map_err(EngineError::unreadable)?;
        let result = Self {
            origin: CapturedOperation::capture(&caller).map_err(EngineError::unreadable)?,
            credential,
            until,
            intent,
            physical,
            purpose,
        };
        if result.current_source(conn, clock).await?.is_none() {
            return Err(EngineError::unreadable(
                "the original update issuer has ended",
            ));
        }
        Ok(result)
    }
    /// Same actual native subject under the original/current raw credential and scope.
    pub async fn current_source(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<(Caller, Option<Time>)>, EngineError> {
        if matches!(&self.credential, Credential::Native) {
            return Ok(None);
        }
        let (caller, until) = match request_origin::current(
            conn,
            clock,
            &self.origin,
            &self.credential,
            self.until,
            self.purpose.operation(),
        )
        .await
        {
            Ok(value) => value,
            Err(Error::Refused | Error::Binding(_)) => return Ok(None),
            Err(Error::Native(error)) => return Err(error),
        };
        match self.purpose {
            Purpose::Machine => self.frame::<Machine>(conn, clock, &caller, until).await,
            Purpose::Host => self.frame::<Host>(conn, clock, &caller, until).await,
        }
    }
    async fn frame<F: Operation>(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        caller: &Caller,
        until: Option<Time>,
    ) -> Result<Option<(Caller, Option<Time>)>, EngineError> {
        let frame = match continuing::prepare_with_caller::<Native<F>>(
            conn,
            clock,
            &self.origin,
            caller,
            self.physical.clone(),
        )
        .await
        {
            Ok(frame) => frame,
            Err(Error::Refused | Error::Binding(_)) => return Ok(None),
            Err(Error::Native(error)) => return Err(error),
        };
        let caller = frame.principal().caller().clone();
        Ok((caller.key_deadline_allows(clock)
            && until.is_none_or(|until| until.get() > clock.now()))
        .then_some((caller, until)))
    }
    /// The actual still-live request must still contain this exact native origin and recommendation.
    pub async fn current_request(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        command: Option<Id>,
    ) -> Result<Option<(Caller, Option<Time>)>, EngineError> {
        updates::lock(conn, self.intent.machine).await?;
        if !updates::matches_issuer(
            conn,
            self.intent.machine,
            self.intent.id,
            &self.intent.recommended,
            &serde_json::to_value(self)?,
            command,
        )
        .await?
        {
            return Ok(None);
        }
        let Some((caller, until)) = self.current_source(conn, clock).await? else {
            return Ok(None);
        };
        Ok((caller.key_deadline_allows(clock)
            && until.is_none_or(|until| until.get() > clock.now()))
        .then_some((caller, until)))
    }
    /// Prepare the same idempotent physical update command from this actual request's persisted grant.
    pub fn command(&self, command: &Outgoing) -> Result<Command, EngineError> {
        let body: NativeUpdate = serde_json::from_value(command.body.clone())?;
        if command.machine != self.intent.machine
            || command.kind != CommandKind::MachineUpdateNow
            || command.key != SerialKey::Machine
            || body.recommended != self.intent.recommended
            || body.automatic
        {
            return Err(EngineError::unreadable(
                "the native update command changed its request",
            ));
        }
        Ok(Command {
            request: self.clone(),
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
/// One exact original-request command inside the existing private command authority channel.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    request: Origin,
    canonical: Json,
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ComponentUpdateCommand([private])")
    }
}
impl Command {
    /// Take the existing drain/physical footprint before its canonical command row.
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        locks(conn, self.request.intent.machine).await
    }
    /// Current original source at the actual first durable command insertion.
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != "machine.update.now"
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
            .request
            .current_request(conn, clock, Some(command.id))
            .await?
            .map(|(caller, until)| vec![(Some(caller), until)]))
    }
    /// Current original source on each still-unaccepted send; accepted reconciliation stays native.
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

/// Existing Host and explicit Machine origins share the same native request and drain reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestOrigin {
    /// Actual raw-credential explicit request.
    Explicit(Box<Origin>),
    /// Existing Host producer and its unchanged source subtype.
    Host(Box<crate::component_update_origin::Origin>),
}
/// Native absence is distinct from damaged known provenance.
pub enum StoredOrigin {
    Legacy,
    Bound(RequestOrigin),
    Invalid,
}
impl RequestOrigin {
    /// Classify the one current ledger, with the same required flag and immutable request identity.
    pub async fn load(conn: &mut Conn, machine: Id) -> Result<StoredOrigin, DbError> {
        let Some(row) = updates::origin(conn, machine).await? else {
            return Ok(StoredOrigin::Invalid);
        };
        match (row.origin_required, row.origin.as_ref()) {
            (false, None) => Ok(StoredOrigin::Legacy),
            (true, Some(value)) => {
                if let Ok(origin) = serde_json::from_value::<Origin>(value.clone()) {
                    return Ok(
                        if origin.intent.machine == row.machine_id
                            && origin.physical.machine == row.machine_id
                            && origin.intent.id == row.request_id
                            && !row.request_id.is_nil()
                            && origin.intent.recommended == row.recommended_version
                            && Some(origin.origin.user()) == row.requested_by
                            && origin.origin.operation() == origin.purpose.operation()
                        {
                            StoredOrigin::Bound(Self::Explicit(Box::new(origin)))
                        } else {
                            StoredOrigin::Invalid
                        },
                    );
                }
                Ok(
                    match crate::component_update_origin::Origin::load(conn, machine).await? {
                        crate::component_update_origin::StoredOrigin::Bound(origin) => {
                            StoredOrigin::Bound(Self::Host(Box::new(origin)))
                        }
                        _ => StoredOrigin::Invalid,
                    },
                )
            }
            _ => Ok(StoredOrigin::Invalid),
        }
    }
    /// Fresh original/current native authority on the same actual request.
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<(Caller, Option<Time>)>, EngineError> {
        match self {
            Self::Explicit(origin) => origin.current_request(conn, clock, None).await,
            Self::Host(origin) => origin.current(conn, clock).await,
        }
    }
    /// Retain one exact command without changing either producer's private wire shape.
    pub fn command(&self, command: &Outgoing) -> Result<UpdateCommand, EngineError> {
        match self {
            Self::Explicit(origin) => origin
                .command(command)
                .map(|command| UpdateCommand::Explicit(Box::new(command))),
            Self::Host(origin) => origin
                .command(command)
                .map(|command| UpdateCommand::Host(Box::new(command))),
        }
    }
}

/// The one component command family preserves both concrete native source encodings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UpdateCommand {
    /// Existing Host command data, serialized unchanged.
    Host(Box<crate::component_update_origin::UpdateCommand>),
    /// Explicit raw-credential command data, serialized unchanged.
    Explicit(Box<Command>),
}
impl From<crate::component_update_origin::UpdateCommand> for UpdateCommand {
    fn from(value: crate::component_update_origin::UpdateCommand) -> Self {
        Self::Host(Box::new(value))
    }
}
impl From<Command> for UpdateCommand {
    fn from(value: Command) -> Self {
        Self::Explicit(Box::new(value))
    }
}
impl UpdateCommand {
    /// Native drain/source locks precede the canonical command row.
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        match self {
            Self::Host(value) => value.lock_source(conn).await,
            Self::Explicit(value) => value.lock_source(conn).await,
        }
    }
    /// The exact original native source is rechecked at first durable insertion.
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        match self {
            Self::Host(value) => value.current_issue_callers(conn, command, clock).await,
            Self::Explicit(value) => value.current_issue_callers(conn, command, clock).await,
        }
    }
    /// Recheck the same concrete source at each still-unaccepted publication.
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        match self {
            Self::Host(value) => value.current_callers(conn, command, clock).await,
            Self::Explicit(value) => value.current_callers(conn, command, clock).await,
        }
    }
}
