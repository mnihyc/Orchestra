//! Actual native import governors, shared by queued preparation and each object writer.
use super::{ImportError, Object, Plan};
use crate::operation_scope::{sources::Facts, targets};
use orch_access_contract::{
    key_scope::{KeyResolvedTarget, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    import_creation_sources::Source,
    imports::{ImportAction, ImportAuthors},
    package::PackageKind,
};
use orch_authz::engine::Graph;
use orch_db::{Conn, queries::chats};
use orch_types::Id;
use std::collections::{BTreeMap, HashSet};

fn error(value: impl std::fmt::Display) -> ImportError {
    crate::EngineError::unreadable(value).into()
}
fn kind(value: PackageKind) -> KeyTargetKind {
    match value {
        PackageKind::Org | PackageKind::Project => KeyTargetKind::Object(ObjectKind::Project),
        PackageKind::Topic => KeyTargetKind::Object(ObjectKind::Topic),
        PackageKind::Thread => KeyTargetKind::Object(ObjectKind::Thread),
        PackageKind::Document => KeyTargetKind::Object(ObjectKind::Document),
        PackageKind::Folder => KeyTargetKind::Object(ObjectKind::DocumentContainer),
        PackageKind::Chat => {
            KeyTargetKind::Identity(orch_access_contract::key_scope::KeyIdentityKind::Chat)
        }
    }
}
/// Native new-parent ownership is part of this creation; its governing right is the preexisting destination.
/// No generated target id or submitted package ancestry supplies authority.
fn governing_parent(plan: &Plan, object: &Object) -> Result<(Id, Option<Id>), ImportError> {
    let mut node = object.lies_in;
    let mut folder = object.container;
    let mut source = object.object.parent;
    let mut visited = HashSet::new();
    while let Some(id) = source {
        if !visited.insert(id) {
            return Err(error("the native import parent chain contains a cycle"));
        }
        let Some(parent) = plan.get(id) else {
            break;
        };
        if !parent.goes()
            || parent.action == ImportAction::Replace
            || parent.target != folder.unwrap_or(node)
        {
            break;
        }
        node = parent.lies_in;
        folder = parent.container;
        source = parent.object.parent;
    }
    Ok((node, folder))
}
/// Resolve typed existing write governors from the actual plan before any public metadata or mutation.
/// `selected` names native source-object ids; None is the whole actual queued intent.
pub async fn facts(
    conn: &mut Conn,
    plan: &Plan,
    selected: Option<Id>,
) -> Result<Vec<Facts<Source>>, ImportError> {
    let objects = match selected {
        Some(id) => plan
            .get(id)
            .filter(|object| object.goes())
            .into_iter()
            .collect::<Vec<_>>(),
        None => plan
            .objects
            .iter()
            .filter(|object| object.goes())
            .collect::<Vec<_>>(),
    };
    let mut parents = Vec::new();
    let mut doc_ids = Vec::new();
    for object in &objects {
        let (node, folder) = governing_parent(plan, object)?;
        parents.push(node);
        doc_ids.extend(folder);
        if object.action == ImportAction::Replace {
            match object.made_as {
                PackageKind::Document | PackageKind::Folder => doc_ids.push(object.target),
                PackageKind::Chat => {}
                _ => parents.push(object.target),
            }
        }
    }
    parents.push(plan.destination.id);
    parents.sort_unstable();
    parents.dedup();
    doc_ids.sort_unstable();
    doc_ids.dedup();
    let mut graph = Graph::load(conn, &parents).await?;
    let docs = orch_authz::documents::read_into(conn, &mut graph, &doc_ids).await?;
    let chat_rows = chats::chats_of_nodes(conn, &parents).await?;
    let mut writes = BTreeMap::<KeyTargetKind, Vec<KeyResolvedTarget>>::new();
    let mut manages = BTreeMap::<KeyTargetKind, Vec<KeyResolvedTarget>>::new();
    for object in objects {
        let (node, folder) = governing_parent(plan, object)?;
        let target = if object.action == ImportAction::Replace
            && matches!(object.made_as, PackageKind::Document | PackageKind::Folder)
        {
            let expected = if object.made_as == PackageKind::Document {
                ObjectKind::Document
            } else {
                ObjectKind::DocumentContainer
            };
            let row = docs
                .iter()
                .find(|row| row.id == object.target && row.kind == expected && !row.deleted)
                .ok_or(ImportError::NotFound)?;
            let site = orch_authz::documents::site_of(&graph, row).ok_or(ImportError::NotFound)?;
            targets::from_site(&graph, &site).ok_or(ImportError::NotFound)?
        } else if object.action == ImportAction::Replace
            && matches!(
                object.made_as,
                PackageKind::Org | PackageKind::Project | PackageKind::Topic | PackageKind::Thread
            )
        {
            targets::from_node_identity(&graph, object.target).ok_or(ImportError::NotFound)?
        } else if object.made_as == PackageKind::Chat
            && let Some(chat) = chat_rows
                .iter()
                .find(|chat| chat.node_id == Some(object.lies_in))
        {
            targets::from_chat_graph(&graph, chat).map_err(error)?
        } else if let Some(folder) = folder {
            let row = docs
                .iter()
                .find(|row| row.id == folder && !row.deleted)
                .ok_or(ImportError::NotFound)?;
            let site = orch_authz::documents::site_of(&graph, row).ok_or(ImportError::NotFound)?;
            targets::from_site(&graph, &site).ok_or(ImportError::NotFound)?
        } else {
            targets::from_node_identity(&graph, node).ok_or(ImportError::NotFound)?
        };
        if !target
            .node_ancestors
            .iter()
            .any(|parent| parent.id == Some(plan.destination.id))
        {
            return Err(ImportError::NotFound);
        }
        let family = kind(object.made_as);
        writes.entry(family).or_default().push(target.clone());
        let author_accounts = plan.authors == ImportAuthors::Accounts;
        let sharing = plan.access.restored && plan.access_objects.contains(&object.object.id);
        if author_accounts || sharing {
            manages.entry(family).or_default().push(target);
        }
    }
    let mut facts = vec![Facts::collection(
        Source::Destinations,
        writes.into_iter().collect(),
    )];
    if plan.access.restored || plan.authors == ImportAuthors::Accounts {
        facts.push(Facts::collection(
            Source::Delegation,
            manages.into_iter().collect(),
        ));
    }
    Ok(facts)
}

/// The authoritative saved-byte producer feeds both initial preparation and continuing work.
pub fn byte_facts(
    bindings: &[crate::operation_scope::blob_sources::Binding],
) -> Vec<Facts<Source>> {
    use crate::operation_scope::blob_sources::SourceKind;
    use orch_api_contract::upload_sources::Source as Byte;
    [
        (Byte::ReadChats, SourceKind::Chat),
        (Byte::ReadThreads, SourceKind::Thread),
        (Byte::ReadDocuments, SourceKind::Document),
        (Byte::ReadEnvironments, SourceKind::Environment),
        (Byte::ReadUploads, SourceKind::RetainedUpload),
    ]
    .into_iter()
    .map(|(role, kind)| {
        let targets = bindings
            .iter()
            .filter(|binding| binding.source == kind)
            .map(|binding| binding.target.clone())
            .collect::<Vec<_>>();
        Facts::collection(
            Source::Bytes(role),
            if targets.is_empty() {
                Vec::new()
            } else {
                vec![(kind.kind(), targets)]
            },
        )
    })
    .collect()
}
