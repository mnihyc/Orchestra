//! What sharing with agents comes to: a document, a chat, a thread and a whole place shared live with the agents of a thread or of a place, for reading or for writing, and taken back; and a copy of them given to a thread; each by the rules of access, all or nothing, and told to the thread.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_api_contract::share::{
    ShareAgentUpdateBody, ShareDocumentsLevel, ShareLevel, ShareParts, ShareTo, ShareToNode,
    ShareToThread, ShareWhat, ShareWhatChat, ShareWhatDocument, ShareWhatNode, ShareWithAgents,
};
use orch_api_contract::threads::ShareMode;
use orch_authz::Caller;
use orch_db::audit::Origin;
use orch_db::queries::access::testing as access_testing;
use orch_engine::sharing::{Record, ShareError};
use orch_link_contract::commands::OrchTool;
use orch_types::Id;
use serde_json::json;
use shared::*;
use std::sync::Arc;

fn ask(
    what: ShareWhat,
    parts: Option<ShareParts>,
    mode: ShareMode,
    to: ShareTo,
    client: &str,
) -> ShareWithAgents {
    ShareWithAgents {
        read_first: None,
        download: None,
        what,
        inside: None,
        parts,
        mode,
        to,
        comment: None,
        expires_at: None,
        client_id: client.to_owned(),
    }
}

fn threads(ids: &[Id]) -> ShareTo {
    ShareTo::Threads(
        ids.iter()
            .map(|id| ShareToThread { thread_id: *id })
            .collect(),
    )
}

fn places(ids: &[Id]) -> ShareTo {
    ShareTo::Nodes(ids.iter().map(|id| ShareToNode { node_id: *id }).collect())
}

fn node(id: Id) -> ShareWhat {
    ShareWhat::Node(ShareWhatNode { node_id: id })
}

fn document(id: Id) -> ShareWhat {
    ShareWhat::Document(ShareWhatDocument { document_id: id })
}

fn chat(id: Id) -> ShareWhat {
    ShareWhat::Chat(ShareWhatChat { chat_id: id })
}

fn timeline(level: ShareLevel) -> Option<ShareParts> {
    Some(ShareParts {
        timeline: Some(level),
        ..ShareParts::default()
    })
}

fn chat_part(level: ShareLevel) -> Option<ShareParts> {
    Some(ShareParts {
        chat: Some(level),
        ..ShareParts::default()
    })
}

fn documents(level: ShareDocumentsLevel) -> Option<ShareParts> {
    Some(ShareParts {
        documents: Some(level),
        ..ShareParts::default()
    })
}

async fn share_as(
    world: &common::World,
    person: Id,
    request: &ShareWithAgents,
) -> Result<Vec<Record>, ShareError> {
    world
        .engine()
        .share_with_agents(&Caller::session(person), &Origin::default(), request)
        .await
}

async fn share_live(world: &common::World, request: &ShareWithAgents) -> Vec<Record> {
    share_as(world, world.ada, request).await.unwrap()
}

/// Cuts every row of a document's page loose, so that its owners alone hold it.
async fn make_private(world: &common::World, document: Id) {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::pages::{PageChange, Whole};
    use orch_access_contract::permissions::ObjectKind;
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        Caller::session(world.ada),
        orch_types::Time::new(chrono::Utc::now()),
    )
    .await
    .unwrap();
    orch_authz::pages::change(
        &mut conn,
        &principal,
        orch_authz::pages::Target::Resource(ObjectRef {
            kind: ObjectKind::Document,
            id: document,
        }),
        &PageChange {
            whole: Some(Whole::Private),
            ..PageChange::default()
        },
        orch_authz::Policy::default(),
        &Origin::default(),
    )
    .await
    .unwrap();
}

struct Place {
    world: common::World,
    reader: Id,
    topic: Id,
}

/// An agent in the world's topic, and a topic of its own with a document and a chat nobody shared yet.
async fn a_reader_and_a_topic_to_share() -> Place {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let topic = world.topic("Docs").await;
    Place {
        world,
        reader,
        topic,
    }
}

async fn read(place: &Place, path: &str) -> orch_link_contract::commands::OrchToolResult {
    call(
        &place.world,
        place.reader,
        OrchTool::ReadShared,
        json!({ "path": path }),
    )
    .await
}

// ---------------------------------------------------------------- live shares

#[tokio::test]
async fn a_document_shared_for_reading_is_read_by_the_agent_and_never_written() {
    let p = a_reader_and_a_topic_to_share().await;
    let doc = p
        .world
        .document_beside(p.topic, "Handbook", &["Be kind.", "Be brief."])
        .await;
    assert_eq!(
        refused(&read(&p, &format!("id:{doc}")).await).0,
        "not_found",
        "until somebody shares it, it is not there for the agent"
    );
    let made = share_live(
        &p.world,
        &ask(
            document(doc),
            None,
            ShareMode::Live,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].parts.documents, Some(ShareDocumentsLevel::Read));
    assert_eq!(
        made[0].grants.len(),
        2,
        "one entry in the View row and one in the Export row"
    );
    let text = ok(read(&p, &format!("id:{doc}")).await)["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(text.contains("Be kind."), "{text}");
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", p.topic), "depth": 2 }),
    )
    .await);
    let path = format!(
        "{}Handbook.md",
        entry(&listing, "docs")["path"].as_str().unwrap()
    );
    let refusal = call(
        &p.world,
        p.reader,
        OrchTool::EditDocument,
        json!({ "command": "replace", "path": path, "old": "kind", "new": "careful" }),
    )
    .await;
    assert_eq!(
        refused(&refusal).0,
        "not_writable",
        "a share for reading lets it read only"
    );
}

#[tokio::test]
async fn a_document_shared_for_editing_is_changed_by_the_agent_under_its_name() {
    let p = a_reader_and_a_topic_to_share().await;
    let doc = p
        .world
        .document_beside(p.topic, "Handbook", &["Be kind."])
        .await;
    let made = share_live(
        &p.world,
        &ask(
            document(doc),
            documents(ShareDocumentsLevel::Edit),
            ShareMode::Live,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    assert_eq!(made[0].parts.documents, Some(ShareDocumentsLevel::Edit));
    assert_eq!(
        made[0].grants.len(),
        3,
        "the View row, the Edit row and the Export row"
    );
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", p.topic), "depth": 2 }),
    )
    .await);
    let path = format!(
        "{}Handbook.md",
        entry(&listing, "docs")["path"].as_str().unwrap()
    );
    ok(call(
        &p.world,
        p.reader,
        OrchTool::EditDocument,
        json!({ "command": "replace", "path": path, "old": "kind", "new": "careful" }),
    )
    .await);
    let by_agent: i64 = p
        .world
        .count(
            "SELECT count(*) FROM document_blocks WHERE document_id = $1 AND removed_at IS NULL AND updated_by_thread IS NOT NULL",
            doc,
        )
        .await;
    assert_eq!(by_agent, 1);
}

#[tokio::test]
async fn a_chat_shared_for_reading_is_not_written_in_until_it_is_shared_for_writing() {
    let p = a_reader_and_a_topic_to_share().await;
    let chat_id = chat_of(&p.world, p.topic).await;
    say_in_chat(&p.world, chat_id, p.world.ada, "Staging is down.").await;
    let made = share_live(
        &p.world,
        &ask(
            chat(chat_id),
            chat_part(ShareLevel::Read),
            ShareMode::Live,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    assert_eq!(made[0].parts.chat, Some(ShareLevel::Read));
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", p.topic) }),
    )
    .await);
    let path = entry(&listing, "chat.md")["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let first = call(
        &p.world,
        p.reader,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "chat", "path": path }, "text": "Looking." }),
    )
    .await;
    assert_eq!(refused(&first).0, "not_writable");
    let updated = p
        .world
        .engine()
        .update_share(
            &Caller::session(p.world.ada),
            &Origin::default(),
            made[0].id,
            &ShareAgentUpdateBody {
                read_first: None,
                parts: chat_part(ShareLevel::Write),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.parts.chat, Some(ShareLevel::Write));
    ok(call(
        &p.world,
        p.reader,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "chat", "path": path }, "text": "Looking." }),
    )
    .await);
}

#[tokio::test]
async fn a_thread_shared_for_writing_takes_a_message_from_the_agent_of_another_topic() {
    let p = a_reader_and_a_topic_to_share().await;
    let other = thread_in_topic(&p.world, p.topic, "Builder").await;
    share_live(
        &p.world,
        &ask(
            node(other),
            timeline(ShareLevel::Write),
            ShareMode::Live,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", p.topic) }),
    )
    .await);
    let path = entry(&listing, "Builder")["path"]
        .as_str()
        .unwrap()
        .to_owned();
    let sent = call(
        &p.world,
        p.reader,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": path }, "text": "The build is green." }),
    )
    .await;
    assert!(sent.error.is_none(), "{:?}", sent.error);
}

#[tokio::test]
async fn a_whole_place_shared_with_the_agents_of_another_covers_what_is_made_in_it_later() {
    let p = a_reader_and_a_topic_to_share().await;
    let made = share_live(
        &p.world,
        &ask(
            node(p.topic),
            Some(ShareParts {
                timeline: Some(ShareLevel::Read),
                chat: Some(ShareLevel::Read),
                documents: Some(ShareDocumentsLevel::Read),
            }),
            ShareMode::Live,
            places(&[p.world.topic]),
            "c1",
        ),
    )
    .await;
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].to, p.world.topic);
    let later = thread_in_topic(&p.world, p.topic, "Later").await;
    talk(&p.world, later, "What changed?", "Nothing yet.").await;
    the_agent_takes_tools_again(&p.world).await;
    let note = p
        .world
        .document_beside(p.topic, "Notes", &["Made afterwards."])
        .await;
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", p.topic), "depth": 3 }),
    )
    .await);
    let all = paths(&listing).join("\n");
    assert!(all.contains("/Later/conversation.md"), "{all}");
    assert!(all.contains("Notes.md"), "{all}");
    let text = ok(read(&p, &format!("id:{note}")).await)["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(text.contains("Made afterwards."));
}

#[tokio::test]
async fn a_share_taken_back_is_gone_at_the_next_call() {
    let p = a_reader_and_a_topic_to_share().await;
    let doc = p
        .world
        .document_beside(p.topic, "Handbook", &["Be kind."])
        .await;
    let made = share_live(
        &p.world,
        &ask(
            document(doc),
            None,
            ShareMode::Live,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    ok(read(&p, &format!("id:{doc}")).await);
    let engine = p.world.engine();
    engine
        .revoke_share(
            &Caller::session(p.world.ada),
            &Origin::default(),
            made[0].id,
        )
        .await
        .unwrap();
    assert_eq!(
        refused(&read(&p, &format!("id:{doc}")).await).0,
        "not_found"
    );
    let removed: i64 = p
        .world
        .count(
            "SELECT count(*) FROM audit_log WHERE action = 'permission.removed' AND details->'object'->>'id' = $1::text",
            doc,
        )
        .await;
    assert_eq!(
        removed, 2,
        "taking a share back is audited as a change of the page, the View row and the Export row each"
    );
    let again = engine
        .revoke_share(
            &Caller::session(p.world.ada),
            &Origin::default(),
            made[0].id,
        )
        .await;
    assert!(matches!(again, Err(ShareError::NotFound)));
}

#[tokio::test]
async fn asking_again_with_the_same_value_shares_once() {
    let p = a_reader_and_a_topic_to_share().await;
    let doc = p
        .world
        .document_beside(p.topic, "Handbook", &["Be kind."])
        .await;
    let request = ask(
        document(doc),
        None,
        ShareMode::Live,
        threads(&[p.reader]),
        "same",
    );
    let first = share_live(&p.world, &request).await;
    let second = share_live(&p.world, &request).await;
    assert_eq!(first[0].id, second[0].id);
    assert!(first[0].fresh && !second[0].fresh);
    let entries: i64 = p
        .world
        .count(
            "SELECT count(*) FROM permission_entries WHERE object_id = $1 AND holder_kind = 'agents' AND removed_at IS NULL",
            doc,
        )
        .await;
    assert_eq!(entries, 2, "the View row and the Export row, once");
    let markers: i64 = p
        .world
        .count(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'marker' AND payload->>'marker' = 'shared'",
            p.reader,
        )
        .await;
    assert_eq!(
        markers, 1,
        "a document leaves one share-history line, including after a retry"
    );
    let notices: serde_json::Value = p
        .world
        .scalar(
            "SELECT pending_notices FROM threads WHERE node_id = $1",
            p.reader,
        )
        .await;
    assert_eq!(notices.as_array().unwrap().len(), 1, "{notices}");
    let marker: serde_json::Value = p
        .world
        .scalar(
            "SELECT payload FROM items WHERE thread_id = $1 AND payload->>'marker' = 'shared'",
            p.reader,
        )
        .await;
    assert_eq!(marker["data"]["what"]["document"]["id"], doc.to_string());
    assert_eq!(marker["data"]["what"]["document"]["title"], "Handbook");
    assert_eq!(marker["data"]["by"]["id"], p.world.ada.to_string());
    let sync = p.world.sync(p.reader).await;
    assert!(sync.items.iter().any(|item| matches!(&item.payload,
        orch_api_contract::threads::ItemPayload::Marker(orch_api_contract::threads::MarkerPayload::Shared(share))
            if matches!(&share.what, orch_api_contract::threads::SharedThing::Document(document) if document.id == doc))));
}

#[tokio::test]
async fn changing_and_taking_back_one_documents_share_are_lines_naming_the_document_and_the_giver()
{
    let p = a_reader_and_a_topic_to_share().await;
    let doc = p
        .world
        .document_beside(p.topic, "Handbook", &["Be kind."])
        .await;
    let made = share_live(
        &p.world,
        &ask(
            document(doc),
            None,
            ShareMode::Live,
            threads(&[p.reader]),
            "document-history",
        ),
    )
    .await;
    let engine = p.world.engine();
    engine
        .update_share(
            &Caller::session(p.world.ada),
            &Origin::default(),
            made[0].id,
            &ShareAgentUpdateBody {
                read_first: None,
                parts: documents(ShareDocumentsLevel::Edit),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    engine
        .revoke_share(
            &Caller::session(p.world.ada),
            &Origin::default(),
            made[0].id,
        )
        .await
        .unwrap();
    let sync = p.world.sync(p.reader).await;
    let mut names = Vec::new();
    for item in &sync.items {
        if let orch_api_contract::threads::ItemPayload::Marker(marker) = &item.payload {
            let (name, share) = match marker {
                orch_api_contract::threads::MarkerPayload::Shared(share) => ("shared", share),
                orch_api_contract::threads::MarkerPayload::Unshared(share) => ("unshared", share),
                _ => continue,
            };
            assert_eq!(share.by.id, p.world.ada);
            assert!(
                matches!(&share.what, orch_api_contract::threads::SharedThing::Document(document)
                if document.id == doc && document.title == "Handbook")
            );
            names.push(name);
        }
    }
    assert_eq!(names, ["shared", "shared", "unshared"]);
}

#[tokio::test]
async fn a_place_shared_with_a_thread_is_a_line_in_its_history_and_a_notice_for_its_next_turn() {
    let p = a_reader_and_a_topic_to_share().await;
    share_live(
        &p.world,
        &ask(
            node(p.topic),
            timeline(ShareLevel::Read),
            ShareMode::Live,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    let payload: serde_json::Value = p
        .world
        .scalar(
            "SELECT payload FROM items WHERE thread_id = $1 AND kind = 'marker' AND payload->>'marker' = 'shared'",
            p.reader,
        )
        .await;
    assert_eq!(payload["data"]["mode"], "live");
    assert_eq!(payload["data"]["what"]["node"]["name"], "Docs");
    let notices: serde_json::Value = p
        .world
        .scalar(
            "SELECT pending_notices FROM threads WHERE node_id = $1",
            p.reader,
        )
        .await;
    assert!(
        notices[0]["body"].as_str().unwrap().contains("list_shared"),
        "{notices}"
    );
    // The marker reads as a marker of the contract.
    let sync = p.world.sync(p.reader).await;
    assert!(sync.items.iter().any(|item| matches!(
        &item.payload,
        orch_api_contract::threads::ItemPayload::Marker(
            orch_api_contract::threads::MarkerPayload::Shared(_)
        )
    )));
}

#[tokio::test]
async fn somebody_who_may_not_change_the_page_shares_nothing() {
    let p = a_reader_and_a_topic_to_share().await;
    let doc = p
        .world
        .document_beside(p.topic, "Handbook", &["Be kind."])
        .await;
    let bea = p.world.person("bea").await;
    {
        let mut conn = p.world.pool.acquire().await.unwrap();
        access_testing::give_role(
            &mut conn,
            p.world.root,
            "user",
            bea,
            "viewer",
            p.world.ada,
            None,
        )
        .await
        .unwrap();
    }
    let result = share_as(
        &p.world,
        bea,
        &ask(
            document(doc),
            None,
            ShareMode::Live,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    assert!(
        matches!(
            result,
            Err(ShareError::Page(_) | ShareError::Denied(_) | ShareError::NotFound)
        ),
        "{result:?}"
    );
    let entries: i64 = p
        .world
        .count(
            "SELECT count(*) FROM permission_entries WHERE object_id = $1 AND holder_kind = 'agents'",
            doc,
        )
        .await;
    assert_eq!(entries, 0);
}

#[tokio::test]
async fn the_agents_of_a_topic_that_has_the_tools_off_cannot_be_shared_with_and_nothing_is_written()
{
    let p = a_reader_and_a_topic_to_share().await;
    let doc = p
        .world
        .document_beside(p.topic, "Handbook", &["Be kind."])
        .await;
    p.world
        .set(p.world.topic, "orch_tools.enabled", json!([]))
        .await;
    let result = share_as(
        &p.world,
        p.world.ada,
        &ask(
            document(doc),
            None,
            ShareMode::Live,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    assert!(
        matches!(
            result,
            Err(ShareError::Unavailable {
                reason: "tools_off",
                ..
            })
        ),
        "{result:?}"
    );
    let entries: i64 = p
        .world
        .count(
            "SELECT count(*) FROM permission_entries WHERE object_id = $1 AND holder_kind = 'agents'",
            doc,
        )
        .await;
    assert_eq!(entries, 0);
}

#[tokio::test]
async fn a_harness_that_cannot_be_given_the_tools_cannot_be_shared_with() {
    let world = common::World::new().await;
    let reader = world.thread("Reader").await;
    let topic = world.topic("Docs").await;
    let result = share_as(
        &world,
        world.ada,
        &ask(node(topic), None, ShareMode::Live, threads(&[reader]), "c1"),
    )
    .await;
    assert!(
        matches!(
            result,
            Err(ShareError::Unavailable {
                reason: "no_tools",
                ..
            })
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn sharing_is_all_or_nothing_over_the_threads_named() {
    let p = a_reader_and_a_topic_to_share().await;
    let doc = p
        .world
        .document_beside(p.topic, "Handbook", &["Be kind."])
        .await;
    let result = share_as(
        &p.world,
        p.world.ada,
        &ask(
            document(doc),
            None,
            ShareMode::Live,
            threads(&[p.reader, orch_types::new_id()]),
            "c1",
        ),
    )
    .await;
    assert!(matches!(result, Err(ShareError::NotFound)), "{result:?}");
    let entries: i64 = p
        .world
        .count(
            "SELECT count(*) FROM permission_entries WHERE object_id = $1 AND holder_kind = 'agents'",
            doc,
        )
        .await;
    assert_eq!(entries, 0);
}

#[tokio::test]
async fn at_most_ten_threads_or_places_are_named_and_a_share_names_some() {
    let p = a_reader_and_a_topic_to_share().await;
    let none = share_as(
        &p.world,
        p.world.ada,
        &ask(node(p.topic), None, ShareMode::Live, threads(&[]), "c1"),
    )
    .await;
    assert!(matches!(none, Err(ShareError::Invalid { .. })));
    let many: Vec<Id> = (0..11).map(|_| orch_types::new_id()).collect();
    let too_many = share_as(
        &p.world,
        p.world.ada,
        &ask(node(p.topic), None, ShareMode::Live, threads(&many), "c2"),
    )
    .await;
    assert!(matches!(too_many, Err(ShareError::Invalid { .. })));
}

// ---------------------------------------------------------------- copies

#[tokio::test]
async fn a_copy_is_refused_while_nothing_can_keep_its_files() {
    let p = a_reader_and_a_topic_to_share().await;
    let result = share_as(
        &p.world,
        p.world.ada,
        &ask(
            node(p.topic),
            None,
            ShareMode::Snapshot,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    assert!(
        matches!(result, Err(ShareError::Unsupported(_))),
        "{result:?}"
    );
    let rows: i64 = p
        .world
        .count(
            "SELECT count(*) FROM agent_shares WHERE thread_id = $1",
            p.reader,
        )
        .await;
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn a_copy_holds_what_the_tree_reads_and_waits_for_the_next_turn() {
    let p = a_reader_and_a_topic_to_share().await;
    let parcels = Arc::new(shared::MemoryParcels::default());
    p.world.engine().attach_parcels(parcels.clone());
    let writer = thread_in_topic(&p.world, p.topic, "Writer").await;
    talk(
        &p.world,
        writer,
        "Why does the login fail?",
        "The cookie is dropped.",
    )
    .await;
    the_agent_takes_tools_again(&p.world).await;
    let doc = p
        .world
        .document_beside(p.topic, "Plan", &["First we build."])
        .await;
    let made = share_live(
        &p.world,
        &ask(
            node(p.topic),
            Some(ShareParts {
                timeline: Some(ShareLevel::Read),
                chat: None,
                documents: Some(ShareDocumentsLevel::Read),
            }),
            ShareMode::Snapshot,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    let copy = made[0].copy.as_ref().unwrap();
    assert_eq!(copy.state, "pending");
    assert!(copy.path.starts_with(".orch/shared/"));
    {
        let kept = parcels.kept.lock().unwrap();
        assert_eq!(kept.len(), 1);
        let files = &kept[0].2;
        let names: Vec<&str> = files.files.iter().map(|file| file.path.as_str()).collect();
        assert!(names.contains(&"index.md"), "{names:?}");
        assert!(
            names
                .iter()
                .any(|name| name.ends_with("/Writer/conversation.md")),
            "{names:?}"
        );
        assert!(
            names.iter().any(|name| name.ends_with("/docs/Plan.md")),
            "{names:?}"
        );
        assert_eq!(
            i64::from(copy.file_count),
            i64::try_from(files.files.len()).unwrap()
        );
    }
    // The text is the text the tree gives an agent that reads it.
    share_live(
        &p.world,
        &ask(
            node(writer),
            timeline(ShareLevel::Read),
            ShareMode::Live,
            threads(&[p.reader]),
            "c2",
        ),
    )
    .await;
    let listing = ok(call(
        &p.world,
        p.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{writer}") }),
    )
    .await);
    let live = ok(read(
        &p,
        entry(&listing, "conversation.md")["path"].as_str().unwrap(),
    )
    .await);
    let kept = parcels.kept.lock().unwrap();
    let in_copy = kept[0]
        .2
        .files
        .iter()
        .find(|file| file.path.ends_with("/Writer/conversation.md"))
        .unwrap();
    assert_eq!(
        String::from_utf8(in_copy.bytes.clone()).unwrap(),
        live["text"].as_str().unwrap()
    );
    let _ = doc;
}

#[tokio::test]
async fn a_copy_leaves_out_what_the_person_who_shares_may_not_read_and_counts_it() {
    let p = a_reader_and_a_topic_to_share().await;
    let parcels = Arc::new(shared::MemoryParcels::default());
    p.world.engine().attach_parcels(parcels.clone());
    p.world.document_beside(p.topic, "Open", &["Anyone."]).await;
    let secret = p
        .world
        .document_beside(p.topic, "Secret", &["Only owners."])
        .await;
    let bea = p.world.person("bea").await;
    {
        let mut conn = p.world.pool.acquire().await.unwrap();
        // A developer reads and drives what is made in the world.
        access_testing::give_role(
            &mut conn,
            p.world.root,
            "user",
            bea,
            "developer",
            p.world.ada,
            None,
        )
        .await
        .unwrap();
    }
    make_private(&p.world, secret).await;
    let reader = p.reader;
    let result = share_as(
        &p.world,
        bea,
        &ask(
            node(p.topic),
            documents(ShareDocumentsLevel::Read),
            ShareMode::Snapshot,
            threads(&[reader]),
            "c1",
        ),
    )
    .await;
    let made = result.unwrap();
    let kept = parcels.kept.lock().unwrap();
    let names: Vec<&str> = kept[0]
        .2
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert!(
        names.iter().any(|name| name.ends_with("/docs/Open.md")),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|name| name.contains("Secret")),
        "{names:?}"
    );
    assert_eq!(kept[0].2.left_out, 1);
    let index = kept[0]
        .2
        .files
        .iter()
        .find(|file| file.path == "index.md")
        .unwrap();
    assert!(
        String::from_utf8(index.bytes.clone())
            .unwrap()
            .contains("1 things in it were left out")
    );
    assert_eq!(made[0].by, Some(bea));
}

#[tokio::test]
async fn a_copy_larger_than_the_limit_is_refused_and_nothing_is_written() {
    let p = a_reader_and_a_topic_to_share().await;
    let parcels = Arc::new(shared::MemoryParcels::default());
    p.world.engine().attach_parcels(parcels.clone());
    p.world
        .document_beside(p.topic, "Plan", &["First we build."])
        .await;
    p.world
        .set(p.world.project, "sharing.snapshot_max_bytes", json!(50))
        .await;
    let result = share_as(
        &p.world,
        p.world.ada,
        &ask(
            node(p.topic),
            documents(ShareDocumentsLevel::Read),
            ShareMode::Snapshot,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    match result {
        Err(ShareError::TooLarge {
            limit,
            bytes,
            scope,
        }) => {
            assert_eq!(limit, 50);
            assert!(bytes > 50);
            assert_eq!(scope, Some(p.topic));
        }
        other => panic!("{other:?}"),
    }
    assert!(parcels.kept.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_copy_cannot_be_shared_for_writing() {
    let p = a_reader_and_a_topic_to_share().await;
    p.world
        .engine()
        .attach_parcels(Arc::new(shared::MemoryParcels::default()));
    let result = share_as(
        &p.world,
        p.world.ada,
        &ask(
            node(p.topic),
            timeline(ShareLevel::Write),
            ShareMode::Snapshot,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    assert!(
        matches!(result, Err(ShareError::Invalid { .. })),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_copy_with_a_comment_queues_an_input_by_the_person_and_taking_it_back_removes_it_from_the_workspace()
 {
    let p = a_reader_and_a_topic_to_share().await;
    let parcels = Arc::new(shared::MemoryParcels::default());
    p.world.engine().attach_parcels(parcels.clone());
    let mut request = ask(
        node(p.topic),
        documents(ShareDocumentsLevel::Read),
        ShareMode::Snapshot,
        threads(&[p.reader]),
        "c1",
    );
    request.comment = Some("Please read these first.".to_owned());
    p.world
        .document_beside(p.topic, "Plan", &["First we build."])
        .await;
    let made = share_live(&p.world, &request).await;
    let input = made[0].input.expect("the comment is an input");
    let (author, content): (Option<Id>, serde_json::Value) =
        sqlx::query_as("SELECT author_id, content FROM thread_inputs WHERE id = $1")
            .bind(input)
            .fetch_one(&mut *p.world.pool.acquire().await.unwrap())
            .await
            .unwrap();
    assert_eq!(author, Some(p.world.ada));
    assert_eq!(content[0]["text"], "Please read these first.");
    let again = share_live(&p.world, &request).await;
    assert_eq!(again[0].id, made[0].id);
    assert!(!again[0].fresh);
    assert_eq!(
        again[0].input,
        Some(input),
        "a repeat finds the input it made"
    );
    p.world
        .engine()
        .revoke_share(
            &Caller::session(p.world.ada),
            &Origin::default(),
            made[0].id,
        )
        .await
        .unwrap();
    let state: String = p
        .world
        .scalar("SELECT state FROM agent_shares WHERE id = $1", made[0].id)
        .await;
    assert_eq!(state, "revoked");
    assert_eq!(parcels.withdrawn.lock().unwrap().as_slice(), &[made[0].id]);
}

// ---------------------------------------------------------------- listing

#[tokio::test]
async fn what_was_shared_is_listed_with_the_thing_and_with_the_place_and_with_the_thread() {
    let p = a_reader_and_a_topic_to_share().await;
    let doc = p
        .world
        .document_beside(p.topic, "Handbook", &["Be kind."])
        .await;
    share_live(
        &p.world,
        &ask(
            document(doc),
            None,
            ShareMode::Live,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    share_live(
        &p.world,
        &ask(
            node(p.topic),
            timeline(ShareLevel::Read),
            ShareMode::Live,
            places(&[p.world.topic]),
            "c2",
        ),
    )
    .await;
    let engine = p.world.engine();
    let ada = Caller::session(p.world.ada);
    let of_doc = engine
        .shares_of(
            &ada,
            &Origin::default(),
            orch_engine::sharing::Subject::Document(doc),
        )
        .await
        .unwrap();
    assert_eq!(of_doc.len(), 1);
    assert_eq!(of_doc[0].to, p.reader);
    let to_topic = engine
        .shares_to_place(&ada, &Origin::default(), p.world.topic)
        .await
        .unwrap();
    assert_eq!(to_topic.len(), 1, "the place's own share, not the thread's");
    assert_eq!(to_topic[0].to, p.world.topic);
    let of_thread = engine
        .shares_of_thread(&ada, &Origin::default(), p.reader)
        .await
        .unwrap();
    assert_eq!(
        of_thread.shares.len(),
        2,
        "what names the thread and what names a place above it"
    );
}

#[tokio::test]
async fn somebody_who_may_not_see_a_thing_is_not_told_that_it_is_shared() {
    let p = a_reader_and_a_topic_to_share().await;
    let doc = p
        .world
        .document_beside(p.topic, "Secret", &["Only owners."])
        .await;
    share_live(
        &p.world,
        &ask(
            document(doc),
            None,
            ShareMode::Live,
            threads(&[p.reader]),
            "c1",
        ),
    )
    .await;
    let bea = p.world.person("bea").await;
    {
        let mut conn = p.world.pool.acquire().await.unwrap();
        access_testing::give_role(
            &mut conn,
            p.world.root,
            "user",
            bea,
            "viewer",
            p.world.ada,
            None,
        )
        .await
        .unwrap();
    }
    // The document is cut loose from the topic's rows: its owners alone hold it.
    make_private(&p.world, doc).await;
    let seen = p
        .world
        .engine()
        .shares_of_thread(&Caller::session(bea), &Origin::default(), p.reader)
        .await;
    match seen {
        Ok(shares) => assert!(shares.shares.is_empty(), "{:?}", shares.shares),
        Err(ShareError::NotFound) => {}
        Err(other) => panic!("{other:?}"),
    }
}
