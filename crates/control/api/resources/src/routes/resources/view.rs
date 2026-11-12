//! Machines, providers, profiles and environments as other things point at them, and what the caller holds on a resource, as the API shows them, for any number of resources in the same few statements.
//!
//! A reference names a resource wherever another one points at it: by its name, its address (the
//! short names of the orgs it lies in, its first owner and its own name, so that nobody mistakes
//! whose thing it is) and how it stands. Every function here reads for a whole list at once, so
//! that the statements of a list do not grow with the number of rows.
//!
//! An operation reuses the native producer's actual content-source set; reference types do not
//! invent a second privilege. A Host's own physical Machine/facade is the same governing subject
//! and remains intrinsic Host content. Qualified distinct sources, such as an Agent's readable
//! Provider/models or an MCP offering, keep their existing source gates under the consuming
//! operation. Use stored relationships and canonical subjects, without adding native View gates
//! or inferring extra authority from a DTO field's type, label or URL.

use crate::middleware::authorization::refusals::user_refs;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::routes::access::page::NameBounds;

use crate::state::AppState;

use orch_api_contract::environment::EnvironmentActivity;

use orch_api_contract::environment::EnvironmentRef;

use orch_api_contract::resources::AgentRef;

use orch_api_contract::resources::MachineRef;

use orch_api_contract::resources::ProviderRef;

use orch_db::Conn;

use orch_db::queries::resources::routes as rows;

use orch_types::Id;

use std::collections::HashMap;

pub use orch_api_base::routes::resources::view::parse;

pub use orch_api_base::routes::resources::view::Holding;

pub use orch_api_base::routes::resources::view::Authority;

pub use orch_api_base::routes::resources::view::accesses;

pub use orch_api_base::routes::resources::view::PreparedAccess;

pub use orch_api_base::routes::resources::view::prepare_accesses;

pub use orch_api_base::routes::resources::view::accesses_prepared;
/// Whether a machine is connected now: the link has a session for it, or the row says it had one
/// when it was last known (another instance of the API may hold the session).
#[must_use]
pub fn is_online(app: &AppState, id: Id, row_says: bool) -> bool {
    app.link.is_online(id) || row_says
}
orch_api_base::__api_bind_9b9d72ac9f!();

/// The machines `ids` as references, by identifier. A machine that is not there is not among them.
pub async fn machine_refs(
    app: &AppState,
    conn: &mut Conn,
    ids: &[Id],
) -> ApiResult<HashMap<Id, MachineRef>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    machine_refs_from_rows(app, rows::machine_names(conn, ids).await?)
}
orch_api_base::__api_bind_78f49040ef!();

/// Convert the native reader's already selected rows without selecting different identities.
pub(crate) fn machine_refs_from_rows(
    app: &AppState,
    selected: Vec<rows::MachineNameRow>,
) -> ApiResult<HashMap<Id, MachineRef>> {
    selected
        .into_iter()
        .map(|row| {
            Ok((
                row.id,
                MachineRef {
                    id: row.id,
                    name: row.name,
                    online: is_online(app, row.id, row.online),
                    platform_os: parse(&row.platform_os)?,
                    placement: parse(&row.placement)?,
                },
            ))
        })
        .collect()
}
orch_api_base::__api_bind_4354480158!();

/// The addresses of the resources `ids`, by identifier: four statements for any number of them.
pub async fn addresses(conn: &mut Conn, ids: &[Id]) -> ApiResult<HashMap<Id, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(orch_authz::resources::addresses(conn, ids)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .collect())
}
orch_api_base::__api_bind_1d9f1e7836!();

/// Uses the same native address construction with an optional protected-name bound.
pub(crate) async fn addresses_with_names(
    conn: &mut Conn,
    ids: &[Id],
    bounds: Option<&NameBounds>,
) -> ApiResult<HashMap<Id, String>> {
    Ok(orch_authz::resources::addresses_with_name_sources(
        conn,
        ids,
        bounds.filter(|b| b.limited).map(|b| &b.nodes),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .collect())
}
orch_api_base::__api_bind_2708eb6c6a!();

/// The providers among `ids`, and those that run on or were registered from one of `machines`,
/// as references, each with the machine it belongs to.
pub async fn provider_refs(
    conn: &mut Conn,
    ids: &[Id],
    machines: &[Id],
) -> ApiResult<Vec<(Option<Id>, ProviderRef)>> {
    provider_refs_with_names(conn, ids, machines, None).await
}
orch_api_base::__api_bind_05cca12189!();

/// The native provider reference producer, with protected address components bounded before lookup.
pub(crate) async fn provider_refs_with_names(
    conn: &mut Conn,
    ids: &[Id],
    machines: &[Id],
    bounds: Option<&NameBounds>,
) -> ApiResult<Vec<(Option<Id>, ProviderRef)>> {
    let found = rows::providers_among(conn, ids, machines).await?;
    let wanted = found.iter().map(|row| row.id).collect::<Vec<_>>();
    let addresses = addresses_with_names(conn, &wanted, bounds).await?;
    provider_refs_with_addresses(found, &addresses)
}
orch_api_base::__api_bind_7577050e26!();

/// The same native reference producer for rows already selected by a family preparation.
pub(crate) async fn provider_refs_from_rows(
    conn: &mut Conn,
    found: Vec<rows::ProviderRefRow>,
) -> ApiResult<Vec<(Option<Id>, ProviderRef)>> {
    provider_refs_from_rows_with_names(conn, found, None).await
}
orch_api_base::__api_bind_42fb982f58!();

/// The same selected registrations with the consuming operation's address bounds.
pub(crate) async fn provider_refs_from_rows_with_names(
    conn: &mut Conn,
    found: Vec<rows::ProviderRefRow>,
    bounds: Option<&NameBounds>,
) -> ApiResult<Vec<(Option<Id>, ProviderRef)>> {
    let wanted: Vec<Id> = found.iter().map(|row| row.id).collect();
    let addresses = addresses_with_names(conn, &wanted, bounds).await?;
    provider_refs_with_addresses(found, &addresses)
}

fn provider_refs_with_addresses(
    found: Vec<rows::ProviderRefRow>,
    addresses: &HashMap<Id, String>,
) -> ApiResult<Vec<(Option<Id>, ProviderRef)>> {
    found
        .into_iter()
        .map(|row| {
            Ok((
                row.machine_id,
                ProviderRef {
                    address: addresses.get(&row.id).cloned().unwrap_or_default(),
                    id: row.id,
                    name: row.name,
                    kind: parse(&row.kind)?,
                    harness: row
                        .harness_kind
                        .as_deref()
                        .map(|kind| crate::programs::named_by(kind, row.harness_name.as_deref())),
                    harness_kind: row.harness_kind,
                    placement: parse(&row.placement)?,
                    status: parse(&row.status)?,
                },
            ))
        })
        .collect()
}

/// The profiles among `ids`, and those that belong to one of `machines`, as references, each with
/// the machine it belongs to.
pub async fn agent_refs(
    app: &AppState,
    conn: &mut Conn,
    ids: &[Id],
    machines: &[Id],
) -> ApiResult<Vec<(Option<Id>, AgentRef)>> {
    if ids.is_empty() && machines.is_empty() {
        return Ok(Vec::new());
    }
    let found = rows::agents_among(conn, ids, machines).await?;
    agent_refs_from_rows(app, conn, found).await
}
orch_api_base::__api_bind_cddd9f4808!();

/// The same native reference producer for already selected agent registrations.
pub(crate) async fn agent_refs_from_rows(
    app: &AppState,
    conn: &mut Conn,
    found: Vec<rows::AgentRefRow>,
) -> ApiResult<Vec<(Option<Id>, AgentRef)>> {
    agent_refs_from_rows_with_names(app, conn, found, None).await
}

/// The same selected profiles with the consuming operation's address bounds.
pub(crate) async fn agent_refs_from_rows_with_names(
    app: &AppState,
    conn: &mut Conn,
    found: Vec<rows::AgentRefRow>,
    bounds: Option<&NameBounds>,
) -> ApiResult<Vec<(Option<Id>, AgentRef)>> {
    let wanted: Vec<Id> = found.iter().map(|row| row.id).collect();
    let addresses = addresses_with_names(conn, &wanted, bounds).await?;
    agent_refs_with_addresses(app, conn, found, &addresses).await
}

/// Uses the same address batch as the resources which point at these already-read profiles.
pub(crate) async fn agent_refs_with_addresses(
    app: &AppState,
    conn: &mut Conn,
    found: Vec<rows::AgentRefRow>,
    addresses: &HashMap<Id, String>,
) -> ApiResult<Vec<(Option<Id>, AgentRef)>> {
    let on: Vec<Id> = found.iter().filter_map(|row| row.machine_id).collect();
    let machine_refs = machine_refs(app, conn, &on).await?;
    Ok(found
        .into_iter()
        .map(|row| {
            (
                row.machine_id,
                AgentRef {
                    address: addresses.get(&row.id).cloned().unwrap_or_default(),
                    machine: row
                        .machine_id
                        .and_then(|machine| machine_refs.get(&machine).cloned()),
                    id: row.id,
                    name: row.name,
                    harness: crate::programs::named_by(
                        &row.harness_kind,
                        row.harness_name.as_deref(),
                    ),
                    harness_kind: row.harness_kind,
                    disabled: row.disabled,
                },
            )
        })
        .collect())
}

/// The environments among `ids`, and those registered from one of `machines`, as references, each
/// with the machine it is a directory of.
pub async fn environment_refs(
    conn: &mut Conn,
    ids: &[Id],
    machines: &[Id],
    now: orch_types::Time,
) -> ApiResult<Vec<(Option<Id>, EnvironmentRef)>> {
    if ids.is_empty() && machines.is_empty() {
        return Ok(Vec::new());
    }
    environment_refs_with_names(conn, ids, machines, now, None).await
}
orch_api_base::__api_bind_98d591f9cc!();

/// Native environment references with admitted optional address components.
pub(crate) async fn environment_refs_with_names(
    conn: &mut Conn,
    ids: &[Id],
    machines: &[Id],
    now: orch_types::Time,
    bounds: Option<&NameBounds>,
) -> ApiResult<Vec<(Option<Id>, EnvironmentRef)>> {
    let found = rows::environments_among(conn, ids, machines).await?;
    environment_refs_from_rows_with_names(conn, found, now, bounds).await
}
orch_api_base::__api_bind_a613953738!();

/// The same native metadata producer for already selected Environment registrations.
pub(crate) async fn environment_refs_from_rows_with_names(
    conn: &mut Conn,
    found: Vec<rows::EnvironmentRefRow>,
    now: orch_types::Time,
    bounds: Option<&NameBounds>,
) -> ApiResult<Vec<(Option<Id>, EnvironmentRef)>> {
    let wanted: Vec<Id> = found.iter().map(|row| row.id).collect();
    let activity = environment_activity(conn, &wanted).await?;
    let exposed = crate::routes::ports::exposures(conn, &wanted, now).await?;
    let addresses = addresses_with_names(conn, &wanted, bounds).await?;
    let owners: Vec<Id> = found.iter().map(|row| row.owner_id).collect();
    let owners = user_refs(conn, &owners).await?;
    found
        .into_iter()
        .map(|row| {
            let owner = owners
                .get(&row.owner_id)
                .cloned()
                .ok_or_else(|| ApiError::internal("the owner of an environment is not there"))?;
            Ok((
                row.machine_id,
                EnvironmentRef {
                    activity: activity.get(&row.id).cloned(),
                    ports_exposed: exposed.contains(&row.id),
                    address: addresses.get(&row.id).cloned().unwrap_or_default(),
                    id: row.id,
                    name: row.name,
                    owner,
                    placement: parse(&row.placement)?,
                    status: parse(&row.status)?,
                },
            ))
        })
        .collect()
}

/// Batched activity observations for references and full resource rows alike.
pub async fn environment_activity(
    conn: &mut Conn,
    ids: &[Id],
) -> ApiResult<HashMap<Id, EnvironmentActivity>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(orch_db::queries::environments::activity(conn, ids)
        .await?
        .into_iter()
        .map(|row| {
            (
                row.environment_id,
                EnvironmentActivity {
                    running_threads: row.running_threads,
                    idle_threads: row.idle_threads,
                    blocked_threads: row.blocked_threads,
                    active_turns: row.active_turns,
                    open_shells: row.open_shells,
                },
            )
        })
        .collect())
}
orch_api_base::__api_bind_df4d6a0d97!();
