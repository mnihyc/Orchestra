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
    let active = generation(&world, c.acme, "active").await;
    let building = generation(&world, c.acme, "building").await;
    let secret = key(
        &world,
        c.root.id(),
        vec![rule("org", c.acme, "AdminSemanticSearchKeep", "manage")],
    )
    .await;
    let receipt = keep(&world, c.acme, active, &secret).await;
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["org_id"], json!(c.acme));
    assert!(receipt["request_id"].is_string());
    assert!(receipt.get("org").is_none() && receipt.get("active").is_none());
    let rows = orch_db::queries::semantic::generations(&mut *world.conn().await, c.acme)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, active);
    assert_ne!(rows[0].id, building);
    assert_eq!(json!(rows[0].administration_id), receipt["request_id"]);
    let chosen = orch_engine::semantic::choice(&mut *world.conn().await, c.acme)
        .await
        .unwrap();
    assert_eq!(chosen.tuning.chunk_tokens, 64);
    let named = chosen.embeddings.unwrap();
    assert_eq!(named.model_name, "retained-model");
    assert_eq!(named.query_prefix, "query: ");
    assert_eq!(named.document_prefix, "text: ");
    let work = Work::load(&mut *world.conn().await, c.acme)
        .await
        .unwrap()
        .unwrap();
    assert!(current(&world, &work.generation(&rows[0])).await);

    let full = key(
        &world,
        c.root.id(),
        vec![
            rule("org", c.acme, "AdminSemanticSearchKeep", "manage"),
            rule("org", c.acme, "AdminSemanticSearchKeep", "read"),
        ],
    )
    .await;
    let full = keep(&world, c.acme, active, &full).await;
    assert_eq!(full["org"]["id"], json!(c.acme));
    assert_eq!(full["active"]["id"], json!(active));
    assert!(full.get("projection").is_none());

    let old = rows[0].clone();
    let scoped_work = Work::load(&mut *world.conn().await, c.acme)
        .await
        .unwrap()
        .unwrap()
        .generation(&old);
    sqlx::query("DELETE FROM search_generations WHERE id=$1")
        .bind(active)
        .execute(&world.db.pool)
        .await
        .unwrap();
    generation(&world, c.acme, "active").await;
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
    world
        .set_at(
            orch_settings::Target::Node(c.acme),
            "search.semantic.enabled",
            json!(true),
        )
        .await;
    let active = generation(&world, c.acme, "active").await;
    let secret = key(
        &world,
        c.root.id(),
        vec![rule("org", c.acme, "AdminSemanticSearchKeep", "manage")],
    )
    .await;
    keep(&world, c.acme, active, &secret).await;
    let before: (String, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT state,updated_at FROM search_indexes WHERE org_id=$1")
            .bind(c.acme)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    world.advance(chrono::Duration::hours(2));
    world.app.app.engine.keep_indexing().await.unwrap();
    let after: (String, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT state,updated_at FROM search_indexes WHERE org_id=$1")
            .bind(c.acme)
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
    .bind(c.acme)
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
