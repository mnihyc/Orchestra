//! What sharing a thread offers beside it of its environment: looking into it, and for whoever sets the thread up the Manage of its files and its shells; whether the people named have that, giving it in the same step where the giver may, and asking for it where they may not.
//!
//! A thread's page holds no door into its environment: its files, its processes, its shells, its
//! browsers and its displays are given on the environment's own page, for all of it. Somebody named
//! on one thread from elsewhere would read the conversation and see none of the files, so the page
//! that gives the thread shows the environment beside it, says whether the people concerned have it,
//! and offers to give it in the same step. Reading a thread (`thread.view`) goes with the
//! environment's Can view: the environment itself and the View row of each of its four doors; setting
//! it up (`thread.manage`) goes also with the Manage of Files and of Terminal. Never with managing the
//! environment, and never with the Manage of Browsers or of VNC: whoever has an agent there that does
//! all of it is a person the environment's own page names.
//!
//! Each offer is a change of the environment's page, made by the caller with their own rows there:
//! they may give what they hold, up to their own, and where they may not (the page keeps sharing to
//! its managers, or they hold less) the same step is a request to whoever can give it. Nothing is
//! given that its owner did not give.

use orch_access_contract::decisions::{MissingAccess, ObjectRef};
use orch_access_contract::pages::{Holder, PageChange, PageKind};
use orch_access_contract::permissions::{Choice, ObjectKind, Permission, Profile};
use orch_db::Conn;
use orch_types::Id;

use crate::depends::{Open, covers, held_units, resolved};
use crate::engine::{Frame, Graph, Site};
use crate::giving::{Acting, Giving};
use crate::nodes::every_permission;
use crate::resources::{self, read_into, site_of};
use crate::usable::Policies;
use crate::{Error, Principal, missing};

/// What is offered of an environment beside a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wants {
    /// Looking in: the environment's Can view, which is the environment itself and the View row of
    /// each of its four doors.
    Looking,
    /// The Manage of one door.
    Door(Permission),
}

impl Wants {
    /// The rows of the environment's page that it names.
    #[must_use]
    pub fn rows(self) -> Vec<Permission> {
        match self {
            Self::Looking => Choice::FullView
                .permissions(ObjectKind::Environment)
                .to_vec(),
            Self::Door(row) => vec![row],
        }
    }
}

/// Something on the environment that a permission of a thread's page is of no use without.
#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    /// The permission of the thread's page: `thread.view` for looking in, `thread.manage` for the
    /// Manage of a door.
    pub for_permission: Permission,
    /// The environment.
    pub object: ObjectRef,
    /// Its name.
    pub name: String,
    /// Its first owner.
    pub owner: Option<Id>,
    /// What is needed there.
    pub wants: Wants,
    /// Whether the holders of the permission on the thread have it there.
    pub open: Open,
    /// Whether the caller can give it: they hold it, and may share there.
    pub can_give: bool,
}

/// What the page that shows a dependency offers to do about it in the same step.
#[derive(Debug, Clone, PartialEq)]
pub enum Offer {
    /// The caller may give it: this change of the environment's page, made with their own rows.
    Give {
        /// The environment.
        object: ObjectRef,
        /// The change of its page.
        change: Box<PageChange>,
    },
    /// The caller may not: the same entries are asked of whoever can give them.
    Ask {
        /// What is asked and of whom: the item of an access request.
        item: Box<MissingAccess>,
        /// Whom the entries would name: the people just named on the thread.
        holders: Vec<Holder>,
    },
}

/// The doors a thread's page shows beside it, as the page of the thread's environment holds them:
/// for each, whether everyone who is let in by the thread has it, and whether the caller can give it.
pub async fn of_thread(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    environment: Id,
    policies: &Policies,
) -> Result<Vec<Dependency>, Error> {
    let mut graph = Graph::load(conn, &[thread]).await?;
    let thread_site = graph.site_of(thread).ok_or(Error::NotFound(thread))?;
    let found = read_into(conn, &mut graph, &[environment]).await?;
    let resource = found
        .into_iter()
        .find(|resource| resource.id == environment)
        .ok_or(Error::NotFound(environment))?;
    let site = site_of(&graph, &resource).ok_or(Error::NotFound(environment))?;
    let policy = policies.at(resource.home).node_policy();
    let mut wanted = every_permission([ObjectKind::Environment]);
    wanted.extend([
        Permission::ThreadView,
        Permission::ThreadInteract,
        Permission::ThreadManage,
    ]);
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        &[thread_site.clone(), site.clone()],
        &wanted,
    )
    .await?;
    let giving = Giving::of(&frame, Some(&site), PageKind::Live, Acting::Ordinary);
    let mut found: Vec<Dependency> = Vec::new();
    for (for_permission, wants) in [
        (Permission::ThreadView, Wants::Looking),
        (
            Permission::ThreadManage,
            Wants::Door(Permission::FilesManage),
        ),
        (
            Permission::ThreadManage,
            Wants::Door(Permission::TerminalManage),
        ),
    ] {
        found.push(Dependency {
            for_permission,
            object: resource.object(),
            name: resource.name.clone(),
            owner: resource.first_owner,
            wants,
            open: open_for(&frame, &thread_site, for_permission, &site, wants),
            can_give: wants
                .rows()
                .iter()
                .all(|row| giving.can_give(*row, Profile::Any)),
        });
    }
    Ok(found)
}

/// How far the people let in by `holder_permission` on the thread also have what `wants` names on
/// the environment: all of them, some, or nobody but its owner.
fn open_for(
    frame: &Frame<'_>,
    holder_site: &Site,
    holder_permission: Permission,
    environment: &Site,
    wants: Wants,
) -> Open {
    let now = frame.now();
    let units = held_units(frame, holder_site, holder_permission);
    if units.is_empty() {
        return Open::Nobody;
    }
    // Who each row of the environment's page lets in: whom it names, in it and in the rows that hold
    // it (managing the environment holds each door's Manage), and its owners.
    let rows = wants.rows();
    let named_in = |row: Permission| -> Vec<Holder> {
        let mut named: Vec<Holder> = Vec::new();
        for level in std::iter::once(row).chain(row.held_through()) {
            named.extend(
                frame
                    .chain(environment, level)
                    .holders(now)
                    .into_iter()
                    .map(|found| resolved(environment, found.entry.holder)),
            );
        }
        for owner in frame.owner_entries(environment.id) {
            if owner.counts(now) {
                named.push(owner.holder);
            }
        }
        named
    };
    let lists: Vec<Vec<Holder>> = rows.iter().map(|row| named_in(*row)).collect();
    let covered = units
        .iter()
        .filter(|forms| {
            lists.iter().all(|named| {
                forms
                    .iter()
                    .any(|form| named.iter().any(|entry| covers(entry, form)))
            })
        })
        .count();
    match covered {
        0 => Open::Nobody,
        count if count == units.len() => Open::All,
        _ => Open::Part,
    }
}

/// The offer for a dependency that is not open to all: an entry for `holders`, the people just named
/// on the thread, in the rows of the environment's page that it needs; given by the caller where they
/// may, asked for otherwise.
pub async fn offer(
    conn: &mut Conn,
    principal: &Principal,
    dependency: &Dependency,
    holders: &[Holder],
    policies: &Policies,
) -> Result<Offer, Error> {
    if dependency.can_give {
        let named: Vec<serde_json::Value> = holders
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()
            .map_err(|error| Error::Stored(error.to_string()))?;
        // Written as a request writes it, so that a member of a change that others add stays theirs.
        let change = match dependency.wants {
            Wants::Looking => serde_json::json!({
                "name": [{ "holders": named, "choice": Choice::FullView }],
            }),
            Wants::Door(row) => serde_json::json!({
                "rows": [{
                    "permission": row.as_str(),
                    "add": named.iter().map(|holder| serde_json::json!({ "holder": holder })).collect::<Vec<_>>(),
                }],
            }),
        };
        let change: PageChange =
            serde_json::from_value(change).map_err(|error| Error::Stored(error.to_string()))?;
        return Ok(Offer::Give {
            object: dependency.object,
            change: Box::new(change),
        });
    }
    let (graph, resource, site) = resources::site(conn, dependency.object).await?;
    let everything = every_permission([dependency.object.kind]);
    let policy = policies.at(resource.home).node_policy();
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        &everything,
    )
    .await?;
    // The request names the selected recipient, who needs the whole offered entry even when
    // the requester already holds some of its permissions.
    let lacking = dependency.wants.rows();
    let mut items = missing::missing_access(conn, &frame, &site, &lacking, true).await?;
    let item = items.pop().ok_or(Error::NotFound(dependency.object.id))?;
    Ok(Offer::Ask {
        item: Box::new(item),
        holders: holders.to_vec(),
    })
}
