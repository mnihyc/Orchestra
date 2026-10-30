//! Any thing that has a page, found by its kind and identifier: where its page stands, and what a person holds on it, the same for a node, a resource and a document.
//!
//! The requests that are written once for every kind of thing (a thing's
//! history, what a person holds on it, the people behind a holder of its
//! page, a preview of a change, handing it over) ask the engine about a
//! thing without knowing which kind it is. A [`Thing`] is what such a request
//! names, and [`site`] and [`effective`] answer for it through the machinery
//! each kind already has, so that no kind needs a reading of its own.

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_access_contract::roles;
use orch_db::Conn;
use orch_types::Id;

use crate::engine::{Frame, Graph, Site};
use crate::nodes::every_permission;
use crate::pages::Target;
use crate::{Error, Policy, Principal, documents, resources};
use orch_access_contract::decisions::Held;

/// What a request that is the same for every kind of thing names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Thing {
    /// A node, whatever kind of node it is: found by its identifier.
    Node(Id),
    /// A resource or a document, by kind and identifier.
    Other(ObjectRef),
}

impl Thing {
    /// The page this thing's page is read and changed as.
    #[must_use]
    pub const fn target(self) -> Target {
        match self {
            Self::Node(id) => Target::Live(id),
            Self::Other(object) => Target::Resource(object),
        }
    }

    /// Its identifier.
    #[must_use]
    pub const fn id(self) -> Id {
        match self {
            Self::Node(id) => id,
            Self::Other(object) => object.id,
        }
    }
}

/// The thing a node, a resource or a document `object` is, whichever of the
/// kinds of node it is: a node is named by its identifier alone.
#[must_use]
pub fn thing_of(object: ObjectRef) -> Thing {
    if object.kind.node().is_some() {
        Thing::Node(object.id)
    } else {
        Thing::Other(object)
    }
}

/// Where the page of `thing` stands, with the nodes it lies among: not found
/// where there is no such thing, or it was deleted, or the place it lies in was.
pub async fn site(conn: &mut Conn, thing: Thing) -> Result<(Graph, Site), Error> {
    match thing {
        Thing::Node(id) => {
            let graph = Graph::load(conn, &[id]).await?;
            let site = graph.site_of(id).ok_or(Error::NotFound(id))?;
            Ok((graph, site))
        }
        Thing::Other(object) if object.kind.is_document() => {
            documents::site_for_page(conn, object.id).await
        }
        Thing::Other(object) => {
            let (graph, resource, site) = resources::site(conn, object).await?;
            if resource.deleted {
                return Err(Error::NotFound(object.id));
            }
            Ok((graph, site))
        }
    }
}

/// What a person holds on a thing, and through what.
#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
    /// Held permissions absent from this person's ordinary frame.
    pub admin_only: Vec<Permission>,
    /// What kind of thing it is.
    pub kind: ObjectKind,
    /// The role they have where a role of the thing is held, and where it comes from.
    pub role: Option<roles::Held>,
    /// Whether they administer the place it is in.
    pub administers: bool,
    /// Whether they are one of its owners, and count as one.
    pub owner: bool,
    /// The root org of the thing, where it has one.
    pub root: Option<Id>,
    /// What they hold, each with how, in the order of the catalogue, cut down to what archive
    /// leaves while it is archived and to what a token carries.
    pub held: Vec<Held>,
}

/// What the person `principal` holds on `thing`: the same answer for a node, a resource and a
/// document.
pub async fn effective(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    policy: Policy,
) -> Result<Effective, Error> {
    let (graph, site) = site(conn, thing).await?;
    effective_at(conn, principal, graph, &site, policy).await
}

/// [`effective`] for a thing whose site has been read already, with the nodes it lies among.
pub async fn effective_at(
    conn: &mut Conn,
    principal: &Principal,
    graph: Graph,
    site: &Site,
    policy: Policy,
) -> Result<Effective, Error> {
    let everything = every_permission([site.kind]);
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(site),
        &everything,
    )
    .await?;
    Ok(Effective {
        admin_only: frame.admin_only(site),
        kind: site.kind,
        role: frame.role_for(site),
        administers: frame.administers(site),
        owner: frame.owns(site),
        root: site.root,
        held: frame.effective(site),
    })
}
