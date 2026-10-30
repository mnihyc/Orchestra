//! A resource's page and visibility, its home at any level of orgs and who may make one there, what is usable by whoever acts (the author of an input, whoever puts a thread somewhere, the thread's first owner for an input no person sent), hosts, and who may add a mount's location.
//!
//! A resource is described to the access engine as a node is: by a [`Site`]. Its page follows the
//! defaults of its home, the org it was made in at whatever level, or for an environment its home
//! node; the admins of that home, and of the places above, administer it; a role without a place
//! is held at the home. A resource of the system has no home and follows nothing: its page is what
//! was put on it, and the system's admins hold every permission on it. So every question about a
//! resource is asked of the same [`Frame`] as one about a node, in the same few statements.
//!
//! [`check`] answers whether a caller holds a permission on one resource, [`needs`] the same for an
//! act that needs permissions on several things at once, with a refusal that names everything
//! missing, object by object. [`visible`] lists what the caller holds anything on, in a number of
//! statements that does not grow with the list. [`effective`] says what a person holds on one and
//! through what. Whether a person may *use* a resource, which is a rule of its own, is in
//! [`usable`](crate::usable).
//!
//! Nothing is asked of a resource's archive: archive is a state of nodes. A resource that is
//! disabled or deleted keeps its page, so that whoever manages it can bring it back; only its use
//! is refused.

use orch_access_contract::decisions::{
    Closed, Decision, Held, MissingOptions, ObjectRef, Refusal, Through,
};
use orch_access_contract::permissions::{ObjectKind, Permission, Placement, ResourceKind};
use orch_access_contract::roles::{self, PlaceRef};
use orch_db::Conn;
use orch_db::queries::resources::{self as rows, AccessFacts, Looker};
use orch_types::Id;

use crate::engine::{Frame, Graph, Site};
use crate::nodes::{Page, archive_allows, every_permission};
use crate::{Act, Error, Policy, Principal, missing};

/// How many candidates are looked at in one round when a list is filtered by what the caller may
/// see: enough that a list of mostly visible resources takes one round, few enough to keep a
/// frame small.
const CANDIDATE_BATCH: i64 = 100;

/// A resource, as a decision about it needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// What kind of resource it is.
    pub kind: ResourceKind,
    /// Its identifier.
    pub id: Id,
    /// What it is called.
    pub name: String,
    /// Its first owner; none for a resource of the system that nobody enrolled.
    pub first_owner: Option<Id>,
    /// The place its page follows: the org it was made in, at any level, or for an environment its
    /// home node. None for a resource of the system.
    pub home: Option<Id>,
    /// The root org of that place.
    pub root: Option<Id>,
    /// Whether it is switched off.
    pub disabled: bool,
    /// Whether it is deleted.
    pub deleted: bool,
    /// For an environment, where it runs; the managed kind for every other resource.
    pub placement: Placement,
    /// For a provider that is a subscription, the harness it is a login of.
    pub subscription: Option<String>,
    /// For a provider, the arrangement types its owner allows.
    pub arrangements: Vec<String>,
    /// The machine of a person that it was registered from, where it was.
    pub machine: Option<Id>,
    /// Whose that machine is.
    pub machine_owner: Option<Id>,
}

impl Resource {
    fn from_row(row: AccessFacts) -> Result<Self, Error> {
        Ok(Self {
            kind: row.kind.parse()?,
            id: row.id,
            name: row.name,
            first_owner: row.owner_id,
            home: row.home_id,
            root: row.org_id,
            disabled: row.disabled,
            deleted: row.deleted,
            placement: row
                .placement
                .as_deref()
                .map(str::parse::<Placement>)
                .transpose()?
                .unwrap_or_default(),
            subscription: if row.subscription {
                row.harness_kind
            } else {
                None
            },
            arrangements: row.arrangements,
            machine: row.machine_id,
            machine_owner: row.machine_owner_id,
        })
    }

    /// The resource as a thing that has a page.
    #[must_use]
    pub fn object(&self) -> ObjectRef {
        ObjectRef {
            kind: self.kind.into(),
            id: self.id,
        }
    }

    /// Whether it is a resource of the system, which has no home.
    #[must_use]
    pub fn of_system(&self) -> bool {
        self.home.is_none()
    }

    /// Whether it was registered from a machine of a person: an environment, a provider, a profile
    /// or an MCP server. It keeps that machine's owner as its first owner for as long as it is
    /// registered there, and is unregistered when their membership ends. A host that somebody
    /// enrolled is not one: it is infrastructure its owners pass on.
    #[must_use]
    pub fn registered_from_a_machine(&self) -> bool {
        self.machine_owner.is_some()
            && matches!(
                self.kind,
                ResourceKind::Provider
                    | ResourceKind::Environment
                    | ResourceKind::Agent
                    | ResourceKind::McpServer
            )
    }
}

/// Reads the resources `ids` into `graph`, with the nodes their homes lie among: two statements
/// however many resources there are. An identifier that is no resource is not among them.
pub async fn read_into(
    conn: &mut Conn,
    graph: &mut Graph,
    ids: &[Id],
) -> Result<Vec<Resource>, Error> {
    let found = rows::resource_facts(conn, ids).await?;
    absorb(conn, graph, found).await
}

async fn absorb(
    conn: &mut Conn,
    graph: &mut Graph,
    found: Vec<AccessFacts>,
) -> Result<Vec<Resource>, Error> {
    let homes: Vec<Id> = found.iter().filter_map(|row| row.home_id).collect();
    graph.extend(conn, &homes).await?;
    found.into_iter().map(Resource::from_row).collect()
}

/// The site of a resource: where its page stands. Nothing where its home is not there or was
/// deleted, or a node above it was.
#[must_use]
pub fn site_of(graph: &Graph, resource: &Resource) -> Option<Site> {
    let mut site = Site {
        kind: resource.kind.into(),
        id: resource.id,
        name: resource.name.clone(),
        first_owner: resource.first_owner,
        placement: resource.placement,
        chain: Vec::new(),
        beside: None,
        role_place: None,
        admin_places: Vec::new(),
        orgs: Vec::new(),
        root: None,
        path: None,
        archived_node: None,
        of_system: true,
    };
    let Some(home) = resource.home else {
        return Some(site);
    };
    let place = graph.site_of(home)?;
    // Its page follows its home's defaults and those above, and the admins of the home administer
    // it; a role without a place is held at the home.
    site.chain = std::iter::once(PlaceRef::Node(home))
        .chain(place.chain.iter().copied())
        .collect();
    site.admin_places = std::iter::once(home)
        .chain(place.admin_places.iter().copied())
        .collect();
    site.role_place = Some(home);
    site.orgs = place.orgs;
    site.root = place.root;
    site.of_system = false;
    Some(site)
}

/// The resource `object`, with the nodes its home lies among and its site.
pub async fn site(conn: &mut Conn, object: ObjectRef) -> Result<(Graph, Resource, Site), Error> {
    let mut graph = Graph::default();
    let found = read_into(conn, &mut graph, &[object.id]).await?;
    let resource = found
        .into_iter()
        .find(|resource| resource.object() == object)
        .ok_or(Error::NotFound(object.id))?;
    let site = site_of(&graph, &resource).ok_or(Error::NotFound(object.id))?;
    Ok((graph, resource, site))
}

/// What the caller lacks on one site, for an access to be refused.
#[derive(Debug, Clone)]
pub(crate) struct Lacking {
    /// The thing.
    pub site: Site,
    /// The permissions of its page that are lacking.
    pub permissions: Vec<Permission>,
    /// For the use of a provider: the least that would do.
    pub options: Option<MissingOptions>,
    /// Why nobody can give it, where it is the subscription kept to its owner.
    pub closed: Option<Closed>,
}

/// The refusal of an act that needs `required` on the thing `anchor`, for which the caller lacks
/// what `lacking` lists, each thing with who can give it. A resource is named, and its first owner
/// given, only to a caller who holds something on it or can see the anchor: the thread that
/// brought them to it.
pub(crate) async fn assemble(
    conn: &mut Conn,
    principal: &Principal,
    policy: Policy,
    graph: Graph,
    anchor: &Site,
    lacking: &[Lacking],
    required: Permission,
) -> Result<Refusal, Error> {
    let mut sites: Vec<Site> = vec![anchor.clone()];
    for item in lacking {
        if !sites.iter().any(|site| site.id == item.site.id) {
            sites.push(item.site.clone());
        }
    }
    let everything = every_permission(sites.iter().map(|site| site.kind));
    let frame = Frame::load(conn, principal, policy, graph, &sites, &everything).await?;
    let sees_anchor = frame.holds_any(anchor);
    let mut missing: Vec<orch_access_contract::decisions::MissingAccess> = Vec::new();
    for item in lacking {
        let known = sees_anchor || frame.holds_any(&item.site);
        if item.closed == Some(Closed::SubscriptionNotShared) {
            let object = orch_access_contract::decisions::MissingObject {
                kind: item.site.kind,
                id: item.site.id,
                name: known.then(|| item.site.name.clone()),
                owner: if known { item.site.first_owner } else { None },
            };
            missing.push(
                orch_access_contract::decisions::MissingAccess::kept_to_its_owner(
                    object,
                    item.options.clone(),
                ),
            );
            continue;
        }
        let mut entries =
            missing::missing_access(conn, &frame, &item.site, &item.permissions, known).await?;
        if let Some(options) = &item.options {
            for entry in &mut entries {
                if entry.permissions.contains(&Permission::ProviderUse) {
                    entry.options = Some(options.clone());
                }
            }
        }
        missing.extend(entries);
    }
    Ok(Refusal::of(
        required,
        ObjectRef {
            kind: anchor.kind,
            id: anchor.id,
        },
        missing,
    ))
}

/// Whether the caller holds every one of `permissions` on the resource `object` now, and if not,
/// everything they lack and who can give it.
///
/// A caller who is suspended where the resource is gets [`Error::Suspended`]. A token that does
/// not carry a permission, or reaches no resource that goes through no node, is refused with
/// nobody to ask. A disabled or deleted resource is decided like any other: its page remains.
pub async fn check_all(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    permissions: &[Permission],
    policy: Policy,
) -> Result<Decision, Error> {
    if let Some(unknown) = permissions
        .iter()
        .find(|permission| !permission.exists_on(object.kind))
    {
        return Err(Error::NoSuchPermission {
            name: unknown.as_str().to_owned(),
            kind: object.kind,
        });
    }
    let (graph, _, site) = site(conn, object).await?;
    let need = Need {
        object,
        permissions: permissions.to_vec(),
        options: None,
    };
    decide(
        conn,
        principal,
        graph,
        &[(site.clone(), need)],
        &site,
        policy,
        None,
    )
    .await
}

/// [`check_all`] for one permission.
pub async fn check(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    permission: Permission,
    policy: Policy,
) -> Result<Decision, Error> {
    check_all(conn, principal, object, &[permission], policy).await
}

/// Uses the physical machine's resource page, or its host facade when that role is offered.
/// View, Use and Manage have the same supplier for registration, moves and policy editing.
pub async fn check_machine(
    conn: &mut Conn,
    principal: &Principal,
    machine: Id,
    permission: Permission,
    policy: Policy,
) -> Result<Decision, Error> {
    let (object, permission) = match rows::hosting::host_of_machine(conn, machine).await? {
        Some(host) => (
            ObjectRef {
                kind: ObjectKind::Host,
                id: host,
            },
            match permission {
                Permission::MachineView => Permission::HostView,
                Permission::MachineUse => Permission::HostUse,
                Permission::MachineManage => Permission::HostManage,
                other => other,
            },
        ),
        None => (
            ObjectRef {
                kind: ObjectKind::Machine,
                id: machine,
            },
            permission,
        ),
    };
    check(conn, principal, object, permission, policy).await
}

/// What an act needs of one thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Need {
    /// The thing: a node or a resource.
    pub object: ObjectRef,
    /// What the act needs on it.
    pub permissions: Vec<Permission>,
    /// For the use of a provider, the least that would do; a refusal carries it.
    pub options: Option<MissingOptions>,
}

/// An act that needs permissions on several things at once. Creating a thread needs `topic.interact`
/// on its topic, the use of its provider and its profile, and the management of its environment; it
/// is one act, and a refusal names everything the caller lacks for it, so that one request can ask
/// for all of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needs {
    /// What the act needs on the thing it is asked of, and so what a refusal says is required.
    pub required: Permission,
    /// The thing it is asked of: a refusal names the others only to somebody who can see this one.
    pub anchor: ObjectRef,
    /// The node through which the resources among the things are reached, where there is one: a
    /// token that is kept to a subtree reaches a resource only through a node in it.
    pub through: Option<Id>,
    /// What it needs, thing by thing; the anchor among them.
    pub items: Vec<Need>,
}

/// Whether the caller holds all that `needs` lists, now, for an act that is not a deletion or an
/// unarchiving; and if not, everything they lack, thing by thing, and who can give it.
///
/// A caller who is suspended where any of the things is gets [`Error::Suspended`]. One who holds all
/// of it but finds a node among the things archived gets [`Error::Archived`], after the test of
/// what they hold, so that nobody learns of an archive who would be refused in any case.
pub async fn needs(
    conn: &mut Conn,
    principal: &Principal,
    needs: &Needs,
    policy: Policy,
) -> Result<Decision, Error> {
    let mut graph = Graph::default();
    let mut nodes: Vec<Id> = Vec::new();
    let mut resource_ids: Vec<Id> = Vec::new();
    for item in &needs.items {
        if item.object.kind.node().is_some() {
            nodes.push(item.object.id);
        } else {
            resource_ids.push(item.object.id);
        }
    }
    nodes.extend(needs.through);
    graph.extend(conn, &nodes).await?;
    let found = read_into(conn, &mut graph, &resource_ids).await?;
    let through_path = needs
        .through
        .and_then(|node| graph.get(node))
        .map(|node| node.path.clone());
    let mut sites: Vec<(Site, Need)> = Vec::new();
    for item in &needs.items {
        let site = if item.object.kind.node().is_some() {
            graph.site_of(item.object.id)
        } else {
            found
                .iter()
                .find(|resource| resource.object() == item.object)
                .and_then(|resource| site_of(&graph, resource))
                .map(|mut site| {
                    site.path.clone_from(&through_path);
                    site
                })
        }
        .ok_or(Error::NotFound(item.object.id))?;
        sites.push((site, item.clone()));
    }
    let anchor = sites
        .iter()
        .find(|(site, _)| site.id == needs.anchor.id)
        .map(|(site, _)| site.clone())
        .ok_or(Error::NotFound(needs.anchor.id))?;
    decide(
        conn,
        principal,
        graph,
        &sites,
        &anchor,
        policy,
        Some(needs.required),
    )
    .await
}

async fn decide(
    conn: &mut Conn,
    principal: &Principal,
    graph: Graph,
    items: &[(Site, Need)],
    anchor: &Site,
    policy: Policy,
    required: Option<Permission>,
) -> Result<Decision, Error> {
    for (site, _) in items {
        if let Some(closing) = principal.suspension().closing(&site.orgs) {
            return Err(Error::Suspended { org: closing.org() });
        }
    }
    let Some(first) = items
        .iter()
        .find_map(|(_, need)| need.permissions.first().copied())
    else {
        return Ok(Decision::Allowed {
            through: Vec::new(),
        });
    };
    let required = required.unwrap_or(first);
    let caller = principal.caller();
    let all_sites: Vec<Site> = items.iter().map(|(site, _)| site.clone()).collect();
    let wanted: Vec<Permission> = {
        let mut wanted: Vec<Permission> = Vec::new();
        for (_, need) in items {
            for permission in &need.permissions {
                if !wanted.contains(permission) {
                    wanted.push(*permission);
                }
            }
        }
        wanted
    };
    let anchor_object = ObjectRef {
        kind: anchor.kind,
        id: anchor.id,
    };
    if items
        .iter()
        .any(|(site, _)| !caller.reaches(site.path.as_deref()))
    {
        return Ok(Decision::Refused(Refusal::of(
            required,
            anchor_object,
            Vec::new(),
        )));
    }
    let frame = Frame::load(conn, principal, policy, graph, &all_sites, &wanted).await?;
    let mut through: Vec<Through> = Vec::new();
    let mut lacking: Vec<Lacking> = Vec::new();
    let mut unscoped = false;
    for (site, need) in items {
        let mut missing: Vec<Permission> = Vec::new();
        for permission in &need.permissions {
            let ways = frame.ways(site, *permission);
            if ways.is_empty() {
                missing.push(*permission);
                unscoped |= !caller.permits(*permission);
            }
            through.extend(ways);
        }
        if !missing.is_empty() {
            lacking.push(Lacking {
                site: site.clone(),
                permissions: missing,
                options: need.options.clone(),
                closed: None,
            });
        }
    }
    if !lacking.is_empty() {
        if unscoped {
            // What a token's scopes leave out is not for anybody to give.
            return Ok(Decision::Refused(Refusal::of(
                required,
                anchor_object,
                Vec::new(),
            )));
        }
        let graph = frame.graph().clone();
        let refusal = assemble(conn, principal, policy, graph, anchor, &lacking, required).await?;
        return Ok(Decision::Refused(refusal));
    }
    for (site, need) in items {
        if let Some(blocked) = site.archived_node
            && need
                .permissions
                .iter()
                .any(|permission| !archive_allows(*permission, Act::Use))
        {
            return Err(Error::Archived { node: blocked });
        }
    }
    Ok(Decision::Allowed { through })
}

/// What a person holds on a resource, and through what.
#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
    /// The actual administration source, including when View is ordinary.
    pub admin_place: Option<PlaceRef>,
    /// Held permissions which the ordinary frame does not give.
    pub admin_only: Vec<Permission>,
    /// The administrative scope used to open a private resource, if any.
    pub admin_access: Option<roles::AdminReach>,
    /// A personal/group assignment here, independent of View and native reach.
    pub participating: bool,
    /// The resource.
    pub object: ObjectRef,
    /// The role they have at its home, and where it comes from.
    pub role: Option<roles::Held>,
    /// Whether they administer its home: hold Admin there or above it.
    pub administers: bool,
    /// Whether they are one of its owners, and count as one.
    pub owner: bool,
    /// What they hold, each with how, in the order of the catalogue, cut down to what a token
    /// carries.
    pub held: Vec<Held>,
}

/// What the person `principal` holds on the resource `object`.
pub async fn effective(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    policy: Policy,
) -> Result<Effective, Error> {
    let (graph, _, site) = site(conn, object).await?;
    let everything = every_permission([site.kind]);
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        &everything,
    )
    .await?;
    Ok(Effective {
        admin_place: frame.admin_place(&site),
        admin_only: frame.admin_only(&site),
        admin_access: frame.admin_access(&site),
        participating: frame.participates(&site),
        object,
        role: frame.role_for(&site),
        administers: frame.administers(&site),
        owner: frame.owns(&site),
        held: frame.effective(&site),
    })
}

/// Exact resource holdings in one frame, without truncating them to a page of the global resource list.
pub async fn effective_among(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
) -> Result<Vec<Effective>, Error> {
    let mut graph = Graph::default();
    let resources = read_into(conn, &mut graph, ids).await?;
    let sites: Vec<Site> = resources
        .iter()
        .filter(|resource| !resource.deleted)
        .filter_map(|resource| site_of(&graph, resource))
        .collect();
    let wanted = every_permission(sites.iter().map(|site| site.kind));
    let frame = Frame::load(conn, principal, policy, graph, &sites, &wanted).await?;
    Ok(sites
        .iter()
        .map(|site| {
            let active = !principal.suspension().applies(&site.orgs);
            Effective {
                admin_place: active.then(|| frame.admin_place(site)).flatten(),
                admin_only: if active {
                    frame.admin_only(site)
                } else {
                    Vec::new()
                },
                admin_access: active.then(|| frame.admin_access(site)).flatten(),
                participating: active && frame.participates(site),
                object: ObjectRef {
                    kind: site.kind,
                    id: site.id,
                },
                role: frame.role_for(site),
                administers: active && frame.administers(site),
                owner: active && frame.owns(site),
                held: if active {
                    frame.effective(site)
                } else {
                    Vec::new()
                },
            }
        })
        .collect())
}

/// A page of a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing<T> {
    /// The items.
    pub items: Vec<T>,
    /// Where the next page begins: the identifier of the last item of this one. None when there
    /// are no more.
    pub next: Option<Id>,
}

/// A resource that a list shows to the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    /// The actual administration source, including when View is ordinary.
    pub admin_place: Option<PlaceRef>,
    /// Held permissions which the ordinary frame does not give.
    pub admin_only: Vec<Permission>,
    /// The administrative scope used to open a private resource, if any.
    pub admin_access: Option<roles::AdminReach>,
    /// A personal/group assignment here, independent of View and native reach.
    pub participating: bool,
    /// The resource.
    pub resource: Resource,
    /// Whether the caller is one of its owners, and counts as one.
    pub owner: bool,
    /// Whether the caller administers its home.
    pub administers: bool,
    /// What the caller holds on it, and how.
    pub held: Vec<Held>,
}

/// The kinds of resource that have a level between looking and managing, and so can be used.
#[must_use]
pub fn use_permission(kind: ResourceKind) -> Option<Permission> {
    match kind {
        ResourceKind::Provider => Some(Permission::ProviderUse),
        ResourceKind::Agent => Some(Permission::AgentUse),
        ResourceKind::Host => Some(Permission::HostUse),
        ResourceKind::Machine => Some(Permission::MachineUse),
        ResourceKind::Template => Some(Permission::TemplateUse),
        ResourceKind::McpServer => Some(Permission::McpUse),
        ResourceKind::Mount => Some(Permission::MountUse),
        ResourceKind::Skill => Some(Permission::SkillUse),
        ResourceKind::Environment => None,
    }
}

/// The candidates of a list of resources of a kind: those the caller might hold something on,
/// paged by identifier.
pub(crate) async fn candidates(
    conn: &mut Conn,
    principal: &Principal,
    kind: ResourceKind,
    after: Option<Id>,
) -> Result<Vec<AccessFacts>, Error> {
    let roots = principal.memberships().roots();
    let looker = Looker {
        user: principal.user(),
        groups: principal.groups(),
        roots: &roots,
        member_of: principal.member_of(),
        system_admin: principal.is_system_admin(),
        now: principal.now().get(),
    };
    Ok(rows::candidate_resources(conn, kind.as_str(), &looker, after, CANDIDATE_BATCH).await?)
}

/// Reads one batch of candidates into their sites and a frame that has read what a decision about
/// every permission of their kind needs: the same statements for one resource and for a hundred.
pub(crate) struct Batch<'p> {
    /// The frame.
    pub frame: Frame<'p>,
    /// The resources, each with its site; one whose home is gone is not here.
    pub resources: Vec<(Resource, Site)>,
    /// The last candidate that was read, where a next batch begins.
    pub last: Option<Id>,
    /// Whether this batch was the last: fewer candidates came than were asked for.
    pub exhausted: bool,
}

impl<'p> Batch<'p> {
    pub(crate) async fn read(
        conn: &mut Conn,
        principal: &'p Principal,
        kind: ResourceKind,
        after: Option<Id>,
        policy: Policy,
    ) -> Result<Batch<'p>, Error> {
        let found = candidates(conn, principal, kind, after).await?;
        let exhausted = i64::try_from(found.len()).is_ok_and(|count| count < CANDIDATE_BATCH);
        let last = found.last().map(|row| row.id);
        let mut graph = Graph::default();
        let all = absorb(conn, &mut graph, found).await?;
        let resources: Vec<(Resource, Site)> = all
            .into_iter()
            .filter_map(|resource| site_of(&graph, &resource).map(|site| (resource, site)))
            .collect();
        let sites: Vec<Site> = resources.iter().map(|(_, site)| site.clone()).collect();
        let everything = every_permission([ObjectKind::from(kind)]);
        let frame = Frame::load(conn, principal, policy, graph, &sites, &everything).await?;
        Ok(Batch {
            frame,
            resources,
            last,
            exhausted,
        })
    }
}

/// The resources of the kind `kind` that the caller holds anything on, which is what a list shows
/// them, one page of them in the order of their identifiers. A resource that is disabled is
/// listed, for whoever manages it brings it back; one that is deleted is not. The statements
/// are those of one batch of candidates however many there are, and another batch is read only
/// while the page is not full.
pub async fn visible(
    conn: &mut Conn,
    principal: &Principal,
    kind: ResourceKind,
    page: Page,
    policy: Policy,
) -> Result<Listing<Seen>, Error> {
    let Page { after, limit } = page;
    let mut items: Vec<Seen> = Vec::new();
    let mut cursor = after;
    while items.len() <= limit {
        let batch = Batch::read(conn, principal, kind, cursor, policy).await?;
        for (resource, site) in &batch.resources {
            if principal.suspension().applies(&site.orgs) {
                continue;
            }
            let held = batch.frame.effective(site);
            if held.is_empty() {
                continue;
            }
            items.push(Seen {
                admin_place: batch.frame.admin_place(site),
                admin_only: batch.frame.admin_only(site),
                admin_access: batch.frame.admin_access(site),
                participating: batch.frame.participates(site),
                resource: resource.clone(),
                owner: batch.frame.owns(site),
                administers: batch.frame.administers(site),
                held,
            });
        }
        cursor = batch.last;
        if batch.exhausted || batch.last.is_none() {
            break;
        }
    }
    let more = items.len() > limit;
    items.truncate(limit);
    Ok(Listing {
        next: if more {
            items.last().map(|item| item.resource.id)
        } else {
            None
        },
        items,
    })
}

/// Which of the resources `ids` the caller holds `permission` on, in the order asked: those that
/// are not there, or are closed to the caller, are left out. The statements are the same for any
/// number of them.
pub async fn holding(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    permission: Permission,
    policy: Policy,
) -> Result<Vec<Resource>, Error> {
    let mut graph = Graph::default();
    let found = read_into(conn, &mut graph, ids).await?;
    let mut sites: Vec<(Resource, Site)> = Vec::new();
    for id in ids {
        if let Some(resource) = found.iter().find(|resource| resource.id == *id)
            && permission.exists_on(resource.kind.into())
            && !sites.iter().any(|(seen, _)| seen.id == resource.id)
            && let Some(site) = site_of(&graph, resource)
        {
            sites.push((resource.clone(), site));
        }
    }
    let all: Vec<Site> = sites.iter().map(|(_, site)| site.clone()).collect();
    let frame = Frame::load(conn, principal, policy, graph, &all, &[permission]).await?;
    Ok(sites
        .into_iter()
        .filter(|(_, site)| {
            !principal.suspension().applies(&site.orgs) && !frame.ways(site, permission).is_empty()
        })
        .map(|(resource, _)| resource)
        .collect())
}

// ---- Addresses -----------------------------------------------------------------------------------

/// The addresses of the resources `ids`, in the order asked: the short names of the orgs from the
/// root org down to the home, the username of the first owner, and the name of the resource, joined
/// by dots (`acme.research.ml.chen.gpu`); a resource of the system is `system.<name>`. It names
/// where a resource comes from, wherever it is listed or offered outside its home and wherever a
/// thread of another place runs on it, so that nobody mistakes whose thing it is; the identifier
/// identifies. Environment segments use the shared short-name expression; queries remain batched.
pub async fn addresses(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, String)>, Error> {
    let mut graph = Graph::default();
    let found = read_into(conn, &mut graph, ids).await?;
    let orgs_of = |resource: &Resource| -> Vec<Id> {
        resource
            .home
            .and_then(|home| graph.get(home))
            .map(|home| {
                home.path
                    .iter()
                    .filter(|id| {
                        graph
                            .get(**id)
                            .is_some_and(|node| node.kind == roles::NodeKind::Org)
                    })
                    .copied()
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut orgs: Vec<Id> = found.iter().flat_map(orgs_of).collect();
    orgs.sort_unstable();
    orgs.dedup();
    let mut owners: Vec<Id> = found
        .iter()
        .filter_map(|resource| resource.first_owner)
        .collect();
    owners.sort_unstable();
    owners.dedup();
    let slugs: std::collections::HashMap<Id, String> =
        orch_db::queries::access::org_slugs(conn, &orgs)
            .await?
            .into_iter()
            .collect();
    let names: std::collections::HashMap<Id, String> =
        orch_db::queries::access::usernames(conn, &owners)
            .await?
            .into_iter()
            .collect();
    let environments: Vec<Id> = found
        .iter()
        .filter(|resource| resource.kind == ResourceKind::Environment)
        .map(|resource| resource.id)
        .collect();
    let segments: std::collections::HashMap<Id, String> =
        orch_db::queries::addresses::environment_segments(conn, &environments)
            .await?
            .into_iter()
            .collect();
    Ok(ids
        .iter()
        .filter_map(|id| found.iter().find(|resource| resource.id == *id))
        .map(|resource| {
            let segment = segments
                .get(&resource.id)
                .map_or(resource.name.as_str(), String::as_str);
            let address = if resource.of_system() {
                format!("system.{segment}")
            } else {
                let mut parts: Vec<&str> = orgs_of(resource)
                    .iter()
                    .filter_map(|org| slugs.get(org).map(String::as_str))
                    .collect();
                if let Some(owner) = resource.first_owner.and_then(|owner| names.get(&owner)) {
                    parts.push(owner);
                }
                parts.push(segment);
                parts.join(".")
            };
            (resource.id, address)
        })
        .collect())
}

// ---- Where a mount's data may be ----------------------------------------------------------------

/// What kind of place a mount's data is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationKind {
    /// A directory that exists already.
    Path,
    /// A volume that Orch creates there, which exposes nothing of the machine.
    Volume,
}

/// A location to add to a mount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationAsk {
    /// The mount.
    pub mount: Id,
    /// The machine that is to have the data: a host's, or a person's own.
    pub machine: Id,
    /// A path or a volume.
    pub kind: LocationKind,
    /// For a volume, the host's storage it is created on; none: the host's default.
    pub storage: Option<String>,
}

/// Whether a location may be added.
#[derive(Debug, Clone, PartialEq)]
pub enum Locating {
    /// It may.
    Allowed,
    /// The caller lacks something: what, thing by thing, and who can give it.
    Refused(Box<Refusal>),
    /// A path on a person's own machine is added by that machine's owner alone.
    NotTheMachinesOwner,
    /// A volume is created on a host, and a person's own machine has no storage for one.
    NoVolumeThere,
    /// They may use the host, and their entry does not let volumes be created on that storage.
    StorageNotAllowed {
        /// The storage.
        storage: String,
    },
    /// There is no such machine.
    NoSuchMachine,
}

fn located(decision: Decision) -> Locating {
    match decision {
        Decision::Allowed { .. } => Locating::Allowed,
        Decision::Refused(refusal) => Locating::Refused(Box::new(refusal)),
    }
}

/// Location changes use the same machine-management authority as its policy. The mount must be
/// usable; paths are bounded by environment.roots, and data-making support comes from facts.
pub async fn may_add_location(
    conn: &mut Conn,
    principal: &Principal,
    ask: &LocationAsk,
    _policies: &crate::usable::Policies,
) -> Result<Locating, Error> {
    match may_change_policy_key(
        conn,
        principal,
        ask.machine,
        KeyAbout::TheMachine,
        true,
        Policy::default(),
    )
    .await?
    {
        Keying::Allowed => {}
        Keying::Refused(refusal) => return Ok(Locating::Refused(refusal)),
        Keying::NoSuchMachine => return Ok(Locating::NoSuchMachine),
        _ => return Ok(Locating::NotTheMachinesOwner),
    }
    if ask.kind == LocationKind::Volume
        && orch_db::queries::resources::mounts::data_driver(conn, ask.machine)
            .await?
            .is_none()
    {
        return Ok(Locating::NoVolumeThere);
    }
    Ok(located(
        check(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Mount,
                id: ask.mount,
            },
            Permission::MountUse,
            Policy::default(),
        )
        .await?,
    ))
}

// ---- A machine's local policy ----------------------------------------------------------------------

/// What a key of a machine's local policy bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAbout {
    /// The environments registered from the machine.
    Environments,
    /// The providers registered from it.
    Providers,
    /// The machine itself.
    TheMachine,
    /// Which keys are handed over to Orch at all.
    TheHandingOver,
}

/// Whether a key of a machine's local policy may be changed from Orch.
#[derive(Debug, Clone, PartialEq)]
pub enum Keying {
    /// It may.
    Allowed,
    /// The caller does not manage what the key bounds.
    Refused(Box<Refusal>),
    /// The key is one its owner kept: it is changed on the machine.
    NotDelegated,
    /// Which keys are handed over is changed on the machine and nowhere else.
    OnTheMachineOnly,
    /// A key that bounds the machine itself stays its owner's; so does one about things nobody
    /// has registered from it.
    NotTheMachinesOwner,
    /// There is no such machine.
    NoSuchMachine,
}

/// Whether the caller may change a key of the local policy of `machine` from Orch. The key must be
/// one its owner handed over (`delegated`, which is a fact of the machine that the caller reads);
/// then it is changed by whoever manages what it bounds: `environment.manage` on every environment
/// registered from the machine for a key about environments, `provider.manage` on every provider
/// registered from it for a key about providers, the machine's owner for the rest. The machine's own
/// owner may change any key they handed over. The connector checks again on the machine.
pub async fn may_change_policy_key(
    conn: &mut Conn,
    principal: &Principal,
    machine: Id,
    about: KeyAbout,
    delegated: bool,
    policy: Policy,
) -> Result<Keying, Error> {
    if about == KeyAbout::TheHandingOver {
        return Ok(Keying::OnTheMachineOnly);
    }
    if !delegated {
        return Ok(Keying::NotDelegated);
    }
    let Some(_) = orch_db::queries::environments::location_machine(conn, machine).await? else {
        return Ok(Keying::NoSuchMachine);
    };
    if let Some(host) = rows::hosting::host_of_machine(conn, machine).await? {
        let decision = check(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Host,
                id: host,
            },
            Permission::HostManage,
            policy,
        )
        .await?;
        return Ok(match decision {
            Decision::Allowed { .. } => Keying::Allowed,
            Decision::Refused(refusal) => Keying::Refused(Box::new(refusal)),
        });
    }
    let decision = check(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Machine,
            id: machine,
        },
        Permission::MachineManage,
        policy,
    )
    .await?;
    Ok(match decision {
        Decision::Allowed { .. } => Keying::Allowed,
        Decision::Refused(refusal) => Keying::Refused(Box::new(refusal)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::NodeFacts;
    use orch_access_contract::roles::NodeKind;
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

    fn provider(home: Option<Id>) -> Resource {
        Resource {
            kind: ResourceKind::Provider,
            id: new_id(),
            name: "Team Pro".into(),
            first_owner: Some(new_id()),
            home,
            root: None,
            disabled: false,
            deleted: false,
            placement: Placement::default(),
            subscription: None,
            arrangements: Vec::new(),
            machine: None,
            machine_owner: None,
        }
    }

    #[test]
    fn a_resources_page_follows_its_home_and_the_places_above_and_its_admins_are_theirs() {
        let (root, department, project) = (new_id(), new_id(), new_id());
        let graph = Graph::of(vec![
            node(root, NodeKind::Org, &[root]),
            node(department, NodeKind::Org, &[root, department]),
            node(project, NodeKind::Project, &[root, department, project]),
        ]);
        let site = site_of(&graph, &provider(Some(department))).unwrap();
        assert_eq!(
            site.chain,
            [
                PlaceRef::Node(department),
                PlaceRef::Node(root),
                PlaceRef::System
            ]
        );
        assert_eq!(site.admin_places, [department, root]);
        assert_eq!(site.role_place, Some(department));
        assert_eq!(site.orgs, [root, department]);
        assert_eq!(site.root, Some(root));
        assert!(!site.of_system);
        assert_eq!(
            site.path, None,
            "a resource is reached through no node of its own"
        );
        // An environment's home is a node that is not an org.
        let environment = Resource {
            kind: ResourceKind::Environment,
            ..provider(Some(project))
        };
        let site = site_of(&graph, &environment).unwrap();
        assert_eq!(site.chain.first(), Some(&PlaceRef::Node(project)));
        assert_eq!(site.admin_places, [project, department, root]);
        assert_eq!(site.orgs, [root, department], "the orgs it lies in");
    }

    #[test]
    fn a_resource_of_the_system_has_no_home_follows_nothing_and_is_administered_by_the_system() {
        let site = site_of(&Graph::default(), &provider(None)).unwrap();
        assert!(site.chain.is_empty() && site.admin_places.is_empty());
        assert!(site.of_system && site.root.is_none() && site.role_place.is_none());
    }

    #[test]
    fn a_resource_whose_home_was_deleted_has_no_site() {
        let (root, department) = (new_id(), new_id());
        let mut deleted = node(department, NodeKind::Org, &[root, department]);
        deleted.deleted = true;
        let graph = Graph::of(vec![node(root, NodeKind::Org, &[root]), deleted]);
        assert!(site_of(&graph, &provider(Some(department))).is_none());
        assert!(
            site_of(&graph, &provider(Some(new_id()))).is_none(),
            "a home that is not there"
        );
    }

    #[test]
    fn only_the_kinds_that_have_a_middle_level_have_a_use() {
        for kind in ResourceKind::ALL {
            assert_eq!(use_permission(*kind).is_some(), kind.has_use(), "{kind}");
        }
    }
}
