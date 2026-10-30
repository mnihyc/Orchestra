//! A document's Links section: what refers to it with the line around each mention, what it refers to, the graph, and the tabs it shows; and that nothing a reader may not read ever shows in any of them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod continuation_read_support;
mod places;
mod rig;
mod support;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use continuation_read_support::{retained, select};
use orch_api_contract::chats::{ChatMessageCreate, ChatMessageCreateBody};
use orch_api_contract::common::{ErrorCode, Page};
use orch_api_contract::documents::{
    DocumentCommentDelete, DocumentDiscussionCreate, DocumentDiscussionCreateBody, DocumentUpdate,
    DocumentUpdateBody, Inline, InlineRefKind, NewDocumentAccess, NodeDocumentCreate,
    NodeDocumentCreateBody,
};
use orch_api_contract::links::{
    DocumentGraphGet, DocumentGraphGetQuery, DocumentLinksGet, DocumentLinksSet,
    DocumentLinksSetBody, DocumentReferencesList, DocumentReferencesListQuery, GraphEdgeKind,
    LinkAnchor, LinkDirection, LinkGraph, LinkedKind, LinkedThing, LinksTab,
};
use orch_api_contract::search::{DocumentRelatedList, RelatedKind};
use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
use orch_types::{Id, Json, Time, new_id};
use rig::Scene;
use serde_json::json;

async fn document(s: &Scene, node: Id, title: &str, markdown: &str, private: bool) -> Id {
    s.rig
        .world
        .app
        .call(NodeDocumentCreate, &[&node])
        .token(&s.ada.token)
        .body(&NodeDocumentCreateBody {
            title: title.into(),
            blocks: None,
            markdown: Some(markdown.into()),
            access: private.then_some(NewDocumentAccess::Private),
        })
        .send()
        .await
        .ok()
        .base
        .id
}

async fn set(s: &Scene, node: Id, key: &str, value: Json, locked: bool) {
    s.rig
        .world
        .app
        .call(NodeSettingSet, &[&node, &key])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value,
            locked: Some(locked),
            expected_revision: None,
        })
        .send()
        .await
        .ok();
}

fn ask(direction: LinkDirection) -> DocumentReferencesListQuery {
    DocumentReferencesListQuery {
        direction,
        limit: None,
        cursor: None,
        of: None,
    }
}

async fn list(
    s: &Scene,
    document: Id,
    token: &str,
    query: &DocumentReferencesListQuery,
) -> Page<LinkedThing> {
    s.rig
        .world
        .app
        .call(DocumentReferencesList, &[&document])
        .token(token)
        .query(query)
        .send()
        .await
        .ok()
}

async fn graph(s: &Scene, document: Id, token: &str, query: &DocumentGraphGetQuery) -> LinkGraph {
    s.rig
        .world
        .app
        .call(DocumentGraphGet, &[&document])
        .token(token)
        .query(query)
        .send()
        .await
        .ok()
}

fn graph_query(depth: u32) -> DocumentGraphGetQuery {
    DocumentGraphGetQuery {
        depth: Some(depth),
        kinds: None,
        meaning: None,
        center: None,
    }
}

async fn say(s: &Scene, chat: Id, words: &str) -> Id {
    s.rig
        .world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(&s.ada.token)
        .body(&ChatMessageCreateBody {
            agent_review: None,
            parts: vec![chat_world::text(words)],
            reply_to: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .message
        .id
}

fn chip(of: Id, label: &str) -> Inline {
    Inline::Ref {
        display: None,
        kind: InlineRefKind::Document,
        id: of,
        path: None,
        part: None,
        label: Some(label.into()),
        name: None,
    }
}

fn sources(page: &Page<LinkedThing>) -> Vec<Id> {
    page.items.iter().map(|item| item.id).collect()
}

/// Every place in which a reader's answers could hold a hidden source's identity or name.
fn mentions_nowhere(answers: &[String], hidden: &[(Id, &str)]) {
    for answer in answers {
        for (id, name) in hidden {
            assert!(!answer.contains(&id.to_string()), "an identity: {answer}");
            assert!(!answer.contains(name), "a name: {answer}");
            let dashless = id.simple().to_string();
            assert!(!answer.contains(&dashless), "an identity: {answer}");
            if let Ok(bytes) = URL_SAFE_NO_PAD.decode(answer.trim_matches('"')) {
                assert!(
                    !bytes.windows(16).any(|window| window == id.as_bytes()),
                    "a cursor holds an identity"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_document_lists_what_refers_to_it_with_the_line_around_each_mention_and_where_to_go() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let target = document(&s, topic, "Plan", "The dates.", false).await;
    let notes = document(
        &s,
        topic,
        "Notes",
        &format!("Decision: see [Plan](/documents/{target}) for the dates."),
        false,
    )
    .await;
    let block: Id = sqlx::query_scalar(
        "SELECT id FROM document_blocks WHERE document_id=$1 AND removed_at IS NULL LIMIT 1",
    )
    .bind(notes)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    let chat = chat_world::chat_of(&s.rig.world, &s.ada, topic).await;
    let message = say(
        &s,
        chat,
        &format!("Check [Plan](/documents/{target}) today"),
    )
    .await;
    s.rig
        .world
        .app
        .call(DocumentDiscussionCreate, &[&notes])
        .token(&s.ada.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: block,
            quote: None,
            content: vec![
                Inline::Text {
                    text: "Was ".into(),
                    marks: None,
                    href: None,
                    color: None,
                    background: None,
                },
                chip(target, "Plan"),
                Inline::Text {
                    text: " agreed?".into(),
                    marks: None,
                    href: None,
                    color: None,
                    background: None,
                },
            ],
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();

    let page = list(&s, target, &s.cai.token, &ask(LinkDirection::In)).await;
    assert!(page.next_cursor.is_none());
    assert_eq!(page.items.len(), 2, "a document and a chat: {page:?}");
    let by_notes = page.items.iter().find(|item| item.id == notes).unwrap();
    assert_eq!(by_notes.kind, LinkedKind::Document);
    assert_eq!(by_notes.name.as_deref(), Some("Notes"));
    assert_eq!(by_notes.mentions, 2, "its block and its comment");
    let in_text = by_notes
        .occurrences
        .iter()
        .find(|each| matches!(each.anchor, LinkAnchor::Block { .. }))
        .unwrap();
    assert_eq!(in_text.line.before, "Decision: see ");
    assert_eq!(in_text.line.reference, "Plan");
    assert_eq!(in_text.line.after, " for the dates.");
    assert!(matches!(in_text.anchor, LinkAnchor::Block { block_id } if block_id == block));
    let in_comment = by_notes
        .occurrences
        .iter()
        .find(|each| matches!(each.anchor, LinkAnchor::Comment { .. }))
        .unwrap();
    assert_eq!(in_comment.line.before, "Was ");
    assert_eq!(in_comment.line.after, " agreed?");
    let by_chat = page
        .items
        .iter()
        .find(|item| item.kind == LinkedKind::Chat)
        .unwrap();
    assert_eq!(by_chat.mentions, 1);
    assert!(
        matches!(by_chat.occurrences[0].anchor, LinkAnchor::Message { message_id } if message_id == message)
    );
    assert_eq!(by_chat.occurrences[0].line.before, "Check ");
    assert_eq!(by_chat.occurrences[0].line.after, " today");

    s.rig
        .world
        .app
        .call(DocumentUpdate, &[&target])
        .token(&s.ada.token)
        .body(&DocumentUpdateBody {
            title: Some("Plan 2027".into()),
            description: None,
        })
        .send()
        .await
        .ok();
    let renamed = list(&s, target, &s.cai.token, &ask(LinkDirection::In)).await;
    let line = &renamed
        .items
        .iter()
        .find(|item| item.id == notes)
        .unwrap()
        .occurrences[0]
        .line;
    assert_eq!(line.reference, "Plan 2027", "the target under its name now");
}

#[tokio::test]
async fn a_source_the_reader_may_not_read_leaves_no_trace_in_the_list_the_counts_the_graph_or_the_feed()
 {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let target = document(&s, topic, "Plan", "The dates.", false).await;
    let open = document(
        &s,
        topic,
        "Open notes",
        &format!("[Plan](/documents/{target})"),
        false,
    )
    .await;
    let seq_before: i64 = sqlx::query_scalar("SELECT doc_seq FROM documents WHERE id=$1")
        .bind(target)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let hidden = document(
        &s,
        topic,
        "Secret Layoffs",
        &format!("[Plan](/documents/{target}) and [Plan](/documents/{target})"),
        true,
    )
    .await;
    let seq_after: i64 = sqlx::query_scalar("SELECT doc_seq FROM documents WHERE id=$1")
        .bind(target)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(
        seq_before, seq_after,
        "a source's change does not touch the document it refers to, so no feed event says anything"
    );

    let reader = list(&s, target, &s.cai.token, &ask(LinkDirection::In)).await;
    assert_eq!(sources(&reader), vec![open]);
    let owner = list(&s, target, &s.ada.token, &ask(LinkDirection::In)).await;
    assert_eq!(owner.items.len(), 2);
    assert_eq!(
        owner
            .items
            .iter()
            .find(|item| item.id == hidden)
            .unwrap()
            .mentions,
        1,
        "one block names it twice, which is one mention"
    );
    let drawn = graph(&s, target, &s.cai.token, &graph_query(2)).await;
    assert!(drawn.nodes.iter().all(|node| node.id != hidden));
    assert!(
        drawn
            .edges
            .iter()
            .all(|edge| edge.from != hidden && edge.to != hidden)
    );
    let seen = graph(&s, target, &s.ada.token, &graph_query(2)).await;
    assert!(seen.nodes.iter().any(|node| node.id == hidden));
    mentions_nowhere(
        &[
            serde_json::to_string(&reader).unwrap(),
            serde_json::to_string(&drawn).unwrap(),
        ],
        &[(hidden, "Secret Layoffs")],
    );
    // A stranger may not even ask.
    let refused = s
        .rig
        .world
        .app
        .call(DocumentReferencesList, &[&target])
        .token(&s.bea.token)
        .query(&ask(LinkDirection::In))
        .send()
        .await;
    assert_eq!(refused.status, 404);
}

#[tokio::test]
async fn a_cursor_never_names_a_hidden_source_and_pages_over_them_deliver_each_readable_source_once()
 {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let target = document(&s, topic, "Plan", "The dates.", false).await;
    let mut visible = Vec::new();
    let mut hidden = Vec::new();
    for round in 0..4 {
        visible.push(
            document(
                &s,
                topic,
                &format!("Open {round}"),
                &format!("[Plan](/documents/{target})"),
                false,
            )
            .await,
        );
        hidden.push(
            document(
                &s,
                topic,
                &format!("Hidden {round}"),
                &format!("[Plan](/documents/{target})"),
                true,
            )
            .await,
        );
    }
    // Two checks a page: the budget runs out over hidden sources and the page goes on with a cursor.
    set(
        &s,
        s.tree.org.base.id,
        "documents.links.access_checks",
        json!(2),
        false,
    )
    .await;
    let mut got = Vec::new();
    let mut cursor = None;
    let mut cursors = Vec::new();
    for _ in 0..20 {
        let page = list(
            &s,
            target,
            &s.cai.token,
            &DocumentReferencesListQuery {
                direction: LinkDirection::In,
                limit: Some(1),
                cursor: cursor.clone(),
                of: None,
            },
        )
        .await;
        assert!(page.items.len() <= 1);
        got.extend(sources(&page));
        cursor = page.next_cursor;
        match &cursor {
            Some(text) => cursors.push(text.clone()),
            None => break,
        }
    }
    assert!(cursor.is_none(), "the pages end");
    got.sort();
    visible.sort();
    assert_eq!(got, visible, "each readable source once, no hidden one");
    let ids: Vec<(Id, &str)> = hidden.iter().map(|id| (*id, "Hidden")).collect();
    mentions_nowhere(&cursors, &ids);
    // A cursor belongs to its reader and its list.
    if let Some(first) = cursors.first() {
        let stolen = s
            .rig
            .world
            .app
            .call(DocumentReferencesList, &[&target])
            .token(&s.dan.token)
            .query(&DocumentReferencesListQuery {
                direction: LinkDirection::In,
                limit: Some(1),
                cursor: Some(first.clone()),
                of: None,
            })
            .send()
            .await;
        assert_eq!(stolen.status, 400);
    }
}

#[tokio::test]
async fn no_edge_is_drawn_through_a_hidden_node() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let far = document(&s, topic, "Far", "End of the line.", false).await;
    let hidden = document(
        &s,
        topic,
        "Hidden middle",
        &format!("[Far](/documents/{far})"),
        true,
    )
    .await;
    let near = document(
        &s,
        topic,
        "Near",
        &format!("[middle](/documents/{hidden})"),
        false,
    )
    .await;
    let reader = graph(&s, near, &s.cai.token, &graph_query(3)).await;
    assert_eq!(reader.nodes.len(), 1, "only itself: {reader:?}");
    assert!(reader.edges.is_empty());
    assert!(
        !reader
            .nodes
            .iter()
            .any(|node| node.id == far || node.id == hidden)
    );
    let owner = graph(&s, near, &s.ada.token, &graph_query(3)).await;
    let ids: Vec<Id> = owner.nodes.iter().map(|node| node.id).collect();
    assert!(ids.contains(&hidden) && ids.contains(&far));
    assert_eq!(owner.edges.len(), 2);
    assert!(
        owner
            .edges
            .iter()
            .all(|edge| edge.kind == GraphEdgeKind::Reference)
    );
    let one = graph(&s, near, &s.ada.token, &graph_query(1)).await;
    assert_eq!(one.nodes.len(), 2, "depth one stops at the middle");
    let current: Vec<_> = owner.nodes.iter().filter(|node| node.current).collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].id, near);
}

#[tokio::test]
async fn the_graph_keeps_to_its_size_its_kinds_and_a_chosen_centre() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let hub = document(&s, topic, "Hub", "The hub.", false).await;
    let mut around = Vec::new();
    for round in 0..4 {
        around.push(
            document(
                &s,
                topic,
                &format!("Spoke {round}"),
                &format!("[Hub](/documents/{hub})"),
                false,
            )
            .await,
        );
    }
    let chat = chat_world::chat_of(&s.rig.world, &s.ada, topic).await;
    say(&s, chat, &format!("[Hub](/documents/{hub})")).await;
    let all = graph(&s, hub, &s.ada.token, &graph_query(1)).await;
    assert_eq!(all.nodes.len(), 6, "the hub, four documents and the chat");
    assert!(!all.truncated);
    let only_documents = graph(
        &s,
        hub,
        &s.ada.token,
        &DocumentGraphGetQuery {
            depth: Some(1),
            kinds: Some("document".into()),
            meaning: None,
            center: None,
        },
    )
    .await;
    assert!(
        only_documents
            .nodes
            .iter()
            .all(|node| node.kind == RelatedKind::Document)
    );
    set(
        &s,
        s.tree.org.base.id,
        "documents.links.graph_nodes",
        json!(3),
        false,
    )
    .await;
    let small = graph(&s, hub, &s.ada.token, &graph_query(1)).await;
    assert_eq!(small.nodes.len(), 3);
    assert!(small.truncated, "a readable node was left out for size");
    let centred = graph(
        &s,
        hub,
        &s.ada.token,
        &DocumentGraphGetQuery {
            depth: Some(1),
            kinds: None,
            meaning: None,
            center: Some(format!("document:{}", around[0])),
        },
    )
    .await;
    assert_eq!(centred.center, around[0]);
    assert!(centred.nodes.iter().any(|node| node.id == hub));
    let nobody = s
        .rig
        .world
        .app
        .call(DocumentGraphGet, &[&hub])
        .token(&s.ada.token)
        .query(&graph_query(4))
        .send()
        .await;
    assert_eq!(nobody.status, 400);
}

#[tokio::test]
async fn superseded_thread_history_is_never_a_source() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let target = document(&s, topic, "Plan", "The dates.", false).await;
    let thread = s.thread().await.base.id;
    s.exchange(
        thread,
        0,
        "First question",
        &format!("Retained: see [Plan](/documents/{target})"),
    )
    .await;
    s.exchange(
        thread,
        1,
        "Second question",
        &format!("Excluded: see [Plan](/documents/{target}) again"),
    )
    .await;
    rig::eventually("native work finishes before the history is cut", || async {
        sqlx::query_scalar::<_, bool>(
            "SELECT NOT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1
              AND (end_work IS NOT NULL OR files_end_due OR files_work_pending))",
        )
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap_or(false)
    })
    .await;
    let before = list(&s, target, &s.ada.token, &ask(LinkDirection::In)).await;
    let mentions = |page: &Page<LinkedThing>| {
        page.items
            .iter()
            .find(|item| item.id == thread)
            .map_or(0, |item| item.mentions)
    };
    assert_eq!(
        mentions(&before),
        2,
        "both answers are in the history it shows"
    );
    let view = retained(
        &s.rig.world.app.app.db,
        thread,
        s.ada.id(),
        Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    select(
        &s.rig.world.app.app.db,
        thread,
        view.child,
        Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    let after = list(&s, target, &s.ada.token, &ask(LinkDirection::In)).await;
    assert_eq!(
        mentions(&after),
        1,
        "only the answer the thread's view still holds"
    );
    let item = after.items.iter().find(|item| item.id == thread).unwrap();
    assert!(matches!(
        item.occurrences[0].anchor,
        LinkAnchor::Turn { turn: Some(1), .. }
    ));
    assert!(item.occurrences[0].line.before.starts_with("Retained: see"));
    for each in &after.items {
        for occurrence in &each.occurrences {
            assert!(!occurrence.line.before.contains("Excluded"));
            assert!(!occurrence.line.after.contains("Excluded"));
        }
    }
}

#[tokio::test]
async fn a_deleted_comment_stops_being_a_source_and_what_a_document_refers_to_shows_locked_targets_by_their_label()
 {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let open_target = document(&s, topic, "Open target", "Readable.", false).await;
    let secret = document(&s, topic, "Secret target", "Hidden.", true).await;
    let notes = document(
        &s,
        topic,
        "Notes",
        &format!(
            "Read [Open target](/documents/{open_target}) and [Budget talks](/documents/{secret})."
        ),
        false,
    )
    .await;
    let out = list(&s, notes, &s.cai.token, &ask(LinkDirection::Out)).await;
    assert_eq!(out.items.len(), 2);
    let open = out
        .items
        .iter()
        .find(|item| item.id == open_target)
        .unwrap();
    assert!(open.readable);
    assert_eq!(open.name.as_deref(), Some("Open target"));
    let locked = out.items.iter().find(|item| item.id == secret).unwrap();
    assert!(!locked.readable);
    assert!(
        locked.name.is_none(),
        "no name of what the reader may not read"
    );
    assert_eq!(
        locked.label.as_deref(),
        Some("Budget talks"),
        "the words its author wrote"
    );
    assert!(
        !serde_json::to_string(&out)
            .unwrap()
            .contains("Secret target")
    );
    let owner = list(&s, notes, &s.ada.token, &ask(LinkDirection::Out)).await;
    assert!(
        owner
            .items
            .iter()
            .find(|item| item.id == secret)
            .unwrap()
            .readable
    );

    // A comment is a source of its own, until it is deleted.
    let block: Id = sqlx::query_scalar(
        "SELECT id FROM document_blocks WHERE document_id=$1 AND removed_at IS NULL LIMIT 1",
    )
    .bind(notes)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    let commented = document(&s, topic, "Commented", "Words.", false).await;
    let comment_block: Id = sqlx::query_scalar(
        "SELECT id FROM document_blocks WHERE document_id=$1 AND removed_at IS NULL LIMIT 1",
    )
    .bind(commented)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    let _ = block;
    let discussion = s
        .rig
        .world
        .app
        .call(DocumentDiscussionCreate, &[&commented])
        .token(&s.ada.token)
        .body(&DocumentDiscussionCreateBody {
            block_id: comment_block,
            quote: None,
            content: vec![chip(open_target, "Open target")],
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let incoming = list(&s, open_target, &s.cai.token, &ask(LinkDirection::In)).await;
    assert!(sources(&incoming).contains(&commented));
    let comment = discussion.comments[0].id;
    s.rig
        .world
        .app
        .call(DocumentCommentDelete, &[&commented, &comment])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let after = list(&s, open_target, &s.cai.token, &ask(LinkDirection::In)).await;
    assert!(!sources(&after).contains(&commented));
}

#[tokio::test]
async fn the_mentions_of_one_source_go_on_with_their_own_cursor() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let target = document(&s, topic, "Plan", "The dates.", false).await;
    let lines: Vec<String> = (0..7)
        .map(|n| format!("Line {n}: [Plan](/documents/{target})"))
        .collect();
    let many = document(&s, topic, "Many", &lines.join("\n\n"), false).await;
    set(
        &s,
        s.tree.org.base.id,
        "documents.links.occurrences",
        json!(3),
        false,
    )
    .await;
    let first = list(&s, target, &s.cai.token, &ask(LinkDirection::In)).await;
    let source = &first.items[0];
    assert_eq!(source.id, many);
    assert_eq!(source.mentions, 7);
    assert_eq!(source.occurrences.len(), 3);
    let mut seen: Vec<Id> = source.occurrences.iter().filter_map(part).collect();
    let mut cursor = source.next_mentions.clone();
    for _ in 0..5 {
        let Some(next) = cursor.take() else { break };
        let page = list(
            &s,
            target,
            &s.cai.token,
            &DocumentReferencesListQuery {
                direction: LinkDirection::In,
                limit: None,
                cursor: Some(next),
                of: Some(format!("document:{many}")),
            },
        )
        .await;
        assert_eq!(page.items.len(), 1);
        seen.extend(page.items[0].occurrences.iter().filter_map(part));
        cursor = page.items[0].next_mentions.clone();
    }
    assert!(cursor.is_none());
    let mut all = seen.clone();
    all.sort();
    all.dedup();
    assert_eq!(seen.len(), 7);
    assert_eq!(all.len(), 7, "every mention once");
}

fn part(occurrence: &orch_api_contract::links::LinkOccurrence) -> Option<Id> {
    match occurrence.anchor {
        LinkAnchor::Block { block_id } => Some(block_id),
        _ => None,
    }
}

#[tokio::test]
async fn the_tabs_a_document_shows_are_set_by_who_manages_it_and_are_display_only() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let target = document(&s, topic, "Plan", "The dates.", false).await;
    let get = |token: &str| {
        let app = &s.rig.world.app;
        let token = token.to_owned();
        async move {
            app.call(DocumentLinksGet, &[&target])
                .token(&token)
                .send()
                .await
                .ok()
        }
    };
    let start = get(&s.cai.token).await;
    assert_eq!(start.shown.len(), 4, "all four by default");
    assert!(start.own.is_none() && !start.locked && !start.writable);
    assert!(get(&s.ada.token).await.writable);
    let put = |token: &str, shown: Option<Vec<LinksTab>>| {
        let app = &s.rig.world.app;
        let token = token.to_owned();
        async move {
            app.call(DocumentLinksSet, &[&target])
                .token(&token)
                .body(&DocumentLinksSetBody { shown })
                .send()
                .await
        }
    };
    assert_eq!(
        put(&s.cai.token, Some(vec![LinksTab::Related]))
            .await
            .status,
        403
    );
    let changed = put(
        &s.ada.token,
        Some(vec![LinksTab::Graph, LinksTab::ReferencedBy]),
    )
    .await
    .ok();
    assert_eq!(changed.shown, vec![LinksTab::ReferencedBy, LinksTab::Graph]);
    assert_eq!(
        get(&s.cai.token).await.shown,
        vec![LinksTab::ReferencedBy, LinksTab::Graph]
    );
    let changes = || async {
        sqlx::query_as::<_, (Id, Id, bool, Json)>(
            "SELECT actor_id,node_id,history,details FROM audit_log
             WHERE object_id=$1 AND action='document.links_tabs_changed' ORDER BY id",
        )
        .bind(target)
        .fetch_all(s.rig.db.pool())
        .await
        .unwrap()
    };
    let recorded = changes().await;
    assert_eq!(
        recorded.len(),
        1,
        "one history line for the successful change"
    );
    assert_eq!(
        (recorded[0].0, recorded[0].1, recorded[0].2),
        (s.ada.id(), topic, true)
    );
    assert_eq!(recorded[0].3["before"], Json::Null);
    assert_eq!(recorded[0].3["after"], json!(["referenced_by", "graph"]));
    put(
        &s.ada.token,
        Some(vec![LinksTab::ReferencedBy, LinksTab::Graph]),
    )
    .await
    .ok();
    assert_eq!(changes().await.len(), 1, "a no-op creates no history line");
    let reset = put(&s.ada.token, None).await.ok();
    assert!(reset.own.is_none());
    assert_eq!(changes().await.len(), 2, "resetting is recorded too");
    put(
        &s.ada.token,
        Some(vec![LinksTab::ReferencedBy, LinksTab::Graph]),
    )
    .await
    .ok();
    assert_eq!(changes().await.len(), 3);
    // Off is display only: the lists still answer.
    let page = list(&s, target, &s.cai.token, &ask(LinkDirection::Out)).await;
    assert!(page.items.is_empty());
    let related = s
        .rig
        .world
        .app
        .call(DocumentRelatedList, &[&target])
        .token(&s.cai.token)
        .send()
        .await;
    assert_eq!(related.status, 200);
    // A lock above the document decides.
    set(
        &s,
        topic,
        "documents.links.shown",
        json!(["references"]),
        true,
    )
    .await;
    let locked = get(&s.ada.token).await;
    assert!(locked.locked && !locked.writable);
    assert_eq!(
        locked.shown,
        vec![LinksTab::References],
        "the lock wins over the document's own choice"
    );
    let refused = put(&s.ada.token, None).await;
    assert_eq!(refused.status, 409);
    assert_eq!(refused.problem().code, ErrorCode::AccessLocked);
    assert_eq!(
        changes().await.len(),
        3,
        "a refused write creates no history line"
    );
}

#[tokio::test]
async fn related_items_page_on_with_a_cursor_that_holds_nothing_hidden() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    set(
        &s,
        topic,
        "related.weights",
        json!({"meaning":"0","place":"1","references":"0"}),
        false,
    )
    .await;
    set(&s, topic, "related.count", json!(2), false).await;
    let source = document(&s, topic, "Source", "Words.", false).await;
    let mut readable = Vec::new();
    for round in 0..5 {
        readable.push(document(&s, topic, &format!("Beside {round}"), "Words.", false).await);
        document(&s, topic, &format!("Private {round}"), "Words.", true).await;
    }
    let page = |cursor: Option<String>| {
        let app = &s.rig.world.app;
        let token = s.cai.token.clone();
        async move {
            app.call(DocumentRelatedList, &[&source])
                .token(&token)
                .query(&orch_api_contract::common::PageQuery {
                    limit: None,
                    cursor,
                })
                .send()
                .await
                .ok()
        }
    };
    let mut got = Vec::new();
    let mut cursor = None;
    let mut cursors = Vec::new();
    for _ in 0..10 {
        let answer = page(cursor.clone()).await;
        assert!(answer.items.len() <= 2);
        got.extend(answer.items.iter().map(|item| item.id));
        cursor = answer.next_cursor;
        match &cursor {
            Some(text) => cursors.push(text.clone()),
            None => break,
        }
    }
    assert!(cursor.is_none());
    got.sort();
    readable.sort();
    assert_eq!(
        got, readable,
        "every readable item once, none of the private ones"
    );
    let hidden: Vec<(Id, &str)> = Vec::new();
    mentions_nowhere(&cursors, &hidden);
    let owner_names: Vec<String> =
        sqlx::query_scalar("SELECT title FROM documents WHERE title LIKE 'Private %'")
            .fetch_all(s.rig.db.pool())
            .await
            .unwrap();
    for cursor in &cursors {
        let bytes = URL_SAFE_NO_PAD.decode(cursor).unwrap();
        let text = String::from_utf8_lossy(&bytes).to_string();
        assert!(owner_names.iter().all(|name| !text.contains(name)));
    }
}
