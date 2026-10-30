//! Defaults pages of the system, an org at any level, a project and a topic: resolution upward, the two profiles by where work runs, locks, and what a change reaches.
//!
//! A place keeps a defaults page for what is made inside it. A row of it
//! follows the defaults of the place above until it is cut loose, and the
//! topmost row that is locked on the way up decides every page below, what
//! they stored for it counting for nothing while it holds. The rows of a
//! thread, its files and its terminal, and of an environment, are two on
//! every defaults page: one for a managed environment, one for a person's
//! machine. A thread follows the one that fits where it runs.
//!
//! Reading and changing a defaults page is reading and changing a page, so
//! the work is [`pages`]; this part names the pages by the
//! place, and resolves a row upward.

use std::collections::{HashMap, HashSet};

use orch_access_contract::pages::PageChange;
use orch_access_contract::permissions::{ObjectKind, Permission, Placement, Profile};
use orch_access_contract::roles::{NodeKind, PlaceRef};
use orch_db::Conn;
use orch_db::audit;
use orch_db::queries::{access, hierarchy};
use orch_types::Id;

use crate::engine::{Frame, Graph, NodeFacts, PageRef};
use crate::pages::{self, ChangeError, Inherited, Outcome, PageView, Target};
use crate::{Error, Policy, Principal};

/// The defaults page of `place` as it is shown. The caller has checked that
/// they may see the place.
pub async fn read(
    conn: &mut Conn,
    principal: &Principal,
    place: PlaceRef,
    policy: Policy,
) -> Result<PageView, Error> {
    pages::read(conn, principal, Target::Defaults(place), policy).await
}

/// Changes the defaults page of `place`, for the caller, in the caller's
/// transaction: a page is changed by whoever holds `access.manage` on the
/// place, and the defaults of the system by a system admin.
pub async fn change(
    conn: &mut Conn,
    principal: &Principal,
    place: PlaceRef,
    change: &PageChange,
    policy: Policy,
    origin: &audit::Origin,
) -> Result<Outcome, ChangeError> {
    pages::change(
        conn,
        principal,
        Target::Defaults(place),
        change,
        policy,
        origin,
    )
    .await
}

/// Whom the defaults of `place` give for `permission` in the row `profile`,
/// by what the place stores and by what it follows, a lock above deciding
/// instead where there is one: what a new page inside the place starts
/// from, entry by entry, with the page each stands on. The caller has
/// checked that they may see the place.
pub async fn resolve(
    conn: &mut Conn,
    principal: &Principal,
    place: PlaceRef,
    permission: Permission,
    profile: Profile,
) -> Result<Vec<Inherited>, Error> {
    Ok(
        resolve_rows(conn, principal, place, &[(permission, profile)])
            .await?
            .pop()
            .unwrap_or_default(),
    )
}

/// Resolves several defaults rows from one reading of their pages, in the order requested.
/// Each row still follows its own profile, inheritance and locks; nothing is cached across requests.
pub async fn resolve_rows(
    conn: &mut Conn,
    principal: &Principal,
    place: PlaceRef,
    rows: &[(Permission, Profile)],
) -> Result<Vec<Vec<Inherited>>, Error> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let (graph, places): (Graph, Vec<PlaceRef>) = match place {
        PlaceRef::System => (Graph::default(), vec![PlaceRef::System]),
        PlaceRef::Node(id) => {
            let graph = Graph::load(conn, &[id]).await?;
            let site = graph.site_of(id).ok_or(Error::NotFound(id))?;
            let mut places = vec![PlaceRef::Node(id)];
            places.extend(site.chain.iter().copied());
            (graph, places)
        }
    };
    let sites: Vec<_> = match place {
        PlaceRef::Node(id) => graph.site_of(id).into_iter().collect(),
        PlaceRef::System => Vec::new(),
    };
    let permissions: Vec<Permission> = rows.iter().map(|(permission, _)| *permission).collect();
    let pages: Vec<PageRef> = places.iter().copied().map(PageRef::defaults_of).collect();
    let frame = Frame::load_with(
        conn,
        principal,
        Policy::default(),
        graph,
        &sites,
        &pages,
        &permissions,
    )
    .await?;
    Ok(rows
        .iter()
        .map(|(permission, profile)| {
            frame
                .defaults_chain(&places, *profile, *permission)
                .holders(principal.now())
                .into_iter()
                .map(|named| Inherited {
                    entry: named.entry.clone(),
                    from: named.from,
                })
                .collect()
        })
        .collect())
}

/// The places a defaults row reaches downward: whether `node` is one where
/// defaults can be kept.
#[must_use]
pub fn is_place(node: &NodeFacts) -> bool {
    node.is_place()
}

/// How many pages inside the place `place` follow at least one of its
/// defaults rows `rows`: what a change of those rows reaches, and what a
/// preview says first. A page follows a row when nothing between it and the
/// place cuts the row loose, and no lock decides the row from a place
/// between or above. A row locked at the place itself is followed by every
/// page inside that has it.
pub async fn followers(
    conn: &mut Conn,
    place: Id,
    rows: &[(Permission, Profile)],
) -> Result<i64, Error> {
    let graph = Graph::load(conn, &[place]).await?;
    let site = graph.site_of(place).ok_or(Error::NotFound(place))?;
    let inside = hierarchy::descendants(conn, place).await?;
    let kinds: HashMap<Id, NodeKind> = inside
        .iter()
        .filter_map(|row| row.kind.parse().ok().map(|kind| (row.id, kind)))
        .collect();
    let words: Vec<String> = rows
        .iter()
        .map(|(permission, _)| permission.as_str().to_owned())
        .collect();
    let mut objects: Vec<Id> = inside.iter().map(|row| row.id).collect();
    objects.push(place);
    objects.extend(site.admin_places.iter().copied());
    let stored = access::stored_rows_of_nodes(conn, &objects, &words).await?;
    // (object, page, profile, permission) -> (inherits, locked)
    let mut flags: HashMap<(Id, &str, &str, &str), (bool, bool)> = HashMap::new();
    for row in &stored {
        flags.insert(
            (
                row.object_id,
                if row.page == "live" {
                    "live"
                } else {
                    "defaults"
                },
                row.profile.as_str(),
                row.permission.as_str(),
            ),
            (row.inherit, row.locked),
        );
    }
    let flag = |object: Id, page: &'static str, profile: Profile, permission: Permission| {
        flags
            .get(&(object, page, profile.as_str(), permission.as_str()))
            .copied()
            .unwrap_or((true, false))
    };
    let mut following: HashSet<Id> = HashSet::new();
    'rows: for (permission, profile) in rows {
        // A lock above the place decides the row; the place's own row decides nothing.
        for above in &site.admin_places {
            if flag(*above, "defaults", *profile, *permission).1 {
                continue 'rows;
            }
        }
        let locked_here = flag(place, "defaults", *profile, *permission).1;
        for page in &inside {
            let Some(kind) = kinds.get(&page.id).copied() else {
                continue;
            };
            let placement = page
                .placement
                .as_deref()
                .and_then(|word| word.parse::<Placement>().ok())
                .unwrap_or_default();
            if !permission.exists_on(ObjectKind::from(kind))
                || permission.profile(placement) != *profile
            {
                continue;
            }
            if locked_here {
                following.insert(page.id);
                continue;
            }
            let mut follows = flag(page.id, "live", Profile::Any, *permission).0;
            let at = page.path.iter().position(|id| *id == place).unwrap_or(0);
            let between = page
                .path
                .get(at + 1..page.path.len().saturating_sub(1))
                .unwrap_or_default();
            for between in between.iter().rev() {
                if kinds
                    .get(between)
                    .is_some_and(|kind| kind.place().is_some())
                {
                    let (inherits, locked) = flag(*between, "defaults", *profile, *permission);
                    follows &= inherits && !locked;
                }
            }
            if follows {
                following.insert(page.id);
            }
        }
    }
    Ok(i64::try_from(following.len()).unwrap_or(i64::MAX))
}
