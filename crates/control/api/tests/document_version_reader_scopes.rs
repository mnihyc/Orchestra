//! Version readers keep historical content and current protected names under their own scopes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod desk;
mod places;
mod support;

use crate::support::FullPermissionPageReply;
use desk::{Desk, paragraph, put, rewrite, text};
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{
    BlockContent, DocumentChangesGet, DocumentChangesGetQuery, DocumentDelete, DocumentVersionGet,
    DocumentVersionList, DocumentVersionListQuery, Inline, InlineRefKind,
};
use orch_api_contract::nodes::DeletedInspectionQuery;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;

// The deployed version window; advancing its clock exercises the ordinary version cutter.
const WINDOW: i64 = 600;

#[derive(Clone, Copy)]
enum Reader {
    List,
    Version,
    Changes,
}
const READERS: [Reader; 3] = [Reader::List, Reader::Version, Reader::Changes];
impl Reader {
    fn operation(self, inspect: bool) -> String {
        format!(
            "{}.{}",
            match self {
                Self::List => "DocumentVersionList",
                Self::Version => "DocumentVersionGet",
                Self::Changes => "DocumentChangesGet",
            },
            if inspect { "inspect_deleted" } else { "read" }
        )
    }
    async fn read(self, scene: &Scene, token: &str, inspect: bool) -> RawReply {
        match self {
            Self::List => {
                list(
                    scene,
                    token,
                    &DocumentVersionListQuery {
                        inspect_deleted: inspect.then_some(true),
                        ..Default::default()
                    },
                )
                .await
            }
            Self::Version => version(scene, token, "1", inspect).await,
            Self::Changes => changes(scene, token, "0", Some(1), inspect).await,
        }
    }
}
struct Scene {
    desk: Desk,
    document: Id,
    block: Id,
}
impl Scene {
    async fn new() -> Self {
        Self::with_content(Desk::new().await, text("Stored first version")).await
    }
    async fn with_content(desk: Desk, content: Vec<Inline>) -> Self {
        let document = desk.make(&desk.ada, "Stored version title").await.base.id;
        let mut block = paragraph("");
        block.content = Some(BlockContent::Inline(content));
        let inserted = desk
            .write(&desk.ada, document, vec![put(block)])
            .await
            .ok()
            .blocks
            .remove(0);
        desk.world.advance(chrono::Duration::seconds(WINDOW + 1));
        desk.write(
            &desk.ada,
            document,
            vec![rewrite(&inserted, "The current live state")],
        )
        .await
        .ok();
        Self {
            desk,
            document,
            block: inserted.id,
        }
    }
}
struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":[action]}]})
}
async fn key(desk: &Desk, user: Id, rules: Option<Vec<Json>>) -> Key {
    let policy = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        serde_json::to_value(scope).unwrap()
    });
    let scopes = if policy.is_none() {
        vec!["*".to_owned()]
    } else {
        Vec::new()
    };
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Document version reader control",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        policy.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn list(scene: &Scene, token: &str, query: &DocumentVersionListQuery) -> RawReply {
    let request = scene
        .desk
        .world
        .app
        .call(DocumentVersionList, &[&scene.document])
        .token(token)
        .query(query);
    request.send_bytes().await
}
async fn version(scene: &Scene, token: &str, number: &str, inspect: bool) -> RawReply {
    let request = scene
        .desk
        .world
        .app
        .call(DocumentVersionGet, &[&scene.document, &number])
        .token(token)
        .query(&DeletedInspectionQuery {
            inspect_deleted: inspect.then_some(true),
        });
    request.send_bytes().await
}
async fn changes(
    scene: &Scene,
    token: &str,
    from: &str,
    to: Option<i64>,
    inspect: bool,
) -> RawReply {
    let request = scene
        .desk
        .world
        .app
        .call(DocumentChangesGet, &[&scene.document])
        .token(token)
        .query(&DocumentChangesGetQuery {
            inspect_deleted: inspect.then_some(true),
            from: from.into(),
            to,
        });
    request.send_bytes().await
}
fn shown(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn denied(reply: RawReply) {
    assert_eq!(
        reply.status,
        StatusCode::FORBIDDEN,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    assert_eq!(reply.problem().code, ErrorCode::AuthzDenied);
}
fn reference(kind: InlineRefKind, id: Id, label: &str) -> Inline {
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
fn find_ref(page: &Json, id: Id) -> &Json {
    page["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|block| {
            block.get("after").unwrap_or(block)["content"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .find(|run| run["type"] == "ref" && run["id"] == json!(id))
        .unwrap()
}
fn assert_ref(page: &Json, id: Id, label: &str, name: Option<&str>) {
    let run = find_ref(page, id);
    assert_eq!(run["label"], json!(label));
    assert_eq!(run["name"], json!(name));
}

#[tokio::test]
async fn both_version_readers_use_their_own_document_selection_without_granting_ordinary_view() {
    let s = Scene::new().await;
    let other = s.desk.make(&s.desk.ada, "Other document").await.base.id;
    let stranger = s.desk.stranger("version_outsider").await;
    let legacy = key(&s.desk, s.desk.ada.id(), None).await;
    for reader in READERS {
        let ordinary = shown(reader.read(&s, &s.desk.ada.token, false).await);
        assert_eq!(
            shown(reader.read(&s, &legacy.secret, false).await),
            ordinary
        );
        let operation = reader.operation(false);
        let selected = key(
            &s.desk,
            s.desk.ada.id(),
            Some(vec![rule(&operation, "document", s.document, "read")]),
        )
        .await;
        assert_eq!(
            shown(reader.read(&s, &selected.secret, false).await),
            ordinary
        );
        assert!(
            ordinary.to_string().contains("ada"),
            "public authors need no AccountRead"
        );
        for rules in [
            Vec::new(),
            vec![rule("DocumentGet.read", "document", s.document, "read")],
            vec![rule(&operation, "document", other, "read")],
            vec![rule(&operation, "document", s.document, "write")],
        ] {
            let wrong = key(&s.desk, s.desk.ada.id(), Some(rules)).await;
            denied(reader.read(&s, &wrong.secret, false).await);
        }
        let no_view = key(
            &s.desk,
            stranger.id(),
            Some(vec![rule(&operation, "document", s.document, "read")]),
        )
        .await;
        denied(reader.read(&s, &stranger.token, false).await);
        denied(reader.read(&s, &no_view.secret, false).await);
    }
}

#[tokio::test]
async fn version_pages_keep_open_span_author_filters_cursors_and_real_empty_results() {
    let s = Scene::new().await;
    let ines = s.desk.member("version_editor", "interactor").await;
    for (by, words) in [
        (&ines, "Second closed version"),
        (&s.desk.ada, "Third closed version"),
    ] {
        let block = s.desk.page(by, s.document).await.remove(0);
        s.desk.world.advance(chrono::Duration::seconds(WINDOW + 1));
        s.desk
            .write(by, s.document, vec![rewrite(&block, words)])
            .await
            .ok();
    }
    let token = key(
        &s.desk,
        s.desk.ada.id(),
        Some(vec![rule(
            "DocumentVersionList.read",
            "document",
            s.document,
            "read",
        )]),
    )
    .await;
    let first = DocumentVersionListQuery {
        limit: Some(2),
        ..Default::default()
    };
    let ordinary = shown(list(&s, &s.desk.ada.token, &first).await);
    assert_eq!(
        ordinary["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["version"].clone())
            .collect::<Vec<_>>(),
        vec![Json::Null, json!(3)]
    );
    assert_eq!(shown(list(&s, &token.secret, &first).await), ordinary);
    let second = DocumentVersionListQuery {
        limit: Some(2),
        cursor: ordinary["next_cursor"].as_str().map(str::to_owned),
        ..Default::default()
    };
    let ordinary = shown(list(&s, &s.desk.ada.token, &second).await);
    assert_eq!(
        ordinary["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["version"].clone())
            .collect::<Vec<_>>(),
        vec![json!(2), json!(1)]
    );
    assert_eq!(shown(list(&s, &token.secret, &second).await), ordinary);
    for author in [ines.id(), s.desk.ada.id(), new_id()] {
        let query = DocumentVersionListQuery {
            author: Some(author.to_string()),
            ..Default::default()
        };
        let ordinary = shown(list(&s, &s.desk.ada.token, &query).await);
        if author != ines.id() && author != s.desk.ada.id() {
            assert_eq!(ordinary["items"], json!([]));
        }
        assert_eq!(shown(list(&s, &token.secret, &query).await), ordinary);
    }
    for query in [
        DocumentVersionListQuery {
            cursor: Some("invalid cursor".into()),
            ..Default::default()
        },
        DocumentVersionListQuery {
            author: Some("invalid author".into()),
            ..Default::default()
        },
    ] {
        assert_eq!(
            list(&s, &s.desk.ada.token, &query).await.problem().code,
            ErrorCode::RequestInvalid
        );
        assert_eq!(
            list(&s, &token.secret, &query).await.problem().code,
            ErrorCode::RequestInvalid
        );
    }
}

#[tokio::test]
async fn a_selected_version_is_its_historical_snapshot_and_keeps_missing_version_boundaries() {
    let s = Scene::new().await;
    let ordinary = shown(version(&s, &s.desk.ada.token, "1", false).await);
    assert_eq!(ordinary["version"]["version"], 1);
    assert_eq!(ordinary["blocks"][0]["id"], json!(s.block));
    assert_eq!(
        ordinary["blocks"][0]["content"][0]["text"],
        "Stored first version"
    );
    assert_eq!(
        desk::said(&s.desk.page(&s.desk.ada, s.document).await[0]),
        "The current live state"
    );
    let token = key(
        &s.desk,
        s.desk.ada.id(),
        Some(vec![rule(
            "DocumentVersionGet.read",
            "document",
            s.document,
            "read",
        )]),
    )
    .await;
    assert_eq!(
        shown(version(&s, &token.secret, "1", false).await),
        ordinary
    );
    for number in ["not-a-version", "0", "99999"] {
        assert_eq!(
            version(&s, &s.desk.ada.token, number, false)
                .await
                .problem()
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            version(&s, &token.secret, number, false)
                .await
                .problem()
                .code,
            ErrorCode::NotFound
        );
    }
}

#[tokio::test]
async fn historical_reference_content_survives_while_live_names_need_this_version_operation() {
    let desk = Desk::new().await;
    let foreign = desk
        .make(&desk.ada, "Current foreign version title")
        .await
        .base
        .id;
    let topic = desk.topic;
    let s = Scene::with_content(
        desk,
        vec![
            reference(
                InlineRefKind::Document,
                foreign,
                "Stored foreign title copy",
            ),
            reference(InlineRefKind::Node, topic, "Stored topic copy"),
        ],
    )
    .await;
    let ordinary = shown(version(&s, &s.desk.ada.token, "1", false).await);
    assert_ref(
        &ordinary,
        foreign,
        "Stored foreign title copy",
        Some("Current foreign version title"),
    );
    assert_ref(&ordinary, topic, "Stored topic copy", Some("Plans"));
    let stored = orch_db::queries::documents::history::snapshot(
        &mut *s.desk.world.conn().await,
        s.document,
        1,
    )
    .await
    .unwrap()
    .unwrap();
    let before = stored
        .iter()
        .map(|b| (b.id, b.content.clone()))
        .collect::<Vec<_>>();
    for extra in [
        Vec::new(),
        vec![rule("DocumentGet.read", "document", foreign, "read")],
        vec![rule("DocumentSyncGet.read", "topic", topic, "read")],
    ] {
        let mut rules = vec![rule(
            "DocumentVersionGet.read",
            "document",
            s.document,
            "read",
        )];
        rules.extend(extra);
        let token = key(&s.desk, s.desk.ada.id(), Some(rules)).await;
        let page = shown(version(&s, &token.secret, "1", false).await);
        assert_ref(&page, foreign, "Stored foreign title copy", None);
        assert_ref(&page, topic, "Stored topic copy", None);
        assert_eq!(page["blocks"][0]["updated_by"]["username"], "ada");
    }
    let token = key(
        &s.desk,
        s.desk.ada.id(),
        Some(vec![
            rule("DocumentVersionGet.read", "document", s.document, "read"),
            rule("DocumentVersionGet.read", "document", foreign, "read"),
            rule("DocumentVersionGet.read", "topic", topic, "read"),
        ]),
    )
    .await;
    assert_eq!(
        shown(version(&s, &token.secret, "1", false).await),
        ordinary
    );
    let after = orch_db::queries::documents::history::snapshot(
        &mut *s.desk.world.conn().await,
        s.document,
        1,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        after
            .iter()
            .map(|b| (b.id, b.content.clone()))
            .collect::<Vec<_>>(),
        before
    );
}

fn authors(reader: Reader, page: &Json) -> Vec<&Json> {
    match reader {
        Reader::List => page["items"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|row| row["authors"].as_array().unwrap())
            .collect(),
        Reader::Version => page["version"]["authors"]
            .as_array()
            .unwrap()
            .iter()
            .collect(),
        Reader::Changes => page["authors"].as_array().unwrap().iter().collect(),
    }
}

#[tokio::test]
async fn stored_agent_authorship_keeps_identity_and_name_while_its_live_thread_ref_is_scoped() {
    use orch_db::queries::documents::batch::{self, Limits, Outcome, Rules};
    use orch_documents::versions::Writer;
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "History written by a stored agent")
        .await
        .base
        .id;
    let thread = new_id();
    let mut tx = desk.world.db.pool.begin().await.unwrap();
    orch_db::queries::hierarchy::testing::insert_node(
        &mut tx,
        thread,
        "thread",
        Some(desk.topic),
        "Live agent writer thread",
        desk.ada.id(),
    )
    .await
    .unwrap();
    let block = paragraph("Historical agent-authored words");
    let block_id = block.id;
    // Actual batch writer/serializer. These are the shipped document/history defaults;
    // this constructs stored authorship, without claiming a native harness authorization test.
    let applied = batch::apply(
        &mut tx,
        document,
        &Writer::Agent {
            thread,
            agent: "Stored agent program".into(),
        },
        None,
        &[put(block)],
        Limits {
            max_bytes: 2_097_152,
            max_blocks: 5_000,
        },
        Rules {
            window_secs: WINDOW,
            kept: 200,
            history_max_bytes: 16_777_216,
        },
        desk.world.now(),
    )
    .await
    .unwrap();
    assert!(matches!(applied, Outcome::Applied(_)));
    tx.commit().await.unwrap();
    desk.world.advance(chrono::Duration::seconds(WINDOW + 1));
    desk.write(&desk.ada, document, vec![put(paragraph("Human open span"))])
        .await
        .ok();
    let s = Scene {
        desk,
        document,
        block: block_id,
    };
    for reader in READERS {
        let ordinary = shown(reader.read(&s, &s.desk.ada.token, false).await);
        let author = authors(reader, &ordinary)
            .into_iter()
            .find(|a| a["agent"] == "Stored agent program")
            .unwrap();
        assert_eq!(author["thread"]["id"], json!(thread));
        assert_eq!(author["thread"]["name"], "Live agent writer thread");
        for extra in [
            Vec::new(),
            vec![rule("DocumentSyncGet.read", "thread", thread, "read")],
        ] {
            let mut rules = vec![rule(&reader.operation(false), "document", document, "read")];
            rules.extend(extra);
            let token = key(&s.desk, s.desk.ada.id(), Some(rules)).await;
            let page = shown(reader.read(&s, &token.secret, false).await);
            let author = authors(reader, &page)
                .into_iter()
                .find(|a| a["agent"] == "Stored agent program")
                .unwrap();
            assert_eq!(author["thread"]["id"], json!(thread));
            assert_eq!(author["thread"]["name"], Json::Null);
            assert_eq!(author["thread"]["redacted"], true);
            if matches!(reader, Reader::Version) {
                assert_eq!(
                    page["blocks"][0]["updated_by"]["agent"],
                    "Stored agent program"
                );
                assert_eq!(
                    page["blocks"][0]["updated_by"]["thread"]["name"],
                    Json::Null
                );
            }
        }
        let full = key(
            &s.desk,
            s.desk.ada.id(),
            Some(vec![
                rule(&reader.operation(false), "document", document, "read"),
                rule(&reader.operation(false), "thread", thread, "read"),
            ]),
        )
        .await;
        assert_eq!(shown(reader.read(&s, &full.secret, false).await), ordinary);
        if matches!(reader, Reader::List) {
            let query = DocumentVersionListQuery {
                author: Some(format!("thread:{thread}")),
                ..Default::default()
            };
            let ordinary = shown(list(&s, &s.desk.ada.token, &query).await);
            assert_eq!(ordinary["items"].as_array().unwrap().len(), 1);
            assert_eq!(ordinary["items"][0]["version"], 1);
            assert_eq!(shown(list(&s, &full.secret, &query).await), ordinary);
        }
    }
}

#[tokio::test]
async fn a_historical_attached_file_label_uses_its_actual_document_owner() {
    use orch_api_contract::documents::{DocumentFileCreate, DocumentFileCreateQuery};
    use orch_api_contract::threads::FILENAME_HEADER;
    let desk = Desk::new().await;
    let owner = desk
        .make(&desk.ada, "Actual historical file owner")
        .await
        .base
        .id;
    let file = {
        let request = desk
            .world
            .app
            .call(DocumentFileCreate, &[&owner])
            .token(&desk.ada.token)
            .query(&DocumentFileCreateQuery::default())
            .header(FILENAME_HEADER, "historical-owner-file.txt")
            .bytes(b"Actual historical attached bytes");
        request.send().await
    }
    .ok();
    let s = Scene::with_content(
        desk,
        vec![reference(
            InlineRefKind::File,
            file.id,
            "Stored attached file label",
        )],
    )
    .await;
    let ordinary = shown(version(&s, &s.desk.ada.token, "1", false).await);
    assert_ref(
        &ordinary,
        file.id,
        "Stored attached file label",
        Some("historical-owner-file.txt"),
    );
    for extra in [
        Vec::new(),
        vec![rule("DocumentGet.read", "document", owner, "read")],
        vec![rule(
            "DocumentVersionGet.read",
            "topic",
            s.desk.topic,
            "read",
        )],
    ] {
        let mut rules = vec![rule(
            "DocumentVersionGet.read",
            "document",
            s.document,
            "read",
        )];
        rules.extend(extra);
        let token = key(&s.desk, s.desk.ada.id(), Some(rules)).await;
        assert_ref(
            &shown(version(&s, &token.secret, "1", false).await),
            file.id,
            "Stored attached file label",
            None,
        );
    }
    let token = key(
        &s.desk,
        s.desk.ada.id(),
        Some(vec![
            rule("DocumentVersionGet.read", "document", s.document, "read"),
            rule("DocumentVersionGet.read", "document", owner, "read"),
        ]),
    )
    .await;
    assert_eq!(
        shown(version(&s, &token.secret, "1", false).await),
        ordinary
    );
}

#[tokio::test]
async fn historical_carried_files_and_node_chat_titles_keep_distinct_governing_sources() {
    use orch_api_contract::chats::{
        ChatAttachmentCreate, ChatBlobGet, ChatBlobGetQuery, ChatMessageCreate,
        ChatMessageCreateBody, ChatPart,
    };
    use orch_api_contract::threads::FILENAME_HEADER;
    let desk = Desk::new().await;
    let topic = desk.another_topic("Live historical chat place").await;
    let orch_api_contract::nodes::Node::Topic(node) =
        places::read(&desk.world, &desk.ada, topic).await
    else {
        panic!("topic")
    };
    let chat = node.base.chat_id;
    let upload = {
        let request = desk
            .world
            .app
            .call(ChatAttachmentCreate, &[&chat])
            .token(&desk.ada.token)
            .header(FILENAME_HEADER, "historical-carried-file.txt")
            .bytes(b"Actual carried bytes");
        request.send().await
    }
    .ok();
    let message = {
        let request = desk
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
            });
        request.send().await
    }
    .ok()
    .into_full()
    .unwrap()
    .message;
    let preview = {
        let request = desk
            .world
            .app
            .call(ChatBlobGet, &[&chat, &upload.blob])
            .token(&desk.ada.token)
            .query(&ChatBlobGetQuery {
                preview: Some(true),
                download: None,
            });
        request.send_bytes().await
    };
    assert_eq!(preview.status, StatusCode::OK);
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
    let s = Scene::with_content(
        desk,
        vec![
            reference(InlineRefKind::File, file, "Stored carried label"),
            reference(InlineRefKind::Chat, chat, "Stored chat label"),
        ],
    )
    .await;
    let ordinary = shown(version(&s, &s.desk.ada.token, "1", false).await);
    assert_ref(
        &ordinary,
        file,
        "Stored carried label",
        Some("historical-carried-file.txt"),
    );
    assert_ref(
        &ordinary,
        chat,
        "Stored chat label",
        Some("Live historical chat place"),
    );
    let operation = "DocumentVersionGet.read";
    for (extra, file_name, chat_name) in [
        (Vec::new(), None, None),
        (vec![rule(operation, "topic", topic, "read")], None, None),
        (
            vec![rule(operation, "chat", chat, "read")],
            Some("historical-carried-file.txt"),
            None,
        ),
        (
            vec![
                rule(operation, "chat", chat, "read"),
                rule(operation, "topic", topic, "read"),
            ],
            Some("historical-carried-file.txt"),
            Some("Live historical chat place"),
        ),
    ] {
        let mut rules = vec![rule(operation, "document", s.document, "read")];
        rules.extend(extra);
        let token = key(&s.desk, s.desk.ada.id(), Some(rules)).await;
        let page = shown(version(&s, &token.secret, "1", false).await);
        assert_ref(&page, file, "Stored carried label", file_name);
        assert_ref(&page, chat, "Stored chat label", chat_name);
        if chat_name.is_some() {
            assert_eq!(page, ordinary);
        }
    }
}

async fn inspection_case(reader: Reader) {
    let s = Scene::new().await;
    let viewer = s.desk.member("version_retained_viewer", "viewer").await;
    {
        let request = s
            .desk
            .world
            .app
            .call(DocumentDelete, &[&s.document])
            .token(&s.desk.ada.token);
        request.send().await
    }
    .ok();
    let ordinary = shown(reader.read(&s, &s.desk.ada.token, true).await);
    let read = key(
        &s.desk,
        s.desk.ada.id(),
        Some(vec![rule(
            &reader.operation(false),
            "document",
            s.document,
            "read",
        )]),
    )
    .await;
    let manager = key(
        &s.desk,
        s.desk.ada.id(),
        Some(vec![rule(
            &reader.operation(true),
            "document",
            s.document,
            "manage",
        )]),
    )
    .await;
    let no_authority = key(
        &s.desk,
        viewer.id(),
        Some(vec![rule(
            &reader.operation(true),
            "document",
            s.document,
            "manage",
        )]),
    )
    .await;
    assert_eq!(
        reader
            .read(&s, &s.desk.ada.token, false)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        shown(reader.read(&s, &manager.secret, true).await),
        ordinary
    );
    denied(reader.read(&s, &read.secret, true).await);
    denied(reader.read(&s, &no_authority.secret, true).await);
    denied(reader.read(&s, &viewer.token, true).await);
    for token in [&read.secret, &manager.secret] {
        assert!(matches!(
            reader.read(&s, token, false).await.problem().code,
            ErrorCode::AuthzDenied | ErrorCode::NotFound
        ));
    }
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT deleted_at IS NOT NULL FROM documents WHERE id=$1")
            .bind(s.document)
            .fetch_one(&s.desk.world.db.pool)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn version_list_inspection_requires_its_manage_selection_and_current_restore_authority() {
    inspection_case(Reader::List).await;
}

#[tokio::test]
async fn version_snapshot_inspection_requires_its_manage_selection_and_current_restore_authority() {
    inspection_case(Reader::Version).await;
}

#[tokio::test]
async fn version_inspection_keeps_retained_parent_identity_without_restoring_the_document_tree() {
    let s = Scene::new().await;
    let token = key(
        &s.desk,
        s.desk.ada.id(),
        Some(
            READERS
                .into_iter()
                .map(|reader| rule(&reader.operation(true), "document", s.document, "manage"))
                .collect(),
        ),
    )
    .await;
    {
        let request = s
            .desk
            .world
            .app
            .call(orch_api_contract::ownership::NodeDelete, &[&s.desk.project])
            .token(&s.desk.ada.token)
            .raw_body(&json!({}));
        request.send().await
    }
    .ok();
    let state = || async {
        sqlx::query_as::<_,(Option<chrono::DateTime<chrono::Utc>>,Option<chrono::DateTime<chrono::Utc>>)>(
        "SELECT (SELECT deleted_at FROM nodes WHERE id=$1),deleted_at FROM documents WHERE id=$2")
        .bind(s.desk.project).bind(s.document).fetch_one(&s.desk.world.db.pool).await.unwrap()
    };
    let before = state().await;
    assert!(before.0.is_some());
    for reader in READERS {
        let ordinary = shown(reader.read(&s, &s.desk.ada.token, true).await);
        assert_eq!(shown(reader.read(&s, &token.secret, true).await), ordinary);
        assert!(matches!(
            reader.read(&s, &token.secret, false).await.problem().code,
            ErrorCode::AuthzDenied | ErrorCode::NotFound
        ));
    }
    assert_eq!(state().await, before);
}

#[tokio::test]
async fn both_history_readers_use_current_key_policy_revocation_and_ordinary_document_view() {
    let s = Scene::new().await;
    for reader in READERS {
        shown(reader.read(&s, &s.desk.ada.token, false).await);
        for revoke in [false, true] {
            let token = key(
                &s.desk,
                s.desk.ada.id(),
                Some(vec![rule(
                    &reader.operation(false),
                    "document",
                    s.document,
                    "read",
                )]),
            )
            .await;
            shown(reader.read(&s, &token.secret, false).await);
            if revoke {
                identity::revoke_api_token(
                    &mut *s.desk.world.conn().await,
                    token.id,
                    s.desk.ada.id(),
                    s.desk.world.now(),
                )
                .await
                .unwrap();
            } else {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(token.id)
                    .bind(serde_json::to_value(KeyScope::default()).unwrap())
                    .execute(&s.desk.world.db.pool)
                    .await
                    .unwrap();
            }
            let after = reader.read(&s, &token.secret, false).await;
            assert_eq!(
                after.status,
                if revoke {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            assert!(!String::from_utf8_lossy(&after.raw).contains("Stored first version"));
        }
    }
    let viewer = s.desk.member("version_scope_viewer", "viewer").await;
    let mut tokens = Vec::new();
    for reader in READERS {
        let token = key(
            &s.desk,
            viewer.id(),
            Some(vec![rule(
                &reader.operation(false),
                "document",
                s.document,
                "read",
            )]),
        )
        .await;
        shown(reader.read(&s, &viewer.token, false).await);
        shown(reader.read(&s, &token.secret, false).await);
        tokens.push((reader, token));
    }
    {
        let request = s
            .desk
            .world
            .app
            .call(
                orch_api_contract::access::PermissionPageUpdate,
                &[&"documents", &s.document],
            )
            .token(&s.desk.ada.token)
            .raw_body(&json!({"whole":"private"}));
        request.send().await
    }
    .full_permission_page()
    .ok();
    for (reader, token) in tokens {
        denied(reader.read(&s, &viewer.token, false).await);
        denied(reader.read(&s, &token.secret, false).await);
    }
}

#[tokio::test]
async fn changes_reader_keeps_before_after_selection_and_its_own_live_reference_sources() {
    let desk = Desk::new().await;
    let foreign = desk
        .make(&desk.ada, "Current changes reference title")
        .await
        .base
        .id;
    let s = Scene::with_content(
        desk,
        vec![reference(
            InlineRefKind::Document,
            foreign,
            "Stored comparison label",
        )],
    )
    .await;
    let ordinary = shown(changes(&s, &s.desk.ada.token, "0", Some(1), false).await);
    assert_eq!(ordinary["from"], 0);
    assert_eq!(ordinary["to"], 1);
    assert!(ordinary["blocks"][0]["before"].is_null());
    assert_ref(
        &ordinary,
        foreign,
        "Stored comparison label",
        Some("Current changes reference title"),
    );
    for extra in [
        Vec::new(),
        vec![rule("DocumentVersionGet.read", "document", foreign, "read")],
    ] {
        let mut rules = vec![rule(
            "DocumentChangesGet.read",
            "document",
            s.document,
            "read",
        )];
        rules.extend(extra);
        let token = key(&s.desk, s.desk.ada.id(), Some(rules)).await;
        let reply = shown(changes(&s, &token.secret, "0", Some(1), false).await);
        assert_ref(&reply, foreign, "Stored comparison label", None);
    }
    let full = key(
        &s.desk,
        s.desk.ada.id(),
        Some(vec![
            rule("DocumentChangesGet.read", "document", s.document, "read"),
            rule("DocumentChangesGet.read", "document", foreign, "read"),
        ]),
    )
    .await;
    assert_eq!(
        shown(changes(&s, &full.secret, "0", Some(1), false).await),
        ordinary
    );
    let no_change = shown(changes(&s, &full.secret, "1", Some(1), false).await);
    assert_eq!(no_change["blocks"], json!([]));
    for (from, to) in [("not-a-version", None), ("0", Some(-1))] {
        assert_eq!(
            changes(&s, &full.secret, from, to, false)
                .await
                .problem()
                .code,
            ErrorCode::RequestInvalid
        );
    }
}

#[tokio::test]
async fn changes_reader_inspection_uses_its_current_manage_variant_and_restore_authority() {
    inspection_case(Reader::Changes).await;
}

#[tokio::test]
async fn changes_json_publication_rechecks_key_expiry_after_activity_bookkeeping_wait() {
    // Use the established action fixture watchdog; the observed SQL lock is the premise.
    let wait = std::time::Duration::from_secs(30);
    for expire in [false, true] {
        let s = Scene::new().await;
        let token = key(
            &s.desk,
            s.desk.ada.id(),
            Some(vec![rule(
                "DocumentChangesGet.read",
                "document",
                s.document,
                "read",
            )]),
        )
        .await;
        let mut blocker = s.desk.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.desk.world.app.clone();
        let document = s.document;
        let secret = token.secret.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(DocumentChangesGet, &[&document])
                .token(&secret)
                .query(&DocumentChangesGetQuery {
                    inspect_deleted: None,
                    from: "0".into(),
                    to: Some(1),
                });
            request.send_bytes().await
        });
        tokio::time::timeout(wait, async {
            loop {
                assert!(
                    !request.is_finished(),
                    "the request did not reach activity bookkeeping"
                );
                let waited: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                     AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid))
                     AND query LIKE '%INSERT INTO api_token_activity%')",
                )
                .bind(pid)
                .fetch_one(&s.desk.world.db.pool)
                .await
                .unwrap();
                if waited {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the completed native JSON projection did not wait at its actual activity append");
        if expire {
            s.desk.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
        if expire {
            assert_eq!(
                reply.status,
                StatusCode::UNAUTHORIZED,
                "key expiry while the actual response is held must prevent publication"
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("Stored first version"));
        } else {
            assert!(shown(reply).to_string().contains("Stored first version"));
        }
    }
}

#[tokio::test]
async fn changes_json_publication_rechecks_native_authority_after_activity_bookkeeping_wait() {
    use orch_api_contract::access::PermissionPageUpdate;
    let desk = Desk::new().await;
    let foreign = desk
        .make(&desk.ada, "Foreign title held through real bookkeeping")
        .await
        .base
        .id;
    let s = Scene::with_content(
        desk,
        vec![reference(
            InlineRefKind::Document,
            foreign,
            "Stored comparison wording",
        )],
    )
    .await;
    let viewer = s.desk.member("heldchangesviewer", "viewer").await;
    let token = key(
        &s.desk,
        viewer.id(),
        Some(vec![
            rule("DocumentChangesGet.read", "document", s.document, "read"),
            rule("DocumentChangesGet.read", "document", foreign, "read"),
        ]),
    )
    .await;
    let control = shown(changes(&s, &token.secret, "0", Some(1), false).await);
    assert_ref(
        &control,
        foreign,
        "Stored comparison wording",
        Some("Foreign title held through real bookkeeping"),
    );
    let mut blocker = s.desk.world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = s.desk.world.app.clone();
    let document = s.document;
    let secret = token.secret.clone();
    let request = tokio::spawn(async move {
        let request = app
            .call(DocumentChangesGet, &[&document])
            .token(&secret)
            .query(&DocumentChangesGetQuery {
                inspect_deleted: None,
                from: "0".into(),
                to: Some(1),
            });
        request.send_bytes().await
    });
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            assert!(
                !request.is_finished(),
                "the native held response did not reach bookkeeping"
            );
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                 AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid))
                 AND query LIKE '%INSERT INTO api_token_activity%')",
            )
            .bind(pid)
            .fetch_one(&s.desk.world.db.pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the actual native JSON response must wait at its activity append");
    {
        let request = s
            .desk
            .world
            .app
            .call(PermissionPageUpdate, &[&"documents", &foreign])
            .token(&s.desk.ada.token)
            .raw_body(&json!({"whole":"private"}));
        request.send().await
    }
    .full_permission_page()
    .ok();
    let refused = {
        let request = s
            .desk
            .world
            .app
            .call(orch_api_contract::documents::DocumentGet, &[&foreign])
            .token(&viewer.token);
        request.send_bytes().await
    };
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "the real ordinary foreign source Read must be gone"
    );
    blocker.commit().await.unwrap();
    let held = tokio::time::timeout(wait, request).await.unwrap().unwrap();
    assert_eq!(
        held.status,
        StatusCode::CONFLICT,
        "the original native metadata authority cut changed"
    );
    assert_eq!(held.problem().code, ErrorCode::ConflictStaleRevision);
    assert!(
        !String::from_utf8_lossy(&held.raw).contains("Foreign title held through real bookkeeping")
    );
    assert!(!String::from_utf8_lossy(&held.raw).contains("Stored comparison wording"));
    let current = shown(changes(&s, &token.secret, "0", Some(1), false).await);
    assert_ref(&current, foreign, "Stored comparison wording", None);
}
