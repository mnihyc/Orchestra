//! Count each person who reaches an environment once, batching pages, owners, holders and suspension checks.

use super::{PAGE_GATES, named_for};
use crate::engine::{Frame, Graph, Site};
use crate::{Error, Policy, Principal, resources};
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_access_contract::roles::Role;
use orch_db::Conn;
use orch_db::queries::access::{self, HolderAudiencePath, HolderAudienceSource, HolderPeopleQuery};
use orch_types::Id;
use std::collections::{HashMap, HashSet};

/// Counts the complete audience the reach endpoint names and redacts, without counting repeated ways in.
pub async fn reach_counts(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
) -> Result<HashMap<Id, i64>, Error> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut graph = Graph::default();
    let found = resources::read_into(conn, &mut graph, ids).await?;
    let environments: Vec<Site> = found
        .iter()
        .filter(|resource| resource.kind == ResourceKind::Environment && !resource.deleted)
        .filter_map(|resource| resources::site_of(&graph, resource))
        .collect();
    let wanted_ids: Vec<Id> = environments.iter().map(|site| site.id).collect();
    let threads = orch_db::queries::environments::threads_in(conn, &wanted_ids).await?;
    graph
        .extend(
            conn,
            &threads
                .iter()
                .map(|thread| thread.thread_id)
                .collect::<Vec<_>>(),
        )
        .await?;
    let mut sites = environments.clone();
    let mut belongs: Vec<Id> = environments.iter().map(|site| site.id).collect();
    for thread in threads {
        if let Some(site) = graph.site_of(thread.thread_id) {
            sites.push(site);
            belongs.push(thread.environment_id);
        }
    }
    let mut wanted = vec![
        Permission::ThreadView,
        Permission::ThreadInteract,
        Permission::EnvironmentUse,
    ];
    wanted.extend(PAGE_GATES);
    let frame = Frame::load(conn, principal, policy, graph, &sites, &wanted).await?;
    // Resolve each distinct indexed holder once for the complete list, not once per chip or owner row.
    let mut sources = Vec::new();
    // Who uses each environment, as the reach endpoint asks it: talking to the agent of a thread there
    // reaches in only for them, and for the drivers of the work in a place its Use row names.
    let mut use_sources = Vec::new();
    let mut work_places: HashMap<Id, Vec<Id>> = HashMap::new();
    let mut physical_sources = Vec::new();
    let mut physical_paths = HashMap::new();
    let mut physical_targets: HashMap<Id, HashSet<Id>> = HashMap::new();
    for (index, site) in sites.iter().enumerate() {
        let is_environment = index < environments.len();
        let gates: &[Permission] = if is_environment {
            &PAGE_GATES
        } else {
            &[Permission::ThreadInteract]
        };
        if is_environment {
            for holder in named_for(&frame, site, Permission::EnvironmentUse) {
                if let Holder::Node { id } = holder {
                    work_places.entry(site.id).or_default().push(id);
                } else if let Some(holder) = source(&frame, site, holder) {
                    use_sources.push(HolderAudienceSource {
                        object: site.id,
                        holder,
                        owner_root: None,
                    });
                }
            }
        }
        for gate in gates {
            for holder in named_for(&frame, site, *gate) {
                if let Some(holder) = source(&frame, site, holder) {
                    sources.push(HolderAudienceSource {
                        object: site.id,
                        holder,
                        owner_root: None,
                    });
                }
            }
        }
        if let Some(source_site) = frame.ownership_source(site)
            && let Some(root) = source_site.root
        {
            let targets = physical_targets.entry(source_site.id).or_default();
            let first = targets.is_empty();
            targets.insert(site.id);
            if first {
                for entry in frame
                    .owner_entries(source_site.id)
                    .iter()
                    .filter(|entry| entry.counts(frame.now()))
                {
                    if matches!(entry.holder, Holder::User { .. } | Holder::Group { .. })
                        && let Some(holder) = source(&frame, source_site, entry.holder)
                    {
                        physical_sources.push(HolderAudienceSource {
                            object: source_site.id,
                            holder,
                            owner_root: Some(root),
                        });
                        physical_paths.insert(source_site.id, source_site.orgs.as_slice());
                    }
                }
            }
        }
        if let Some(root) = site.root {
            for entry in frame
                .owner_entries(site.id)
                .iter()
                .filter(|entry| entry.counts(frame.now()))
            {
                if matches!(entry.holder, Holder::User { .. } | Holder::Group { .. })
                    && let Some(holder) = source(&frame, site, entry.holder)
                {
                    let owner = HolderAudienceSource {
                        object: site.id,
                        holder,
                        owner_root: Some(root),
                    };
                    if is_environment {
                        use_sources.push(owner);
                    }
                    sources.push(owner);
                }
            }
        }
    }
    let physical_paths: Vec<_> = physical_paths
        .into_iter()
        .map(|(object, orgs)| HolderAudiencePath { object, orgs })
        .collect();
    for (source, user) in
        access::holder_audiences(conn, &physical_sources, &physical_paths, frame.now().get())
            .await?
    {
        for target in physical_targets.get(&source).into_iter().flatten() {
            let owner = HolderAudienceSource {
                object: *target,
                holder: HolderPeopleQuery {
                    kind: "user",
                    id: Some(user),
                    role: "",
                },
                owner_root: None,
            };
            if wanted_ids.contains(target) {
                use_sources.push(owner);
            }
            sources.push(owner);
        }
    }
    let paths: Vec<_> = sites
        .iter()
        .map(|site| HolderAudiencePath {
            object: site.id,
            orgs: &site.orgs,
        })
        .collect();
    let people = access::holder_audiences(conn, &sources, &paths, frame.now().get()).await?;
    let mut users: HashMap<Id, HashSet<Id>> = HashMap::new();
    for (environment, person) in
        access::holder_audiences(conn, &use_sources, &paths, frame.now().get()).await?
    {
        users.entry(environment).or_default().insert(person);
    }
    let thread_paths: HashMap<Id, &[Id]> = sites
        .iter()
        .skip(environments.len())
        .filter_map(|site| site.path.as_deref().map(|path| (site.id, path)))
        .collect();
    let belongs: HashMap<_, _> = sites.iter().map(|site| site.id).zip(belongs).collect();
    let mut audiences: HashMap<Id, HashSet<Id>> =
        wanted_ids.iter().map(|id| (*id, HashSet::new())).collect();
    for (site, person) in people {
        let Some(environment) = belongs.get(&site) else {
            continue;
        };
        // Somebody let in by a thread's page reaches in only while they use the environment.
        let through_thread = !wanted_ids.contains(&site);
        if through_thread
            && !users
                .get(environment)
                .is_some_and(|users| users.contains(&person))
            && !thread_paths.get(&site).is_some_and(|path| {
                work_places
                    .get(environment)
                    .is_some_and(|places| places.iter().any(|place| path.contains(place)))
            })
        {
            continue;
        }
        audiences.entry(*environment).or_default().insert(person);
    }
    Ok(audiences
        .into_iter()
        .map(|(id, people)| (id, i64::try_from(people.len()).unwrap_or(i64::MAX)))
        .collect())
}

fn source(frame: &Frame<'_>, site: &Site, holder: Holder) -> Option<HolderPeopleQuery> {
    let (kind, id, role) = match holder {
        Holder::User { id } => ("user", id, ""),
        Holder::Group { id } => ("group", id, ""),
        Holder::Members { id } => ("members", id, ""),
        Holder::Role { role, at } => {
            let place = at.or(site.role_place)?;
            let node = frame.graph().get(place)?;
            if node.deleted
                || node
                    .path
                    .iter()
                    .any(|id| frame.graph().get(*id).is_some_and(|node| node.deleted))
            {
                return None;
            }
            (
                if role == Role::Viewer && at.is_some() {
                    "visible"
                } else {
                    "role"
                },
                place,
                role.as_str(),
            )
        }
        // These represent no enumerable person in the reach view, as in the detailed endpoint.
        Holder::Everyone | Holder::Node { .. } | Holder::Agents { .. } => return None,
    };
    Some(HolderPeopleQuery {
        kind,
        id: Some(id),
        role,
    })
}
