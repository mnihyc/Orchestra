//! Individual document summary options use real HTTP/native permissions, CAS and receipt boundaries.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]
mod desk;
#[path = "document_summary_options/permission_rules.rs"]
mod permission_rules;
mod places;
mod support;
use desk::Desk;
use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::{
    common::{ErrorCode, ReceiptProjection},
    summaries::{
        DocumentSummaryOptionsGet, DocumentSummaryOptionsSet, SummaryOptionsBody, SummaryTarget,
    },
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{
    documents,
    identity::{self, NewApiToken},
    summaries,
};
use orch_settings::Target;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, new_id};
use serde_json::json;

fn body(options: Json, revision: i64) -> SummaryOptionsBody {
    serde_json::from_value(json!({"options":options,"expected_revision":revision})).unwrap()
}
async fn key(desk: &Desk, rule: Json, lifetime: chrono::Duration) -> String {
    let policy: KeyScope = serde_json::from_value(json!({"rules":[rule]})).unwrap();
    assert!(policy.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *desk.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: desk.ada.id(),
            name: "Document summary options",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: desk.world.now(),
            expires_at: Some(desk.world.now() + lifetime),
        },
        Some(&serde_json::to_value(policy).unwrap()),
    )
    .await
    .unwrap();
    secret
}
fn rule(kind: &str, id: Id, operation: &str, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":operation,"actions":[action]}]})
}

async fn waiting_on(desk: &Desk, pattern: &str) {
    // One fixture watchdog for native lock cuts, independent of the controlled domain clock.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() \
                 AND pid<>pg_backend_pid() AND wait_event_type='Lock' AND query LIKE $1)",
            )
            .bind(pattern)
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the actual native source wait is reached");
}

#[tokio::test]
async fn reading_and_setting_own_options_never_generates_or_rewrites_the_document() {
    let desk = Desk::new().await;
    let document = desk.make(&desk.ada, "Written plan").await.base.id;
    desk.paragraphs(&desk.ada, document, &["Keep this exact written content."])
        .await;
    let target = SummaryTarget::Document { id: document };
    let before = documents::get(&mut *desk.world.conn().await, document)
        .await
        .unwrap()
        .unwrap();
    let initial = desk
        .world
        .app
        .call(DocumentSummaryOptionsGet, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(initial.target, target);
    assert_eq!(initial.revision, 0);
    assert!(initial.own.is_none());
    assert!(!initial.options.automatic);
    assert_eq!(initial.min_interval_secs, 300);
    assert!(initial.options.min_interval_secs.is_none());
    assert!(initial.writable);
    assert!(!initial.locked);
    assert!(
        summaries::get(&mut *desk.world.conn().await, target, None)
            .await
            .unwrap()
            .is_none(),
        "a read creates no cache"
    );
    let reply = desk
        .world
        .app
        .call(DocumentSummaryOptionsSet, &[&document])
        .token(&desk.ada.token)
        .body(&body(
            json!({"automatic":true,"on":["content_changed"],"output_chars":1200}),
            0,
        ))
        .send()
        .await
        .ok();
    assert_eq!(reply.projection, ReceiptProjection::Receipt);
    assert_eq!(reply.target, target);
    assert_eq!(reply.revision, 1);
    let found = desk
        .world
        .app
        .call(DocumentSummaryOptionsGet, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(found.options.automatic);
    assert_eq!(found.min_interval_secs, 300);
    assert!(
        found.options.min_interval_secs.is_none(),
        "an effective default is not a stored own choice"
    );
    assert_eq!(found.options.output_chars.unwrap().get(), 1200);
    assert_eq!(found.own, Some(found.options.clone()));
    let after = documents::get(&mut *desk.world.conn().await, document)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before, after,
        "only the summary options/cache/audit changed"
    );
    let work:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM resource_summary_requests)+(SELECT count(*) FROM model_inputs WHERE purpose='summary')+(SELECT count(*) FROM summary_calls)")
        .fetch_one(&desk.world.db.pool).await.unwrap();
    assert_eq!(
        work, 0,
        "setting automatic options is not a Refresh/model act"
    );
}

#[tokio::test]
async fn an_individual_document_inherits_resets_and_obeys_the_containing_places_lock_and_revision()
{
    let desk = Desk::new().await;
    let document = desk.make(&desk.ada, "Plan").await.base.id;
    desk.world
        .set_at(
            Target::Node(desk.topic),
            keys::SUMMARY_TARGETS,
            json!({"document":{"automatic":true,"output_chars":900}}),
        )
        .await;
    let own = json!({"automatic":false,"on":[],"min_interval_secs":0,"output_chars":2000});
    desk.world
        .app
        .call(DocumentSummaryOptionsSet, &[&document])
        .token(&desk.ada.token)
        .body(&body(own.clone(), 0))
        .send()
        .await
        .ok();
    let selected = desk
        .world
        .app
        .call(DocumentSummaryOptionsGet, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(serde_json::to_value(selected.options).unwrap(), own);
    let stale = desk
        .world
        .app
        .call(DocumentSummaryOptionsSet, &[&document])
        .token(&desk.ada.token)
        .body(&body(json!({"automatic":true}), 0))
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    sqlx::query("UPDATE settings SET locked=true WHERE node_id=$1 AND key=$2")
        .bind(desk.topic)
        .bind(keys::SUMMARY_TARGETS)
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    let locked = desk
        .world
        .app
        .call(DocumentSummaryOptionsGet, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(locked.locked);
    assert!(!locked.writable);
    assert_eq!(locked.options.output_chars.unwrap().get(), 900);
    assert!(
        locked.own.is_some(),
        "ancestor locking never rewrites the stored own choice"
    );
    let refused = desk
        .world
        .app
        .call(DocumentSummaryOptionsSet, &[&document])
        .token(&desk.ada.token)
        .body(&body(Json::Null, 1))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AccessLocked);
    sqlx::query("UPDATE settings SET locked=false WHERE node_id=$1 AND key=$2")
        .bind(desk.topic)
        .bind(keys::SUMMARY_TARGETS)
        .execute(&desk.world.db.pool)
        .await
        .unwrap();
    let inherited = desk
        .world
        .app
        .call(DocumentSummaryOptionsSet, &[&document])
        .token(&desk.ada.token)
        .body(&body(Json::Null, 1))
        .send()
        .await
        .ok();
    assert_eq!(inherited.revision, 2);
    let read = desk
        .world
        .app
        .call(DocumentSummaryOptionsGet, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(read.own.is_none());
    assert_eq!(read.options.output_chars.unwrap().get(), 900);
    let a = body(json!({"automatic":false,"output_chars":800}), 2);
    let b = body(json!({"automatic":false,"output_chars":700}), 2);
    let (a, b) = tokio::join!(
        desk.world
            .app
            .call(DocumentSummaryOptionsSet, &[&document])
            .token(&desk.ada.token)
            .body(&a)
            .send(),
        desk.world
            .app
            .call(DocumentSummaryOptionsSet, &[&document])
            .token(&desk.ada.token)
            .body(&b)
            .send(),
    );
    assert!(
        (a.status == StatusCode::OK && b.status == StatusCode::CONFLICT)
            || (b.status == StatusCode::OK && a.status == StatusCode::CONFLICT)
    );
    let read = desk
        .world
        .app
        .call(DocumentSummaryOptionsGet, &[&document])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(read.revision, 3);
}

#[tokio::test]
async fn native_read_and_write_roles_keep_document_identity_and_never_lend_content_to_a_write_only_key()
 {
    let desk = Desk::new().await;
    let document = desk.make(&desk.ada, "Private authored text").await.base.id;
    let life = chrono::Duration::hours(1);
    let write = key(
        &desk,
        rule("document", document, "DocumentSummaryOptionsSet", "write"),
        life,
    )
    .await;
    let result = desk
        .world
        .app
        .call(DocumentSummaryOptionsSet, &[&document])
        .token(&write)
        .body(&body(json!({"automatic":false,"output_chars":1200}), 0))
        .send_bytes()
        .await;
    assert_eq!(
        result.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&result.raw)
    );
    let receipt: Json = serde_json::from_slice(&result.raw).unwrap();
    assert_eq!(
        receipt,
        json!({"projection":"receipt","target":{"kind":"document","id":document},"revision":1})
    );
    let cannot_read = desk
        .world
        .app
        .call(DocumentSummaryOptionsGet, &[&document])
        .token(&write)
        .send_bytes()
        .await;
    assert_eq!(cannot_read.status, StatusCode::FORBIDDEN);
    let read = key(
        &desk,
        rule("document", document, "DocumentSummaryOptionsGet", "read"),
        life,
    )
    .await;
    let options = desk
        .world
        .app
        .call(DocumentSummaryOptionsGet, &[&document])
        .token(&read)
        .send()
        .await
        .ok();
    assert_eq!(options.revision, 1);
    let cannot_write = desk
        .world
        .app
        .call(DocumentSummaryOptionsSet, &[&document])
        .token(&read)
        .body(&body(Json::Null, 1))
        .send_bytes()
        .await;
    assert_eq!(cannot_write.status, StatusCode::FORBIDDEN);
    let parent_only = key(
        &desk,
        rule("topic", desk.topic, "DocumentSummaryOptionsGet", "read"),
        life,
    )
    .await;
    let refused = desk
        .world
        .app
        .call(DocumentSummaryOptionsGet, &[&document])
        .token(&parent_only)
        .send_bytes()
        .await;
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "a topic-family rule cannot become document Read through a guessed parent"
    );
    let outsider = desk.stranger("outside").await;
    assert_ne!(
        desk.world
            .app
            .call(DocumentSummaryOptionsGet, &[&document])
            .token(&outsider.token)
            .send_bytes()
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn an_original_key_expiring_during_the_native_document_wait_has_no_first_write() {
    let desk = Desk::new().await;
    let document = desk.make(&desk.ada, "No late write").await.base.id;
    let key = key(
        &desk,
        rule("document", document, "DocumentSummaryOptionsSet", "write"),
        chrono::Duration::seconds(1),
    )
    .await;
    let mut holder = desk.world.db.pool.begin().await.unwrap();
    documents::lock(&mut holder, document)
        .await
        .unwrap()
        .unwrap();
    let request = desk
        .world
        .app
        .call(DocumentSummaryOptionsSet, &[&document])
        .token(&key)
        .body(&body(json!({"automatic":true}), 0));
    let release = async {
        // This is a fixture watchdog, not a product pacing/retry value. Wait for the real
        // PostgreSQL source-row wait, then move only the existing controllable domain clock.
        waiting_on(&desk, "%FROM documents d%").await;
        desk.world.advance(chrono::Duration::seconds(2));
        holder.commit().await.unwrap();
    };
    let (result, ()) = tokio::join!(request.send_bytes(), release);
    assert_ne!(
        result.status,
        StatusCode::OK,
        "a live first-effect boundary must refuse the expired original key"
    );
    assert!(
        summaries::get(
            &mut *desk.world.conn().await,
            SummaryTarget::Document { id: document },
            None
        )
        .await
        .unwrap()
        .is_none()
    );
}

#[tokio::test]
async fn a_document_moving_during_native_preparation_cannot_borrow_the_old_places_key_authority() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Moving summary settings")
        .await
        .base
        .id;
    let destination = desk.another_topic("New governing place").await;
    let token = key(
        &desk,
        json!({
            "target":{"kind":"topic","id":desk.topic,"reach":"subtree"},
            "resources":["document"],
            "allow":[{"operation":"DocumentSummaryOptionsSet","actions":["write"]}]
        }),
        chrono::Duration::hours(1),
    )
    .await;
    // Positive control: this real subtree key admits the actual document before it moves.
    let before = desk
        .world
        .app
        .call(DocumentSummaryOptionsSet, &[&document])
        .token(&token)
        .body(&body(Json::Null, 0))
        .send_bytes()
        .await;
    assert_eq!(
        before.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&before.raw)
    );
    assert!(
        summaries::get(
            &mut *desk.world.conn().await,
            SummaryTarget::Document { id: document },
            None
        )
        .await
        .unwrap()
        .is_none()
    );
    let mut holder = desk.world.db.pool.begin().await.unwrap();
    documents::containers::lock_scopes(&mut holder, &[desk.topic, destination])
        .await
        .unwrap();
    let request = desk
        .world
        .app
        .call(DocumentSummaryOptionsSet, &[&document])
        .token(&token)
        .body(&body(json!({"automatic":true,"output_chars":1200}), 0));
    let move_source = async {
        waiting_on(&desk, "%document_tree_lock%").await;
        // The native scope trigger is active. Change actual stored placement, not a mocked
        // path/projection; the waiting request must resolve that same source again.
        sqlx::query("UPDATE documents SET node_id=$2,parent_container_id=NULL WHERE id=$1")
            .bind(document)
            .bind(destination)
            .execute(&mut *holder)
            .await
            .unwrap();
        holder.commit().await.unwrap();
    };
    let (reply, ()) = tokio::join!(request.send_bytes(), move_source);
    assert_ne!(
        reply.status,
        StatusCode::OK,
        "an old-place key cannot create an override in the new place"
    );
    let source = documents::get(&mut *desk.world.conn().await, document)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.node_id, destination);
    assert!(
        summaries::get(
            &mut *desk.world.conn().await,
            SummaryTarget::Document { id: document },
            None
        )
        .await
        .unwrap()
        .is_none()
    );
}

#[tokio::test]
async fn raw_option_bodies_cannot_choose_a_writer_or_producer_and_invalid_budgets_make_no_cache() {
    let desk = Desk::new().await;
    let document = desk
        .make(&desk.ada, "Strict summary configuration")
        .await
        .base
        .id;
    let mut cases = Vec::new();
    for field in ["writer_id", "requested_by", "payer_id", "authority"] {
        let mut input = json!({"options":{"automatic":true},"expected_revision":0});
        input[field] = json!(new_id());
        cases.push(input);
    }
    cases.extend([
        json!({"options":{"automatic":true,"producer":{"kind":"model","provider_id":new_id()}},"expected_revision":0}),
        json!({"options":{"automatic":true,"output_chars":0},"expected_revision":0}),
        json!({"options":{"automatic":true,"context_tokens":0},"expected_revision":0}),
        json!({"options":{"automatic":true},"expected_revision":-1}),
    ]);
    for input in cases {
        let reply = desk
            .world
            .app
            .call(DocumentSummaryOptionsSet, &[&document])
            .token(&desk.ada.token)
            .raw_body(&input)
            .send_bytes()
            .await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
    }
    assert!(
        summaries::get(
            &mut *desk.world.conn().await,
            SummaryTarget::Document { id: document },
            None
        )
        .await
        .unwrap()
        .is_none()
    );
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM resource_summary_requests")
        .fetch_one(&desk.world.db.pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
}
