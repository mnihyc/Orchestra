//! Current document readers share ordinary content while bounding live external labels.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod realtime_support;
mod support;

use desk::{Desk, paragraph, put};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    BlockContent, Document, DocumentBlock, DocumentDelete, DocumentFileCreate,
    DocumentFileCreateQuery, DocumentGet, DocumentSync, DocumentSyncGet, DocumentSyncGetQuery,
    Inline, InlineRefKind, NewBlock,
};
use orch_api_contract::nodes::DeletedInspectionQuery;
use orch_api_contract::threads::FILENAME_HEADER;
use orch_api_core::testing::Reply;
use orch_types::{Id, new_id};
use serde_json::{Value, json};

struct Key {
    id: Id,
    secret: String,
}

fn rule(operation: &str, kind: &str, id: Id, action: &str) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":[action]}]})
}

async fn key(world: &support::World, user: Id, rules: Vec<Value>) -> Key {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let now = world.now();
    create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Document reader control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: now,
            expires_at: Some(now + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    Key { id, secret }
}

async fn narrow_key(world: &support::World, key: &Key, rules: Vec<Value>) {
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(key.id)
        .bind(serde_json::to_value(scope).unwrap())
        .execute(&world.db.pool)
        .await
        .unwrap();
}

async fn get(desk: &Desk, token: &str, document: Id, inspect: bool) -> Reply<Document> {
    desk.world
        .app
        .call(DocumentGet, &[&document])
        .token(token)
        .query(&DeletedInspectionQuery {
            inspect_deleted: inspect.then_some(true),
        })
        .send()
        .await
}

async fn sync(
    desk: &Desk,
    token: &str,
    document: Id,
    since: Option<i64>,
    inspect: bool,
) -> Reply<DocumentSync> {
    desk.world
        .app
        .call(DocumentSyncGet, &[&document])
        .token(token)
        .query(&DocumentSyncGetQuery {
            since,
            inspect_deleted: inspect.then_some(true),
        })
        .send()
        .await
}

fn roots(desk: &Desk, operation: &str) -> Vec<Value> {
    vec![
        rule(operation, "org", desk.org, "read"),
        rule(operation, "project", desk.project, "read"),
        rule(operation, "topic", desk.topic, "read"),
    ]
}

fn inline_ref(kind: InlineRefKind, id: Id, label: &str) -> Inline {
    Inline::Ref {
        display: None,
        kind,
        id,
        path: None,
        part: None,
        label: Some(label.into()),
        name: None,
    }
}

fn block_ref(kind: InlineRefKind, id: Id, label: &str) -> NewBlock {
    let mut block = paragraph("Stored text stays. ");
    let Some(BlockContent::Inline(runs)) = &mut block.content else {
        panic!("paragraph")
    };
    runs.push(inline_ref(kind, id, label));
    block
}

fn assert_ref(block: &DocumentBlock, id: Id, label: &str, name: Option<&str>) {
    let BlockContent::Inline(runs) = &block.content else {
        panic!("inline paragraph")
    };
    let found = runs
        .iter()
        .find(|run| matches!(run,Inline::Ref{id:found,..}if *found==id))
        .unwrap();
    let Inline::Ref {
        label: stored,
        name: live,
        ..
    } = found
    else {
        unreachable!()
    };
    assert_eq!(stored.as_deref(), Some(label));
    assert_eq!(live, &Some(name.map(str::to_owned)));
}

#[tokio::test]
async fn each_document_reader_requires_its_exact_selection_target_and_current_ordinary_authority() {
    let desk = Desk::new().await;
    let viewer = desk.member("doc-reader", "viewer").await;
    let stranger = desk.stranger("no-document-rights").await;
    let document = desk.make(&desk.ada, "Readable document").await.base.id;
    let other = desk.make(&desk.ada, "Different document").await.base.id;
    for operation in ["DocumentGet.read", "DocumentSyncGet.read"] {
        let correct = key(
            &desk.world,
            viewer.id(),
            vec![rule(operation, "document", document, "read")],
        )
        .await;
        let wrong_target = key(
            &desk.world,
            viewer.id(),
            vec![rule(operation, "document", other, "read")],
        )
        .await;
        let wrong_operation = key(
            &desk.world,
            viewer.id(),
            vec![rule(
                if operation == "DocumentGet.read" {
                    "DocumentSyncGet.read"
                } else {
                    "DocumentGet.read"
                },
                "document",
                document,
                "read",
            )],
        )
        .await;
        let outside = key(
            &desk.world,
            stranger.id(),
            vec![rule(operation, "document", document, "read")],
        )
        .await;
        if operation == "DocumentGet.read" {
            assert_eq!(
                get(&desk, &viewer.token, document, false)
                    .await
                    .ok()
                    .base
                    .id,
                document
            );
            assert_eq!(
                get(&desk, &correct.secret, document, false)
                    .await
                    .ok()
                    .base
                    .id,
                document
            );
            for token in [
                &wrong_target.secret,
                &wrong_operation.secret,
                &outside.secret,
            ] {
                assert_eq!(
                    get(&desk, token, document, false).await.problem().code,
                    ErrorCode::AuthzDenied
                );
            }
        } else {
            assert_eq!(
                sync(&desk, &viewer.token, document, None, false)
                    .await
                    .ok()
                    .document
                    .base
                    .id,
                document
            );
            assert_eq!(
                sync(&desk, &correct.secret, document, None, false)
                    .await
                    .ok()
                    .document
                    .base
                    .id,
                document
            );
            for token in [
                &wrong_target.secret,
                &wrong_operation.secret,
                &outside.secret,
            ] {
                assert_eq!(
                    sync(&desk, token, document, None, false)
                        .await
                        .problem()
                        .code,
                    ErrorCode::AuthzDenied
                );
            }
        }
    }
}

#[tokio::test]
async fn live_labels_are_scoped_but_stored_words_files_and_public_people_remain_document_content() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Current reader document")
        .await
        .base
        .id;
    let source = desk.make(&desk.ada, "External live title").await.base.id;
    let mut block = paragraph("Stored literal: External live title; this is copied text. ");
    let block_id = block.id;
    let Some(BlockContent::Inline(runs)) = &mut block.content else {
        panic!("paragraph")
    };
    runs.push(inline_ref(
        InlineRefKind::Document,
        source,
        "Stored copied label",
    ));
    runs.push(inline_ref(
        InlineRefKind::Node,
        desk.topic,
        "Stored place label",
    ));
    desk.write(&desk.ada, document, vec![put(block)]).await.ok();
    let file = desk
        .world
        .app
        .call(DocumentFileCreate, &[&document])
        .token(&desk.ada.token)
        .query(&DocumentFileCreateQuery::default())
        .header(FILENAME_HEADER, "stored-file.txt")
        .bytes(b"Actual bytes already attached to this document")
        .send()
        .await
        .ok()
        .into_full()
        .expect("this native creation control requires its full response");
    let ordinary = sync(&desk, &desk.ada.token, document, None, false)
        .await
        .ok();
    let parent_name = ordinary.document.base.node.name.clone().unwrap();
    let source_block = ordinary
        .blocks
        .iter()
        .find(|row| row.id == block_id)
        .unwrap();
    assert_ref(
        source_block,
        source,
        "Stored copied label",
        Some("External live title"),
    );
    assert_ref(
        source_block,
        desk.topic,
        "Stored place label",
        Some(&parent_name),
    );
    for operation in ["DocumentGet.read", "DocumentSyncGet.read"] {
        let primary = rule(operation, "document", document, "read");
        let only = key(&desk.world, desk.ada.id(), vec![primary.clone()]).await;
        let mut rules = vec![primary.clone(), rule(operation, "document", source, "read")];
        rules.extend(roots(&desk, operation));
        let full = key(&desk.world, desk.ada.id(), rules).await;
        if operation == "DocumentGet.read" {
            let expected = get(&desk, &desk.ada.token, document, false).await.ok();
            let hidden = get(&desk, &only.secret, document, false).await.ok();
            assert!(hidden.base.node.redacted && hidden.base.node.name.is_none());
            assert_eq!(hidden.base.owner, expected.base.owner);
            assert_eq!(hidden.files, expected.files);
            assert!(
                hidden
                    .files
                    .iter()
                    .any(|entry| entry.id == file.id && entry.name == "stored-file.txt")
            );
            assert_eq!(
                get(&desk, &full.secret, document, false).await.ok(),
                expected
            );
            narrow_key(&desk.world, &full, vec![primary]).await;
            assert!(
                get(&desk, &full.secret, document, false)
                    .await
                    .ok()
                    .base
                    .node
                    .name
                    .is_none()
            );
        } else {
            let hidden = sync(&desk, &only.secret, document, None, false).await.ok();
            let row = hidden.blocks.iter().find(|row| row.id == block_id).unwrap();
            assert_ref(row, source, "Stored copied label", None);
            assert_ref(row, desk.topic, "Stored place label", None);
            assert!(
                serde_json::to_string(&row.content)
                    .unwrap()
                    .contains("Stored literal: External live title")
            );
            assert_eq!(row.updated_by, source_block.updated_by);
            assert_eq!(hidden.document.files, ordinary.document.files);
            let unrelated = key(
                &desk.world,
                desk.ada.id(),
                vec![
                    primary.clone(),
                    rule("DocumentGet.read", "document", source, "read"),
                    rule("DocumentGet.read", "topic", desk.topic, "read"),
                ],
            )
            .await;
            let unrelated = sync(&desk, &unrelated.secret, document, None, false)
                .await
                .ok();
            assert_ref(
                unrelated
                    .blocks
                    .iter()
                    .find(|row| row.id == block_id)
                    .unwrap(),
                source,
                "Stored copied label",
                None,
            );
            assert_eq!(
                sync(&desk, &full.secret, document, None, false).await.ok(),
                ordinary
            );
            narrow_key(&desk.world, &full, vec![primary]).await;
            let now = sync(&desk, &full.secret, document, None, false).await.ok();
            assert_ref(
                now.blocks.iter().find(|row| row.id == block_id).unwrap(),
                source,
                "Stored copied label",
                None,
            );
        }
        desk.world
            .app
            .call(orch_api_contract::auth::MeTokenDelete, &[&full.id])
            .token(&desk.ada.token)
            .send()
            .await
            .ok();
        let revoked = if operation == "DocumentGet.read" {
            get(&desk, &full.secret, document, false).await.problem()
        } else {
            sync(&desk, &full.secret, document, None, false)
                .await
                .problem()
        };
        assert_eq!(revoked.code, ErrorCode::AuthInvalidToken);
    }
}

#[tokio::test]
async fn folder_ancestry_names_need_their_own_read_without_hiding_stored_placement() {
    use orch_api_contract::document_tree::{
        DocumentContainerCreate, DocumentContainerCreateBody, DocumentDirectory,
        DocumentTreeCommand, DocumentTreeItem, DocumentTreeOperate, DocumentTreeOperationBody,
    };
    let desk = Desk::new().await;
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([42; 32]),
    );
    let made = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: None,
            name: "Protected folder label".into(),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    let Some(DocumentTreeItem::Container(folder)) = made.entries.first().map(|entry| entry.item)
    else {
        panic!("actual folder")
    };
    let document = desk.make(&desk.ada, "Stored child basename").await.base.id;
    desk.world
        .app
        .call(DocumentTreeOperate, &[])
        .token(&desk.ada.token)
        .body(&DocumentTreeOperationBody {
            command: DocumentTreeCommand::Move,
            items: vec![DocumentTreeItem::Document(document)],
            to: Some(DocumentDirectory::Container(folder)),
            name: None,
            client_id: new_id().to_string(),
            version: None,
            deletion_plan: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this native tree control requires its full response");
    let normal = get(&desk, &desk.ada.token, document, false).await.ok();
    assert!(
        serde_json::to_string(&normal.location.ancestors)
            .unwrap()
            .contains("Protected folder label")
    );
    for operation in ["DocumentGet.read", "DocumentSyncGet.read"] {
        let primary = rule(operation, "document", document, "read");
        let narrow = key(&desk.world, desk.ada.id(), vec![primary.clone()]).await;
        let mut rules = vec![
            primary,
            rule(operation, "document_container", folder, "read"),
        ];
        rules.extend(roots(&desk, operation));
        let full = key(&desk.world, desk.ada.id(), rules).await;
        let hidden = if operation == "DocumentGet.read" {
            get(&desk, &narrow.secret, document, false).await.ok()
        } else {
            sync(&desk, &narrow.secret, document, None, false)
                .await
                .ok()
                .document
        };
        assert_eq!(hidden.location.parent, normal.location.parent);
        assert_eq!(hidden.location.basename, normal.location.basename);
        let hidden_folder = hidden
            .location
            .ancestors
            .iter()
            .find(|row| row.id == folder)
            .expect("hidden folder identity remains");
        assert!(hidden_folder.basename.is_none() && hidden_folder.title.is_none());
        assert!(
            !serde_json::to_string(&hidden.location.ancestors)
                .unwrap()
                .contains("Protected folder label")
        );
        let shown = if operation == "DocumentGet.read" {
            get(&desk, &full.secret, document, false).await.ok()
        } else {
            sync(&desk, &full.secret, document, None, false)
                .await
                .ok()
                .document
        };
        assert_eq!(shown.location, normal.location);
    }
}

#[tokio::test]
async fn node_chat_fallback_title_and_carried_file_name_use_their_actual_distinct_sources() {
    use orch_api_contract::chats::{
        ChatAttachmentCreate, ChatBlobGet, ChatBlobGetQuery, ChatMessageCreate,
        ChatMessageCreateBody, ChatPart,
    };
    let desk = Desk::new().await;
    let target = desk
        .make(&desk.ada, "Names beside an actual chat file")
        .await
        .base
        .id;
    let external = desk.another_topic("External live node-chat title").await;
    let orch_api_contract::nodes::Node::Topic(topic) =
        places::read(&desk.world, &desk.ada, external).await
    else {
        panic!("topic")
    };
    let chat = topic.base.chat_id;
    let upload = desk
        .world
        .app
        .call(ChatAttachmentCreate, &[&chat])
        .token(&desk.ada.token)
        .header(FILENAME_HEADER, "carried-snapshot.txt")
        .bytes(b"Stored carried bytes")
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
                blob: upload.blob,
                name: upload.name.clone(),
                size: upload.size,
                media_type: upload.media_type.clone(),
            }],
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .message;
    let preview = desk
        .world
        .app
        .call(ChatBlobGet, &[&chat, &upload.blob])
        .token(&desk.ada.token)
        .query(&ChatBlobGetQuery {
            preview: Some(true),
            download: None,
        })
        .send_bytes()
        .await;
    assert_eq!(preview.status, 200);
    let file: Id = preview.headers["x-orch-file-id"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let witness: (Id, Id, i32) =
        sqlx::query_as("SELECT chat_id,message_id,carried FROM chat_file_references WHERE id=$1")
            .bind(file)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert_eq!(witness, (chat, message.id, 1));
    let chat_block = block_ref(InlineRefKind::Chat, chat, "Stored chat caption");
    let chat_block_id = chat_block.id;
    let file_block = block_ref(InlineRefKind::File, file, "Stored carried caption");
    let file_block_id = file_block.id;
    desk.write(&desk.ada, target, vec![put(chat_block), put(file_block)])
        .await
        .ok();
    let ordinary = sync(&desk, &desk.ada.token, target, None, false).await.ok();
    assert_ref(
        ordinary
            .blocks
            .iter()
            .find(|row| row.id == chat_block_id)
            .unwrap(),
        chat,
        "Stored chat caption",
        Some("External live node-chat title"),
    );
    assert_ref(
        ordinary
            .blocks
            .iter()
            .find(|row| row.id == file_block_id)
            .unwrap(),
        file,
        "Stored carried caption",
        Some("carried-snapshot.txt"),
    );
    let primary = rule("DocumentSyncGet.read", "document", target, "read");
    let only = key(&desk.world, desk.ada.id(), vec![primary.clone()]).await;
    let hidden = sync(&desk, &only.secret, target, None, false).await.ok();
    assert_ref(
        hidden
            .blocks
            .iter()
            .find(|row| row.id == chat_block_id)
            .unwrap(),
        chat,
        "Stored chat caption",
        None,
    );
    assert_ref(
        hidden
            .blocks
            .iter()
            .find(|row| row.id == file_block_id)
            .unwrap(),
        file,
        "Stored carried caption",
        None,
    );
    let chat_read = rule("DocumentSyncGet.read", "chat", chat, "read");
    let carried = key(
        &desk.world,
        desk.ada.id(),
        vec![primary.clone(), chat_read.clone()],
    )
    .await;
    let shown = sync(&desk, &carried.secret, target, None, false).await.ok();
    assert_ref(
        shown
            .blocks
            .iter()
            .find(|row| row.id == file_block_id)
            .unwrap(),
        file,
        "Stored carried caption",
        Some("carried-snapshot.txt"),
    );
    assert_ref(
        shown
            .blocks
            .iter()
            .find(|row| row.id == chat_block_id)
            .unwrap(),
        chat,
        "Stored chat caption",
        None,
    );
    let named = key(
        &desk.world,
        desk.ada.id(),
        vec![
            primary,
            chat_read,
            rule("DocumentSyncGet.read", "topic", external, "read"),
        ],
    )
    .await;
    let shown = sync(&desk, &named.secret, target, None, false).await.ok();
    assert_ref(
        shown
            .blocks
            .iter()
            .find(|row| row.id == chat_block_id)
            .unwrap(),
        chat,
        "Stored chat caption",
        Some("External live node-chat title"),
    );
}

#[tokio::test]
async fn deleted_inspection_needs_its_manage_selection_and_actual_restore_authority_without_auto_upgrade()
 {
    let desk = Desk::new().await;
    let viewer = desk.member("retained-viewer", "viewer").await;
    let document = desk
        .make(&desk.ada, "Retained private content")
        .await
        .base
        .id;
    desk.paragraphs(&desk.ada, document, &["Retained block words"])
        .await;
    desk.world
        .app
        .call(DocumentDelete, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        get(&desk, &desk.ada.token, document, false)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let counts = || async {
        sqlx::query_as::<_,(i64,i64,i64)>("SELECT (SELECT count(*) FROM document_states),(SELECT count(*) FROM notifications),(SELECT count(*) FROM document_versions)")
        .fetch_one(&desk.world.db.pool).await.unwrap()
    };
    let before = counts().await;
    for (live, inspect) in [
        ("DocumentGet.read", "DocumentGet.inspect_deleted"),
        ("DocumentSyncGet.read", "DocumentSyncGet.inspect_deleted"),
    ] {
        let read = key(
            &desk.world,
            desk.ada.id(),
            vec![rule(live, "document", document, "read")],
        )
        .await;
        let manager = key(
            &desk.world,
            desk.ada.id(),
            vec![rule(inspect, "document", document, "manage")],
        )
        .await;
        let no_authority = key(
            &desk.world,
            viewer.id(),
            vec![rule(inspect, "document", document, "manage")],
        )
        .await;
        if live == "DocumentGet.read" {
            let ordinary = get(&desk, &desk.ada.token, document, true).await.ok();
            let shown = get(&desk, &manager.secret, document, true).await.ok();
            assert_eq!(shown.base.title, ordinary.base.title);
            assert!(shown.inspection.unwrap().readonly);
            assert_eq!(
                get(&desk, &read.secret, document, true)
                    .await
                    .problem()
                    .code,
                ErrorCode::AuthzDenied
            );
            let refused = get(&desk, &viewer.token, document, true).await.problem();
            assert_eq!(refused.code, ErrorCode::AuthzDenied);
            assert_eq!(
                refused.extra["required"], "doc.manage",
                "the ordinary restoration guard supplies this refusal"
            );
            let refused = get(&desk, &no_authority.secret, document, true)
                .await
                .problem();
            assert_eq!(refused.code, ErrorCode::AuthzDenied);
            assert_eq!(
                refused.extra["required"], "doc.manage",
                "the ordinary restoration guard supplies this refusal"
            );
            assert!(matches!(
                get(&desk, &manager.secret, document, false)
                    .await
                    .problem()
                    .code,
                ErrorCode::AuthzDenied | ErrorCode::NotFound
            ));
        } else {
            let ordinary = sync(&desk, &desk.ada.token, document, None, true)
                .await
                .ok();
            let shown = sync(&desk, &manager.secret, document, None, true)
                .await
                .ok();
            assert!(shown.document.inspection.unwrap().readonly);
            assert_eq!(shown.blocks, ordinary.blocks);
            assert_eq!(
                sync(&desk, &read.secret, document, None, true)
                    .await
                    .problem()
                    .code,
                ErrorCode::AuthzDenied
            );
            let refused = sync(&desk, &viewer.token, document, None, true)
                .await
                .problem();
            assert_eq!(refused.code, ErrorCode::AuthzDenied);
            assert_eq!(
                refused.extra["required"], "doc.manage",
                "the ordinary restoration guard supplies this refusal"
            );
            let refused = sync(&desk, &no_authority.secret, document, None, true)
                .await
                .problem();
            assert_eq!(refused.code, ErrorCode::AuthzDenied);
            assert_eq!(
                refused.extra["required"], "doc.manage",
                "the ordinary restoration guard supplies this refusal"
            );
            assert!(matches!(
                sync(&desk, &manager.secret, document, None, false)
                    .await
                    .problem()
                    .code,
                ErrorCode::AuthzDenied | ErrorCode::NotFound
            ));
        }
    }
    assert_eq!(
        counts().await,
        before,
        "inspection adds no seen position, notice or document version"
    );
}

async fn doc_ws_server(desk: &Desk) -> realtime_support::Server {
    let clock: std::sync::Arc<dyn orch_types::Clock> =
        std::sync::Arc::new(desk.world.app.clock.clone());
    let db = orch_db::Db::from_pool(desk.world.db.pool.clone());
    let mut tuning = realtime_support::quick(&orch_config::ServerConfig::default());
    // Existing document/chat fixtures use publication rather than a repair sweep for these reads.
    tuning.sweep_every = std::time::Duration::from_secs(60);
    realtime_support::Server::over(&db, &clock, tuning).await
}

async fn doc_ws_follow(
    server: &realtime_support::Server,
    token: &str,
    request: i64,
    document: Id,
    since: i64,
) -> realtime_support::Client {
    let mut client = realtime_support::Client::signed_in(server.addr, token).await;
    client
        .sub(request, &format!("doc:{document}"), Some(since))
        .await;
    let ack = client.expect("subscribed").await;
    assert_eq!(ack["mode"], "delta");
    assert_eq!(ack["seq"], since);
    client.expect("presence").await;
    client
}

async fn doc_ws_next(client: &mut realtime_support::Client) -> Value {
    client.until("sync").await.pop().unwrap()
}

fn doc_ws_blocks(frame: &Value) -> Vec<DocumentBlock> {
    serde_json::from_value(frame["data"]["blocks"].clone())
        .expect("a real block write publishes its typed changed blocks")
}

fn doc_ws_reference(
    block: &DocumentBlock,
    source: Id,
) -> (&Option<String>, &Option<Option<String>>) {
    use orch_api_contract::documents::{BlockContent, Inline};
    let BlockContent::Inline(runs) = &block.content else {
        panic!("the reference fixture is an inline paragraph")
    };
    runs.iter()
        .find_map(|run| match run {
            Inline::Ref {
                id, label, name, ..
            } if *id == source => Some((label, name)),
            _ => None,
        })
        .expect("the same stored reference target remains in the reader's block")
}

fn doc_ws_no_sequence_advance(frames: &[Value], before: i64) {
    assert!(
        frames.iter().all(|frame| {
            frame["op"] != "sync" || frame["seq"].as_i64().is_some_and(|seq| seq <= before)
        }),
        "no unauthorized or retained-content publication advances the subscriber"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn document_socket_uses_sync_read_and_keeps_each_keys_current_optional_source_bounds() {
    let desk = Desk::new().await;
    let source = desk.make(&desk.ada, "External document label 682").await;
    let document = desk.make(&desk.ada, "The shared document").await;
    let id = document.base.id;
    let before = get(&desk, &desk.ada.token, id, false).await.ok().base.seq;
    let operation = "DocumentSyncGet.read";
    let primary = rule(operation, "document", id, "read");
    let rich = key(
        &desk.world,
        desk.ada.id(),
        vec![
            primary.clone(),
            rule(operation, "document", source.base.id, "read"),
            rule(operation, "org", desk.org, "read"),
            rule(operation, "project", desk.project, "read"),
            rule(operation, "topic", desk.topic, "read"),
        ],
    )
    .await;
    let narrow = key(&desk.world, desk.ada.id(), vec![primary.clone()]).await;
    let get_only = key(
        &desk.world,
        desk.ada.id(),
        vec![rule("DocumentGet.read", "document", id, "read")],
    )
    .await;
    let server = doc_ws_server(&desk).await;
    let mut denied = realtime_support::Client::signed_in(server.addr, &get_only.secret).await;
    denied.sub(1, &format!("doc:{id}"), Some(before)).await;
    assert_eq!(
        denied.expect("error").await["code"],
        "forbidden",
        "DocumentGet cannot substitute for the existing document feed's Sync operation"
    );
    let mut wide = doc_ws_follow(&server, &rich.secret, 2, id, before).await;
    let mut small = doc_ws_follow(&server, &narrow.secret, 3, id, before).await;

    let stored_label = "Stored label: External document label 682";
    let block = block_ref(InlineRefKind::Document, source.base.id, stored_label);
    let block_id = block.id;
    let first = desk.write(&desk.ada, id, vec![put(block)]).await.ok();
    let wide_frame = doc_ws_next(&mut wide).await;
    let small_frame = doc_ws_next(&mut small).await;
    assert_eq!(wide_frame["seq"], first.seq);
    assert_eq!(small_frame["seq"], first.seq);
    let rich_http = sync(&desk, &rich.secret, id, Some(before), false)
        .await
        .ok();
    let narrow_http = sync(&desk, &narrow.secret, id, Some(before), false)
        .await
        .ok();
    let ordinary = sync(&desk, &desk.ada.token, id, Some(before), false)
        .await
        .ok();
    assert_eq!(doc_ws_blocks(&wide_frame), rich_http.blocks);
    assert_eq!(doc_ws_blocks(&small_frame), narrow_http.blocks);
    assert_eq!(
        rich_http.blocks, ordinary.blocks,
        "sufficient source bounds preserve the ordinary incremental block projection"
    );
    let shown = rich_http
        .blocks
        .iter()
        .find(|block| block.id == block_id)
        .unwrap();
    let hidden = narrow_http
        .blocks
        .iter()
        .find(|block| block.id == block_id)
        .unwrap();
    let (shown_label, shown_name) = doc_ws_reference(shown, source.base.id);
    let (hidden_label, hidden_name) = doc_ws_reference(hidden, source.base.id);
    assert_eq!(shown_label.as_deref(), Some(stored_label));
    assert_eq!(
        hidden_label, shown_label,
        "stored words are never scrubbed because they contain a live name"
    );
    assert_eq!(
        shown_name.as_ref().and_then(|name| name.as_deref()),
        Some(source.base.title.as_str())
    );
    assert_eq!(
        *hidden_name,
        Some(None),
        "the dynamic name is explicitly hidden"
    );
    assert_eq!(hidden.updated_by, shown.updated_by);
    assert!(
        matches!(&hidden.updated_by, Some(orch_api_contract::documents::Author::User(user))
        if user.id == desk.ada.id() && user.username == "ada")
    );

    narrow_key(&desk.world, &rich, vec![primary.clone()]).await;
    let second_block = block_ref(InlineRefKind::Document, source.base.id, stored_label);
    let second_id = second_block.id;
    let second = desk
        .write(&desk.ada, id, vec![put(second_block)])
        .await
        .ok();
    let reset = wide.expect("resync").await;
    assert_eq!(reset["reload"], true);
    assert_eq!(reset["id"], 2);
    let peer = doc_ws_next(&mut small).await;
    assert_eq!(peer["seq"], second.seq);
    let refreshed = sync(&desk, &rich.secret, id, None, false).await.ok();
    assert_eq!(refreshed.document.base.seq, second.seq);
    let block = refreshed
        .blocks
        .iter()
        .find(|block| block.id == second_id)
        .unwrap();
    let (label, name) = doc_ws_reference(block, source.base.id);
    assert_eq!(label.as_deref(), Some(stored_label));
    assert_eq!(*name, Some(None));
    // Replace the whole old view before reopening its incremental feed on this same
    // authenticated connection; its accepted credential ceiling is never discarded.
    wide.send(json!({"op":"unsub","id":2})).await;
    assert_eq!(wide.expect("closed").await["reason"], "unsubscribed");
    wide.sub(2, &format!("doc:{id}"), Some(refreshed.document.base.seq))
        .await;
    let ack = wide.expect("subscribed").await;
    assert_eq!(ack["mode"], "delta");
    assert_eq!(ack["seq"], refreshed.document.base.seq);
    wide.expect("presence").await;

    let next_block = block_ref(InlineRefKind::Document, source.base.id, stored_label);
    let next_id = next_block.id;
    let third = desk.write(&desk.ada, id, vec![put(next_block)]).await.ok();
    let wide_frame = doc_ws_next(&mut wide).await;
    let small_frame = doc_ws_next(&mut small).await;
    assert_eq!(wide_frame["seq"], third.seq);
    let after = sync(&desk, &rich.secret, id, Some(second.seq), false)
        .await
        .ok();
    assert_eq!(
        doc_ws_blocks(&wide_frame),
        after.blocks,
        "current key source bounds are restored after the whole-view reload"
    );
    assert_eq!(
        doc_ws_blocks(&small_frame),
        after.blocks,
        "the two credentials now project the same narrow view"
    );
    let block = after
        .blocks
        .iter()
        .find(|block| block.id == next_id)
        .unwrap();
    let (label, name) = doc_ws_reference(block, source.base.id);
    assert_eq!(label.as_deref(), Some(stored_label));
    assert_eq!(*name, Some(None));

    narrow_key(
        &desk.world,
        &rich,
        vec![rule("DocumentGet.read", "document", id, "read")],
    )
    .await;
    let last = desk
        .write(
            &desk.ada,
            id,
            vec![put(paragraph(
                "Only the still-admitted Sync key may receive this",
            ))],
        )
        .await
        .ok();
    let closed = wide.until("closed").await;
    assert_eq!(closed.last().unwrap()["reason"], "revoked");
    doc_ws_no_sequence_advance(&closed, third.seq);
    let peer = doc_ws_next(&mut small).await;
    assert_eq!(peer["seq"], last.seq);
    assert!(!doc_ws_blocks(&peer).is_empty());
    drop((denied, wide, small));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_current_document_sync_key_gets_deleted_without_a_retained_content_publication() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Document whose live stream ends")
        .await;
    let id = document.base.id;
    let written = desk
        .write(
            &desk.ada,
            id,
            vec![put(paragraph("Retained-only document body 427"))],
        )
        .await
        .ok();
    let current = key(
        &desk.world,
        desk.ada.id(),
        vec![rule("DocumentSyncGet.read", "document", id, "read")],
    )
    .await;
    let server = doc_ws_server(&desk).await;
    let mut reader = doc_ws_follow(&server, &current.secret, 4, id, written.seq).await;
    desk.world
        .app
        .call(DocumentDelete, &[&id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let closed = reader.until("closed").await;
    assert_eq!(
        closed.last().unwrap()["reason"],
        "deleted",
        "the unchanged credential is authorized for its identity; its live source was deleted"
    );
    doc_ws_no_sequence_advance(&closed, written.seq);
    assert!(
        closed.iter().all(|frame| !frame
            .to_string()
            .contains("Retained-only document body 427")),
        "a live read never upgrades into retained-content inspection"
    );
    drop(reader);
    server.stop().await;
}

#[tokio::test]
async fn retained_document_placement_does_not_require_restoring_its_deleted_parent() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Document below deleted project")
        .await
        .base
        .id;
    desk.paragraphs(&desk.ada, document, &["Retained descendant content"])
        .await;
    let manager = key(
        &desk.world,
        desk.ada.id(),
        vec![
            rule(
                "DocumentGet.inspect_deleted",
                "document",
                document,
                "manage",
            ),
            rule(
                "DocumentSyncGet.inspect_deleted",
                "document",
                document,
                "manage",
            ),
        ],
    )
    .await;
    let live_reader = key(
        &desk.world,
        desk.ada.id(),
        vec![
            rule("DocumentGet.read", "document", document, "read"),
            rule("DocumentSyncGet.read", "document", document, "read"),
        ],
    )
    .await;
    desk.world
        .app
        .call(orch_api_contract::ownership::NodeDelete, &[&desk.project])
        .token(&desk.ada.token)
        .raw_body(&json!({}))
        .send()
        .await
        .ok();
    let state = || async {
        sqlx::query_as::<_, (Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>)>(
            "SELECT (SELECT deleted_at FROM nodes WHERE id=$1), deleted_at FROM documents WHERE id=$2",
        )
        .bind(desk.project)
        .bind(document)
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap()
    };
    let before = state().await;
    assert!(before.0.is_some());
    let ordinary = get(&desk, &desk.ada.token, document, true).await.ok();
    let shown = get(&desk, &manager.secret, document, true).await.ok();
    assert_eq!(shown.base.id, ordinary.base.id);
    assert_eq!(shown.base.title, ordinary.base.title);
    assert!(shown.inspection.unwrap().readonly);
    let ordinary = sync(&desk, &desk.ada.token, document, None, true)
        .await
        .ok();
    let shown = sync(&desk, &manager.secret, document, None, true)
        .await
        .ok();
    assert_eq!(shown.blocks, ordinary.blocks);
    assert!(shown.document.inspection.unwrap().readonly);
    for reply in [
        get(&desk, &live_reader.secret, document, true).await.status,
        get(&desk, &manager.secret, document, false).await.status,
    ] {
        assert_ne!(reply, 200, "live reads never become retained inspection");
    }
    assert_eq!(
        state().await,
        before,
        "inspection does not restore either object"
    );
}
