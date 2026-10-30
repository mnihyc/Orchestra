//! Reading a page: stored rows, entries that expired or were removed, holders matched to a person, and the cache under the authorization epoch.
//!
//! [`read`] gives a page as it is shown: every row it has, what each row
//! comes to through the rows it follows and where that stands, who is
//! named, which lock decides, and what the caller may change. [`change`]
//! is the one way a page is changed: it checks that the change fits the
//! page, that the caller may make it and each part of it, that no lock above
//! decides a row it touches, that nobody is named from outside the root org
//! without the right to, and that the Owners row keeps a person; it then
//! writes the rows and entries, one audit row for each, the line of
//! history that goes with it, and moves the authorization epoch once. All of
//! it is in the caller's transaction, so that it is made as a whole or not
//! at all.
//!
//! A page that is read is read fresh: nothing is kept between requests,
//! because nothing here is slow enough to need it. What a caller would keep
//! belongs under the [epoch](crate::epoch), which every change moves.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{self, ObjectRef, Refusal, Through};
use orch_access_contract::pages::{
    self, Entry, EntrySource, Holder, Origin, Owners, OwnersFault, PageChange, PageKind, PageOf,
    PageState, StoredKind,
};
use orch_access_contract::permissions::{ObjectKind, Options, Permission, Profile, ResourceKind};
use orch_access_contract::roles::{NodeKind, PlaceKind, PlaceRef, Role};
use orch_db::Conn;
use orch_db::audit::{self, Actor};
use orch_db::queries::access::{self, NewEntry, PageKey, RowWrite};
use orch_db::queries::hierarchy;
use orch_db::queries::resources as resource_rows;
use orch_types::fields::FieldError;
use orch_types::{Id, Json, Revision, Time};

use crate::engine::{Frame, Graph, PageRef, Site, entry_of};
use crate::giving::{Acting, Gives, Giving, Judgement};
use crate::usable::ResourcePolicy;
use crate::{Error, Policy, Principal, external, missing, resources};

/// Which page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// The page of a node.
    Live(Id),
    /// The page of a resource: a provider, an agent profile, an environment, a host, a template, an
    /// MCP server or a mount.
    Resource(ObjectRef),
    /// The defaults page of a place: an org, a project, a topic, or the
    /// system.
    Defaults(PlaceRef),
}

/// What a person sees of a row of a page, with the one lock that decides
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowLock {
    /// Where the lock was set.
    pub by: PlaceRef,
    /// Whether that is this defaults page itself, where the row is changed.
    pub here: bool,
}

/// An entry that gives a row to somebody by way of a row it follows.
#[derive(Debug, Clone, PartialEq)]
pub struct Inherited {
    /// The entry.
    pub entry: Entry,
    /// The page it stands on.
    pub from: Origin,
}

/// One row of a page, as it is shown.
#[derive(Debug, Clone, PartialEq)]
pub struct RowView {
    /// The permission.
    pub permission: Permission,
    /// Which of the rows of the permission it is: `any` on a thing's page.
    pub profile: Profile,
    /// Whether the row follows the default of the place above.
    pub inherit: bool,
    /// What following amounts to now; none for a row that is cut loose and
    /// that no lock decides.
    pub inherited: Option<Vec<Inherited>>,
    /// Whom the row names itself, those entries that count now.
    pub entries: Vec<Entry>,
    /// The lock that decides it, if there is one.
    pub locked: Option<RowLock>,
    /// Whether the row is locked here, on this defaults page.
    pub lock_set_here: bool,
    /// Whether the caller manages this row: may take holders out of it, cut it loose and tie it
    /// back.
    pub writable: bool,
    /// Whether the caller may name somebody in it: they hold the permission here themself and may
    /// share here, as an owner, as a manager of the page or because the place lets holders
    /// share; true also for somebody who does not manage the row.
    pub can_give: bool,
}

/// A page as it is shown.
#[derive(Debug, Clone, PartialEq)]
pub struct PageView {
    /// Which page it is.
    pub target: Target,
    /// The name of what it is of.
    pub name: String,
    /// The kind of node, for a node's page and for a place's defaults; none
    /// for the page of a resource.
    pub kind: Option<NodeKind>,
    /// Its own page, or the defaults it gives.
    pub page: PageKind,
    /// The first owner; none for a defaults page.
    pub first_owner: Option<Id>,
    /// The Owners row, those named now, in the order they were named. A
    /// defaults page has none.
    pub owners: Vec<Entry>,
    /// Whether the caller may change the Owners row.
    pub owners_writable: bool,
    /// Whether the caller may name somebody from outside the root org here.
    pub external: bool,
    /// The places whose admins hold `access.manage` on it without being
    /// named, nearest first; the system is the last.
    pub admins_of: Vec<PlaceRef>,
    /// The rows, in the order of the catalogue.
    pub rows: Vec<RowView>,
    /// How the page stands against the defaults it follows.
    pub state: PageState,
    /// For a defaults page of a place: how many pages inside it follow at
    /// least one of its rows. None for the other pages.
    pub followers: Option<i64>,
    /// How many times the page was changed, and one: a page nobody changed
    /// is the first.
    pub revision: Revision,
    /// Whether the caller may change the page at all.
    pub writable: bool,
    /// What the caller may give here, which is at most what they hold.
    pub gives: Gives,
    /// The place where a role that names no place is held for this page: the place a page of a place
    /// is of, or that its defaults are of, the place a thread lies in, a resource's home, the node a
    /// document lies beside; none for the defaults of the system.
    pub role_place: Option<Id>,
    /// The nearest archived node on the way up, itself included.
    pub archived_node: Option<Id>,
}

/// What reading or changing a page needs to know of where it stands.
struct Setup<'p> {
    frame: Frame<'p>,
    site: Option<Site>,
    resource: Option<resources::Resource>,
    page_of: PageOf,
    page: PageRef,
    places: Vec<PlaceRef>,
}

impl<'p> Setup<'p> {
    async fn load(
        conn: &mut Conn,
        principal: &'p Principal,
        target: Target,
        policy: Policy,
    ) -> Result<Setup<'p>, Error> {
        let (graph, site, resource) = match target {
            Target::Live(node) | Target::Defaults(PlaceRef::Node(node)) => {
                let graph = Graph::load(conn, &[node]).await?;
                let site = graph.site_of(node).ok_or(Error::NotFound(node))?;
                (graph, Some(site), None)
            }
            Target::Defaults(PlaceRef::System) => (Graph::default(), None, None),
            Target::Resource(object) if object.kind.is_document() => {
                let (graph, site) = crate::documents::site_for_page(conn, object.id).await?;
                (graph, Some(site), None)
            }
            Target::Resource(object) => {
                let (graph, resource, site) = resources::site(conn, object).await?;
                (graph, Some(site), Some(resource))
            }
        };
        let (page_of, page, places) = match (target, &site) {
            (Target::Live(_) | Target::Resource(_), Some(site)) => (
                PageOf::Thing {
                    kind: site.kind,
                    of_system: site.of_system,
                },
                site.page(),
                site.chain.clone(),
            ),
            (Target::Defaults(PlaceRef::Node(id)), Some(site)) => {
                let kind = site
                    .kind
                    .node()
                    .and_then(NodeKind::place)
                    .ok_or(Error::NotFound(id))?;
                let mut places = vec![PlaceRef::Node(id)];
                places.extend(site.chain.iter().copied());
                (
                    PageOf::Defaults(kind),
                    PageRef::defaults_of(PlaceRef::Node(id)),
                    places,
                )
            }
            _ => (
                PageOf::Defaults(PlaceKind::System),
                PageRef::defaults_of(PlaceRef::System),
                vec![PlaceRef::System],
            ),
        };
        let sites: Vec<Site> = site.iter().cloned().collect();
        let everything: Vec<Permission> = Permission::ALL.to_vec();
        let mut extra = vec![page];
        if let Some(place) = site
            .as_ref()
            .filter(|site| site.kind.node().is_some_and(|node| node.place().is_some()))
        {
            extra.push(PageRef::defaults_of(PlaceRef::Node(place.id)));
        }
        let frame =
            Frame::load_with(conn, principal, policy, graph, &sites, &extra, &everything).await?;
        Ok(Setup {
            frame,
            site,
            resource,
            page_of,
            page,
            places,
        })
    }

    /// The row `permission` / `profile` of the page with the rows it
    /// follows.
    fn chain(&self, permission: Permission, profile: Profile) -> pages::Chain<'_> {
        match (&self.site, self.page.page) {
            (Some(site), PageKind::Live) => self.frame.chain(site, permission),
            _ => self.frame.defaults_chain(&self.places, profile, permission),
        }
    }

    /// Whether the page follows nothing: the defaults of the system, and the page of a resource
    /// of the system, which is what was put on it.
    fn follows_nothing(&self) -> bool {
        self.page.kind == StoredKind::System
            || (self.page.page == PageKind::Live
                && self.site.as_ref().is_some_and(|site| site.of_system))
    }

    /// Whether the caller may change the page: they hold `access.manage`
    /// on what it is of; for the system's defaults, they are a system admin.
    fn ways_to_change(&self) -> Vec<Through> {
        match &self.site {
            Some(site) => self.frame.ways(site, Permission::AccessManage),
            None if self.frame.principal().is_system_admin() => vec![Through::SystemAdmin],
            None => Vec::new(),
        }
    }

    fn frame_is_defaults(&self) -> bool {
        self.page.page == PageKind::Defaults
    }

    fn owns_or_administers(&self) -> bool {
        match &self.site {
            Some(site) => self.frame.owns(site) || self.frame.administers(site),
            None => self.frame.principal().is_system_admin(),
        }
    }
}

/// Reads the page `target` for display, for the caller. The caller has
/// checked that they may see what the page is of: `node.view` on the node,
/// and on a place for its defaults.
pub async fn read(
    conn: &mut Conn,
    principal: &Principal,
    target: Target,
    policy: Policy,
) -> Result<PageView, Error> {
    let setup = Setup::load(conn, principal, target, policy).await?;
    let writable = !setup.ways_to_change().is_empty();
    let giving = Giving::of(
        &setup.frame,
        setup.site.as_ref(),
        setup.page.page,
        Acting::Ordinary,
    );
    let now = principal.now();
    let mut rows = Vec::new();
    for (permission, profile) in setup.page_of.rows() {
        let chain = setup.chain(permission, profile);
        let stored = match setup.page.page {
            PageKind::Live => setup
                .frame
                .store()
                .row(setup.page, Profile::Any, permission),
            PageKind::Defaults => setup.frame.store().row(setup.page, profile, permission),
        };
        let follows_nothing = setup.follows_nothing();
        let locked = chain.locked_by().map(|by| RowLock {
            by,
            here: chain.locked_above().is_none(),
        });
        rows.push(RowView {
            permission,
            profile,
            inherit: !follows_nothing && stored.is_none_or(|row| row.inherit),
            inherited: chain
                .inherited(now)
                .filter(|_| !follows_nothing)
                .map(|named| {
                    named
                        .into_iter()
                        .map(|named| Inherited {
                            entry: named.entry.clone(),
                            from: named.from,
                        })
                        .collect()
                }),
            entries: stored
                .map(|row| row.names(now).cloned().collect())
                .unwrap_or_default(),
            lock_set_here: stored.is_some_and(|row| row.locked),
            writable: writable && chain.locked_above().is_none(),
            can_give: giving.can_give(permission, profile),
            locked,
        });
    }
    let changed = rows.iter().any(|row| {
        (!row.inherit && !setup.follows_nothing()) || !row.entries.is_empty() || row.lock_set_here
    });
    let restricted = match (&setup.site, setup.page.page) {
        (Some(site), PageKind::Live) => setup.frame.restricted(site),
        _ => false,
    };
    let mut owners: Vec<Entry> = match (&setup.site, setup.page.page) {
        (Some(site), PageKind::Live) => setup
            .frame
            .owner_entries(site.id)
            .iter()
            .filter(|entry| entry.counts(now))
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    // The first owner is first in the row as it is shown; the others stay in the order they were named.
    if let Some(first) = setup.site.as_ref().and_then(|site| site.first_owner) {
        owners.sort_by_key(|entry| entry.holder != Holder::User { id: first });
    }
    let external = setup.site.as_ref().is_some_and(|site| {
        !setup
            .frame
            .ways(site, Permission::AccessExternal)
            .is_empty()
    });
    let mut admins_of: Vec<PlaceRef> = setup
        .site
        .iter()
        .flat_map(|site| site.admin_places.iter().copied().map(PlaceRef::Node))
        .collect();
    admins_of.push(PlaceRef::System);
    let revision = access::page_revision(conn, &setup.page.key()).await? + 1;
    let followers = match (&setup.site, setup.page.page) {
        (Some(site), PageKind::Defaults) => {
            Some(crate::defaults::followers(conn, site.id, &setup.page_of.rows()).await?)
        }
        _ => None,
    };
    Ok(PageView {
        target,
        name: setup
            .site
            .as_ref()
            .map_or_else(|| "the system".to_owned(), |site| site.name.clone()),
        kind: setup.site.as_ref().and_then(|site| site.kind.node()),
        page: setup.page.page,
        first_owner: match setup.page.page {
            PageKind::Live => setup.site.as_ref().and_then(|site| site.first_owner),
            PageKind::Defaults => None,
        },
        owners,
        owners_writable: writable && setup.owns_or_administers(),
        external,
        admins_of,
        rows,
        state: PageState::of(changed, restricted),
        followers,
        revision,
        writable,
        gives: giving.gives(),
        role_place: setup.site.as_ref().and_then(|site| site.role_place),
        archived_node: setup.site.as_ref().and_then(|site| site.archived_node),
    })
}

/// Why a page could not be changed as asked.
#[derive(Debug, thiserror::Error)]
pub enum ChangeError {
    /// The page could not be read, the caller is suspended where it is, or
    /// the database failed.
    #[error(transparent)]
    Engine(#[from] Error),
    /// The caller may not make this change, or a part of it: what they
    /// lack, and who can give it.
    #[error("the caller may not make this change")]
    Denied(Box<Refusal>),
    /// Only a system admin changes the defaults of the system.
    #[error("only a system admin changes the defaults of the system")]
    SystemAdminOnly,
    /// The change does not fit the page.
    #[error("the change does not fit the page")]
    Invalid(Vec<FieldError>),
    /// The page was changed since the caller saw it.
    #[error("the page was changed since it was read")]
    Stale {
        /// The revision it has now.
        current: Revision,
    },
    /// A row the change touches is decided by a lock set at a place above.
    #[error("{permission} is decided by a lock above")]
    Locked {
        /// The row.
        permission: Permission,
        /// Where the lock was set.
        by: PlaceRef,
    },
    /// The change names somebody from outside the root org, and the caller
    /// may not.
    #[error("somebody from outside the root org is named without the right to")]
    Outside {
        /// The nearest people who may.
        ask: Vec<Id>,
        /// How many more could.
        ask_more: i64,
    },
    /// The last person in the Owners row is not taken out.
    #[error("the last person in an Owners row is not taken out")]
    LastOwner,
    /// An entry is made or changed in the Use row of a subscription that its home keeps to its
    /// first owner: nobody else uses it, whoever would name them.
    #[error("the subscription is kept to its first owner")]
    SubscriptionNotShared,
    /// What is registered from a person's machine keeps that machine's owner as its first owner for
    /// as long as it is registered there.
    #[error("the first owner stays the owner of the machine")]
    FirstOwnerStays {
        /// The owner of the machine.
        owner: Id,
    },
    /// An entry that names somebody from outside the root tree, new or given a new end, would run
    /// beyond the bound of its place (`sharing.external_max_days`): it has no end, or a later one.
    #[error("an entry for somebody from outside would run beyond the bound of its place")]
    ExternalLifetime {
        /// The latest end it may have, counted from now.
        latest: Time,
        /// Where the bound is set: an org, a project or a topic; `None` for the system.
        place: Option<Id>,
    },
}

impl From<orch_db::DbError> for ChangeError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Engine(error.into())
    }
}

/// What a change did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Whether anything was changed at all. A change that says what the page
    /// already says changes nothing, and moves nothing.
    pub changed: bool,
    /// The revision of the page after the change.
    pub revision: Revision,
    /// The authorization epoch after the change; the one before it, as far
    /// as this change knows, when nothing changed.
    pub epoch: i64,
    /// Whether the change named the caller where the page did not give them
    /// what the row is for: an admin forcing an act. Its audit row says so.
    pub r#override: bool,
    /// Rows that a lock above decides, which a change of a whole page left
    /// alone.
    pub skipped: Vec<(Permission, Profile, PlaceRef)>,
}

/// An entry of the page as it is stored, with the row it is in.
#[derive(Debug, Clone)]
struct StoredEntry {
    permission: Permission,
    profile: Profile,
    entry: Entry,
}

/// Names that a line of history uses, found when first needed.
struct Names<'a> {
    known: HashMap<Id, String>,
    graph: &'a Graph,
    root: Option<Id>,
    role_place: Option<Id>,
}

impl Names<'_> {
    async fn user(&mut self, conn: &mut Conn, id: Id) -> Result<String, Error> {
        if let Some(name) = self.known.get(&id) {
            return Ok(name.clone());
        }
        let name = if let Some(person) = orch_db::queries::identity::user_by_id(conn, id).await? {
            let addresses = access::holder_user_addresses(conn, &[id]).await?;
            let address = addresses
                .iter()
                .find(|(_, root, _, _)| Some(*root) == self.root)
                .or_else(|| addresses.first())
                .map_or_else(
                    || format!("@{}", person.username),
                    |(_, _, _, address)| address.clone(),
                );
            format!("{} · {address}", person.display_name)
        } else {
            "a removed person".to_owned()
        };
        self.known.insert(id, name.clone());
        Ok(name)
    }

    async fn group(&mut self, conn: &mut Conn, id: Id) -> Result<String, Error> {
        if let Some(name) = self.known.get(&id) {
            return Ok(name.clone());
        }
        let name = match access::groups_full(conn, &[id]).await?.into_iter().next() {
            Some(group) => format!(
                "the group {}, {}",
                group.name,
                self.place(conn, group.org_id).await?
            ),
            None => "a removed group".to_owned(),
        };
        self.known.insert(id, name.clone());
        Ok(name)
    }

    async fn place(&self, conn: &mut Conn, id: Id) -> Result<String, Error> {
        let graph = if self.graph.get(id).is_some() {
            self.graph.clone()
        } else {
            Graph::load(conn, &[id]).await?
        };
        let Some(node) = graph.get(id) else {
            return Ok("a removed place".to_owned());
        };
        if node
            .path
            .iter()
            .any(|id| graph.get(*id).is_some_and(|node| node.deleted))
        {
            return Ok("a removed place".to_owned());
        }
        Ok(node
            .path
            .iter()
            .filter_map(|id| graph.get(*id).map(|node| node.name.clone()))
            .collect::<Vec<_>>()
            .join(" > "))
    }

    async fn holder(&mut self, conn: &mut Conn, holder: &Holder) -> Result<String, Error> {
        Ok(match *holder {
            Holder::User { id } => self.user(conn, id).await?,
            Holder::Group { id } => self.group(conn, id).await?,
            Holder::Role { role, at } => match at.or(self.role_place) {
                Some(place) => {
                    format!("the {}s of {}", role.name(), self.place(conn, place).await?)
                }
                None => format!("the {}s of the system", role.name()),
            },
            Holder::Node { id } => format!("the work in {}", self.place(conn, id).await?),
            Holder::Agents { id } => format!("the agents of {}", self.place(conn, id).await?),
            Holder::Members { id } => format!("the members of {}", self.place(conn, id).await?),
            Holder::Everyone => "everyone".to_owned(),
        })
    }
}

fn holder_json(holder: &Holder) -> Json {
    serde_json::to_value(holder).unwrap_or(Json::Null)
}

/// The path of a row of the change as the request wrote it: `name` for the
/// rows a choice made, `rows[i]` for the others.
fn row_path(index: usize, named: usize) -> String {
    match index.checked_sub(named) {
        Some(index) => format!("rows[{index}]"),
        None => "name".to_owned(),
    }
}

/// Makes the changes, one by one, and writes the line of history of each.
struct Writer<'a> {
    conn: &'a mut Conn,
    page_key: PageKey,
    defaults: bool,
    node: Option<Id>,
    object: Option<ObjectRef>,
    root: Option<Id>,
    by: Id,
    now: DateTime<Utc>,
    origin: &'a audit::Origin,
    actor: String,
    thing: String,
    names: Names<'a>,
    rows_now: HashMap<(Permission, Profile), (bool, bool)>,
    entries_now: Vec<StoredEntry>,
    changed: bool,
    overridden: bool,
    /// How the entries this change makes came about.
    source: EntrySource,
    /// Only an agent share of a place retains two entries with the same holder and row.
    share_inside: Option<bool>,
}

impl Writer<'_> {
    async fn audit(
        &mut self,
        action: AuditAction,
        object: Option<Id>,
        r#override: bool,
        summary: String,
        details: Json,
    ) -> Result<(), Error> {
        let summary = if self.defaults && action.as_str().starts_with("permission.") {
            let agents = details
                .get("holder")
                .and_then(|holder| holder.get("kind"))
                .and_then(Json::as_str)
                == Some("agents");
            let what = if agents {
                "who may act as an agent"
            } else {
                "who may access things"
            };
            format!(
                "{} changed {} in {}'s defaults",
                self.actor, what, self.thing
            )
        } else {
            summary
        };
        let kind = if self
            .object
            .is_some_and(|thing| thing.kind == ObjectKind::DocumentContainer)
            && action == AuditAction::DocumentTransferred
        {
            "document_container"
        } else {
            action
                .object_kind()
                .unwrap_or_else(|| self.object.map_or("node", |thing| thing.kind.as_str()))
        };
        let mut entry = audit::Entry::new(
            Actor::User(self.by),
            action.as_str(),
            kind,
            object,
            &summary,
        );
        entry.history = action.is_history_line();
        entry.r#override = r#override && action.takes_override_mark();
        entry.node_id = self.node;
        entry.org_id = self.root;
        // A resource has no node: the line says which resource it is about, and an environment's
        // history is found by its own column.
        entry.environment_id = self
            .object
            .filter(|thing| thing.kind == ObjectKind::Environment)
            .map(|thing| thing.id);
        entry.details = details;
        if let (Some(thing), Json::Object(members)) = (self.object, &mut entry.details) {
            members.insert(
                "object".to_owned(),
                serde_json::json!({ "kind": thing.kind.as_str(), "id": thing.id }),
            );
        }
        audit::append(self.conn, &entry, self.origin, self.now).await?;
        Ok(())
    }

    fn page_word(&self) -> &'static str {
        if self.defaults { "defaults" } else { "page" }
    }

    /// Stores the flags of a row as they are to be, when they differ from
    /// what they are.
    async fn flags(
        &mut self,
        permission: Permission,
        profile: Profile,
        inherit: Option<bool>,
        locked: Option<bool>,
    ) -> Result<(), Error> {
        let (was_inherit, was_locked) = self
            .rows_now
            .get(&(permission, profile))
            .copied()
            .unwrap_or((true, false));
        let (inherit, locked) = (inherit.unwrap_or(was_inherit), locked.unwrap_or(was_locked));
        if (inherit, locked) == (was_inherit, was_locked) {
            return Ok(());
        }
        access::write_row(
            self.conn,
            &RowWrite {
                page: &self.page_key,
                profile: profile.as_str(),
                permission: permission.as_str(),
                inherit,
                locked,
            },
            Some(self.by),
            self.now,
        )
        .await?;
        self.rows_now
            .insert((permission, profile), (inherit, locked));
        self.changed = true;
        let what = match (inherit != was_inherit, locked != was_locked) {
            (true, _) if !inherit => "cut loose from the defaults",
            (true, _) => "made to follow the defaults",
            (_, true) if locked => "locked",
            _ => "unlocked",
        };
        let summary = format!(
            "{} {} the {} row of the {} of {}",
            self.actor,
            what,
            permission.label(),
            self.page_word(),
            self.thing
        );
        self.audit(
            orch_db::audit::action!(AuditAction::PermissionRowChanged),
            None,
            false,
            summary,
            serde_json::json!({
                "permission": permission.as_str(),
                "profile": profile.as_str(),
                "page": self.page_word(),
                "inherit": inherit,
                "locked": locked,
            }),
        )
        .await
    }

    /// Names a holder in a row. A holder who is named already keeps what
    /// the entry says except for what the request says anew.
    async fn add(
        &mut self,
        permission: Permission,
        profile: Profile,
        entry: &pages::CheckedEntry,
        r#override: bool,
    ) -> Result<(), Error> {
        let (kind, holder_id, holder_role) = entry.holder.stored();
        let options = entry.options.to_json();
        let given_options = !matches!(&options, Json::Object(members) if members.is_empty());
        let expires_at = entry.expires_at.map(Time::get);
        let existing = access::find_entry_for_share(
            self.conn,
            &self.page_key,
            profile.as_str(),
            permission.as_str(),
            kind.as_str(),
            holder_id,
            holder_role.map(Role::as_str),
            self.share_inside,
        )
        .await?;
        let who = self.names.holder(self.conn, &entry.holder).await?;
        if let Some(existing) = existing {
            // An entry that is still good keeps what the request does not say anew. One that
            // has ended counts for nothing: naming the holder again names them afresh.
            let ended = existing.expires_at.is_some_and(|end| end <= self.now);
            let now_options = if given_options || ended {
                options
            } else {
                existing.options.clone()
            };
            let now_expires = if ended {
                expires_at
            } else {
                expires_at.or(existing.expires_at)
            };
            let now_note = if entry.note.is_empty() && !ended {
                existing.note.clone()
            } else {
                entry.note.clone()
            };
            if now_options == existing.options
                && now_expires == existing.expires_at
                && now_note == existing.note
            {
                return Ok(());
            }
            access::rewrite_entry(self.conn, existing.id, &now_options, now_expires, &now_note)
                .await?;
            self.changed = true;
            let (action, word) = if ended {
                self.overridden |= r#override;
                (
                    orch_db::audit::action!(AuditAction::PermissionAdded),
                    "named",
                )
            } else {
                (
                    orch_db::audit::action!(AuditAction::PermissionUpdated),
                    "changed",
                )
            };
            let summary = format!(
                "{} {} {} in the {} row of the {} of {}",
                self.actor,
                word,
                who,
                permission.label(),
                self.page_word(),
                self.thing
            );
            return self
                .audit(
                    action,
                    Some(existing.id),
                    ended && r#override,
                    summary,
                    entry_details(
                        permission,
                        profile,
                        self.page_word(),
                        &entry.holder,
                        &now_options,
                        now_expires,
                    ),
                )
                .await;
        }
        let id = orch_types::new_id();
        access::insert_entry_for_share(
            self.conn,
            &NewEntry {
                id,
                page: &self.page_key,
                profile: profile.as_str(),
                permission: permission.as_str(),
                holder_kind: kind.as_str(),
                holder_id,
                holder_role: holder_role.map(Role::as_str),
                options: &options,
                expires_at,
                note: &entry.note,
                source: self.source.as_str(),
                created_by: Some(self.by),
                at: self.now,
            },
            self.share_inside,
        )
        .await?;
        self.changed = true;
        self.overridden |= r#override;
        let summary = format!(
            "{} named {} in the {} row of the {} of {}",
            self.actor,
            who,
            permission.label(),
            self.page_word(),
            self.thing
        );
        self.entries_now.push(StoredEntry {
            permission,
            profile,
            entry: Entry {
                id,
                holder: entry.holder,
                options: options.clone(),
                expires_at: entry.expires_at,
                note: entry.note.clone(),
                source: self.source,
                created_by: Some(self.by),
                created_at: Time::new(self.now),
                removed_at: None,
                revision: 1,
            },
        });
        self.audit(
            AuditAction::PermissionAdded,
            Some(id),
            r#override,
            summary,
            entry_details(
                permission,
                profile,
                self.page_word(),
                &entry.holder,
                &options,
                expires_at,
            ),
        )
        .await
    }

    /// Changes an entry that exists, in the parts the request names.
    async fn update(
        &mut self,
        permission: Permission,
        profile: Profile,
        update: &pages::CheckedUpdate,
    ) -> Result<(), Error> {
        let Some(position) = self
            .entries_now
            .iter()
            .position(|stored| stored.entry.id == update.entry)
        else {
            return Ok(());
        };
        let before = self.entries_now[position].entry.clone();
        let options = update
            .options
            .as_ref()
            .map_or_else(|| before.options.clone(), Options::to_json);
        let expires_at = update.expires_at.map_or(before.expires_at, |end| end);
        let note = update.note.clone().unwrap_or_else(|| before.note.clone());
        if options == before.options && expires_at == before.expires_at && note == before.note {
            return Ok(());
        }
        access::rewrite_entry(
            self.conn,
            before.id,
            &options,
            expires_at.map(Time::get),
            &note,
        )
        .await?;
        self.changed = true;
        let who = self.names.holder(self.conn, &before.holder).await?;
        let summary = format!(
            "{} changed {} in the {} row of the {} of {}",
            self.actor,
            who,
            permission.label(),
            self.page_word(),
            self.thing
        );
        let entry = &mut self.entries_now[position].entry;
        entry.options = options.clone();
        entry.expires_at = expires_at;
        entry.note = note;
        self.audit(
            AuditAction::PermissionUpdated,
            Some(before.id),
            false,
            summary,
            entry_details(
                permission,
                profile,
                self.page_word(),
                &before.holder,
                &options,
                expires_at.map(Time::get),
            ),
        )
        .await
    }

    /// Stops naming an entry's holder.
    async fn remove(
        &mut self,
        permission: Permission,
        profile: Profile,
        id: Id,
    ) -> Result<(), Error> {
        let Some(position) = self
            .entries_now
            .iter()
            .position(|stored| stored.entry.id == id)
        else {
            return Ok(());
        };
        let removed = self.entries_now.remove(position).entry;
        if !access::remove_entry(self.conn, id, self.by, self.now).await? {
            return Ok(());
        }
        self.changed = true;
        let who = self.names.holder(self.conn, &removed.holder).await?;
        let summary = format!(
            "{} took {} out of the {} row of the {} of {}",
            self.actor,
            who,
            permission.label(),
            self.page_word(),
            self.thing
        );
        self.audit(
            orch_db::audit::action!(AuditAction::PermissionRemoved),
            Some(id),
            false,
            summary,
            entry_details(
                permission,
                profile,
                self.page_word(),
                &removed.holder,
                &removed.options,
                removed.expires_at.map(Time::get),
            ),
        )
        .await
    }

    /// Cuts every row loose, except those a lock above decides.
    async fn make_private(
        &mut self,
        rows: &[(Permission, Profile)],
        locked_above: &HashMap<(Permission, Profile), PlaceRef>,
    ) -> Result<Vec<(Permission, Profile, PlaceRef)>, Error> {
        let mut skipped = Vec::new();
        let mut cut = 0_usize;
        for (permission, profile) in rows {
            if let Some(place) = locked_above.get(&(*permission, *profile)) {
                skipped.push((*permission, *profile, *place));
                continue;
            }
            let (inherit, locked) = self
                .rows_now
                .get(&(*permission, *profile))
                .copied()
                .unwrap_or((true, false));
            if inherit {
                access::write_row(
                    self.conn,
                    &RowWrite {
                        page: &self.page_key,
                        profile: profile.as_str(),
                        permission: permission.as_str(),
                        inherit: false,
                        locked,
                    },
                    Some(self.by),
                    self.now,
                )
                .await?;
                self.rows_now
                    .insert((*permission, *profile), (false, locked));
                cut += 1;
            }
        }
        if cut > 0 {
            self.changed = true;
            let summary = format!(
                "{} made the {} of {} private",
                self.actor,
                self.page_word(),
                self.thing
            );
            self.audit(
                AuditAction::PermissionRowChanged,
                None,
                false,
                summary,
                serde_json::json!({ "whole": "private", "page": self.page_word(), "rows": cut }),
            )
            .await?;
        }
        Ok(skipped)
    }

    /// Puts the page back to following the defaults throughout, except the
    /// rows a lock above decides.
    async fn reset(
        &mut self,
        locked_above: &HashMap<(Permission, Profile), PlaceRef>,
    ) -> Result<Vec<(Permission, Profile, PlaceRef)>, Error> {
        let mut skipped = Vec::new();
        let mut rows = 0_usize;
        let stored: Vec<((Permission, Profile), (bool, bool))> = self
            .rows_now
            .iter()
            .map(|(key, flags)| (*key, *flags))
            .collect();
        for ((permission, profile), (inherit, locked)) in stored {
            if let Some(place) = locked_above.get(&(permission, profile)) {
                skipped.push((permission, profile, *place));
                continue;
            }
            if !inherit || locked {
                access::write_row(
                    self.conn,
                    &RowWrite {
                        page: &self.page_key,
                        profile: profile.as_str(),
                        permission: permission.as_str(),
                        inherit: true,
                        locked: false,
                    },
                    Some(self.by),
                    self.now,
                )
                .await?;
                self.rows_now.insert((permission, profile), (true, false));
                rows += 1;
            }
        }
        let doomed: Vec<(Permission, Profile, Id)> = self
            .entries_now
            .iter()
            .filter(|stored| stored.permission != Permission::AccessOwn)
            .filter(|stored| !locked_above.contains_key(&(stored.permission, stored.profile)))
            .map(|stored| (stored.permission, stored.profile, stored.entry.id))
            .collect();
        let entries = doomed.len();
        for (_, _, id) in &doomed {
            access::remove_entry(self.conn, *id, self.by, self.now).await?;
        }
        self.entries_now
            .retain(|stored| !doomed.iter().any(|(_, _, id)| *id == stored.entry.id));
        if rows > 0 || entries > 0 {
            self.changed = true;
            let summary = format!(
                "{} put the {} of {} back to follow the defaults",
                self.actor,
                self.page_word(),
                self.thing
            );
            self.audit(
                AuditAction::PermissionRowChanged,
                None,
                false,
                summary,
                serde_json::json!({
                    "whole": "reset", "page": self.page_word(), "rows": rows, "entries": entries
                }),
            )
            .await?;
        }
        Ok(skipped)
    }

    /// Writes the change of the Owners row.
    async fn owners(
        &mut self,
        node: Id,
        first_before: Option<Id>,
        after: &pages::OwnersAfter,
    ) -> Result<(), Error> {
        let options = Json::Object(serde_json::Map::new());
        for holder in &after.add {
            let (kind, holder_id, holder_role) = holder.stored();
            let id = orch_types::new_id();
            access::insert_entry(
                self.conn,
                &NewEntry {
                    id,
                    page: &self.page_key,
                    profile: Profile::Any.as_str(),
                    permission: Permission::AccessOwn.as_str(),
                    holder_kind: kind.as_str(),
                    holder_id,
                    holder_role: holder_role.map(Role::as_str),
                    options: &options,
                    expires_at: None,
                    note: "",
                    source: EntrySource::Page.as_str(),
                    created_by: Some(self.by),
                    at: self.now,
                },
            )
            .await?;
            self.changed = true;
            let who = self.names.holder(self.conn, holder).await?;
            let summary = format!("{} named {} an owner of {}", self.actor, who, self.thing);
            self.audit(
                AuditAction::PermissionAdded,
                Some(id),
                false,
                summary,
                entry_details(
                    Permission::AccessOwn,
                    Profile::Any,
                    "page",
                    holder,
                    &options,
                    None,
                ),
            )
            .await?;
        }
        for id in &after.remove {
            let removed = access::owner_entries(self.conn, std::slice::from_ref(&self.page_key))
                .await?
                .into_iter()
                .find(|row| row.id == *id);
            if !access::remove_entry(self.conn, *id, self.by, self.now).await? {
                continue;
            }
            self.changed = true;
            let holder = removed
                .as_ref()
                .map(entry_of)
                .transpose()?
                .map(|entry| entry.holder);
            let who = match &holder {
                Some(holder) => self.names.holder(self.conn, holder).await?,
                None => id.to_string(),
            };
            let summary = format!(
                "{} took {} out of the owners of {}",
                self.actor, who, self.thing
            );
            self.audit(
                AuditAction::PermissionRemoved,
                Some(*id),
                false,
                summary,
                serde_json::json!({
                    "permission": Permission::AccessOwn.as_str(),
                    "page": "page",
                    "holder": holder.as_ref().map_or(Json::Null, holder_json),
                }),
            )
            .await?;
        }
        if first_before != Some(after.first) {
            let action = match self.object {
                Some(thing) if thing.kind == ObjectKind::Document => {
                    orch_db::queries::documents::set_owner(self.conn, thing.id, after.first)
                        .await?;
                    orch_db::audit::action!(AuditAction::DocumentTransferred)
                }
                Some(thing) if thing.kind == ObjectKind::DocumentContainer => {
                    orch_db::queries::documents::containers::set_owner(
                        self.conn,
                        thing.id,
                        after.first,
                        self.now,
                    )
                    .await?;
                    orch_db::audit::action!(AuditAction::DocumentTransferred)
                }
                Some(thing) => {
                    resource_rows::set_resource_owner(
                        self.conn,
                        thing.kind.as_str(),
                        thing.id,
                        after.first,
                        self.now,
                    )
                    .await?;
                    orch_db::audit::action!(AuditAction::ResourceTransferred)
                }
                None => {
                    hierarchy::set_first_owner(self.conn, node, after.first, self.now).await?;
                    orch_db::audit::action!(AuditAction::NodeTransferred)
                }
            };
            self.changed = true;
            let who = self.names.user(self.conn, after.first).await?;
            let summary = format!(
                "{} made {} the first owner of {}",
                self.actor, who, self.thing
            );
            self.audit(
                action,
                Some(node),
                false,
                summary,
                serde_json::json!({ "from": first_before, "to": after.first }),
            )
            .await?;
        }
        Ok(())
    }
}

fn entry_details(
    permission: Permission,
    profile: Profile,
    page: &str,
    holder: &Holder,
    options: &Json,
    expires_at: Option<DateTime<Utc>>,
) -> Json {
    serde_json::json!({
        "permission": permission.as_str(),
        "profile": profile.as_str(),
        "page": page,
        "holder": holder_json(holder),
        "options": options,
        "expires_at": expires_at.map(|end| Time::new(end).to_string()),
    })
}

async fn refuse(
    conn: &mut Conn,
    setup: &Setup<'_>,
    site: &Site,
    required: Permission,
    lacking: &[Permission],
) -> Result<ChangeError, Error> {
    let caller = setup.frame.principal().caller();
    // A token that does not carry the permission, or does not reach the page, is refused with
    // nobody to ask: what its scopes leave out is not for anybody to give.
    if !caller.permits(required) || !caller.reaches(site.path.as_deref()) {
        return Ok(ChangeError::Denied(Box::new(Refusal::of(
            required,
            decisions::ObjectRef {
                kind: site.kind,
                id: site.id,
            },
            Vec::new(),
        ))));
    }
    Ok(ChangeError::Denied(Box::new(
        missing::refusal(conn, &setup.frame, site, required, lacking).await?,
    )))
}

/// Changes the page `target` as `change` says, for the caller, in the
/// caller's transaction.
///
/// The caller needs `access.manage` on what the page is of (a system admin
/// for the defaults of the system), except for a change that only names
/// holders and takes back entries the caller made: that is sharing, and takes
/// holding what is given and, for somebody who neither owns the thing nor
/// manages its page, that the place lets holders share (see [`giving`](crate::giving)).
/// What a change gives is at most what its giver holds: a change that gives more is refused
/// whole, with what the giver lacks. Naming a holder of `access.manage`,
/// changing the Owners row, or naming a holder of the right to name somebody
/// from outside takes more, as the contract says; naming somebody from
/// outside the root org takes `access.external`, or being an owner where
/// the policy lets owners. A row decided by a lock above is changed where
/// the lock is. When the change names the caller where the page did not
/// give them what the row is for, and they hold the right to change the
/// page only by administering the place, it is an override and the audit row
/// of that entry says so.
pub async fn change(
    conn: &mut Conn,
    principal: &Principal,
    target: Target,
    change: &PageChange,
    policy: Policy,
    origin: &audit::Origin,
) -> Result<Outcome, ChangeError> {
    change_with(
        conn,
        principal,
        target,
        change,
        policy,
        None,
        EntrySource::Page,
        None,
        Acting::Ordinary,
        origin,
    )
    .await
}

/// The change that a share with agents comes to on one page of what is shared: the entries of the
/// rows of the areas, on the page of the thing, and for a place shared with what is in it on its
/// defaults page too. The sharer needs what sharing needs: holding what is given (on a defaults
/// page, on a thing that follows the row) and, if they neither own the thing nor manage its page,
/// that the place lets holders share; on a defaults page they need no more than that, as the
/// share writes it as a part of itself.
pub async fn change_by_share(
    conn: &mut Conn,
    principal: &Principal,
    target: Target,
    change: &PageChange,
    policy: Policy,
    share_inside: Option<bool>,
    origin: &audit::Origin,
) -> Result<Outcome, ChangeError> {
    change_with(
        conn,
        principal,
        target,
        change,
        policy,
        None,
        EntrySource::Page,
        share_inside,
        Acting::Share,
        origin,
    )
    .await
}

/// What the sharer of a share with agents may give of it, told before anything is written.
#[derive(Debug, Clone, PartialEq)]
pub enum ShareJudgement {
    /// They may give these rows, which are all of those asked or, for a defaults page, those of them
    /// in the forms of environment that they hold.
    Allowed(Vec<(Permission, Profile)>),
    /// They may not, and this is everything they lack and who can give it.
    Refused(Box<Refusal>),
}

/// Judges the rows of a share with agents, `(permission, form)` for the page `target`: whether the
/// sharer may share here and holds every one of them. The refusal names everything lacking.
pub async fn judge_share(
    conn: &mut Conn,
    principal: &Principal,
    target: Target,
    rows: &[(Permission, Profile)],
    policy: Policy,
) -> Result<ShareJudgement, Error> {
    let setup = Setup::load(conn, principal, target, policy).await?;
    let giving = Giving::of(
        &setup.frame,
        setup.site.as_ref(),
        setup.page.page,
        Acting::Share,
    );
    let lacking = match giving.judge(rows) {
        Judgement::Allowed { rows } => return Ok(ShareJudgement::Allowed(rows)),
        Judgement::KeptToOwners => vec![Permission::AccessManage],
        Judgement::Lacks(lacking) => lacking,
    };
    let Some(site) = &setup.site else {
        return Ok(ShareJudgement::Allowed(rows.to_vec()));
    };
    let required = lacking.first().copied().unwrap_or(Permission::AccessManage);
    match refuse(conn, &setup, site, required, &lacking).await? {
        ChangeError::Denied(refusal) => Ok(ShareJudgement::Refused(refusal)),
        _ => Ok(ShareJudgement::Allowed(rows.to_vec())),
    }
}

/// [`change`] for the page of a resource, with the policies resolved at its home: whether its
/// owners may name somebody from outside the root org, and whether the subscription it is, if
/// it is one, is kept to its first owner, in which case an entry in its Use row is refused.
pub async fn change_resource(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    change: &PageChange,
    policy: &ResourcePolicy,
    origin: &audit::Origin,
) -> Result<Outcome, ChangeError> {
    change_resource_under(
        conn,
        principal,
        object,
        change,
        policy.node_policy(),
        policy,
        origin,
    )
    .await
}

/// [`change_resource`] under a `policy` that also says whether the home lets holders share
/// (`sharing.allow_holders`), as a page of a resource or of a document is changed by somebody who
/// holds what they give without managing it.
pub async fn change_resource_under(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    change: &PageChange,
    policy: Policy,
    resources: &ResourcePolicy,
    origin: &audit::Origin,
) -> Result<Outcome, ChangeError> {
    change_with(
        conn,
        principal,
        Target::Resource(object),
        change,
        policy,
        Some(resources),
        EntrySource::Page,
        None,
        Acting::Ordinary,
        origin,
    )
    .await
}

/// The change that an approved request comes to: the same change of the page of a node, a
/// resource or a document, by the person who approved it, and the entries it makes are marked as
/// those of an approved request. `policy` is what the settings say at the thing's home.
pub async fn change_for_request(
    conn: &mut Conn,
    principal: &Principal,
    target: Target,
    change: &PageChange,
    policy: &ResourcePolicy,
    origin: &audit::Origin,
) -> Result<Outcome, ChangeError> {
    let subscriptions = matches!(target, Target::Resource(_)).then_some(policy);
    change_with(
        conn,
        principal,
        target,
        change,
        policy.node_policy(),
        subscriptions,
        EntrySource::Request,
        None,
        Acting::Decision,
        origin,
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "the caller, the page and the change, the two policies, how entries come about, how the change is made, and where from"
)]
async fn change_with(
    conn: &mut Conn,
    principal: &Principal,
    target: Target,
    change: &PageChange,
    policy: Policy,
    resource_policy: Option<&ResourcePolicy>,
    source: EntrySource,
    share_inside: Option<bool>,
    acting: Acting,
    origin: &audit::Origin,
) -> Result<Outcome, ChangeError> {
    match target {
        Target::Live(node) | Target::Defaults(PlaceRef::Node(node)) => {
            if !hierarchy::lock(conn, node).await? {
                return Err(Error::NotFound(node).into());
            }
        }
        Target::Resource(object) if object.kind == ObjectKind::DocumentContainer => {
            let alive = orch_db::queries::documents::containers::lock(conn, object.id)
                .await?
                .is_some_and(|row| row.deleted_at.is_none());
            if !alive {
                return Err(Error::NotFound(object.id).into());
            }
        }
        Target::Resource(object) if object.kind == ObjectKind::Document => {
            orch_db::queries::documents::containers::lock_document_scope(conn, object.id).await?;
            let alive = orch_db::queries::documents::lock(conn, object.id)
                .await?
                .is_some_and(|document| document.deleted_at.is_none());
            if !alive {
                return Err(Error::NotFound(object.id).into());
            }
        }
        Target::Resource(object) => {
            if !resource_rows::lock_resource(conn, object.kind.as_str(), object.id).await? {
                return Err(Error::NotFound(object.id).into());
            }
        }
        Target::Defaults(PlaceRef::System) => access::lock_system_defaults(conn).await?,
    }
    let setup = Setup::load(conn, principal, target, policy).await?;
    let now = principal.now();
    let by = principal.user();

    let orgs: &[Id] = setup.site.as_ref().map_or(&[], |site| site.orgs.as_slice());
    if let Some(closing) = principal.suspension().closing(orgs) {
        return Err(Error::Suspended { org: closing.org() }.into());
    }
    let manages = setup.ways_to_change();
    let giving = Giving::of(&setup.frame, setup.site.as_ref(), setup.page.page, acting);
    // Somebody who does not manage the page may still share: name holders further in rows they
    // hold, and take back what they gave, where the place lets holders share. Nothing else is
    // theirs, and a defaults page and the decision of a request are managing the page.
    let sharing_only = manages.is_empty();
    // Somebody who holds nothing on the thing has nothing to share there, and does not read its page
    // by asking to change it.
    let holds_something = setup
        .site
        .as_ref()
        .is_some_and(|site| setup.frame.holds_any(site));
    if sharing_only && (!holds_something || !giving.may_share()) {
        return Err(match &setup.site {
            Some(site) => {
                refuse(
                    conn,
                    &setup,
                    site,
                    Permission::AccessManage,
                    &[Permission::AccessManage],
                )
                .await?
            }
            None => ChangeError::SystemAdminOnly,
        });
    }

    let mut checked = pages::check(change, setup.page_of).map_err(ChangeError::Invalid)?;
    // The Export rows that travel with a choice are named as far as the giver may give them: what they
    // do not hold, what a lock above decides and what the place keeps to its owners is left out of the
    // change, not refused, since the person is let read all the same (the export of a place).
    for row in checked.rows.iter_mut().filter(|row| row.optional) {
        if !giving.can_give(row.permission, row.profile) {
            row.add.clear();
        }
    }
    if sharing_only
        && (checked.owners.is_some()
            || checked.whole.is_some()
            || checked
                .rows
                .iter()
                .any(|row| row.inherit.is_some() || row.locked.is_some())
            // A change that names nobody and takes nothing back is no sharing either: reading a
            // page is not changing it.
            || (acting == Acting::Ordinary
                && checked.rows.iter().all(|row| {
                    row.add.is_empty() && row.update.is_empty() && row.remove.is_empty()
                })))
    {
        return Err(match &setup.site {
            Some(site) => {
                refuse(
                    conn,
                    &setup,
                    site,
                    Permission::AccessManage,
                    &[Permission::AccessManage],
                )
                .await?
            }
            None => ChangeError::SystemAdminOnly,
        });
    }
    let named_rows = checked.rows.len() - change.rows.as_ref().map_or(0, Vec::len);
    let page_key = setup.page.key();
    let revision_before = access::page_revision(conn, &page_key).await? + 1;
    if let Some(expected) = checked.expected_revision
        && expected != revision_before
    {
        return Err(ChangeError::Stale {
            current: revision_before,
        });
    }

    // Nobody else uses a subscription that its home keeps to its first owner, whoever would name
    // them and whatever a default would give.
    if setup
        .resource
        .as_ref()
        .zip(resource_policy)
        .is_some_and(|(resource, shared)| {
            resource.kind == ResourceKind::Provider && shared.keeps_to_its_owner(resource)
        })
        && checked.rows.iter().any(|row| {
            row.permission == Permission::ProviderUse
                && (!row.add.is_empty() || !row.update.is_empty())
        })
    {
        return Err(ChangeError::SubscriptionNotShared);
    }

    let asks = checked.asks();
    if asks.owner_or_admin && !setup.owns_or_administers() {
        return Err(match &setup.site {
            Some(site) => {
                refuse(
                    conn,
                    &setup,
                    site,
                    Permission::AccessOwn,
                    &[Permission::AccessOwn],
                )
                .await?
            }
            None => ChangeError::SystemAdminOnly,
        });
    }
    if asks.admin {
        let admin = match &setup.site {
            Some(site) => setup.frame.administers(site),
            None => principal.is_system_admin(),
        };
        if !admin {
            return Err(match &setup.site {
                Some(site) => {
                    refuse(
                        conn,
                        &setup,
                        site,
                        Permission::AccessExternal,
                        &[Permission::AccessExternal],
                    )
                    .await?
                }
                None => ChangeError::SystemAdminOnly,
            });
        }
    }

    // What is stored for the page now, read under the lock.
    let mut entries_now: Vec<StoredEntry> = Vec::new();
    for stored in access::entries_of(conn, &page_key).await? {
        if let (Ok(permission), Ok(profile)) = (
            stored.permission.parse::<Permission>(),
            stored.profile.parse::<Profile>(),
        ) {
            entries_now.push(StoredEntry {
                permission,
                profile,
                entry: entry_of(&stored)?,
            });
        }
    }
    let mut rows_now: HashMap<(Permission, Profile), (bool, bool)> = HashMap::new();
    for stored in access::stored_rows_of(conn, &page_key).await? {
        if let (Ok(permission), Ok(profile)) = (
            stored.permission.parse::<Permission>(),
            stored.profile.parse::<Profile>(),
        ) {
            rows_now.insert((permission, profile), (stored.inherit, stored.locked));
        }
    }

    // A row that a lock above decides is changed where the lock is.
    let mut locked_above: HashMap<(Permission, Profile), PlaceRef> = HashMap::new();
    for (permission, profile) in setup.page_of.rows() {
        if let Some(place) = setup.chain(permission, profile).locked_above() {
            locked_above.insert((permission, profile), place);
        }
    }
    for row in &checked.rows {
        let touches = row.inherit.is_some()
            || row.locked.is_some()
            || !row.add.is_empty()
            || !row.update.is_empty()
            || !row.remove.is_empty();
        if touches && let Some(place) = locked_above.get(&(row.permission, row.profile)) {
            return Err(ChangeError::Locked {
                permission: row.permission,
                by: *place,
            });
        }
    }

    // A sharer undoes their own: changing or taking out what somebody else made, or telling an
    // entry somebody else made anew, is managing the page.
    if sharing_only {
        let theirs = |id: &Id| {
            entries_now
                .iter()
                .any(|stored| stored.entry.id == *id && stored.entry.created_by != Some(by))
        };
        let alters_another = checked.rows.iter().any(|row| {
            row.remove.iter().any(&theirs)
                || row.update.iter().any(|update| theirs(&update.entry))
                || row.add.iter().any(|entry| {
                    let says_more = !matches!(entry.options.to_json(), Json::Object(ref members) if members.is_empty())
                        || entry.expires_at.is_some()
                        || !entry.note.is_empty();
                    says_more
                        && entries_now.iter().any(|stored| {
                            stored.permission == row.permission
                                && stored.profile == row.profile
                                && stored.entry.holder == entry.holder
                                && stored.entry.created_by != Some(by)
                        })
                })
        });
        if alters_another {
            return Err(match &setup.site {
                Some(site) => {
                    refuse(
                        conn,
                        &setup,
                        site,
                        Permission::AccessManage,
                        &[Permission::AccessManage],
                    )
                    .await?
                }
                None => ChangeError::SystemAdminOnly,
            });
        }
    }

    // Nobody gives more than they hold. An admin who names themself is forcing an act their
    // page does not give them, which is a override and is marked as one; the entry that follows
    // is then given by someone who holds it.
    let mut asked: Vec<(Permission, Profile)> = Vec::new();
    for row in &checked.rows {
        let gives_something = row.add.iter().any(|entry| {
            !(entry.holder == (Holder::User { id: by })
                && giving.administers()
                && giving.manages()
                && principal.caller().permits(row.permission))
        }) || row.update.iter().any(|update| {
            sharing_only
                || entries_now
                    .iter()
                    .find(|stored| stored.entry.id == update.entry)
                    .is_some_and(|stored| crate::giving::entry_widens(&stored.entry, update))
        }) || (sharing_only && !row.remove.is_empty());
        if gives_something && !asked.contains(&(row.permission, row.profile)) {
            asked.push((row.permission, row.profile));
        }
    }
    if !asked.is_empty() {
        match giving.judge(&asked) {
            Judgement::Allowed { rows } => {
                if acting == Acting::Share && setup.page.page == PageKind::Defaults {
                    for row in &mut checked.rows {
                        if !rows.contains(&(row.permission, row.profile)) {
                            row.add.clear();
                        }
                    }
                }
            }
            Judgement::KeptToOwners => {
                return Err(match &setup.site {
                    Some(site) => {
                        refuse(
                            conn,
                            &setup,
                            site,
                            Permission::AccessManage,
                            &[Permission::AccessManage],
                        )
                        .await?
                    }
                    None => ChangeError::SystemAdminOnly,
                });
            }
            Judgement::Lacks(lacking) => {
                let required = lacking.first().copied().unwrap_or(Permission::AccessManage);
                return Err(match &setup.site {
                    Some(site) => refuse(conn, &setup, site, required, &lacking).await?,
                    None => ChangeError::SystemAdminOnly,
                });
            }
        }
    }

    let mut faults: Vec<FieldError> = Vec::new();

    // A selected group or place is named only while its source place remains visible.
    let named_holders: Vec<Holder> = checked
        .rows
        .iter()
        .flat_map(|row| row.add.iter().map(|entry| entry.holder))
        .chain(
            checked
                .owners
                .iter()
                .flat_map(|owners| owners.add.iter().copied()),
        )
        .collect();
    let groups: Vec<Id> = named_holders
        .iter()
        .filter_map(|holder| match holder {
            Holder::Group { id } => Some(*id),
            _ => None,
        })
        .collect();
    let mut sources: Vec<Id> = access::groups_full(conn, &groups)
        .await?
        .iter()
        .map(|group| group.org_id)
        .collect();
    sources.extend(named_holders.iter().filter_map(|holder| match holder {
        Holder::Role { at, .. } => *at,
        Holder::Members { id } | Holder::Node { id } | Holder::Agents { id } => Some(*id),
        _ => None,
    }));
    sources.sort_unstable();
    sources.dedup();
    if !sources.is_empty() {
        let visible = crate::nodes::holding(
            conn,
            setup.frame.principal(),
            &sources,
            orch_access_contract::permissions::PermissionName::OfNode(
                orch_access_contract::permissions::NodePermission::View,
            ),
            crate::Act::Use,
            Policy::default(),
        )
        .await?;
        if sources
            .iter()
            .any(|id| !visible.iter().any(|place| place.id == *id))
        {
            faults.push(FieldError {
                path: "holder".into(),
                message: "the place a holder belongs to must be visible to you".into(),
            });
        }
        let nodes = orch_db::queries::hierarchy::with_ancestors(conn, &sources).await?;
        if named_holders.iter().any(|holder| matches!(holder, Holder::Members { id } if !nodes.iter().any(|node| node.id == *id && node.kind == "org"))) {
            faults.push(FieldError { path: "holder".into(), message: "members by domain belong to an org".into() });
        }
        if named_holders.iter().any(|holder| matches!(holder, Holder::Role { at: Some(id), .. } if !nodes.iter().any(|node| node.id == *id && matches!(node.kind.as_str(), "org" | "project" | "topic")))) {
            faults.push(FieldError { path: "holder".into(), message: "a role is held at an org, project or topic".into() });
        }
    }

    // The Owners row.
    let mut owners_after: Option<pages::OwnersAfter> = None;
    if let Some(checked_owners) = &checked.owners {
        let Some(site) = setup.site.as_ref().filter(|_| !setup.frame_is_defaults()) else {
            return Err(ChangeError::Invalid(vec![FieldError {
                path: "owners".into(),
                message: "is for the page of a thing".into(),
            }]));
        };
        let current = Owners {
            entries: setup.frame.owner_entries(site.id).to_vec(),
            first: site.first_owner.unwrap_or(by),
        };
        for (index, id) in checked_owners.remove.iter().enumerate() {
            if !current.entries.iter().any(|entry| entry.id == *id) {
                faults.push(FieldError {
                    path: format!("owners.remove[{index}]"),
                    message: "is no entry of the Owners row".into(),
                });
            }
        }
        if faults.is_empty() {
            match current.after(checked_owners, now) {
                Ok(after) => owners_after = Some(after),
                Err(OwnersFault::LastPerson) => return Err(ChangeError::LastOwner),
            }
            // Other source-bound registrations keep their source owner. Environment ownership
            // is independent of the machine owners' inherent management.
            if let (Some(after), Some(resource)) = (&owners_after, &setup.resource)
                && let Some(machine_owner) = resource.machine_owner
                && resource.registered_from_a_machine()
                && resource.kind != ResourceKind::Environment
                && after.first != machine_owner
            {
                return Err(ChangeError::FirstOwnerStays {
                    owner: machine_owner,
                });
            }
        }
    }

    // Who is named from outside the root org. An admin who names themself is
    // not an outsider to the place they administer: it is how they force an
    // act there, and a system admin is a member of no tree.
    if let Some(root) = setup.site.as_ref().and_then(|site| site.root) {
        let self_named = Holder::User { id: by };
        let administers = setup
            .site
            .as_ref()
            .is_some_and(|site| setup.frame.administers(site));
        let named: Vec<Holder> = checked
            .rows
            .iter()
            .flat_map(|row| row.add.iter().map(|entry| entry.holder))
            .filter(|holder| !(administers && *holder == self_named))
            .collect();
        if external::any_outside(conn, root, &named).await? {
            if setup.page.page == PageKind::Defaults {
                faults.push(FieldError {
                    path: "rows".into(),
                    message: "a defaults page names nobody from outside the root org".into(),
                });
            } else if let Some(site) = &setup.site
                && setup
                    .frame
                    .ways(site, Permission::AccessExternal)
                    .is_empty()
            {
                return Err(outside(conn, &setup, site).await?);
            }
        }
        if let (Some(after), Some(site)) = (&owners_after, &setup.site)
            && external::any_outside(conn, root, &after.add).await?
        {
            return Err(outside(conn, &setup, site).await?);
        }
    }

    // An entry that names somebody from outside the root tree runs no longer than the bound of its
    // place (`sharing.external_max_days`, counted from now): a new one without an end or with a later
    // one is refused with the latest end it may have and the place that sets the bound, and so is one
    // given a new end beyond it. An admin who names themself is no outsider to what they administer.
    if let Some(site) = &setup.site
        && let Some(root) = site.root
        && setup.page.page != PageKind::Defaults
    {
        let bounds = external::LifeBounds::load(conn).await?;
        if let Some(bound) = bounds.at(&external::way_of(site)) {
            let self_named = Holder::User { id: by };
            let administers = setup.frame.administers(site);
            let mut beyond: Vec<Holder> = Vec::new();
            for row in &checked.rows {
                beyond.extend(
                    row.add
                        .iter()
                        .filter(|entry| !bound.admits(entry.expires_at, now))
                        .map(|entry| entry.holder),
                );
                for update in &row.update {
                    let Some(end) = update.expires_at else {
                        continue;
                    };
                    if bound.admits(end, now) {
                        continue;
                    }
                    beyond.extend(
                        entries_now
                            .iter()
                            .filter(|stored| stored.entry.id == update.entry)
                            .map(|stored| stored.entry.holder),
                    );
                }
            }
            beyond.retain(|holder| !(administers && *holder == self_named));
            if !beyond.is_empty() && external::any_outside(conn, root, &beyond).await? {
                return Err(ChangeError::ExternalLifetime {
                    latest: bound.latest_end(now),
                    place: bound.place,
                });
            }
        }
    }

    // Rows: what is referred to.
    for (index, row) in checked.rows.iter().enumerate() {
        let path = row_path(index, named_rows);
        if setup.follows_nothing() && row.inherit.is_some() {
            faults.push(FieldError {
                path: format!("{path}.inherit"),
                message: "is refused: this page follows nothing".into(),
            });
        }
        let in_row = |id: &Id| {
            entries_now.iter().any(|stored| {
                stored.entry.id == *id
                    && stored.permission == row.permission
                    && stored.profile == row.profile
            })
        };
        for (at, update) in row.update.iter().enumerate() {
            if !in_row(&update.entry) {
                faults.push(FieldError {
                    path: format!("{path}.update[{at}].entry_id"),
                    message: "is no entry of this row".into(),
                });
            }
        }
        for (at, id) in row.remove.iter().enumerate() {
            if !in_row(id) {
                faults.push(FieldError {
                    path: format!("{path}.remove[{at}]"),
                    message: "is no entry of this row".into(),
                });
            }
        }
    }
    if checked.whole == Some(pages::Whole::Private) && setup.follows_nothing() {
        faults.push(FieldError {
            path: "whole".into(),
            message: "is refused: this page follows nothing to be cut loose from".into(),
        });
    }
    if !faults.is_empty() {
        return Err(ChangeError::Invalid(faults));
    }

    // A new agents entry changes what the receiving agents may work with. The ordinary page
    // request must ask that side too; a share already asked it before reaching this writer.
    if acting == Acting::Ordinary {
        let recipients: std::collections::BTreeSet<Id> = checked
            .rows
            .iter()
            .flat_map(|row| &row.add)
            .filter_map(|entry| match entry.holder {
                Holder::Agents { id } => Some(id),
                _ => None,
            })
            .collect();
        for recipient in recipients {
            match crate::nodes::check_named(
                conn,
                principal,
                recipient,
                orch_access_contract::permissions::PermissionName::OfNode(
                    orch_access_contract::permissions::NodePermission::Interact,
                ),
                crate::Act::Use,
                Policy::default(),
            )
            .await?
            {
                decisions::Decision::Allowed { .. } => {}
                decisions::Decision::Refused(refusal) => {
                    return Err(ChangeError::Denied(Box::new(refusal)));
                }
            }
        }
    }

    // Everything fits: make the change.
    let actor = {
        let mut names = Names {
            known: HashMap::new(),
            graph: setup.frame.graph(),
            root: setup.site.as_ref().and_then(|site| site.root),
            role_place: setup.site.as_ref().and_then(|site| site.role_place),
        };
        names.user(conn, by).await?
    };
    let mut writer = Writer {
        page_key: page_key.clone(),
        defaults: setup.page.page == PageKind::Defaults,
        node: setup
            .site
            .as_ref()
            .filter(|site| site.kind.node().is_some())
            .map(|site| site.id),
        object: match target {
            Target::Resource(object) => Some(object),
            _ => None,
        },
        root: setup.site.as_ref().and_then(|site| site.root),
        by,
        now: now.get(),
        origin,
        actor,
        thing: setup
            .site
            .as_ref()
            .map_or_else(|| "the system".to_owned(), |site| site.name.clone()),
        names: Names {
            known: HashMap::new(),
            graph: setup.frame.graph(),
            root: setup.site.as_ref().and_then(|site| site.root),
            role_place: setup.site.as_ref().and_then(|site| site.role_place),
        },
        rows_now,
        entries_now,
        changed: false,
        overridden: false,
        source,
        share_inside,
        conn,
    };
    let all_rows = setup.page_of.rows();
    let mut skipped = Vec::new();
    match checked.whole {
        Some(pages::Whole::Private) => {
            skipped = writer.make_private(&all_rows, &locked_above).await?
        }
        Some(pages::Whole::Reset) => skipped = writer.reset(&locked_above).await?,
        None => {}
    }
    if let (Some(after), Some(site)) = (&owners_after, &setup.site) {
        writer.owners(site.id, site.first_owner, after).await?;
    }
    for row in &checked.rows {
        writer
            .flags(row.permission, row.profile, row.inherit, row.locked)
            .await?;
        for entry in &row.add {
            let r#override = !writer.defaults
                && entry.holder == (Holder::User { id: by })
                && setup.site.as_ref().is_some_and(|site| {
                    decisions::is_override(&manages, &setup.frame.ways(site, row.permission))
                });
            writer
                .add(row.permission, row.profile, entry, r#override)
                .await?;
        }
        for update in &row.update {
            writer.update(row.permission, row.profile, update).await?;
        }
        for id in &row.remove {
            writer.remove(row.permission, row.profile, *id).await?;
        }
    }
    let (changed, r#override) = (writer.changed, writer.overridden);
    if !changed {
        return Ok(Outcome {
            changed: false,
            revision: revision_before,
            epoch: access::epoch(conn).await?,
            r#override: false,
            skipped,
        });
    }
    let count = access::count_page_change(conn, &page_key, Some(by), now.get()).await?;
    let epoch = access::bump_epoch(conn).await?;
    if let Target::Resource(object) = target
        && object.kind == ObjectKind::DocumentContainer
    {
        orch_db::queries::documents::containers::touch(conn, object.id, now.get()).await?;
    }
    Ok(Outcome {
        changed: true,
        revision: count + 1,
        epoch,
        r#override,
        skipped,
    })
}

async fn outside(conn: &mut Conn, setup: &Setup<'_>, site: &Site) -> Result<ChangeError, Error> {
    let who = missing::who_may_name_outsiders(conn, &setup.frame, site).await?;
    let (ask, ask_more) = decisions::nearest(&who).ask();
    Ok(ChangeError::Outside { ask, ask_more })
}

/// Writes the Owners row of a new thing, selecting an active member as first owner for an outside
/// creator and naming that creator in its Manage row. Other rows follow the defaults. Moves the
/// authorization epoch, which a new owner and a new place both do.
pub async fn start(
    conn: &mut Conn,
    kind: StoredKind,
    thing: Id,
    maker: Id,
    at: Time,
) -> Result<(), Error> {
    let (first, maker_manage) =
        crate::ownership::first_for_new_page(conn, kind, thing, maker, at).await?;
    let page = PageRef::live(kind, thing).key();
    let options = Json::Object(serde_json::Map::new());
    access::insert_entry(
        conn,
        &NewEntry {
            id: orch_types::new_id(),
            page: &page,
            profile: Profile::Any.as_str(),
            permission: Permission::AccessOwn.as_str(),
            holder_kind: "user",
            holder_id: Some(first),
            holder_role: None,
            options: &options,
            expires_at: None,
            note: "",
            source: EntrySource::Creation.as_str(),
            created_by: Some(maker),
            at: at.get(),
        },
    )
    .await?;
    if let Some(permission) = maker_manage {
        access::insert_entry(
            conn,
            &NewEntry {
                id: orch_types::new_id(),
                page: &page,
                profile: Profile::Any.as_str(),
                permission: permission.as_str(),
                holder_kind: "user",
                holder_id: Some(maker),
                holder_role: None,
                options: &options,
                expires_at: None,
                note: "",
                source: EntrySource::Creation.as_str(),
                created_by: Some(maker),
                at: at.get(),
            },
        )
        .await?;
    }
    access::bump_epoch(conn).await?;
    Ok(())
}
