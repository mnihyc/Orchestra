//! Automatic reference labels are optional copied source content, bounded by this key's operation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod support;

use desk::{Desk, paragraph, put};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    BlockContent, DocumentGet, DocumentOp, DocumentOpsCreate, DocumentOpsCreateBody,
    DocumentOpsCreateResponse, Inline, InlineRefKind,
};
use orch_types::{Id, new_id};
use serde_json::{Value, json};

/// Store a real rule-mode credential with no legacy fallback. The one-hour lifetime remains
/// within ordinary place bounds while these fixtures keep their clock fixed.
async fn operation_key(desk: &Desk, rules: Value) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};

    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let stored = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let now = desk.world.now();
    let mut conn = desk.world.conn().await;
    create_api_token_scoped(
        &mut conn,
        &NewApiToken {
            id: new_id(),
            user_id: desk.ada.id(),
            name: "Reference label source control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: now,
            expires_at: Some(now + chrono::Duration::hours(1)),
        },
        Some(&stored),
    )
    .await
    .unwrap();
    secret
}

fn at(document: Id, actions: &[&str]) -> Value {
    json!({
        "target":{"kind":"document","id":document,"reach":"self"},
        "allow":[{"operation":"DocumentOpsCreate","actions":actions}]
    })
}

fn reference(source: Id, label: Option<&str>) -> (Id, DocumentOp) {
    reference_of(InlineRefKind::Document, source, label)
}

fn reference_of(kind: InlineRefKind, source: Id, label: Option<&str>) -> (Id, DocumentOp) {
    let mut block = paragraph("");
    block.content = Some(BlockContent::Inline(vec![Inline::Ref {
        display: None,
        kind,
        id: source,
        path: None,
        part: None,
        label: label.map(str::to_owned),
        name: None,
    }]));
    (block.id, put(block))
}

async fn send(desk: &Desk, key: &str, document: Id, ops: Vec<DocumentOp>) {
    let reply = desk
        .world
        .app
        .call(DocumentOpsCreate, &[&document])
        .token(key)
        .body(&DocumentOpsCreateBody {
            expected_seq: None,
            client_id: new_id().to_string(),
            ops,
        })
        .send()
        .await;
    let raw: Value = serde_json::from_slice(&reply.raw).unwrap();
    let DocumentOpsCreateResponse::Receipt(receipt) = reply.ok() else {
        panic!("source Read does not grant Read of the destination response");
    };
    assert_eq!(
        raw,
        json!({"projection":"receipt","document_id":document,"seq":receipt.seq})
    );
}

async fn stored_block(desk: &Desk, document: Id, block: Id) -> Value {
    sqlx::query_scalar("SELECT content FROM document_blocks WHERE document_id=$1 AND id=$2")
        .bind(document)
        .bind(block)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_document_write_key_omits_out_of_scope_auto_labels_but_copies_an_allowed_source() {
    let desk = Desk::new().await;
    let destination = desk.make(&desk.ada, "Destination").await;
    let source = desk
        .make(&desk.ada, "Source title outside the first key")
        .await;
    let ordinary = desk
        .world
        .app
        .call(DocumentGet, &[&source.base.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(ordinary.base.title, "Source title outside the first key");
    let write_only = operation_key(&desk, json!([at(destination.base.id, &["write"])])).await;
    let source_read = operation_key(
        &desk,
        json!([
            at(destination.base.id, &["write"]),
            at(source.base.id, &["read"])
        ]),
    )
    .await;
    let (hidden_label, op) = reference(source.base.id, None);
    send(&desk, &write_only, destination.base.id, vec![op]).await;
    let stored = stored_block(&desk, destination.base.id, hidden_label).await;
    assert!(
        !stored
            .to_string()
            .contains("Source title outside the first key"),
        "an internal readable-name lookup must not copy a name outside this key's scope"
    );
    let full = desk.sync(&desk.ada, destination.base.id).await;
    let BlockContent::Inline(runs) = &full.blocks[0].content else {
        panic!("a reference paragraph");
    };
    assert!(matches!(&runs[0], Inline::Ref { label: None, .. }));

    let (captured_label, op) = reference(source.base.id, None);
    send(&desk, &source_read, destination.base.id, vec![op]).await;
    let stored = stored_block(&desk, destination.base.id, captured_label).await;
    assert!(
        stored
            .to_string()
            .contains("Source title outside the first key")
    );
    let full = desk.sync(&desk.ada, destination.base.id).await;
    let block = full
        .blocks
        .iter()
        .find(|block| block.id == captured_label)
        .unwrap();
    let BlockContent::Inline(runs) = &block.content else {
        panic!("a reference paragraph");
    };
    assert!(matches!(
        &runs[0],
        Inline::Ref { label: Some(label), .. } if label == "Source title outside the first key"
    ));
    assert_eq!(
        full.blocks.len(),
        2,
        "optional autofill never refuses the authorized write"
    );
}

#[tokio::test]
async fn supplied_reference_labels_need_no_source_read_even_when_empty() {
    let desk = Desk::new().await;
    let destination = desk.make(&desk.ada, "Written references").await;
    let source = desk.make(&desk.ada, "An ungranted source name").await;
    let key = operation_key(&desk, json!([at(destination.base.id, &["write"])])).await;
    let refused = desk
        .world
        .app
        .call(DocumentGet, &[&source.base.id])
        .token(&key)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    for supplied in ["The caller's label", ""] {
        let (block, op) = reference(source.base.id, Some(supplied));
        send(&desk, &key, destination.base.id, vec![op]).await;
        let stored = stored_block(&desk, destination.base.id, block).await;
        assert!(!stored.to_string().contains("An ungranted source name"));
        let full = desk.sync(&desk.ada, destination.base.id).await;
        let row = full.blocks.iter().find(|row| row.id == block).unwrap();
        let BlockContent::Inline(runs) = &row.content else {
            panic!("a reference paragraph");
        };
        assert!(matches!(
            &runs[0],
            Inline::Ref { label: Some(label), .. } if label == supplied
        ));
    }
}

/// Inspect the persisted label, not the current readable `name` supplied by a later view.
async fn label_of(desk: &Desk, document: Id, block: Id) -> Option<String> {
    let shown = desk.sync(&desk.ada, document).await;
    let block = shown.blocks.iter().find(|row| row.id == block).unwrap();
    let BlockContent::Inline(runs) = &block.content else {
        panic!("a reference paragraph");
    };
    let Inline::Ref { label, .. } = &runs[0] else {
        panic!("a reference");
    };
    label.clone()
}

fn source_at(kind: &str, id: Id) -> Value {
    json!({
        "target":{"kind":kind,"id":id,"reach":"self"},
        "allow":[{"operation":"DocumentOpsCreate","actions":["read"]}]
    })
}

/// The same stored file pointer is used in every request. Only Read on its actual owner may
/// supply its name: neither no Read nor exact Read on the ambient topic supplies that authority.
async fn file_owner_controls(
    desk: &Desk,
    destination: Id,
    file: Id,
    filename: &str,
    owner_kind: &str,
    owner: Id,
) {
    assert_ne!(
        file, owner,
        "the real file/reference UUID is not its owner's identity"
    );
    let (ordinary, op) = reference_of(InlineRefKind::File, file, None);
    desk.write(&desk.ada, destination, vec![op]).await.ok();
    assert_eq!(
        label_of(desk, destination, ordinary).await.as_deref(),
        Some(filename),
        "the same account's ordinary producer really resolves this file name"
    );
    for (extra_read, expected) in [
        (None, None),
        (Some(source_at("topic", desk.topic)), None),
        (Some(source_at(owner_kind, owner)), Some(filename)),
    ] {
        let mut rules = vec![at(destination, &["write"])];
        rules.extend(extra_read);
        let key = operation_key(desk, json!(rules)).await;
        let (block, op) = reference_of(InlineRefKind::File, file, None);
        send(desk, &key, destination, vec![op]).await;
        assert_eq!(
            label_of(desk, destination, block).await.as_deref(),
            expected
        );
        let stored = stored_block(desk, destination, block).await;
        assert_eq!(
            stored.to_string().contains(filename),
            expected.is_some(),
            "the raw stored block copies the file name only with its actual owner's Read"
        );
    }
}

#[tokio::test]
async fn a_document_file_label_uses_its_actual_document_owner_not_its_uuid_or_ambient_topic() {
    use orch_api_contract::documents::{DocumentFileCreate, DocumentFileCreateQuery};
    use orch_api_contract::threads::FILENAME_HEADER;

    let desk = Desk::new().await;
    let destination = desk.make(&desk.ada, "References to attached files").await;
    let source = desk.make(&desk.ada, "The attachment owner").await;
    let file = desk
        .world
        .app
        .call(DocumentFileCreate, &[&source.base.id])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .header(FILENAME_HEADER, "private-document-attachment.txt")
        .bytes(b"Real document attachment bytes")
        .send()
        .await
        .ok();
    let owner: Id = sqlx::query_scalar("SELECT document_id FROM document_files WHERE id=$1")
        .bind(file.id)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        owner, source.base.id,
        "the normal upload stored the association"
    );
    file_owner_controls(
        &desk,
        destination.base.id,
        file.id,
        &file.name,
        "document",
        owner,
    )
    .await;
}

#[tokio::test]
async fn a_carried_file_label_uses_its_actual_chat_owner_not_its_reference_uuid_or_ambient_topic() {
    use orch_api_contract::chats::{
        ChatAttachmentCreate, ChatBlobGet, ChatBlobGetQuery, ChatMessageCreate,
        ChatMessageCreateBody, ChatPart,
    };
    use orch_api_contract::nodes::Node;
    use orch_api_contract::threads::FILENAME_HEADER;

    let desk = Desk::new().await;
    let destination = desk.make(&desk.ada, "References to carried files").await;
    let Node::Topic(topic) = places::read(&desk.world, &desk.ada, desk.topic).await else {
        panic!("the existing topic");
    };
    let chat = topic.base.chat_id;
    let uploaded = desk
        .world
        .app
        .call(ChatAttachmentCreate, &[&chat])
        .token(&desk.ada.token)
        .header(FILENAME_HEADER, "private-chat-attachment.txt")
        .bytes(b"Real carried file bytes")
        .send()
        .await
        .ok();
    let message = desk
        .world
        .app
        .call(ChatMessageCreate, &[&chat])
        .token(&desk.ada.token)
        .body(&ChatMessageCreateBody {
            parts: vec![ChatPart::Attachment {
                blob: uploaded.blob,
                name: uploaded.name.clone(),
                size: uploaded.size,
                media_type: uploaded.media_type.clone(),
            }],
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the browser receives its full ordinary message")
        .message;
    // The existing preview endpoint creates the stable carried-file reference. No synthetic
    // chat_file_references row or guessed reference UUID supplies the positive premise.
    let preview = desk
        .world
        .app
        .call(ChatBlobGet, &[&chat, &uploaded.blob])
        .token(&desk.ada.token)
        .query(&ChatBlobGetQuery {
            preview: Some(true),
            download: None,
        })
        .send_bytes()
        .await;
    assert_eq!(preview.status, 200);
    let reference: Id = preview.headers["x-orch-file-id"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let (owner, carrying, slot): (Id, Id, i32) =
        sqlx::query_as("SELECT chat_id,message_id,carried FROM chat_file_references WHERE id=$1")
            .bind(reference)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert_eq!((owner, carrying, slot), (chat, message.id, 1));
    file_owner_controls(
        &desk,
        destination.base.id,
        reference,
        &uploaded.name,
        "chat",
        owner,
    )
    .await;
}

fn assert_stored_reference_and_name(
    block: &orch_api_contract::documents::DocumentBlock,
    source: Id,
    expected_name: Option<&str>,
) {
    let BlockContent::Inline(runs) = &block.content else {
        panic!("the stored paragraph content remains intact");
    };
    assert!(runs.iter().any(|run| matches!(
        run, Inline::Text { text, .. } if text == "Stored surrounding text"
    )));
    let reference = runs
        .iter()
        .find(|run| matches!(run, Inline::Ref { id, .. } if *id == source))
        .unwrap();
    let Inline::Ref { label, name, .. } = reference else {
        unreachable!();
    };
    assert_eq!(label.as_deref(), Some("Stored snapshot label"));
    assert_eq!(name, &Some(expected_name.map(str::to_owned)));
}

fn change_props(block: &orch_api_contract::documents::DocumentBlock, color: u32) -> DocumentOp {
    use orch_api_contract::documents::{BlockProps, TextProps};
    DocumentOp::Update {
        id: block.id,
        base_seq: block.seq_updated,
        block_type: None,
        props: Some(BlockProps::Text(TextProps {
            color: Some(color),
            ..TextProps::default()
        })),
        content: None,
    }
}

#[tokio::test]
async fn full_document_mutations_replay_and_conflicts_preserve_stored_labels_but_scope_dynamic_names()
 {
    use orch_api_contract::documents::DocumentBlock;

    let desk = Desk::new().await;
    let destination = desk.make(&desk.ada, "Readable destination").await;
    let source = desk.make(&desk.ada, "Current external source name").await;
    let mut block = paragraph("Stored surrounding text");
    let Some(BlockContent::Inline(runs)) = &mut block.content else {
        panic!("a paragraph");
    };
    runs.push(Inline::Ref {
        display: None,
        kind: InlineRefKind::Document,
        id: source.base.id,
        path: None,
        part: None,
        label: Some("Stored snapshot label".into()),
        name: None,
    });
    let seeded = desk
        .write(&desk.ada, destination.base.id, vec![put(block)])
        .await
        .ok();
    let original = seeded.blocks[0].clone();
    assert_stored_reference_and_name(
        &original,
        source.base.id,
        Some("Current external source name"),
    );
    let destination_only =
        operation_key(&desk, json!([at(destination.base.id, &["write", "read"])])).await;
    let both = operation_key(
        &desk,
        json!([
            at(destination.base.id, &["write", "read"]),
            at(source.base.id, &["read"])
        ]),
    )
    .await;
    // The new input changes only props: the reference and its label already reside in the
    // touched stored block, so inspecting only references in the submitted body is insufficient.
    let first_body = DocumentOpsCreateBody {
        expected_seq: None,
        client_id: new_id().to_string(),
        ops: vec![change_props(&original, 3)],
    };
    let first = desk
        .world
        .app
        .call(DocumentOpsCreate, &[&destination.base.id])
        .token(&destination_only)
        .body(&first_body)
        .send()
        .await;
    assert!(
        !String::from_utf8(first.raw.to_vec())
            .unwrap()
            .contains("Current external source name")
    );
    let first = first
        .ok()
        .into_full()
        .expect("Read on D permits D's full stored content");
    assert_eq!(first.document.title, "Readable destination");
    assert_eq!(first.blocks.len(), 1);
    assert_stored_reference_and_name(&first.blocks[0], source.base.id, None);

    let second_body = DocumentOpsCreateBody {
        expected_seq: None,
        client_id: new_id().to_string(),
        ops: vec![change_props(&first.blocks[0], 4)],
    };
    let second = desk
        .world
        .app
        .call(DocumentOpsCreate, &[&destination.base.id])
        .token(&both)
        .body(&second_body)
        .send()
        .await
        .ok()
        .into_full()
        .expect("same-operation Read on both actual resources allows the current reference name");
    assert_eq!(second.blocks.len(), 1);
    assert_stored_reference_and_name(
        &second.blocks[0],
        source.base.id,
        Some("Current external source name"),
    );
    let raw_stored = stored_block(&desk, destination.base.id, original.id).await;
    assert!(raw_stored.to_string().contains("Stored snapshot label"));
    assert!(
        !raw_stored
            .to_string()
            .contains("Current external source name")
    );

    // Same account and accepted client_id, but the current credential cannot disclose S.
    let replay = desk
        .world
        .app
        .call(DocumentOpsCreate, &[&destination.base.id])
        .token(&destination_only)
        .body(&second_body)
        .send()
        .await;
    assert!(
        !String::from_utf8(replay.raw.to_vec())
            .unwrap()
            .contains("Current external source name")
    );
    let replay = replay
        .ok()
        .into_full()
        .expect("the current key still reads D");
    assert_eq!(replay.seq, second.seq);
    assert_eq!(replay.blocks.len(), 1);
    assert_stored_reference_and_name(&replay.blocks[0], source.base.id, None);

    let stale_body = DocumentOpsCreateBody {
        expected_seq: None,
        client_id: new_id().to_string(),
        ops: vec![change_props(&original, 5)],
    };
    for (key, name) in [
        (&destination_only, None),
        (&both, Some("Current external source name")),
    ] {
        let refused = desk
            .world
            .app
            .call(DocumentOpsCreate, &[&destination.base.id])
            .token(key)
            .body(&stale_body)
            .send()
            .await;
        if name.is_none() {
            assert!(
                !String::from_utf8(refused.raw.to_vec())
                    .unwrap()
                    .contains("Current external source name")
            );
        }
        let problem = refused.problem();
        assert_eq!(problem.code, ErrorCode::ConflictStaleRevision);
        let current: Vec<DocumentBlock> =
            serde_json::from_value(problem.extra["current"].clone()).unwrap();
        assert_eq!(current.len(), 1);
        assert_stored_reference_and_name(&current[0], source.base.id, name);
    }
    assert_eq!(
        desk.sync(&desk.ada, destination.base.id).await.seq,
        second.seq
    );
}

#[tokio::test]
async fn full_document_responses_scope_the_parent_name_and_its_inline_alias_together() {
    use orch_api_contract::nodes::NodeKind;

    let desk = Desk::new().await;
    let destination = desk
        .make(&desk.ada, "Document metadata remains readable")
        .await;
    let (block, op) = reference_of(
        InlineRefKind::Node,
        desk.topic,
        Some("Stored parent snapshot"),
    );
    let baseline = desk
        .write(&desk.ada, destination.base.id, vec![op])
        .await
        .ok();
    let parent_name = baseline
        .document
        .node
        .name
        .clone()
        .expect("the ordinary owner can name the parent");
    assert_eq!(baseline.document.node.kind, NodeKind::Topic);
    assert!(!baseline.document.node.redacted);
    let original = baseline.blocks.iter().find(|row| row.id == block).unwrap();
    let BlockContent::Inline(runs) = &original.content else {
        panic!("a reference paragraph");
    };
    assert!(matches!(&runs[0], Inline::Ref { name: Some(Some(name)), .. } if name == &parent_name));
    let key_without_parent =
        operation_key(&desk, json!([at(destination.base.id, &["write", "read"])])).await;
    let key_with_parent = operation_key(
        &desk,
        json!([
            at(destination.base.id, &["write", "read"]),
            source_at("topic", desk.topic)
        ]),
    )
    .await;
    let mut current = original.clone();
    for (key, color, expected) in [
        (&key_without_parent, 6, None),
        (&key_with_parent, 7, Some(parent_name.as_str())),
    ] {
        let response = desk
            .world
            .app
            .call(DocumentOpsCreate, &[&destination.base.id])
            .token(key)
            .body(&DocumentOpsCreateBody {
                expected_seq: None,
                client_id: new_id().to_string(),
                ops: vec![change_props(&current, color)],
            })
            .send()
            .await;
        if expected.is_none() {
            assert!(
                !String::from_utf8(response.raw.to_vec())
                    .unwrap()
                    .contains(&parent_name),
                "the same protected name cannot leak through attribution/parent aliases"
            );
        }
        let response = response
            .ok()
            .into_full()
            .expect("the key still reads all stored D content");
        assert_eq!(
            response.document.title,
            "Document metadata remains readable"
        );
        assert_eq!(response.document.node.id, desk.topic);
        assert_eq!(response.document.node.kind, NodeKind::Topic);
        assert_eq!(response.document.node.name.as_deref(), expected);
        assert_eq!(response.document.node.redacted, expected.is_none());
        assert!(response.document.node.thread_display.is_none());
        assert_eq!(
            response.document.owner.id,
            desk.ada.id(),
            "public UserRef identity semantics are unchanged"
        );
        assert_eq!(response.document.owner.username, "ada");
        current = response
            .blocks
            .into_iter()
            .find(|row| row.id == block)
            .unwrap();
        let BlockContent::Inline(runs) = &current.content else {
            panic!("a reference paragraph");
        };
        let Inline::Ref { label, name, .. } = &runs[0] else {
            panic!("a reference");
        };
        assert_eq!(label.as_deref(), Some("Stored parent snapshot"));
        assert_eq!(name, &Some(expected.map(str::to_owned)));
    }
}
