//! The effect a change of a page would have, worked out before it is made: how many pages it reaches and who gains and who loses what on them, counted and named as far as the caller may know them.
//!
//! A preview is not a second reading of the change. The change is made, by the engine's own
//! [`pages::change`], inside a trial that is undone however it ends, and what each page gives
//! to whom is read before and after: whatever the change would do, it does here, and nothing is left
//! behind. A page is *reached* when what it follows or stores for some permission differs after the
//! change; a person *gains* a permission on a page when they hold it after and did not before, and
//! *loses* it in the other case. Owners, the admins of the place and the people behind each holder
//! are the engine's, found as it finds them for a refusal.
//!
//! A person is named only to a caller who may know them: a system admin, the caller themself, a member
//! of a tree the caller is a member of, and anybody who held something on the pages already, whom the
//! pages' own chips list for the caller. The others are counted and not named. Nobody is named twice.
//! A place with more than [`PAGES_MOST`] pages of one kind inside it is examined for the first
//! [`PAGES_MOST`] by identifier of each kind; a person suspended in an org but not altogether is
//! counted as anybody is, since they hold what is named again as soon as it is lifted.
//!
//! A place is never one thing here. The page of a topic, a project or an org is one page like any
//! other, and a change of it reaches that page alone and opens nothing inside the place; a change of
//! the defaults of a place reaches the pages inside it that follow the rows it touches, and the place's
//! own page is not among them. The page of one document says nothing of the node it lies beside.

use std::collections::{BTreeSet, HashMap, HashSet};

use orch_access_contract::pages::{Holder, PageChange};
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::{PlaceRef, Role};
use orch_db::Conn;
use orch_db::audit::Origin;
use orch_db::queries::access::{self, everything};
use orch_types::Id;

use crate::engine::{Frame, Graph, Site};
use crate::giving::{Acting, Giving};
use crate::nodes::every_permission;
use crate::pages::{self, ChangeError};
use crate::things::{self, Thing};
use crate::usable::ResourcePolicy;
use crate::{Error, Policy, Principal, documents, holders, resources};
use orch_access_contract::pages::PageKind;
use orch_access_contract::permissions::Profile;

/// The most people a preview names in each of its lists; the rest are counted.
pub const PEOPLE_MOST: usize = 200;
/// The most pages of each kind (nodes, resources, documents) a place's preview looks at.
pub const PAGES_MOST: usize = 20_000;

/// Whether a readable thing has any row this caller may share. This is checked before a sharing preview reads recipients.
pub async fn may_share(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    policy: Policy,
) -> Result<bool, Error> {
    let (graph, site) = things::site(conn, thing).await?;
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        Permission::ALL,
    )
    .await?;
    let view = site
        .kind
        .node()
        .and_then(|kind| orch_access_contract::permissions::NodePermission::View.on(kind))
        .or(site.kind.is_document().then_some(Permission::DocView));
    if view.map_or_else(
        || !frame.holds_any(&site),
        |view| frame.ways(&site, view).is_empty(),
    ) {
        return Ok(false);
    }
    let giving = Giving::of(&frame, Some(&site), PageKind::Live, Acting::Ordinary);
    Ok(giving.may_share()
        && site
            .kind
            .permissions()
            .any(|permission| giving.can_give(permission, Profile::Any)))
}

/// Selected people who already hold considered rows, through the same holder expansion and page chains as the effect preview.
pub async fn already_held(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    policy: Policy,
    selected: &[Holder],
    permissions: &[Permission],
) -> Result<(Vec<Moved>, i64), Error> {
    if !may_share(conn, principal, thing, policy).await? {
        return Err(Error::NotFound(thing.id()));
    }
    let (mut graph, site) = things::site(conn, thing).await?;
    let places: Vec<_> = selected
        .iter()
        .filter_map(|holder| match holder {
            Holder::Role { at, .. } => *at,
            Holder::Members { id } | Holder::Node { id } | Holder::Agents { id } => Some(*id),
            _ => None,
        })
        .collect();
    graph.extend(conn, &places).await?;
    let taken = take(
        conn,
        principal,
        policy,
        graph.clone(),
        std::slice::from_ref(&site),
    )
    .await?;
    let frame = Frame::load(
        conn,
        principal,
        policy,
        graph,
        std::slice::from_ref(&site),
        Permission::ALL,
    )
    .await?;
    let mut wanted = HashSet::new();
    for holder in selected {
        // Everyone contributes to consequences, but is never a directory of names behind a page chip.
        if *holder == Holder::Everyone {
            wanted.extend(access::all_active_users(conn).await?);
        } else {
            wanted.extend(holders::people_of(conn, &frame, &site, holder).await?);
        }
    }
    let everyone: Vec<_> = wanted.iter().copied().collect();
    let active: HashSet<_> = access::active_among(conn, &everyone, &[])
        .await?
        .into_iter()
        .collect();
    let known: HashSet<_> = if principal.is_system_admin() {
        wanted.clone()
    } else {
        everything::sharing_a_tree_with(conn, principal.user(), &everyone)
            .await?
            .into_iter()
            .collect()
    };
    let named_direct: HashSet<_> = selected
        .iter()
        .filter_map(|holder| match holder {
            Holder::User { id } => Some(*id),
            _ => None,
        })
        .collect();
    let listed: HashSet<_> = taken
        .people
        .iter()
        .filter(|(key, _)| **key != Key::Holder(Holder::Everyone))
        .flat_map(|(_, ids)| ids.iter().copied())
        .collect();
    let mut out = Vec::new();
    let mut more = 0;
    for user in wanted.into_iter().filter(|user| active.contains(user)) {
        let held: Vec<_> = permissions
            .iter()
            .copied()
            .filter(|permission| {
                taken.keys.get(&(site.id, *permission)).is_some_and(|keys| {
                    keys.iter().any(|key| {
                        taken
                            .people
                            .get(key)
                            .is_some_and(|people| people.contains(&user))
                    })
                })
            })
            .collect();
        if held.is_empty() {
            continue;
        }
        if user == principal.user()
            || known.contains(&user)
            || named_direct.contains(&user)
            || listed.contains(&user)
        {
            out.push(Moved {
                user,
                permissions: held,
                pages: 1,
            });
        } else {
            more += 1;
        }
    }
    out.sort_by_key(|person| person.user);
    more += i64::try_from(out.len().saturating_sub(PEOPLE_MOST)).unwrap_or(i64::MAX);
    out.truncate(PEOPLE_MOST);
    Ok((out, more))
}

/// Which pages a preview is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// One page alone: the page of a thing, or of a place itself, which opens nothing inside it.
    Page(Thing),
    /// Every page inside a place that follows its defaults, the place's own page not among them: a
    /// node, or the system.
    Inside(PlaceRef),
}

/// A person who would gain or lose something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    /// The person.
    pub user: Id,
    /// What they would gain or lose, in the order of the catalogue.
    pub permissions: Vec<Permission>,
    /// On how many pages.
    pub pages: i64,
}

/// What a change would do.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Preview {
    /// How many pages it reaches: one for the page of a thing.
    pub pages: i64,
    /// For a preview of the defaults of the system: in how many root orgs those pages lie; none for
    /// every other preview.
    pub orgs: Option<i64>,
    /// The people who would gain, the most pages first.
    pub gaining: Vec<Moved>,
    /// The people who would lose, the most pages first.
    pub losing: Vec<Moved>,
    /// The people beyond the first [`PEOPLE_MOST`] of either list, and those the caller may not
    /// know by name.
    pub more: i64,
}

/// Whom a page gives a permission to, before the people behind them are found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    /// A holder a row names, with the place of a role made explicit.
    Holder(Holder),
    /// A shipped/implicit local role is an actual role, rather than page-only viewing of its place.
    ImplicitViewer(Id),
    /// The owners of a page.
    Owners(Id),
    /// The admins of the place a page is in, and the system's.
    Admins(Id),
}

/// What the pages give, read once.
struct Taken {
    /// For each page and permission, whom it gives it to.
    keys: HashMap<(Id, Permission), HashSet<Key>>,
    /// For each page, what stands under its rows now: the permission, and the holder with the
    /// options and the end of the entry. Two entries that say the same are the same, whichever
    /// entry they are.
    entries: HashMap<Id, BTreeSet<(&'static str, String)>>,
    /// The people behind each key.
    people: HashMap<Key, Vec<Id>>,
}

/// A role without a place is held where the page is, which the key says.
fn made_explicit(holder: Holder, site: &Site) -> Holder {
    match holder {
        Holder::Role { role, at } => Holder::Role {
            role,
            at: at.or(site.role_place),
        },
        other => other,
    }
}

/// The permissions whose holders a page lists: all of its kind's but the Owners row, which is not a row.
fn rows_of(site: &Site) -> impl Iterator<Item = Permission> {
    site.kind
        .permissions()
        .filter(|permission| *permission != Permission::AccessOwn)
}

async fn take(
    conn: &mut Conn,
    principal: &Principal,
    policy: Policy,
    graph: Graph,
    sites: &[Site],
) -> Result<Taken, Error> {
    let everything = every_permission(sites.iter().map(|site| site.kind));
    let frame = Frame::load(conn, principal, policy, graph, sites, &everything).await?;
    let now = frame.now();
    let mut keys: HashMap<(Id, Permission), HashSet<Key>> = HashMap::new();
    let mut entries: HashMap<Id, BTreeSet<(&'static str, String)>> = HashMap::new();
    let mut first: HashMap<Key, usize> = HashMap::new();
    for (index, site) in sites.iter().enumerate() {
        for permission in rows_of(site) {
            let mut named: HashSet<Key> = HashSet::new();
            if permission != Permission::AccessExternal || policy.owners_name_outsiders {
                named.insert(Key::Owners(site.id));
            }
            if permission == Permission::AccessManage {
                named.insert(Key::Admins(site.id));
            }
            for row in std::iter::once(permission)
                .chain(permission.held_through())
                .filter(|row| *row != Permission::AccessOwn)
            {
                for held in frame.chain(site, row).holders(now) {
                    named.insert(match (held.entry.holder, site.role_place) {
                        (
                            Holder::Role {
                                role: Role::Viewer,
                                at: None,
                            },
                            Some(place),
                        ) => Key::ImplicitViewer(place),
                        _ => Key::Holder(made_explicit(held.entry.holder, site)),
                    });
                    entries.entry(site.id).or_default().insert((
                        row.as_str(),
                        format!(
                            "{:?} {} {:?}",
                            held.entry.holder, held.entry.options, held.entry.expires_at
                        ),
                    ));
                }
            }
            for key in &named {
                first.entry(*key).or_insert(index);
            }
            keys.insert((site.id, permission), named);
        }
    }
    let people = expand(conn, &frame, sites, &first).await?;
    Ok(Taken {
        keys,
        entries,
        people,
    })
}

/// The most (place, person) pairs read at once for the roles given in all the places of a preview.
const ROLE_ROWS_MOST: i64 = 200_000;

/// The people behind each key, found for all the keys together: a handful of statements however many
/// pages there are, since what a page names is the same few holders and the owners of each page are
/// already read. It gives what [`holders::people_of`], [`holders::owner_people`] and
/// [`holders::admins_at`] give for each key one at a time.
async fn expand(
    conn: &mut Conn,
    frame: &Frame<'_>,
    sites: &[Site],
    keys: &HashMap<Key, usize>,
) -> Result<HashMap<Key, Vec<Id>>, Error> {
    let now = frame.now();
    let graph = frame.graph();
    let mut ways: HashMap<Id, Vec<Id>> = HashMap::new();
    let mut places: BTreeSet<Id> = BTreeSet::new();
    let mut owned: BTreeSet<Id> = BTreeSet::new();
    let mut roles: Vec<Role> = Vec::new();
    let mut groups: BTreeSet<Id> = BTreeSet::new();
    let mut member_orgs: BTreeSet<Id> = BTreeSet::new();
    for (key, index) in keys {
        let site = &sites[*index];
        match key {
            Key::Holder(Holder::Role {
                role,
                at: Some(place),
            }) => {
                if let Some(node) = graph.get(*place).filter(|node| {
                    !node.deleted
                        && !node
                            .path
                            .iter()
                            .any(|id| graph.get(*id).is_some_and(|node| node.deleted))
                }) {
                    let way: Vec<Id> = graph.places_on(node).iter().map(|each| each.id).collect();
                    places.extend(way.iter().copied());
                    owned.extend(way.iter().copied());
                    ways.insert(*place, way);
                    if !roles.contains(role) {
                        roles.push(*role);
                    }
                }
            }
            Key::Holder(Holder::Group { id }) => {
                groups.insert(*id);
            }
            Key::Holder(Holder::Members { id }) => {
                member_orgs.insert(*id);
            }
            Key::ImplicitViewer(place) => {
                places.insert(*place);
            }
            Key::Owners(id) => {
                owned.insert(*id);
            }
            Key::Admins(_) => {
                places.extend(site.admin_places.iter().copied());
                owned.extend(site.admin_places.iter().copied());
                if !roles.contains(&Role::Admin) {
                    roles.push(Role::Admin);
                }
            }
            Key::Holder(_) => {}
        }
    }
    // Who owns each place and thing asked about: the people named in its Owners row and the members
    // of the groups named there, who count while they are members of its root org.
    let mut owners: HashMap<Id, Vec<Id>> = HashMap::new();
    let mut by_group: Vec<(Id, Id)> = Vec::new();
    for id in &owned {
        let mut users: Vec<Id> = Vec::new();
        for entry in frame
            .owner_entries(*id)
            .iter()
            .filter(|entry| entry.counts(now))
        {
            match entry.holder {
                Holder::User { id: user } => users.push(user),
                Holder::Group { id: group } => {
                    groups.insert(group);
                    by_group.push((*id, group));
                }
                _ => {}
            }
        }
        owners.insert(*id, users);
    }
    let in_groups: Vec<Id> = groups.iter().copied().collect();
    let mut members_by_domain: HashMap<Id, Vec<Id>> = HashMap::new();
    if !member_orgs.is_empty() {
        for (org, user) in access::member_people(
            conn,
            &member_orgs.into_iter().collect::<Vec<_>>(),
            ROLE_ROWS_MOST,
        )
        .await?
        {
            members_by_domain.entry(org).or_default().push(user);
        }
    }
    let mut members_of: HashMap<Id, Vec<Id>> = HashMap::new();
    if !in_groups.is_empty() {
        for (group, user) in
            access::group_people(conn, &in_groups, ROLE_ROWS_MOST, now.get()).await?
        {
            members_of.entry(group).or_default().push(user);
        }
    }
    for (id, group) in by_group {
        if let Some(members) = members_of.get(&group) {
            owners
                .entry(id)
                .or_default()
                .extend(members.iter().copied());
        }
    }
    let mut roots: HashMap<Id, Id> = graph.nodes().map(|node| (node.id, node.root)).collect();
    for site in sites {
        if let Some(root) = site.root {
            roots.entry(site.id).or_insert(root);
        }
    }
    let root_of = |id: Id| -> Option<Id> { roots.get(&id).copied() };
    let mut candidates: HashMap<Id, BTreeSet<Id>> = HashMap::new();
    for (id, users) in &owners {
        if let Some(root) = root_of(*id) {
            candidates
                .entry(root)
                .or_default()
                .extend(users.iter().copied());
        }
    }
    let mut members: HashMap<Id, HashSet<Id>> = HashMap::new();
    for (root, users) in candidates {
        let asked: Vec<Id> = users.into_iter().collect();
        members.insert(
            root,
            access::members_among(conn, root, &asked)
                .await?
                .into_iter()
                .collect(),
        );
    }
    let owners_of = |id: Id| -> Vec<Id> {
        let Some(root) = root_of(id) else {
            return Vec::new();
        };
        let counted = members.get(&root);
        owners
            .get(&id)
            .into_iter()
            .flatten()
            .copied()
            .filter(|user| counted.is_some_and(|counted| counted.contains(user)))
            .collect()
    };
    // Who was given each role in each of the places: once for every role asked about.
    let place_list: Vec<Id> = places.iter().copied().collect();
    let mut given: Vec<(Role, HashMap<Id, Vec<Id>>)> = Vec::new();
    for role in &roles {
        let names: Vec<String> = Role::ALL
            .iter()
            .filter(|held| held.includes(*role))
            .map(|held| held.as_str().to_owned())
            .collect();
        let mut at: HashMap<Id, Vec<Id>> = HashMap::new();
        if !place_list.is_empty() {
            for (place, user) in
                access::people_given_role(conn, &place_list, &names, now.get(), ROLE_ROWS_MOST)
                    .await?
            {
                at.entry(place).or_default().push(user);
            }
        }
        given.push((*role, at));
    }
    let system_admins = if roles.is_empty() {
        Vec::new()
    } else {
        access::system_admin_people(conn, holders::SCAN_MOST).await?
    };
    let mut actual_viewers: HashMap<Id, Vec<Id>> = HashMap::new();
    if keys.keys().any(|key| matches!(key, Key::ImplicitViewer(_))) {
        let roles: Vec<String> = Role::ALL
            .iter()
            .map(|role| role.as_str().to_owned())
            .collect();
        for (place, user) in
            access::people_in_roles(conn, &place_list, &roles, now.get(), ROLE_ROWS_MOST, false)
                .await?
        {
            actual_viewers.entry(place).or_default().push(user);
        }
    }
    let mut explicit_viewers: HashMap<Id, Vec<Id>> = HashMap::new();
    if keys.keys().any(|key| {
        matches!(
            key,
            Key::Holder(Holder::Role {
                role: Role::Viewer,
                ..
            })
        )
    }) {
        let roles: Vec<String> = Role::ALL
            .iter()
            .map(|role| role.as_str().to_owned())
            .collect();
        for (place, user) in
            access::people_in_roles(conn, &place_list, &roles, now.get(), ROLE_ROWS_MOST, true)
                .await?
        {
            explicit_viewers.entry(place).or_default().push(user);
        }
    }
    let given_at = |role: Role, place: Id| -> Vec<Id> {
        given
            .iter()
            .find(|(each, _)| *each == role)
            .and_then(|(_, at)| at.get(&place))
            .cloned()
            .unwrap_or_default()
    };
    let mut people: HashMap<Key, Vec<Id>> = HashMap::new();
    for (key, index) in keys {
        let site = &sites[*index];
        let found: Vec<Id> = match key {
            Key::Holder(Holder::User { id }) => vec![*id],
            Key::Holder(Holder::Group { id }) => members_of.get(id).cloned().unwrap_or_default(),
            Key::Holder(Holder::Members { id }) => {
                members_by_domain.get(id).cloned().unwrap_or_default()
            }
            Key::Holder(Holder::Everyone) => access::all_active_users(conn).await?,
            Key::ImplicitViewer(place) => actual_viewers.get(place).cloned().unwrap_or_default(),
            Key::Holder(Holder::Role {
                role: Role::Viewer,
                at: Some(place),
            }) => explicit_viewers.get(place).cloned().unwrap_or_default(),
            Key::Holder(Holder::Role {
                role,
                at: Some(place),
            }) => {
                let mut found: Vec<Id> = Vec::new();
                for on_the_way in ways.get(place).into_iter().flatten() {
                    found.extend(given_at(*role, *on_the_way));
                    if *role != Role::Viewer {
                        found.extend(owners_of(*on_the_way));
                    }
                }
                if ways.contains_key(place) && *role != Role::Viewer {
                    found.extend(system_admins.iter().copied());
                }
                found
            }
            Key::Holder(_) => Vec::new(),
            Key::Owners(id) => owners_of(*id),
            Key::Admins(_) => {
                let mut found: Vec<Id> = Vec::new();
                for place in &site.admin_places {
                    found.extend(given_at(Role::Admin, *place));
                    found.extend(owners_of(*place));
                }
                found.extend(system_admins.iter().copied());
                found
            }
        };
        people.insert(*key, found);
    }
    Ok(people)
}

/// The sites a scope is about, with the nodes they lie among.
async fn sites_of(conn: &mut Conn, scope: Scope) -> Result<(Graph, Vec<Site>), Error> {
    match scope {
        Scope::Page(thing) => {
            let (graph, site) = things::site(conn, thing).await?;
            Ok((graph, vec![site]))
        }
        Scope::Inside(place) => {
            let limit = i64::try_from(PAGES_MOST).unwrap_or(i64::MAX);
            let inside = everything::inside(conn, place.node(), limit).await?;
            let mut graph = Graph::load(conn, &inside.nodes).await?;
            let mut sites: Vec<Site> = inside
                .nodes
                .iter()
                .filter_map(|id| graph.site_of(*id))
                .collect();
            for resource in resources::read_into(conn, &mut graph, &inside.resources).await? {
                if !resource.deleted
                    && let Some(site) = resources::site_of(&graph, &resource)
                {
                    sites.push(site);
                }
            }
            for document in documents::read_into(conn, &mut graph, &inside.documents).await? {
                if let Some(site) = documents::site_of(&graph, &document) {
                    sites.push(site);
                }
            }
            Ok((graph, sites))
        }
    }
}

/// What the pages of a scope give, before a change.
pub struct Before {
    scope: Scope,
    policy: Policy,
    taken: Taken,
}

impl Before {
    /// Reads what the pages of `scope` give now, for the caller.
    pub async fn take(
        conn: &mut Conn,
        principal: &Principal,
        scope: Scope,
        policy: Policy,
    ) -> Result<Self, Error> {
        let (graph, sites) = sites_of(conn, scope).await?;
        let taken = take(conn, principal, policy, graph, &sites).await?;
        Ok(Self {
            scope,
            policy,
            taken,
        })
    }

    /// Reads what the pages give now, after the change was made on `conn`, and says what differs.
    /// `conn` is a connection on which the change was made and will be undone.
    pub async fn finish(&self, conn: &mut Conn, principal: &Principal) -> Result<Preview, Error> {
        let (graph, sites) = sites_of(conn, self.scope).await?;
        let after = take(conn, principal, self.policy, graph, &sites).await?;
        let mut gains: HashMap<Id, Tally> = HashMap::new();
        let mut losses: HashMap<Id, Tally> = HashMap::new();
        let mut reached: i64 = 0;
        let mut roots: HashSet<Id> = HashSet::new();
        let nothing: HashSet<Key> = HashSet::new();
        let none: Vec<Id> = Vec::new();
        for site in &sites {
            let mut differs = self.taken.entries.get(&site.id) != after.entries.get(&site.id);
            for permission in rows_of(site) {
                let was = self
                    .taken
                    .keys
                    .get(&(site.id, permission))
                    .unwrap_or(&nothing);
                let is = after.keys.get(&(site.id, permission)).unwrap_or(&nothing);
                differs |= was != is;
                let held = |keys: &HashSet<Key>, people: &HashMap<Key, Vec<Id>>| -> HashSet<Id> {
                    keys.iter()
                        .flat_map(|key| people.get(key).unwrap_or(&none).iter().copied())
                        .collect()
                };
                let before = held(was, &self.taken.people);
                let now = held(is, &after.people);
                // A move changes the people behind a role or an admin key even when the row
                // still names the same holder. Those gains reach the page just as a row edit does.
                differs |= before != now;
                for person in now.difference(&before) {
                    gains.entry(*person).or_default().note(permission, site.id);
                }
                for person in before.difference(&now) {
                    losses.entry(*person).or_default().note(permission, site.id);
                }
            }
            if differs {
                reached += 1;
                roots.extend(site.root);
            }
        }
        let listed: HashSet<Id> = self
            .taken
            .people
            .iter()
            .filter(|(key, _)| **key != Key::Holder(Holder::Everyone))
            .flat_map(|(_, people)| people.iter().copied())
            .collect();
        let everyone: Vec<Id> = gains.keys().chain(losses.keys()).copied().collect();
        let known: HashSet<Id> = if principal.is_system_admin() {
            everyone.iter().copied().collect()
        } else {
            everything::sharing_a_tree_with(conn, principal.user(), &everyone)
                .await?
                .into_iter()
                .collect()
        };
        let active: HashSet<Id> = access::active_among(conn, &everyone, &[])
            .await?
            .into_iter()
            .collect();
        let mut more: i64 = 0;
        let mut name_them = |tallies: HashMap<Id, Tally>| -> Vec<Moved> {
            let mut named: Vec<Moved> = Vec::new();
            for (user, tally) in tallies {
                if !active.contains(&user) {
                    continue;
                }
                if user == principal.user() || known.contains(&user) || listed.contains(&user) {
                    named.push(Moved {
                        user,
                        permissions: tally.permissions.into_iter().collect(),
                        pages: tally.pages,
                    });
                } else {
                    more += 1;
                }
            }
            named.sort_by(|first, second| {
                second
                    .pages
                    .cmp(&first.pages)
                    .then_with(|| first.user.cmp(&second.user))
            });
            let beyond = named.len().saturating_sub(PEOPLE_MOST);
            more += i64::try_from(beyond).unwrap_or(i64::MAX);
            named.truncate(PEOPLE_MOST);
            named
        };
        let gaining = name_them(gains);
        let losing = name_them(losses);
        Ok(Preview {
            pages: match self.scope {
                Scope::Page(_) => 1,
                Scope::Inside(_) => reached,
            },
            orgs: match self.scope {
                Scope::Inside(PlaceRef::System) => {
                    Some(i64::try_from(roots.len()).unwrap_or(i64::MAX))
                }
                Scope::Inside(PlaceRef::Node(_)) | Scope::Page(_) => None,
            },
            gaining,
            losing,
            more,
        })
    }
}

/// What one person gains or loses, counted as the pages are walked.
#[derive(Default)]
struct Tally {
    permissions: BTreeSet<Permission>,
    pages: i64,
    last: Option<Id>,
}

impl Tally {
    fn note(&mut self, permission: Permission, page: Id) {
        self.permissions.insert(permission);
        if self.last != Some(page) {
            self.pages += 1;
            self.last = Some(page);
        }
    }
}

/// What a change of the page of a thing would do. The change is made and undone; it needs the
/// right to make it, and is refused as it would be, for whatever reason. `policy` is what the
/// settings say at the thing's home.
pub async fn of_page(
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    change: &PageChange,
    policy: &ResourcePolicy,
    origin: &Origin,
) -> Result<Preview, ChangeError> {
    let before = Before::take(conn, principal, Scope::Page(thing), policy.node_policy()).await?;
    let mut trial = access::begin_trial(conn).await?;
    let outcome = async {
        match thing {
            Thing::Node(_) => {
                pages::change(
                    &mut trial,
                    principal,
                    thing.target(),
                    change,
                    policy.node_policy(),
                    origin,
                )
                .await?;
            }
            Thing::Other(object) => {
                pages::change_resource(&mut trial, principal, object, change, policy, origin)
                    .await?;
            }
        }
        Ok::<_, ChangeError>(before.finish(&mut trial, principal).await?)
    }
    .await;
    access::end_trial(trial).await?;
    outcome
}

/// What a change of the defaults of a place would do: how many pages inside it follow the rows it
/// touches and who gains and loses what on them. The change is made and undone.
pub async fn of_defaults(
    conn: &mut Conn,
    principal: &Principal,
    place: PlaceRef,
    change: &PageChange,
    policy: Policy,
    origin: &Origin,
) -> Result<Preview, ChangeError> {
    let before = Before::take(conn, principal, Scope::Inside(place), policy).await?;
    let mut trial = access::begin_trial(conn).await?;
    let outcome = async {
        pages::change(
            &mut trial,
            principal,
            pages::Target::Defaults(place),
            change,
            policy,
            origin,
        )
        .await?;
        Ok::<_, ChangeError>(before.finish(&mut trial, principal).await?)
    }
    .await;
    access::end_trial(trial).await?;
    outcome
}
