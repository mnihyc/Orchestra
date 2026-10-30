//! Real HTTP and database proofs of permanent document scope, deleted inspection and stale deletion selection.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::{
    documents::*,
    nodes::{AuditObjectState, NodeAuditList, NodeAuditListQuery},
};
use orch_types::{Id, Time};
use serde_json::json;
use support::World;

async fn world() -> World {
    World::new().await
}

async fn make(world: &World, who: &places::Who, node: Id, title: &str) -> Document {
    world
        .app
        .call(NodeDocumentCreate, &[&node])
        .token(&who.token)
        .body(&NodeDocumentCreateBody {
            title: title.to_owned(),
            markdown: Some("retained evidence body".to_owned()),
            blocks: None,
            access: None,
        })
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn document_acts_keep_their_event_time_place_and_no_content_before_and_after_purge() {
    let world = world().await;
    let ada = places::person(&world, "ada").await;
    let lee = places::person(&world, "lee").await;
    let org = places::make_org(&world, &ada, "Acme", "acme").await;
    places::join(&world, org.base.id, org.base.id, &lee, "developer", &ada).await;
    let project = places::make_project(&world, &ada, org.base.id, "Project").await;
    let source = places::make_topic(&world, &ada, project.base.id, "Source").await;
    let target = places::make_topic(&world, &ada, project.base.id, "Target").await;
    let doc = make(&world, &lee, source.base.id, "Original name").await;
    let id = doc.base.id;
    world
        .app
        .call(DocumentUpdate, &[&id])
        .token(&lee.token)
        .body(&DocumentUpdateBody {
            title: Some("Renamed".to_owned()),
            description: None,
        })
        .send()
        .await
        .ok();
    world
        .app
        .call(DocumentMove, &[&id])
        .token(&lee.token)
        .body(&DocumentMoveBody {
            node_id: target.base.id,
        })
        .send()
        .await
        .ok();
    world
        .app
        .call(DocumentDelete, &[&id])
        .token(&lee.token)
        .send()
        .await
        .ok();
    world
        .app
        .call(DocumentRestore, &[&id])
        .token(&lee.token)
        .body(&DocumentRestoreBody {
            expected_deletion_id: None,
        })
        .send()
        .await
        .ok();
    world
        .app
        .call(DocumentDelete, &[&id])
        .token(&lee.token)
        .send()
        .await
        .ok();
    let source_rows = world
        .app
        .call(NodeAuditList, &[&source.base.id])
        .token(&ada.token)
        .query(&NodeAuditListQuery {
            actor: Some(lee.person.id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    for action in ["document.created", "document.renamed", "document.moved"] {
        assert!(
            source_rows.items.iter().any(|r| r.action == action),
            "{action}"
        );
    }
    let target_rows = world
        .app
        .call(NodeAuditList, &[&target.base.id])
        .token(&ada.token)
        .query(&NodeAuditListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        target_rows
            .items
            .iter()
            .any(|r| r.action == "document.moved")
    );
    assert!(
        target_rows
            .items
            .iter()
            .any(|r| r.action == "document.deleted")
    );
    assert!(
        target_rows
            .items
            .iter()
            .any(|r| r.action == "document.restored"
                && r.node.as_ref().is_some_and(|n| n.id == target.base.id))
    );
    assert!(
        source_rows
            .items
            .iter()
            .all(|r| !r.details.to_string().contains("retained evidence body"))
    );
    assert!(source_rows.items.iter().any(
        |r| r.action == "document.created" && r.object.name.as_deref() == Some("Original name")
    ));
    world.advance(chrono::Duration::days(31));
    let mut tx = orch_db::Db::from_pool(world.db.pool.clone())
        .begin()
        .await
        .unwrap();
    let purged = orch_db::queries::notifications::upkeep::purge_soft_deleted(
        &mut tx,
        world.now() - chrono::Duration::days(30),
        world.now(),
    )
    .await
    .unwrap();
    assert!(purged.iter().any(|p| p.table == "documents" && p.rows == 1));
    tx.commit().await.unwrap();
    let after = world
        .app
        .call(NodeAuditList, &[&source.base.id])
        .token(&ada.token)
        .query(&NodeAuditListQuery::default())
        .send()
        .await
        .ok();
    let event = after
        .items
        .iter()
        .find(|r| r.action == "document.created")
        .unwrap();
    assert_eq!(event.object.state, AuditObjectState::Purged);
    assert_eq!(event.object.purged_at, Some(Time::new(world.now())));
    assert!(!event.object.can_inspect && !event.object.can_restore);
}

#[tokio::test]
async fn deleted_document_inspection_is_daily_read_only_and_an_old_event_cannot_restore_a_new_delete_at_the_same_time()
 {
    let world = world().await;
    let ada = places::person(&world, "ada").await;
    let org = places::make_org(&world, &ada, "Acme", "acme").await;
    let project = places::make_project(&world, &ada, org.base.id, "Project").await;
    let topic = places::make_topic(&world, &ada, project.base.id, "Topic").await;
    let doc = make(&world, &ada, topic.base.id, "Inspection").await;
    let id = doc.base.id;
    world
        .app
        .call(DocumentDelete, &[&id])
        .token(&ada.token)
        .send()
        .await
        .ok();
    let first: Id = sqlx::query_scalar(
        "SELECT deletion_id FROM audit_object_state WHERE object_kind='document' AND object_id=$1",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    let before:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM document_states),(SELECT count(*) FROM notifications),(SELECT count(*) FROM document_versions)").fetch_one(&world.db.pool).await.unwrap();
    let before_rows:serde_json::Value=sqlx::query_scalar("SELECT jsonb_build_object('state',(SELECT jsonb_agg(to_jsonb(s) ORDER BY to_jsonb(s)::text) FROM document_states s),'notices',(SELECT jsonb_agg(to_jsonb(n) ORDER BY n.id) FROM notifications n))").fetch_one(&world.db.pool).await.unwrap();
    for _ in 0..2 {
        let inspected = world
            .app
            .call(DocumentGet, &[&id])
            .token(&ada.token)
            .query(&orch_api_contract::nodes::DeletedInspectionQuery {
                inspect_deleted: Some(true),
            })
            .send()
            .await
            .ok();
        assert!(inspected.inspection.unwrap().readonly);
        assert_eq!(inspected.base.title, "Inspection");
    }
    let after:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM document_states),(SELECT count(*) FROM notifications),(SELECT count(*) FROM document_versions)").fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(before, after);
    let after_rows:serde_json::Value=sqlx::query_scalar("SELECT jsonb_build_object('state',(SELECT jsonb_agg(to_jsonb(s) ORDER BY to_jsonb(s)::text) FROM document_states s),'notices',(SELECT jsonb_agg(to_jsonb(n) ORDER BY n.id) FROM notifications n))").fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(before_rows, after_rows);
    let (reads,history):(i64,bool)=sqlx::query_as("SELECT count(*),bool_or(history) FROM audit_log WHERE action='deleted.inspected' AND object_id=$1").bind(id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!((reads, history), (1, false));
    let refused = world
        .app
        .call(DocumentSeen, &[&id])
        .token(&ada.token)
        .raw_query("inspect_deleted=true")
        .body(&DocumentSeenBody { seq: doc.base.seq })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    world
        .app
        .call(DocumentRestore, &[&id])
        .token(&ada.token)
        .body(&DocumentRestoreBody {
            expected_deletion_id: Some(first),
        })
        .send()
        .await
        .ok();
    world
        .app
        .call(DocumentDelete, &[&id])
        .token(&ada.token)
        .send()
        .await
        .ok();
    let second: Id = sqlx::query_scalar(
        "SELECT deletion_id FROM audit_object_state WHERE object_kind='document' AND object_id=$1",
    )
    .bind(id)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_ne!(first, second);
    let refused = world
        .app
        .call(DocumentRestore, &[&id])
        .token(&ada.token)
        .body(&DocumentRestoreBody {
            expected_deletion_id: Some(first),
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    let rows = world
        .app
        .call(NodeAuditList, &[&topic.base.id])
        .token(&ada.token)
        .query(&NodeAuditListQuery::default())
        .send()
        .await
        .ok();
    let old = rows
        .items
        .iter()
        .find(|r| r.event_deletion_id == Some(first))
        .unwrap();
    assert_eq!(old.object.deletion_id, Some(second));
    assert!(
        !json!(old.details)
            .to_string()
            .contains("retained evidence body")
    );
    world.advance(chrono::Duration::days(1));
    world
        .app
        .call(DocumentGet, &[&id])
        .token(&ada.token)
        .raw_query("inspect_deleted=true")
        .send()
        .await
        .ok();
    let (reads,history):(i64,bool)=sqlx::query_as("SELECT count(*),bool_or(history) FROM audit_log WHERE action='deleted.inspected' AND object_id=$1").bind(id).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!((reads, history), (2, false));
}

#[tokio::test]
async fn a_deleted_child_requires_its_own_restore_guard_and_a_parent_is_restored_separately() {
    use orch_api_contract::{
        nodes::{NodeChildList, NodeChildListQuery, NodeGet},
        ownership::{NodeDelete, NodeRestore},
    };
    let world = world().await;
    let ada = places::person(&world, "ada").await;
    let reader = places::person(&world, "reader").await;
    let org = places::make_org(&world, &ada, "Acme", "acme").await;
    places::join(&world, org.base.id, org.base.id, &reader, "viewer", &ada).await;
    let project = places::make_project(&world, &ada, org.base.id, "Project").await;
    let topic = places::make_topic(&world, &ada, project.base.id, "Private child").await;
    let doc = make(&world, &ada, topic.base.id, "The document").await;
    let scoped = world
        .app
        .call(orch_api_contract::auth::MeTokenCreate, &[])
        .token(&ada.token)
        .body(&orch_api_contract::auth::ApiTokenCreateBody {
            name: "Only the child".to_owned(),
            scopes: Some(vec!["*".to_owned()]),
            node_id: Some(topic.base.id),
            expires_at: None,
            proof: orch_api_contract::auth::SensitiveProof {
                password: Some(support::PASSWORD.to_owned()),
                ..Default::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .expect("the first answer carries the secret");
    world
        .app
        .call(NodeDelete, &[&project.base.id])
        .token(&ada.token)
        .raw_body(&json!({}))
        .send()
        .await
        .ok();
    let deletion: Id = sqlx::query_scalar("SELECT deletion_id FROM nodes WHERE id=$1")
        .bind(project.base.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let inspected = world
        .app
        .call(NodeGet, &[&topic.base.id])
        .token(&ada.token)
        .raw_query("inspect_deleted=true")
        .send()
        .await;
    assert_eq!(inspected.status, 200, "{:?}", inspected.body);
    let refused = world
        .app
        .call(NodeGet, &[&topic.base.id])
        .token(&reader.token)
        .raw_query("inspect_deleted=true")
        .send()
        .await
        .problem();
    assert_ne!(refused.status, 200);
    let scoped_refused = world
        .app
        .call(NodeGet, &[&topic.base.id])
        .token(&scoped)
        .raw_query("inspect_deleted=true")
        .send()
        .await
        .problem();
    assert_ne!(
        scoped_refused.status, 200,
        "the token cannot borrow the parent restoration right"
    );
    let scoped_doc = world
        .app
        .call(DocumentGet, &[&doc.base.id])
        .token(&scoped)
        .raw_query("inspect_deleted=true")
        .send()
        .await
        .ok();
    assert!(scoped_doc.inspection.unwrap().readonly);
    {
        let mut conn = world.conn().await;
        orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("node", org.base.id, "live"),
            "any",
            "org.manage",
            ("user", Some(reader.id()), None),
            None,
            world.now(),
        )
        .await
        .unwrap();
        orch_authz::epoch::bump(&mut conn).await.unwrap();
    }
    let isolated = world
        .app
        .call(NodeChildList, &[&project.base.id])
        .token(&reader.token)
        .query(&NodeChildListQuery {
            inspect_deleted: Some(true),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        isolated.items.is_empty(),
        "restoring this project uses org.manage; its child independently needs project.manage"
    );
    let refused = world
        .app
        .call(NodeRestore, &[&topic.base.id])
        .token(&ada.token)
        .raw_body(&json!({"expected_deletion_id":deletion}))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NodeRestoreParentFirst);
    let refused = world
        .app
        .call(DocumentRestore, &[&doc.base.id])
        .token(&ada.token)
        .raw_body(&json!({"expected_deletion_id":deletion}))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NodeRestoreParentFirst);
    let children = world
        .app
        .call(NodeChildList, &[&project.base.id])
        .token(&ada.token)
        .query(&NodeChildListQuery {
            inspect_deleted: Some(true),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(children.items.len(), 1);
    world
        .app
        .call(NodeRestore, &[&project.base.id])
        .token(&ada.token)
        .raw_body(&json!({"expected_deletion_id":deletion}))
        .send()
        .await
        .ok();
    assert_eq!(
        world
            .app
            .call(DocumentGet, &[&doc.base.id])
            .token(&ada.token)
            .send()
            .await
            .ok()
            .base
            .title,
        "The document"
    );
    world
        .app
        .call(NodeDelete, &[&project.base.id])
        .token(&ada.token)
        .raw_body(&json!({}))
        .send()
        .await
        .ok();
    let refused = world
        .app
        .call(NodeRestore, &[&project.base.id])
        .token(&ada.token)
        .raw_body(&json!({"expected_deletion_id":deletion}))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    world
        .app
        .call(NodeDelete, &[&org.base.id])
        .token(&ada.token)
        .raw_body(&json!({}))
        .send()
        .await
        .ok();
    let root_group: Id = sqlx::query_scalar("SELECT deletion_id FROM nodes WHERE id=$1")
        .bind(org.base.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    world
        .app
        .call(NodeGet, &[&org.base.id])
        .token(&ada.token)
        .raw_query("inspect_deleted=true")
        .send()
        .await
        .ok();
    let denied_root = world
        .app
        .call(NodeGet, &[&org.base.id])
        .token(&reader.token)
        .raw_query("inspect_deleted=true")
        .send()
        .await
        .problem();
    assert_ne!(denied_root.status, 200);
    world
        .app
        .call(NodeRestore, &[&org.base.id])
        .token(&ada.token)
        .raw_body(&json!({"expected_deletion_id":root_group}))
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn kind_and_action_intersect_before_paging_and_both_export_formats_hold_the_same_real_acts() {
    use orch_api_contract::admin::{AdminAuditList, AdminAuditListQuery, AuditExportFormat};
    use orch_api_contract::nodes::{AuditKind, NodeAuditExport, NodeAuditExportQuery};
    let world = world().await;
    let ada = places::admin(&world, "ada").await;
    let lee = places::person(&world, "lee").await;
    let org = places::make_org(&world, &ada, "Acme", "acme").await;
    places::join(&world, org.base.id, org.base.id, &lee, "developer", &ada).await;
    let project = places::make_project(&world, &ada, org.base.id, "Project").await;
    let topic = places::make_topic(&world, &ada, project.base.id, "Topic").await;
    let doc = make(&world, &lee, topic.base.id, "First").await;
    for title in ["Second", "Third", "Fourth"] {
        world
            .app
            .call(DocumentUpdate, &[&doc.base.id])
            .token(&lee.token)
            .body(&DocumentUpdateBody {
                title: Some(title.to_owned()),
                description: None,
            })
            .send()
            .await
            .ok();
    }
    let query = NodeAuditListQuery {
        actor: Some(lee.person.id),
        kind: Some(AuditKind::Changed),
        action: Some("document.renamed".to_owned()),
        limit: Some(1),
        ..Default::default()
    };
    let first = world
        .app
        .call(NodeAuditList, &[&topic.base.id])
        .token(&ada.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(first.items.len(), 1);
    assert!(first.next_cursor.is_some());
    let second = world
        .app
        .call(NodeAuditList, &[&topic.base.id])
        .token(&ada.token)
        .query(&NodeAuditListQuery {
            cursor: first.next_cursor,
            ..query.clone()
        })
        .send()
        .await
        .ok();
    assert_eq!(second.items.len(), 1);
    assert_ne!(first.items[0].id, second.items[0].id);
    let contradictory = world
        .app
        .call(NodeAuditList, &[&topic.base.id])
        .token(&ada.token)
        .query(&NodeAuditListQuery {
            kind: Some(AuditKind::Deleted),
            ..query.clone()
        })
        .send()
        .await
        .ok();
    assert!(contradictory.items.is_empty());
    assert!(contradictory.next_cursor.is_none());
    let system = world
        .app
        .call(AdminAuditList, &[])
        .token(&ada.token)
        .query(&AdminAuditListQuery {
            actor: query.actor,
            kind: query.kind,
            action: query.action.clone(),
            node_id: Some(topic.base.id),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(system.items.len(), 3);
    for format in [AuditExportFormat::Jsonl, AuditExportFormat::Csv] {
        let file = world
            .app
            .call(NodeAuditExport, &[&topic.base.id])
            .token(&ada.token)
            .query(&NodeAuditExportQuery {
                actor: query.actor,
                include_agents: Some(true),
                action: query.action.clone(),
                kind: query.kind,
                from: Time::new(world.now() - chrono::Duration::days(1)),
                to: Time::new(world.now() + chrono::Duration::days(1)),
                format,
            })
            .send_bytes()
            .await;
        assert_eq!(file.status, 200);
        let body = std::str::from_utf8(&file.raw).unwrap();
        assert_eq!(
            body.lines().count(),
            if format == AuditExportFormat::Jsonl {
                3
            } else {
                4
            }
        );
        assert!(!body.contains("retained evidence body"));
        if format == AuditExportFormat::Jsonl {
            let entries: Vec<orch_api_contract::nodes::AuditEntry> = body
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(
                entries
                    .iter()
                    .map(|e| e.id)
                    .collect::<std::collections::BTreeSet<_>>(),
                system.items.iter().map(|e| e.id).collect()
            );
        }
    }
    let too_long = world
        .app
        .call(NodeAuditExport, &[&topic.base.id])
        .token(&ada.token)
        .query(&NodeAuditExportQuery {
            actor: None,
            include_agents: None,
            action: None,
            kind: None,
            from: Time::new(world.now()),
            to: Time::new(world.now() + chrono::Duration::days(367)),
            format: AuditExportFormat::Jsonl,
        })
        .send_bytes()
        .await;
    assert_eq!(too_long.status, 400);
}

#[tokio::test]
async fn a_stream_uses_one_snapshot_and_stops_before_its_next_page_when_a_current_reader_is_revoked()
 {
    use http_body_util::BodyExt;
    use orch_api_contract::admin::{AdminAuditExport, AdminAuditExportQuery, AuditExportFormat};
    let world = world().await;
    let ada = places::admin(&world, "ada").await;
    let org = places::make_org(&world, &ada, "Acme", "acme").await;
    let project = places::make_project(&world, &ada, org.base.id, "Project").await;
    let topic = places::make_topic(&world, &ada, project.base.id, "Topic").await;
    let doc = make(&world, &ada, topic.base.id, "Before").await;
    for index in 0..501 {
        world
            .app
            .call(DocumentUpdate, &[&doc.base.id])
            .token(&ada.token)
            .body(&DocumentUpdateBody {
                title: Some(format!("Name {index}")),
                description: None,
            })
            .send()
            .await
            .ok();
    }
    let response = world
        .app
        .call(AdminAuditExport, &[])
        .token(&ada.token)
        .query(&AdminAuditExportQuery {
            actor: Some(ada.person.id),
            include_agents: Some(true),
            action: Some("document.renamed".to_owned()),
            kind: None,
            node_id: Some(topic.base.id),
            from: Time::new(world.now() - chrono::Duration::days(1)),
            to: Time::new(world.now() + chrono::Duration::days(1)),
            format: AuditExportFormat::Jsonl,
        })
        .send_response()
        .await;
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();
    world
        .app
        .call(DocumentUpdate, &[&doc.base.id])
        .token(&ada.token)
        .body(&DocumentUpdateBody {
            title: Some("After snapshot".to_owned()),
            description: None,
        })
        .send()
        .await
        .ok();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(std::str::from_utf8(&first).unwrap().lines().count(), 500);
    assert!(
        !std::str::from_utf8(&first)
            .unwrap()
            .contains("After snapshot")
    );
    let mut tx = orch_db::Db::from_pool(world.db.pool.clone())
        .begin()
        .await
        .unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
        .bind(ada.person.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(body.frame().await.unwrap().is_err());
    assert!(body.frame().await.is_none());
    let exported:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='audit.exported' AND details->>'scope'='system'").fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(exported, 1);
}

#[tokio::test]
async fn provider_endpoint_history_keeps_only_canonical_hosts_after_changes_and_deletion() {
    use orch_api_contract::resources::{
        CreateProvider, ProviderCreate, ProviderDelete, ProviderUpdate, Upstream,
    };
    let world = world().await;
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([7; 32]),
    );
    let ada = places::person(&world, "ada").await;
    let org = places::make_org(&world, &ada, "Acme", "acme").await;
    let created = world
        .app
        .call(ProviderCreate, &[])
        .token(&ada.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Model server".to_owned(),
            home_id: org.base.id,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![Upstream {
                protocol: "openai-responses".to_owned(),
                base_url: "https://API.Example.TEST:8443/private/path".to_owned(),
                options: json!({}),
            }],
            api_key: Some("private-provider-key".to_owned()),
            secrets: None,
            key_machine_id: None,
            sources: None,
        })
        .send()
        .await
        .ok()
        .provider;
    let id = created.base.id;
    let refused=world.app.call(ProviderUpdate,&[&id]).token(&ada.token).raw_body(&json!({"expected_revision":created.revision,"upstreams":[{"protocol":"openai-responses","base_url":"https://someone:private-credential@API.Example.TEST/private?key=private-query","options":{}}]})).send().await.problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    world.app.call(ProviderUpdate,&[&id]).token(&ada.token).raw_body(&json!({"expected_revision":created.revision,"upstreams":[{"protocol":"openai-responses","base_url":"https://Second.Example.TEST:8443/other-secret-path","options":{}}]})).send().await.ok();
    world
        .app
        .call(ProviderDelete, &[&id])
        .token(&ada.token)
        .send()
        .await
        .ok();
    let rows = world
        .app
        .call(NodeAuditList, &[&org.base.id])
        .token(&ada.token)
        .query(&NodeAuditListQuery::default())
        .send()
        .await
        .ok();
    for (action, host) in [
        ("provider.created", "api.example.test"),
        ("provider.updated", "second.example.test"),
    ] {
        let event = rows
            .items
            .iter()
            .find(|e| e.action == action && e.object.id == Some(id))
            .unwrap();
        assert_eq!(event.details["endpoint_hosts"], json!([host]));
        let text = serde_json::to_string(event).unwrap();
        for excluded in [
            "8443",
            "private/path",
            "private-query",
            "other-secret-path",
            "second-query",
            "private-provider-key",
        ] {
            assert!(!text.contains(excluded), "{excluded}");
        }
    }
}
