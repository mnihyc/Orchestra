//! Compound tree writes admit complete native closures and retain truthful completed receipts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod desk;
mod places;
mod support;

use desk::Desk;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::{document_tree::*, documents::VersionCause};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::{
    documents,
    identity::{self, NewApiToken},
};
use orch_types::{Id, Json, new_id};
use serde_json::json;

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
const COMMANDS: [DocumentTreeCommand; 7] = [
    DocumentTreeCommand::CreateFolder,
    DocumentTreeCommand::Copy,
    DocumentTreeCommand::Move,
    DocumentTreeCommand::Rename,
    DocumentTreeCommand::Delete,
    DocumentTreeCommand::Restore,
    DocumentTreeCommand::RestoreVersion,
];
struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, kind: &str, id: Id, reach: &str, actions: &[&str]) -> Json {
    let reach = if kind == "document" { "self" } else { reach };
    json!({"target":{"kind":kind,"id":id,"reach":reach},"allow":[{"operation":operation,"actions":actions}]})
}
fn scope(rules: Vec<Json>) -> Json {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    json!(scope)
}
async fn key(desk: &Desk, user: Id, rules: Vec<Json>) -> Key {
    let key = Key {
        id: new_id(),
        secret: new_token(API_TOKEN_PREFIX),
    };
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: key.id,
            user_id: user,
            name: "Native compound document operation",
            token_hash: &token_hash(&key.secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + chrono::Duration::hours(1)),
        },
        Some(&scope(rules)),
    )
    .await
    .unwrap();
    key
}
async fn folder(desk: &Desk, node: Id, name: &str) -> Id {
    let request = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&node])
        .token(&desk.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: None,
            name: name.into(),
            client_id: new_id().to_string(),
        });
    let full = request.send().await.ok().into_full().unwrap();
    match full.entries[0].item {
        DocumentTreeItem::Container(id) => id,
        _ => panic!("the actual native folder was not returned"),
    }
}
async fn child(desk: &Desk, parent: Id, title: &str) -> Id {
    let mut body = desk::body(title);
    body.blocks = Some(vec![desk::paragraph("Original native words")]);
    let request = desk
        .world
        .app
        .call(DocumentContainerDocumentCreate, &[&parent])
        .token(&desk.ada.token)
        .body(&body);
    request.send().await.ok().into_full().unwrap().base.id
}
async fn call(desk: &Desk, secret: &str, body: &DocumentTreeOperationBody) -> RawReply {
    let request = desk
        .world
        .app
        .call(DocumentTreeOperate, &[])
        .token(secret)
        .body(body);
    request.send_bytes().await
}
fn accepted(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn state(desk: &Desk) -> Json {
    sqlx::query_scalar("SELECT jsonb_build_object(
        'folders',(SELECT jsonb_agg(to_jsonb(c) ORDER BY id) FROM document_containers c),
        'documents',(SELECT jsonb_agg(to_jsonb(d) ORDER BY id) FROM documents d),
        'blocks',(SELECT jsonb_agg(to_jsonb(b) ORDER BY id) FROM document_blocks b),
        'files',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM document_files f),
        'versions',(SELECT jsonb_agg(to_jsonb(v) ORDER BY document_id,version) FROM document_versions v),
        'pages',(SELECT jsonb_agg(to_jsonb(r) ORDER BY object_kind,object_id,page,profile,permission) FROM permission_rows r WHERE object_kind IN ('document','document_container')),
        'grants',(SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM permission_entries e WHERE object_kind IN ('document','document_container')),
        'operations',(SELECT jsonb_agg(to_jsonb(o) ORDER BY actor_kind,actor_id,request_ref) FROM document_tree_operations o),
        'audit',(SELECT jsonb_agg(to_jsonb(a) ORDER BY at,id) FROM audit_log a WHERE object_kind IN ('document','document_container')),
        'readmes',(SELECT jsonb_agg(jsonb_build_array(id,readme_document_id,tree_seq) ORDER BY id) FROM nodes),
        'blobs',(SELECT jsonb_agg(to_jsonb(b) ORDER BY hash) FROM blobs b))")
        .fetch_one(&desk.world.db.pool).await.unwrap()
}
struct Case {
    desk: Desk,
    body: DocumentTreeOperationBody,
    root: Id,
    document: Id,
    tail: Id,
    destination: Id,
    destination_node: Id,
}
impl Case {
    async fn new(command: DocumentTreeCommand) -> Self {
        let desk = Desk::new().await;
        orch_api_core::routes::resources::sealing::install_master_key(
            orch_db::secrets::MasterKey::new([42; 32]),
        );
        let root = folder(&desk, desk.topic, "Native source").await;
        let document = child(&desk, root, "Original text").await;
        let tail = child(&desk, root, "Private native tail").await;
        let destination_node = desk.another_topic("Tree destination").await;
        let destination = folder(&desk, destination_node, "Native destination").await;
        let items = if matches!(command, DocumentTreeCommand::CreateFolder) {
            Vec::new()
        } else if matches!(
            command,
            DocumentTreeCommand::Rename | DocumentTreeCommand::RestoreVersion
        ) {
            vec![DocumentTreeItem::Document(document)]
        } else {
            vec![DocumentTreeItem::Container(root)]
        };
        let mut body = DocumentTreeOperationBody {
            command,
            items,
            to: matches!(
                command,
                DocumentTreeCommand::CreateFolder
                    | DocumentTreeCommand::Copy
                    | DocumentTreeCommand::Move
            )
            .then_some(DocumentDirectory::Container(destination)),
            name: matches!(
                command,
                DocumentTreeCommand::CreateFolder | DocumentTreeCommand::Rename
            )
            .then(|| "Supplied native title".into()),
            client_id: new_id().to_string(),
            version: None,
            deletion_plan: None,
        };
        if command == DocumentTreeCommand::RestoreVersion {
            let mut tx = desk.world.db.pool.begin().await.unwrap();
            let row = documents::lock(&mut tx, document).await.unwrap().unwrap();
            body.version = Some(
                documents::history::cut(&mut tx, &row, VersionCause::Restore, desk.world.now())
                    .await
                    .unwrap(),
            );
            tx.commit().await.unwrap();
            desk.paragraphs(
                &desk.ada,
                document,
                &["Material written after the saved version"],
            )
            .await;
        }
        if matches!(
            command,
            DocumentTreeCommand::Delete | DocumentTreeCommand::Restore
        ) {
            let mut deletion = DocumentTreeOperationBody {
                command: DocumentTreeCommand::Delete,
                items: vec![DocumentTreeItem::Container(root)],
                to: None,
                name: None,
                client_id: new_id().to_string(),
                version: None,
                deletion_plan: None,
            };
            let request = desk
                .world
                .app
                .call(DocumentTreeDeletePreview, &[])
                .token(&desk.ada.token)
                .body(&deletion);
            deletion.deletion_plan =
                Some(request.send().await.ok().into_full().unwrap().stamp.into());
            if command == DocumentTreeCommand::Delete {
                body.deletion_plan = deletion.deletion_plan;
            } else {
                let full: DocumentTreeReceipt =
                    serde_json::from_value(accepted(call(&desk, &desk.ada.token, &deletion).await))
                        .unwrap();
                assert_eq!(full.entries.len(), 3);
            }
        }
        Self {
            desk,
            body,
            root,
            document,
            tail,
            destination,
            destination_node,
        }
    }
    fn op(&self) -> &'static str {
        match self.body.command {
            DocumentTreeCommand::CreateFolder => "DocumentTreeOperate.create_folder",
            DocumentTreeCommand::Copy => "DocumentTreeOperate.copy",
            DocumentTreeCommand::Move => "DocumentTreeOperate.move",
            DocumentTreeCommand::Rename => "DocumentTreeOperate.rename",
            DocumentTreeCommand::Delete => "DocumentTreeOperate.delete",
            DocumentTreeCommand::Restore => "DocumentTreeOperate.restore",
            DocumentTreeCommand::RestoreVersion => "DocumentTreeOperate.restore_version",
        }
    }
    fn source(&self) -> (&'static str, Id) {
        if matches!(
            self.body.command,
            DocumentTreeCommand::Rename | DocumentTreeCommand::RestoreVersion
        ) {
            ("document", self.document)
        } else {
            ("document_container", self.root)
        }
    }
    fn action(&self) -> &'static str {
        match self.body.command {
            DocumentTreeCommand::Copy => "read",
            DocumentTreeCommand::Rename => "write",
            _ => "manage",
        }
    }
    fn rules(&self, read: bool) -> Vec<Json> {
        let mut rules = Vec::new();
        if self.body.command != DocumentTreeCommand::CreateFolder {
            let (kind, id) = self.source();
            let mut actions = vec![self.action()];
            if read && !actions.contains(&"read") {
                actions.push("read");
            }
            rules.push(rule(self.op(), kind, id, "subtree", &actions));
        }
        if matches!(
            self.body.command,
            DocumentTreeCommand::CreateFolder
                | DocumentTreeCommand::Copy
                | DocumentTreeCommand::Move
                | DocumentTreeCommand::Restore
        ) {
            let (kind, id) = if self.body.command == DocumentTreeCommand::Restore {
                ("topic", self.desk.topic)
            } else {
                ("document_container", self.destination)
            };
            rules.push(rule(
                self.op(),
                kind,
                id,
                "subtree",
                if read { &["write", "read"] } else { &["write"] },
            ));
        }
        if read {
            rules.push(json!({"target":{"kind":"org","id":self.desk.org,"reach":"subtree"},"resources":["org","project","topic","thread"],"allow":[{"operation":self.op(),"actions":["read"]}]}));
        }
        rules
    }
}
async fn wait(desk: &Desk, pid: i32, pattern: &str, task: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(WAIT,async{loop{
        assert!(!task.is_finished(),"the actual native wait must be reached");
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(pattern).fetch_one(&desk.world.db.pool).await.unwrap();
        if blocked{break;}tokio::task::yield_now().await;
    }}).await.expect("observe the actual native query and literal blocking PID");
}

#[tokio::test]
async fn every_tree_intent_keeps_its_native_effect_and_truthful_ids_without_borrowing_get_read() {
    for command in COMMANDS {
        let case = Case::new(command).await;
        let mut rules = case.rules(false);
        let (kind, id) = if command == DocumentTreeCommand::CreateFolder {
            ("document_container", case.destination)
        } else {
            case.source()
        };
        rules.push(rule(
            if kind == "document" {
                "DocumentGet.read"
            } else {
                "DocumentContainerGet.read"
            },
            kind,
            id,
            "subtree",
            &["read"],
        ));
        let token = key(&case.desk, case.desk.ada.id(), rules).await;
        let answer = accepted(call(&case.desk, &token.secret, &case.body).await);
        assert_eq!(answer["projection"], "receipt", "{command:?}");
        assert_eq!(answer.as_object().unwrap().len(), 3);
        let operation: Id = serde_json::from_value(answer["operation"].clone()).unwrap();
        let entries: Vec<DocumentTreeItem> =
            serde_json::from_value(answer["entries"].clone()).unwrap();
        assert!(!entries.is_empty());
        let saved:Id=sqlx::query_scalar("SELECT operation_id FROM document_tree_operations WHERE actor_kind='user' AND actor_id=$1 AND request_ref=$2").bind(case.desk.ada.id()).bind(&case.body.client_id).fetch_one(&case.desk.world.db.pool).await.unwrap();
        assert_eq!(saved, operation);
        let ordinary: DocumentTreeReceipt = serde_json::from_value(accepted(
            call(&case.desk, &case.desk.ada.token, &case.body).await,
        ))
        .unwrap();
        assert_eq!(ordinary.operation, operation);
        assert_eq!(
            ordinary
                .entries
                .iter()
                .map(|entry| entry.item)
                .collect::<Vec<_>>(),
            entries
        );
        match command {
            DocumentTreeCommand::Copy => {
                assert_eq!(ordinary.copied.len(), 3);
                assert!(
                    ordinary
                        .entries
                        .iter()
                        .all(|entry| entry.node == case.destination_node)
                );
            }
            DocumentTreeCommand::Move => assert!(
                ordinary
                    .entries
                    .iter()
                    .all(|entry| entry.node == case.destination_node)
            ),
            DocumentTreeCommand::Delete => assert!(
                ordinary
                    .entries
                    .iter()
                    .all(|entry| entry.deleted_at.is_some())
            ),
            DocumentTreeCommand::Restore => assert!(
                ordinary
                    .entries
                    .iter()
                    .all(|entry| entry.deleted_at.is_none())
            ),
            DocumentTreeCommand::Rename => {
                assert_eq!(ordinary.entries[0].title, "Supplied native title")
            }
            DocumentTreeCommand::CreateFolder => assert!(matches!(
                entries.as_slice(),
                [DocumentTreeItem::Container(_)]
            )),
            DocumentTreeCommand::RestoreVersion => {
                let blocks =
                    documents::live_blocks(&mut *case.desk.world.conn().await, case.document)
                        .await
                        .unwrap();
                assert!(
                    blocks
                        .iter()
                        .all(|row| !row.text.contains("Material written after"))
                );
            }
        }
        let before = state(&case.desk).await;
        assert_eq!(
            accepted(call(&case.desk, &token.secret, &case.body).await),
            answer,
            "{command:?}: native replay keeps its original receipt"
        );
        assert_eq!(
            state(&case.desk).await,
            before,
            "{command:?}: no repeated header, material, pin, page, history or ledger"
        );
    }
}

#[tokio::test]
async fn own_operation_read_returns_the_complete_native_result_for_every_tree_intent() {
    for command in COMMANDS {
        let case = Case::new(command).await;
        let token = key(&case.desk, case.desk.ada.id(), case.rules(true)).await;
        let full: DocumentTreeReceipt =
            serde_json::from_value(accepted(call(&case.desk, &token.secret, &case.body).await))
                .unwrap();
        let ordinary: DocumentTreeReceipt = serde_json::from_value(accepted(
            call(&case.desk, &case.desk.ada.token, &case.body).await,
        ))
        .unwrap();
        assert_eq!(full.operation, ordinary.operation, "{command:?}");
        assert_eq!(full.entries, ordinary.entries, "{command:?}");
        assert_eq!(full.copied, ordinary.copied);
        assert_eq!(
            (full.gaining, full.losing, full.restore_until),
            (ordinary.gaining, ordinary.losing, ordinary.restore_until)
        );
        assert_eq!(
            full.view
                .entries
                .iter()
                .map(|row| row.entry.item)
                .collect::<Vec<_>>(),
            ordinary
                .view
                .entries
                .iter()
                .map(|row| row.entry.item)
                .collect::<Vec<_>>()
        );
        assert!(!full.view.entries.is_empty(), "{command:?}");
    }
}

#[tokio::test]
async fn atomic_tree_intents_require_every_mixed_descendant_and_reject_kind_action_operation_and_native_authority_loans()
 {
    for command in [
        DocumentTreeCommand::Copy,
        DocumentTreeCommand::Move,
        DocumentTreeCommand::Delete,
        DocumentTreeCommand::Restore,
    ] {
        let case = Case::new(command).await;
        let before = state(&case.desk).await;
        for fault in ["self_only", "wrong_kind", "wrong_action", "wrong_operation"] {
            let mut rules = case.rules(false);
            let action = case.action();
            rules[0] = match fault {
                "self_only" => rule(
                    case.op(),
                    "document_container",
                    case.root,
                    "self",
                    &[action],
                ),
                "wrong_kind" => rule(case.op(), "document", case.root, "subtree", &[action]),
                "wrong_action" => rule(
                    case.op(),
                    "document_container",
                    case.root,
                    "subtree",
                    if action == "read" {
                        &["write"]
                    } else {
                        &["read"]
                    },
                ),
                _ => rule(
                    if command == DocumentTreeCommand::Copy {
                        "DocumentGet.read"
                    } else {
                        "DocumentTreeDeletePreview"
                    },
                    "document_container",
                    case.root,
                    "subtree",
                    &[action],
                ),
            };
            rules.push(rule(
                "DocumentGet.read",
                "document",
                case.tail,
                "self",
                &["read"],
            ));
            let token = key(&case.desk, case.desk.ada.id(), rules).await;
            assert_eq!(
                call(&case.desk, &token.secret, &case.body).await.status,
                StatusCode::FORBIDDEN,
                "{command:?}/{fault}"
            );
            assert_eq!(state(&case.desk).await, before, "{command:?}/{fault}");
        }
        let stranger = case.desk.stranger("native-stranger").await;
        let unowned = key(&case.desk, stranger.id(), case.rules(true)).await;
        let refused = call(&case.desk, &unowned.secret, &case.body).await;
        assert!(matches!(
            refused.status,
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ));
        assert_eq!(state(&case.desk).await, before);
    }
}

#[tokio::test]
async fn tree_operation_expiry_after_actual_scope_descendant_readme_and_audit_waits_leaves_no_partial_native_act()
 {
    for footprint in ["scope", "descendant", "readme", "audit"] {
        for expire in [false, true] {
            let case = Case::new(DocumentTreeCommand::Move).await;
            sqlx::query("UPDATE nodes SET readme_document_id=$2 WHERE id=$1")
                .bind(case.desk.topic)
                .bind(case.document)
                .execute(&case.desk.world.db.pool)
                .await
                .unwrap();
            let before = state(&case.desk).await;
            let token = key(&case.desk, case.desk.ada.id(), case.rules(false)).await;
            let mut blocker = case.desk.world.db.pool.begin().await.unwrap();
            let pattern = match footprint {
                "scope" => {
                    sqlx::query(
                        "SELECT node_id FROM document_tree_scopes WHERE node_id=$1 FOR UPDATE",
                    )
                    .bind(case.desk.topic)
                    .fetch_one(&mut *blocker)
                    .await
                    .unwrap();
                    "%SELECT document_tree_lock%"
                }
                "descendant" => {
                    sqlx::query("SELECT id FROM documents WHERE id=$1 FOR UPDATE")
                        .bind(case.tail)
                        .fetch_one(&mut *blocker)
                        .await
                        .unwrap();
                    "%FROM documents%FOR UPDATE%"
                }
                "readme" => {
                    sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
                        .bind(case.desk.topic)
                        .fetch_one(&mut *blocker)
                        .await
                        .unwrap();
                    "%readme_document_id=ANY%FOR UPDATE%"
                }
                _ => {
                    sqlx::query("LOCK TABLE audit_log IN SHARE MODE")
                        .execute(&mut *blocker)
                        .await
                        .unwrap();
                    "%INSERT INTO audit_log%"
                }
            };
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = case.desk.world.app.clone();
            let body = case.body.clone();
            let secret = token.secret.clone();
            let task = tokio::spawn(async move {
                let request = app
                    .call(DocumentTreeOperate, &[])
                    .token(&secret)
                    .body(&body);
                request.send_bytes().await
            });
            wait(&case.desk, pid, pattern, &task).await;
            if expire {
                case.desk.world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(WAIT, task).await.unwrap().unwrap();
            if expire {
                assert!(
                    matches!(
                        reply.status,
                        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                    ),
                    "{footprint}: {}",
                    String::from_utf8_lossy(&reply.raw)
                );
                assert_eq!(state(&case.desk).await, before, "{footprint}");
            } else {
                assert_eq!(accepted(reply)["projection"], "receipt");
                let held: Option<Id> =
                    sqlx::query_scalar("SELECT readme_document_id FROM nodes WHERE id=$1")
                        .bind(case.desk.topic)
                        .fetch_one(&case.desk.world.db.pool)
                        .await
                        .unwrap();
                assert!(held.is_none());
            }
        }
    }
}

#[tokio::test]
async fn completed_tree_publication_uses_current_result_and_context_read_without_reapplying_input_authority()
 {
    for (command, change) in [
        (DocumentTreeCommand::Rename, "none"),
        (DocumentTreeCommand::Rename, "input_write"),
        (DocumentTreeCommand::Rename, "result_read"),
        (DocumentTreeCommand::Rename, "expired"),
        (DocumentTreeCommand::Rename, "placement"),
        (DocumentTreeCommand::Copy, "context_read"),
    ] {
        let case = Case::new(command).await;
        let token = key(&case.desk, case.desk.ada.id(), case.rules(true)).await;
        let mut blocker = case.desk.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = case.desk.world.app.clone();
        let body = case.body.clone();
        let secret = token.secret.clone();
        let task = tokio::spawn(async move {
            let request = app
                .call(DocumentTreeOperate, &[])
                .token(&secret)
                .body(&body);
            request.send_bytes().await
        });
        wait(&case.desk, pid, "%INSERT INTO api_token_activity%", &task).await;
        let operation:Id=sqlx::query_scalar("SELECT operation_id FROM document_tree_operations WHERE actor_kind='user' AND actor_id=$1 AND request_ref=$2").bind(case.desk.ada.id()).bind(&case.body.client_id).fetch_one(&case.desk.world.db.pool).await.unwrap();
        if matches!(change, "input_write" | "result_read" | "context_read") {
            let mut rules = case.rules(true);
            rules[0]["allow"][0]["actions"] = json!(if change == "input_write" {
                vec!["read"]
            } else if change == "result_read" {
                vec!["write"]
            } else {
                vec!["write"]
            });
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(token.id)
                .bind(scope(rules))
                .execute(&case.desk.world.db.pool)
                .await
                .unwrap();
        } else if change == "expired" {
            case.desk.world.advance(chrono::Duration::hours(2));
        } else if change == "placement" {
            sqlx::query("UPDATE documents SET parent_container_id=NULL WHERE id=$1")
                .bind(case.document)
                .execute(&case.desk.world.db.pool)
                .await
                .unwrap();
        }
        let committed = state(&case.desk).await;
        blocker.commit().await.unwrap();
        let answer = accepted(tokio::time::timeout(WAIT, task).await.unwrap().unwrap());
        assert_eq!(answer["operation"], json!(operation));
        if matches!(change, "none" | "input_write") {
            assert!(answer.get("projection").is_none());
            assert_eq!(answer["entries"][0]["title"], "Supplied native title");
        } else {
            assert_eq!(answer["projection"], "receipt");
            assert_eq!(answer.as_object().unwrap().len(), 3);
        }
        assert_eq!(
            state(&case.desk).await,
            committed,
            "{change}: no repeated operation, material, audit, history or permission work"
        );
    }
}
