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

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use crate::routes::access::page::entries_named;

use crate::routes::access::page::entries_named_at_bounded;

use crate::routes::access::page::entry_name_sources;

use orch_access_contract::decisions::Held;

use orch_access_contract::decisions::Through;

use orch_access_contract::pages as stored;

use orch_access_contract::permissions::Permission;

use orch_access_contract::roles::AdminReach;

use orch_access_contract::roles::PlaceRef;

use orch_api_contract::access::Entry;

use orch_api_contract::access::ResourceAccess;

use orch_api_contract::access::ResourceThrough;

use orch_authz::Principal;

pub use orch_db::Conn;

use orch_db::queries::access;

pub use orch_types::Id;

use orch_types::Json;

use serde::de::DeserializeOwned;

pub use std::collections::HashMap;

use std::collections::HashSet;
/// A word the database keeps, as the API's value of that name.
pub fn parse<T: DeserializeOwned>(word: &str) -> ApiResult<T> {
    serde_json::from_value(Json::String(word.to_owned()))
        .map_err(|_| ApiError::internal(format!("{word:?} is not a value the API knows")))
}

orch_api_base::api_family_callback! {
    bind __api_bind_9b9d72ac9f, __api_install_9b9d72ac9f, __api_callback_9b9d72ac9f;
    slot __ApiCallback9b9d72ac9f, __API_CALLBACK_9B9D72AC9F, __api_set_9b9d72ac9f;
    base orch_api_base::routes::resources::view;

    fn is_online<'__api>(arg0: &'__api orch_api_base::state::AppState, arg1: orch_types::Id, arg2: bool) -> bool;
}

orch_api_base::api_family_callback! {
    bind __api_bind_78f49040ef, __api_install_78f49040ef, __api_callback_78f49040ef;
    slot __ApiCallback78f49040ef, __API_CALLBACK_78F49040EF, __api_set_78f49040ef;
    base orch_api_base::routes::resources::view;

    async fn machine_refs<'__api>(arg0: &'__api orch_api_base::state::AppState, arg1: &'__api mut orch_db::Conn, arg2: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::resources::MachineRef>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_4354480158, __api_install_4354480158, __api_callback_4354480158;
    slot __ApiCallback4354480158, __API_CALLBACK_4354480158, __api_set_4354480158;
    base orch_api_base::routes::resources::view;

    fn machine_refs_from_rows<'__api>(arg0: &'__api orch_api_base::state::AppState, arg1: Vec<orch_db::queries::resources::routes::MachineNameRow>) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::resources::MachineRef>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_1d9f1e7836, __api_install_1d9f1e7836, __api_callback_1d9f1e7836;
    slot __ApiCallback1d9f1e7836, __API_CALLBACK_1D9F1E7836, __api_set_1d9f1e7836;
    base orch_api_base::routes::resources::view;

    async fn addresses<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, String>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_2708eb6c6a, __api_install_2708eb6c6a, __api_callback_2708eb6c6a;
    slot __ApiCallback2708eb6c6a, __API_CALLBACK_2708EB6C6A, __api_set_2708eb6c6a;
    base orch_api_base::routes::resources::view;

    async fn addresses_with_names<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id], arg2: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, String>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_05cca12189, __api_install_05cca12189, __api_callback_05cca12189;
    slot __ApiCallback05cca12189, __API_CALLBACK_05CCA12189, __api_set_05cca12189;
    base orch_api_base::routes::resources::view;

    async fn provider_refs<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id], arg2: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<Vec<(Option<orch_types::Id>, orch_api_contract::resources::ProviderRef)>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_7577050e26, __api_install_7577050e26, __api_callback_7577050e26;
    slot __ApiCallback7577050e26, __API_CALLBACK_7577050E26, __api_set_7577050e26;
    base orch_api_base::routes::resources::view;

    async fn provider_refs_with_names<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id], arg2: &'__api [orch_types::Id], arg3: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<Vec<(Option<orch_types::Id>, orch_api_contract::resources::ProviderRef)>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_42fb982f58, __api_install_42fb982f58, __api_callback_42fb982f58;
    slot __ApiCallback42fb982f58, __API_CALLBACK_42FB982F58, __api_set_42fb982f58;
    base orch_api_base::routes::resources::view;

    async fn provider_refs_from_rows<'__api>(arg0: &'__api mut orch_db::Conn, arg1: Vec<orch_db::queries::resources::routes::ProviderRefRow>) -> orch_api_base::middleware::requests::ApiResult<Vec<(Option<orch_types::Id>, orch_api_contract::resources::ProviderRef)>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_cddd9f4808, __api_install_cddd9f4808, __api_callback_cddd9f4808;
    slot __ApiCallbackcddd9f4808, __API_CALLBACK_CDDD9F4808, __api_set_cddd9f4808;
    base orch_api_base::routes::resources::view;

    async fn agent_refs<'__api>(arg0: &'__api orch_api_base::state::AppState, arg1: &'__api mut orch_db::Conn, arg2: &'__api [orch_types::Id], arg3: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<Vec<(Option<orch_types::Id>, orch_api_contract::resources::AgentRef)>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_98d591f9cc, __api_install_98d591f9cc, __api_callback_98d591f9cc;
    slot __ApiCallback98d591f9cc, __API_CALLBACK_98D591F9CC, __api_set_98d591f9cc;
    base orch_api_base::routes::resources::view;

    async fn environment_refs<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id], arg2: &'__api [orch_types::Id], arg3: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<Vec<(Option<orch_types::Id>, orch_api_contract::environment::EnvironmentRef)>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_a613953738, __api_install_a613953738, __api_callback_a613953738;
    slot __ApiCallbacka613953738, __API_CALLBACK_A613953738, __api_set_a613953738;
    base orch_api_base::routes::resources::view;

    async fn environment_refs_with_names<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id], arg2: &'__api [orch_types::Id], arg3: orch_types::Time, arg4: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<Vec<(Option<orch_types::Id>, orch_api_contract::environment::EnvironmentRef)>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_df4d6a0d97, __api_install_df4d6a0d97, __api_callback_df4d6a0d97;
    slot __ApiCallbackdf4d6a0d97, __API_CALLBACK_DF4D6A0D97, __api_set_df4d6a0d97;
    base orch_api_base::routes::resources::view;

    async fn environment_activity<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::environment::EnvironmentActivity>>;
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
    pub fn from_seen(seen: &'a orch_authz::resources::Seen) -> Self {
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
    pub fn from_effective(effective: &'a orch_authz::resources::Effective) -> Self {
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
pub struct Authority {
    pub permissions: HashSet<Permission>,
    pub owner: bool,
    pub administers: bool,
    pub participating: bool,
    pub admin_access: Option<AdminReach>,
    pub admin_place: Option<PlaceRef>,
    pub admin_only: HashSet<Permission>,
}

impl Authority {
    pub fn holding(held: &Holding<'_>) -> Self {
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
    pub fn seen(seen: &orch_authz::resources::Seen) -> Self {
        Self::holding(&Holding::from_seen(seen))
    }
    pub fn effective(held: &orch_authz::resources::Effective) -> Self {
        Self::holding(&Holding::from_effective(held))
    }
    pub fn preserved_by(&self, fresh: &Self) -> bool {
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
pub struct PreparedAccess {
    pub authority: HashMap<Id, Authority>,
    pub entries: Vec<stored::Entry>,
    pub places: Vec<Id>,
    pub names: NameSources,
}

impl PreparedAccess {
    /// Preserve the native holdings that selected the already produced protected shape.
    pub fn preserves_authority(&self, fresh: &Self) -> bool {
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
pub async fn prepare_accesses(
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
pub async fn accesses_prepared(
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
