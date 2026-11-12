//! The actual root, generation, retained operation and native index lock decide these acts.
use super::*;
use orch_api_contract::admin::{
    AdminSemanticSearchKeep, AdminSemanticSearchRebuild, SemanticSearchKeepBody,
};
use orch_engine::semantic::administration::Work;

async fn generation(world: &World, org: Id, state: &str) -> Id {
    let id = new_id();
    sqlx::query("INSERT INTO search_generations(id,org_id,provider_id,model_name,asked_dimensions,dimensions,query_prefix,document_prefix,chunking,state,created_at)
        VALUES($1,$2,$3,'retained-model',256,256,'query: ','text: ','parts-v1:64',$4,$5)")
        .bind(id).bind(org).bind(new_id()).bind(state).bind(world.now()).execute(&world.db.pool).await.unwrap();
    id
}
async fn keep(world: &World, org: Id, generation: Id, secret: &str) -> Json {
    value(
        world
            .app
            .call(AdminSemanticSearchKeep, &[&org])
            .token(secret)
            .body(&SemanticSearchKeepBody {
                generation_id: generation,
            })
            .send_bytes()
            .await,
    )
}
async fn current(world: &World, work: &Work) -> bool {
    let mut tx = world.app.app.db.begin().await.unwrap();
    let allowed = match work.checkpoint(&mut tx, world.app.app.clock.as_ref()).await {
        Ok(checked) => checked.before.check(world.app.app.clock.as_ref()).is_ok(),
        Err(_) => false,
    };
    tx.commit().await.unwrap();
    allowed
}

#[tokio::test]
async fn semantic_keep_uses_the_actual_generation_and_its_own_optional_read() {
    let world = World::new().await;
    let c = cast(&world).await;
    // Native system administration does not lend this key private OrgManage.
    // Keep the unowned-root refusal, then use an actual owned root for positives.
    let unowned_generation = generation(&world, c.acme, "active").await;
    let unowned_key = key(
        &world,
        c.root.id(),
        vec![rule("org", c.acme, "AdminSemanticSearchKeep", "manage")],
    )
    .await;
    let refused = world
        .app
        .call(AdminSemanticSearchKeep, &[&c.acme])
        .token(&unowned_key)
        .body(&SemanticSearchKeepBody {
            generation_id: unowned_generation,
        })
        .send_bytes()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "org.manage");
    assert_eq!(
        orch_db::queries::semantic::generations(&mut *world.conn().await, c.acme)
            .await
            .unwrap()[0]
            .id,
        unowned_generation,
        "a refused Keep preserves the existing generation",
    );
    let org = places::make_org(&world, &c.root, "Owned semantic root", "owned-semantic")
        .await
        .base
        .id;
    let active = generation(&world, org, "active").await;
    let building = generation(&world, org, "building").await;
    let secret = key(
        &world,
        c.root.id(),
        vec![rule("org", org, "AdminSemanticSearchKeep", "manage")],
    )
    .await;
    let receipt = keep(&world, org, active, &secret).await;
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["org_id"], json!(org));
    assert!(receipt["request_id"].is_string());
    assert!(receipt.get("org").is_none() && receipt.get("active").is_none());
    let rows = orch_db::queries::semantic::generations(&mut *world.conn().await, org)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, active);
    assert_ne!(rows[0].id, building);
    assert_eq!(json!(rows[0].administration_id), receipt["request_id"]);
    let chosen = orch_engine::semantic::choice(&mut *world.conn().await, org)
        .await
        .unwrap();
    assert_eq!(chosen.tuning.chunk_tokens, 64);
    let named = chosen.embeddings.unwrap();
    assert_eq!(named.model_name, "retained-model");
    assert_eq!(named.query_prefix, "query: ");
    assert_eq!(named.document_prefix, "text: ");
    let work = Work::load(&mut *world.conn().await, org)
        .await
        .unwrap()
        .unwrap();
    assert!(current(&world, &work.generation(&rows[0])).await);

    let full = key(
        &world,
        c.root.id(),
        vec![
            rule("org", org, "AdminSemanticSearchKeep", "manage"),
            rule("org", org, "AdminSemanticSearchKeep", "read"),
        ],
    )
    .await;
    let full = keep(&world, org, active, &full).await;
    assert_eq!(full["org"]["id"], json!(org));
    assert_eq!(full["active"]["id"], json!(active));
    assert!(full.get("projection").is_none());

    assert!(
        !current(&world, &work.generation(&rows[0])).await,
        "the second accepted request cannot lend its origin to the first request's work",
    );
    let old = orch_db::queries::semantic::generations(&mut *world.conn().await, org)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == active)
        .expect("the retained generation carries the second request's actual association");
    let scoped_work = Work::load(&mut *world.conn().await, org)
        .await
        .unwrap()
        .unwrap()
        .generation(&old);
    assert!(current(&world, &scoped_work).await);
    sqlx::query("DELETE FROM search_generations WHERE id=$1")
        .bind(active)
        .execute(&world.db.pool)
        .await
        .unwrap();
    generation(&world, org, "active").await;
    assert!(
        !current(&world, &scoped_work).await,
        "another generation at the same root cannot validate old work"
    );
}

#[tokio::test]
async fn semantic_rebuild_preserves_manage_and_does_not_borrow_another_operations_read() {
    let world = World::new().await;
    let c = cast(&world).await;
    let active = generation(&world, c.acme, "active").await;
    let wrong = key(
        &world,
        c.root.id(),
        vec![rule("org", c.acme, "AdminSemanticSearchRebuild", "read")],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(AdminSemanticSearchRebuild, &[&c.acme])
            .token(&wrong)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        orch_db::queries::semantic::generations(&mut *world.conn().await, c.acme)
            .await
            .unwrap()[0]
            .id,
        active
    );
    let other = key(
        &world,
        c.root.id(),
        vec![rule(
            "org",
            new_id(),
            "AdminSemanticSearchRebuild",
            "manage",
        )],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(AdminSemanticSearchRebuild, &[&c.acme])
            .token(&other)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let write = key(
        &world,
        c.root.id(),
        vec![
            rule("org", c.acme, "AdminSemanticSearchRebuild", "manage"),
            rule("org", c.acme, "AdminSemanticSearchKeep", "read"),
        ],
    )
    .await;
    let receipt = value(
        world
            .app
            .call(AdminSemanticSearchRebuild, &[&c.acme])
            .token(&write)
            .send_bytes()
            .await,
    );
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["org_id"], json!(c.acme));
    assert!(
        orch_db::queries::semantic::generations(&mut *world.conn().await, c.acme)
            .await
            .unwrap()
            .is_empty()
    );
    let work = Work::load(&mut *world.conn().await, c.acme)
        .await
        .unwrap()
        .unwrap();
    assert!(current(&world, &work).await);
    let reader = key(
        &world,
        c.root.id(),
        vec![
            rule("org", c.acme, "AdminSemanticSearchRebuild", "manage"),
            rule("org", c.acme, "AdminSemanticSearchRebuild", "read"),
        ],
    )
    .await;
    let full = value(
        world
            .app
            .call(AdminSemanticSearchRebuild, &[&c.acme])
            .token(&reader)
            .send_bytes()
            .await,
    );
    assert_eq!(full["org"]["id"], json!(c.acme));
    assert!(
        !current(&world, &work).await,
        "a new accepted request cannot validate the old request's publication"
    );
    let ordinary = value(
        world
            .app
            .call(AdminSemanticSearchRebuild, &[&c.acme])
            .token(&c.root.token)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["org"]["id"], json!(c.acme));
    let outsider = key(
        &world,
        c.boss.id(),
        vec![rule("org", c.acme, "AdminSemanticSearchRebuild", "manage")],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(AdminSemanticSearchRebuild, &[&c.acme])
            .token(&outsider)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn semantic_native_work_keeps_its_original_key_current_scope_and_system_standing() {
    for change in ["revoke", "scope", "standing", "expiry"] {
        let world = World::new().await;
        let c = cast(&world).await;
        let secret = key(
            &world,
            c.root.id(),
            vec![rule("org", c.acme, "AdminSemanticSearchRebuild", "manage")],
        )
        .await;
        value(
            world
                .app
                .call(AdminSemanticSearchRebuild, &[&c.acme])
                .token(&secret)
                .send_bytes()
                .await,
        );
        let work = Work::load(&mut *world.conn().await, c.acme)
            .await
            .unwrap()
            .unwrap();
        assert!(current(&world, &work).await);
        match change {
            "revoke" => {
                sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE token_hash=$1")
                    .bind(token_hash(&secret))
                    .bind(world.now())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1")
                    .bind(token_hash(&secret))
                    .bind(json!({"rules":[]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "standing" => {
                sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
                    .bind(c.root.id())
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            _ => world.advance(chrono::Duration::hours(2)),
        }
        assert!(
            !current(&world, &work).await,
            "the original {change} refuses later native work"
        );
        assert!(
            Work::for_choice(
                &mut *world.conn().await,
                &orch_engine::semantic::choice(&mut *world.conn().await, c.acme)
                    .await
                    .unwrap()
            )
            .await
            .unwrap()
            .is_some(),
            "revocation alone never reclassifies this request as ordinary work"
        );
    }
}

#[tokio::test]
async fn semantic_native_work_rechecks_after_the_actual_index_lock_wait() {
    for expire in [false, true] {
        let world = World::new().await;
        let c = cast(&world).await;
        let secret = key(
            &world,
            c.root.id(),
            vec![rule("org", c.acme, "AdminSemanticSearchRebuild", "manage")],
        )
        .await;
        value(
            world
                .app
                .call(AdminSemanticSearchRebuild, &[&c.acme])
                .token(&secret)
                .send_bytes()
                .await,
        );
        let work = Work::load(&mut *world.conn().await, c.acme)
            .await
            .unwrap()
            .unwrap();
        let mut blocker = world.app.app.db.begin().await.unwrap();
        sqlx::query("SELECT org_id FROM search_indexes WHERE org_id=$1 FOR UPDATE")
            .bind(c.acme)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let db = world.app.app.db.clone();
        let clock = world.app.app.clock.clone();
        let waiting = tokio::spawn(async move {
            let mut tx = db.begin().await.unwrap();
            let outcome = match work.checkpoint(&mut tx, clock.as_ref()).await {
                Ok(checked) => checked.before.check(clock.as_ref()).is_ok(),
                Err(_) => false,
            };
            tx.commit().await.unwrap();
            outcome
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                assert!(!waiting.is_finished(), "the real index lock must be reached");
                let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                    AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE 'SELECT org_id FROM search_indexes%FOR UPDATE%')")
                    .bind(pid).fetch_one(&world.db.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap();
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let allowed = tokio::time::timeout(std::time::Duration::from_secs(30), waiting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(allowed, !expire);
    }
}

#[tokio::test]
async fn semantic_indexer_does_not_claim_or_dispatch_work_after_its_original_key_expires() {
    let world = World::new().await;
    let c = cast(&world).await;
    // This positive reaches the native effect with real OrgManage; only its
    // original credential's later expiry may stop the indexer.
    let org = places::make_org(&world, &c.root, "Owned semantic root", "owned-semantic")
        .await
        .base
        .id;
    world
        .set_at(
            orch_settings::Target::Node(org),
            "search.semantic.enabled",
            json!(true),
        )
        .await;
    let active = generation(&world, org, "active").await;
    let secret = key(
        &world,
        c.root.id(),
        vec![rule("org", org, "AdminSemanticSearchKeep", "manage")],
    )
    .await;
    keep(&world, org, active, &secret).await;
    let before: (String, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT state,updated_at FROM search_indexes WHERE org_id=$1")
            .bind(org)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    world.advance(chrono::Duration::hours(2));
    world.app.app.engine.keep_indexing().await.unwrap();
    let after: (String, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT state,updated_at FROM search_indexes WHERE org_id=$1")
            .bind(org)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        after, before,
        "the actual native indexer must stop before its claim effect"
    );
    let calls: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM summary_calls WHERE node_id=$1 AND purpose='embeddings'",
    )
    .bind(org)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(calls, 0);
}

#[tokio::test]
async fn semantic_setting_first_effect_runs_after_validation_and_before_any_native_write() {
    use orch_settings::write::{Write, WriteError, write_before};
    let world = World::new().await;
    let c = cast(&world).await;
    let origin = orch_db::audit::Origin::default();
    let request = Write {
        actor: orch_db::audit::Actor::User(c.root.id()),
        target: orch_settings::Target::Node(c.acme),
        key: "search.semantic.enabled",
        value: json!(true),
        locked: None,
        expected_revision: None,
        may_lock: false,
        origin: &origin,
    };
    let mut tx = world.app.app.db.begin().await.unwrap();
    let denied = write_before(&mut tx, &request, world.now(), || {
        Err(WriteError::OperationRefused)
    })
    .await;
    assert!(matches!(denied, Err(WriteError::OperationRefused)));
    tx.commit().await.unwrap();
    let saved: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM settings WHERE node_id=$1 AND key='search.semantic.enabled')",
    )
    .bind(c.acme)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert!(!saved);
    let mut tx = world.app.app.db.begin().await.unwrap();
    assert!(
        write_before(&mut tx, &request, world.now(), || Ok(()))
            .await
            .unwrap()
            .changed
    );
    tx.commit().await.unwrap();
    let mut tx = world.app.app.db.begin().await.unwrap();
    assert!(
        !write_before(&mut tx, &request, world.now(), || panic!(
            "an unchanged native setting has no first effect"
        ))
        .await
        .unwrap()
        .changed
    );
    tx.commit().await.unwrap();
}

async fn header_call(
    world: &World,
    org: Id,
    kept: Option<Id>,
    secret: &str,
    act: Id,
) -> orch_api_core::testing::RawReply {
    if let Some(generation_id) = kept {
        let call = world
            .app
            .call(AdminSemanticSearchKeep, &[&org])
            .token(secret)
            .header("Idempotency-Key", &act.to_string())
            .body(&SemanticSearchKeepBody { generation_id });
        call.send_bytes().await
    } else {
        let call = world
            .app
            .call(AdminSemanticSearchRebuild, &[&org])
            .token(secret)
            .header("Idempotency-Key", &act.to_string());
        call.send_bytes().await
    }
}
async fn header_administration(keep: bool) {
    let world = World::new().await;
    let c = cast(&world).await;
    let org = places::make_org(&world, &c.root, "One index request", "one-index-request")
        .await
        .base
        .id;
    let active = generation(&world, org, "active").await;
    let operation = if keep {
        "AdminSemanticSearchKeep"
    } else {
        "AdminSemanticSearchRebuild"
    };
    let kept = keep.then_some(active);
    let writer = key(
        &world,
        c.root.id(),
        vec![rule("org", org, operation, "manage")],
    )
    .await;
    let act = new_id();
    let first = value(header_call(&world, org, kept, &writer, act).await);
    assert_eq!(first["projection"], "receipt");
    let request: Id = serde_json::from_value(first["request_id"].clone()).unwrap();
    let before: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(c.root.id())
    .bind(act.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        before.0["outcome"],
        json!({"kind":"semantic_administration","id":request,"data":{"org":org}})
    );
    let native: (Json, String) =
        sqlx::query_as("SELECT administration,xmin::text FROM search_indexes WHERE org_id=$1")
            .bind(org)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        Work::load(&mut *world.conn().await, org)
            .await
            .unwrap()
            .unwrap()
            .request_id(),
        request
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE token_hash=$1")
        .bind(token_hash(&writer))
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let reader = key(
        &world,
        c.root.id(),
        vec![rule("org", org, operation, "read")],
    )
    .await;
    let full = value(header_call(&world, org, kept, &reader, act).await);
    assert!(full.get("projection").is_none());
    assert_eq!(full["org"]["id"], json!(org));
    if keep {
        assert_eq!(full["active"]["id"], json!(active));
    }
    let neighbour = key(
        &world,
        c.root.id(),
        vec![rule("org", org, "AdminSemanticSearchGet", "read")],
    )
    .await;
    assert_eq!(
        value(header_call(&world, org, kept, &neighbour, act).await),
        first
    );
    let after: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(c.root.id())
    .bind(act.to_string())
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(after, before);
    let unchanged: (Json, String) =
        sqlx::query_as("SELECT administration,xmin::text FROM search_indexes WHERE org_id=$1")
            .bind(org)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(unchanged, native);
    assert_eq!(
        Work::load(&mut *world.conn().await, org)
            .await
            .unwrap()
            .unwrap()
            .request_id(),
        request
    );
}
#[tokio::test]
async fn a_rebuild_header_keeps_one_native_request_and_reads_it_under_only_the_new_read_key() {
    header_administration(false).await;
}
#[tokio::test]
async fn a_keep_header_keeps_one_native_request_and_does_not_drop_or_retain_another_generation() {
    header_administration(true).await;
}
