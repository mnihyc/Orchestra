//! Native machine-policy preparation shared by the HTTP writer and retained command checks.

use crate::EngineError;
use orch_access_contract::{
    decisions::{ObjectRef, Refusal},
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{common::FieldFault, resources::MachinePolicySection};
use orch_authz::{
    Policy, Principal,
    resources::{KeyAbout, Keying, may_change_policy_key},
};
use orch_db::{
    Conn, DbError,
    queries::{environment_facts, resources},
};
use orch_link_contract::policy::{Bounds, LocalPolicy, MachineDrivers, PolicyKey};
use orch_types::{Id, Json};
use std::collections::BTreeMap;

/// The same intrinsic key question used by policy rendering and mutation preparation.
pub fn policy_key_about(key: PolicyKey) -> KeyAbout {
    if key == PolicyKey::PolicyDelegated {
        return KeyAbout::TheHandingOver;
    }
    match key.bounds() {
        Bounds::Environments => KeyAbout::Environments,
        Bounds::Providers => KeyAbout::Providers,
        Bounds::Machine => KeyAbout::TheMachine,
    }
}

/// Machine viewers and the existing section-only hosted-work readers use one native gate.
pub async fn policy_sections(
    conn: &mut Conn,
    principal: &Principal,
    machine: Id,
) -> Result<Vec<MachinePolicySection>, EngineError> {
    let (kind, id, permission) = match resources::hosting::host_of_machine(conn, machine).await? {
        Some(host) => (ObjectKind::Host, host, Permission::HostView),
        None => (ObjectKind::Machine, machine, Permission::MachineView),
    };
    if orch_authz::resources::check(
        conn,
        principal,
        ObjectRef { kind, id },
        permission,
        Policy::default(),
    )
    .await?
    .is_allowed()
    {
        return Ok(MachinePolicySection::ALL.to_vec());
    }
    let related = environment_facts::hosted_resources(conn, machine).await?;
    let effective =
        orch_authz::resources::effective_among(conn, principal, &related, Policy::default())
            .await?;
    let environment = effective.iter().any(|resource| {
        resource
            .held
            .iter()
            .any(|held| held.permission == Permission::EnvironmentManage)
    });
    let provider = effective.iter().any(|resource| {
        resource
            .held
            .iter()
            .any(|held| held.permission == Permission::ProviderManage)
    });
    let mut sections = Vec::new();
    if environment || provider {
        sections.extend([
            MachinePolicySection::Environment,
            MachinePolicySection::Node,
        ]);
    }
    if provider {
        sections.push(MachinePolicySection::Provider);
    }
    Ok(sections)
}

/// Factual operands selected by the original key-by-key policy preparer, without issuing a command.
pub struct PolicyPreparation {
    /// The actual selected stored machine.
    pub row: resources::MachineRow,
    /// Its same current effective local policy.
    pub before: Option<LocalPolicy>,
    /// Canonical validated keys and the values supplied for them.
    pub changes: BTreeMap<PolicyKey, Json>,
}

/// The native refusal category and its original typed operands, never an admission proof.
#[derive(Debug, thiserror::Error)]
pub enum PolicyFailure {
    /// The ordinary native policy door has no subject visible to this reader.
    #[error("the machine policy is not available")]
    Missing,
    /// A supplied name or value does not fit the existing local-policy contract.
    #[error("invalid policy key {0}: {1}")]
    Invalid(String, String),
    /// The access engine's original ordinary refusal.
    #[error("the native machine policy action was refused")]
    Refused(Box<Refusal>),
    /// The connector operator retains these keys locally.
    #[error("these policy keys are kept locally")]
    NotDelegated(Vec<String>),
    /// This native key is controlled by the physical owner.
    #[error("this key is controlled by the machine owner")]
    NotOwner(String),
    /// Actual reported driver schemas reject supplied settings.
    #[error("the driver configuration does not fit the reported schema")]
    Drivers(Vec<FieldFault>),
    /// The native data source failed.
    #[error(transparent)]
    Native(#[from] EngineError),
}
impl From<DbError> for PolicyFailure {
    fn from(error: DbError) -> Self {
        Self::Native(error.into())
    }
}
impl From<orch_authz::Error> for PolicyFailure {
    fn from(error: orch_authz::Error) -> Self {
        Self::Native(error.into())
    }
}
impl From<serde_json::Error> for PolicyFailure {
    fn from(error: serde_json::Error) -> Self {
        Self::Native(error.into())
    }
}

/// The original effect-free policy selection, validation and ordinary delegation decisions.
pub async fn prepare_policy(
    conn: &mut Conn,
    principal: &Principal,
    machine: Id,
    body: &BTreeMap<String, Json>,
) -> Result<PolicyPreparation, PolicyFailure> {
    let row = resources::machine(conn, machine)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or(PolicyFailure::Missing)?;
    if policy_sections(conn, principal, machine).await?.is_empty() {
        return Err(PolicyFailure::Missing);
    }
    let mut changes = BTreeMap::new();
    for (name, value) in body {
        let key: PolicyKey = name.parse().map_err(|_| {
            PolicyFailure::Invalid(
                name.clone(),
                "this is no key of a machine's local policy".into(),
            )
        })?;
        let mut trial = LocalPolicy::default();
        trial
            .set(key, value)
            .map_err(|error| PolicyFailure::Invalid(name.clone(), error.to_string()))?;
        changes.insert(key, value.clone());
    }
    let facts = environment_facts::machine(conn, machine, None).await?;
    let before = match facts
        .as_ref()
        .and_then(|facts| facts.policy.as_ref())
        .filter(|value| !value.is_null())
    {
        Some(value) => Some(serde_json::from_value::<LocalPolicy>(value.clone())?),
        None => None,
    };
    let mut not_delegated = Vec::new();
    for key in changes.keys() {
        let delegated = before.as_ref().is_some_and(|policy| policy.delegates(*key));
        match may_change_policy_key(
            conn,
            principal,
            machine,
            policy_key_about(*key),
            delegated,
            Policy::default(),
        )
        .await?
        {
            Keying::Allowed => {}
            Keying::Refused(refusal) => return Err(PolicyFailure::Refused(refusal)),
            Keying::NotDelegated | Keying::OnTheMachineOnly => {
                not_delegated.push(key.as_str().to_owned())
            }
            Keying::NotTheMachinesOwner => return Err(PolicyFailure::NotOwner(key.to_string())),
            Keying::NoSuchMachine => return Err(PolicyFailure::Missing),
        }
    }
    if !not_delegated.is_empty() {
        not_delegated.sort();
        return Err(PolicyFailure::NotDelegated(not_delegated));
    }
    Ok(PolicyPreparation {
        row,
        before,
        changes,
    })
}

/// Validate only changed driver configurations against this same machine's reported schemas.
/// The HTTP producer retains its existing online-before-schema refusal order.
pub async fn validate_policy_drivers(
    conn: &mut Conn,
    prepared: &PolicyPreparation,
) -> Result<(), PolicyFailure> {
    if let Some(value) = prepared.changes.get(&PolicyKey::Drivers) {
        let proposed: MachineDrivers = serde_json::from_value(value.clone()).map_err(|_| {
            PolicyFailure::Invalid(
                "drivers".into(),
                "the driver settings have an invalid shape".into(),
            )
        })?;
        if let Some(host_id) = resources::hosting::host_of_machine(conn, prepared.row.id).await?
            && let Some(host) = resources::hosting::host(conn, host_id).await?
        {
            let errors = driver_configuration_faults(
                &proposed,
                prepared.before.as_ref().map(|policy| &policy.drivers),
                &host.driver_configuration_schemas,
                &host.drivers,
            );
            if !errors.is_empty() {
                return Err(PolicyFailure::Drivers(errors));
            }
        }
    }
    Ok(())
}

/// The same bounded local validator re-exported by the plugin runtime; references never fetch.
pub fn driver_configuration_faults(
    proposed: &MachineDrivers,
    previous: Option<&MachineDrivers>,
    registered_schemas: &Json,
    descriptors: &Json,
) -> Vec<FieldFault> {
    let mut errors = Vec::new();
    for (name, value) in &proposed.configurations {
        if previous.and_then(|old| old.configurations.get(name)) == Some(value) {
            continue;
        }
        let Some(schema) = registered_schemas
            .get(name.as_str())
            .filter(|schema| !schema.is_null())
            .or_else(|| {
                descriptors
                    .get(name.as_str())
                    .and_then(|driver| driver.get("configuration_schema"))
                    .filter(|schema| !schema.is_null())
            })
        else {
            continue;
        };
        let prefix = format!("drivers.configurations.{name}");
        match orch_types::schema::validate(schema, value) {
            Ok(()) => {}
            Err(orch_types::schema::Error::Schema { .. }) => errors.push(FieldFault {
                path: prefix,
                message: "the driver reported an invalid configuration schema".into(),
            }),
            Err(orch_types::schema::Error::Value { faults }) => {
                errors.extend(faults.into_iter().map(|fault| FieldFault {
                    path: format!("{prefix}{}", fault.path),
                    message: fault.message,
                }))
            }
        }
        if errors.len() >= orch_types::schema::MAX_FAULTS {
            errors.truncate(orch_types::schema::MAX_FAULTS);
            break;
        }
    }
    errors
}
