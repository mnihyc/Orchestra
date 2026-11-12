//! A page as the engine reads it, turned into the permission page a client draws: every holder by name, every entry with the person who wrote it, and every place it comes from.
//!
//! The engine speaks in identifiers. A page names people, groups and places,
//! and a client needs a name and a picture for each. They are looked up here,
//! once for the whole page, in a few statements however long the page is. An
//! entry is `outside` when its holder is, or reaches, somebody who is no
//! member of the root org the thing lies in.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::routes::nodes::view;

pub use orch_access_contract::pages::Entry;

pub use orch_access_contract::pages::Holder;

use orch_access_contract::pages::Origin;

pub use orch_access_contract::roles::PlaceRef;

use orch_access_contract::roles::Role;

pub use orch_api_contract::access as api;

use orch_api_contract::directory::Group;

pub use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeOrSystem;

pub use orch_api_contract::nodes::NodeRef;

pub use orch_api_contract::users::UserRef;

pub use orch_authz::pages::PageView;

pub use orch_authz::Policy;

pub use orch_authz::Principal;

pub use orch_db::Conn;

pub use orch_db::queries::access;

use orch_db::queries::hierarchy;

pub use orch_types::Id;

pub use orch_types::Time;

use serde::Serialize;

use serde::de::DeserializeOwned;

pub use std::collections::BTreeSet;

pub use std::collections::HashMap;

pub use std::collections::HashSet;
/// A word of one contract as the same word of another.
pub fn same<A: Serialize, B: DeserializeOwned>(word: &A) -> Option<B> {
    serde_json::to_value(word)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

orch_api_base::api_family_callback! {
    bind __api_bind_38b4b9df26, __api_install_38b4b9df26, __api_callback_38b4b9df26;
    slot __ApiCallback38b4b9df26, __API_CALLBACK_38B4B9DF26, __api_set_38b4b9df26;
    base orch_api_base::routes::access::page;

    async fn user_refs_at<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Option<orch_types::Id>, arg3: &'__api [orch_types::Id]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::users::UserRef>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_68981fb735, __api_install_68981fb735, __api_callback_68981fb735;
    slot __ApiCallback68981fb735, __API_CALLBACK_68981FB735, __api_set_68981fb735;
    base orch_api_base::routes::access::page;

    async fn user_refs_at_bounded<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Option<orch_types::Id>, arg3: &'__api [orch_types::Id], arg4: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::users::UserRef>>;
}

/// The existing public references and the exact membership that supplied each emitted address.
pub struct ProjectedUserRefs {
    pub users: HashMap<Id, UserRef>,
    pub addresses: std::collections::BTreeMap<Id, (Id, Id)>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_7a87b2e19a, __api_install_7a87b2e19a, __api_callback_7a87b2e19a;
    slot __ApiCallback7a87b2e19a, __API_CALLBACK_7A87B2E19A, __api_set_7a87b2e19a;
    base orch_api_base::routes::access::page;

    async fn project_user_refs<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Option<orch_types::Id>, arg3: &'__api [orch_types::Id], arg4: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::access::page::ProjectedUserRefs>;
}

/// Names already admitted by this operation's shared typed scope binder. Ordinary page readers
/// pass no bound; this only narrows protected names and domain-derived addresses.
#[derive(Clone, Default)]
pub struct NameBounds {
    /// False for ordinary callers: preserve the exact existing unbounded name producer.
    pub limited: bool,
    pub nodes: BTreeSet<Id>,
    pub containers: BTreeSet<Id>,
}

impl NameBounds {
    pub async fn refs(
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
    pub async fn qualified_refs(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        ids: &[Id],
    ) -> ApiResult<HashMap<Id, NodeRef>> {
        qualified_refs_bounded(conn, principal, ids, self.limited.then_some(self)).await
    }

    /// Keeps every folder identity and its ordinary redaction while bounding name lookup.
    pub async fn container_refs(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        ids: &[Id],
    ) -> ApiResult<HashMap<Id, orch_api_contract::document_tree::DocumentContainerRef>> {
        container_refs_bounded(conn, principal, ids, self.limited.then_some(self)).await
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_2a8d78de40, __api_install_2a8d78de40, __api_callback_2a8d78de40;
    slot __ApiCallback2a8d78de40, __API_CALLBACK_2A8D78DE40, __api_set_2a8d78de40;
    base orch_api_base::routes::access::page;

    async fn qualified_refs_bounded<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::nodes::NodeRef>>;
}

/// The source identities the existing page projection may name. Public people remain public;
/// their identifiers let the caller resolve optional domain-derived addresses. Group DTOs need
/// admission as a whole before projection because they have no redacted wire form.
#[derive(Clone, Default)]
pub struct NameSources {
    pub users: BTreeSet<Id>,
    pub groups: BTreeSet<Id>,
    pub nodes: BTreeSet<Id>,
    pub containers: BTreeSet<Id>,
}

orch_api_base::api_family_callback! {
    bind __api_bind_f2ec241b11, __api_install_f2ec241b11, __api_callback_f2ec241b11;
    slot __ApiCallbackf2ec241b11, __API_CALLBACK_F2EC241B11, __api_set_f2ec241b11;
    base orch_api_base::routes::access::page;

    fn entry_name_sources<'__api>(arg0: &'__api [orch_access_contract::pages::Entry], arg1: Option<orch_types::Id>) -> orch_api_base::routes::access::page::NameSources;
}

orch_api_base::api_family_callback! {
    bind __api_bind_d24bfacb4d, __api_install_d24bfacb4d, __api_callback_d24bfacb4d;
    slot __ApiCallbackd24bfacb4d, __API_CALLBACK_D24BFACB4D, __api_set_d24bfacb4d;
    base orch_api_base::routes::access::page;

    fn holder_list_name_sources<'__api>(arg0: &'__api [Vec<orch_access_contract::pages::Holder>], arg1: Option<orch_types::Id>) -> orch_api_base::routes::access::page::NameSources;
}

orch_api_base::api_family_callback! {
    bind __api_bind_2632b760ea, __api_install_2632b760ea, __api_callback_2632b760ea;
    slot __ApiCallback2632b760ea, __API_CALLBACK_2632B760EA, __api_set_2632b760ea;
    base orch_api_base::routes::access::page;

    async fn container_refs_bounded<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api [orch_types::Id], arg3: Option<&'__api orch_api_base::routes::access::page::NameBounds>) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::document_tree::DocumentContainerRef>>;
}

/// Everyone and everything a page names, looked up.
pub struct Names {
    pub read_first: HashSet<Id>,
    pub users: HashMap<Id, UserRef>,
    pub groups: HashMap<Id, Group>,
    pub nodes: HashMap<Id, NodeRef>,
    pub containers: HashMap<Id, orch_api_contract::document_tree::DocumentContainerRef>,
    /// Members of the root org and service identities kept by its orgs.
    pub local_holders: HashSet<Id>,
    /// The root org of the groups and places named.
    pub roots: HashMap<Id, Id>,
    pub root: Option<Id>,
    /// How many people each role named in the page stands for, by the role and the place it is held in.
    pub counts: HashMap<(Role, Option<Id>), i64>,
    /// Where a role that names no place is held for the thing.
    pub role_place: Option<Id>,
}

/// The identifiers a set of entries and places mention.
#[derive(Default)]
pub struct Wanted {
    pub entries: Vec<Id>,
    pub users: Vec<Id>,
    pub groups: Vec<Id>,
    pub nodes: Vec<Id>,
    pub containers: Vec<Id>,
    pub roles: Vec<(Role, Option<Id>)>,
}

impl Wanted {
    pub fn page(page: &PageView) -> Self {
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

    pub fn entries(entries: &[Entry]) -> Self {
        let mut wanted = Self::default();
        for entry in entries {
            wanted.entry(entry);
        }
        wanted
    }

    pub fn holder_lists(lists: &[Vec<Holder>]) -> Self {
        let mut wanted = Self::default();
        for holder in lists.iter().flatten() {
            wanted.holder(holder);
        }
        wanted
    }

    pub fn sources(self, role_place: Option<Id>) -> NameSources {
        NameSources {
            users: self.users.into_iter().collect(),
            groups: self.groups.into_iter().collect(),
            nodes: self.nodes.into_iter().chain(role_place).collect(),
            containers: self.containers.into_iter().collect(),
        }
    }

    pub fn holder(&mut self, holder: &Holder) {
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

    pub fn entry(&mut self, entry: &Entry) {
        if matches!(&entry.holder, Holder::Agents { .. }) {
            self.entries.push(entry.id);
        }
        self.holder(&entry.holder);
        self.users.extend(entry.created_by);
    }

    pub fn place(&mut self, place: PlaceRef) {
        self.nodes.extend(place.node());
    }

    pub fn origin(&mut self, origin: Origin) {
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
    pub async fn entries(
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
    pub async fn holders(
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
    pub async fn holders_bounded(
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
    pub async fn entries_bounded(
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
    pub async fn holder_lists(
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
    pub async fn holder_lists_bounded(
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

    pub async fn holder_lists_with(
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
    pub async fn load(
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
                            revision: row.revision,
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

    pub fn node(&self, id: Id) -> NodeRef {
        self.nodes.get(&id).cloned().unwrap_or(NodeRef {
            thread_display: None,
            id,
            kind: NodeKind::Org,
            name: None,
            redacted: true,
        })
    }

    pub fn place(&self, place: PlaceRef) -> NodeOrSystem {
        match place {
            PlaceRef::System => NodeOrSystem::SYSTEM,
            PlaceRef::Node(id) => NodeOrSystem::Node(self.node(id)),
        }
    }

    pub fn origin(&self, origin: Origin) -> Option<NodeOrSystem> {
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
    pub fn elsewhere(&self, id: Id) -> bool {
        match (self.root, self.roots.get(&id)) {
            (Some(root), Some(found)) => *found != root,
            _ => false,
        }
    }

    /// The holder with its names.
    pub fn holder(&self, holder: &Holder) -> Option<api::Holder> {
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
    pub fn outside(&self, holder: &Holder) -> bool {
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

orch_api_base::api_family_callback! {
    bind __api_bind_bd744fe3cb, __api_install_bd744fe3cb, __api_callback_bd744fe3cb;
    slot __ApiCallbackbd744fe3cb, __API_CALLBACK_BD744FE3CB, __api_set_bd744fe3cb;
    base orch_api_base::routes::access::page;

    async fn permission_page<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: &'__api orch_authz::pages::PageView, arg3: Option<orch_types::Id>, arg4: orch_types::Time) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::access::PermissionPage>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_3af1ff1c83, __api_install_3af1ff1c83, __api_callback_3af1ff1c83;
    slot __ApiCallback3af1ff1c83, __API_CALLBACK_3AF1FF1C83, __api_set_3af1ff1c83;
    base orch_api_base::routes::access::page;

    async fn entries_named<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Option<orch_types::Id>, arg3: &'__api [orch_access_contract::pages::Entry]) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::access::Entry>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_88985d68e3, __api_install_88985d68e3, __api_callback_88985d68e3;
    slot __ApiCallback88985d68e3, __API_CALLBACK_88985D68E3, __api_set_88985d68e3;
    base orch_api_base::routes::access::page;

    async fn entries_named_at_bounded<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: Option<orch_types::Id>, arg3: Option<orch_types::Id>, arg4: &'__api [orch_access_contract::pages::Entry], arg5: &'__api orch_api_base::routes::access::page::NameBounds) -> orch_api_base::middleware::requests::ApiResult<std::collections::HashMap<orch_types::Id, orch_api_contract::access::Entry>>;
}
