//! Attention, search, unread and tree projections, and the names the finder looks through.
//!
//! Item text is an explicit projection of what was said or named, never the
//! raw call or its output. Masking precedes truncation. The same projector
//! keeps live item text and repairs it; chat and document repair use their
//! writers' own extractors. Repair compares source fields before writing so
//! a concurrent edit cannot be replaced by an old projection.

use crate::{Engine, EngineError};
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_api_contract::documents::BlockType;
use orch_authz::{
    Act, Policy, Principal,
    engine::{Frame, Graph},
};
use orch_chat::redaction::{Pattern, Redactor};
use orch_db::{
    Conn,
    queries::{search as rows, threads},
    secrets::MasterKey,
};
use orch_documents::blocks;
use orch_settings::{
    Place,
    secrets::{self, Owner},
};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

/// The most characters stored for a conversation item, regardless of script.
pub const SEARCH_TEXT_MAX_CHARS: usize = 20_000;
/// At most ten MiB of four-byte text per repair batch, before source payloads.
const REPAIR_BATCH: i64 = 128;

static MASTER_KEY: OnceLock<MasterKey> = OnceLock::new();

/// Installs the key used to mask the indexed text of a thread's secrets.
pub fn install_master_key(key: MasterKey) -> bool {
    MASTER_KEY.set(key).is_ok()
}

/// Orch reserves these delimiters for injected context. An echoed block (including an unfinished
/// streamed one) is diagnostic input, not conversation to index. Escaped authored delimiters stay text.
fn without_orch_context(text: &str) -> String {
    if !text.contains("<orch>") {
        return text.to_owned();
    }
    let mut visible = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<orch>") {
        visible.push_str(&rest[..start]);
        let mut hidden = &rest[start + "<orch>".len()..];
        let mut depth = 1;
        loop {
            let Some(end) = hidden.find("</orch>") else {
                return visible.trim().to_owned();
            };
            if let Some(open) = hidden.find("<orch>").filter(|open| *open < end) {
                depth += 1;
                hidden = &hidden[open + "<orch>".len()..];
            } else {
                depth -= 1;
                hidden = &hidden[end + "</orch>".len()..];
                if depth == 0 {
                    break;
                }
            }
        }
        // Removing a block must not join two formerly separate words into a new search term.
        visible.push('\n');
        rest = hidden;
    }
    visible.push_str(rest);
    visible.trim().to_owned()
}

fn raw_item(kind: &str, payload: &Json) -> Option<String> {
    let text = |value: Option<&Json>| value.and_then(Json::as_str).map(without_orch_context);
    match kind {
        "agent_message" => text(payload.get("text")),
        "reasoning" if payload.get("hidden").and_then(Json::as_bool) != Some(true) => {
            text(payload.get("summary"))
        }
        "plan" => {
            let mut parts: Vec<String> = text(payload.get("explanation")).into_iter().collect();
            parts.extend(
                payload
                    .get("steps")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|step| text(step.get("text"))),
            );
            Some(parts.join("\n"))
        }
        "tool_call" => {
            let tool = payload.get("tool");
            let mut parts: Vec<String> = text(payload.get("title")).into_iter().collect();
            match tool
                .and_then(|tool| tool.get("kind"))
                .and_then(Json::as_str)
            {
                Some("command") => parts.extend(text(tool.and_then(|tool| tool.get("command")))),
                Some("file_change") => parts.extend(
                    tool.and_then(|tool| tool.get("changes"))
                        .and_then(Json::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|change| text(change.get("path"))),
                ),
                Some("file_read") => parts.extend(text(tool.and_then(|tool| tool.get("path")))),
                Some("mcp") => {
                    parts.extend(text(tool.and_then(|tool| tool.get("server"))));
                    parts.extend(text(tool.and_then(|tool| tool.get("tool"))));
                }
                Some("file_search" | "web_search") => {
                    parts.extend(text(tool.and_then(|tool| tool.get("query"))))
                }
                Some("web_fetch") => parts.extend(text(tool.and_then(|tool| tool.get("url")))),
                _ => {}
            }
            Some(parts.join("\n"))
        }
        "error" => text(payload.get("message")),
        "user_message" => {
            let parts = payload.get("parts").and_then(Json::as_array)?;
            let mut found: Vec<String> = parts
                .iter()
                .filter(|part| part.get("type").and_then(Json::as_str) == Some("text"))
                .filter_map(|part| text(part.get("text")))
                .collect();
            found.extend(
                parts
                    .iter()
                    .filter(|part| {
                        matches!(
                            part.get("type").and_then(Json::as_str),
                            Some("file" | "image")
                        )
                    })
                    .filter_map(|part| text(part.get("name"))),
            );
            Some(found.join("\n"))
        }
        "announcement" => Some(
            [text(payload.get("title")), text(payload.get("body"))]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        "approval" => text(payload.get("title")),
        _ => None,
    }
}

fn cut(text: Option<String>) -> Option<String> {
    text.filter(|text| !text.is_empty())
        .map(|text| text.chars().take(SEARCH_TEXT_MAX_CHARS).collect())
}

/// The unmasked plain-text fields of an item, for callers whose source was
/// already masked. Raw calls, command output and hidden reasoning are excluded.
#[must_use]
pub fn plain_item(kind: &str, payload: &Json) -> Option<String> {
    cut(raw_item(kind, payload))
}

#[derive(Debug)]
enum Projector {
    Hidden,
    Ready(Redactor),
}

impl Projector {
    async fn load(conn: &mut Conn, thread: Id) -> Result<Self, EngineError> {
        let Some(row) = threads::thread(conn, thread).await? else {
            return Ok(Self::Hidden);
        };
        if threads::forking::side_parent(conn, thread).await?.is_some() {
            return Ok(Self::Hidden);
        }
        let place = Place::Node(thread);
        let patterns: Vec<Pattern> = orch_settings::get(conn, place, keys::REDACTION_PATTERNS)
            .await?
            .unwrap_or_default();
        let mask_secrets: bool = orch_settings::get(conn, place, keys::REDACTION_MASK_SECRETS)
            .await?
            .unwrap_or(true);
        let mut values = Vec::new();
        if mask_secrets {
            let mut owners = vec![Owner::Node(row.topic_id)];
            owners.extend(row.environment_id.map(Owner::Environment));
            for owner in owners {
                if secrets::list(conn, owner)
                    .await
                    .map_err(|error| EngineError::unreadable(error.to_string()))?
                    .is_empty()
                {
                    continue;
                }
                let Some(key) = MASTER_KEY.get() else {
                    tracing::warn!(%thread, "search text was omitted because its secrets could not be opened");
                    return Ok(Self::Hidden);
                };
                match secrets::open_resolved(conn, key, owner).await {
                    Ok(opened) => values.extend(opened.into_iter().map(|secret| secret.value)),
                    Err(_) => {
                        tracing::warn!(%thread, "search text was omitted because its secrets could not be opened");
                        return Ok(Self::Hidden);
                    }
                }
            }
        }
        Ok(Self::Ready(Redactor::new(&patterns, values)))
    }

    async fn text(
        &self,
        conn: &mut Conn,
        thread: Id,
        kind: &str,
        payload: &Json,
    ) -> Result<Option<String>, EngineError> {
        let Self::Ready(masking) = self else {
            return Ok(None);
        };
        let raw = if kind == "approval" {
            match payload
                .get("approval_id")
                .and_then(Json::as_str)
                .and_then(|id| id.parse().ok())
            {
                Some(id) => rows::approval_title(conn, thread, id).await?,
                None => raw_item(kind, payload),
            }
        } else {
            raw_item(kind, payload)
        };
        Ok(cut(raw.map(|text| masking.mask(&text))))
    }
}

/// Builds live search text, including the exclusion of every side-thread item.
pub async fn item_text(
    conn: &mut Conn,
    thread: Id,
    kind: &str,
    payload: &Json,
) -> Result<Option<String>, EngineError> {
    Projector::load(conn, thread)
        .await?
        .text(conn, thread, kind, payload)
        .await
}

/// The extractor of the search text of the items of one thread, loaded once: it reads the thread, its settings and the secrets it masks,
/// which writing many items must not read again for each of them.
pub struct ItemTexts {
    projector: Projector,
}

impl ItemTexts {
    /// Loads the extractor of `thread`.
    pub async fn of(conn: &mut Conn, thread: Id) -> Result<Self, EngineError> {
        Ok(Self {
            projector: Projector::load(conn, thread).await?,
        })
    }

    /// The search text of an item of `kind` with `payload`: none where the thread keeps none or the item says nothing.
    pub async fn text(
        &self,
        conn: &mut Conn,
        thread: Id,
        kind: &str,
        payload: &Json,
    ) -> Result<Option<String>, EngineError> {
        self.projector.text(conn, thread, kind, payload).await
    }
}

/// Keeps a completed projection, including clearing text that is now excluded.
pub async fn keep_item_text(
    conn: &mut Conn,
    id: Id,
    payload: &Json,
    status: &str,
    text: Option<&str>,
) -> Result<(), EngineError> {
    rows::put_item_texts(
        conn,
        &json!([{"id":id,"payload":payload,"status":status,"text":text}]),
    )
    .await?;
    Ok(())
}

/// The projection rows changed by an explicit repair.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rebuilt {
    /// Conversation items changed.
    pub items: u64,
    /// Chat messages changed.
    pub messages: u64,
    /// Document blocks changed.
    pub blocks: u64,
}

/// Repairs every text projection using its live extractor, without changing
/// content, user-visible sequences or attention counters. The caller owns the transaction.
pub async fn rebuild(conn: &mut Conn) -> Result<Rebuilt, EngineError> {
    let mut rebuilt = Rebuilt::default();
    let mut projectors = HashMap::new();
    let mut after = None;
    loop {
        let items = rows::projection_items(conn, after, REPAIR_BATCH).await?;
        if items.is_empty() {
            break;
        }
        // Cache at most two repair batches of threads, while reusing a common
        // thread's masking for a long conversation across adjacent batches.
        if projectors.len() > REPAIR_BATCH as usize {
            projectors.clear();
        }
        after = items.last().map(|item| item.id);
        let mut updates = Vec::with_capacity(items.len());
        for item in items {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                projectors.entry(item.thread_id)
            {
                entry.insert(Projector::load(conn, item.thread_id).await?);
            }
            let projector = projectors
                .get(&item.thread_id)
                .ok_or_else(|| EngineError::unreadable("a projection's thread was not loaded"))?;
            let text = projector
                .text(conn, item.thread_id, &item.kind, &item.payload)
                .await?;
            updates.push(
                json!({"id":item.id,"payload":item.payload,"status":item.status,"text":text}),
            );
        }
        rebuilt.items += rows::put_item_texts(conn, &json!(updates)).await?;
    }
    after = None;
    loop {
        let messages = rows::projection_messages(conn, after, REPAIR_BATCH).await?;
        if messages.is_empty() {
            break;
        }
        after = messages.last().map(|message| message.id);
        let updates: Vec<Json> = messages.into_iter().map(|message| {
            let text = if message.deleted_at.is_none() && message.kind == "message" {
                orch_chat::messages::search_text_of(&orch_chat::views::parts_of(&message.content))
            } else { None };
            json!({"id":message.id,"content":message.content,"deleted_at":message.deleted_at,"text":text})
        }).collect();
        rebuilt.messages += rows::put_message_texts(conn, &json!(updates)).await?;
    }
    after = None;
    loop {
        let held = rows::projection_blocks(conn, after, REPAIR_BATCH).await?;
        if held.is_empty() {
            break;
        }
        after = held.last().map(|block| block.id);
        let updates: Vec<Json> = held.into_iter().map(|block| {
            let kind: BlockType = serde_json::from_value(json!(block.block_type)).unwrap_or(BlockType::Paragraph);
            let text = if block.removed_at.is_none() { blocks::plain_text(kind, &block.props, &blocks::content_from_json(&block.content)) } else { String::new() };
            json!({"id":block.id,"props":block.props,"content":block.content,"block_type":block.block_type,"removed_at":block.removed_at,"text":text})
        }).collect();
        rebuilt.blocks += rows::put_block_texts(conn, &json!(updates)).await?;
    }
    Ok(rebuilt)
}

impl Engine {
    /// Repairs only legacy projections that still contain reserved context, in short independent
    /// transactions. The same extractor used for ingestion clears it; raw content and sequences stay.
    pub async fn repair_context_search(&self) -> Result<u64, EngineError> {
        let mut after = None;
        let mut repaired = 0;
        while !self.is_stopped() {
            let mut tx = self.inner.db.begin().await?;
            let items = rows::context_projection_items(&mut tx, after, REPAIR_BATCH).await?;
            if items.is_empty() {
                break;
            }
            after = items.last().map(|item| item.id);
            let mut projectors = HashMap::new();
            let mut updates = Vec::with_capacity(items.len());
            for item in items {
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    projectors.entry(item.thread_id)
                {
                    entry.insert(Projector::load(&mut tx, item.thread_id).await?);
                }
                let projector = projectors.get(&item.thread_id).ok_or_else(|| {
                    EngineError::unreadable("a projection's thread was not loaded")
                })?;
                let text = projector
                    .text(&mut tx, item.thread_id, &item.kind, &item.payload)
                    .await?;
                updates.push(
                    json!({"id":item.id,"payload":item.payload,"status":item.status,"text":text}),
                );
            }
            repaired += rows::put_item_texts(&mut tx, &json!(updates)).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        Ok(repaired)
    }

    /// Rebuilds stored search text from the current content in one transaction.
    pub async fn rebuild_search(&self) -> Result<Rebuilt, EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let result = rebuild(&mut tx).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(result)
    }
}

/// Batched name visibility and the same administrative provenance as the opened pages.
pub struct NameVisibility {
    visible: HashSet<(String, Id)>,
    /// Only native View has this marker; ordinary page access does not.
    pub admin_access: HashMap<(String, Id), orch_access_contract::roles::AdminReach>,
    /// Explicit ordinary assignments/follows, from the same frames as visibility.
    pub participating: HashSet<(String, Id)>,
}

impl NameVisibility {
    /// Whether the candidate's name is visible.
    #[must_use]
    pub fn contains(&self, key: &(String, Id)) -> bool {
        self.visible.contains(key)
    }
}

/// Resolves all name kinds using the shared frames, without one request for each result.
pub async fn visible_names(
    conn: &mut Conn,
    principal: &Principal,
    candidates: &[rows::NameRow],
) -> Result<NameVisibility, EngineError> {
    let mut seen = HashSet::new();
    let mut admin_access = HashMap::new();
    let mut participating = HashSet::new();
    let nodes: Vec<Id> = candidates
        .iter()
        .filter(|row| matches!(row.kind.as_str(), "org" | "project" | "topic" | "thread"))
        .map(|row| row.id)
        .collect();
    if !nodes.is_empty() {
        for node in orch_authz::nodes::holding(
            conn,
            principal,
            &nodes,
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await?
        {
            let key = (node.kind.as_str().to_owned(), node.id);
            if node.participating {
                participating.insert(key.clone());
            }
            if let Some(place) = node.admin_access {
                admin_access.insert(key.clone(), place);
            }
            seen.insert(key);
        }
    }
    let documents: Vec<Id> = candidates
        .iter()
        .filter(|row| matches!(row.kind.as_str(), "document" | "document_container"))
        .map(|row| row.id)
        .collect();
    if !documents.is_empty() {
        for listed in
            orch_authz::documents::listed(conn, principal, &documents, Policy::default()).await?
        {
            if !listed.document.deleted && listed.holds(Permission::DocView) {
                let key = (listed.document.kind.as_str().to_owned(), listed.document.id);
                if listed.participating {
                    participating.insert(key.clone());
                }
                if let Some(place) = listed.admin_access {
                    admin_access.insert(key.clone(), place);
                }
                seen.insert(key);
            }
        }
    }
    let tags: Vec<Id> = candidates
        .iter()
        .filter(|row| row.kind == "tag")
        .map(|row| row.id)
        .collect();
    if !tags.is_empty() {
        let scopes = rows::tag_scopes(conn, &tags).await?;
        let mut nodes: Vec<Id> = scopes.iter().map(|scope| scope.node_id).collect();
        nodes.sort_unstable();
        nodes.dedup();
        let readable: HashMap<_, _> = orch_authz::nodes::holding(
            conn,
            principal,
            &nodes,
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await?
        .into_iter()
        .map(|node| (node.id, (node.admin_access, node.participating)))
        .collect();
        for scope in scopes {
            if let Some((place, takes_part)) = readable.get(&scope.node_id) {
                let key = ("tag".to_owned(), scope.tag_id);
                if *takes_part {
                    participating.insert(key.clone());
                }
                if let Some(place) = place {
                    admin_access.insert(key.clone(), *place);
                }
                seen.insert(key);
            }
        }
    }
    let chat_ids: Vec<_> = candidates
        .iter()
        .filter(|row| row.kind == "chat")
        .map(|row| row.id)
        .collect();
    let chat_rows = orch_db::queries::chats::chats(conn, &chat_ids).await?;
    let chat_access = orch_authz::chats::listed_access(conn, principal, &chat_rows).await?;
    for row in candidates.iter().filter(|row| row.kind == "chat") {
        if let Some(access) = chat_access.get(&row.id) {
            let key = (row.kind.clone(), row.id);
            if access.participating {
                participating.insert(key.clone());
            }
            if let Some(place) = access.admin_access {
                admin_access.insert(key.clone(), place);
            }
            seen.insert(key);
        }
    }
    let directories: Vec<Id> = candidates
        .iter()
        .filter(|row| matches!(row.kind.as_str(), "user" | "group"))
        .filter_map(|row| row.node_id)
        .collect();
    let directories: HashMap<_, _> = orch_authz::nodes::holding(
        conn,
        principal,
        &directories,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await?
    .into_iter()
    .map(|node| (node.id, (node.admin_access, node.participating)))
    .collect();
    for row in candidates
        .iter()
        .filter(|row| matches!(row.kind.as_str(), "user" | "group"))
    {
        if let Some((place, takes_part)) = row.node_id.and_then(|id| directories.get(&id)) {
            let key = (row.kind.clone(), row.id);
            if *takes_part {
                participating.insert(key.clone());
            }
            if let Some(place) = place {
                admin_access.insert(key.clone(), *place);
            }
            seen.insert(key);
        }
    }
    let mut resources: Vec<Id> = candidates
        .iter()
        .filter(|row| {
            !matches!(
                row.kind.as_str(),
                "org"
                    | "project"
                    | "topic"
                    | "thread"
                    | "document"
                    | "document_container"
                    | "chat"
                    | "user"
                    | "group"
                    | "tag"
            )
        })
        .map(|row| row.id)
        .collect();
    let machine_ids: Vec<_> = candidates
        .iter()
        .filter(|row| row.kind == "machine")
        .map(|row| row.id)
        .collect();
    let machine_hosts = if machine_ids.is_empty() {
        Vec::new()
    } else {
        orch_db::queries::resources::hosting::machine_hosts(conn, &machine_ids).await?
    };
    resources.extend(machine_hosts.iter().map(|(_, host, _)| *host));
    if !resources.is_empty() {
        let mut graph = Graph::default();
        let facts = orch_authz::resources::read_into(conn, &mut graph, &resources).await?;
        let sites: Vec<_> = facts
            .iter()
            .filter_map(|resource| orch_authz::resources::site_of(&graph, resource))
            .collect();
        let permissions: Vec<_> = sites
            .iter()
            .flat_map(|site| site.kind.permissions())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let frame = Frame::load(
            conn,
            principal,
            Policy::default(),
            graph,
            &sites,
            &permissions,
        )
        .await?;
        for site in sites {
            if !principal.suspension().applies(&site.orgs) && !frame.effective(&site).is_empty() {
                let key = (site.kind.as_str().to_owned(), site.id);
                if frame.participates(&site) {
                    participating.insert(key.clone());
                }
                if let Some(place) = frame.admin_access(&site) {
                    admin_access.insert(key.clone(), place);
                }
                seen.insert(key);
            }
        }
    }
    for (machine, host, _) in machine_hosts {
        let host = ("host".to_owned(), host);
        if seen.contains(&host) {
            let machine = ("machine".to_owned(), machine);
            if participating.contains(&host) {
                participating.insert(machine.clone());
            }
            if let Some(place) = admin_access.get(&host).copied() {
                admin_access.insert(machine.clone(), place);
            }
            seen.insert(machine);
        }
    }
    Ok(NameVisibility {
        visible: seen,
        admin_access,
        participating,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_context_is_excluded_even_when_echoed_or_still_streaming() {
        for (text, expected) in [
            (
                "<orch>\nWhere you are\nprivate injected words\n</orch>",
                None,
            ),
            (
                "Before<orch>one\nsecret</orch>after<orch>two</orch>終わり",
                Some("Before\nafter\n終わり"),
            ),
            (
                "Public progress <orch>unfinished injected words",
                Some("Public progress"),
            ),
            (
                "Before<orch>outer<orch>inner</orch>still hidden</orch>after",
                Some("Before\nafter"),
            ),
            (
                r"An authored <\orch> example stays literal",
                Some(r"An authored <\orch> example stays literal"),
            ),
        ] {
            assert_eq!(
                plain_item("agent_message", &json!({"text":text})).as_deref(),
                expected
            );
            assert_eq!(
                plain_item("reasoning", &json!({"summary":text})).as_deref(),
                expected
            );
        }
    }

    #[test]
    fn an_item_indexes_what_was_named_and_never_a_calls_raw_values_or_output() {
        let call = json!({"title":"Run checks","tool":{"kind":"command","command":"cargo test","output":{"inline":"output-secret"}},"raw":{"arguments":"raw-secret"}});
        assert_eq!(
            plain_item("tool_call", &call).as_deref(),
            Some("Run checks\ncargo test")
        );
        assert_eq!(plain_item("marker", &json!({"text":"never-index"})), None);
        assert_eq!(
            plain_item(
                "reasoning",
                &json!({"summary":"hidden-secret","hidden":true})
            ),
            None
        );
    }

    #[test]
    fn input_text_precedes_attachment_names_and_ignores_identifiers_and_mime_types() {
        let parts = json!({"parts":[{"type":"image","name":"diagram.png","media_type":"secret/type"},{"type":"text","text":"Look here"},{"type":"file","name":"notes.txt","blob":"secret-hash"}]});
        assert_eq!(
            plain_item("user_message", &parts).as_deref(),
            Some("Look here\ndiagram.png\nnotes.txt")
        );
    }

    #[test]
    fn unicode_is_truncated_by_characters_after_secrets_have_been_masked() {
        let masking = Redactor::new(&[], vec!["sensitive-value".to_owned()]);
        let text = format!("{}sensitive-value", "界".repeat(SEARCH_TEXT_MAX_CHARS - 2));
        let projected = cut(Some(masking.mask(&text))).unwrap();
        assert_eq!(projected.chars().count(), SEARCH_TEXT_MAX_CHARS);
        assert!(!projected.ends_with("se"));
    }
}
