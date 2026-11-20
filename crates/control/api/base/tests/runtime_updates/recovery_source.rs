//! A reviewed provider recovery retains the actual HTTP key at its native boundaries.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::workspace::JobCreationResponse;
use orch_api_core::middleware::authentication::{new_api_token, token_hash};
use orch_db::queries::{identity, jobs};

#[tokio::test]
async fn a_lost_provider_recovery_keeps_its_actual_confirmation_key_after_the_http_reply() {
    recovery_source(false, false).await;
}

#[tokio::test]
async fn a_reviewed_host_pool_keeps_its_intent_and_original_key_without_drawing_during_review() {
    recovery_source(true, false).await;
}

#[tokio::test]
async fn a_different_provider_manager_keeps_its_own_key_and_pool_placement_authority() {
    recovery_source(true, true).await;
}

async fn recovery_source(pooled: bool, managed: bool) {
    let world = World::new().await;
    let team = team(&world).await;
    let actor = if managed { &team.c.boss } else { &team.c.admin };
    world.app.app.engine.stop();
    let mut conn = world.conn().await;
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1")
        .bind(team.host)
        .bind(json!({"docker":{"name":"docker","title":"Docker","spec_schemas":{"provider":{}},"sources":["image"]}}))
        .execute(&mut *conn).await.unwrap();
    let template: Id = sqlx::query_scalar("SELECT template_id FROM runtimes WHERE provider_id=$1")
        .bind(team.provider)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE runtimes SET state='removed',lost_at=$2 WHERE provider_id=$1")
        .bind(team.provider)
        .bind(world.now())
        .execute(&mut *conn)
        .await
        .unwrap();
    let mut selected_pool = None;
    let target = if pooled {
        let pool = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            "pool",
            Some(team.c.admin.id()),
            Some(team.c.acme),
            "Recovery host pool",
        )
        .await
        .unwrap();
        selected_pool = Some(pool);
        orch_authz::ownership::start_resource(
            &mut conn,
            orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Pool,
                id: pool,
            },
            team.c.admin.id(),
            orch_authz::ownership::NewAccess::OnlyMe,
            Time::new(world.now()),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE pools SET kind='host',definition=$2 WHERE id=$1").bind(pool)
            .bind(json!({"kind":"host","enabled":true,"policy":{"type":"round_robin"},"lend_members":true,"members":[],"config":{}}))
            .execute(&mut *conn).await.unwrap();
        sqlx::query(
            "INSERT INTO pool_members(pool_id,id,position,rule,enabled) VALUES($1,$2,0,$3,true)",
        )
        .bind(pool)
        .bind(new_id())
        .bind(json!({"type":"single","resource":{"kind":"host","id":team.host}}))
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO pool_lending(id,resource_kind,resource_id,giver_id,matcher,options) VALUES($1,'host',$2,$3,$4,'{}')")
            .bind(new_id()).bind(team.host).bind(team.c.admin.id()).bind(json!({"kind":"pool","pool_id":pool})).execute(&mut *conn).await.unwrap();
        orch_provider_contract::pools::Target::Pool {
            id: pool,
            kind: orch_provider_contract::pools::Kind::Host,
        }
    } else {
        orch_provider_contract::pools::Target::host(team.host)
    };
    sqlx::query("UPDATE providers SET runtime_unavailable='Its runtime was declared lost',status='offline',runtime_config=runtime_config||jsonb_build_object('runtime.template_id',$2::uuid::text,'runtime.host_id',$3::jsonb) WHERE id=$1")
        .bind(team.provider).bind(template).bind(json!(&target))
        .execute(&mut *conn).await.unwrap();
    let key_id = new_id();
    let secret = new_api_token(key_id);
    let scope: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"provider","id":team.provider,"reach":"self"},
        "resources":["provider"],"allow":[
            {"operation":"ProviderRuntimeUpdatePreflight","actions":["manage"]},
            {"operation":"ProviderRuntimeUpdateCreate","actions":["manage"]}
        ]
    }]}))
    .unwrap();
    assert!(scope.validate().is_empty());
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id: key_id,
            user_id: actor.id(),
            name: "Reviewed runtime confirmation",
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
    drop(conn);
    resources::name_person(&world, "template", template, "template.use", actor.id()).await;
    if managed {
        resources::name_person(
            &world,
            "provider",
            team.provider,
            "provider.manage",
            actor.id(),
        )
        .await;
        resources::name_person(
            &world,
            "pool",
            selected_pool.unwrap(),
            "pool.use",
            actor.id(),
        )
        .await;
    }
    let plan = world
        .app
        .call(
            provider_api::ProviderRuntimeUpdatePreflight,
            &[&team.provider],
        )
        .token(&secret)
        .body(&api::UpdatePreflightBody::default())
        .send()
        .await
        .ok();
    assert!(plan.recovery);
    assert!(plan.blockers.is_empty());
    let saved = jobs::forks::preflight(&mut *world.conn().await, plan.preflight_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.plan["provider_recovery"]["recipe"]["host"],
        json!(&target)
    );
    let drawn: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pool_choices WHERE source->>'provider'=$1")
            .bind(team.provider.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(drawn, 0, "review cannot reserve or consume a pool turn");
    let response = world
        .app
        .call(provider_api::ProviderRuntimeUpdateCreate, &[&team.provider])
        .token(&secret)
        .body(&ForkCreateBody {
            preflight_id: plan.preflight_id,
            options: None,
            notify: Some(false),
        })
        .send()
        .await
        .ok();
    let id = match response {
        JobCreationResponse::Full(job) => job.id,
        JobCreationResponse::Receipt(receipt) => receipt.job_id,
    };
    let row = jobs::job(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.plan["provider_recovery"]["host"], json!(&target));
    let configured: serde_json::Value =
        sqlx::query_scalar("SELECT runtime_config->'runtime.host_id' FROM providers WHERE id=$1")
            .bind(team.provider)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        configured,
        json!(&target),
        "the accepted job retains the reviewed pool instead of a promised physical member"
    );
    assert!(
        orch_engine::jobs::lifecycle_commands::environment::present(&row),
        "accepted singleton recovery must retain its real native original source"
    );
    let clock = world.app.app.clock.clone();
    let access = orch_api_core::thread_access::AuthzAccess::new(clock.clone());
    let checked = orch_engine::jobs::lifecycle_commands::environment::principal(
        &mut *world.conn().await,
        &row,
        clock.as_ref(),
        &access,
        None,
    )
    .await
    .expect("prove the live original boundary before withdrawing its key");
    assert_eq!(
        checked.principal.user(),
        actor.id(),
        "the provider owner never becomes the acting manager"
    );
    assert_eq!(
        checked.principal.caller().admitted_operation(),
        Some("ProviderRuntimeUpdateCreate")
    );
    assert_eq!(
        checked
            .principal
            .caller()
            .grant()
            .and_then(|grant| grant.token()),
        Some(key_id)
    );
    jobs::merge_plan(
        &mut *world.conn().await,
        id,
        &json!({"runtime":new_id()}),
        world.now(),
    )
    .await
    .unwrap();
    let broken = jobs::job(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        orch_engine::jobs::lifecycle_commands::environment::principal(
            &mut *world.conn().await,
            &broken,
            clock.as_ref(),
            &access,
            None,
        )
        .await
        .is_err(),
        "a retained runtime reference must name this provider's actual native runtime"
    );
    sqlx::query("UPDATE jobs SET plan=plan-'runtime' WHERE id=$1")
        .bind(id)
        .execute(&world.db.pool)
        .await
        .unwrap();
    orch_engine::jobs::lifecycle_commands::environment::principal(
        &mut *world.conn().await,
        &row,
        clock.as_ref(),
        &access,
        None,
    )
    .await
    .expect("the original key still works after the bad native association is removed");
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key_id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        orch_engine::jobs::lifecycle_commands::environment::principal(
            &mut *world.conn().await,
            &row,
            clock.as_ref(),
            &access,
            None,
        )
        .await
        .is_err(),
        "an ended actual key cannot become its owner's session"
    );
    let sent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind IN ('node.runtime.create','node.runtime.start')",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(sent, 0);
}
