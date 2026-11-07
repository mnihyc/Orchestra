//! Actual report scopes, native aggregates and current publication authority.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::storage::{StorageGet, StorageKind, StorageQuery};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use orch_driver_contract::resources::{Resource, ResourceInventory, ResourceKind, ResourceRef};
use orch_types::{Json, new_id};

struct Key {
    id: Id,
    secret: String,
}
fn rule(operation: &str, action: &str, kind: &str, id: Id) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":operation,"actions":[action]}]})
}
async fn key(world: &World, who: &Who, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules": rules})).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: who.id(),
            name: "Native storage report",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    Key { id, secret }
}
async fn inventory(world: &World, host: Id, label: &str, bytes: i64) {
    let inventory = ResourceInventory {
        observed_at: Time::new(world.now()),
        unavailable: Vec::new(),
        resources: vec![Resource {
            resource: ResourceRef {
                kind: ResourceKind::Image,
                id: label.into(),
            },
            name: label.into(),
            size_bytes: Some(bytes),
            labels: Default::default(),
            depends_on: Vec::new(),
            aliases: Vec::new(),
            last_used_at: Some(Time::new(world.now() - chrono::Duration::days(8))),
            busy: false,
        }],
    };
    sqlx::query("UPDATE hosts SET resource_inventory=$2 WHERE id=$1")
        .bind(host)
        .bind(json!(inventory))
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(host)
    .execute(&world.db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn exact_storage_manage_uses_the_stored_host_and_keeps_reference_read_independent() {
    let world = World::new().await;
    let c = company(&world).await;
    let host = a_host(&world, &c.boss, Some(c.acme), "Private storage host").await;
    inventory(&world, host, "retained-image", 17).await;
    let query = StorageQuery {
        scope: Some(format!("host:{host}")),
        ..Default::default()
    };
    for rules in [
        vec![rule("HostGet", "read", "host", host)],
        vec![rule("StorageGet", "read", "host", host)],
        vec![rule("StorageGet", "manage", "machine", host)],
    ] {
        let k = key(&world, &c.boss, rules).await;
        assert_eq!(
            world
                .app
                .call(StorageGet, &[])
                .token(&k.secret)
                .query(&query)
                .send()
                .await
                .status
                .as_u16(),
            403
        );
    }
    let k = key(
        &world,
        &c.boss,
        vec![rule("StorageGet", "manage", "host", host)],
    )
    .await;
    let report = world
        .app
        .call(StorageGet, &[])
        .token(&k.secret)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!((report.totals.items, report.totals.bytes), (1, 17));
    assert_eq!(report.items[0].name, "retained-image");
    assert!(report.name.is_empty());
    assert!(
        report.items[0]
            .host
            .as_ref()
            .is_none_or(|host| host.name.is_empty() && host.address.is_empty())
    );
    assert!(
        report.items[0]
            .place
            .as_ref()
            .is_none_or(|place| place.name.is_none() && place.redacted)
    );
    assert!(report.items[0].owner.is_none());
    let rich = key(
        &world,
        &c.boss,
        vec![
            rule("StorageGet", "manage", "host", host),
            rule("StorageGet", "read", "host", host),
            rule("StorageGet", "read", "org", c.acme),
            rule("StorageGet", "read", "account", c.boss.id()),
        ],
    )
    .await;
    let native = world
        .app
        .call(StorageGet, &[])
        .token(&c.boss.token)
        .query(&query)
        .send()
        .await
        .ok();
    let scoped = world
        .app
        .call(StorageGet, &[])
        .token(&rich.secret)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(
        scoped, native,
        "the same native formatter keeps all ordinary report fields"
    );
    let viewer = key(
        &world,
        &c.viewer,
        vec![rule("StorageGet", "manage", "host", host)],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(StorageGet, &[])
            .token(&viewer.secret)
            .query(&query)
            .send()
            .await
            .status
            .as_u16(),
        404,
        "scope cannot grant native Host Manage"
    );
}

#[tokio::test]
async fn account_namespace_never_lends_a_hosts_records_or_counts_and_filters_precede_the_window() {
    let world = World::new().await;
    let c = company(&world).await;
    let a = a_host(&world, &c.boss, Some(c.acme), "Included host").await;
    let b = a_host(&world, &c.boss, Some(c.acme), "Excluded host").await;
    inventory(&world, a, "included-private-image", 11).await;
    inventory(&world, b, "excluded-private-image", 101).await;
    let query = StorageQuery {
        kind: Some(StorageKind::Image),
        page: orch_api_contract::common::PageQuery {
            limit: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    let empty = key(
        &world,
        &c.boss,
        vec![rule("StorageGet", "manage", "account", c.boss.id())],
    )
    .await;
    let empty = world
        .app
        .call(StorageGet, &[])
        .token(&empty.secret)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!((empty.totals.items, empty.totals.bytes), (0, 0));
    assert!(empty.items.is_empty());
    let k = key(
        &world,
        &c.boss,
        vec![
            rule("StorageGet", "manage", "account", c.boss.id()),
            rule("StorageGet", "manage", "host", a),
        ],
    )
    .await;
    let report = world
        .app
        .call(StorageGet, &[])
        .token(&k.secret)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!((report.totals.items, report.totals.bytes), (1, 11));
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].name, "included-private-image");
    assert!(report.next_cursor.is_none());
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("excluded-private-image")
    );
    let ordinary = world
        .app
        .call(StorageGet, &[])
        .token(&c.boss.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!((ordinary.totals.items, ordinary.totals.bytes), (2, 112));
    assert!(ordinary.next_cursor.is_some());
}

#[tokio::test]
async fn subtree_storage_uses_current_native_placement_and_system_scope_keeps_its_admin_gate() {
    let world = World::new().await;
    let c = company(&world).await;
    let other = places::make_org(&world, &c.boss, "Other home", "other-storage")
        .await
        .base
        .id;
    let host = a_host(&world, &c.boss, Some(c.acme), "Moving storage").await;
    inventory(&world, host, "moving-image", 13).await;
    let k = key(&world, &c.boss, vec![json!({"target":{"kind":"org","id":c.acme,"reach":"subtree"},"resources":["org","host"],"allow":[{"operation":"StorageGet","actions":["manage"]}]})]).await;
    let query = StorageQuery {
        scope: Some(format!("node:{}", c.acme)),
        ..Default::default()
    };
    assert_eq!(
        world
            .app
            .call(StorageGet, &[])
            .token(&k.secret)
            .query(&query)
            .send()
            .await
            .ok()
            .totals
            .items,
        1
    );
    sqlx::query("UPDATE hosts SET home_id=$2 WHERE id=$1")
        .bind(host)
        .bind(other)
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        world
            .app
            .call(StorageGet, &[])
            .token(&k.secret)
            .query(&query)
            .send()
            .await
            .ok()
            .totals
            .items,
        0
    );
    let system = key(&world, &c.boss, vec![json!({"target":{"kind":"system","reach":"self"},"resources":["system"],"allow":[{"operation":"StorageGet","actions":["manage"]}]})]).await;
    assert_eq!(
        world
            .app
            .call(StorageGet, &[])
            .token(&system.secret)
            .query(&StorageQuery {
                scope: Some("system".into()),
                ..Default::default()
            })
            .send()
            .await
            .status
            .as_u16(),
        404
    );
}

#[tokio::test]
async fn an_actual_final_activity_wait_rechecks_original_key_and_native_storage_authority() {
    for mode in ["unchanged", "expired", "scope", "native-manage"] {
        let world = World::new().await;
        let c = company(&world).await;
        let host = a_host(&world, &c.boss, Some(c.acme), "Held private storage").await;
        inventory(&world, host, "held-private-image", 19).await;
        let grant = name_person(&world, "host", host, "host.manage", c.viewer.id()).await;
        let k = key(
            &world,
            &c.viewer,
            vec![rule("StorageGet", "manage", "host", host)],
        )
        .await;
        let mut lock = world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *lock)
            .await
            .unwrap();
        let app = world.app.clone();
        let secret = k.secret;
        let request = tokio::spawn(async move {
            let call = app
                .call(StorageGet, &[])
                .token(&secret)
                .query(&StorageQuery {
                    scope: Some(format!("host:{host}")),
                    ..Default::default()
                });
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {loop {
            assert!(!request.is_finished(), "the report must reach the actual activity INSERT");
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')").bind(blocker).fetch_one(&world.db.pool).await.unwrap();
            if waiting {break;} tokio::task::yield_now().await;
        }}).await.expect("the native publication wait must be observed");
        match mode {
            "expired" => world.advance(chrono::Duration::hours(2)),
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(k.id)
                    .bind(json!({"rules":[]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "native-manage" => {
                sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE id=$1")
                    .bind(grant)
                    .bind(world.now())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            _ => {}
        }
        lock.rollback().await.unwrap();
        let reply = request.await.unwrap();
        if mode == "unchanged" {
            assert!(
                reply.status.is_success(),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert!(String::from_utf8_lossy(&reply.raw).contains("held-private-image"));
        } else {
            assert!(!reply.status.is_success());
            assert!(!String::from_utf8_lossy(&reply.raw).contains("held-private-image"));
        }
    }
}

fn cleanup_rule(operation: &str, host: Id, read: bool) -> Json {
    json!({"target":{"kind":"host","id":host,"reach":"self"},"resources":["host","machine"],"allow":[{"operation":operation,"actions":if read {vec!["manage","read"]} else {vec!["manage"]}}]})
}
async fn cleanup_request(
    world: &World,
    who: &Who,
    host: Id,
) -> orch_api_contract::storage::CleanupRequest {
    let report = world
        .app
        .call(StorageGet, &[])
        .token(&who.token)
        .query(&StorageQuery {
            scope: Some(format!("host:{host}")),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    orch_api_contract::storage::CleanupRequest {
        scope: format!("host:{host}"),
        resource: None,
        items: report.items.iter().map(|row| row.id.clone()).collect(),
    }
}

#[tokio::test]
async fn cleanup_preview_binds_actual_destinations_and_returns_an_usable_manage_only_receipt() {
    use orch_api_contract::storage::{CleanupPlanResponse, StorageCleanupPreflight};
    let world = World::new().await;
    let c = company(&world).await;
    let host = a_host(&world, &c.boss, Some(c.acme), "Private cleanup host").await;
    inventory(&world, host, "cleanup-image-id", 23).await;
    sqlx::query("UPDATE hosts SET resource_inventory=jsonb_set(resource_inventory,'{resources,0,name}',to_jsonb('private-cleanup-image'::text)) WHERE id=$1").bind(host).execute(&world.db.pool).await.unwrap();
    let request = cleanup_request(&world, &c.boss, host).await;
    for rules in [
        vec![cleanup_rule("StorageGet", host, false)],
        vec![rule("StorageCleanupPreflight", "read", "host", host)],
        vec![rule("StorageCleanupPreflight", "manage", "host", host)],
    ] {
        let k = key(&world, &c.boss, rules).await;
        assert_eq!(
            world
                .app
                .call(StorageCleanupPreflight, &[])
                .token(&k.secret)
                .body(&request)
                .send()
                .await
                .status
                .as_u16(),
            403
        );
    }
    let k = key(
        &world,
        &c.boss,
        vec![cleanup_rule("StorageCleanupPreflight", host, false)],
    )
    .await;
    let response = world
        .app
        .call(StorageCleanupPreflight, &[])
        .token(&k.secret)
        .body(&request)
        .send()
        .await
        .ok();
    let CleanupPlanResponse::Receipt(receipt) = response else {
        panic!("Manage cannot disclose the existing storage preview");
    };
    assert_eq!(receipt.items.len(), 1);
    assert_eq!(receipt.items[0].id, request.items[0]);
    assert!(
        receipt.items[0]
            .choices
            .contains(&orch_api_contract::storage::CleanupChoice::Remove)
    );
    let encoded = serde_json::to_value(&receipt).unwrap();
    assert!(encoded.get("confirmation_name").is_none());
    assert!(encoded.get("people").is_none());
    assert!(encoded["items"][0].get("record").is_none());
    assert!(!encoded.to_string().contains("Private cleanup host"));
    assert!(!encoded.to_string().contains("private-cleanup-image"));
    let saved = orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, receipt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.created_by, c.boss.id());
    assert_eq!(saved.plan["targets"].as_object().unwrap().len(), 1);
    let origin: orch_authz::CapturedOperation =
        serde_json::from_value(saved.plan["operation"].clone()).unwrap();
    assert_eq!(origin.operation(), "StorageCleanupPreflight");
    assert_eq!(origin.user(), c.boss.id());
    let k = key(
        &world,
        &c.boss,
        vec![
            cleanup_rule("StorageCleanupPreflight", host, true),
            rule("StorageCleanupPreflight", "read", "org", c.acme),
            rule("StorageCleanupPreflight", "read", "account", c.boss.id()),
        ],
    )
    .await;
    let rich = world
        .app
        .call(StorageCleanupPreflight, &[])
        .token(&k.secret)
        .body(&request)
        .send()
        .await
        .ok()
        .into_full()
        .expect("the operation's own complete Read admits its native preview");
    let ordinary = world
        .app
        .call(StorageCleanupPreflight, &[])
        .token(&c.boss.token)
        .body(&request)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(rich.items, ordinary.items);
    assert_eq!(rich.confirmation_name, ordinary.confirmation_name);
    assert_eq!(rich.people, ordinary.people);
}

#[tokio::test]
async fn cleanup_preview_uses_the_explicit_selected_records_without_borrowing_other_managed_candidates()
 {
    use orch_api_contract::storage::{CleanupPlanResponse, StorageCleanupPreflight};
    let world = World::new().await;
    let c = company(&world).await;
    let a = a_host(&world, &c.boss, Some(c.acme), "Selected cleanup").await;
    let b = a_host(&world, &c.boss, Some(c.acme), "Other cleanup").await;
    inventory(&world, a, "selected-cleanup-image", 29).await;
    inventory(&world, b, "excluded-cleanup-image", 31).await;
    let mut request = cleanup_request(&world, &c.boss, a).await;
    request.scope = "me".into();
    let k = key(
        &world,
        &c.boss,
        vec![
            rule("StorageCleanupPreflight", "manage", "account", c.boss.id()),
            cleanup_rule("StorageCleanupPreflight", a, false),
        ],
    )
    .await;
    let response = world
        .app
        .call(StorageCleanupPreflight, &[])
        .token(&k.secret)
        .body(&request)
        .send()
        .await
        .ok();
    let CleanupPlanResponse::Receipt(receipt) = response else {
        panic!("metadata needs Read");
    };
    assert_eq!(receipt.items.len(), 1);
    assert_eq!(receipt.items[0].id, request.items[0]);
    let saved = orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, receipt.id)
        .await
        .unwrap()
        .unwrap();
    let sources: orch_engine::jobs::cleanup::sources::Sources =
        serde_json::from_value(saved.plan["sources"].clone()).unwrap();
    assert_eq!(sources.records.len(), 1);
    assert_eq!(sources.records[0].object.id, Some(a));
    assert!(
        sources
            .physical
            .iter()
            .all(|reference| reference.object.id == Some(a)),
        "the canonical machine facade is the selected Host, never the other Host"
    );
}

#[tokio::test]
async fn cleanup_preview_expires_at_the_actual_save_wait_and_rolls_back_the_entire_new_plan() {
    use orch_api_contract::storage::StorageCleanupPreflight;
    for expired in [false, true] {
        let world = World::new().await;
        let c = company(&world).await;
        let host = a_host(&world, &c.boss, Some(c.acme), "Waiting cleanup").await;
        inventory(&world, host, "waiting-image", 37).await;
        let request = cleanup_request(&world, &c.boss, host).await;
        let k = key(
            &world,
            &c.boss,
            vec![cleanup_rule("StorageCleanupPreflight", host, false)],
        )
        .await;
        let before: i64 =
            sqlx::query_scalar("SELECT count(*) FROM preflights WHERE kind='storage_cleanup'")
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        let mut lock = world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE preflights IN SHARE MODE")
            .execute(&mut *lock)
            .await
            .unwrap();
        let app = world.app.clone();
        let secret = k.secret;
        let pending = tokio::spawn(async move {
            let call = app
                .call(StorageCleanupPreflight, &[])
                .token(&secret)
                .body(&request);
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{
            assert!(!pending.is_finished(),"the native preview must reach its actual save");
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO preflights%')").bind(blocker).fetch_one(&world.db.pool).await.unwrap();
            if waiting{break;}tokio::task::yield_now().await;
        }}).await.expect("the actual preflight INSERT wait must be observed");
        if expired {
            world.advance(chrono::Duration::hours(2));
        }
        lock.rollback().await.unwrap();
        let reply = pending.await.unwrap();
        assert_eq!(
            reply.status.is_success(),
            !expired,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        let after: i64 =
            sqlx::query_scalar("SELECT count(*) FROM preflights WHERE kind='storage_cleanup'")
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(after, before + i64::from(!expired));
    }
}

#[tokio::test]
async fn a_completed_preview_keeps_one_receipt_when_its_response_read_ends_during_real_publication()
{
    use orch_api_contract::storage::{CleanupPlanResponse, StorageCleanupPreflight};
    for lose_read in [false, true] {
        let world = World::new().await;
        let c = company(&world).await;
        let host = a_host(&world, &c.boss, Some(c.acme), "Private late preview").await;
        inventory(&world, host, "late-preview-id", 41).await;
        let request = cleanup_request(&world, &c.boss, host).await;
        let k = key(
            &world,
            &c.boss,
            vec![
                cleanup_rule("StorageCleanupPreflight", host, true),
                rule("StorageCleanupPreflight", "read", "org", c.acme),
                rule("StorageCleanupPreflight", "read", "account", c.boss.id()),
            ],
        )
        .await;
        let mut lock = world.db.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *lock)
            .await
            .unwrap();
        let app = world.app.clone();
        let secret = k.secret;
        let pending = tokio::spawn(async move {
            let call = app
                .call(StorageCleanupPreflight, &[])
                .token(&secret)
                .body(&request);
            call.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{
            assert!(!pending.is_finished(),"the accepted preview must reach the real publication wait");
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')").bind(blocker).fetch_one(&world.db.pool).await.unwrap();
            if waiting{break;}tokio::task::yield_now().await;
        }}).await.expect("the real response activity wait must be observed");
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM preflights WHERE kind='storage_cleanup'")
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        if lose_read {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(k.id)
                .bind(json!({"rules":[cleanup_rule("StorageCleanupPreflight",host,false)]}))
                .execute(&world.db.pool)
                .await
                .unwrap();
        }
        lock.rollback().await.unwrap();
        let reply = pending.await.unwrap();
        assert!(
            reply.status.is_success(),
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        let response: CleanupPlanResponse = serde_json::from_slice(&reply.raw).unwrap();
        if lose_read {
            assert!(matches!(response, CleanupPlanResponse::Receipt(_)));
            assert!(!String::from_utf8_lossy(&reply.raw).contains("Private late preview"));
        } else {
            assert_eq!(
                response.into_full().unwrap().confirmation_name,
                "Private late preview"
            );
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM preflights WHERE kind='storage_cleanup'")
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(count, 1, "response refresh cannot replay the native save");
    }
}
