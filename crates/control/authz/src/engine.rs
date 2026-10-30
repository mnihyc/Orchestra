//! The rule behind every decision: where a page stands, what is stored for it, and what a person holds on it.
//!
//! A thing is described by a [`Site`]: the page it has, the defaults pages
//! it follows (the place it lies in, and the places above, up to the
//! system), where a role without a place is held for it, whose admins
//! administer it, and which orgs it lies in. A [`Frame`] is what was read
//! to decide about some sites for one [`Principal`]: the nodes on their
//! ways (the [`Graph`]), the rows and entries that are stored for their
//! pages, the owners and the roles that reach the person. Reading it takes
//! the same few statements for one site and for a hundred; deciding is then
//! the contract's rules over what was read, with no further statement.
//!
//! Nodes are the first thing described this way, in [`nodes`](crate::nodes).
//! A resource or a document is described by building its `Site` and its
//! pages' references, and is then decided by the same frame.

use std::collections::{HashMap, HashSet};

use orch_access_contract::decisions::{self, Held, Standing, Through, Via};
use orch_access_contract::pages::{
    Chain, Entry, EntrySource, Holder, Named, PageKind, Row, StoredKind,
};
use orch_access_contract::permissions::{
    HolderKind, NodePermission, ObjectKind, Permission, Placement, Profile, WhenArchived,
};
use orch_access_contract::roles::{self, AdminReach, NodeKind, PlaceRef, Role};
use orch_db::Conn;
use orch_db::queries::access::{self, EntryRow, PageKey, SYSTEM_ID, StoredRow};
use orch_db::queries::hierarchy::{self, NodeRow};
use orch_types::{Id, Time};

use crate::{Error, Policy, Principal};

/// A page outside the person's own trees, with the scope needed to bound its entries.
struct OutsidePage {
    page: PageRef,
    way: Vec<Id>,
    role_place: Option<Id>,
    root: Id,
    administers: bool,
}

/// A node of the hierarchy, as a decision needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeFacts {
    /// Its identifier.
    pub id: Id,
    /// What kind of node it is.
    pub kind: NodeKind,
    /// The node it lies in; none for a root org.
    pub parent: Option<Id>,
    /// Its root org; itself for a root org.
    pub root: Id,
    /// The nodes from its root org down to itself, both included.
    pub path: Vec<Id>,
    /// Its name.
    pub name: String,
    /// Its first owner.
    pub first_owner: Id,
    /// Whether it is archived itself.
    pub archived: bool,
    /// Whether it is deleted itself.
    pub deleted: bool,
    /// For a thread: the kind of environment it runs in; managed while it
    /// has none.
    pub placement: Placement,
}

impl NodeFacts {
    fn from_row(row: NodeRow) -> Result<Self, Error> {
        Ok(Self {
            id: row.id,
            kind: row.kind.parse()?,
            parent: row.parent_id,
            root: row.org_id,
            path: row.path,
            name: row.name,
            first_owner: row.owner_id,
            archived: row.archived_at.is_some(),
            deleted: row.deleted_at.is_some(),
            placement: match row.placement.as_deref() {
                Some(word) => word.parse()?,
                None => Placement::default(),
            },
        })
    }

    /// Whether it is a place, where roles are held: an org, a project or a
    /// topic.
    #[must_use]
    pub fn is_place(&self) -> bool {
        self.kind.place().is_some()
    }
}

/// The nodes a decision reads: the nodes asked about and every node on the
/// way from their root orgs down to them.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    nodes: HashMap<Id, NodeFacts>,
    thread_environments: HashMap<Id, Option<Id>>,
    environment_columns: HashMap<Id, Placement>,
    environment_sources: HashMap<Id, Option<Site>>,
    document_parents: HashMap<Id, Option<Id>>,
    document_containers: HashMap<Id, orch_db::queries::documents::containers::ContainerRow>,
}

impl Graph {
    /// Loads real document parents separately from the node, role and token hierarchy.
    pub async fn extend_documents(&mut self, conn: &mut Conn, ids: &[Id]) -> Result<(), Error> {
        use orch_db::queries::documents::containers;
        self.document_parents
            .extend(containers::document_parents(conn, ids).await?);
        let rows = containers::ancestors_many(conn, ids).await?;
        let nodes: Vec<_> = rows.iter().map(|row| row.node_id).collect();
        for row in rows {
            self.document_parents
                .insert(row.id, row.parent_container_id);
            self.document_containers.insert(row.id, row);
        }
        self.extend(conn, &nodes).await
    }

    /// Whether this identity is a real document container, rather than text or a node.
    #[must_use]
    pub fn is_document_container(&self, id: Id) -> bool {
        self.document_containers.contains_key(&id)
    }

    /// A real folder's document site, without a project, role or token-path substitution.
    #[must_use]
    pub fn document_container_site(&self, id: Id) -> Option<Site> {
        self.document_containers.get(&id).and_then(|row| {
            crate::documents::site_of(self, &crate::documents::Document::of_container(row))
        })
    }

    /// Prospective directory membership for the ordinary move-to-node audience preview.
    pub(crate) fn document_at_root(&mut self, id: Id) {
        self.document_parents.insert(id, None);
    }

    /// Nearest containing document folders first; malformed cycles never grant inherited access.
    #[must_use]
    pub fn document_ancestors(&self, id: Id) -> Vec<Id> {
        let mut out = Vec::new();
        let mut seen = HashSet::from([id]);
        let mut parent = self.document_parents.get(&id).copied().flatten();
        while let Some(id) = parent {
            if !seen.insert(id) {
                return Vec::new();
            }
            out.push(id);
            parent = self.document_parents.get(&id).copied().flatten();
        }
        out
    }

    /// A deleted containing folder hides a live document until its own parent-first restoration.
    #[must_use]
    pub fn document_parent_deleted(&self, id: Id) -> bool {
        self.document_ancestors(id).iter().any(|parent| {
            self.document_containers
                .get(parent)
                .is_none_or(|row| row.deleted_at.is_some())
        })
    }

    /// Reads the nodes `ids` with their ancestors.
    pub async fn load(conn: &mut Conn, ids: &[Id]) -> Result<Self, Error> {
        let mut graph = Self::default();
        graph.extend(conn, ids).await?;
        Ok(graph)
    }

    /// Adds the nodes `ids` with their ancestors, those that are not here
    /// yet.
    pub async fn extend(&mut self, conn: &mut Conn, ids: &[Id]) -> Result<(), Error> {
        let missing: Vec<Id> = ids
            .iter()
            .copied()
            .filter(|id| !self.nodes.contains_key(id))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        for row in hierarchy::with_ancestors(conn, &missing).await? {
            if row.kind == "thread" {
                self.thread_environments.insert(row.id, row.environment_id);
            }
            let node = NodeFacts::from_row(row)?;
            self.nodes.insert(node.id, node);
        }
        Ok(())
    }

    /// Adds nodes that were read elsewhere, as they were.
    pub fn add_rows(&mut self, rows: Vec<NodeRow>) -> Result<(), Error> {
        for row in rows {
            let environment = row.environment_id;
            let node = NodeFacts::from_row(row)?;
            if node.kind == NodeKind::Thread {
                self.thread_environments.insert(node.id, environment);
            }
            self.nodes.insert(node.id, node);
        }
        Ok(())
    }

    /// A graph of nodes given as they are, for what builds sites without
    /// reading.
    #[must_use]
    pub fn of(nodes: Vec<NodeFacts>) -> Self {
        Self {
            nodes: nodes.into_iter().map(|node| (node.id, node)).collect(),
            ..Self::default()
        }
    }

    /// Reads the hosting machine's delegation once for every environment needed by this frame.
    /// The ordinary comparison clones this prepared graph, so it uses the same reports without another read.
    pub(crate) async fn load_profiles(
        &mut self,
        conn: &mut Conn,
        sites: &[Site],
    ) -> Result<(), Error> {
        let threads: Vec<Id> = self
            .nodes()
            .filter(|node| {
                node.kind == NodeKind::Thread && !self.thread_environments.contains_key(&node.id)
            })
            .map(|node| node.id)
            .collect();
        if !threads.is_empty() {
            let bindings = hierarchy::thread_environments(conn, &threads).await?;
            self.thread_environments
                .extend(threads.into_iter().map(|id| (id, None)));
            self.thread_environments.extend(bindings);
        }
        let mut environments: Vec<Id> = self
            .thread_environments
            .values()
            .flatten()
            .copied()
            .chain(
                sites
                    .iter()
                    .filter(|site| site.kind == ObjectKind::Environment)
                    .map(|site| site.id),
            )
            .filter(|id| !self.environment_columns.contains_key(id))
            .collect();
        environments.sort_unstable();
        environments.dedup();
        if !environments.is_empty() {
            self.environment_columns.extend(
                orch_db::queries::environment_facts::access_profiles(conn, &environments).await?,
            );
        }
        Ok(())
    }

    async fn load_ownership_sources(
        &mut self,
        conn: &mut Conn,
        sites: &[Site],
    ) -> Result<(), Error> {
        let ids: Vec<Id> = sites
            .iter()
            .filter(|site| {
                site.kind == ObjectKind::Environment
                    && !self.environment_sources.contains_key(&site.id)
            })
            .map(|site| site.id)
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let sources = orch_db::queries::environment_facts::ownership_sources(conn, &ids).await?;
        let source_ids: Vec<Id> = sources.iter().map(|(_, _, id)| *id).collect();
        let resources = crate::resources::read_into(conn, self, &source_ids).await?;
        self.environment_sources
            .extend(ids.into_iter().map(|id| (id, None)));
        for (environment, kind, id) in sources {
            if let Some(resource) = resources.iter().find(|resource| {
                resource.id == id && resource.kind.as_str() == kind && !resource.deleted
            }) {
                let site = crate::resources::site_of(self, resource);
                self.environment_sources.insert(environment, site);
            }
        }
        Ok(())
    }

    /// The environment the thread `thread` runs in, as the graph read it with the thread; none for a thread
    /// that runs in none, or that the graph has not read.
    #[must_use]
    pub fn environment_of_thread(&self, thread: Id) -> Option<Id> {
        self.thread_environments.get(&thread).copied().flatten()
    }

    /// The prepared permission profile; a synthetic thread's placement names its prototype column.
    #[must_use]
    pub fn profile(&self, site: &Site, permission: Permission) -> Profile {
        let environment = match site.kind {
            ObjectKind::Thread => self.thread_environments.get(&site.id).copied().flatten(),
            ObjectKind::Environment => Some(site.id),
            _ => None,
        };
        let column = environment
            .and_then(|environment| self.environment_columns.get(&environment).copied())
            .unwrap_or(site.placement);
        permission.profile(column)
    }

    /// Environment columns for the indexed inventory's inherited-thread projection.
    pub(crate) fn environment_profiles(&self) -> impl Iterator<Item = (Id, Placement)> + '_ {
        self.environment_columns
            .iter()
            .map(|(id, column)| (*id, *column))
    }

    /// The node `id`, if it was read.
    #[must_use]
    pub fn get(&self, id: Id) -> Option<&NodeFacts> {
        self.nodes.get(&id)
    }

    /// Every node that was read.
    pub fn nodes(&self) -> impl Iterator<Item = &NodeFacts> {
        self.nodes.values()
    }

    /// The nodes on the way from the root org down to `node`, itself
    /// included, as far as they were read; nothing if one is missing.
    fn way(&self, node: &NodeFacts) -> Option<Vec<&NodeFacts>> {
        node.path.iter().map(|id| self.nodes.get(id)).collect()
    }

    /// The places on the way down to `node`, root first, itself included
    /// when it is one: where roles reach it from.
    #[must_use]
    pub fn places_on(&self, node: &NodeFacts) -> Vec<&NodeFacts> {
        self.way(node)
            .map(|way| way.into_iter().filter(|node| node.is_place()).collect())
            .unwrap_or_default()
    }

    /// The orgs on the way down to `node`, root first, itself included when
    /// it is one: what a suspension in an org closes it by.
    #[must_use]
    pub fn orgs_on(&self, node: &NodeFacts) -> Vec<Id> {
        self.way(node)
            .map(|way| {
                way.into_iter()
                    .filter(|node| node.kind == NodeKind::Org)
                    .map(|node| node.id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The site of the node `id`: nothing where it is not there, or it or
    /// a node above it was deleted.
    #[must_use]
    pub fn site_of(&self, id: Id) -> Option<Site> {
        let node = self.nodes.get(&id)?;
        let way = self.way(node)?;
        if way.iter().any(|on_the_way| on_the_way.deleted)
            && !orch_db::queries::trace::retained_read(id)
        {
            return None;
        }
        let places: Vec<&NodeFacts> = way.iter().copied().filter(|n| n.is_place()).collect();
        let above: Vec<Id> = places
            .iter()
            .filter(|place| place.id != node.id)
            .map(|place| place.id)
            .rev()
            .collect();
        let mut chain: Vec<PlaceRef> = above.iter().copied().map(PlaceRef::Node).collect();
        chain.push(PlaceRef::System);
        Some(Site {
            kind: node.kind.into(),
            id: node.id,
            name: node.name.clone(),
            first_owner: Some(node.first_owner),
            placement: node.placement,
            chain,
            beside: None,
            role_place: if node.is_place() {
                Some(node.id)
            } else {
                above.first().copied()
            },
            admin_places: above,
            orgs: way
                .iter()
                .filter(|n| n.kind == NodeKind::Org)
                .map(|n| n.id)
                .collect(),
            root: Some(node.root),
            path: Some(node.path.clone()),
            archived_node: way.iter().rev().find(|n| n.archived).map(|n| n.id),
            of_system: false,
        })
    }
}

/// A thing that has a page, with everything a decision needs to know of
/// where that page stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    /// What kind of thing it is.
    pub kind: ObjectKind,
    /// Which one.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// Its first owner, where it has been read.
    pub first_owner: Option<Id>,
    /// The actual environment placement. A frame selects its shipped permission
    /// column independently, from the hosting machine's reported section delegation.
    pub placement: Placement,
    /// The defaults pages its rows follow, nearest first: the place it
    /// lies in, the places above it, and the system last.
    pub chain: Vec<PlaceRef>,
    /// For a document: the node whose row of the same name it follows
    /// before the defaults.
    pub beside: Option<Id>,
    /// Where a role without a place is held for it: the node itself, for a
    /// resource its home, for a document its node. A role is held there and
    /// in the places above.
    pub role_place: Option<Id>,
    /// The places whose admins administer it, nearest first, the system
    /// left out: the admins of the first, and of those above it.
    pub admin_places: Vec<Id>,
    /// The orgs it lies in, root org first: what a suspension in an org
    /// closes it by.
    pub orgs: Vec<Id>,
    /// The root org of its tree: the org whose members are no outsiders.
    pub root: Option<Id>,
    /// The nodes from its root org down to it, for a token that is kept to
    /// a subtree; none for a resource that is reached through no node.
    pub path: Option<Vec<Id>>,
    /// The nearest archived node on the way up, itself included: under it
    /// everything is read-only.
    pub archived_node: Option<Id>,
    /// Whether it is a resource of the system, which has no home.
    pub of_system: bool,
}

impl Site {
    /// Its own page.
    #[must_use]
    pub fn page(&self) -> PageRef {
        PageRef::live(StoredKind::from(self.kind), self.id)
    }

    /// Whether it is archived, itself or through a node above it.
    #[must_use]
    pub fn is_archived(&self) -> bool {
        self.archived_node.is_some()
    }
}

/// One page whose rows are stored: of a thing, or the defaults of a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageRef {
    /// What the page is of: the system, or a node, a resource, a document.
    pub kind: StoredKind,
    /// Which one; the nil identifier for the system.
    pub id: Id,
    /// Its own page, or the defaults it gives.
    pub page: PageKind,
}

impl PageRef {
    /// The own page of the thing `id` of kind `kind`.
    #[must_use]
    pub const fn live(kind: StoredKind, id: Id) -> Self {
        Self {
            kind,
            id,
            page: PageKind::Live,
        }
    }

    /// The defaults page of a place.
    #[must_use]
    pub fn defaults_of(place: PlaceRef) -> Self {
        match place {
            PlaceRef::System => Self {
                kind: StoredKind::System,
                id: SYSTEM_ID,
                page: PageKind::Defaults,
            },
            PlaceRef::Node(id) => Self {
                kind: StoredKind::Node,
                id,
                page: PageKind::Defaults,
            },
        }
    }

    /// The page as the database keys it.
    #[must_use]
    pub fn key(self) -> PageKey {
        PageKey::new(self.kind.as_str(), self.id, self.page.as_str())
    }
}

/// Reads an entry as it is stored into the contract's entry.
pub fn entry_of(row: &EntryRow) -> Result<Entry, Error> {
    let holder = Holder::from_stored(
        row.holder_kind.parse::<HolderKind>()?,
        row.holder_id,
        row.holder_role
            .as_deref()
            .map(str::parse::<Role>)
            .transpose()?,
    )
    .ok_or_else(|| {
        Error::Stored(format!(
            "entry {} names a holder that does not go together",
            row.id
        ))
    })?;
    Ok(Entry {
        id: row.id,
        holder,
        options: row.options.clone(),
        expires_at: row.expires_at.map(Time::new),
        note: row.note.clone(),
        source: row.source.parse::<EntrySource>()?,
        created_by: row.created_by,
        created_at: Time::new(row.created_at),
        removed_at: row.removed_at.map(Time::new),
        revision: row.revision,
    })
}

/// The rows and entries that are stored for some pages.
#[derive(Debug, Default)]
pub struct Store {
    rows: HashMap<(PageRef, Profile, Permission), Row>,
    /// The keys of `rows` by page: a page's rows are found without reading every page's, so a list of thousands of
    /// documents is read in time proportional to it, not to its square.
    pages: HashMap<PageRef, Vec<(Profile, Permission)>>,
}

impl Store {
    /// Reads the rows of `permissions` on the pages `pages`: two statements.
    /// A row that is not stored is not here, and follows. A row of a
    /// permission the catalogue does not have cannot be asked about and is
    /// left out.
    pub async fn load(
        conn: &mut Conn,
        pages: &[PageRef],
        permissions: &[Permission],
    ) -> Result<Self, Error> {
        let keys: Vec<PageKey> = pages.iter().map(|page| page.key()).collect();
        let words: Vec<String> = permissions
            .iter()
            .map(|permission| permission.as_str().to_owned())
            .collect();
        let mut store = Self::default();
        for stored in access::stored_rows(conn, &keys, &words).await? {
            store.put_row(&stored)?;
        }
        for entry in access::entries(conn, &keys, &words).await? {
            store.put_entry(&entry)?;
        }
        Ok(store)
    }

    fn page_of(kind: &str, id: Id, page: &str) -> Result<PageRef, Error> {
        Ok(PageRef {
            kind: kind.parse()?,
            id,
            page: page.parse()?,
        })
    }

    fn put_row(&mut self, stored: &StoredRow) -> Result<(), Error> {
        let Ok(permission) = stored.permission.parse::<Permission>() else {
            return Ok(());
        };
        let page = Self::page_of(&stored.object_kind, stored.object_id, &stored.page)?;
        let profile: Profile = stored.profile.parse()?;
        let row = self.row_mut(page, profile, permission);
        row.inherit = stored.inherit;
        row.locked = stored.locked;
        Ok(())
    }

    fn put_entry(&mut self, stored: &EntryRow) -> Result<(), Error> {
        let Ok(permission) = stored.permission.parse::<Permission>() else {
            return Ok(());
        };
        let page = Self::page_of(&stored.object_kind, stored.object_id, &stored.page)?;
        let profile: Profile = stored.profile.parse()?;
        let entry = entry_of(stored)?;
        self.row_mut(page, profile, permission).entries.push(entry);
        Ok(())
    }

    /// The stored row `permission` / `profile` of `page`, made following when nothing was stored for it yet.
    fn row_mut(&mut self, page: PageRef, profile: Profile, permission: Permission) -> &mut Row {
        match self.rows.entry((page, profile, permission)) {
            std::collections::hash_map::Entry::Occupied(row) => row.into_mut(),
            std::collections::hash_map::Entry::Vacant(vacant) => {
                self.pages
                    .entry(page)
                    .or_default()
                    .push((profile, permission));
                vacant.insert(Row::following(permission, profile))
            }
        }
    }

    /// The row `permission` / `profile` of `page`, where anything is stored
    /// for it.
    #[must_use]
    pub fn row(&self, page: PageRef, profile: Profile, permission: Permission) -> Option<&Row> {
        self.rows.get(&(page, profile, permission))
    }

    /// Every stored row of `page`.
    pub fn rows_of(&self, page: PageRef) -> impl Iterator<Item = &Row> {
        self.pages
            .get(&page)
            .into_iter()
            .flatten()
            .filter_map(move |(profile, permission)| self.rows.get(&(page, *profile, *permission)))
    }

    /// The places that role holders in the stored entries name explicitly.
    fn role_places(&self) -> HashSet<Id> {
        self.rows
            .values()
            .flat_map(|row| row.entries.iter())
            .filter_map(|entry| match entry.holder {
                Holder::Role { at, .. } => at,
                _ => None,
            })
            .collect()
    }
}

/// What was read to decide about some sites for one person.
#[derive(Debug)]
pub struct Frame<'p> {
    principal: &'p Principal,
    policy: Policy,
    graph: Graph,
    store: Store,
    owners: HashMap<Id, Vec<Entry>>,
    roles: HashMap<Id, Option<roles::Held>>,
    /// The nearest real place at which this person is an Admin or owner, before system reach.
    admin_sources: HashMap<Id, PlaceRef>,
    /// The same sites under this person's ordinary pages, never the session's native reach.
    ordinary_held: Option<HashMap<SiteKey, HashSet<Permission>>>,
    /// Explicit ordinary assignments at this page or below, and current follows.
    /// These focus facts never confer a permission or replace ordinary View.
    participation: HashSet<PageRef>,
}

// An uncustomized thread can be projected without a real identifier. Its topic,
// placement and archive state still select distinct inherited permission rows.
type SiteKey = (Id, ObjectKind, Option<Id>, Placement, Option<Id>);

fn site_key(site: &Site) -> SiteKey {
    (
        site.id,
        site.kind,
        site.role_place,
        site.placement,
        site.archived_node,
    )
}

impl<'p> Frame<'p> {
    /// Reads what a decision about `sites` needs, for the permissions
    /// `permissions` and the levels above them: the rows of their pages and
    /// of the defaults they follow, the Owners rows of every place on their
    /// ways, and the roles that reach the person there. `graph` holds the
    /// nodes of the sites and their ancestors already. Pages, role places and
    /// physical environment control are loaded in batches, never once per site;
    /// the ordinary comparison reuses the same prepared environment reports.
    pub async fn load(
        conn: &mut Conn,
        principal: &'p Principal,
        policy: Policy,
        graph: Graph,
        sites: &[Site],
        permissions: &[Permission],
    ) -> Result<Frame<'p>, Error> {
        Self::load_with(conn, principal, policy, graph, sites, &[], permissions).await
    }

    /// [`Frame::load`], and the pages `extra` besides: what is read of a
    /// page that belongs to no site, as the defaults of the system.
    pub async fn load_with(
        conn: &mut Conn,
        principal: &'p Principal,
        policy: Policy,
        mut graph: Graph,
        sites: &[Site],
        extra: &[PageRef],
        permissions: &[Permission],
    ) -> Result<Frame<'p>, Error> {
        let mut expanded = sites.to_vec();
        let mut seen: HashSet<_> = sites.iter().map(|site| (site.kind, site.id)).collect();
        for site in sites {
            for id in graph.document_ancestors(site.id) {
                if let Some(parent) = graph.document_container_site(id)
                    && seen.insert((parent.kind, parent.id))
                {
                    expanded.push(parent);
                }
            }
        }
        graph.load_ownership_sources(conn, &expanded).await?;
        for source in graph.environment_sources.values().flatten() {
            if seen.insert((source.kind, source.id)) {
                expanded.push(source.clone());
            }
        }
        let sites = expanded.as_slice();
        let mut pages: HashSet<PageRef> = extra.iter().copied().collect();
        for site in sites {
            pages.insert(site.page());
            pages.extend(
                graph
                    .document_ancestors(site.id)
                    .into_iter()
                    .map(|id| PageRef::live(StoredKind::DocumentContainer, id)),
            );
            if let Some(node) = site.beside {
                pages.insert(PageRef::live(StoredKind::Node, node));
            }
            pages.extend(site.chain.iter().copied().map(PageRef::defaults_of));
        }
        // An outside creator's Manage entry lasts while they can enter its place. Read those
        // places in the same batch, so checking that condition never adds one query per thing.
        pages.extend(
            graph
                .nodes()
                .map(|node| PageRef::live(StoredKind::Node, node.id)),
        );
        let mut wanted: Vec<Permission> = Vec::new();
        let place_permissions = crate::nodes::every_permission([
            ObjectKind::Org,
            ObjectKind::Project,
            ObjectKind::Topic,
            ObjectKind::Thread,
        ]);
        let views: Vec<Permission> = sites
            .iter()
            .map(|site| view_permission(site.kind))
            .collect();
        let mut expanded_permissions = HashSet::new();
        for permission in permissions
            .iter()
            .chain(place_permissions.iter())
            .chain(views.iter())
        {
            // Hundreds of inherited sites can request the same row; its covering levels are identical.
            if !expanded_permissions.insert(*permission) {
                continue;
            }
            for level in std::iter::once(*permission).chain(permission.held_through()) {
                if !wanted.contains(&level) {
                    wanted.push(level);
                }
            }
        }
        let pages: Vec<PageRef> = pages.into_iter().collect();
        let store = Store::load(conn, &pages, &wanted).await?;

        let elsewhere: Vec<Id> = store.role_places().into_iter().collect();
        graph.extend(conn, &elsewhere).await?;

        let mut ownable: HashSet<PageRef> = graph
            .nodes()
            .map(|node| PageRef::live(StoredKind::Node, node.id))
            .collect();
        ownable.extend(sites.iter().map(Site::page));
        ownable.extend(
            sites
                .iter()
                .flat_map(|site| graph.document_ancestors(site.id))
                .map(|id| PageRef::live(StoredKind::DocumentContainer, id)),
        );
        let ownable: Vec<PageKey> = ownable.into_iter().map(PageRef::key).collect();
        let mut owners: HashMap<Id, Vec<Entry>> = HashMap::new();
        for stored in access::owner_entries(conn, &ownable).await? {
            owners
                .entry(stored.object_id)
                .or_default()
                .push(entry_of(&stored)?);
        }

        let place_ids: Vec<Id> = graph
            .nodes()
            .filter(|node| node.is_place())
            .map(|node| node.id)
            .collect();
        let mut grants: HashMap<Id, Vec<Role>> = HashMap::new();
        for stored in access::roles_reaching(
            conn,
            &place_ids,
            principal.user(),
            principal.groups(),
            principal.member_of(),
            principal.now().get(),
        )
        .await?
        {
            grants
                .entry(stored.node_id)
                .or_default()
                .push(stored.role.parse()?);
        }

        graph.load_profiles(conn, sites).await?;
        let mut frame = Frame {
            principal,
            policy,
            graph,
            store,
            owners,
            roles: HashMap::new(),
            admin_sources: HashMap::new(),
            ordinary_held: None,
            participation: HashSet::new(),
        };
        frame.roles = frame
            .graph
            .nodes()
            .filter(|node| node.is_place())
            .map(|node| (node.id, frame.role_on_the_way_to(node, &grants)))
            .collect();
        frame.admin_sources = frame
            .graph
            .nodes()
            .filter(|node| node.is_place())
            .filter_map(|node| {
                if !frame
                    .role_at(node.id)
                    .is_some_and(|held| held.role == Role::Admin)
                {
                    return None;
                }
                frame
                    .graph
                    .places_on(node)
                    .into_iter()
                    .rev()
                    .find(|place| {
                        frame.counts_as_owner(frame.owners.get(&place.id), Some(place.root))
                            || grants
                                .get(&place.id)
                                .is_some_and(|roles| roles.contains(&Role::Admin))
                    })
                    .map(|place| (node.id, PlaceRef::Node(place.id)))
            })
            .collect();
        frame.end_outside_creations(sites);
        frame.bound_outside_entries(conn, sites).await?;
        if principal.caller().as_admin() && sites.iter().any(|site| frame.administers(site)) {
            // A system admin may also occur in a holder graph. Re-evaluate with the mode
            // off rather than subtracting Through::Admin from an already elevated answer.
            // This is one batched frame, never one query for every listed object.
            let ordinary = principal.ordinary(conn).await?;
            let baseline = Box::pin(Frame::load_with(
                conn,
                &ordinary,
                policy,
                frame.graph.clone(),
                sites,
                extra,
                permissions,
            ))
            .await?;
            frame.ordinary_held = Some(
                sites
                    .iter()
                    .map(|site| {
                        (
                            site_key(site),
                            baseline
                                .effective(site)
                                .into_iter()
                                .map(|held| held.permission)
                                .collect(),
                        )
                    })
                    .collect(),
            );
            frame.admin_sources = baseline.admin_sources.clone();
            frame.participation = baseline.participation.clone();
        } else if !principal.suspension().altogether() {
            let ordinary = principal.ordinary(conn).await?;
            let nodes: Vec<_> = frame.graph.nodes().map(|node| node.id).collect();
            let scope_roots: Vec<_> = frame
                .graph
                .nodes()
                .map(|node| node.root)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            let mut closed = ordinary.suspension().in_orgs().to_vec();
            closed.extend(ordinary.suspension().held());
            let roots = ordinary.memberships().roots();
            let marks = access::participation::marks(
                conn,
                ordinary.user(),
                ordinary.groups(),
                ordinary.member_of(),
                ordinary.now(),
                &nodes,
                &ownable,
                &roots,
                &closed,
                ordinary
                    .caller()
                    .grant()
                    .and_then(crate::TokenGrant::subtree),
                &scope_roots,
            )
            .await?;
            for mark in marks {
                frame
                    .participation
                    .insert(PageRef::live(mark.kind.parse()?, mark.id));
            }
        }
        Ok(frame)
    }

    /// An entry lets a person from outside a tree in only while `sharing.external_max_days` admits it where
    /// it is ([`crate::external::LifeBound`]): on the pages of the sites and places whose tree they are not in,
    /// the entries that let them in from outside and run beyond the bound (or never end) are not counted for
    /// them, until somebody gives them a nearer end. Which entries those are is decided as when they are
    /// written ([`crate::external::any_outside`]): one naming them, everyone, or the members of a place of
    /// another tree; a group or a role at a place of another tree; never a role held where the thing is, nor
    /// an admin's naming of themself where they administer. Nothing is read for a person who is in every
    /// tree asked about, one statement otherwise, and the trees of the groups and places that let them in
    /// beyond the bound where there are such.
    async fn bound_outside_entries(
        &mut self,
        conn: &mut Conn,
        sites: &[Site],
    ) -> Result<(), Error> {
        let principal: &Principal = self.principal;
        // Each page of something whose tree they are not in, with the way to it, where a role without a
        // place is held for it, its root org, and whether they administer it.
        let mut outside: Vec<OutsidePage> = Vec::new();
        let others = self
            .graph
            .nodes()
            .filter_map(|node| self.graph.site_of(node.id));
        for site in sites.iter().cloned().chain(others) {
            let page = site.page();
            if let Some(root) = site.root
                && !principal.is_in_tree(root)
                && !outside.iter().any(|known| known.page == page)
            {
                let administers = self.administers(&site);
                outside.push(OutsidePage {
                    page,
                    way: crate::external::way_of(&site),
                    role_place: site.role_place,
                    root,
                    administers,
                });
            }
        }
        if outside.is_empty() {
            return Ok(());
        }
        let bounds = crate::external::LifeBounds::load(conn).await?;
        if bounds.is_empty() {
            return Ok(());
        }
        let now = principal.now();
        let person = principal.person();
        let themself = Holder::User {
            id: principal.user(),
        };
        let mut beyond: HashSet<Id> = HashSet::new();
        // Entries of a group, or of a role at a place, beyond the bound: from outside only where the group
        // or the place is of another tree than the page's (its root org is kept with each).
        let mut by_tree: Vec<(Id, Id, Holder)> = Vec::new();
        for OutsidePage {
            page,
            way,
            role_place,
            root,
            administers,
        } in &outside
        {
            let Some(bound) = bounds.at(way) else {
                continue;
            };
            for row in self.store.rows_of(*page) {
                for entry in &row.entries {
                    if bound.admits(entry.expires_at, now)
                        || (*administers && entry.holder == themself)
                        || !entry.holder.matches(&person, |at| {
                            at.or(*role_place)
                                .and_then(|place| self.role_at(place))
                                .map(|held| held.role)
                        })
                    {
                        continue;
                    }
                    match entry.holder {
                        // A role held where the thing is names the tree's own people.
                        Holder::Role { at: None, .. } => {}
                        Holder::Group { .. } | Holder::Role { at: Some(_), .. } => {
                            by_tree.push((entry.id, *root, entry.holder));
                        }
                        _ => {
                            beyond.insert(entry.id);
                        }
                    }
                }
            }
        }
        if !by_tree.is_empty() {
            let groups: Vec<Id> = by_tree
                .iter()
                .filter_map(|(_, _, holder)| match holder {
                    Holder::Group { id } => Some(*id),
                    _ => None,
                })
                .collect();
            let places: Vec<Id> = by_tree
                .iter()
                .filter_map(|(_, _, holder)| match holder {
                    Holder::Role {
                        at: Some(place), ..
                    } => Some(*place),
                    _ => None,
                })
                .collect();
            let group_roots: HashMap<Id, Id> = if groups.is_empty() {
                HashMap::new()
            } else {
                orch_db::queries::access::groups(conn, &groups)
                    .await?
                    .into_iter()
                    .map(|group| (group.id, group.root_id))
                    .collect()
            };
            let place_roots: HashMap<Id, Id> = if places.is_empty() {
                HashMap::new()
            } else {
                orch_db::queries::access::roots_of(conn, &places)
                    .await?
                    .into_iter()
                    .collect()
            };
            for (entry, root, holder) in by_tree {
                let of = match holder {
                    Holder::Group { id } => group_roots.get(&id),
                    Holder::Role {
                        at: Some(place), ..
                    } => place_roots.get(&place),
                    _ => None,
                };
                if of != Some(&root) {
                    beyond.insert(entry);
                }
            }
        }
        if beyond.is_empty() {
            return Ok(());
        }
        for ((page, _, _), row) in &mut self.store.rows {
            if outside.iter().any(|known| known.page == *page) {
                row.entries.retain(|entry| !beyond.contains(&entry.id));
            }
        }
        Ok(())
    }

    fn end_outside_creations(&mut self, sites: &[Site]) {
        // Process ancestors first: losing entry to an outside-created folder also ends entry to
        // things made through that folder. Paths have no cycles.
        let mut places: Vec<_> = self.graph.nodes().collect();
        places.sort_by_key(|node| node.path.len());
        let mut candidates: Vec<_> = places
            .into_iter()
            .filter_map(|node| self.graph.site_of(node.id).map(|site| (site, node.parent)))
            .collect();
        candidates.extend(
            sites
                .iter()
                .filter(|site| site.kind.node().is_none())
                .cloned()
                .map(|site| {
                    let place = self
                        .graph
                        .document_ancestors(site.id)
                        .first()
                        .copied()
                        .or(site.beside)
                        .or(site.role_place);
                    (site, place)
                }),
        );
        candidates.sort_by_key(|(site, _)| {
            (
                site.path.as_ref().map_or(0, Vec::len),
                self.graph.document_ancestors(site.id).len()
                    + usize::from(site.kind.node().is_none()),
            )
        });
        for (site, place) in candidates {
            if site.root.is_none_or(|root| self.principal.is_in_tree(root)) {
                continue;
            }
            let Some(place) = place.and_then(|id| {
                self.graph
                    .document_container_site(id)
                    .or_else(|| self.graph.site_of(id))
            }) else {
                continue;
            };
            // Credential scopes narrow actions, not the lifetime of the person's existing entry.
            let reachable =
                !decisions::effective(place.kind, &self.standing(&place), false, |permission| {
                    self.named(&place, permission)
                })
                .is_empty();
            if reachable {
                continue;
            }
            let maker = self.principal.user();
            for ((page, _, permission), row) in &mut self.store.rows {
                if *page == site.page()
                    && permission.level() == orch_access_contract::permissions::Level::Manage
                {
                    row.entries.retain(|entry| {
                        !(entry.source == EntrySource::Creation
                            && entry.created_by == Some(maker)
                            && entry.holder == (Holder::User { id: maker }))
                    });
                }
            }
        }
    }

    /// Whether the person is one of the owners named in `entries`, and is a
    /// member of the tree of `root`, which is the while for which owning
    /// counts.
    fn counts_as_owner(&self, entries: Option<&Vec<Entry>>, root: Option<Id>) -> bool {
        let person = self.principal.person();
        let now = self.principal.now();
        root.is_some_and(|root| self.principal.is_in_tree(root))
            && entries.is_some_and(|entries| {
                entries
                    .iter()
                    .filter(|entry| entry.counts(now))
                    .any(|entry| entry.holder.matches(&person, |_| None))
            })
    }

    /// The highest role the person has at `node`, a place: as a system admin
    /// in a session, as an owner of a place on the way, or given to them
    /// there. Nothing for a person who is suspended in an org on the way.
    fn role_on_the_way_to(
        &self,
        node: &NodeFacts,
        grants: &HashMap<Id, Vec<Role>>,
    ) -> Option<roles::Held> {
        if node.deleted
            || node
                .path
                .iter()
                .any(|id| self.graph.get(*id).is_some_and(|ancestor| ancestor.deleted))
        {
            return None;
        }
        if self
            .principal
            .suspension()
            .applies(&self.graph.orgs_on(node))
        {
            return None;
        }
        let mut ways = Vec::new();
        if self.principal.is_system_admin() {
            ways.push(roles::Held::system_admin());
        }
        for place in self.graph.places_on(node) {
            if self.counts_as_owner(self.owners.get(&place.id), Some(place.root)) {
                ways.push(roles::Held::owner_of(place.id));
            }
            for role in grants.get(&place.id).into_iter().flatten() {
                ways.push(roles::Held::given(*role, place.id));
            }
        }
        roles::role_at(ways)
    }

    /// Who is asked about.
    #[must_use]
    pub fn principal(&self) -> &Principal {
        self.principal
    }

    /// The moment the decisions are made at.
    #[must_use]
    pub fn now(&self) -> Time {
        self.principal.now()
    }

    /// The nodes that were read.
    #[must_use]
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// The rows and entries that were read.
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The policy decisions were made under.
    #[must_use]
    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// The entries of the Owners row of `id` that are stored.
    #[must_use]
    pub fn owner_entries(&self, id: Id) -> &[Entry] {
        self.owners.get(&id).map_or(&[], Vec::as_slice)
    }

    /// The role the person has at the place `place`, and where it comes
    /// from; nothing where they have none, are suspended there, or the
    /// place was not read.
    #[must_use]
    pub fn role_at(&self, place: Id) -> Option<roles::Held> {
        self.roles.get(&place).copied().flatten()
    }

    /// The role the person has where a role of the site is held.
    #[must_use]
    pub fn role_for(&self, site: &Site) -> Option<roles::Held> {
        site.role_place.and_then(|place| self.role_at(place))
    }

    /// Whether the person is one of the owners of the site, and counts as
    /// one: a member of its root org.
    #[must_use]
    pub fn owns(&self, site: &Site) -> bool {
        self.counts_as_owner(self.owners.get(&site.id), site.root)
    }

    /// The canonical resource whose owners inherently manage this environment.
    /// Only connected environments have this source; this confers no target ownership.
    #[must_use]
    pub fn ownership_source(&self, site: &Site) -> Option<&Site> {
        (site.kind == ObjectKind::Environment)
            .then(|| {
                self.graph
                    .environment_sources
                    .get(&site.id)
                    .and_then(Option::as_ref)
            })
            .flatten()
    }

    /// Whether the person administers the site: they are a system admin in a
    /// session, or hold Admin at the place it lies in or at a place above it,
    /// as an owner of such a place included.
    #[must_use]
    pub fn administers(&self, site: &Site) -> bool {
        self.principal.caller().as_admin()
            && (self.principal.is_system_admin()
                || site
                    .role_place
                    .and_then(|place| self.role_at(place))
                    .is_some_and(|held| held.role == Role::Admin)
                || site
                    .admin_places
                    .first()
                    .and_then(|place| self.role_at(*place))
                    .is_some_and(|held| held.role == Role::Admin))
    }

    /// Whether an ordinary role or personal/group assignment names this person
    /// here or below, or they follow this conversation. Broad inherited View
    /// and this session's native administrator reach do not create participation.
    #[must_use]
    pub fn participates(&self, site: &Site) -> bool {
        !self.principal.suspension().applies(&site.orgs)
            && self.principal.caller().reaches(site.path.as_deref())
            && self.participation.contains(&site.page())
    }

    /// The independent chat area follows the place's explicit participation,
    /// or a current follow of this node's chat.
    #[must_use]
    pub fn participates_chat(&self, site: &Site) -> bool {
        self.participates(site)
            || (!self.principal.suspension().applies(&site.orgs)
                && self.principal.caller().reaches(site.path.as_deref())
                && self.follows_chat(site.id))
    }

    /// A direct/group chat follow, or a node-chat follow keyed by its node.
    #[must_use]
    pub fn follows_chat(&self, id: Id) -> bool {
        self.participation
            .contains(&PageRef::live(StoredKind::Chat, id))
    }

    /// Followed ordinary threads may be focused without expanding them into
    /// the inventory's permission graph.
    #[must_use]
    pub fn participation_ids(&self, kind: StoredKind) -> Vec<Id> {
        let mut ids: Vec<_> = self
            .participation
            .iter()
            .filter(|page| page.kind == kind)
            .map(|page| page.id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Whether the ordinary pages give this permission, before native reach is applied.
    #[must_use]
    pub fn ordinary_holds(&self, site: &Site, view: Permission) -> bool {
        self.ordinary_held.as_ref().map_or_else(
            || !self.ways(site, view).is_empty(),
            |held| {
                held.get(&site_key(site))
                    .is_some_and(|permissions| permissions.contains(&view))
            },
        )
    }

    /// The administrative place which alone made this object readable, private or not.
    /// A directly shared page and an owner's own page have no admin marker.
    #[must_use]
    pub fn admin_access(&self, site: &Site) -> Option<AdminReach> {
        self.admin_access_for(site, view_permission(site.kind))
    }

    /// As [`Self::admin_access`], for an independently private area such as a chat.
    #[must_use]
    pub fn admin_access_for(&self, site: &Site, permission: Permission) -> Option<AdminReach> {
        if !self.administers(site)
            || self.ordinary_holds(site, permission)
            || self.ways(site, permission).is_empty()
        {
            return None;
        }
        self.admin_place(site).map(|place| AdminReach {
            place,
            private: std::iter::once(permission)
                .chain(permission.held_through())
                .any(|row| {
                    self.chain(site, row)
                        .cut_below(site.root.map_or(PlaceRef::System, PlaceRef::Node))
                }),
        })
    }

    /// The place actually administered by this enabled session, including the place itself.
    #[must_use]
    pub fn admin_place(&self, site: &Site) -> Option<PlaceRef> {
        if !self.administers(site) || self.principal.suspension().applies(&site.orgs) {
            return None;
        }
        site.role_place
            .into_iter()
            .chain(site.admin_places.iter().copied())
            .find_map(|place| self.admin_sources.get(&place).copied())
            .or_else(|| self.principal.is_system_admin().then_some(PlaceRef::System))
    }

    /// Exactly the held permissions which this person's ordinary frame does not give.
    #[must_use]
    pub fn admin_only(&self, site: &Site) -> Vec<Permission> {
        self.effective(site)
            .into_iter()
            .map(|held| held.permission)
            .filter(|permission| !self.ordinary_holds(site, *permission))
            .collect()
    }

    /// What is known of the person on the site apart from what its rows
    /// name them for.
    #[must_use]
    pub fn standing(&self, site: &Site) -> Standing {
        Standing {
            suspended: self.principal.suspension().applies(&site.orgs),
            owner: self.owns(site),
            source_owner: self
                .ownership_source(site)
                .filter(|source| {
                    !self.principal.suspension().applies(&source.orgs) && self.owns(source)
                })
                .map(|source| {
                    (
                        orch_access_contract::decisions::ObjectRef {
                            kind: source.kind,
                            id: source.id,
                        },
                        Permission::EnvironmentManage,
                    )
                }),
            administers: self.administers(site),
            system_resource_admin: site.of_system
                && self.principal.is_system_admin()
                // A physical owner's machine stays personal even when it has no home node.
                && (site.kind != ObjectKind::Machine || site.first_owner.is_none()),
            owners_name_outsiders: self.policy.owners_name_outsiders,
        }
    }

    /// The row `permission` of the site, with the rows it follows.
    #[must_use]
    pub fn chain(&self, site: &Site, permission: Permission) -> Chain<'_> {
        let profile = self.graph.profile(site, permission);
        let mut chain = Chain::of(self.store.row(site.page(), Profile::Any, permission));
        for id in self.graph.document_ancestors(site.id) {
            chain = chain.in_document_container(
                id,
                self.store.row(
                    PageRef::live(StoredKind::DocumentContainer, id),
                    Profile::Any,
                    permission,
                ),
            );
        }
        if let Some(node) = site.beside {
            chain = chain.beside(
                node,
                self.store.row(
                    PageRef::live(StoredKind::Node, node),
                    Profile::Any,
                    permission,
                ),
            );
        }
        for place in &site.chain {
            chain = chain.under(
                *place,
                self.store
                    .row(PageRef::defaults_of(*place), profile, permission),
            );
        }
        chain
    }

    /// The row `permission` / `profile` of the defaults page of the first
    /// place of `places`, with the defaults it follows: the rest of `places`
    /// in turn. `places` ends with the system.
    #[must_use]
    pub fn defaults_chain(
        &self,
        places: &[PlaceRef],
        profile: Profile,
        permission: Permission,
    ) -> Chain<'_> {
        let mut places = places.iter();
        let first = places.next().copied().unwrap_or(PlaceRef::System);
        let mut chain = Chain::of_defaults(
            first,
            self.store
                .row(PageRef::defaults_of(first), profile, permission),
        );
        for place in places {
            chain = chain.under(
                *place,
                self.store
                    .row(PageRef::defaults_of(*place), profile, permission),
            );
        }
        chain
    }

    /// The entries of the row `permission` of the site that name the
    /// person, with the page each stands on: on the row, on what it
    /// follows, or under a lock on what the lock decides.
    #[must_use]
    pub fn named(&self, site: &Site, permission: Permission) -> Vec<Via> {
        self.matching(site, permission, None)
            .into_iter()
            .map(|named| Via {
                entry: named.entry.id,
                from: named.from,
            })
            .collect()
    }

    /// The entries of the row `permission` of the site that count now and are the person's, with
    /// the page each stands on: those that name them, a group of theirs, a role they hold, or
    /// everyone. With `work`, the path of a thread (the nodes from its root org down to it), also
    /// those that name the work in a place on that path: a resource named so is used by the threads
    /// there and by whoever gives them input, which names no person.
    #[must_use]
    pub fn matching(
        &self,
        site: &Site,
        permission: Permission,
        work: Option<&[Id]>,
    ) -> Vec<Named<'_>> {
        self.chain(site, permission)
            .holders(self.now())
            .into_iter()
            .filter(|named| {
                work.is_some_and(|path| named.entry.holder.is_work_on(path))
                    || self.principal.is_held_by(&named.entry.holder, |at| {
                        at.or(site.role_place)
                            .and_then(|place| self.role_at(place))
                            .map(|held| held.role)
                    })
            })
            .collect()
    }

    /// The ways in which the person holds `permission` on the site: none
    /// when they do not, or when their token does not carry it or does not
    /// reach the site. Archive is not looked at here.
    #[must_use]
    pub fn ways(&self, site: &Site, permission: Permission) -> Vec<Through> {
        let caller = self.principal.caller();
        if !caller.permits(permission) || !caller.reaches(site.path.as_deref()) {
            return Vec::new();
        }
        decisions::holds(permission, &self.standing(site), |row| {
            self.named(site, row)
        })
    }

    /// What a token leaves of the permissions held: all for a session.
    fn narrowed(&self, site: &Site, held: Vec<Held>) -> Vec<Held> {
        let caller = self.principal.caller();
        if !caller.reaches(site.path.as_deref()) {
            return Vec::new();
        }
        held.into_iter()
            .filter(|held| caller.permits(held.permission))
            .collect()
    }

    /// Everything the person holds on the site, each with how, cut down to
    /// what archive leaves while the site is archived, and to what a token
    /// carries. Needs the rows of every permission of its kind to have been
    /// read.
    #[must_use]
    pub fn effective(&self, site: &Site) -> Vec<Held> {
        let held =
            decisions::effective(site.kind, &self.standing(site), site.is_archived(), |row| {
                self.named(site, row)
            });
        self.narrowed(site, held)
    }

    /// Former mutation rights for an admitted retained document, still narrowed by this credential.
    pub(crate) fn effective_as_it_was(&self, site: &Site) -> Vec<Held> {
        let held = decisions::effective(site.kind, &self.standing(site), false, |row| {
            self.named(site, row)
        });
        self.narrowed(site, held)
    }

    /// The same decision for an uncustomized page whose live Owners row names this member.
    /// A hierarchy query chooses this projection only after checking that actual row and membership.
    #[must_use]
    pub fn effective_owned(&self, site: &Site) -> Vec<Held> {
        let mut standing = self.standing(site);
        standing.owner = true;
        let held = decisions::effective(site.kind, &standing, site.is_archived(), |row| {
            self.named(site, row)
        });
        self.narrowed(site, held)
    }

    /// The requested permissions from the same decision, optionally for an uncustomized page owned by this member.
    /// The requested rows and their covering levels must have been read, as for [`Self::effective`].
    #[must_use]
    pub fn effective_for(&self, site: &Site, permissions: &[Permission], owned: bool) -> Vec<Held> {
        if !self.principal.caller().reaches(site.path.as_deref()) {
            return Vec::new();
        }
        let wanted: Vec<_> = permissions
            .iter()
            .copied()
            .filter(|permission| {
                permission.exists_on(site.kind)
                    && self.principal.caller().permits(*permission)
                    && (!site.is_archived() || permission.when_archived() != WhenArchived::Withheld)
            })
            .collect();
        let mut standing = self.standing(site);
        if owned {
            standing.owner = true;
        }
        let mut rows = HashMap::new();
        for permission in &wanted {
            for row in std::iter::once(*permission).chain(permission.held_through()) {
                if row != Permission::AccessOwn && row.exists_on(site.kind) {
                    rows.entry(row).or_insert_with(|| self.named(site, row));
                }
            }
        }
        wanted
            .into_iter()
            .filter_map(|permission| {
                let through = decisions::holds(permission, &standing, |row| {
                    rows.get(&row).cloned().unwrap_or_default()
                });
                (!through.is_empty()).then_some(Held {
                    permission,
                    through,
                })
            })
            .collect()
    }

    /// Whether the same native decision grants one permission, without collecting unused provenance.
    /// An owned projection assumes the same member-owned uncustomized page as [`Self::effective_owned`].
    #[must_use]
    pub fn has_permission(&self, site: &Site, permission: Permission, owned: bool) -> bool {
        let mut standing = self.standing(site);
        if owned {
            standing.owner = true;
        }
        self.permission_held(site, permission, &standing, true)
    }

    fn permission_held(
        &self,
        site: &Site,
        permission: Permission,
        standing: &Standing,
        respect_archive: bool,
    ) -> bool {
        if !permission.exists_on(site.kind)
            || !self.principal.caller().permits(permission)
            || !self.principal.caller().reaches(site.path.as_deref())
            || (respect_archive
                && site.is_archived()
                && permission.when_archived() == WhenArchived::Withheld)
        {
            return false;
        }
        if !decisions::holds(permission, standing, |_| Vec::new()).is_empty() {
            return true;
        }
        std::iter::once(permission)
            .chain(permission.held_through())
            .filter(|row| *row != Permission::AccessOwn && row.exists_on(site.kind))
            .any(|row| {
                let entries = self.named(site, row);
                !entries.is_empty()
                    && !decisions::holds(permission, standing, |wanted| {
                        if wanted == row {
                            entries.clone()
                        } else {
                            Vec::new()
                        }
                    })
                    .is_empty()
            })
    }

    /// Whether the person holds anything on the site, which is what shows
    /// them its name. Needs the rows of every permission of its kind.
    #[must_use]
    pub fn holds_any(&self, site: &Site) -> bool {
        let standing = self.standing(site);
        let view = view_permission(site.kind);
        // Knowing a name survives archive. Check View first, then the other ordinary ways to know it.
        std::iter::once(view)
            .chain(site.kind.permissions().filter(|p| *p != view))
            .any(|permission| self.permission_held(site, permission, &standing, false))
    }

    /// Whether the site's View row is cut loose somewhere below the
    /// defaults of its root org: what other products call private. Needs
    /// the View row to have been read.
    #[must_use]
    pub fn restricted(&self, site: &Site) -> bool {
        let Some(root) = site.root else {
            return false;
        };
        site.kind
            .node()
            .and_then(|node| NodePermission::View.on(node))
            .is_some_and(|view| self.chain(site, view).cut_below(PlaceRef::Node(root)))
    }

    /// Whether the site's View row is cut loose somewhere below the defaults of the place `place`,
    /// which lies on its way: on its own page, or on the defaults page of a place between it and
    /// `place`. A place, or a thing in it, that was made private in that way does not follow `place`,
    /// and is passed over by whatever reaches what follows it. Needs the View row to have been read.
    #[must_use]
    pub fn restricted_below(&self, site: &Site, place: Id) -> bool {
        site.kind
            .node()
            .and_then(|node| NodePermission::View.on(node))
            .is_some_and(|view| self.chain(site, view).cut_below(PlaceRef::Node(place)))
    }
}

/// The permission which opens an object, for ordinary access and private-admin markers.
#[must_use]
pub const fn view_permission(kind: ObjectKind) -> Permission {
    match kind {
        ObjectKind::Org => Permission::OrgView,
        ObjectKind::Project => Permission::ProjectView,
        ObjectKind::Topic => Permission::TopicView,
        ObjectKind::Thread => Permission::ThreadView,
        ObjectKind::Document | ObjectKind::DocumentContainer => Permission::DocView,
        ObjectKind::Provider => Permission::ProviderView,
        ObjectKind::Agent => Permission::AgentView,
        ObjectKind::Environment => Permission::EnvironmentView,
        ObjectKind::Host => Permission::HostView,
        ObjectKind::Machine => Permission::MachineView,
        ObjectKind::Template => Permission::TemplateView,
        ObjectKind::McpServer => Permission::McpView,
        ObjectKind::Mount => Permission::MountView,
        ObjectKind::Skill => Permission::SkillView,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn node(id: Id, kind: NodeKind, path: &[Id]) -> NodeFacts {
        NodeFacts {
            id,
            kind,
            parent: path.iter().rev().nth(1).copied(),
            root: path[0],
            path: path.to_vec(),
            name: format!("{kind}"),
            first_owner: new_id(),
            archived: false,
            deleted: false,
            placement: Placement::default(),
        }
    }

    struct Tree {
        graph: Graph,
        root: Id,
        department: Id,
        project: Id,
        topic: Id,
        thread: Id,
        side: Id,
    }

    fn tree() -> Tree {
        let (root, department, project, topic, thread, side) =
            (new_id(), new_id(), new_id(), new_id(), new_id(), new_id());
        let graph = Graph::of(vec![
            node(root, NodeKind::Org, &[root]),
            node(department, NodeKind::Org, &[root, department]),
            node(project, NodeKind::Project, &[root, department, project]),
            node(topic, NodeKind::Topic, &[root, department, project, topic]),
            node(
                thread,
                NodeKind::Thread,
                &[root, department, project, topic, thread],
            ),
            node(
                side,
                NodeKind::Thread,
                &[root, department, project, topic, thread, side],
            ),
        ]);
        Tree {
            graph,
            root,
            department,
            project,
            topic,
            thread,
            side,
        }
    }

    #[test]
    fn a_page_s_stored_rows_are_its_own_however_many_pages_are_stored() {
        let stored = |id: Id, permission: &str, inherit: bool| StoredRow {
            object_kind: "document".into(),
            object_id: id,
            page: "live".into(),
            profile: "any".into(),
            permission: permission.into(),
            inherit,
            locked: false,
            updated_by: None,
            updated_at: chrono::Utc::now(),
            revision: 1,
        };
        let page = |id: Id| PageRef {
            kind: StoredKind::Document,
            id,
            page: PageKind::Live,
        };
        let (first, second, unstored) = (new_id(), new_id(), new_id());
        let mut store = Store::default();
        store.put_row(&stored(first, "doc.view", false)).unwrap();
        store.put_row(&stored(second, "doc.view", true)).unwrap();
        store.put_row(&stored(first, "doc.edit", true)).unwrap();
        // Stored again, a row is changed in place, not held twice.
        store.put_row(&stored(first, "doc.view", true)).unwrap();
        let rows: HashSet<(Permission, bool)> = store
            .rows_of(page(first))
            .map(|row| (row.permission, row.inherit))
            .collect();
        assert_eq!(
            rows,
            HashSet::from([(Permission::DocEdit, true), (Permission::DocView, true)])
        );
        assert_eq!(store.rows_of(page(first)).count(), 2);
        assert_eq!(store.rows_of(page(second)).count(), 1);
        assert_eq!(store.rows_of(page(unstored)).count(), 0);
    }

    #[test]
    fn a_page_follows_the_defaults_of_the_places_it_lies_in_nearest_first_and_the_system_last() {
        let t = tree();
        let site = t.graph.site_of(t.thread).unwrap();
        assert_eq!(
            site.chain,
            [
                PlaceRef::Node(t.topic),
                PlaceRef::Node(t.project),
                PlaceRef::Node(t.department),
                PlaceRef::Node(t.root),
                PlaceRef::System
            ]
        );
        assert_eq!(site.admin_places.first(), Some(&t.topic));
        assert_eq!(
            site.role_place,
            Some(t.topic),
            "a thread has no roles of its own"
        );
        let org = t.graph.site_of(t.department).unwrap();
        assert_eq!(org.chain, [PlaceRef::Node(t.root), PlaceRef::System]);
        assert_eq!(
            org.role_place,
            Some(t.department),
            "a role given on a place reaches its own page"
        );
        assert_eq!(org.admin_places, [t.root]);
        let root = t.graph.site_of(t.root).unwrap();
        assert_eq!(root.chain, [PlaceRef::System]);
        assert!(
            root.admin_places.is_empty(),
            "only the system administers a root org"
        );
    }

    #[test]
    fn a_side_thread_follows_the_places_and_not_the_thread_it_was_taken_from() {
        let t = tree();
        let site = t.graph.site_of(t.side).unwrap();
        assert_eq!(site.chain.first(), Some(&PlaceRef::Node(t.topic)));
        assert!(!site.chain.contains(&PlaceRef::Node(t.thread)));
        assert_eq!(site.orgs, [t.root, t.department]);
        assert_eq!(site.path.as_ref().unwrap().last(), Some(&t.side));
    }

    #[test]
    fn nothing_is_a_site_that_was_deleted_or_lies_below_what_was() {
        let mut t = tree();
        let mut nodes: Vec<NodeFacts> = t.graph.nodes().cloned().collect();
        for node in &mut nodes {
            if node.id == t.project {
                node.deleted = true;
            }
        }
        t.graph = Graph::of(nodes);
        assert!(t.graph.site_of(t.project).is_none());
        assert!(t.graph.site_of(t.thread).is_none(), "below a deleted node");
        assert!(t.graph.site_of(t.department).is_some());
        assert!(t.graph.site_of(new_id()).is_none());
    }

    #[test]
    fn the_nearest_archived_node_on_the_way_up_is_the_one_that_is_named() {
        let t = tree();
        let mut nodes: Vec<NodeFacts> = t.graph.nodes().cloned().collect();
        for node in &mut nodes {
            if node.id == t.project || node.id == t.topic {
                node.archived = true;
            }
        }
        let graph = Graph::of(nodes);
        assert_eq!(
            graph.site_of(t.thread).unwrap().archived_node,
            Some(t.topic)
        );
        assert_eq!(
            graph.site_of(t.project).unwrap().archived_node,
            Some(t.project)
        );
        assert_eq!(graph.site_of(t.department).unwrap().archived_node, None);
        assert!(graph.site_of(t.thread).unwrap().is_archived());
    }

    #[test]
    fn an_entry_that_does_not_read_is_a_stored_fault_and_not_a_guess() {
        let row = EntryRow {
            id: new_id(),
            object_kind: "node".into(),
            object_id: new_id(),
            page: "live".into(),
            profile: "any".into(),
            permission: "thread.view".into(),
            holder_kind: "role".into(),
            holder_id: None,
            holder_role: None,
            options: serde_json::json!({}),
            expires_at: None,
            note: String::new(),
            source: "page".into(),
            created_by: None,
            created_at: chrono::Utc::now(),
            removed_at: None,
            removed_by: None,
            revision: 1,
        };
        assert!(matches!(entry_of(&row), Err(Error::Stored(_))));
        let user = EntryRow {
            holder_kind: "user".into(),
            holder_id: Some(new_id()),
            ..row
        };
        assert!(entry_of(&user).is_ok());
    }
}
