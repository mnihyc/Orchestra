//! Proves which requested role audiences do not depend on current permission-page viewers.
//!
//! This classifies sources, not people. The legacy grouped inverse query remains responsible
//! for ordinary role membership, owners, expiry, suspension and native role identities.

use crate::Error;
use crate::engine::Graph;
use orch_access_contract::roles::Role;
use orch_db::Conn;
use orch_db::queries::access::{self, EntryRow, PageKey, RoleRow};
use orch_types::{Id, Time};
use std::collections::{HashMap, HashSet, VecDeque};

type Key = (Id, Role);

#[derive(Default)]
struct Dependencies {
    roles: HashSet<Key>,
    groups: HashSet<Id>,
    dynamic: bool,
}

#[derive(Default)]
struct Metadata {
    graph: Graph,
    loaded: HashSet<Id>,
    roles: HashMap<Id, Vec<RoleRow>>,
    owners: HashMap<Id, Vec<EntryRow>>,
}

impl Metadata {
    /// Each referenced place and its ancestors are loaded once, independently of how many
    /// requested role levels use them. No permission rows or user rosters are read here.
    async fn extend(&mut self, conn: &mut Conn, keys: &[Key], now: Time) -> Result<(), Error> {
        let nodes: Vec<_> = keys
            .iter()
            .map(|(place, _)| *place)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        self.graph.extend(conn, &nodes).await?;
        let places: Vec<_> = self
            .graph
            .nodes()
            .filter(|node| node.is_place())
            .map(|node| node.id)
            .filter(|place| self.loaded.insert(*place))
            .collect();
        if places.is_empty() {
            return Ok(());
        }
        for row in access::roles_in_places(conn, &places, now.get()).await? {
            self.roles.entry(row.node_id).or_default().push(row);
        }
        let pages: Vec<_> = places
            .into_iter()
            .map(|place| PageKey::new("node", place, "live"))
            .collect();
        for row in access::owner_entries(conn, &pages).await? {
            if row.expires_at.is_none_or(|end| end > now.get()) {
                self.owners.entry(row.object_id).or_default().push(row);
            }
        }
        Ok(())
    }

    fn dependencies(&self, (place, minimum): Key) -> Result<Dependencies, Error> {
        let mut found = Dependencies::default();
        let Some(node) = self
            .graph
            .get(place)
            .filter(|node| node.is_place() && self.graph.site_of(place).is_some())
        else {
            // A thread, missing place or removed path is not a proven ordinary role source.
            // Its existing exact path may still return the empty audience.
            found.dynamic = true;
            return Ok(found);
        };
        for ancestor in self.graph.places_on(node) {
            // Owning a place supplies its Admin role, so owners are relevant to every minimum.
            for owner in self.owners.get(&ancestor.id).into_iter().flatten() {
                match (owner.holder_kind.as_str(), owner.holder_id) {
                    ("user", Some(_)) => {}
                    ("group", Some(group)) => {
                        found.groups.insert(group);
                    }
                    _ => found.dynamic = true,
                }
            }
            for row in self.roles.get(&ancestor.id).into_iter().flatten() {
                if !row.role.parse::<Role>()?.includes(minimum) {
                    continue;
                }
                match row.subject_kind.as_str() {
                    "user" | "members" => {}
                    "group" => {
                        found.groups.insert(row.subject_id);
                    }
                    "role" => {
                        let role = row
                            .subject_role
                            .as_deref()
                            .ok_or_else(|| Error::Stored("role subject has no level".into()))?
                            .parse()?;
                        if role == Role::Viewer {
                            // An explicit Viewer subject means current page View, not a role.
                            // Mark it dynamic without opening the source's permission graph.
                            found.dynamic = true;
                        } else {
                            found.roles.insert((row.subject_id, role));
                        }
                    }
                    _ => found.dynamic = true,
                }
            }
        }
        Ok(found)
    }
}

/// Zero-based indices of located `role` sources whose complete role/owner dependencies are
/// independent of page Viewer sources. Pure role cycles remain on the legacy inverse path;
/// a Viewer subject or nonplain group makes every source depending on it dynamic.
pub(super) async fn sources(
    conn: &mut Conn,
    sources: &[access::HolderPeopleQuery],
    now: Time,
) -> Result<HashSet<usize>, Error> {
    let roots: Vec<_> = sources
        .iter()
        .enumerate()
        .filter_map(|(index, source)| {
            (source.kind == "role")
                .then_some(source.id)
                .flatten()
                .map(|place| Ok((index, (place, source.role.parse::<Role>()?))))
        })
        .collect::<Result<_, Error>>()?;
    if roots.is_empty() {
        return Ok(HashSet::new());
    }
    let mut seen = HashSet::new();
    let mut frontier: Vec<_> = roots
        .iter()
        .map(|(_, key)| *key)
        .filter(|key| seen.insert(*key))
        .collect();
    let mut metadata = Metadata::default();
    let mut dependencies = HashMap::new();
    while !frontier.is_empty() {
        metadata.extend(conn, &frontier, now).await?;
        let mut next = Vec::new();
        for key in frontier {
            let found = metadata.dependencies(key)?;
            for dependency in &found.roles {
                if seen.insert(*dependency) {
                    next.push(*dependency);
                }
            }
            dependencies.insert(key, found);
        }
        frontier = next;
    }
    let groups: HashSet<_> = dependencies
        .values()
        .flat_map(|found| found.groups.iter().copied())
        .collect();
    let plain = super::audiences::plain_groups(conn, groups).await?;
    let mut parents: HashMap<Key, Vec<Key>> = HashMap::new();
    let mut dynamic = HashSet::new();
    for (key, found) in &dependencies {
        if found.dynamic || found.groups.iter().any(|group| !plain.contains(group)) {
            dynamic.insert(*key);
        }
        for dependency in &found.roles {
            parents.entry(*dependency).or_default().push(*key);
        }
    }
    // Reverse propagation visits each affected dependency once, including cycles. Work scales
    // with referenced places and role levels rather than the number of audience members.
    let mut pending: VecDeque<_> = dynamic.iter().copied().collect();
    while let Some(key) = pending.pop_front() {
        for parent in parents.get(&key).into_iter().flatten() {
            if dynamic.insert(*parent) {
                pending.push_back(*parent);
            }
        }
    }
    Ok(roots
        .into_iter()
        .filter(|(_, key)| !dynamic.contains(key))
        .map(|(index, _)| index)
        .collect())
}
