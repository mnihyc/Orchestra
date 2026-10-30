//! What a reader is told of the things a list names: each one's name as they may read it, and the places it lies in.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::routes::documents::build::Cast;
use crate::routes::nodes::view;
use orch_api_contract::documents::InlineRefKind;
use orch_api_contract::search::{RelatedAncestor, RelatedKind};
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_db::queries::chats as chat_rows;
use orch_db::queries::related as related_rows;
use orch_documents::links::Names;
use orch_types::Id;
use std::collections::{HashMap, HashSet};

/// A readable thing's name and the places it lies in.
#[derive(Debug, Clone)]
pub(super) struct Placed {
    pub kind: RelatedKind,
    pub name: String,
    pub context: Vec<RelatedAncestor>,
}

/// The kind of thing the edge store calls `document`, `chat` or `conversation`.
pub(super) fn kind_of(word: &str) -> RelatedKind {
    match word {
        "chat" => RelatedKind::Chat,
        "conversation" => RelatedKind::Thread,
        _ => RelatedKind::Document,
    }
}

/// The names the edge store gives a kind, from the words of the API.
pub(super) fn store_kind(kind: RelatedKind) -> &'static str {
    match kind {
        RelatedKind::Document => "document",
        RelatedKind::Chat => "chat",
        RelatedKind::Thread => "conversation",
    }
}

/// The name and the places of readable documents, chats and threads, as the reader may know them. Only identities the
/// caller already found readable belong here.
pub(super) async fn placed(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<HashMap<Id, Placed>> {
    let items = related_rows::items(conn, ids).await?;
    if items.is_empty() {
        return Ok(HashMap::new());
    }
    let nodes: Vec<Id> = items.iter().map(|item| item.node_id).collect();
    let ways = view::ways(conn, principal, &nodes, Policy::default()).await?;
    let documents: Vec<_> = items
        .iter()
        .filter(|item| item.kind == "document")
        .map(|item| orch_engine::document_tree::Item::Document(item.source_id))
        .collect();
    let folders = orch_engine::document_tree::load_entries_in(
        conn,
        orch_engine::document_tree::Actor::Person(principal),
        &[],
        &documents,
        now.into(),
    )
    .await
    .map_err(ApiError::internal)?;
    let ancestors: HashMap<_, _> = folders
        .ancestors
        .iter()
        .map(|ancestor| (ancestor.id, ancestor))
        .collect();
    let chat_names = people_names(conn, principal, &items, now).await?;
    let mut out = HashMap::new();
    for item in items {
        let kind = kind_of(&item.kind);
        let name = chat_names
            .get(&item.source_id)
            .cloned()
            .unwrap_or_else(|| item.name.clone());
        let mut context: Vec<RelatedAncestor> = ways
            .get(&item.node_id)
            .into_iter()
            .flatten()
            .filter(|step| step.id != item.source_id)
            .map(|step| RelatedAncestor {
                id: step.id,
                name: step.name.clone(),
            })
            .collect();
        let mut parent = item.parent_container_id;
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        while let Some(id) = parent {
            if !seen.insert(id) {
                break;
            }
            let Some(folder) = ancestors.get(&id) else {
                break;
            };
            path.push(RelatedAncestor {
                id,
                name: folder.title.clone(),
            });
            parent = match folder.parent {
                orch_engine::document_tree::Directory::Container(id) => Some(id),
                _ => None,
            };
        }
        context.extend(path.into_iter().rev());
        out.insert(
            item.source_id,
            Placed {
                kind,
                name,
                context,
            },
        );
    }
    Ok(out)
}

/// The names of the chats among `items` that have none of their own, from the other people in them: the chats, their
/// members and the people are each read once for all of them, whatever their number.
async fn people_names(
    conn: &mut Conn,
    principal: &Principal,
    items: &[related_rows::Item],
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<HashMap<Id, String>> {
    let unnamed: Vec<Id> = items
        .iter()
        .filter(|item| item.kind == "chat" && item.name.is_empty())
        .map(|item| item.source_id)
        .collect();
    if unnamed.is_empty() {
        return Ok(HashMap::new());
    }
    let mut others: HashMap<Id, Vec<Id>> = HashMap::new();
    for member in chat_rows::members_in(conn, &unnamed).await? {
        if member.active_at(now) && member.user_id != principal.user() {
            others
                .entry(member.chat_id)
                .or_default()
                .push(member.user_id);
        }
    }
    let wanted: Vec<Id> = others.values().flatten().copied().collect();
    let people = refusals::user_refs(conn, &wanted).await?;
    Ok(others
        .into_iter()
        .map(|(chat, users)| {
            let name = users
                .iter()
                .filter_map(|user| people.get(user).map(|person| person.display_name.clone()))
                .collect::<Vec<_>>()
                .join(", ");
            (
                chat,
                if name.is_empty() {
                    "Chat".to_owned()
                } else {
                    name
                },
            )
        })
        .collect())
}

/// What a reader may call each chip of a line, from the names loaded for them.
pub(super) struct CastNames<'a>(pub &'a Cast);

impl Names for CastNames<'_> {
    fn chip(&self, kind: InlineRefKind, id: Id, path: Option<&str>) -> Option<String> {
        match kind {
            InlineRefKind::Node => self.0.node(id).and_then(|node| node.name),
            InlineRefKind::Document => self.0.title_of(id),
            InlineRefKind::Chat => self.0.chat_of(id),
            InlineRefKind::File => self.0.file_of(id),
            InlineRefKind::EnvironmentFile => self.0.environment_file_of(id, path?),
        }
    }

    fn person(&self, id: Id) -> Option<String> {
        self.0.person(id).map(|person| person.display_name)
    }
}
