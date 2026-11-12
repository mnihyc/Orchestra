//! Current typed placement facts shared by ordinary operation producers.
//! Missing graph information fails closed; no paths are accepted from credential claims.

use crate::engine::{Graph, Site};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{
    KeyIdentityKind, KeyObjectRef, KeyResolvedTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_db::queries::{chats, exports, identity, imports};
use orch_db::{Conn, DbError};
use orch_types::Id;
use std::collections::HashMap;

fn reference(kind: KeyTargetKind, id: Id) -> KeyObjectRef {
    KeyObjectRef { kind, id: Some(id) }
}

/// The current typed node path already loaded by the authorization graph. Missing graph
/// facts are a failure, never a partially assembled subtree that happens to admit a key.
pub fn node_path(graph: &Graph, id: Id) -> Option<Vec<KeyObjectRef>> {
    let node = graph.get(id)?;
    node.path
        .iter()
        .map(|id| {
            let ancestor = graph.get(*id)?;
            Some(reference(
                KeyTargetKind::Object(ancestor.kind.into()),
                ancestor.id,
            ))
        })
        .collect()
}

/// Reuse a site that the ordinary producer already loaded. Document containers remain
/// separate from node ancestry. A resource's home comes from its stored site, not from
/// the resource kinds advertised by a creation operation.
pub fn from_site(graph: &Graph, site: &Site) -> Option<KeyResolvedTarget> {
    let node = if site.kind.node().is_some() {
        Some(site.id)
    } else if site.kind.is_document() {
        site.beside
    } else {
        // resources::site_of sets this to the exact stored home, including a thread home.
        site.role_place
    };
    let node_ancestors = match node {
        Some(id) => node_path(graph, id)?,
        None if site.of_system => Vec::new(),
        None => return None,
    };
    let document_ancestors = if site.kind.is_document() {
        graph
            .document_ancestors(site.id)
            .into_iter()
            .map(|id| {
                if !graph.is_document_container(id) {
                    return None;
                }
                Some(reference(
                    KeyTargetKind::Object(ObjectKind::DocumentContainer),
                    id,
                ))
            })
            .collect::<Option<Vec<_>>>()?
    } else {
        Vec::new()
    };
    Some(KeyResolvedTarget {
        credential_account: None,
        object: reference(KeyTargetKind::Object(site.kind), site.id),
        node_ancestors,
        document_ancestors,
    })
}

/// Preserve existing not-found, database and authorization outcomes for transport adapters.
#[derive(Debug, thiserror::Error)]
pub enum TargetError {
    /// The actual object or its complete current placement is unavailable.
    #[error("the operation target is unavailable")]
    Missing,
    /// Existing stored-source query failure.
    #[error(transparent)]
    Db(#[from] DbError),
    /// Existing graph/page/resource lookup outcome, with its original lifecycle semantics.
    #[error(transparent)]
    Authz(#[from] crate::Error),
}

/// An existing node and its current ordinary Graph/Site placement.
pub async fn node(conn: &mut Conn, id: Id) -> Result<KeyResolvedTarget, TargetError> {
    let loaded = Graph::load(conn, &[id]).await;
    let graph = loaded?;
    let site = graph.site_of(id).ok_or(TargetError::Missing)?;
    from_site(&graph, &site).ok_or(TargetError::Missing)
}

/// Current typed identity/path, including a retained node; this grants no inspection authority.
pub async fn node_identity(conn: &mut Conn, id: Id) -> Result<KeyResolvedTarget, TargetError> {
    let loaded = Graph::load(conn, &[id]).await;
    let graph = loaded?;
    from_node_identity(&graph, id).ok_or(TargetError::Missing)
}

/// The same current retained identity/path from an already loaded graph. Batch
/// projections reuse these facts without guessing a kind or re-querying each node.
pub fn from_node_identity(graph: &Graph, id: Id) -> Option<KeyResolvedTarget> {
    let actual = graph.get(id)?;
    Some(KeyResolvedTarget {
        object: reference(KeyTargetKind::Object(actual.kind.into()), id),
        node_ancestors: node_path(graph, id)?,
        document_ancestors: Vec::new(),
        credential_account: None,
    })
}

/// Current stored resource identity/path for an already selected native context reference.
/// Includes retained rows without granting live Get, inspection, restore or mutation authority.
pub fn from_resource_identity(
    graph: &Graph,
    resource: &crate::resources::Resource,
) -> Option<KeyResolvedTarget> {
    Some(KeyResolvedTarget {
        object: reference(KeyTargetKind::Object(resource.kind.into()), resource.id),
        node_ancestors: match resource.home {
            Some(home) => node_path(graph, home)?,
            None => Vec::new(),
        },
        document_ancestors: Vec::new(),
        credential_account: None,
    })
}

/// An exact existing permission-page object with the ordinary reader's liveness rules.
pub async fn page_object(
    conn: &mut Conn,
    object: ObjectRef,
) -> Result<KeyResolvedTarget, TargetError> {
    let target = if object.kind.node().is_some() {
        node(conn, object.id).await?
    } else if object.kind.is_document() {
        let loaded = crate::documents::site_for_page(conn, object.id).await;
        let (graph, site) = loaded?;
        from_site(&graph, &site).ok_or(TargetError::Missing)?
    } else {
        let loaded = crate::resources::site(conn, object).await;
        let (graph, resource, site) = loaded?;
        if resource.deleted {
            return Err(TargetError::Missing);
        }
        from_site(&graph, &site).ok_or(TargetError::Missing)?
    };
    if target.object != reference(KeyTargetKind::Object(object.kind), object.id) {
        return Err(TargetError::Missing);
    }
    Ok(target)
}

/// The same live resource-page subjects as `page_object`, resolved in one native batch.
/// Every requested kind must match its actual row; absent/deleted/home-less facts fail closed.
pub async fn resource_objects(
    conn: &mut Conn,
    objects: &[ObjectRef],
) -> Result<Vec<KeyResolvedTarget>, TargetError> {
    if objects.is_empty() {
        return Ok(Vec::new());
    }
    if objects
        .iter()
        .any(|object| object.kind.node().is_some() || object.kind.is_document())
    {
        return Err(TargetError::Missing);
    }
    let ids: Vec<_> = objects.iter().map(|object| object.id).collect();
    let mut graph = Graph::default();
    let rows = crate::resources::read_into(conn, &mut graph, &ids).await?;
    let rows: HashMap<_, _> = rows.into_iter().map(|row| (row.object(), row)).collect();
    objects
        .iter()
        .map(|object| {
            let row = rows
                .get(object)
                .filter(|row| !row.deleted)
                .ok_or(TargetError::Missing)?;
            let site = crate::resources::site_of(&graph, row).ok_or(TargetError::Missing)?;
            from_site(&graph, &site).ok_or(TargetError::Missing)
        })
        .collect()
}

/// The exact stored resource identity and its complete current home, including retained rows.
/// This grants no page visibility, restoration or runtime access. Custody readers whose ordinary
/// permission survives removal use these facts before applying their unchanged ordinary door.
pub async fn resource_identity(
    conn: &mut Conn,
    object: ObjectRef,
) -> Result<KeyResolvedTarget, TargetError> {
    let mut graph = Graph::default();
    let rows = crate::resources::read_into(conn, &mut graph, &[object.id]).await?;
    let resource = rows
        .into_iter()
        .find(|row| row.object() == object)
        .ok_or(TargetError::Missing)?;
    let node_ancestors = match resource.home {
        Some(home) => node_path(&graph, home).ok_or(TargetError::Missing)?,
        None if resource.of_system() => Vec::new(),
        None => return Err(TargetError::Missing),
    };
    Ok(KeyResolvedTarget {
        object: reference(KeyTargetKind::Object(object.kind), object.id),
        node_ancestors,
        document_ancestors: Vec::new(),
        credential_account: None,
    })
}

/// The existing physical machine permission subject: its Host facade when present,
/// otherwise its personal Machine page. Keep the subject's actual identity/home;
/// the operation separately retains Machine as its business resource family.
pub async fn machine_subject(
    conn: &mut Conn,
    machine: Id,
) -> Result<KeyResolvedTarget, TargetError> {
    let host = orch_db::queries::resources::hosting::host_of_machine(conn, machine).await?;
    page_object(conn, machine_object(machine, host)).await
}

fn machine_object(machine: Id, host: Option<Id>) -> ObjectRef {
    match host {
        Some(id) => ObjectRef {
            kind: ObjectKind::Host,
            id,
        },
        None => ObjectRef {
            kind: ObjectKind::Machine,
            id: machine,
        },
    }
}

/// The same live permission subjects in one bulk Graph/Site read. Callers decide which
/// physical references actually exist; a missing/deleted canonical page is never silently
/// converted to a personal-machine subject or an absent response reference.
pub async fn machine_subjects(
    conn: &mut Conn,
    machines: &[Id],
) -> Result<HashMap<Id, KeyResolvedTarget>, TargetError> {
    machine_targets(conn, machines, false).await
}

/// The same canonical physical subjects as identity facts for already-authorized native refs.
/// Retention does not turn this into live Machine Get or private inspection authority.
pub async fn machine_identities(
    conn: &mut Conn,
    machines: &[Id],
) -> Result<HashMap<Id, KeyResolvedTarget>, TargetError> {
    machine_targets(conn, machines, true).await
}

async fn machine_targets(
    conn: &mut Conn,
    machines: &[Id],
    identities: bool,
) -> Result<HashMap<Id, KeyResolvedTarget>, TargetError> {
    if machines.is_empty() {
        return Ok(HashMap::new());
    }
    let facades: HashMap<_, _> =
        orch_db::queries::resources::hosting::machine_hosts(conn, machines)
            .await?
            .into_iter()
            .map(|(machine, host, _)| (machine, host))
            .collect();
    let subjects: Vec<_> = machines
        .iter()
        .map(|machine| {
            (
                *machine,
                machine_object(*machine, facades.get(machine).copied()),
            )
        })
        .collect();
    let ids: Vec<_> = subjects.iter().map(|(_, object)| object.id).collect();
    let mut graph = Graph::default();
    let rows = crate::resources::read_into(conn, &mut graph, &ids).await?;
    let rows: HashMap<_, _> = rows.into_iter().map(|row| (row.object(), row)).collect();
    let mut targets = HashMap::new();
    for (machine, object) in subjects {
        let row = rows
            .get(&object)
            .filter(|row| identities || !row.deleted)
            .ok_or(TargetError::Missing)?;
        let target = if identities {
            from_resource_identity(&graph, row).ok_or(TargetError::Missing)?
        } else {
            let site = crate::resources::site_of(&graph, row).ok_or(TargetError::Missing)?;
            from_site(&graph, &site).ok_or(TargetError::Missing)?
        };
        targets.insert(machine, target);
    }
    Ok(targets)
}

/// Stored chat identity and actual node/org placement, preserving Chat as its own kind.
pub async fn from_chat(
    conn: &mut Conn,
    chat: &chats::ChatRow,
) -> Result<KeyResolvedTarget, TargetError> {
    let place = chat.home();
    let graph = Graph::load(conn, &[place]).await?;
    from_chat_graph(&graph, chat)
}

/// The same live Chat subject from a native producer's already loaded current graph.
pub fn from_chat_graph(
    graph: &Graph,
    chat: &chats::ChatRow,
) -> Result<KeyResolvedTarget, TargetError> {
    let place = chat.home();
    let site = graph.site_of(place).ok_or(TargetError::Missing)?;
    let mut target = from_site(graph, &site).ok_or(TargetError::Missing)?;
    if chat.kind == chats::KIND_DIRECT
        && target.object.kind != KeyTargetKind::Object(ObjectKind::Org)
    {
        return Err(TargetError::Missing);
    }
    target.object = reference(KeyTargetKind::Identity(KeyIdentityKind::Chat), chat.id);
    Ok(target)
}

/// Read the same stored chat used by ordinary chat producers.
pub async fn chat(conn: &mut Conn, id: Id) -> Result<KeyResolvedTarget, TargetError> {
    let row = chats::chat(conn, id).await?.ok_or(TargetError::Missing)?;
    from_chat(conn, &row).await
}

/// The retained chat's actual keeping node/org and current ancestry. This is identity evidence
/// for already accepted work; it neither reads message contents nor grants chat visibility.
pub async fn chat_identity(conn: &mut Conn, id: Id) -> Result<KeyResolvedTarget, TargetError> {
    let row = chats::chat(conn, id).await?.ok_or(TargetError::Missing)?;
    let mut target = node_identity(conn, row.home()).await?;
    if row.kind == chats::KIND_DIRECT
        && target.object.kind != KeyTargetKind::Object(ObjectKind::Org)
    {
        return Err(TargetError::Missing);
    }
    target.object = reference(KeyTargetKind::Identity(KeyIdentityKind::Chat), row.id);
    Ok(target)
}

/// An account has placement only when it is a service account with an actual owning org.
pub async fn from_account(
    conn: &mut Conn,
    account: &identity::UserRow,
) -> Result<KeyResolvedTarget, TargetError> {
    let graph = match account.service_org_id {
        Some(org) => Graph::load(conn, &[org]).await?,
        None => Graph::default(),
    };
    from_account_identity(&graph, account.id, &account.kind, account.service_org_id)
        .ok_or(TargetError::Missing)
}

/// The same Account identity facts after a producer loaded actual owner rows
/// and keeping-org ancestry in batches. This grants no account visibility or authority.
pub fn from_account_identity(
    graph: &Graph,
    id: Id,
    kind: &str,
    service_org: Option<Id>,
) -> Option<KeyResolvedTarget> {
    let node_ancestors = match (kind, service_org) {
        ("service", Some(org)) => {
            // Keeping-org placement stays factual for retained/disabled service accounts;
            // the ordinary account producer still owns lifecycle and visibility decisions.
            let owner = from_node_identity(graph, org)?;
            if owner.object.kind != KeyTargetKind::Object(ObjectKind::Org) {
                return None;
            }
            owner.node_ancestors
        }
        ("human", None) => Vec::new(),
        _ => return None,
    };
    Some(KeyResolvedTarget {
        object: reference(KeyTargetKind::Identity(KeyIdentityKind::Account), id),
        node_ancestors,
        document_ancestors: Vec::new(),
        credential_account: None,
    })
}

/// Read the same stored account identity, granting no access to private account fields.
pub async fn account(conn: &mut Conn, id: Id) -> Result<KeyResolvedTarget, TargetError> {
    let row = identity::user_by_id(conn, id)
        .await?
        .ok_or(TargetError::Missing)?;
    from_account(conn, &row).await
}

fn artifact_identity(
    graph: &Graph,
    kind: KeyIdentityKind,
    id: Id,
    node: Option<Id>,
) -> Option<KeyResolvedTarget> {
    Some(KeyResolvedTarget {
        object: reference(KeyTargetKind::Identity(kind), id),
        node_ancestors: match node {
            Some(node) => node_path(graph, node)?,
            None => Vec::new(),
        },
        document_ancestors: Vec::new(),
        credential_account: None,
    })
}

/// Actual archive identity and its currently stored place, including a retained place.
/// A purged place leaves no current ancestry; request JSON and creator membership never
/// supply replacement placement. The native producer still checks archive custody.
pub fn from_export_graph(graph: &Graph, row: &exports::ExportRow) -> Option<KeyResolvedTarget> {
    artifact_identity(graph, KeyIdentityKind::Export, row.id, row.node_id)
}

/// The same archive facts on the caller's native current checkpoint connection.
pub async fn from_export(
    conn: &mut Conn,
    row: &exports::ExportRow,
) -> Result<KeyResolvedTarget, TargetError> {
    let graph = Graph::load(conn, &row.node_id.into_iter().collect::<Vec<_>>()).await?;
    from_export_graph(&graph, row).ok_or(TargetError::Missing)
}

/// Actual import identity and its stored destination, without reopening imported objects,
/// borrowing old plan ancestry or turning the creator into an Account ancestor.
pub fn from_import_graph(graph: &Graph, row: &imports::ImportRow) -> Option<KeyResolvedTarget> {
    artifact_identity(graph, KeyIdentityKind::Import, row.id, row.node_id)
}

/// The native owner-filtered history row carries the same factual Import identity and
/// nullable destination without loading its request, files or retained loss payloads.
pub fn from_import_line_graph(
    graph: &Graph,
    row: &imports::ImportLineRow,
) -> Option<KeyResolvedTarget> {
    artifact_identity(graph, KeyIdentityKind::Import, row.id, row.node_id)
}

/// The same import facts on the caller's native current checkpoint connection.
pub async fn from_import(
    conn: &mut Conn,
    row: &imports::ImportRow,
) -> Result<KeyResolvedTarget, TargetError> {
    let graph = Graph::load(conn, &row.node_id.into_iter().collect::<Vec<_>>()).await?;
    from_import_graph(&graph, row).ok_or(TargetError::Missing)
}

/// The single system identity is the only target without an object ID.
pub fn system() -> KeyResolvedTarget {
    KeyResolvedTarget {
        object: KeyObjectRef {
            kind: KeyTargetKind::Identity(KeyIdentityKind::System),
            id: None,
        },
        node_ancestors: Vec::new(),
        document_ancestors: Vec::new(),
        credential_account: None,
    }
}

#[cfg(test)]
#[path = "key_targets/artifact_tests.rs"]
mod artifact_tests;
