//! What a role held in a place, or a share of the place with what is in it, reaches: the places, threads and documents inside that still follow the place, and those made private or cut loose throughout, as far as the caller may see them.
//!
//! A role is held in a place and reaches everything inside it through the
//! pages there: a page that follows the defaults of the place is reached,
//! one whose View row was cut loose on the way up is not, whatever role is
//! given. The count says which is which before somebody is added, so that
//! nobody is told "everything in it" of a place that has private corners.
//!
//! A thing inside **follows** the place when its View row (`org.view`,
//! `project.view`, `topic.view`, `thread.view`; for a document `doc.view`) is
//! not cut loose on its own page or on the defaults page of any place between
//! it and the place, a lock deciding the row from above standing for all of
//! it; it is **apart** when it is. The documents written beside the place
//! itself are the place's own, and are apart only when the document was made
//! private. A side thread is no thing of its own and is not counted, nor are
//! the documents beside one. A thing the caller may not see is not counted at
//! all, in neither count.
//!
//! The counting is the engine's own: the same rows and the same chain that
//! decide what a person holds are asked of every thing inside in the same few
//! statements, however many there are.

use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_access_contract::roles::PlaceRef;
use orch_db::Conn;
use orch_db::queries::{documents as document_rows, hierarchy};
use orch_types::Id;

use crate::engine::{Frame, Graph, Site};
use crate::{Error, Policy, Principal, documents};

/// How many places, threads and documents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Orgs, projects (folders among them) and topics.
    pub places: i64,
    /// Threads.
    pub threads: i64,
    /// Documents.
    pub documents: i64,
}

/// What lies inside a place, as far as the caller may see it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Covers {
    /// What follows the place: a role held there, or a share of the place with what is in it,
    /// reaches it.
    pub follows: Counts,
    /// What was made private or cut loose throughout: neither reaches it.
    pub apart: Counts,
}

fn view_of(kind: ObjectKind) -> Option<Permission> {
    match kind {
        ObjectKind::Org => Some(Permission::OrgView),
        ObjectKind::Project => Some(Permission::ProjectView),
        ObjectKind::Topic => Some(Permission::TopicView),
        ObjectKind::Thread => Some(Permission::ThreadView),
        _ => None,
    }
}

fn sees(frame: &Frame<'_>, site: &Site, permission: Permission) -> bool {
    let principal = frame.principal();
    !principal.suspension().applies(&site.orgs)
        && principal.caller().reaches(site.path.as_deref())
        && !frame.ways(site, permission).is_empty()
}

/// What lies inside the place `place`, counted for the caller; none where `place` is not an org, a
/// project or a topic. Seven statements and the few of one frame, whatever lies inside.
pub async fn of_place(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    policy: Policy,
) -> Result<Option<Covers>, Error> {
    let mut graph = Graph::load(conn, &[place]).await?;
    let Some(here) = graph.site_of(place) else {
        return Err(Error::NotFound(place));
    };
    if !here.kind.node().is_some_and(|node| node.place().is_some()) {
        return Ok(None);
    }
    graph.add_rows(hierarchy::descendants(conn, place).await?)?;
    let sides = hierarchy::side_threads_inside(conn, place).await?;
    let ids = document_rows::beside_or_inside(conn, place).await?;
    let made = documents::read_into(conn, &mut graph, &ids).await?;
    let nodes: Vec<Site> = graph
        .nodes()
        .filter(|node| node.id != place && !sides.contains(&node.id))
        .filter(|node| node.path.contains(&place))
        .filter_map(|node| graph.site_of(node.id))
        .collect();
    let mut sites = nodes.clone();
    let mut written: Vec<(Site, bool)> = Vec::new();
    for document in &made {
        if let Some(site) = documents::site_of(&graph, document) {
            written.push((site.clone(), document.node == place));
            sites.push(site);
        }
    }
    let wanted = [
        Permission::OrgView,
        Permission::ProjectView,
        Permission::TopicView,
        Permission::ThreadView,
        Permission::DocView,
    ];
    let frame = Frame::load(conn, principal, policy, graph, &sites, &wanted).await?;
    let here = PlaceRef::Node(place);
    let mut covers = Covers::default();
    for site in &nodes {
        let Some(view) = view_of(site.kind) else {
            continue;
        };
        if !sees(&frame, site, view) {
            continue;
        }
        let counts = if frame.chain(site, view).cut_below(here) {
            &mut covers.apart
        } else {
            &mut covers.follows
        };
        if site.kind == ObjectKind::Thread {
            counts.threads += 1;
        } else {
            counts.places += 1;
        }
    }
    for (site, beside_the_place) in &written {
        if !sees(&frame, site, Permission::DocView) {
            continue;
        }
        // The documents of the place itself follow its own page and are private only by their own
        // row; the others follow the rows of the node they lie beside, and the places up to this one.
        let apart = if *beside_the_place {
            frame
                .store()
                .row(
                    site.page(),
                    orch_access_contract::permissions::Profile::Any,
                    Permission::DocView,
                )
                .is_some_and(|row| !row.inherit)
        } else {
            frame.chain(site, Permission::DocView).cut_below(here)
        };
        if apart {
            covers.apart.documents += 1;
        } else {
            covers.follows.documents += 1;
        }
    }
    Ok(Some(covers))
}
