//! The indexed root-search window and query vector, prepared concurrently before the agent's selected reach is hydrated.

use super::{Arguments, MeaningTask, SearchCursor, SemanticPosition};
use crate::{engine::Engine, error::EngineError};
use orch_db::queries::search::{self as rows, candidates};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::Id;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Position {
    pub start: candidates::Position,
    pub next: Option<candidates::Position>,
    pub done: bool,
}

pub(super) struct Prepared {
    pub text: rows::CandidateSet,
    pub documents: Vec<Id>,
    pub conversations: HashSet<Id>,
    pub chats: HashSet<Id>,
    pub position: Position,
    pub meaning: Option<crate::semantic::query::Meaning>,
}

pub(super) async fn prepare(
    engine: &Engine,
    thread: Id,
    args: &Arguments,
    cursor: Option<&SearchCursor>,
) -> Result<Prepared, EngineError> {
    let wanted = |kind: &str| {
        args.kinds
            .as_ref()
            .is_none_or(|kinds| kinds.iter().any(|k| k == kind))
    };
    let semantic = cursor
        .and_then(|cursor| cursor.meaning.clone())
        .unwrap_or(SemanticPosition {
            offset: 0,
            done: cursor.is_some_and(|c| c.fused.is_none()),
            generation: None,
            reason: None,
        });
    let mut conn = engine.db().acquire().await?;
    let org = orch_db::queries::semantic::org_of(&mut conn, thread).await?;
    let mut scopes = vec![thread];
    let mut shared_documents = Vec::new();
    let mut shared_chats = Vec::new();
    if let Some(asking) = engine.asking(&mut conn, thread).await? {
        use orch_authz::reach::{AgentReach, DocumentPolicy};
        if asking.documents_policy != DocumentPolicy::None || asking.policy == AgentReach::Topic {
            scopes.push(asking.agent.topic);
        }
        if asking.policy == AgentReach::Project {
            scopes.extend(asking.agent.project);
        }
        for grant in orch_authz::reach::grants(&mut conn, &asking.agent, asking.now).await? {
            use orch_access_contract::pages::StoredKind;
            match grant.page.kind {
                StoredKind::Node => scopes.push(grant.page.id),
                StoredKind::Document => shared_documents.push(grant.page.id),
                StoredKind::Chat => shared_chats.push(grant.page.id),
                _ => {}
            }
        }
    }
    let readable = serde_json::json!({"scopes":scopes,"documents":shared_documents,"chats":shared_chats,"groups":[]});
    let source_kinds: Vec<_> = [
        ("document", wanted("documents")),
        ("chat", wanted("chats")),
        ("conversation", wanted("conversations")),
    ]
    .into_iter()
    .filter_map(|(kind, enabled)| enabled.then_some(kind))
    .collect();
    let semantic_filter = serde_json::json!({"readable":readable,"source_kinds":source_kinds});
    drop(conn);
    let mut meaning = if !semantic.done
        && !args.query.contains('"')
        && (wanted("documents") || wanted("conversations") || wanted("chats"))
    {
        if let Some(org) = org {
            let engine = engine.clone();
            let query = args.query.clone();
            MeaningTask(Some(tokio::spawn(async move {
                engine
                    .meaning_filtered(
                        org,
                        &query,
                        semantic.offset,
                        semantic.generation,
                        Some(semantic_filter),
                    )
                    .await
            })))
        } else {
            MeaningTask(None)
        }
    } else {
        MeaningTask(None)
    };
    let mut position = cursor
        .and_then(|cursor| cursor.indexed.clone())
        .unwrap_or_default();
    let mut tx = engine.db().begin_snapshot().await?;
    let mut cancellation = rows::cancel_on_drop(engine.db(), &mut tx).await?;
    let count = get::<i64>(&mut tx, Place::Node(thread), keys::SEARCH_WORD_CANDIDATES)
        .await?
        .ok_or_else(|| EngineError::unreadable("word candidate setting missing"))?;
    let probe = get::<i64>(
        &mut tx,
        Place::Node(thread),
        keys::SEARCH_WORD_PROBE_CANDIDATES,
    )
    .await?
    .ok_or_else(|| EngineError::unreadable("word probe setting missing"))?;
    let recent = get::<i64>(
        &mut tx,
        Place::Node(thread),
        keys::SEARCH_WORD_RECENT_CANDIDATES,
    )
    .await?
    .ok_or_else(|| EngineError::unreadable("recent word setting missing"))?;
    let tuning = get::<candidates::Tuning>(&mut tx, Place::Node(thread), keys::SEARCH_WORD_TUNING)
        .await?
        .ok_or_else(|| EngineError::unreadable("word retrieval setting missing"))?;
    let item_kinds: Vec<String> = [
        "user_message",
        "agent_message",
        "reasoning",
        "plan",
        "tool_call",
        "announcement",
        "approval",
        "error",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let empty = serde_json::json!({"search_readable":readable});
    let ask = rows::Content {
        continuation: None,
        selected: None,
        items: None,
        user: Id::nil(),
        groups: &[],
        member_roots: &[],
        grants: &empty,
        at: engine.inner.clock.now(),
        scope: None,
        chat: None,
        document: None,
        participating: false,
        relevance: true,
        names: true,
        with_file: false,
        mentions_me: None,
        text: &args.query,
        item_kinds: &item_kinds,
        chat_messages: wanted("chats"),
        document_content: wanted("documents") || wanted("files"),
        author: None,
        from: None,
        to: None,
        after: None,
        limit: i64::MAX,
    };
    let parts = if position.done {
        Vec::new()
    } else {
        let window = candidates::window_with_tuning(
            &mut tx,
            &ask,
            position.start.clone(),
            count,
            probe,
            recent,
            &tuning,
        )
        .await?;
        position.next = window.next;
        window.parts
    };
    cancellation.disarm();
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let mut documents: Vec<Id> = parts
        .iter()
        .filter(|p| p.kind == "document")
        .map(|p| p.source_id)
        .collect();
    let mut conversations: HashSet<Id> = parts
        .iter()
        .filter(|p| p.kind == "item")
        .map(|p| p.source_id)
        .collect();
    let mut chats: HashSet<Id> = parts
        .iter()
        .filter(|p| p.kind == "chat_message")
        .map(|p| p.source_id)
        .collect();
    let answer = if let Some(task) = meaning.0.as_mut() {
        let result = task.await;
        meaning.0 = None;
        Some(match result {
            Ok(Ok(answer)) => answer,
            _ => crate::semantic::query::Meaning::Unavailable("error".into()),
        })
    } else {
        None
    };
    if let Some(crate::semantic::query::Meaning::Found { near, .. }) = &answer {
        for part in near {
            match part.kind.as_str() {
                "document" => documents.push(part.source_id),
                "conversation" => {
                    conversations.insert(part.source_id);
                }
                "chat" => {
                    chats.insert(part.source_id);
                }
                _ => {}
            }
        }
    }
    documents.sort_unstable();
    documents.dedup();
    Ok(Prepared {
        text: rows::CandidateSet::of(&parts),
        documents,
        conversations,
        chats,
        position,
        meaning: answer,
    })
}
