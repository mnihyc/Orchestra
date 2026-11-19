//! A node's page read upward through the orgs it lies in: who holds what, levels, advanced rows, owners, restricted nodes with redacted ancestors, and archiving by whoever holds the node's Archive row.
//!
//! [`check`] answers whether a caller holds a permission on a node, now,
//! and if not says everything they lack and who can give it. [`holding`]
//! and [`children`] answer the same for many nodes with the same few
//! statements, so that a list costs the same for ten nodes as for a
//! thousand. [`effective`] says what a person holds on a node and through
//! what. [`ancestors`] lists the nodes above one as the caller may see
//! them: by name where they hold anything there, redacted where not.
//!
//! Archive is applied here, per act: a node that is archived, itself or
//! through a node above it, keeps its View permissions, `access.manage`,
//! and what deletes it or brings it back, and holds nothing else.

use orch_access_contract::decisions::{Decision, Held, ObjectRef, Refusal, Through};
use orch_access_contract::permissions::{
    NodePermission, ObjectKind, Permission, PermissionName, WhenArchived,
};
use orch_access_contract::roles::{self, NodeKind};
use orch_db::Conn;
use orch_db::queries::hierarchy;
use orch_types::Id;

use crate::engine::{Frame, Graph, NodeFacts, Site};
use crate::{Act, Error, Policy, Principal, missing};

/// How many children are looked at in one statement when a list is
/// filtered by what the caller may see: enough that a page of mostly
/// visible children takes one round, few enough to keep a frame small.
const CHILD_BATCH: i64 = 100;

/// Whether `permission` is still held by whoever holds it, for `act`, on a
/// node that is archived.
#[must_use]
pub fn archive_allows(permission: Permission, act: Act) -> bool {
    match permission.when_archived() {
        WhenArchived::Holds => true,
        WhenArchived::HoldsToDeleteOrUnarchive => matches!(act, Act::Delete | Act::Unarchive),
        WhenArchived::HoldsToUnarchive => matches!(act, Act::Unarchive),
        WhenArchived::Withheld => false,
    }
}

/// The permission that the name `name` means on a node of kind `kind`.
pub fn resolve(kind: NodeKind, name: PermissionName) -> Result<Permission, Error> {
    name.on(ObjectKind::from(kind))
        .ok_or_else(|| Error::NoSuchPermission {
            name: name.as_str().to_owned(),
            kind: ObjectKind::from(kind),
        })
}

/// The permissions a frame needs to have read to say what a person holds on
/// a node of kind `kind`: all of them.
pub(crate) fn every_permission(kinds: impl IntoIterator<Item = ObjectKind>) -> Vec<Permission> {
    let mut all: Vec<Permission> = Vec::new();
    for kind in kinds {
        for permission in kind.permissions() {
            if !all.contains(&permission) {
                all.push(permission);
            }
        }
    }
    all
}

/// Whether the caller holds `permission` on the site for `act`: they hold
/// it by what their pages and roles give them, within what their token
/// carries and reaches, and the site's archive leaves it for the act.
fn holds_for_act(frame: &Frame<'_>, site: &Site, permission: Permission, act: Act) -> bool {
    !frame.ways(site, permission).is_empty()
        && (!site.is_archived() || archive_allows(permission, act))
}

/// Whether the caller holds every one of `permissions` on the node `node`
/// now, for `act`; and if not, everything they lack and who can give it.
///
/// A caller who is suspended where the node is gets [`Error::Suspended`],
/// naming the org. One who would hold what the act needs but finds the node
/// archived gets [`Error::Archived`], after the test of what they hold, so
/// that nobody learns of an archive who would be refused in any case. A
/// token that does not carry a permission, or does not reach the node, is
/// refused with nobody to ask.
pub async fn check_all(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    permissions: &[Permission],
    act: Act,
    policy: Policy,
) -> Result<Decision, Error> {
    let graph = Graph::load(conn, &[node]).await?;
    decide(conn, principal, graph, node, permissions, act, policy).await
}

/// [`check_all`] for one permission.
pub async fn check(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    permission: Permission,
    act: Act,
    policy: Policy,
) -> Result<Decision, Error> {
    check_all(conn, principal, node, &[permission], act, policy).await
}

/// [`check`] for a permission as a request names it: `node.view` and its
/// kin mean the permission of the node's own area, by its kind.
pub async fn check_named(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    name: PermissionName,
    act: Act,
    policy: Policy,
) -> Result<Decision, Error> {
    let graph = Graph::load(conn, &[node]).await?;
    let kind = graph.get(node).ok_or(Error::NotFound(node))?.kind;
    let permission = resolve(kind, name)?;
    decide(conn, principal, graph, node, &[permission], act, policy).await
}

async fn decide(
    conn: &mut Conn,
    principal: &Principal,
    graph: Graph,
    node: Id,
    permissions: &[Permission],
    act: Act,
    policy: Policy,
) -> Result<Decision, Error> {
    let site = graph.site_of(node).ok_or(Error::NotFound(node))?;
    if let Some(closing) = principal.suspension().closing(&site.orgs) {
        return Err(Error::Suspended { org: closing.org() });
    }
    let Some(first) = permissions.first().copied() else {
        return Ok(Decision::Allowed {
            through: Vec::new(),
        });
    };
    let object = ObjectRef {
        kind: site.kind,
        id: site.id,
    };
    let caller = principal.caller();
    if !caller.reaches(site.path.as_deref()) {
        return Ok(Decision::Refused(Refusal::of(first, object, Vec::new())));
    }
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        permissions,
    )
    .await?;
    let mut through: Vec<Through> = Vec::new();
    let mut lacking: Vec<Permission> = Vec::new();
    for permission in permissions {
        let ways = frame.ways(&site, *permission);
        if ways.is_empty() {
            lacking.push(*permission);
        }
        through.extend(ways);
    }
    if let Some(required) = lacking.first().copied() {
        if lacking
            .iter()
            .any(|permission| !caller.permits(*permission))
        {
            return Ok(Decision::Refused(Refusal::of(required, object, Vec::new())));
        }
        let graph = frame.graph().clone();
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
        let refusal = missing::refusal(conn, &frame, &site, required, &lacking).await?;
        return Ok(Decision::Refused(refusal));
    }
    if let Some(blocked) = site.archived_node
        && permissions
            .iter()
            .any(|permission| !archive_allows(*permission, act))
    {
        return Err(Error::Archived { node: blocked });
    }
    Ok(Decision::Allowed { through })
}

/// A node that a list shows to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// Native reach alone gives View, from the same frame as a full page.
    pub admin_access: Option<orch_access_contract::roles::AdminReach>,
    /// An ordinary assignment here or below, or a follow; independent of View.
    pub participating: bool,
    /// The node.
    pub id: Id,
    /// What kind of node it is.
    pub kind: NodeKind,
    /// Its name: the caller holds something there, which shows it.
    pub name: String,
    /// Whether its View permission is cut loose below its root org: what
    /// other products call private.
    pub restricted: bool,
    /// Whether it is archived, itself or through a node above it.
    pub archived: bool,
}

fn listed_for(frame: &Frame<'_>, site: &Site, name: PermissionName, act: Act) -> Option<Listed> {
    let kind = site.kind.node()?;
    let permission = name.on(site.kind)?;
    let principal = frame.principal();
    if principal.suspension().applies(&site.orgs)
        || !principal.caller().reaches(site.path.as_deref())
        || !holds_for_act(frame, site, permission, act)
    {
        return None;
    }
    Some(Listed {
        admin_access: frame.admin_access_for(site, permission),
        participating: frame.participates(site),
        id: site.id,
        kind,
        name: site.name.clone(),
        restricted: frame.restricted(site),
        archived: site.is_archived(),
    })
}

fn permissions_for(sites: &[Site], name: PermissionName) -> Vec<Permission> {
    let mut wanted: Vec<Permission> = Vec::new();
    for site in sites {
        let own = name.on(site.kind);
        let view = site
            .kind
            .node()
            .and_then(|kind| NodePermission::View.on(kind));
        for permission in own.into_iter().chain(view) {
            if !wanted.contains(&permission) {
                wanted.push(permission);
            }
        }
    }
    wanted
}

/// Which of the nodes `ids` the caller holds the permission `name` on, for
/// `act`, in the order they were asked: nodes that are not there, are
/// deleted, or are closed to the caller are left out. The statements are
/// the same for any number of nodes.
pub async fn holding(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    name: PermissionName,
    act: Act,
    policy: Policy,
) -> Result<Vec<Listed>, Error> {
    let graph = Graph::load(conn, ids).await?;
    let mut sites: Vec<Site> = Vec::new();
    for id in ids {
        if let Some(site) = graph.site_of(*id)
            && !sites.iter().any(|seen| seen.id == site.id)
        {
            sites.push(site);
        }
    }
    let wanted = permissions_for(&sites, name);
    let frame = Frame::load(conn, principal, policy, graph, &sites, &wanted).await?;
    Ok(sites
        .iter()
        .filter_map(|site| listed_for(&frame, site, name, act))
        .collect())
}

/// Where a list of children begins and how long it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// The child after which it begins; none for the first page.
    pub after: Option<Id>,
    /// The most children a page holds.
    pub limit: usize,
}

/// A page of the children of a node that the caller holds a permission on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Children {
    /// The children the caller holds the permission on, in the order they
    /// were made.
    pub items: Vec<Listed>,
    /// Where the next page begins: the last child of this one. None when
    /// there are no more.
    pub next: Option<Id>,
}

/// The children of `parent` that the caller holds the permission `name` on,
/// in the order they were made, one page of them. A child the caller may not see is not there, and does not count
/// toward the limit. The parent itself is not looked at: the caller of this
/// asks for the right to see it first.
pub async fn children(
    conn: &mut Conn,
    principal: &Principal,
    parent: Id,
    name: PermissionName,
    act: Act,
    policy: Policy,
    page: Page,
) -> Result<Children, Error> {
    let Page { after, limit } = page;
    let base = Graph::load(conn, &[parent]).await?;
    if base.site_of(parent).is_none() {
        return Err(Error::NotFound(parent));
    }
    let mut items: Vec<Listed> = Vec::new();
    let mut cursor = after;
    let mut exhausted = false;
    while items.len() <= limit && !exhausted {
        let batch = hierarchy::children_after(conn, parent, cursor, CHILD_BATCH).await?;
        exhausted = i64::try_from(batch.len()).is_ok_and(|count| count < CHILD_BATCH);
        let Some(last) = batch.last().map(|row| row.id) else {
            break;
        };
        cursor = Some(last);
        let mut graph = base.clone();
        graph.add_rows(batch)?;
        let sites: Vec<Site> = graph
            .nodes()
            .filter(|node| node.parent == Some(parent))
            .filter_map(|node| graph.site_of(node.id))
            .collect();
        let mut sites = sites;
        sites.sort_by_key(|site| site.id);
        let wanted = permissions_for(&sites, name);
        let frame = Frame::load(conn, principal, policy, graph, &sites, &wanted).await?;
        items.extend(
            sites
                .iter()
                .filter_map(|site| listed_for(&frame, site, name, act)),
        );
    }
    let more = items.len() > limit;
    items.truncate(limit);
    Ok(Children {
        next: if more {
            items.last().map(|item| item.id)
        } else {
            None
        },
        items,
    })
}

/// A node above another, as the caller may see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ancestor {
    /// The node.
    pub id: Id,
    /// What kind of node it is.
    pub kind: NodeKind,
    /// Its name; none where the caller holds nothing there.
    pub name: Option<String>,
    /// Whether the name is withheld.
    pub redacted: bool,
}

/// The nodes above `node`, root org first: each by name where the caller
/// holds anything on it, or is a member of it when it is an org, and as a
/// redacted reference where not. Naming somebody on a thread whose topic
/// they hold nothing on gives them that thread only, and its ancestors
/// appear to them this way.
pub async fn ancestors(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    policy: Policy,
) -> Result<Vec<Ancestor>, Error> {
    let graph = Graph::load(conn, &[node]).await?;
    let facts = graph.get(node).ok_or(Error::NotFound(node))?.clone();
    if graph.site_of(node).is_none() {
        return Err(Error::NotFound(node));
    }
    let above: Vec<NodeFacts> = facts
        .path
        .iter()
        .filter(|id| **id != node)
        .filter_map(|id| graph.get(*id).cloned())
        .collect();
    let sites: Vec<Site> = above
        .iter()
        .filter_map(|ancestor| graph.site_of(ancestor.id))
        .collect();
    let everything = every_permission(sites.iter().map(|site| site.kind));
    let frame = Frame::load(conn, principal, policy, graph, &sites, &everything).await?;
    Ok(sites
        .iter()
        .map(|site| {
            let known = frame.holds_any(site)
                || (site.kind == ObjectKind::Org
                    && site.root.is_some_and(|root| principal.is_in_tree(root)));
            Ancestor {
                id: site.id,
                kind: site.kind.node().unwrap_or(NodeKind::Org),
                name: known.then(|| site.name.clone()),
                redacted: !known,
            }
        })
        .collect())
}

/// What a person holds on a node, and through what.
#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
    /// The node.
    pub node: Id,
    /// Its kind.
    pub kind: NodeKind,
    /// The role they have there, and where it comes from.
    pub role: Option<roles::Held>,
    /// Whether they administer it: hold Admin at the place it lies in or
    /// above it.
    pub administers: bool,
    /// Whether they are one of its owners.
    pub owner: bool,
    /// Whether its View permission is cut loose below its root org.
    pub restricted: bool,
    /// The nearest archived node on the way up, itself included.
    pub archived_node: Option<Id>,
    /// What they hold, each with how, in the order of the catalogue: cut
    /// down to what archive leaves while it is archived, and to what a
    /// token carries.
    pub held: Vec<Held>,
}

/// What the person `principal` holds on the node `node`.
pub async fn effective(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    policy: Policy,
) -> Result<Effective, Error> {
    let graph = Graph::load(conn, &[node]).await?;
    let site = graph.site_of(node).ok_or(Error::NotFound(node))?;
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
        node,
        kind: site.kind.node().unwrap_or(NodeKind::Org),
        role: frame.role_for(&site),
        administers: frame.administers(&site),
        owner: frame.owns(&site),
        restricted: frame.restricted(&site),
        archived_node: site.archived_node,
        held: frame.effective(&site),
    })
}

/// Whether the node `node` is restricted.
pub async fn is_restricted(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    policy: Policy,
) -> Result<bool, Error> {
    let graph = Graph::load(conn, &[node]).await?;
    let site = graph.site_of(node).ok_or(Error::NotFound(node))?;
    let view = site
        .kind
        .node()
        .and_then(|kind| NodePermission::View.on(kind))
        .into_iter()
        .collect::<Vec<_>>();
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        &view,
    )
    .await?;
    Ok(frame.restricted(&site))
}

/// Which of the nodes `ids` were made private below the place `place`, which lies on the way
/// down to each of them: their View is cut loose on their own page or on the defaults of a place
/// between them and `place`, and so they do not follow it. The same few statements for one node as
/// for a thousand; a node that is not there, or lies in what was deleted, is not among them.
/// Whether a node follows a place depends on its pages and not on who asks; `principal` is only
/// whom the frame is read for.
pub async fn restricted_below(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    place: Id,
    policy: Policy,
) -> Result<std::collections::HashSet<Id>, Error> {
    let graph = Graph::load(conn, ids).await?;
    let sites: Vec<Site> = ids.iter().filter_map(|id| graph.site_of(*id)).collect();
    let mut views: Vec<Permission> = Vec::new();
    for site in &sites {
        if let Some(view) = site
            .kind
            .node()
            .and_then(|kind| NodePermission::View.on(kind))
            && !views.contains(&view)
        {
            views.push(view);
        }
    }
    let frame = Frame::load(conn, principal, policy, graph, &sites, &views).await?;
    Ok(sites
        .iter()
        .filter(|site| frame.restricted_below(site, place))
        .map(|site| site.id)
        .collect())
}
