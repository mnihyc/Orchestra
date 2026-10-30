//! Exact access-engine projections for SQL hierarchy pages, without expanding ordinary threads into memory.

use std::collections::{HashMap, HashSet};

use orch_access_contract::decisions::Held;
use orch_access_contract::permissions::{Level, NodePermission, ObjectKind, Permission, Placement};
use orch_access_contract::roles::{NodeKind, PlaceRef};
use orch_db::{
    Conn,
    queries::{
        chats,
        everything::{self as rows, Grant},
    },
};
use orch_types::{Id, Json};
use serde_json::json;

use crate::{Error, Frame, Policy, Principal, Site, documents, engine::Graph, resources};

/// A finder-compatible path step; the API turns its kind into a public node reference.
#[derive(Debug, Clone)]
pub struct Step {
    /// The node.
    pub id: Id,
    /// Its kind.
    pub kind: NodeKind,
    /// Its name only when the finder would reveal it.
    pub name: Option<String>,
}

/// Decisions prepared in the caller's snapshot and then applied by the listing statement.
pub struct Projection {
    /// Resource standings from this same frame, reused by bounded typed list views.
    pub resources: Vec<resources::Seen>,
    /// Decisions indexed by actual page and by a thread's parent and environment profile.
    pub grants: Json,
    /// The paths' authorized names, including deliberately redacted steps.
    pub names: HashMap<Id, Step>,
    /// The roots in which the caller's Owners entries count.
    pub member_roots: Vec<Id>,
}

impl Projection {
    /// A conservative indexed entrance; actual parts still require their exact current frame.
    #[must_use]
    pub fn search_candidates(&self, principal: &Principal) -> Json {
        fn readable(value: &Json) -> bool {
            match value {
                Json::Object(map) => {
                    map.get("view").and_then(Json::as_bool) == Some(true)
                        || map
                            .iter()
                            .any(|(key, value)| key != "owned" && readable(value))
                }
                Json::Array(values) => values.iter().any(readable),
                _ => false,
            }
        }
        fn paths(value: &Json, chat: bool) -> bool {
            match value {
                Json::Object(map) => map.iter().any(|(key, value)| {
                    key != "owned"
                        && (if chat && key == "chat" {
                            readable(value)
                        } else if !chat && key == "view" {
                            value.as_bool() == Some(true)
                        } else if key == "chat" || key == "view" {
                            false
                        } else {
                            paths(value, chat)
                        })
                }),
                Json::Array(values) => values.iter().any(|value| paths(value, chat)),
                _ => false,
            }
        }
        let ids = |key: &str, chat: bool| -> HashSet<Id> {
            self.grants
                .get(key)
                .and_then(Json::as_object)
                .into_iter()
                .flatten()
                .filter(|(_, value)| paths(value, chat))
                .filter_map(|(id, _)| id.parse().ok())
                .collect()
        };
        let documents = ids("document_defaults", false);
        let mut threads = ids("threads", false);
        let chat_scopes = ids("threads", true);
        let mut names = HashSet::new();
        let mut chat_nodes = HashSet::new();
        let mut explicit = HashSet::new();
        for (id, grant) in self
            .grants
            .get("objects")
            .and_then(Json::as_object)
            .into_iter()
            .flatten()
        {
            let Ok(id) = id.parse::<Id>() else { continue };
            if let Some(node) = self.names.get(&id) {
                if grant.get("view").and_then(Json::as_bool) == Some(true) {
                    names.insert(id);
                    if node.kind == NodeKind::Thread {
                        threads.insert(id);
                    }
                }
                if grant.get("chat").is_some_and(readable) {
                    chat_nodes.insert(id);
                }
            } else if grant.get("view").and_then(Json::as_bool) == Some(true) {
                explicit.insert(id);
            }
        }
        let scopes: HashSet<Id> = documents
            .iter()
            .chain(&threads)
            .chain(&chat_scopes)
            .copied()
            .collect();
        json!({"scopes":scopes,"doc_scopes":documents,"thread_scopes":threads,"chat_scopes":chat_scopes,
            "chat_nodes":chat_nodes,"names":names,"documents":explicit,"chats":explicit,
            "person":principal.user(),"groups":principal.groups()})
    }
}

// Indexed search projects content View and its covering rows; edit/share controls use the ordinary projection.
const SEARCH_READ_PERMISSIONS: &[Permission] = &[
    Permission::OrgView,
    Permission::ProjectView,
    Permission::TopicView,
    Permission::ThreadView,
    Permission::DocView,
    Permission::ChatView,
    Permission::EnvironmentView,
];

fn main_view(kind: ObjectKind) -> Option<Permission> {
    match kind {
        ObjectKind::Document | ObjectKind::DocumentContainer => Some(Permission::DocView),
        kind => kind
            .node()
            .and_then(|kind| NodePermission::View.on(kind))
            .or_else(|| {
                kind.resource().and_then(|resource| {
                    kind.permissions().find(|permission| {
                        permission.level() == Level::View && permission.area() == resource.area()
                    })
                })
            }),
    }
}

fn level(held: &[Held], area: orch_access_contract::permissions::Area) -> &'static str {
    let levels: Vec<Level> = held
        .iter()
        .filter(|each| each.permission.area() == area)
        .map(|each| each.permission.level())
        .collect();
    if levels.contains(&Level::Manage) || levels.contains(&Level::Own) {
        "manage"
    } else if levels
        .iter()
        .any(|level| matches!(level, Level::Interact | Level::Edit | Level::Use))
    {
        "take_part"
    } else {
        "view"
    }
}

fn grant(frame: &Frame<'_>, site: &Site, owned: bool) -> Grant {
    let held = if owned {
        frame.effective_owned(site)
    } else {
        frame.effective(site)
    };
    let has = |permission| held.iter().any(|each| each.permission == permission);
    let view = main_view(site.kind).is_some_and(has);
    let mut result = Grant {
        admin_access: if owned {
            None
        } else {
            frame.admin_access(site)
        },
        participating: owned || frame.participates(site),
        view,
        listed: view || (site.kind.resource().is_some() && !held.is_empty()),
        named: view
            || frame.holds_any(site)
            || (owned && !held.is_empty())
            || (site.kind == ObjectKind::Org
                && site
                    .root
                    .is_some_and(|root| frame.principal().is_in_tree(root))),
        access: if view {
            level(
                &held,
                main_view(site.kind)
                    .map(Permission::area)
                    .or_else(|| site.kind.resource().map(|kind| kind.area()))
                    .unwrap_or(orch_access_contract::permissions::Area::Access),
            )
        } else {
            "path_only"
        }
        .to_owned(),
        // Holder-sharing policy is resolved by the route for just the rows it returns.
        share: view && (has(Permission::AccessManage) || frame.owns(site) || owned),
        settings: view && has(Permission::SettingsManage),
        chat: None,
    };
    if site.kind.node().is_some() {
        let chat_view = has(Permission::ChatView);
        result.chat = Some(Box::new(Grant {
            admin_access: if owned {
                None
            } else {
                frame.admin_access_for(site, Permission::ChatView)
            },
            participating: owned || frame.participates_chat(site),
            view: chat_view,
            listed: chat_view,
            named: chat_view,
            access: if !chat_view {
                "path_only"
            } else if has(Permission::ChatManage) {
                "manage"
            } else if has(Permission::ChatInteract) {
                "take_part"
            } else {
                "view"
            }
            .to_owned(),
            share: false,
            settings: false,
            chat: None,
        }));
    }
    result
}

// Content search consumes View and chat View. The frame still resolves their ordinary covering levels.
fn search_grant(frame: &Frame<'_>, site: &Site, owned: bool, names: bool) -> Grant {
    let view_permission = main_view(site.kind);
    let has = |permission| frame.has_permission(site, permission, owned);
    let view = view_permission.is_some_and(has);
    let mut result = Grant {
        admin_access: (!owned).then(|| frame.admin_access(site)).flatten(),
        participating: owned || frame.participates(site),
        view,
        listed: view,
        named: view
            || (names && frame.holds_any(site))
            || (site.kind == ObjectKind::Org
                && site
                    .root
                    .is_some_and(|root| frame.principal().is_in_tree(root))),
        access: if view { "view" } else { "path_only" }.to_owned(),
        ..Grant::default()
    };
    if site.kind.node().is_some() {
        let chat_view = has(Permission::ChatView);
        result.chat = Some(Box::new(Grant {
            admin_access: (!owned)
                .then(|| frame.admin_access_for(site, Permission::ChatView))
                .flatten(),
            participating: owned || frame.participates_chat(site),
            view: chat_view,
            listed: chat_view,
            named: chat_view,
            access: if chat_view { "view" } else { "path_only" }.to_owned(),
            ..Grant::default()
        }));
    }
    result
}

/// Prepares visibility using the very same sites, frame, owners, defaults, locks and credential rules
/// as the tree and finder. Only exceptional thread pages are read: ordinary threads share their
/// parent's two inherited projections. The SQL query chooses the owner's projection only for a
/// live matching Owners entry in a root the caller belongs to.
pub async fn project(
    conn: &mut Conn,
    principal: &Principal,
    parent: Option<Id>,
) -> Result<Projection, Error> {
    let seeds = rows::access_seeds(conn, parent).await?;
    project_seeds(conn, principal, seeds, false, false, true).await
}

/// Prepares the same exact frame for an indexed search's selected content, without reading every document inventory.
pub async fn project_search(
    conn: &mut Conn,
    principal: &Principal,
    parent: Option<Id>,
    documents: &[Id],
    chats: &[Id],
    nodes: &[Id],
) -> Result<Projection, Error> {
    let seeds = rows::search_seeds(conn, parent, documents, chats, nodes).await?;
    let hints = documents.is_empty() && chats.is_empty() && nodes.is_empty();
    project_seeds(conn, principal, seeds, true, true, hints).await
}

/// Derives complete small-scope hints or declines them before loading a large access graph.
pub async fn project_search_scope(
    conn: &mut Conn,
    principal: &Principal,
    parent: Option<Id>,
    budget: i64,
) -> Result<Option<Projection>, Error> {
    if budget <= 0 {
        return Ok(None);
    }
    let seeds =
        rows::search_seeds_window(conn, parent, &[], &[], &[], Some(budget.saturating_add(1)))
            .await?;
    if i64::try_from(seeds.len()).unwrap_or(i64::MAX) > budget {
        return Ok(None);
    }
    project_seeds(conn, principal, seeds, true, true, true)
        .await
        .map(Some)
}

async fn project_seeds(
    conn: &mut Conn,
    principal: &Principal,
    seeds: Vec<rows::AccessSeed>,
    document_hints: bool,
    searching: bool,
    include_inherited: bool,
) -> Result<Projection, Error> {
    let mut node_ids: Vec<Id> = seeds
        .iter()
        .filter(|seed| seed.kind == "node")
        .map(|seed| seed.id)
        .collect();
    if let Some(subtree) = principal
        .caller()
        .grant()
        .and_then(crate::TokenGrant::subtree)
    {
        node_ids.push(subtree);
    }
    let phase = std::time::Instant::now();
    let mut graph = Graph::load(conn, &node_ids).await?;
    let docs: Vec<Id> = seeds
        .iter()
        .filter(|seed| seed.kind == "document")
        .map(|seed| seed.id)
        .collect();
    let docs = documents::read_into(conn, &mut graph, &docs).await?;
    let resource_ids: Vec<Id> = seeds
        .iter()
        .filter(|seed| {
            matches!(
                seed.kind.as_str(),
                "environment"
                    | "provider"
                    | "agent"
                    | "machine"
                    | "host"
                    | "template"
                    | "mcp_server"
                    | "mount"
                    | "skill"
            )
        })
        .map(|seed| seed.id)
        .collect();
    let resources = resources::read_into(conn, &mut graph, &resource_ids).await?;
    let direct_ids: Vec<Id> = seeds
        .iter()
        .filter(|seed| seed.kind == "chat")
        .map(|seed| seed.id)
        .collect();
    let direct_chats = chats::chats(conn, &direct_ids).await?;
    let participants =
        orch_db::queries::search::participant_chats(conn, principal.user(), &direct_ids).await?;
    let mut sites: Vec<Site> = graph
        .nodes()
        .filter_map(|node| graph.site_of(node.id))
        .collect();
    sites.extend(
        docs.iter()
            .filter_map(|doc| documents::site_of(&graph, doc)),
    );
    sites.extend(
        resources
            .iter()
            .filter_map(|resource| resources::site_of(&graph, resource)),
    );
    let mut decision_sites = sites.clone();
    for topic in graph
        .nodes()
        .filter(|node| include_inherited && node.kind == NodeKind::Topic)
    {
        let Some(mut child) = graph.site_of(topic.id) else {
            continue;
        };
        child.id = Id::nil();
        child.kind = ObjectKind::Thread;
        child.first_owner = None;
        child.chain.insert(0, PlaceRef::Node(topic.id));
        child.admin_places.insert(0, topic.id);
        child.role_place = Some(topic.id);
        child.path = Some(topic.path.iter().copied().chain([Id::nil()]).collect());
        for placement in [Placement::Managed, Placement::Connected] {
            child.placement = placement;
            for archived in [
                graph.site_of(topic.id).and_then(|site| site.archived_node),
                Some(topic.id),
            ] {
                child.archived_node = archived;
                decision_sites.push(child.clone());
            }
        }
    }
    // A document with no private page follows these exact node/default chains. These are candidate
    // hints only: each actual document still gets its own frame before any payload is returned.
    let document_sites: Vec<Site> = graph
        .nodes()
        .filter(|_| document_hints && include_inherited)
        .filter_map(|node| {
            documents::site_of(
                &graph,
                &documents::Document {
                    kind: ObjectKind::Document,
                    id: Id::nil(),
                    node: node.id,
                    root: Id::nil(),
                    owner: Id::nil(),
                    title: String::new(),
                    deleted: false,
                    made_by_thread: None,
                    made_by_agent: None,
                },
            )
        })
        .collect();
    decision_sites.extend(document_sites.iter().cloned());
    if document_hints {
        tracing::debug!(target: "orch::search::timing", phase="access_graph", elapsed_ms=phase.elapsed().as_secs_f64()*1000., sites=decision_sites.len());
    }
    let phase = std::time::Instant::now();
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        &decision_sites,
        if document_hints {
            SEARCH_READ_PERMISSIONS
        } else {
            Permission::ALL
        },
    )
    .await?;
    if document_hints {
        tracing::debug!(target: "orch::search::timing", phase="access_frame", elapsed_ms=phase.elapsed().as_secs_f64()*1000.);
    }
    let phase = std::time::Instant::now();
    let projected = |site: &Site, owned, names| {
        if document_hints {
            search_grant(&frame, site, owned, names)
        } else {
            grant(&frame, site, owned)
        }
    };
    let document_defaults: serde_json::Map<String, Json> = document_sites.iter().filter_map(|site| site.beside.map(|node| (
        node.to_string(), json!({"view": projected(site, false, false).view, "owned": projected(site, true, false).view})
    ))).collect();
    let mut objects = serde_json::Map::new();
    let mut names = HashMap::new();
    let mut reachable = HashSet::new();
    for site in &sites {
        let held = projected(site, false, true);
        if site.kind.node().is_some() {
            names.insert(
                site.id,
                Step {
                    id: site.id,
                    kind: site.kind.node().unwrap_or(NodeKind::Org),
                    name: held.named.then(|| site.name.clone()),
                },
            );
        }
        if (held.view || held.chat.as_ref().is_some_and(|chat| chat.view))
            && site.kind.resource().is_none()
        {
            reachable.extend(site.path.iter().flatten().copied());
        }
        objects.insert(
            site.id.to_string(),
            serde_json::to_value(held).map_err(|error| Error::Stored(error.to_string()))?,
        );
    }
    let mut inherited = serde_json::Map::new();
    for topic in frame
        .graph()
        .nodes()
        .filter(|node| include_inherited && node.kind == NodeKind::Topic)
    {
        let Some(place) = frame.graph().site_of(topic.id) else {
            continue;
        };
        let mut child = place.clone();
        child.id = Id::nil();
        child.kind = ObjectKind::Thread;
        child.first_owner = None;
        child.chain.insert(0, PlaceRef::Node(topic.id));
        child.admin_places.insert(0, topic.id);
        child.role_place = Some(topic.id);
        child.path = Some(topic.path.iter().copied().chain([Id::nil()]).collect());
        let mut profiles = serde_json::Map::new();
        for placement in [Placement::Managed, Placement::Connected] {
            child.placement = placement;
            child.archived_node = place.archived_node;
            let live = json!({"ordinary": projected(&child, false, false), "owned": projected(&child, true, false)});
            child.archived_node = Some(topic.id);
            let archived = json!({"ordinary": projected(&child, false, false), "owned": projected(&child, true, false)});
            profiles.insert(
                placement.as_str().to_owned(),
                json!({"live":live,"archived":archived}),
            );
        }
        inherited.insert(topic.id.to_string(), Json::Object(profiles));
    }
    if document_hints {
        tracing::debug!(target: "orch::search::timing", phase="access_projection", elapsed_ms=phase.elapsed().as_secs_f64()*1000.);
    }
    let inspected_chats = crate::chats::people_chat_access_in(conn, &frame, &direct_ids).await?;
    for chat in direct_chats {
        let member = participants.iter().find(|each| each.id == chat.id);
        let root = frame.graph().site_of(chat.org_id);
        // Exactly the finder's participant rule: inspection rights never create a name hit.
        let member_access = member.is_some_and(|member| {
            !principal.suspension().applies(&[member.org_id])
                && (principal.is_in_tree(member.org_id) || member.guest)
                && principal.caller().permits(Permission::ChatView)
                && principal.caller().reaches(Some(&[member.org_id]))
        });
        let inspection = inspected_chats.get(&chat.id);
        let native = inspection.is_some_and(|access| access.admin_place.is_some());
        let view = member_access || (!searching && inspection.is_some());
        let participating = member_access && frame.follows_chat(chat.id);
        let archived = root.as_ref().is_some_and(Site::is_archived);
        if view {
            reachable.extend(
                root.iter()
                    .flat_map(|site| site.path.iter().flatten().copied()),
            );
        }
        objects.insert(
            chat.id.to_string(),
            json!(Grant {
                admin_access: if member_access {
                    None
                } else {
                    inspection.and_then(|access| access.admin_access)
                },
                participating,
                view,
                listed: view,
                named: member_access,
                access: if native && !archived {
                    "manage"
                } else if view && !archived && principal.caller().permits(Permission::ChatInteract)
                {
                    "take_part"
                } else if view {
                    "view"
                } else {
                    "path_only"
                }
                .to_owned(),
                share: false,
                settings: false,
                chat: None,
            }),
        );
    }
    let environment_columns: std::collections::BTreeMap<String, &str> = frame
        .graph()
        .environment_profiles()
        .map(|(id, column)| (id.to_string(), column.as_str()))
        .collect();
    let resources = resources
        .into_iter()
        .filter_map(|resource| {
            let site = resources::site_of(frame.graph(), &resource)?;
            if principal.suspension().applies(&site.orgs) {
                return None;
            }
            let held = frame.effective(&site);
            if held.is_empty() {
                return None;
            }
            Some(resources::Seen {
                admin_place: frame.admin_place(&site),
                admin_only: frame.admin_only(&site),
                admin_access: frame.admin_access(&site),
                participating: frame.participates(&site),
                owner: frame.owns(&site),
                administers: frame.administers(&site),
                resource,
                held,
            })
        })
        .collect();
    Ok(Projection {
        resources,
        grants: json!({"document_defaults":document_defaults,"objects":objects,"threads":inherited,"environments":environment_columns,"reachable":reachable,"system_admin":principal.is_system_admin(),"machines":principal.caller().reaches(None),
            "participating_nodes":frame.participation_ids(orch_access_contract::pages::StoredKind::Node),
            "participating_chats":frame.participation_ids(orch_access_contract::pages::StoredKind::Chat)}),
        names,
        member_roots: principal.memberships().roots(),
    })
}
