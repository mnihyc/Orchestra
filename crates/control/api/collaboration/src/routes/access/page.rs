//! A page as the engine reads it, turned into the permission page a client draws: every holder by name, every entry with the person who wrote it, and every place it comes from.
//!
//! The engine speaks in identifiers. A page names people, groups and places,
//! and a client needs a name and a picture for each. They are looked up here,
//! once for the whole page, in a few statements however long the page is. An
//! entry is `outside` when its holder is, or reaches, somebody who is no
//! member of the root org the thing lies in.

use crate::middleware::authorization::refusals::node_kind;

use crate::middleware::authorization::refusals::user_refs;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::routes::nodes::view;

use orch_access_contract::pages::Entry;

use orch_access_contract::pages::Holder;

use orch_access_contract::permissions::Profile;

use orch_access_contract::roles::PlaceRef;

use orch_api_contract::access as api;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeRef;

use orch_api_contract::nodes::PageState;

use orch_api_contract::users::UserRef;

use orch_authz::giving::AgentLevel;

use orch_authz::giving::Gives;

use orch_authz::pages::PageView;

use orch_authz::pages::RowView;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::access;

use orch_types::Id;

use orch_types::Time;

use std::collections::BTreeSet;

use std::collections::HashMap;

use std::collections::HashSet;

pub use orch_api_base::routes::access::page::same;

pub use orch_api_base::routes::access::page::ProjectedUserRefs;

pub use orch_api_base::routes::access::page::NameBounds;

pub use orch_api_base::routes::access::page::NameSources;

pub use orch_api_base::routes::access::page::Names;

pub use orch_api_base::routes::access::page::Wanted;
/// A person always carries an address, limited to domains the reader may view.
pub(crate) async fn user_refs_at(
    conn: &mut Conn,
    principal: &Principal,
    root: Option<Id>,
    users: &[Id],
) -> ApiResult<HashMap<Id, UserRef>> {
    user_refs_at_bounded(conn, principal, root, users, None).await
}
orch_api_base::__api_bind_38b4b9df26!();

pub(crate) async fn user_refs_at_bounded(
    conn: &mut Conn,
    principal: &Principal,
    root: Option<Id>,
    users: &[Id],
    bounds: Option<&NameBounds>,
) -> ApiResult<HashMap<Id, UserRef>> {
    Ok(project_user_refs(conn, principal, root, users, bounds)
        .await?
        .users)
}
orch_api_base::__api_bind_68981fb735!();

/// Retain native address origins while the ordinary projection selects them, not from reply JSON.
pub(crate) async fn project_user_refs(
    conn: &mut Conn,
    principal: &Principal,
    root: Option<Id>,
    users: &[Id],
    bounds: Option<&NameBounds>,
) -> ApiResult<ProjectedUserRefs> {
    if users.is_empty() {
        return Ok(ProjectedUserRefs {
            users: HashMap::new(),
            addresses: Default::default(),
        });
    }
    let mut names = user_refs(conn, users).await?;
    let addresses = access::holder_user_addresses(conn, users).await?;
    let domains: Vec<Id> = addresses.iter().map(|(_, _, domain, _)| *domain).collect();
    let visible: HashSet<Id> = orch_authz::nodes::holding(
        conn,
        principal,
        &domains,
        orch_access_contract::permissions::PermissionName::OfNode(
            orch_access_contract::permissions::NodePermission::View,
        ),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|node| node.id)
    .collect();
    let mut sources = std::collections::BTreeMap::new();
    for (user, tree, domain, address) in addresses {
        if visible.contains(&domain)
            && bounds.is_none_or(|bounds| bounds.nodes.contains(&domain))
            && root.is_none_or(|root| root == tree)
            && let Some(named) = names.get_mut(&user)
        {
            named.address = Some(address);
            sources.insert(user, (tree, domain));
        }
    }
    Ok(ProjectedUserRefs {
        users: names,
        addresses: sources,
    })
}
orch_api_base::__api_bind_7a87b2e19a!();

async fn qualified_refs_bounded(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    bounds: Option<&NameBounds>,
) -> ApiResult<HashMap<Id, NodeRef>> {
    let Some(bounds) = bounds else {
        return view::qualified_refs(conn, principal, ids, Policy::default()).await;
    };
    let graph = orch_authz::engine::Graph::load(conn, ids)
        .await
        .map_err(ApiError::internal)?;
    let all: BTreeSet<Id> = ids
        .iter()
        .flat_map(|id| {
            graph
                .get(*id)
                .into_iter()
                .flat_map(|node| node.path.iter().copied())
        })
        .collect();
    let refs = view::refs_with_name_sources(
        conn,
        principal,
        &all.into_iter().collect::<Vec<_>>(),
        Policy::default(),
        &bounds.nodes,
    )
    .await?;
    Ok(ids
        .iter()
        .filter_map(|id| {
            let named = if bounds.nodes.contains(id) {
                view::qualified_from(&graph, &refs, *id)
            } else {
                refs.get(id).cloned()
            };
            named.map(|named| (*id, named))
        })
        .collect())
}
orch_api_base::__api_bind_2a8d78de40!();

/// The identities named by a page, without loading names, member lists or content.
pub(crate) fn page_name_sources(page: &PageView) -> NameSources {
    Wanted::page(page).sources(page.role_place)
}

/// The identities named by permission entries, including their recorded givers.
pub(crate) fn entry_name_sources(entries: &[Entry], role_place: Option<Id>) -> NameSources {
    Wanted::entries(entries).sources(role_place)
}
orch_api_base::__api_bind_f2ec241b11!();

/// The identities named by effective-access holder chains, using the ordinary holder walk.
pub(crate) fn holder_list_name_sources(
    lists: &[Vec<Holder>],
    role_place: Option<Id>,
) -> NameSources {
    Wanted::holder_lists(lists).sources(role_place)
}
orch_api_base::__api_bind_d24bfacb4d!();

async fn container_refs_bounded(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    bounds: Option<&NameBounds>,
) -> ApiResult<HashMap<Id, orch_api_contract::document_tree::DocumentContainerRef>> {
    let Some(bounds) = bounds else {
        return crate::routes::document_tree::container_refs(conn, principal, ids).await;
    };
    let admitted: Vec<Id> = ids
        .iter()
        .copied()
        .filter(|id| bounds.containers.contains(id))
        .collect();
    let mut refs: HashMap<_, _> = ids
        .iter()
        .map(|id| {
            (
                *id,
                orch_api_contract::document_tree::DocumentContainerRef {
                    id: *id,
                    kind:
                        orch_api_contract::document_tree::DocumentContainerMark::DocumentContainer,
                    name: None,
                    redacted: true,
                },
            )
        })
        .collect();
    refs.extend(crate::routes::document_tree::container_refs(conn, principal, &admitted).await?);
    Ok(refs)
}
orch_api_base::__api_bind_2632b760ea!();

fn profile(profile: Profile) -> api::RowProfile {
    match profile {
        Profile::Any => api::RowProfile::Any,
        Profile::Managed => api::RowProfile::Managed,
        Profile::Connected => api::RowProfile::Connected,
    }
}

fn row(row: &RowView, names: &Names) -> api::PermissionRow {
    api::PermissionRow {
        permission: row.permission.as_str().to_owned(),
        profile: profile(row.profile),
        inherit: row.inherit,
        inherited: row
            .inherited
            .as_ref()
            .map(|inherited| api::InheritedHolders {
                holders: inherited
                    .iter()
                    .filter_map(|each| names.holder(&each.entry.holder))
                    .collect(),
                entries: inherited
                    .iter()
                    .filter_map(|each| names.entry(&each.entry))
                    .collect(),
                from: inherited.first().and_then(|each| names.origin(each.from)),
            }),
        entries: row
            .entries
            .iter()
            .filter_map(|entry| names.entry(entry))
            .collect(),
        locked: row.locked.as_ref().map(|lock| api::RowLock {
            by: names.place(lock.by),
            here: lock.here,
        }),
        writable: row.writable,
        can_give: row.can_give,
    }
}

/// What the reader of a page may give, in the words of the API.
fn gives(gives: &Gives) -> api::PageGives {
    use orch_access_contract::permissions::Permission as P;
    use orch_api_contract::share::{ShareDocumentsLevel, ShareLevel, ShareParts};
    let held = |permission| gives.agent_permissions.contains(&permission);
    let level = |read, write, manage| {
        if held(manage) {
            Some(ShareLevel::Manage)
        } else if held(write) {
            Some(ShareLevel::Write)
        } else if held(read) {
            Some(ShareLevel::Read)
        } else {
            None
        }
    };
    api::PageGives {
        choices: gives.choices.iter().filter_map(same).collect(),
        roles: gives.roles.iter().filter_map(same).collect(),
        agents: gives
            .agents
            .iter()
            .map(|level| match level {
                AgentLevel::View => api::AgentLevel::View,
                AgentLevel::Edit => api::AgentLevel::Edit,
                AgentLevel::Manage => api::AgentLevel::Manage,
            })
            .collect(),
        agent_parts: ShareParts {
            timeline: level(P::ThreadView, P::ThreadInteract, P::ThreadManage),
            chat: level(P::ChatView, P::ChatInteract, P::ChatManage),
            thread_fork: held(P::ThreadFork).then_some(true),
            thread_archive: held(P::ThreadArchive).then_some(true),
            documents: if held(P::DocManage) {
                Some(ShareDocumentsLevel::Manage)
            } else if held(P::DocEdit) {
                Some(ShareDocumentsLevel::Edit)
            } else if held(P::DocView) {
                Some(ShareDocumentsLevel::Read)
            } else {
                None
            },
        },
        holders_share: gives.holders_share,
    }
}

/// The permission page a client draws from what the engine read.
pub async fn permission_page(
    conn: &mut Conn,
    principal: &Principal,
    view_of: &PageView,
    root: Option<Id>,
    now: Time,
) -> ApiResult<api::PermissionPage> {
    permission_page_with(conn, principal, view_of, root, now, None, None).await
}
orch_api_base::__api_bind_bd744fe3cb!();

/// The same page projection with operation-admitted external names. Its own ACL information,
/// counts and object name retain the existing page authority, including Manage-only inspection.
/// The caller admits full Group and dependency sources before this producer materializes them.
pub(crate) async fn permission_page_bounded(
    conn: &mut Conn,
    principal: &Principal,
    view_of: &PageView,
    root: Option<Id>,
    now: Time,
    bounds: &NameBounds,
    dependencies: &super::dependencies::PreparedDependencies,
) -> ApiResult<api::PermissionPage> {
    permission_page_with(
        conn,
        principal,
        view_of,
        root,
        now,
        bounds.limited.then_some(bounds),
        Some(dependencies),
    )
    .await
}

async fn permission_page_with(
    conn: &mut Conn,
    principal: &Principal,
    view_of: &PageView,
    root: Option<Id>,
    now: Time,
    bounds: Option<&NameBounds>,
    dependencies: Option<&super::dependencies::PreparedDependencies>,
) -> ApiResult<api::PermissionPage> {
    let names = Names::load(
        conn,
        principal,
        root,
        view_of.role_place,
        now,
        Wanted::page(view_of),
        bounds,
    )
    .await?;
    let admin_places: Vec<Id> = view_of
        .admins_of
        .iter()
        .filter_map(|place| place.node())
        .collect();
    let admins = access::admin_people(
        conn,
        &admin_places,
        now.get(),
        view_of.admins_of.contains(&PlaceRef::System),
    )
    .await?;
    let kind = view_of
        .kind
        .map(|kind| node_kind(kind.as_str()).unwrap_or(NodeKind::Org));
    let target = match view_of.target {
        orch_authz::pages::Target::Live(id) => Some(id),
        orch_authz::pages::Target::Defaults(place) => place.node(),
        orch_authz::pages::Target::Resource(object) => Some(object.id),
    };
    // The page of a resource or a document says what it is by the kind it has, as the address of
    // its requests does; only the defaults of the system are of no kind but the system.
    let kind_word = match view_of.target {
        orch_authz::pages::Target::Resource(object) => object.kind.as_str().to_owned(),
        orch_authz::pages::Target::Live(_) | orch_authz::pages::Target::Defaults(_) => kind
            .map_or_else(
                || "system".to_owned(),
                |kind| same::<_, String>(&kind).unwrap_or_default(),
            ),
    };
    Ok(api::PermissionPage {
        object: api::ObjectRef {
            kind: kind_word,
            id: target.unwrap_or_else(Id::nil),
            name: view_of.name.clone(),
        },
        page: match view_of.page {
            orch_access_contract::pages::PageKind::Live => api::PageKind::Live,
            orch_access_contract::pages::PageKind::Defaults => api::PageKind::Defaults,
        },
        owner: view_of
            .first_owner
            .and_then(|owner| names.users.get(&owner).cloned()),
        owners: view_of
            .owners
            .iter()
            .filter_map(|entry| names.entry(entry))
            .collect(),
        owners_writable: view_of.owners_writable,
        external: view_of.external,
        admins: api::PageAdmins {
            of: view_of
                .admins_of
                .iter()
                .map(|place| names.place(*place))
                .collect(),
            count: i64::try_from(admins.len()).unwrap_or(i64::MAX),
        },
        rows: view_of.rows.iter().map(|each| row(each, &names)).collect(),
        state: match view_of.state {
            orch_access_contract::pages::PageState::Follows => PageState::Follows,
            orch_access_contract::pages::PageState::Custom => PageState::Custom,
            orch_access_contract::pages::PageState::Restricted => PageState::Restricted,
        },
        followers: view_of.followers,
        gives: gives(&view_of.gives),
        covers: None,
        depends: match dependencies {
            Some(prepared) => {
                super::dependencies::project_page(conn, principal, prepared, bounds).await?
            }
            None => super::dependencies::of_page(conn, principal, view_of).await?,
        },
        reaches: None,
        revision: view_of.revision,
    })
}

/// The entries `entries` with their names, for a view that is not a page:
/// what gives a person a permission. A role that names no place is counted
/// as the people who hold it anywhere; [`entries_named_at`] says where.
pub async fn entries_named(
    conn: &mut Conn,
    principal: &Principal,
    root: Option<Id>,
    entries: &[Entry],
) -> ApiResult<HashMap<Id, api::Entry>> {
    entries_named_at(conn, principal, root, None, entries).await
}
orch_api_base::__api_bind_3af1ff1c83!();

/// [`entries_named`] for entries of a thing whose roles that name no place are held in `role_place`.
pub async fn entries_named_at(
    conn: &mut Conn,
    principal: &Principal,
    root: Option<Id>,
    role_place: Option<Id>,
    entries: &[Entry],
) -> ApiResult<HashMap<Id, api::Entry>> {
    let mut wanted = Wanted::default();
    for entry in entries {
        wanted.entry(entry);
    }
    let names = Names::load(
        conn,
        principal,
        root,
        role_place,
        principal.now(),
        wanted,
        None,
    )
    .await?;
    Ok(entries
        .iter()
        .filter_map(|entry| names.entry(entry).map(|named| (entry.id, named)))
        .collect())
}

/// [`entries_named_at`] with the caller's current operation-level name bound.
pub(crate) async fn entries_named_at_bounded(
    conn: &mut Conn,
    principal: &Principal,
    root: Option<Id>,
    role_place: Option<Id>,
    entries: &[Entry],
    bounds: &NameBounds,
) -> ApiResult<HashMap<Id, api::Entry>> {
    let mut wanted = Wanted::default();
    for entry in entries {
        wanted.entry(entry);
    }
    let names = Names::load(
        conn,
        principal,
        root,
        role_place,
        principal.now(),
        wanted,
        bounds.limited.then_some(bounds),
    )
    .await?;
    Ok(entries
        .iter()
        .filter_map(|entry| names.entry(entry).map(|named| (entry.id, named)))
        .collect())
}
orch_api_base::__api_bind_88985d68e3!();
