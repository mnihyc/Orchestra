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
use crate::middleware::requests::{ApiError, ApiResult};
use crate::routes::access::page::{
    NameBounds, NameSources, entries_named, entries_named_at_bounded, entry_name_sources,
};
use crate::state::AppState;
use orch_access_contract::decisions::{Held, Through};
use orch_access_contract::pages as stored;
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::{AdminReach, PlaceRef};
use orch_api_contract::access::{Entry, ResourceAccess, ResourceThrough};
use orch_api_contract::environment::{EnvironmentActivity, EnvironmentRef};
use orch_api_contract::resources::{AgentRef, MachineRef, ProviderRef};
use orch_authz::Principal;
use orch_db::Conn;
use orch_db::queries::access;
use orch_db::queries::resources::routes as rows;
use orch_types::{Id, Json};
use serde::de::DeserializeOwned;
use std::collections::{HashMap, HashSet};

/// A word the database keeps, as the API's value of that name.
pub fn parse<T: DeserializeOwned>(word: &str) -> ApiResult<T> {
    serde_json::from_value(Json::String(word.to_owned()))
        .map_err(|_| ApiError::internal(format!("{word:?} is not a value the API knows")))
}

/// Whether a machine is connected now: the link has a session for it, or the row says it had one
/// when it was last known (another instance of the API may hold the session).
#[must_use]
pub fn is_online(app: &AppState, id: Id, row_says: bool) -> bool {
    app.link.is_online(id) || row_says
}

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

/// The providers among `ids`, and those that run on or were registered from one of `machines`,
/// as references, each with the machine it belongs to.
pub async fn provider_refs(
    conn: &mut Conn,
    ids: &[Id],
    machines: &[Id],
) -> ApiResult<Vec<(Option<Id>, ProviderRef)>> {
    provider_refs_with_names(conn, ids, machines, None).await
}
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

/// The same native reference producer for rows already selected by a family preparation.
pub(crate) async fn provider_refs_from_rows(
    conn: &mut Conn,
    found: Vec<rows::ProviderRefRow>,
) -> ApiResult<Vec<(Option<Id>, ProviderRef)>> {
    let wanted: Vec<Id> = found.iter().map(|row| row.id).collect();
    let addresses = addresses(conn, &wanted).await?;
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

/// The same native reference producer for already selected agent registrations.
pub(crate) async fn agent_refs_from_rows(
    app: &AppState,
    conn: &mut Conn,
    found: Vec<rows::AgentRefRow>,
) -> ApiResult<Vec<(Option<Id>, AgentRef)>> {
    let wanted: Vec<Id> = found.iter().map(|row| row.id).collect();
    let addresses = addresses(conn, &wanted).await?;
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

/// The same native metadata producer for already selected Environment registrations.
pub(crate) async fn environment_refs_from_rows(
    conn: &mut Conn,
    found: Vec<rows::EnvironmentRefRow>,
    now: orch_types::Time,
) -> ApiResult<Vec<(Option<Id>, EnvironmentRef)>> {
    environment_refs_from_rows_with_names(conn, found, now, None).await
}
async fn environment_refs_from_rows_with_names(
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

/// What the caller holds on one resource, as the engine told it.
#[derive(Debug, Clone, Copy)]
pub struct Holding<'a> {
    /// The native act's administration source, even when View is ordinary.
    pub admin_place: Option<orch_access_contract::roles::PlaceRef>,
    /// Held permissions absent from the ordinary frame.
    pub admin_only: &'a [orch_access_contract::permissions::Permission],
    /// The administrative scope used for this private resource.
    pub admin_access: Option<orch_access_contract::roles::AdminReach>,
    /// Its ordinary page gives View.
    pub participating: bool,
    /// The resource.
    pub id: Id,
    /// Whether the caller is one of its owners and counts as one.
    pub owner: bool,
    /// Whether the caller administers its home.
    pub administers: bool,
    /// What they hold on it, each with the ways.
    pub held: &'a [Held],
}

impl<'a> Holding<'a> {
    /// Borrow the existing native resource decision; no permission is inferred or added.
    pub(crate) fn from_seen(seen: &'a orch_authz::resources::Seen) -> Self {
        Self {
            admin_place: seen.admin_place,
            admin_only: &seen.admin_only,
            admin_access: seen.admin_access,
            participating: seen.participating,
            id: seen.resource.id,
            owner: seen.owner,
            administers: seen.administers,
            held: &seen.held,
        }
    }

    /// Borrow the equivalent native scalar/bulk effective-access result.
    pub(crate) fn from_effective(effective: &'a orch_authz::resources::Effective) -> Self {
        Self {
            admin_place: effective.admin_place,
            admin_only: &effective.admin_only,
            admin_access: effective.admin_access,
            participating: effective.participating,
            id: effective.object.id,
            owner: effective.owner,
            administers: effective.administers,
            held: &effective.held,
        }
    }
}

/// Native ordinary authority that selected the protected shape of an admitted body.
/// Comparison consumes existing Effective/Seen facts; it defines no permission rule.
#[derive(Clone)]
pub(crate) struct Authority {
    permissions: HashSet<Permission>,
    owner: bool,
    administers: bool,
    participating: bool,
    admin_access: Option<AdminReach>,
    admin_place: Option<PlaceRef>,
    admin_only: HashSet<Permission>,
}
impl Authority {
    fn holding(held: &Holding<'_>) -> Self {
        Self {
            permissions: held.held.iter().map(|held| held.permission).collect(),
            owner: held.owner,
            administers: held.administers,
            participating: held.participating,
            admin_access: held.admin_access,
            admin_place: held.admin_place,
            admin_only: held.admin_only.iter().copied().collect(),
        }
    }
    pub(crate) fn seen(seen: &orch_authz::resources::Seen) -> Self {
        Self::holding(&Holding::from_seen(seen))
    }
    pub(crate) fn effective(held: &orch_authz::resources::Effective) -> Self {
        Self::holding(&Holding::from_effective(held))
    }
    pub(crate) fn preserved_by(&self, fresh: &Self) -> bool {
        self.permissions.is_subset(&fresh.permissions)
            && (!self.owner || fresh.owner)
            && (!self.administers || fresh.administers)
            && (!self.participating || fresh.participating)
            && self
                .admin_access
                .is_none_or(|old| fresh.admin_access == Some(old))
            && self
                .admin_place
                .is_none_or(|old| fresh.admin_place == Some(old))
            && self.admin_only.is_subset(&fresh.admin_only)
    }
}

/// What the caller holds on each of the resources, with the entries that give it named: the
/// statements are those of one resource, however many there are.
pub async fn accesses(
    conn: &mut Conn,
    principal: &Principal,
    holdings: &[Holding<'_>],
) -> ApiResult<HashMap<Id, ResourceAccess>> {
    let prepared = prepare_accesses(conn, holdings).await?;
    accesses_prepared(conn, principal, holdings, &prepared, None).await
}

/// The existing access projection's stored entries and name identities, before any
/// foreign names or full Group values are projected. The caller admits those sources.
#[derive(Clone)]
pub(crate) struct PreparedAccess {
    authority: HashMap<Id, Authority>,
    entries: Vec<stored::Entry>,
    places: Vec<Id>,
    pub(crate) names: NameSources,
}

impl PreparedAccess {
    /// Preserve the native holdings that selected the already produced protected shape.
    pub(crate) fn preserves_authority(&self, fresh: &Self) -> bool {
        self.authority.iter().all(|(id, old)| {
            fresh
                .authority
                .get(id)
                .is_some_and(|current| old.preserved_by(current))
        })
    }
}

/// Reuse the ordinary access walk once, retaining its actual entries for projection
/// in the same read snapshot. NameSources contains identities, never permission proof.
pub(crate) async fn prepare_accesses(
    conn: &mut Conn,
    holdings: &[Holding<'_>],
) -> ApiResult<PreparedAccess> {
    let places: Vec<Id> = holdings
        .iter()
        .flat_map(|holding| {
            holding
                .admin_place
                .filter(|_| !holding.admin_only.is_empty())
                .into_iter()
                .chain(holding.admin_access.map(|reach| reach.place))
        })
        .filter_map(orch_access_contract::roles::PlaceRef::node)
        .collect();
    let mut entry_ids: Vec<Id> = holdings
        .iter()
        .flat_map(|holding| holding.held.iter())
        .flat_map(|held| held.through.iter())
        .filter_map(|through| match through {
            Through::Entry(via) => Some(via.entry),
            _ => None,
        })
        .collect();
    entry_ids.sort_unstable();
    entry_ids.dedup();
    let entries = if entry_ids.is_empty() {
        Vec::new()
    } else {
        let found = access::entries_by_id(conn, &entry_ids).await?;
        found
            .iter()
            .filter_map(|row| orch_authz::engine::entry_of(row).ok())
            .collect()
    };
    let mut names = entry_name_sources(&entries, None);
    names.nodes.extend(places.iter().copied());
    Ok(PreparedAccess {
        authority: holdings
            .iter()
            .map(|holding| (holding.id, Authority::holding(holding)))
            .collect(),
        entries,
        places,
        names,
    })
}

/// Render the same access rows with an optional bound on protected name sources.
/// Full Group sources must already be admitted; no entry or count is redacted away.
pub(crate) async fn accesses_prepared(
    conn: &mut Conn,
    principal: &Principal,
    holdings: &[Holding<'_>],
    prepared: &PreparedAccess,
    bounds: Option<&NameBounds>,
) -> ApiResult<HashMap<Id, ResourceAccess>> {
    let places = match bounds {
        Some(bounds) => {
            bounds
                .qualified_refs(conn, principal, &prepared.places)
                .await?
        }
        None => {
            crate::routes::nodes::view::qualified_refs(
                conn,
                principal,
                &prepared.places,
                orch_authz::Policy::default(),
            )
            .await?
        }
    };
    let named: HashMap<Id, Entry> = if prepared.entries.is_empty() {
        HashMap::new()
    } else {
        match bounds {
            Some(bounds) => {
                entries_named_at_bounded(conn, principal, None, None, &prepared.entries, bounds)
                    .await?
            }
            None => entries_named(conn, principal, None, &prepared.entries).await?,
        }
    };
    Ok(holdings
        .iter()
        .map(|holding| {
            let mut through: Vec<ResourceThrough> = Vec::new();
            for way in holding.held.iter().flat_map(|held| held.through.iter()) {
                let each = match way {
                    Through::Owner => Some(ResourceThrough::Owner),
                    Through::SourceOwner { .. } => Some(ResourceThrough::SourceOwner),
                    Through::Entry(via) => {
                        named.get(&via.entry).map(|entry| ResourceThrough::Entry {
                            holder: entry.holder.clone(),
                            by: entry.created_by.clone(),
                        })
                    }
                    // Administering is told by `administers`, a higher level by the permissions.
                    Through::Admin | Through::SystemAdmin | Through::Level { .. } => None,
                };
                if let Some(each) = each
                    && !through.contains(&each)
                {
                    through.push(each);
                }
            }
            (
                holding.id,
                ResourceAccess {
                    admin_place: (!holding.admin_only.is_empty())
                        .then_some(holding.admin_place)
                        .flatten()
                        .and_then(|place| crate::routes::nodes::view::admin_place(place, &places)),
                    admin_only: holding
                        .admin_only
                        .iter()
                        .map(|permission| permission.as_str().to_owned())
                        .collect(),
                    admin_access: holding
                        .admin_access
                        .and_then(|place| crate::routes::nodes::view::admin_access(place, &places)),
                    participating: holding.participating,
                    permissions: holding
                        .held
                        .iter()
                        .map(|held| held.permission.as_str().to_owned())
                        .collect(),
                    administers: holding.administers,
                    through,
                },
            )
        })
        .collect())
}
