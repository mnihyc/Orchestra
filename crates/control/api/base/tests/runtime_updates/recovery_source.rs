//! A reviewed provider recovery retains the actual HTTP key at its native boundaries.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::workspace::JobCreationResponse;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{identity, jobs};

#[tokio::test]
async fn a_lost_provider_recovery_keeps_its_actual_confirmation_key_after_the_http_reply() {
    let world = World::new().await;
    let team = team(&world).await;
    world.app.app.engine.stop();
    let mut conn = world.conn().await;
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
    sqlx::query("UPDATE providers SET runtime_unavailable='Its runtime was declared lost',status='offline',runtime_config=runtime_config||jsonb_build_object('runtime.template_id',$2::uuid::text,'runtime.host_id',$3::jsonb) WHERE id=$1")
        .bind(team.provider).bind(template).bind(json!(orch_provider_contract::pools::Target::host(team.host)))
        .execute(&mut *conn).await.unwrap();
    let key_id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
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
            user_id: team.c.admin.id(),
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
    resources::name_person(
        &world,
        "template",
        template,
        "template.use",
        team.c.admin.id(),
    )
    .await;
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
