//! A node as the API shows it to one caller: what is stored of it, its way down from the root org with the names the caller may know, and what the caller holds on it, for any number of nodes in the same few statements.
//!
//! Every list of nodes is made here. The nodes are read together with the
//! nodes above them, the access engine is asked once about all of them
//! (the caller's role, the permissions held, whether they own or administer
//! it, whether its page is still the defaults, whether it is private), and
//! what a view adds from other tables is counted for all of them at once. So
//! a list costs the same number of statements for one node as for a
//! thousand. A node the caller may not view is built all the same and marked
//! as one they may not: whoever lists decides what to do with it.

use crate::middleware::authorization::{
    self,
    refusals::{node_kind, user_refs},
};
use crate::middleware::requests::{ApiError, ApiResult};
use chrono::{DateTime, Utc};
use orch_access_contract::pages::PageState as StoredPageState;
use orch_access_contract::permissions::{NodePermission, ObjectKind, Permission};
use orch_access_contract::roles::{self, NodeKind as StoredKind, PlaceRef};
use orch_api_contract::access::Role;
use orch_api_contract::common::SubscriptionLevel;
use orch_api_contract::documents::DocumentRef;
use orch_api_contract::nodes::{
    Access, FieldSource, Node, NodeBase, NodeKind, NodeOrSystem, NodeRef, Org, OrgKind, OrgType,
    PageState, Project, ProjectKind, Tag, Topic, TopicAttention, TopicKind, TopicStatus,
};
use orch_api_contract::threads::ThreadKind;
use orch_api_contract::users::UserRef;
use orch_authz::engine::{Frame, Graph, Site};
use orch_authz::{Act, Policy, Principal};
use orch_db::Conn;
use orch_db::queries::hierarchy::{self, NodeDetail};
use orch_types::{Id, Time};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::{BTreeSet, HashMap};

/// A node built for one caller.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    /// The node.
    pub node: Node,
    /// Whether the caller may view it: holds its View permission, is not
    /// suspended where it lies, and has a token that reaches it.
    pub viewable: bool,
}

/// A word of one contract as the same word of another.
fn same<A: Serialize, B: DeserializeOwned>(word: &A) -> Option<B> {
    serde_json::to_value(word)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

/// The kind of node as the API names it.
#[must_use]
pub fn kind_of(kind: StoredKind) -> NodeKind {
    match kind {
        StoredKind::Org => NodeKind::Org,
        StoredKind::Project => NodeKind::Project,
        StoredKind::Topic => NodeKind::Topic,
        StoredKind::Thread => NodeKind::Thread,
    }
}

fn level(word: &str) -> Option<SubscriptionLevel> {
    same(&word)
}

fn source(word: &str) -> FieldSource {
    if word == "auto" {
        FieldSource::Auto
    } else {
        FieldSource::User
    }
}

fn time(at: DateTime<Utc>) -> Time {
    Time::new(at)
}

/// Every permission of every kind among `sites`: what the access engine
/// needs to have read to say all that a person holds on them.
pub fn every_permission(sites: &[Site]) -> Vec<Permission> {
    let mut all: Vec<Permission> = Vec::new();
    let mut seen: Vec<ObjectKind> = Vec::new();
    for site in sites {
        if seen.contains(&site.kind) {
            continue;
        }
        seen.push(site.kind);
        for permission in site.kind.permissions() {
            if !all.contains(&permission) {
                all.push(permission);
            }
        }
    }
    all
}

/// What the caller holds on the sites that were read, and the references
/// to the nodes on their ways.
struct Standing<'p> {
    frame: Frame<'p>,
    sites: HashMap<Id, Site>,
    refs: HashMap<Id, NodeRef>,
}

/// Reads what is needed to say what the caller holds on `ids` and on every
/// node above them.
async fn stand<'p>(
    conn: &mut Conn,
    principal: &'p Principal,
    ids: &[Id],
    policy: Policy,
) -> ApiResult<Standing<'p>> {
    stand_with_participation(conn, principal, ids, policy, true).await
}

async fn stand_with_participation<'p>(
    conn: &mut Conn,
    principal: &'p Principal,
    ids: &[Id],
    policy: Policy,
    participation: bool,
) -> ApiResult<Standing<'p>> {
    stand_with_name_sources(conn, principal, ids, policy, participation, None).await
}

/// The same standing producer with an additional trusted disclosure bound on protected node names.
async fn stand_with_name_sources<'p>(
    conn: &mut Conn,
    principal: &'p Principal,
    ids: &[Id],
    policy: Policy,
    participation: bool,
    name_sources: Option<&BTreeSet<Id>>,
) -> ApiResult<Standing<'p>> {
    let (frame, sites) = reference_frame(conn, principal, ids, policy, participation).await?;
    let readable_threads: Vec<Id> = sites
        .iter()
        .filter(|site| {
            name_sources.is_none_or(|sources| sources.contains(&site.id))
                && reference_display_known(&frame, principal, site)
        })
        .map(|site| site.id)
        .collect();
    let displays = orch_engine::views::thread_displays(
        conn,
        &readable_threads,
        Some(principal.user()),
        principal.now(),
    )
    .await
    .map_err(ApiError::internal)?;
    let refs = sites
        .iter()
        .map(|site| {
            let known = name_sources.is_none_or(|sources| sources.contains(&site.id))
                && reference_known(&frame, principal, site);
            (
                site.id,
                NodeRef {
                    thread_display: displays.get(&site.id).cloned().map(Box::new),
                    id: site.id,
                    kind: kind_of(site.kind.node().unwrap_or(StoredKind::Org)),
                    name: known.then(|| site.name.clone()),
                    redacted: !known,
                },
            )
        })
        .collect();
    Ok(Standing {
        frame,
        sites: sites.into_iter().map(|site| (site.id, site)).collect(),
        refs,
    })
}

async fn reference_frame<'p>(
    conn: &mut Conn,
    principal: &'p Principal,
    ids: &[Id],
    policy: Policy,
    participation: bool,
) -> ApiResult<(Frame<'p>, Vec<Site>)> {
    let graph = Graph::load(conn, ids).await.map_err(ApiError::internal)?;
    let sites: Vec<Site> = graph
        .nodes()
        .filter_map(|node| graph.site_of(node.id))
        .collect();
    let wanted = every_permission(&sites);
    let frame = if participation {
        Frame::load(conn, principal, policy, graph, &sites, &wanted).await
    } else {
        Frame::load_with_participation(conn, principal, policy, graph, &sites, &wanted, &[]).await
    }
    .map_err(ApiError::internal)?;
    Ok((frame, sites))
}

fn reference_known(frame: &Frame<'_>, principal: &Principal, site: &Site) -> bool {
    frame.holds_any(site)
        || (site.kind == ObjectKind::Org
            && site.root.is_some_and(|root| principal.is_in_tree(root)))
}

fn reference_display_known(frame: &Frame<'_>, principal: &Principal, site: &Site) -> bool {
    site.kind == ObjectKind::Thread
        && !principal.suspension().applies(&site.orgs)
        && principal.caller().reaches(site.path.as_deref())
        && !frame.ways(site, Permission::ThreadView).is_empty()
}

/// The native reference producer's name and thread-display authority before formatting.
pub(crate) struct ReferenceDisclosure {
    pub(crate) names: BTreeSet<Id>,
    pub(crate) displays: BTreeSet<Id>,
}

/// Preserve the same two native predicates used by refs, without constructing public metadata.
pub(crate) async fn reference_disclosure(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
) -> ApiResult<ReferenceDisclosure> {
    if ids.is_empty() {
        return Ok(ReferenceDisclosure {
            names: BTreeSet::new(),
            displays: BTreeSet::new(),
        });
    }
    let (frame, sites) = reference_frame(conn, principal, ids, policy, false).await?;
    Ok(ReferenceDisclosure {
        names: sites
            .iter()
            .filter(|site| reference_known(&frame, principal, site))
            .map(|site| site.id)
            .collect(),
        displays: sites
            .iter()
            .filter(|site| reference_display_known(&frame, principal, site))
            .map(|site| site.id)
            .collect(),
    })
}

/// Identity-only form of the exact ordinary reference visibility predicate. Source
/// selection needs no live display/name projection and creates no alternative ACL.
pub(crate) async fn reference_visibility(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
) -> ApiResult<BTreeSet<Id>> {
    Ok(reference_disclosure(conn, principal, ids, policy)
        .await?
        .names)
}

/// The native reference builder's ThreadView cut, without constructing protected displays.
pub(crate) async fn reference_thread_visibility(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
) -> ApiResult<BTreeSet<Id>> {
    if ids.is_empty() {
        return Ok(BTreeSet::new());
    }
    let (frame, sites) = reference_frame(conn, principal, ids, policy, false).await?;
    Ok(sites
        .iter()
        .filter(|site| {
            site.kind == ObjectKind::Thread
                && !principal.suspension().applies(&site.orgs)
                && principal.caller().reaches(site.path.as_deref())
                && !frame.ways(site, Permission::ThreadView).is_empty()
        })
        .map(|site| site.id)
        .collect())
}

/// The ordinary native header's authority shape, without constructing a Node reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferenceAuthority {
    permissions: BTreeSet<Permission>,
    owns: bool,
    administers: bool,
    admin: Option<(PlaceRef, bool)>,
    role: Option<(roles::Role, PlaceRef)>,
}

impl ReferenceAuthority {
    pub(crate) fn preserves(&self, fresh: &Self) -> bool {
        self.permissions.is_subset(&fresh.permissions)
            && (!self.owns || fresh.owns)
            && (!self.administers || fresh.administers)
            && self.admin == fresh.admin
            && self.role == fresh.role
    }
}

pub(crate) async fn reference_authority(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    policy: Policy,
) -> ApiResult<ReferenceAuthority> {
    let (frame, sites) = reference_frame(conn, principal, &[id], policy, true).await?;
    let site = sites
        .iter()
        .find(|site| site.id == id)
        .ok_or_else(ApiError::not_found)?;
    Ok(ReferenceAuthority {
        permissions: frame
            .effective(site)
            .iter()
            .map(|held| held.permission)
            .collect(),
        owns: frame.owns(site),
        administers: frame.administers(site),
        admin: frame
            .admin_access(site)
            .map(|reach| (reach.place, reach.private)),
        role: frame.role_for(site).map(|role| (role.role, role.at)),
    })
}

/// Whether the caller may view the site.
fn viewable(standing: &Standing<'_>, site: &Site) -> bool {
    let principal = standing.frame.principal();
    let Some(view) = site
        .kind
        .node()
        .and_then(|kind| NodePermission::View.on(kind))
    else {
        return false;
    };
    !principal.suspension().applies(&site.orgs)
        && principal.caller().reaches(site.path.as_deref())
        && !standing.frame.ways(site, view).is_empty()
}

fn role_of(role: roles::Role) -> Option<Role> {
    same(&role)
}

fn access_of(
    standing: &Standing<'_>,
    site: &Site,
    path: &[Id],
    subscriptions: &HashMap<Id, SubscriptionLevel>,
) -> Access {
    let frame = &standing.frame;
    let mut permissions: Vec<String> = frame
        .effective(site)
        .iter()
        .map(|held| held.permission.as_str().to_owned())
        .collect();
    permissions.sort();
    let held = frame.role_for(site);
    Access {
        admin_place: (!frame.admin_only(site).is_empty())
            .then(|| frame.admin_place(site))
            .flatten()
            .and_then(|place| admin_place_of(standing, place)),
        admin_only: frame
            .admin_only(site)
            .iter()
            .map(|permission| permission.as_str().to_owned())
            .collect(),
        admin_access: frame
            .admin_access(site)
            .and_then(|place| admin_access_of(standing, place)),
        participating: frame.participates(site),
        permissions,
        role: held.and_then(|held| role_of(held.role)),
        role_from: held.and_then(|held| match held.at {
            PlaceRef::System => Some(NodeOrSystem::SYSTEM),
            PlaceRef::Node(place) => standing.refs.get(&place).cloned().map(NodeOrSystem::Node),
        }),
        administers: frame.administers(site),
        owns: frame.owns(site),
        subscription: subscriptions.get(&site.id).copied(),
        effective_subscription: path
            .iter()
            .rev()
            .find_map(|place| subscriptions.get(place).copied())
            .unwrap_or(SubscriptionLevel::Off),
        inherited_subscription: Some(
            path.iter()
                .rev()
                .filter(|place| **place != site.id)
                .find_map(|place| subscriptions.get(place).copied())
                .unwrap_or(SubscriptionLevel::Off),
        ),
    }
}

/// Native provenance uses the same qualified place names as holders.
pub(crate) fn admin_access(
    reach: orch_access_contract::roles::AdminReach,
    refs: &HashMap<Id, NodeRef>,
) -> Option<orch_api_contract::access::AdminAccess> {
    Some(orch_api_contract::access::AdminAccess {
        place: admin_place(reach.place, refs)?,
        private: reach.private,
    })
}

/// The source of a native act, named from the same batch as the object's marker.
pub(crate) fn admin_place(place: PlaceRef, refs: &HashMap<Id, NodeRef>) -> Option<NodeOrSystem> {
    let place = match place {
        PlaceRef::System => NodeOrSystem::SYSTEM,
        PlaceRef::Node(id) => NodeOrSystem::Node(refs.get(&id)?.clone()),
    };
    Some(place)
}

fn admin_place_of(standing: &Standing<'_>, place: PlaceRef) -> Option<NodeOrSystem> {
    let place = match place {
        PlaceRef::System => NodeOrSystem::SYSTEM,
        PlaceRef::Node(id) => NodeOrSystem::Node(qualified_of(standing, id)?),
    };
    Some(place)
}

fn admin_access_of(
    standing: &Standing<'_>,
    reach: orch_access_contract::roles::AdminReach,
) -> Option<orch_api_contract::access::AdminAccess> {
    Some(orch_api_contract::access::AdminAccess {
        place: admin_place_of(standing, reach.place)?,
        private: reach.private,
    })
}

/// Names provenance from any batched frame in one qualified-place lookup.
pub(crate) async fn named_admin_access<K: Clone + Eq + std::hash::Hash>(
    conn: &mut Conn,
    principal: &Principal,
    places: &HashMap<K, orch_access_contract::roles::AdminReach>,
) -> ApiResult<HashMap<K, orch_api_contract::access::AdminAccess>> {
    let ids: Vec<Id> = places
        .values()
        .filter_map(|reach| reach.place.node())
        .collect();
    let refs = qualified_refs(conn, principal, &ids, Policy::default()).await?;
    Ok(places
        .iter()
        .filter_map(|(id, place)| admin_access(*place, &refs).map(|marker| (id.clone(), marker)))
        .collect())
}

fn page_state(standing: &Standing<'_>, site: &Site, now: Time) -> PageState {
    let frame = &standing.frame;
    let changed = frame
        .store()
        .rows_of(site.page())
        .filter(|row| row.permission != Permission::AccessOwn)
        .any(|row| !row.inherit || row.locked || row.names(now).next().is_some());
    match StoredPageState::of(changed, frame.restricted(site)) {
        StoredPageState::Follows => PageState::Follows,
        StoredPageState::Custom => PageState::Custom,
        StoredPageState::Restricted => PageState::Restricted,
    }
}

/// What the views of the nodes among `details` add from other tables.
struct Extras {
    thread_display_states: HashMap<Id, orch_api_contract::threads::ThreadDisplayState>,
    work_attention: HashMap<Id, orch_api_contract::nodes::TopicWorkAttention>,
    orgs: HashMap<Id, hierarchy::OrgFacts>,
    topics: HashMap<Id, hierarchy::TopicFacts>,
    topic_counts: HashMap<Id, i64>,
    project_counts: HashMap<Id, i64>,
    tags: HashMap<Id, Vec<hierarchy::NodeTagRow>>,
    environments: HashMap<Id, Vec<orch_api_contract::environment::EnvironmentRef>>,
    defaults: HashMap<Id, Option<Id>>,
}

async fn extras(
    conn: &mut Conn,
    details: &[&NodeDetail],
    principal: &Principal,
    now: DateTime<Utc>,
    topic_sources: Option<&crate::routes::environments::reading::TopicSummarySources>,
) -> ApiResult<Extras> {
    let of_kind = |kind: &str| -> Vec<Id> {
        details
            .iter()
            .filter(|detail| detail.kind == kind)
            .map(|detail| detail.id)
            .collect()
    };
    let (orgs, projects, topics) = (of_kind("org"), of_kind("project"), of_kind("topic"));
    let mut found = Extras {
        thread_display_states: HashMap::new(),
        work_attention: HashMap::new(),
        orgs: HashMap::new(),
        topics: HashMap::new(),
        topic_counts: HashMap::new(),
        project_counts: HashMap::new(),
        tags: HashMap::new(),
        environments: HashMap::new(),
        defaults: HashMap::new(),
    };
    if !orgs.is_empty() {
        found.orgs = hierarchy::org_facts(conn, &orgs, now)
            .await?
            .into_iter()
            .map(|facts| (facts.node_id, facts))
            .collect();
    }
    found.defaults = match topic_sources {
        Some(sources) => sources.defaults.clone(),
        None => orch_settings::resolver::get_at_nodes(
            conn,
            &topics,
            orch_settings_contract::registry::keys::ENV_DEFAULT_ID,
        )
        .await
        .map_err(ApiError::internal)?,
    };
    if !projects.is_empty() {
        found.topic_counts = hierarchy::topic_counts(conn, &projects)
            .await?
            .into_iter()
            .collect();
        found.project_counts = hierarchy::project_counts(conn, &projects)
            .await?
            .into_iter()
            .collect();
    }
    if !topics.is_empty() {
        // Retain one existing API page of metadata while preserving the complete aggregate.
        let batch_size =
            usize::try_from(orch_api_contract::common::MAX_LIMIT).map_err(ApiError::internal)?;
        let mut after = None;
        loop {
            let rows = orch_db::queries::source::topic_threads_page(
                conn,
                &topics,
                after,
                i64::try_from(batch_size).map_err(ApiError::internal)?,
            )
            .await?;
            if rows.is_empty() {
                break;
            }
            let last_page = rows.len() < batch_size;
            after = rows.last().map(|(topic, thread, _)| (*topic, *thread));
            let ids: Vec<Id> = rows.iter().map(|(_, thread, _)| *thread).collect();
            let visible = orch_authz::nodes::holding(
                conn,
                principal,
                &ids,
                authorization::VIEW,
                Act::Use,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|node| node.id)
            .collect::<std::collections::HashSet<_>>();
            let ids: Vec<Id> = ids.into_iter().filter(|id| visible.contains(id)).collect();
            let main_ids: Vec<Id> = rows
                .iter()
                .filter(|(_, thread, main)| *main && visible.contains(thread))
                .map(|(_, thread, _)| *thread)
                .collect();
            let counts = orch_db::queries::source::topic_attention_counts(conn, &main_ids).await?;
            for (topic, uncommitted, unpushed, failed) in counts {
                let count = found.work_attention.entry(topic).or_default();
                count.uncommitted += uncommitted;
                count.unpushed += unpushed;
                count.failed += failed;
            }
            let displays = orch_engine::views::thread_displays(
                conn,
                &ids,
                Some(principal.user()),
                principal.now(),
            )
            .await
            .map_err(ApiError::internal)?;
            let priority = orch_api_contract::threads::THREAD_DISPLAY_URGENCY;
            for (topic, thread, _) in rows {
                if let Some(display) = displays.get(&thread)
                    && let Some(rank) = priority.iter().position(|state| *state == display.state)
                    && found.thread_display_states.get(&topic).is_none_or(|held| {
                        priority
                            .iter()
                            .position(|state| state == held)
                            .is_none_or(|previous| rank < previous)
                    })
                {
                    found.thread_display_states.insert(topic, display.state);
                }
            }
            if last_page {
                break;
            }
        }
        found.environments = match topic_sources {
            Some(sources) => {
                crate::routes::environments::reading::topic_summaries_from_sources(
                    conn,
                    principal,
                    sources,
                    Time::new(now),
                )
                .await?
            }
            None => {
                crate::routes::environments::reading::topic_summaries(
                    conn,
                    principal,
                    &topics,
                    Time::new(now),
                )
                .await?
            }
        };
        found.topics = hierarchy::topic_facts(conn, &topics)
            .await?
            .into_iter()
            .map(|facts| (facts.node_id, facts))
            .collect();
    }
    let nodes: Vec<Id> = details.iter().map(|detail| detail.id).collect();
    for tag in hierarchy::node_tags(conn, &nodes).await? {
        found.tags.entry(tag.tagged_id).or_default().push(tag);
    }
    Ok(found)
}

/// The nodes `ids` as the caller sees them, in the order asked. A node that
/// is not there, or is deleted, or lies in what is deleted, is left out. A
/// thread is made with the rest of what every node has, and the threads
/// routes add what is its own.
pub async fn build(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    now: DateTime<Utc>,
    policy: Policy,
) -> ApiResult<Vec<Built>> {
    build_with_name_sources(conn, principal, ids, now, policy, None, None, None).await
}

#[allow(
    clippy::too_many_arguments,
    reason = "retain the native node checkpoint and independent name, environment and README source bounds"
)]
async fn build_with_name_sources(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    now: DateTime<Utc>,
    policy: Policy,
    name_sources: Option<&BTreeSet<Id>>,
    topic_sources: Option<&crate::routes::environments::reading::TopicSummarySources>,
    readme_sources: Option<&BTreeSet<Id>>,
) -> ApiResult<Vec<Built>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let standing =
        stand_with_name_sources(conn, principal, ids, policy, true, name_sources).await?;
    let wanted: Vec<Id> = ids
        .iter()
        .copied()
        .filter(|id| standing.sites.contains_key(id))
        .collect();
    let details: HashMap<Id, NodeDetail> = hierarchy::details(conn, &wanted, now)
        .await?
        .into_iter()
        .map(|detail| (detail.id, detail))
        .collect();
    let used: Vec<&NodeDetail> = wanted.iter().filter_map(|id| details.get(id)).collect();
    let extras = extras(conn, &used, principal, now, topic_sources).await?;
    let readmes = Box::pin(readmes_for(
        conn,
        principal,
        &wanted,
        now,
        policy,
        readme_sources,
        name_sources,
    ))
    .await?;

    let mut people: Vec<Id> = Vec::new();
    for detail in &used {
        people.push(detail.owner_id);
        people.extend(detail.archived_by);
    }
    let people = user_refs(conn, &people).await?;
    let mut on_the_ways: Vec<Id> = used
        .iter()
        .flat_map(|detail| detail.path.iter().copied())
        .collect();
    on_the_ways.sort_unstable();
    on_the_ways.dedup();
    let subscriptions: HashMap<Id, SubscriptionLevel> =
        hierarchy::subscriptions_on(conn, principal.user(), &on_the_ways)
            .await?
            .into_iter()
            .filter_map(|row| level(&row.level).map(|level| (row.node_id, level)))
            .collect();

    let mut built = Vec::new();
    for id in wanted {
        let (Some(detail), Some(site)) = (details.get(&id), standing.sites.get(&id)) else {
            continue;
        };
        if detail.deleted_at.is_some() && !orch_db::queries::trace::retained_read(id) {
            continue;
        }
        let Some(owner) = people.get(&detail.owner_id).cloned() else {
            return Err(ApiError::internal(format!(
                "the first owner of {id} is not a user"
            )));
        };
        let path: Vec<NodeRef> = detail
            .path
            .iter()
            .filter_map(|place| standing.refs.get(place).cloned())
            .collect();
        let access = access_of(&standing, site, &detail.path, &subscriptions);
        let common = Common {
            id,
            detail,
            readme: readmes.get(&id).cloned(),
            owner,
            path,
            page: page_state(&standing, site, principal.now()),
            archived: site.is_archived(),
            archived_by: detail.archived_by.and_then(|by| people.get(&by).cloned()),
            access,
            tags: tags_of(detail.id, &extras, &standing),
        };
        let node = match node_kind(&detail.kind) {
            Some(NodeKind::Org) => org(common.base(OrgKind::Org), detail, &extras, name_sources),
            Some(NodeKind::Project) => Node::Project(Project {
                base: common.base(ProjectKind::Project),
                topic_count: extras.topic_counts.get(&id).copied().unwrap_or_default(),
                project_count: extras.project_counts.get(&id).copied().unwrap_or_default(),
            }),
            Some(NodeKind::Topic) => topic(common.base(TopicKind::Topic), detail, &extras),
            Some(NodeKind::Thread) => Node::Thread(
                // The generic node reader carries this branch even when showing an org.
                // Its binding/input/environment futures must not enlarge every node request.
                Box::pin(crate::routes::threads::view::thread_with_name_sources(
                    conn,
                    principal,
                    common.base(ThreadKind::Thread),
                    now,
                    name_sources,
                ))
                .await?,
            ),
            None => continue,
        };
        built.push(Built {
            node,
            viewable: viewable(&standing, site),
        });
    }
    Ok(built)
}

/// The READMEs the nodes `ids` name that are still their own documents and that the caller may read, as a list
/// shows a document, by the node that names each.
async fn readmes_for(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    now: DateTime<Utc>,
    policy: Policy,
    readme_sources: Option<&BTreeSet<Id>>,
    name_sources: Option<&BTreeSet<Id>>,
) -> ApiResult<HashMap<Id, DocumentRef>> {
    let mut rows = orch_db::queries::documents::readmes_of(conn, ids).await?;
    if let Some(allowed) = readme_sources {
        rows.retain(|row| allowed.contains(&row.id));
    }
    if rows.is_empty() {
        return Ok(HashMap::new());
    }
    let documents: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let standing: HashMap<Id, orch_authz::documents::Listed> =
        orch_authz::documents::listed(conn, principal, &documents, policy)
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|each| (each.document.id, each))
            .collect();
    let readable: Vec<(
        orch_db::queries::documents::DocumentRow,
        orch_authz::documents::Listed,
    )> = rows
        .into_iter()
        .filter_map(|row| {
            let held = standing
                .get(&row.id)
                .filter(|each| each.holds(Permission::DocView))?
                .clone();
            Some((row, held))
        })
        .collect();
    let refs = if readme_sources.is_some() {
        let allowed: Vec<_> = name_sources
            .into_iter()
            .flatten()
            .copied()
            .map(crate::routes::documents::build::WrittenLabelSource::Node)
            .collect();
        crate::routes::documents::build::ProjectedDocumentRef::capture_many_allowed(
            conn, principal, &readable, now, policy, &allowed,
        )
        .await?
        .0
    } else {
        crate::routes::documents::build::refs(conn, principal, &readable, now, policy).await?
    };
    Ok(refs
        .into_iter()
        .map(|document| (document.node.id, document))
        .collect())
}

/// What every view of a node shares, ready to be put under its kind.
struct Common<'a> {
    id: Id,
    detail: &'a NodeDetail,
    readme: Option<DocumentRef>,
    owner: UserRef,
    path: Vec<NodeRef>,
    page: PageState,
    archived: bool,
    archived_by: Option<UserRef>,
    access: Access,
    tags: Vec<Tag>,
}

impl Common<'_> {
    fn base<K>(&self, kind: K) -> NodeBase<K> {
        let detail = self.detail;
        NodeBase {
            inspection: None,
            id: self.id,
            kind,
            parent_id: detail.parent_id,
            org_id: detail.org_id,
            path: self.path.clone(),
            name: detail.name.clone(),
            description: detail.description.clone(),
            color: detail.color.and_then(|color| u32::try_from(color).ok()),
            custom_color: detail.custom_color.clone(),
            readme: self.readme.clone(),
            owner: self.owner.clone(),
            owners: detail.owners,
            title_source: source(&detail.title_source),
            description_source: source(&detail.description_source),
            tags: self.tags.clone(),
            tags_source: source(detail.tags_source.as_deref().unwrap_or("auto")),
            auto_updated_at: detail.auto_updated_at.map(time),
            page: self.page,
            chat_id: detail.chat_id.unwrap_or_else(Id::nil),
            archived: self.archived,
            archived_at: detail.archived_at.map(time),
            archived_by: self.archived_by.clone(),
            revision: detail.revision,
            created_at: time(detail.created_at),
            updated_at: time(detail.updated_at),
            access: self.access.clone(),
        }
    }
}

fn org(
    base: NodeBase<OrgKind>,
    detail: &NodeDetail,
    extras: &Extras,
    name_sources: Option<&BTreeSet<Id>>,
) -> Node {
    let facts = extras.orgs.get(&detail.id);
    // A qualified address uses the same admitted ancestor names as its native path.
    let address = if name_sources.is_some()
        && base
            .path
            .iter()
            .any(|node| node.kind == NodeKind::Org && node.id != detail.id && node.redacted)
    {
        String::new()
    } else {
        facts.map(|facts| facts.address.clone()).unwrap_or_default()
    };
    Node::Org(Org {
        base,
        slug: detail.slug.clone().unwrap_or_default(),
        address,
        depth: facts
            .and_then(|facts| u32::try_from(facts.depth).ok())
            .unwrap_or_default(),
        org_kind: if detail.org_kind.as_deref() == Some("personal") {
            OrgType::Personal
        } else {
            OrgType::Team
        },
        closed: detail.closed.unwrap_or_default(),
        flat: detail.flat.unwrap_or_default(),
        member_count: facts.map(|facts| facts.member_count).unwrap_or_default(),
        people_count: facts.map(|facts| facts.people_count),
        org_count: facts.map(|facts| facts.org_count).unwrap_or_default(),
        project_count: facts.map(|facts| facts.project_count).unwrap_or_default(),
        changed_at: time(facts.map_or(detail.updated_at, |facts| facts.changed_at)),
    })
}

fn tags_of(id: Id, extras: &Extras, standing: &Standing<'_>) -> Vec<Tag> {
    extras
        .tags
        .get(&id)
        .into_iter()
        .flatten()
        .map(|tag| Tag {
            id: tag.id,
            node: standing.refs.get(&tag.node_id).cloned().unwrap_or(NodeRef {
                thread_display: None,
                id: tag.node_id,
                kind: NodeKind::Org,
                name: None,
                redacted: true,
            }),
            name: tag.name.clone(),
            color: u32::try_from(tag.color).unwrap_or_default(),
        })
        .collect()
}

fn topic(base: NodeBase<TopicKind>, detail: &NodeDetail, extras: &Extras) -> Node {
    let facts = extras.topics.get(&detail.id);
    let mut environments = extras
        .environments
        .get(&detail.id)
        .cloned()
        .unwrap_or_default();
    let default_environment_id = extras.defaults.get(&detail.id).copied().flatten();
    environments.sort_by_key(|environment| Some(environment.id) != default_environment_id);
    Node::Topic(Topic {
        thread_display_state: extras.thread_display_states.get(&detail.id).copied(),
        base,
        environments,
        default_environment_id,
        status: if detail.topic_status.as_deref() == Some("done") {
            TopicStatus::Done
        } else {
            TopicStatus::Open
        },
        done_at: detail.done_at.map(time),
        thread_count: facts.map(|facts| facts.thread_count).unwrap_or_default(),
        attention: TopicAttention {
            running: facts.map(|facts| facts.running).unwrap_or_default(),
            waiting_approval: facts
                .map(|facts| facts.waiting_approval)
                .unwrap_or_default(),
            blocked: facts.map(|facts| facts.blocked).unwrap_or_default(),
        },
        work_attention: extras
            .work_attention
            .get(&detail.id)
            .cloned()
            .unwrap_or_default(),
        last_activity_at: facts.and_then(|facts| facts.last_activity_at).map(time),
        seq: detail.topic_seq.unwrap_or_default(),
    })
}

/// The nodes `ids` the caller may view, as they see them, in the order
/// asked.
pub async fn visible(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    now: DateTime<Utc>,
    policy: Policy,
) -> ApiResult<Vec<Node>> {
    Ok(build(conn, principal, ids, now, policy)
        .await?
        .into_iter()
        .filter(|built| built.viewable)
        .map(|built| built.node)
        .collect())
}

/// The same batched visible-node producer with already-admitted optional reference sources.
pub(crate) async fn visible_with_read_sources(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    now: DateTime<Utc>,
    names: &BTreeSet<Id>,
    readmes: &BTreeSet<Id>,
) -> ApiResult<Vec<Node>> {
    Ok(build_with_name_sources(
        conn,
        principal,
        ids,
        now,
        Policy::default(),
        Some(names),
        None,
        Some(readmes),
    )
    .await?
    .into_iter()
    .filter(|built| built.viewable)
    .map(|built| built.node)
    .collect())
}

/// The same batched native builder for already-selected visible or retained nodes.
#[allow(
    clippy::too_many_arguments,
    reason = "the native producer accepts separately admitted optional sources"
)]
pub(crate) async fn build_with_read_sources(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    now: DateTime<Utc>,
    policy: Policy,
    names: &BTreeSet<Id>,
    readmes: &BTreeSet<Id>,
) -> ApiResult<Vec<Built>> {
    build_with_name_sources(
        conn,
        principal,
        ids,
        now,
        policy,
        Some(names),
        None,
        Some(readmes),
    )
    .await
}

/// One node as the caller sees it, whether they may view it or not.
pub async fn one(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    now: DateTime<Utc>,
    policy: Policy,
) -> ApiResult<Option<Node>> {
    Ok(Box::pin(build(conn, principal, &[id], now, policy))
        .await?
        .into_iter()
        .next()
        .map(|built| built.node))
}

/// The ordinary view with the producer's additional bounds on current ancestor names.
pub(crate) async fn one_with_name_sources(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    now: DateTime<Utc>,
    policy: Policy,
    name_sources: &BTreeSet<Id>,
    topic_sources: Option<&crate::routes::environments::reading::TopicSummarySources>,
) -> ApiResult<Option<Node>> {
    Ok(Box::pin(build_with_name_sources(
        conn,
        principal,
        &[id],
        now,
        policy,
        Some(name_sources),
        topic_sources,
        None,
    ))
    .await?
    .into_iter()
    .next()
    .map(|built| built.node))
}

/// The same native node projection with this compound read's admitted optional
/// README and environment sources. Ordinary callers retain the unbounded wrapper.
#[allow(
    clippy::too_many_arguments,
    reason = "the same native checkpoint accepts each already-admitted source bound without changing ordinary callers"
)]
pub(crate) async fn one_with_read_sources(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    now: DateTime<Utc>,
    policy: Policy,
    name_sources: &BTreeSet<Id>,
    topic_sources: Option<&crate::routes::environments::reading::TopicSummarySources>,
    readme_sources: &BTreeSet<Id>,
) -> ApiResult<Option<Node>> {
    Ok(Box::pin(build_with_name_sources(
        conn,
        principal,
        &[id],
        now,
        policy,
        Some(name_sources),
        topic_sources,
        Some(readme_sources),
    ))
    .await?
    .into_iter()
    .next()
    .map(|built| built.node))
}

/// The references to the nodes on the way down to each of `ids`, root
/// first, each by name where the caller may know it.
pub async fn ways(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
) -> ApiResult<HashMap<Id, Vec<NodeRef>>> {
    Ok(ways_with_admin(conn, principal, ids, policy).await?.0)
}

/// The same ordinary path projection with a trusted operation bound on live protected names.
pub(crate) async fn ways_with_name_sources(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
    allowed: &BTreeSet<Id>,
) -> ApiResult<HashMap<Id, Vec<NodeRef>>> {
    let standing =
        stand_with_name_sources(conn, principal, ids, policy, true, Some(allowed)).await?;
    Ok(ids
        .iter()
        .filter_map(|id| {
            let node = standing.frame.graph().get(*id)?;
            Some((
                *id,
                node.path
                    .iter()
                    .filter_map(|place| standing.refs.get(place).cloned())
                    .collect(),
            ))
        })
        .collect())
}

/// Readable personal exception paths, using the same View and redaction supplier as node pages.
pub(crate) async fn viewable_ways(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
) -> ApiResult<HashMap<Id, Vec<NodeRef>>> {
    let standing = stand(conn, principal, ids, policy).await?;
    Ok(viewable_paths(&standing, ids))
}

/// The same native readable paths, with protected names bounded by the consuming operation.
pub(crate) async fn viewable_ways_with_name_sources(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
    allowed: &BTreeSet<Id>,
) -> ApiResult<HashMap<Id, Vec<NodeRef>>> {
    let standing =
        stand_with_name_sources(conn, principal, ids, policy, true, Some(allowed)).await?;
    Ok(viewable_paths(&standing, ids))
}

fn viewable_paths(standing: &Standing<'_>, ids: &[Id]) -> HashMap<Id, Vec<NodeRef>> {
    ids.iter()
        .filter_map(|id| {
            let site = standing.sites.get(id)?;
            if !viewable(standing, site) {
                return None;
            }
            let node = standing.frame.graph().get(*id)?;
            Some((
                *id,
                node.path
                    .iter()
                    .filter_map(|place| standing.refs.get(place).cloned())
                    .collect(),
            ))
        })
        .collect()
}

/// The paths and native-only View marks from the very same frame, including live readers.
pub async fn ways_with_admin(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
) -> ApiResult<(
    HashMap<Id, Vec<NodeRef>>,
    HashMap<Id, orch_api_contract::access::AdminAccess>,
)> {
    let standing = stand(conn, principal, ids, policy).await?;
    let paths = ids
        .iter()
        .filter_map(|id| {
            let node = standing.frame.graph().get(*id)?;
            Some((
                *id,
                node.path
                    .iter()
                    .filter_map(|place| standing.refs.get(place).cloned())
                    .collect(),
            ))
        })
        .collect();
    let markers = ids
        .iter()
        .filter_map(|id| {
            let site = standing.sites.get(id)?;
            let marker = admin_access_of(&standing, standing.frame.admin_access(site)?)?;
            Some((*id, marker))
        })
        .collect();
    Ok((paths, markers))
}

/// The nodes `ids` as references, each by name where the caller may know it.
pub async fn refs(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
) -> ApiResult<HashMap<Id, NodeRef>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let standing = stand_with_participation(conn, principal, ids, policy, false).await?;
    Ok(ids
        .iter()
        .filter_map(|id| standing.refs.get(id).map(|node| (*id, node.clone())))
        .collect())
}

/// Uses the same actual graph identity and ordinary visibility, with a trusted operation-level
/// set restricting names and thread-display metadata. Missing names remain typed redacted refs.
pub(crate) async fn refs_with_name_sources(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
    allowed: &BTreeSet<Id>,
) -> ApiResult<HashMap<Id, NodeRef>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let standing =
        stand_with_name_sources(conn, principal, ids, policy, false, Some(allowed)).await?;
    Ok(ids
        .iter()
        .filter_map(|id| standing.refs.get(id).map(|node| (*id, node.clone())))
        .collect())
}

/// Holder names retain the whole visible path, with a private ancestor kept redacted.
pub async fn qualified_refs(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    policy: Policy,
) -> ApiResult<HashMap<Id, NodeRef>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let standing = stand_with_participation(conn, principal, ids, policy, false).await?;
    Ok(ids
        .iter()
        .filter_map(|id| qualified_of(&standing, *id).map(|node| (*id, node)))
        .collect())
}

fn qualified_of(standing: &Standing<'_>, id: Id) -> Option<NodeRef> {
    qualified_from(standing.frame.graph(), &standing.refs, id)
}

/// The existing path formatter over already projected references; callers can preserve their
/// additional name bound without performing another unbounded visibility/name lookup.
pub(crate) fn qualified_from(
    graph: &Graph,
    refs: &HashMap<Id, NodeRef>,
    id: Id,
) -> Option<NodeRef> {
    let node = graph.get(id)?;
    if node
        .path
        .iter()
        .any(|id| graph.get(*id).is_some_and(|node| node.deleted))
    {
        return Some(NodeRef {
            thread_display: None,
            id,
            kind: kind_of(node.kind),
            name: Some("a removed place".to_owned()),
            redacted: false,
        });
    }
    let mut named = refs.get(&id)?.clone();
    if named.name.is_some() {
        named.name = Some(
            node.path
                .iter()
                .map(|part| {
                    refs.get(part)
                        .and_then(|part| part.name.as_deref())
                        .unwrap_or("a private place")
                })
                .collect::<Vec<_>>()
                .join(" > "),
        );
    }
    Some(named)
}

/// The node a node lies in, if it is not a root org.
#[must_use]
pub fn parent_of(node: &Node) -> Option<Id> {
    match node {
        Node::Org(node) => node.base.parent_id,
        Node::Project(node) => node.base.parent_id,
        Node::Topic(node) => node.base.parent_id,
        Node::Thread(node) => node.base.parent_id,
    }
}
