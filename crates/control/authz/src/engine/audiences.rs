//! Only the group, role and viewer dependencies referenced by one permission frame.

use super::{
    Entry, Graph, Holder, PageKey, PageRef, Permission, Role, Site, Store, StoredKind, entry_of,
};
use crate::Error;
use orch_db::Conn;
use orch_db::queries::access;
use orch_types::{Id, Time};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(super) struct Group {
    pub org: Id,
    pub members: Vec<Holder>,
}

#[derive(Debug)]
pub(super) struct Grant {
    pub place: Id,
    pub subject: Holder,
    pub role: Role,
}

pub(super) struct Prepared {
    pub sites: Vec<Site>,
    pub groups: HashMap<Id, Group>,
    pub grants: Vec<Grant>,
    pub owners: HashMap<Id, Vec<Entry>>,
    pub ownable: Vec<PageKey>,
}

fn group_holder(row: access::GroupHolderRow) -> Result<Holder, Error> {
    if let Some(id) = row.user_id {
        Ok(Holder::User { id })
    } else if let Some(id) = row.member_group_id {
        Ok(Holder::Group { id })
    } else if let Some(id) = row.member_org_id {
        Ok(Holder::Members { id })
    } else if let (Some(id), Some(role)) = (row.member_node_id, row.member_role) {
        Ok(Holder::Role {
            role: role.parse()?,
            at: Some(id),
        })
    } else {
        Err(Error::Stored("group member has no holder".into()))
    }
}

fn role_holder(row: &access::RoleRow) -> Result<Holder, Error> {
    let id = row.subject_id;
    match row.subject_kind.as_str() {
        "user" => Ok(Holder::User { id }),
        "group" => Ok(Holder::Group { id }),
        "members" => Ok(Holder::Members { id }),
        "role" => Ok(Holder::Role {
            role: row
                .subject_role
                .as_deref()
                .ok_or_else(|| Error::Stored("role subject has no level".into()))?
                .parse()?,
            at: Some(id),
        }),
        _ => Err(Error::Stored("role subject has an unknown kind".into())),
    }
}

impl Prepared {
    #[allow(
        clippy::too_many_arguments,
        reason = "one prepared frame and its referenced holder worklist"
    )]
    pub(super) async fn load(
        conn: &mut Conn,
        graph: &mut Graph,
        store: &mut Store,
        mut loaded_pages: HashSet<PageRef>,
        wanted: &[Permission],
        sites: &[Site],
        requested: &[Holder],
        as_of: (Id, Time),
    ) -> Result<Self, Error> {
        let (person, now) = as_of;
        let mut prepared = Self {
            sites: sites
                .iter()
                .filter(|site| site.kind.node().is_some())
                .cloned()
                .collect(),
            groups: HashMap::new(),
            grants: Vec::new(),
            owners: HashMap::new(),
            ownable: Vec::new(),
        };
        let mut seen_sites: HashSet<_> = prepared.sites.iter().map(|site| site.id).collect();
        let mut seen_nodes: HashSet<_> = graph.nodes().map(|node| node.id).collect();
        let mut seen_groups = HashSet::new();
        let mut seen_roles = HashSet::new();
        let mut seen_owners = HashSet::new();
        loop {
            let owner_pages: Vec<_> = graph
                .nodes()
                .map(|node| PageRef::live(StoredKind::Node, node.id))
                .chain(sites.iter().map(Site::page))
                .chain(
                    sites
                        .iter()
                        .flat_map(|site| graph.document_ancestors(site.id))
                        .map(|id| PageRef::live(StoredKind::DocumentContainer, id)),
                )
                .filter(|page| seen_owners.insert(*page))
                .map(PageRef::key)
                .collect();
            if !owner_pages.is_empty() {
                for row in access::owner_entries(conn, &owner_pages).await? {
                    prepared
                        .owners
                        .entry(row.object_id)
                        .or_default()
                        .push(entry_of(&row)?);
                }
                prepared.ownable.extend(owner_pages);
            }
            let places: Vec<_> = graph
                .nodes()
                .filter(|node| node.is_place())
                .map(|node| node.id)
                .filter(|id| seen_roles.insert(*id))
                .collect();
            if !places.is_empty() {
                for row in access::roles_in_places(conn, &places, now.get()).await? {
                    prepared.grants.push(Grant {
                        place: row.node_id,
                        subject: role_holder(&row)?,
                        role: row.role.parse()?,
                    });
                }
            }
            let holders: Vec<_> = requested
                .iter()
                .copied()
                .chain(
                    store
                        .rows
                        .values()
                        .flat_map(|row| &row.entries)
                        .filter(|entry| entry.counts(now))
                        .map(|entry| entry.holder),
                )
                .chain(
                    prepared
                        .owners
                        .values()
                        .flatten()
                        .filter(|entry| entry.counts(now))
                        .map(|entry| entry.holder),
                )
                .chain(prepared.grants.iter().map(|grant| grant.subject))
                .chain(
                    prepared
                        .groups
                        .values()
                        .flat_map(|group| group.members.iter().copied()),
                )
                .collect();
            let groups: Vec<_> = holders
                .iter()
                .filter_map(|holder| match holder {
                    Holder::Group { id } if seen_groups.insert(*id) => Some(*id),
                    _ => None,
                })
                .collect();
            if !groups.is_empty() {
                let mut members = access::group_holders_many(conn, &groups, Some(person)).await?;
                for group in access::groups(conn, &groups).await? {
                    prepared.groups.insert(
                        group.id,
                        Group {
                            org: group.org_id,
                            members: members
                                .remove(&group.id)
                                .unwrap_or_default()
                                .into_iter()
                                .map(group_holder)
                                .collect::<Result<_, _>>()?,
                        },
                    );
                }
            }
            let nodes: Vec<_> = holders
                .iter()
                .filter_map(|holder| match holder {
                    Holder::Role { at: Some(id), .. } => Some(*id),
                    _ => None,
                })
                .chain(prepared.groups.values().map(|group| group.org))
                .filter(|id| seen_nodes.insert(*id))
                .collect();
            if !nodes.is_empty() {
                graph.extend(conn, &nodes).await?;
            }
            let mut added = HashSet::new();
            for holder in &holders {
                if let Holder::Role {
                    role: Role::Viewer,
                    at: Some(id),
                } = holder
                    && seen_sites.insert(*id)
                    && let Some(site) = graph.site_of(*id)
                {
                    added.insert(site.page());
                    added.extend(site.chain.iter().copied().map(PageRef::defaults_of));
                    prepared.sites.push(site);
                }
            }
            // Source parents also carry current outside-creation and ownership dependencies.
            added.extend(
                graph
                    .nodes()
                    .map(|node| PageRef::live(StoredKind::Node, node.id)),
            );
            added.retain(|page| loaded_pages.insert(*page));
            if added.is_empty() && nodes.is_empty() && groups.is_empty() {
                break;
            }
            if !added.is_empty() {
                store.extend(
                    Store::load(conn, &added.into_iter().collect::<Vec<_>>(), wanted).await?,
                );
            }
        }
        Ok(prepared)
    }
}
