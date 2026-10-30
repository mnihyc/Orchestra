//! Naming somebody from outside the root org on the page of a node, a resource or a document: the row that takes, or owning the thing where the policy lets owners; who names the holders of that row; from then on it is access like anybody's, wherever they work.
//!
//! An outsider to a thing is somebody who is no member of its root org: a person, a group of
//! another root org, a role held there, a role given to any of them here, and the agents or the
//! work of a place there. The people of another department of the same tree are no outsiders.
//! A service identity kept by an org of the tree is local for naming, without becoming a member.
//! Changing a page takes `access.manage`; naming an outsider on it takes one thing more: the row
//! `access.external` there, which ships with Developers, or being one of its owners where the
//! policy `sharing.allow_external` resolves true at its place (for a node at that node, for a
//! resource at its home, for a document at its node). The owners of a thing do not hold the row by
//! owning it, or the policy would decide nothing; and only an admin of the place names the holders
//! of that row.
//!
//! Once they are named it is access like anybody's: nothing later asks which org a person or a
//! thread is of, so a provider that names somebody runs their threads in their own org, and an
//! environment that names them takes their threads. This part only decides who may name, and
//! tells which of a set of holders are outsiders; the naming itself is the ordinary change of a
//! page.

use orch_access_contract::decisions::{self, Giver, ObjectRef, Through};
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::PlaceRef;
use orch_db::Conn;
use orch_db::queries::access;
use orch_types::{Id, Time};

use crate::engine::{Frame, Graph, Site};
use crate::missing::{self, Who};
use crate::nodes::every_permission;
use crate::{Error, Policy, Principal, resources};

/// The holders of a change that may be from outside the root org, to be told from the others by
/// what is stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Candidates {
    users: Vec<Id>,
    groups: Vec<Id>,
    places: Vec<Id>,
    everyone: bool,
}

impl Candidates {
    /// The holders among `holders` that can be outsiders, sorted by what they name.
    #[must_use]
    pub fn of(holders: &[Holder]) -> Self {
        let mut candidates = Self::default();
        for holder in holders {
            match *holder {
                Holder::User { id } => candidates.users.push(id),
                Holder::Group { id } => candidates.groups.push(id),
                Holder::Role {
                    at: Some(place), ..
                } => candidates.places.push(place),
                Holder::Node { id } | Holder::Agents { id } | Holder::Members { id } => {
                    candidates.places.push(id)
                }
                Holder::Everyone => candidates.everyone = true,
                // A role held where the thing is names the tree's own people.
                Holder::Role { at: None, .. } => {}
            }
        }
        // A page change can name the same holder in several rows. The database
        // returns each identity once, so compare distinct candidates with it.
        for ids in [
            &mut candidates.users,
            &mut candidates.groups,
            &mut candidates.places,
        ] {
            ids.sort_unstable();
            ids.dedup();
        }
        candidates
    }

    /// Whether any of them is from outside the tree of `root`. Everyone is: every user of the
    /// system includes those of other trees.
    pub async fn any_outside(&self, conn: &mut Conn, root: Id) -> Result<bool, Error> {
        if self.everyone {
            return Ok(true);
        }
        if !self.users.is_empty() {
            let local = access::local_holders_among(conn, root, &self.users).await?;
            if self.users.iter().any(|user| !local.contains(user)) {
                return Ok(true);
            }
        }
        if !self.groups.is_empty() {
            let found = access::groups(conn, &self.groups).await?;
            if found.len() != self.groups.len() || found.iter().any(|group| group.root_id != root) {
                return Ok(true);
            }
        }
        if !self.places.is_empty() {
            let found = access::roots_of(conn, &self.places).await?;
            if found.len() != self.places.len() || found.iter().any(|(_, of)| *of != root) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Whether any of `holders` is from outside the tree of the root org `root`.
pub async fn any_outside(conn: &mut Conn, root: Id, holders: &[Holder]) -> Result<bool, Error> {
    Candidates::of(holders).any_outside(conn, root).await
}

/// How long an entry that lets somebody from outside the root tree in may run at one place, as
/// `sharing.external_max_days` bounds it there: the fewest days of the system and of the places on the
/// way to it, and the place that sets that bound (`None` for the system).
///
/// The bound is counted from the moment the entry is written, renewed or used: an outside entry is within
/// it when it has an end, and that end is no later than the bound from now. So a new or renewed entry
/// may end at most that many days on, and an entry a stricter policy or a move puts beyond the bound,
/// or that never ends, lets nobody in until it is given a nearer end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifeBound {
    /// The most days.
    pub days: i64,
    /// Where it is set: an org, a project or a topic; `None` for the system.
    pub place: Option<Id>,
}

impl LifeBound {
    /// The latest end an outside entry may have when it is written or used at `now`.
    #[must_use]
    pub fn latest_end(self, now: Time) -> Time {
        chrono::Duration::try_days(self.days)
            .and_then(|span| now.get().checked_add_signed(span))
            .map_or_else(
                || Time::new(chrono::DateTime::<chrono::Utc>::MAX_UTC),
                Time::new,
            )
    }

    /// Whether an entry that ends at `end` (never, for `None`) is within the bound at `now`.
    #[must_use]
    pub fn admits(self, end: Option<Time>, now: Time) -> bool {
        end.is_some_and(|end| end <= self.latest_end(now))
    }
}

/// Where `sharing.external_max_days` is set, read once: the system's value and each node's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifeBounds(Vec<(Option<Id>, i64)>);

impl LifeBounds {
    /// Reads where the bound is set: one statement.
    pub async fn load(conn: &mut Conn) -> Result<Self, Error> {
        Ok(Self(
            orch_db::queries::outsiders::external_max_days(conn).await?,
        ))
    }

    /// Whether it is set nowhere: then nothing is bounded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The bound at the place whose way from its root org is `way` (the place itself last): the fewest
    /// days set at the system or on that way; none where none is.
    #[must_use]
    pub fn at(&self, way: &[Id]) -> Option<LifeBound> {
        self.0
            .iter()
            .filter(|(place, days)| *days > 0 && place.is_none_or(|node| way.contains(&node)))
            .min_by_key(|(_, days)| *days)
            .map(|(place, days)| LifeBound {
                days: *days,
                place: *place,
            })
    }
}

/// The nodes on the way to a site from its root org: its own path where it is a node or lies at one,
/// otherwise the places whose defaults it follows (a resource's home and the places above it).
#[must_use]
pub fn way_of(site: &Site) -> Vec<Id> {
    site.path.clone().unwrap_or_else(|| {
        site.chain
            .iter()
            .filter_map(|place| match place {
                PlaceRef::Node(id) => Some(*id),
                PlaceRef::System => None,
            })
            .collect()
    })
}

/// Whether a person may name somebody from outside the root org on a page, and if not who may.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Naming {
    /// The ways in which they hold the right: the row `access.external` names them, or they own
    /// the thing where the policy lets owners. Empty when they may not.
    pub through: Vec<Through>,
    /// The nearest people who may, when the person may not: those who may change the page and hold
    /// the right to name an outsider on it. The option is shown disabled, with who can.
    pub ask: Vec<Id>,
    /// How many others could.
    pub ask_more: i64,
}

impl Naming {
    /// Whether they may.
    #[must_use]
    pub fn allowed(&self) -> bool {
        !self.through.is_empty()
    }
}

async fn site_of_thing(conn: &mut Conn, object: ObjectRef) -> Result<(Graph, Site), Error> {
    if object.kind.node().is_some() {
        let graph = Graph::load(conn, &[object.id]).await?;
        let site = graph.site_of(object.id).ok_or(Error::NotFound(object.id))?;
        Ok((graph, site))
    } else {
        let (graph, _, site) = resources::site(conn, object).await?;
        Ok((graph, site))
    }
}

/// Whether the person may name somebody from outside the root org on the page of the node or
/// resource `object`. `policy` carries `sharing.allow_external` resolved at its place.
pub async fn may_name(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    policy: Policy,
) -> Result<Naming, Error> {
    let (graph, site) = site_of_thing(conn, object).await?;
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
    let through = frame.ways(&site, Permission::AccessExternal);
    if !through.is_empty() {
        return Ok(Naming {
            through,
            ask: Vec::new(),
            ask_more: 0,
        });
    }
    let who = missing::who_may_name_outsiders(conn, &frame, &site).await?;
    let (ask, ask_more) = decisions::nearest(&who).ask();
    Ok(Naming {
        through,
        ask,
        ask_more,
    })
}

/// The people who may name the holders of the row `access.external` on `object`: its admins, and
/// nobody else, because the owners of a thing do not hold the row and so do not give it. Nearest
/// first.
pub async fn who_names_the_holders_of_the_row(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    policy: Policy,
) -> Result<Vec<Giver>, Error> {
    let (graph, site) = site_of_thing(conn, object).await?;
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
    missing::givers(
        conn,
        &frame,
        &site,
        Who::to_give(Permission::AccessExternal),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_access_contract::roles::Role;
    use orch_types::new_id;

    #[test]
    fn a_role_held_where_the_thing_is_names_the_trees_own_people_and_is_no_candidate() {
        let candidates = Candidates::of(&[Holder::Role {
            role: Role::Developer,
            at: None,
        }]);
        assert_eq!(candidates, Candidates::default());
    }

    #[test]
    fn a_person_a_group_a_place_and_everyone_are_the_candidates() {
        let (user, group, place) = (new_id(), new_id(), new_id());
        let candidates = Candidates::of(&[
            Holder::User { id: user },
            Holder::Group { id: group },
            Holder::Role {
                role: Role::Viewer,
                at: Some(place),
            },
            Holder::Node { id: place },
            Holder::Everyone,
        ]);
        assert_eq!(candidates.users, [user]);
        assert_eq!(candidates.groups, [group]);
        assert_eq!(candidates.places, [place]);
        assert!(candidates.everyone);
    }
}
