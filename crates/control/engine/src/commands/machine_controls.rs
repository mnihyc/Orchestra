//! Exact native physical machine operations, retaining their original credential and command.

use super::Outgoing;
use crate::EngineError;
use crate::operation_scope::{
    continuing::{self, Error, Source},
    request_origin::{self, Credential},
    sources::{self, Facts},
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    machine_sources::{self as declarations, Source as Role},
    operation_sources::Declaration,
    resources::{
        HostUpdate, MachinePolicyUpdate, MachinePolicyUpdateBody, MachineProbe,
        MachineProgramsChange, MachineRolesSet,
    },
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{
        link::{CommandRow, NewCommand},
        resources,
    },
};
use orch_link_contract::commands::{
    CommandKind, MachinePolicySet, MachineRolesSet as NativeRolesSet, SerialKey,
};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

mod policy;
mod roles;
pub use policy::{
    PolicyFailure, PolicyPreparation, driver_configuration_faults, policy_key_about,
    policy_sections, prepare_policy, validate_policy_drivers,
};
pub use roles::role_change_refusal;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Physical {
    machine: Id,
    owner: Option<Id>,
    host: Option<Id>,
    runtime: Option<Id>,
    facade: Option<Id>,
}
impl Physical {
    async fn load(conn: &mut Conn, machine: Id) -> Result<Self, Error> {
        let row = resources::machine(conn, machine)
            .await?
            .filter(|row| row.is_active())
            .ok_or(Error::Refused)?;
        Ok(Self {
            machine: row.id,
            owner: row.owner_id,
            host: row.host_id,
            runtime: row.runtime_id,
            facade: resources::hosting::host_of_machine(conn, machine).await?,
        })
    }
}
trait Operation: Send + Sync + 'static {
    const ROUTE: Route;
    const DECLARATION: &'static Declaration<Role>;
    const KIND: CommandKind;
    const EXCLUSIVE: bool = false;
    const REQUIRE_VIEW: bool = true;
    const FAMILY: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Machine);
    fn valid_body(body: &Json) -> bool;
    fn policy_request(_body: &Json) -> Option<MachinePolicyUpdateBody> {
        None
    }
    fn roles_request(_body: &Json) -> Option<NativeRolesSet> {
        None
    }
    fn require_request<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _request: &'a Request,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Probe;
impl Operation for Probe {
    const ROUTE: Route = MachineProbe::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::PROBE;
    const KIND: CommandKind = CommandKind::MachineProbe;
    fn valid_body(body: &Json) -> bool {
        body == &serde_json::json!({})
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Programs;
impl Operation for Programs {
    const ROUTE: Route = MachineProgramsChange::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::PROGRAMS;
    const KIND: CommandKind = CommandKind::MachinePrograms;
    fn valid_body(body: &Json) -> bool {
        serde_json::from_value::<orch_api_contract::machine_programs::MachineProgramsCommand>(
            body.clone(),
        )
        .is_ok()
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Roles;
impl Operation for Roles {
    const ROUTE: Route = MachineRolesSet::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::ROLES;
    const KIND: CommandKind = CommandKind::MachineRolesSet;
    const EXCLUSIVE: bool = true;
    fn valid_body(body: &Json) -> bool {
        Self::roles_request(body).is_some_and(|request| {
            orch_link_contract::enrollment_choices::connector_roles(&request.roles.join(","))
                .is_ok_and(|roles| roles.len() == request.roles.len())
        })
    }
    fn roles_request(body: &Json) -> Option<NativeRolesSet> {
        serde_json::from_value(body.clone()).ok()
    }
    fn require_request<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        request: &'a Request,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let roles = request.roles.as_ref().ok_or(Error::Refused)?;
            if role_change_refusal(conn, request.physical.machine, &roles.roles)
                .await?
                .is_some()
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Gateway;
impl Operation for Gateway {
    const ROUTE: Route = orch_api_contract::gateways::MachineGatewaySet::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::GATEWAY;
    const KIND: CommandKind = CommandKind::MachineGatewaySet;
    const EXCLUSIVE: bool = true;
    const REQUIRE_VIEW: bool = false;
    fn valid_body(body: &Json) -> bool {
        serde_json::from_value::<orch_link_contract::commands::MachineGatewaySet>(body.clone())
            .is_ok_and(|request| match request.choice.selection {
                orch_link_contract::gateways::GatewaySelection::Automatic => {
                    request.choice.gateway_id.is_none()
                }
                orch_link_contract::gateways::GatewaySelection::Selected => request
                    .choice
                    .gateway_id
                    .as_ref()
                    .is_some_and(|id| !id.is_empty()),
            })
    }
}
#[derive(Clone, PartialEq, Eq)]
struct PolicyChange;
impl Operation for PolicyChange {
    const ROUTE: Route = MachinePolicyUpdate::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::POLICY;
    const KIND: CommandKind = CommandKind::MachinePolicySet;
    const EXCLUSIVE: bool = true;
    const REQUIRE_VIEW: bool = false;
    fn valid_body(body: &Json) -> bool {
        Self::policy_request(body).is_some_and(|values| !values.is_empty())
    }
    fn policy_request(body: &Json) -> Option<MachinePolicyUpdateBody> {
        serde_json::from_value::<MachinePolicySet>(body.clone())
            .ok()
            .map(|request| request.values)
    }
    fn require_request<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        request: &'a Request,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let values = request.policy.as_ref().ok_or(Error::Refused)?;
            let prepared =
                policy::prepare_policy(conn, principal, request.physical.machine, values)
                    .await
                    .map_err(|error| match error {
                        PolicyFailure::Native(error) => Error::Native(error),
                        _ => Error::Refused,
                    })?;
            policy::validate_policy_drivers(conn, &prepared)
                .await
                .map_err(|error| match error {
                    PolicyFailure::Native(error) => Error::Native(error),
                    _ => Error::Refused,
                })
        })
    }
}
const HOST_POLICY: Declaration<Role> = Declaration {
    operation: "HostUpdate",
    bindings: &[orch_api_contract::operation_sources::Binding {
        source: Role::Governor,
        target: orch_api_contract::operations::TargetResolver::PermissionObject,
        usage: orch_api_contract::operations::FacetUsage::Required,
        cardinality: orch_api_contract::operation_sources::Cardinality::One,
    }],
};
#[derive(Clone, PartialEq, Eq)]
struct HostPolicy;
impl Operation for HostPolicy {
    const ROUTE: Route = HostUpdate::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &HOST_POLICY;
    const KIND: CommandKind = CommandKind::MachinePolicySet;
    const EXCLUSIVE: bool = true;
    const REQUIRE_VIEW: bool = false;
    const FAMILY: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Host);
    fn valid_body(body: &Json) -> bool {
        PolicyChange::valid_body(body)
    }
    fn policy_request(body: &Json) -> Option<MachinePolicyUpdateBody> {
        PolicyChange::policy_request(body)
    }
    fn require_request<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        request: &'a Request,
    ) -> BoxFuture<'a, Result<(), Error>> {
        PolicyChange::require_request(conn, principal, request)
    }
}
struct Request {
    physical: Physical,
    roles: Option<NativeRolesSet>,
    policy: Option<MachinePolicyUpdateBody>,
}

async fn physical_locks<F: Operation>(conn: &mut Conn, physical: &Physical) -> Result<(), Error> {
    sqlx::query(if F::EXCLUSIVE {
        "SELECT id FROM machines WHERE id=$1 FOR UPDATE"
    } else {
        "SELECT id FROM machines WHERE id=$1 FOR SHARE"
    })
    .bind(physical.machine)
    .fetch_one(&mut *conn)
    .await
    .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM hosts WHERE machine_id=$1 FOR SHARE")
        .bind(physical.machine)
        .fetch_all(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

struct Native<F>(std::marker::PhantomData<F>);
impl<F: Operation> Source for Native<F> {
    type Role = Role;
    type Input = Request;
    type Value = Physical;
    const ROUTE: Route = F::ROUTE;
    const DECLARATION: &'static Declaration<Role> = F::DECLARATION;

    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a Request,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Physical, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let fresh = Physical::load(conn, input.physical.machine).await?;
            if fresh != input.physical {
                return Err(Error::Refused);
            }
            let target = targets::machine_subject(conn, input.physical.machine)
                .await
                .map_err(EngineError::unreadable)?;
            if F::FAMILY == KeyTargetKind::Object(ObjectKind::Host) && fresh.facade.is_none() {
                return Err(Error::Refused);
            }
            Ok((fresh, vec![Facts::one(Role::Governor, F::FAMILY, target)]))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Request,
        _value: &'a Physical,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            for permission in [Permission::MachineView, Permission::MachineManage] {
                if !F::REQUIRE_VIEW && permission == Permission::MachineView {
                    continue;
                }
                if !orch_authz::resources::check_machine(
                    conn,
                    principal,
                    input.physical.machine,
                    permission,
                    Policy::default(),
                )
                .await?
                .is_allowed()
                {
                    return Err(Error::Refused);
                }
            }
            F::require_request(conn, principal, input).await
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Request,
        _value: &'a Physical,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(physical_locks::<F>(conn, &input.physical))
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Request,
        opened: &'a Physical,
        fresh: &'a Physical,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(opened == fresh) })
    }
}

/// Private native provenance, stored beside the command and never copied to its body.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound = "")]
struct Authority<F: Operation> {
    origin: CapturedOperation,
    credential: Credential,
    raw_until: Option<Time>,
    physical: Physical,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    roles_request: Option<NativeRolesSet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    policy_request: Option<MachinePolicyUpdateBody>,
    canonical: Json,
    #[serde(skip)]
    family: std::marker::PhantomData<F>,
}
impl<F: Operation> std::fmt::Debug for Authority<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MachineOperationAuthority([private])")
    }
}
impl<F: Operation> Authority<F> {
    async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        physical_locks::<F>(conn, &self.physical)
            .await
            .map_err(EngineError::unreadable)
    }
    /// Capture the real source and original/current credential minimum before its native queue.
    async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        credential: Credential,
        raw_until: Option<Time>,
        command: &Outgoing,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        if matches!(&credential, Credential::Native)
            || !credential.matches(caller)
            || caller
                .admitted_operation()
                .is_some_and(|operation| operation != F::ROUTE.name)
            || command.kind != F::KIND
            || command.key != SerialKey::Machine
            || !F::valid_body(&command.body)
        {
            return Err(EngineError::unreadable(
                "this is not the original native machine operation",
            ));
        }
        let physical = Physical::load(conn, command.machine)
            .await
            .map_err(EngineError::unreadable)?;
        let request = Request {
            physical: physical.clone(),
            roles: F::roles_request(&command.body),
            policy: F::policy_request(&command.body),
        };
        let (_, facts) = Native::<F>::prepare(conn, caller, &request, clock.time())
            .await
            .map_err(EngineError::unreadable)?;
        let caller = sources::bind_required(F::ROUTE, F::DECLARATION, &facts)
            .map_err(EngineError::unreadable)?
            .admit(caller, false)
            .map_err(EngineError::unreadable)?;
        let authority = Self {
            origin: CapturedOperation::capture(&caller).map_err(EngineError::unreadable)?,
            credential,
            raw_until,
            physical,
            roles_request: request.roles,
            policy_request: request.policy,
            family: std::marker::PhantomData,
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        };
        if authority.current(conn, clock).await?.is_none() {
            return Err(EngineError::unreadable(
                "the original machine operation has ended",
            ));
        }
        Ok(authority)
    }

    /// Restore exact raw identity and original/current scope at the actual native queue or persistence cut.
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<(Caller, Option<Time>)>, EngineError> {
        if matches!(&self.credential, Credential::Native) {
            return Ok(None);
        }
        if let Some(request) = &self.roles_request {
            let body = serde_json::to_value(request)?;
            if !F::valid_body(&body)
                || self.canonical.get("body_hash")
                    != Some(&Json::String(
                        crate::jobs::command_identity::hash(&body)?.to_string(),
                    ))
            {
                return Ok(None);
            }
        }
        if let Some(values) = &self.policy_request {
            let body = serde_json::to_value(MachinePolicySet {
                values: values.clone(),
            })?;
            if !F::valid_body(&body)
                || self.canonical.get("body_hash")
                    != Some(&Json::String(
                        crate::jobs::command_identity::hash(&body)?.to_string(),
                    ))
            {
                return Ok(None);
            }
        }
        let (caller, until) = match request_origin::current(
            conn,
            clock,
            &self.origin,
            &self.credential,
            self.raw_until,
            F::ROUTE.name,
        )
        .await
        {
            Ok(current) => current,
            Err(Error::Refused | Error::Binding(_)) => return Ok(None),
            Err(Error::Native(error)) => return Err(error),
        };
        let frame = match continuing::prepare_with_caller::<Native<F>>(
            conn,
            clock,
            &self.origin,
            &caller,
            Request {
                physical: self.physical.clone(),
                roles: self.roles_request.clone(),
                policy: self.policy_request.clone(),
            },
        )
        .await
        {
            Ok(frame) => frame,
            Err(Error::Refused | Error::Binding(_)) => return Ok(None),
            Err(Error::Native(error)) => return Err(error),
        };
        Ok(Some((frame.principal().caller().clone(), until)))
    }

    async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != F::KIND.as_str()
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
    async fn current_callers(
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

macro_rules! native_command {
    ($name:ident, $family:ty, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Authority<$family>);
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($name), "([private])"))
            }
        }
        impl $name {
            pub(super) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
                self.0.lock_source(conn).await
            }
            /// Capture this concrete native operation and exact original raw credential.
            pub async fn capture(
                conn: &mut Conn,
                caller: &Caller,
                credential: Credential,
                raw_until: Option<Time>,
                command: &Outgoing,
                clock: &dyn Clock,
            ) -> Result<Self, EngineError> {
                Authority::capture(conn, caller, credential, raw_until, command, clock)
                    .await
                    .map(Self)
            }
            /// Current original scope and native physical association on the owner's transaction.
            pub async fn current(
                &self,
                conn: &mut Conn,
                clock: &dyn Clock,
            ) -> Result<Option<(Caller, Option<Time>)>, EngineError> {
                self.0.current(conn, clock).await
            }
            pub(super) async fn current_issue_callers(
                &self,
                conn: &mut Conn,
                command: &NewCommand<'_>,
                clock: &dyn Clock,
            ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
                self.0.current_issue_callers(conn, command, clock).await
            }
            pub(super) async fn current_callers(
                &self,
                conn: &mut Conn,
                command: &CommandRow,
                clock: &dyn Clock,
            ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
                self.0.current_callers(conn, command, clock).await
            }
        }
    };
}
native_command!(
    MachineProbeAuthority,
    Probe,
    "Private native facts probe provenance, with its exact original operation and physical source."
);
native_command!(
    MachineProgramsAuthority,
    Programs,
    "Private native program operation provenance, with its exact original operation and physical source."
);

native_command!(
    MachineRolesAuthority,
    Roles,
    "Private delegated-role provenance, with its exact original operation and physical source."
);
native_command!(
    MachineGatewayAuthority,
    Gateway,
    "Private primary gateway selection provenance, retaining its exact operation and physical source."
);

native_command!(
    MachinePolicyAuthority,
    PolicyChange,
    "Private key-by-key policy provenance, with its exact original operation and physical source."
);
native_command!(
    HostPolicyAuthority,
    HostPolicy,
    "Private native Host policy synchronization provenance, retaining its original HostUpdate operation."
);
