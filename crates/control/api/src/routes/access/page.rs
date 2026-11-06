//! A page as the engine reads it, turned into the permission page a client draws: every holder by name, every entry with the person who wrote it, and every place it comes from.
//!
//! The engine speaks in identifiers. A page names people, groups and places,
//! and a client needs a name and a picture for each. They are looked up here,
//! once for the whole page, in a few statements however long the page is. An
//! entry is `outside` when its holder is, or reaches, somebody who is no
//! member of the root org the thing lies in.

use crate::middleware::authorization::refusals::{node_kind, user_refs};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::routes::nodes::view;
use orch_access_contract::pages::{Entry, Holder, Origin};
use orch_access_contract::permissions::Profile;
use orch_access_contract::roles::{PlaceRef, Role};
use orch_api_contract::access as api;
use orch_api_contract::directory::Group;
use orch_api_contract::nodes::{NodeKind, NodeOrSystem, NodeRef, PageState};
use orch_api_contract::users::UserRef;
use orch_authz::giving::{AgentLevel, Gives};
use orch_authz::pages::{PageView, RowView};
use orch_authz::{Act, Policy, Principal};
use orch_db::Conn;
use orch_db::queries::{access, hierarchy};
use orch_types::{Id, Time};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::{BTreeSet, HashMap, HashSet};

/// A word of one contract as the same word of another.
pub fn same<A: Serialize, B: DeserializeOwned>(word: &A) -> Option<B> {
    serde_json::to_value(word)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

/// A person always carries an address, limited to domains the reader may view.
pub(crate) async fn user_refs_at(
    conn: &mut Conn,
    principal: &Principal,
    root: Option<Id>,
    users: &[Id],
) -> ApiResult<HashMap<Id, UserRef>> {
    user_refs_at_bounded(conn, principal, root, users, None).await
}

pub(crate) async fn user_refs_at_bounded(
    conn: &mut Conn,
    principal: &Principal,
    root: Option<Id>,
    users: &[Id],
    bounds: Option<&NameBounds>,
) -> ApiResult<HashMap<Id, UserRef>> {
    if users.is_empty() {
        return Ok(HashMap::new());
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
    for (user, tree, domain, address) in addresses {
        if visible.contains(&domain)
            && bounds.is_none_or(|bounds| bounds.nodes.contains(&domain))
            && root.is_none_or(|root| root == tree)
            && let Some(named) = names.get_mut(&user)
        {
            named.address = Some(address);
        }
    }
    Ok(names)
}

/// Names already admitted by this operation's shared typed scope binder. Ordinary page readers
/// pass no bound; this only narrows protected names and domain-derived addresses.
#[derive(Clone, Default)]
pub(crate) struct NameBounds {
    /// False for ordinary callers: preserve the exact existing unbounded name producer.
    pub(crate) limited: bool,
    pub(crate) nodes: BTreeSet<Id>,
    pub(crate) containers: BTreeSet<Id>,
}
impl NameBounds {
    pub(crate) async fn refs(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        ids: &[Id],
    ) -> ApiResult<HashMap<Id, NodeRef>> {
        if self.limited {
            view::refs_with_name_sources(conn, principal, ids, Policy::default(), &self.nodes).await
        } else {
            view::refs(conn, principal, ids, Policy::default()).await
        }
    }

    /// Names the same visible qualified paths as the page, within this operation's bound.
    pub(crate) async fn qualified_refs(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        ids: &[Id],
    ) -> ApiResult<HashMap<Id, NodeRef>> {
        qualified_refs_bounded(conn, principal, ids, self.limited.then_some(self)).await
    }

    /// Keeps every folder identity and its ordinary redaction while bounding name lookup.
    pub(crate) async fn container_refs(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        ids: &[Id],
    ) -> ApiResult<HashMap<Id, orch_api_contract::document_tree::DocumentContainerRef>> {
        container_refs_bounded(conn, principal, ids, self.limited.then_some(self)).await
    }
}

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

/// The source identities the existing page projection may name. Public people remain public;
/// their identifiers let the caller resolve optional domain-derived addresses. Group DTOs need
/// admission as a whole before projection because they have no redacted wire form.
#[derive(Clone, Default)]
pub(crate) struct NameSources {
    pub(crate) users: BTreeSet<Id>,
    pub(crate) groups: BTreeSet<Id>,
    pub(crate) nodes: BTreeSet<Id>,
    pub(crate) containers: BTreeSet<Id>,
}

/// The identities named by a page, without loading names, member lists or content.
pub(crate) fn page_name_sources(page: &PageView) -> NameSources {
    Wanted::page(page).sources(page.role_place)
}

/// The identities named by permission entries, including their recorded givers.
pub(crate) fn entry_name_sources(entries: &[Entry], role_place: Option<Id>) -> NameSources {
    Wanted::entries(entries).sources(role_place)
}

/// The identities named by effective-access holder chains, using the ordinary holder walk.
pub(crate) fn holder_list_name_sources(
    lists: &[Vec<Holder>],
    role_place: Option<Id>,
) -> NameSources {
    Wanted::holder_lists(lists).sources(role_place)
}

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

/// Everyone and everything a page names, looked up.
pub struct Names {
    read_first: HashSet<Id>,
    users: HashMap<Id, UserRef>,
    groups: HashMap<Id, Group>,
    nodes: HashMap<Id, NodeRef>,
    containers: HashMap<Id, orch_api_contract::document_tree::DocumentContainerRef>,
    /// Members of the root org and service identities kept by its orgs.
    local_holders: HashSet<Id>,
    /// The root org of the groups and places named.
    roots: HashMap<Id, Id>,
    root: Option<Id>,
    /// How many people each role named in the page stands for, by the role and the place it is held in.
    counts: HashMap<(Role, Option<Id>), i64>,
    /// Where a role that names no place is held for the thing.
    role_place: Option<Id>,
}

/// The identifiers a set of entries and places mention.
#[derive(Default)]
struct Wanted {
    entries: Vec<Id>,
    users: Vec<Id>,
    groups: Vec<Id>,
    nodes: Vec<Id>,
    containers: Vec<Id>,
    roles: Vec<(Role, Option<Id>)>,
}

impl Wanted {
    fn page(page: &PageView) -> Self {
        let mut wanted = Self::default();
        wanted.users.extend(page.first_owner);
        for entry in &page.owners {
            wanted.entry(entry);
        }
        for row in &page.rows {
            for entry in &row.entries {
                wanted.entry(entry);
            }
            for inherited in row.inherited.iter().flatten() {
                wanted.entry(&inherited.entry);
                wanted.origin(inherited.from);
            }
            if let Some(lock) = &row.locked {
                wanted.place(lock.by);
            }
        }
        for place in &page.admins_of {
            wanted.place(*place);
        }
        wanted
    }

    fn entries(entries: &[Entry]) -> Self {
        let mut wanted = Self::default();
        for entry in entries {
            wanted.entry(entry);
        }
        wanted
    }

    fn holder_lists(lists: &[Vec<Holder>]) -> Self {
        let mut wanted = Self::default();
        for holder in lists.iter().flatten() {
            wanted.holder(holder);
        }
        wanted
    }

    fn sources(self, role_place: Option<Id>) -> NameSources {
        NameSources {
            users: self.users.into_iter().collect(),
            groups: self.groups.into_iter().collect(),
            nodes: self.nodes.into_iter().chain(role_place).collect(),
            containers: self.containers.into_iter().collect(),
        }
    }

    fn holder(&mut self, holder: &Holder) {
        match *holder {
            Holder::Role { role, at } => {
                self.nodes.extend(at);
                self.roles.push((role, at));
            }
            Holder::Group { id } => self.groups.push(id),
            Holder::User { id } => self.users.push(id),
            Holder::Node { id } | Holder::Agents { id } | Holder::Members { id } => {
                self.nodes.push(id)
            }
            Holder::Everyone => {}
        }
    }

    fn entry(&mut self, entry: &Entry) {
        if matches!(&entry.holder, Holder::Agents { .. }) {
            self.entries.push(entry.id);
        }
        self.holder(&entry.holder);
        self.users.extend(entry.created_by);
    }

    fn place(&mut self, place: PlaceRef) {
        self.nodes.extend(place.node());
    }

    fn origin(&mut self, origin: Origin) {
        match origin {
            Origin::Own => {}
            Origin::Node(node) => self.nodes.push(node),
            Origin::DocumentContainer(id) => self.containers.push(id),
            Origin::Defaults(place) => self.place(place),
        }
    }
}

impl Names {
    /// Ordinary permission entries, using exactly the page's holder and identity redaction.
    pub(crate) async fn entries(
        conn: &mut Conn,
        principal: &Principal,
        root: Option<Id>,
        place: Option<Id>,
        entries: &[Entry],
    ) -> ApiResult<Vec<api::Entry>> {
        let mut wanted = Wanted::default();
        for entry in entries {
            wanted.entry(entry);
        }
        let names = Self::load(conn, principal, root, place, principal.now(), wanted, None).await?;
        Ok(entries
            .iter()
            .filter_map(|entry| names.entry(entry))
            .collect())
    }
    /// Names a list of holders by the same visibility rules as a permission page.
    pub(crate) async fn holders(
        conn: &mut Conn,
        principal: &Principal,
        root: Option<Id>,
        place: Option<Id>,
        holders: &[Holder],
    ) -> ApiResult<Vec<api::Holder>> {
        let mut wanted = Wanted::default();
        for holder in holders {
            wanted.holder(holder);
        }
        let names = Self::load(conn, principal, root, place, principal.now(), wanted, None).await?;
        Ok(holders
            .iter()
            .filter_map(|holder| names.holder(holder))
            .collect())
    }
    /// The same holder producer with operation-admitted protected names. Group data itself
    /// must already be admitted by the caller because Group has no redacted DTO variant.
    pub(crate) async fn holders_bounded(
        conn: &mut Conn,
        principal: &Principal,
        root: Option<Id>,
        place: Option<Id>,
        holders: &[Holder],
        bounds: &NameBounds,
    ) -> ApiResult<Vec<api::Holder>> {
        let mut wanted = Wanted::default();
        for holder in holders {
            wanted.holder(holder);
        }
        let names = Self::load(
            conn,
            principal,
            root,
            place,
            principal.now(),
            wanted,
            bounds.limited.then_some(bounds),
        )
        .await?;
        Ok(holders
            .iter()
            .filter_map(|holder| names.holder(holder))
            .collect())
    }

    /// The same entry producer with operation-admitted protected names.
    pub(crate) async fn entries_bounded(
        conn: &mut Conn,
        principal: &Principal,
        root: Option<Id>,
        place: Option<Id>,
        entries: &[Entry],
        bounds: &NameBounds,
    ) -> ApiResult<Vec<api::Entry>> {
        let named = entries_named_at_bounded(conn, principal, root, place, entries, bounds).await?;
        Ok(entries
            .iter()
            .filter_map(|entry| named.get(&entry.id).cloned())
            .collect())
    }

    /// [`Names::holders`] of several lists, every list named as that would name it, with one
    /// lookup for all of them: a holder's name and head count depend on that holder alone.
    pub(crate) async fn holder_lists(
        conn: &mut Conn,
        principal: &Principal,
        root: Option<Id>,
        place: Option<Id>,
        lists: &[Vec<Holder>],
    ) -> ApiResult<Vec<Vec<api::Holder>>> {
        Self::holder_lists_with(conn, principal, root, place, lists, None).await
    }

    /// The same holder-chain projection with operation-admitted names. The caller admits each
    /// full Group source first; role head counts remain metadata of the authorized page.
    pub(crate) async fn holder_lists_bounded(
        conn: &mut Conn,
        principal: &Principal,
        root: Option<Id>,
        place: Option<Id>,
        lists: &[Vec<Holder>],
        bounds: &NameBounds,
    ) -> ApiResult<Vec<Vec<api::Holder>>> {
        Self::holder_lists_with(
            conn,
            principal,
            root,
            place,
            lists,
            bounds.limited.then_some(bounds),
        )
        .await
    }

    async fn holder_lists_with(
        conn: &mut Conn,
        principal: &Principal,
        root: Option<Id>,
        place: Option<Id>,
        lists: &[Vec<Holder>],
        bounds: Option<&NameBounds>,
    ) -> ApiResult<Vec<Vec<api::Holder>>> {
        let names = Self::load(
            conn,
            principal,
            root,
            place,
            principal.now(),
            Wanted::holder_lists(lists),
            bounds,
        )
        .await?;
        Ok(lists
            .iter()
            .map(|holders| {
                holders
                    .iter()
                    .filter_map(|holder| names.holder(holder))
                    .collect()
            })
            .collect())
    }
    /// Looks up everything `entries` and `places` name, for a thing whose
    /// root org is `root`.
    async fn load(
        conn: &mut Conn,
        principal: &Principal,
        root: Option<Id>,
        role_place: Option<Id>,
        now: Time,
        wanted: Wanted,
        bounds: Option<&NameBounds>,
    ) -> ApiResult<Self> {
        // How many people the roles named stand for, for all of them in one statement.
        let mut chips: Vec<(Role, Option<Id>)> =
            wanted.roles.iter().map(|(role, at)| (*role, *at)).collect();
        chips.sort_unstable();
        chips.dedup();
        let words: Vec<(String, Option<Id>, bool)> = chips
            .iter()
            .map(|(role, place)| {
                (
                    role.as_str().to_owned(),
                    place.or(role_place),
                    place.is_some(),
                )
            })
            .collect();
        let heads = orch_authz::holders::role_head_counts(conn, &words, now)
            .await
            .map_err(ApiError::internal)?;
        let read_first: HashSet<Id> = if wanted.entries.is_empty() {
            HashSet::new()
        } else {
            sqlx::query_scalar("SELECT id FROM permission_entries WHERE id=ANY($1) AND read_first AND holder_kind='agents' AND permission='doc.view'")
                .bind(&wanted.entries).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?.into_iter().collect()
        };
        let counts: HashMap<(Role, Option<Id>), i64> = chips.into_iter().zip(heads).collect();
        let mut users = wanted.users;
        users.sort_unstable();
        users.dedup();
        let mut groups = wanted.groups;
        groups.sort_unstable();
        groups.dedup();
        let mut nodes = wanted.nodes;
        nodes.extend(role_place);
        nodes.sort_unstable();
        nodes.dedup();
        let user_names = user_refs_at_bounded(conn, principal, root, &users, bounds).await?;
        let group_rows = if groups.is_empty() {
            Vec::new()
        } else {
            access::groups_full(conn, &groups).await?
        };
        let node_refs = qualified_refs_bounded(conn, principal, &nodes, bounds).await?;
        let container_refs =
            container_refs_bounded(conn, principal, &wanted.containers, bounds).await?;
        let group_orgs: Vec<Id> = group_rows.iter().map(|row| row.org_id).collect();
        let group_places = qualified_refs_bounded(conn, principal, &group_orgs, bounds).await?;
        let group_people = if group_rows.is_empty() {
            Vec::new()
        } else {
            orch_authz::holders::group_people(conn, &groups, now)
                .await
                .map_err(ApiError::internal)?
        };
        let mut group_counts = HashMap::<Id, i64>::new();
        for (group, _) in group_people {
            *group_counts.entry(group).or_default() += 1;
        }
        let node_rows = if nodes.is_empty() {
            Vec::new()
        } else {
            hierarchy::with_ancestors(conn, &nodes).await?
        };
        let local_holders: HashSet<Id> = match root {
            Some(root) if !users.is_empty() => access::local_holders_among(conn, root, &users)
                .await?
                .into_iter()
                .collect(),
            _ => HashSet::new(),
        };
        let mut roots: HashMap<Id, Id> =
            group_rows.iter().map(|row| (row.id, row.root_id)).collect();
        roots.extend(node_rows.iter().map(|row| (row.id, row.org_id)));
        Ok(Self {
            read_first,
            users: user_names,
            groups: group_rows
                .into_iter()
                .map(|row| {
                    (
                        row.id,
                        Group {
                            id: row.id,
                            org_id: row.org_id,
                            place: group_places
                                .get(&row.org_id)
                                .and_then(|place| place.name.clone()),
                            name: row.name,
                            description: row.description,
                            member_count: group_counts.get(&row.id).copied().unwrap_or(0),
                        },
                    )
                })
                .collect(),
            nodes: node_refs,
            containers: container_refs,
            local_holders,
            roots,
            root,
            counts,
            role_place,
        })
    }

    fn node(&self, id: Id) -> NodeRef {
        self.nodes.get(&id).cloned().unwrap_or(NodeRef {
            thread_display: None,
            id,
            kind: NodeKind::Org,
            name: None,
            redacted: true,
        })
    }

    fn place(&self, place: PlaceRef) -> NodeOrSystem {
        match place {
            PlaceRef::System => NodeOrSystem::SYSTEM,
            PlaceRef::Node(id) => NodeOrSystem::Node(self.node(id)),
        }
    }

    fn origin(&self, origin: Origin) -> Option<NodeOrSystem> {
        match origin {
            Origin::Own => None,
            Origin::Node(node) => Some(NodeOrSystem::Node(self.node(node))),
            Origin::DocumentContainer(id) => Some(NodeOrSystem::DocumentContainer(
                self.containers
                    .get(&id)
                    .cloned()
                    .unwrap_or(orch_api_contract::document_tree::DocumentContainerRef {
                    id,
                    kind:
                        orch_api_contract::document_tree::DocumentContainerMark::DocumentContainer,
                    name: None,
                    redacted: true,
                }),
            )),
            Origin::Defaults(place) => Some(self.place(place)),
        }
    }

    /// Whether `id`, a group or a place, is of another tree than the thing.
    fn elsewhere(&self, id: Id) -> bool {
        match (self.root, self.roots.get(&id)) {
            (Some(root), Some(found)) => *found != root,
            _ => false,
        }
    }

    /// The holder with its names.
    fn holder(&self, holder: &Holder) -> Option<api::Holder> {
        Some(match *holder {
            Holder::Role { role, at } => api::Holder::Role {
                role: same(&role)?,
                at: at.map(|place| self.node(place)),
                place: at
                    .or(self.role_place)
                    .and_then(|place| self.node(place).name),
                count: self.counts.get(&(role, at)).copied().unwrap_or(0),
            },
            Holder::Group { id } => api::Holder::Group {
                group: self.groups.get(&id)?.clone(),
            },
            Holder::User { id } => api::Holder::User {
                user: self.users.get(&id)?.clone(),
            },
            Holder::Node { id } => api::Holder::Node {
                node: self.node(id),
            },
            Holder::Agents { id } => api::Holder::Agents {
                node: self.node(id),
            },
            Holder::Members { id } => api::Holder::Members {
                node: self.node(id),
            },
            Holder::Everyone => api::Holder::Everyone,
        })
    }

    /// Whether the holder is, or reaches, somebody outside the root org.
    fn outside(&self, holder: &Holder) -> bool {
        match *holder {
            Holder::User { id } => self.root.is_some() && !self.local_holders.contains(&id),
            Holder::Group { id }
            | Holder::Node { id }
            | Holder::Agents { id }
            | Holder::Members { id } => self.elsewhere(id),
            Holder::Role { at, .. } => at.is_some_and(|place| self.elsewhere(place)),
            Holder::Everyone => false,
        }
    }

    /// The entry with the names of everyone it mentions; none when its
    /// holder is gone (a user or a group that does not exist any more).
    pub fn entry(&self, entry: &Entry) -> Option<api::Entry> {
        Some(api::Entry {
            read_first: self.read_first.contains(&entry.id),
            id: entry.id,
            holder: self.holder(&entry.holder)?,
            options: entry.options.clone(),
            outside: self.outside(&entry.holder),
            expires_at: entry.expires_at,
            note: entry.note.clone(),
            source: same(&entry.source)?,
            created_by: entry.created_by.and_then(|by| self.users.get(&by).cloned()),
            created_at: entry.created_at,
            revision: entry.revision,
        })
    }
}

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
    let level = |read, write| {
        if held(write) {
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
            timeline: level(P::ThreadView, P::ThreadInteract),
            chat: level(P::ChatView, P::ChatInteract),
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
