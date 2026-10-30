//! Current audience sets for truthful operation receipts, separate from the acting agent's authority.

use super::{Actor, Error, Item, access};
use orch_access_contract::permissions::Permission;
use orch_authz::{Caller, Frame, Policy, Principal, documents, engine::Graph, holders};
use orch_db::{Conn, queries::documents::containers};
use orch_types::{Id, Time};
use std::collections::{BTreeSet, HashMap};

pub(super) async fn read(
    conn: &mut Conn,
    actor: Actor<'_>,
    entries: &[containers::EntryRow],
    now: Time,
) -> Result<HashMap<Item, BTreeSet<Id>>, Error> {
    // This principal is only a context for the native all-holders projection. Actual operation
    // authority was checked separately with Actor; an agent never borrows this person's permissions.
    let principal = match actor {
        Actor::Person(principal) => principal.clone(),
        Actor::Agent { .. } => {
            let identity = access::identity(conn, actor).await?;
            Principal::load(conn, Caller::session(identity.owner), now).await?
        }
    };
    let ids: Vec<_> = entries
        .iter()
        .filter(|row| row.deleted_at.is_none())
        .map(|row| row.id)
        .collect();
    let mut graph = Graph::default();
    let descriptions = documents::read_into(conn, &mut graph, &ids).await?;
    let sites: Vec<_> = descriptions
        .iter()
        .filter_map(|document| documents::site_of(&graph, document))
        .collect();
    let frame = Frame::load(
        conn,
        &principal,
        Policy::default(),
        graph,
        &sites,
        &[Permission::DocView],
    )
    .await?;
    let mut result = HashMap::new();
    for site in &sites {
        let item = if site.kind == orch_access_contract::permissions::ObjectKind::DocumentContainer
        {
            Item::Container(site.id)
        } else {
            Item::Document(site.id)
        };
        result.insert(
            item,
            holders::who_holds(conn, &frame, site, Permission::DocView)
                .await?
                .into_iter()
                .collect(),
        );
    }
    Ok(result)
}

pub(super) fn difference(
    before: &HashMap<Item, BTreeSet<Id>>,
    after: &HashMap<Item, BTreeSet<Id>>,
    copies: &[super::Copied],
) -> (i64, i64) {
    let mut gaining = BTreeSet::new();
    let mut losing = BTreeSet::new();
    let pairs: Vec<_> = if copies.is_empty() {
        before
            .keys()
            .chain(after.keys())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|item| (item, item))
            .collect()
    } else {
        copies
            .iter()
            .map(|copy| (copy.source, copy.destination))
            .collect()
    };
    let empty = BTreeSet::new();
    for (source, destination) in pairs {
        let was = before.get(&source).unwrap_or(&empty);
        let now = after.get(&destination).unwrap_or(&empty);
        gaining.extend(now.difference(was).copied());
        losing.extend(was.difference(now).copied());
    }
    (
        i64::try_from(gaining.len()).unwrap_or(i64::MAX),
        i64::try_from(losing.len()).unwrap_or(i64::MAX),
    )
}
