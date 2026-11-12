//! Real provider updates and model reads feed the same final wire option constraints.
use super::*;
use orch_api_contract::resources::{ProviderGet, ProviderUpdate};
use orch_model_contract::parameters::{outgoing, resolve};
use serde_json::Value;

async fn current(world: &World, by: &Who, id: Id) -> Provider {
    world
        .app
        .call(ProviderGet, &[&id])
        .token(&by.token)
        .send()
        .await
        .ok()
}
async fn update(world: &World, by: &Who, provider: Id, mut body: Value) -> Provider {
    body["expected_revision"] = json!(current(world, by, provider).await.revision);
    world
        .app
        .call(ProviderUpdate, &[&provider])
        .token(&by.token)
        .raw_body(&body)
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .provider
}

#[tokio::test]
async fn endpoint_and_named_group_options_survive_native_updates_and_bound_final_requests() {
    let world = world().await;
    let c = company(&world).await;
    let p = provider(&world, &c.boss, c.acme, None).await;
    let endpoint = json!({"openai-responses":{"type":"object","additionalProperties":false,"properties":{"temperature":{"type":"number","minimum":0,"maximum":1}}}});
    let group = json!({"frontier":{"request_options":{"openai-responses":{"type":"object","additionalProperties":false,"properties":{"temperature":{"type":"number","maximum":0.5}}}}}});
    let changed = update(&world, &c.boss, p.base.id, json!({
        "upstreams":[{"protocol":"openai-responses","base_url":"https://api.example.com/v1","options":{"request_options":endpoint}}],
        "model_group_options":group,
    })).await;
    assert_eq!(
        serde_json::to_value(&changed.model_group_options).unwrap(),
        group
    );
    assert_eq!(changed.upstreams[0].options["request_options"], endpoint);
    let local: ModelParameters = serde_json::from_value(json!({"request_options":{"openai-responses":{
        "type":"object","properties":{"temperature":{"type":"number","maximum":2,"default":0.25}}
    }}})).unwrap();
    let model = create(
        &world,
        &c.boss,
        p.base.id,
        "layered-model",
        Some(local.clone()),
        Some(words(&["frontier"])),
    )
    .await
    .ok()
    .into_full()
    .unwrap();
    assert_eq!(
        model.parameters, local,
        "an editor still reads its own declaration"
    );
    let list = world
        .app
        .call(ProviderModelList, &[&p.base.id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(
        list[0].option_constraints["openai-responses"]["properties"]["temperature"]["default"],
        0.25
    );
    let row = orch_db::queries::resources::thread_engine::model(
        &mut *world.conn().await,
        p.base.id,
        "layered-model",
    )
    .await
    .unwrap()
    .unwrap();
    let composed =
        orch_engine::binding::model_parameters_for(&row, Some("openai-responses")).unwrap();
    let defaults = resolve("openai-responses", &composed, &json!({}), &json!({})).unwrap();
    assert_eq!(defaults["temperature"], 0.25);
    for temperature in [0.0, 0.5] {
        outgoing(
            "openai-responses",
            &composed,
            &serde_json::to_vec(
                &json!({"model":"layered-model","input":"fixture","temperature":temperature}),
            )
            .unwrap(),
        )
        .unwrap();
    }
    for temperature in [-0.25, 0.75, 1.5] {
        assert!(
            outgoing(
                "openai-responses",
                &composed,
                &serde_json::to_vec(
                    &json!({"model":"layered-model","input":"fixture","temperature":temperature})
                )
                .unwrap()
            )
            .is_err()
        );
    }
    let stored: Value = sqlx::query_scalar("SELECT model_group_options FROM providers WHERE id=$1")
        .bind(p.base.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(stored, group);
    let saved = current(&world, &c.boss, p.base.id).await;
    let stale = world
        .app
        .call(ProviderUpdate, &[&p.base.id])
        .token(&c.boss.token)
        .raw_body(&json!({"expected_revision":p.revision,"model_group_options":{}}))
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(
        current(&world, &c.boss, p.base.id)
            .await
            .model_group_options,
        saved.model_group_options
    );
}

#[tokio::test]
async fn model_option_snapshots_stay_frozen_while_new_reads_take_the_new_group_limit() {
    let world = world().await;
    let c = company(&world).await;
    let p = provider(&world, &c.boss, c.acme, None).await;
    create(
        &world,
        &c.boss,
        p.base.id,
        "frozen-model",
        None,
        Some(words(&["frontier"])),
    )
    .await
    .ok();
    let group = |maximum| {
        json!({"model_group_options":{"frontier":{"request_options":{"openai-responses":{
            "type":"object","properties":{"temperature":{"type":"number","maximum":maximum}}
        }}}}})
    };
    update(&world, &c.boss, p.base.id, group(0.5)).await;
    let mut snapshot = world.app.app.db.begin_snapshot().await.unwrap();
    let before =
        orch_db::queries::resources::thread_engine::model(&mut snapshot, p.base.id, "frozen-model")
            .await
            .unwrap()
            .unwrap();
    let frozen =
        orch_engine::binding::model_parameters_for(&before, Some("openai-responses")).unwrap();
    update(&world, &c.boss, p.base.id, group(0.25)).await;
    let same =
        orch_db::queries::resources::thread_engine::model(&mut snapshot, p.base.id, "frozen-model")
            .await
            .unwrap()
            .unwrap();
    assert_eq!(same, before);
    snapshot.commit().await.unwrap();
    let after = orch_db::queries::resources::thread_engine::model(
        &mut *world.conn().await,
        p.base.id,
        "frozen-model",
    )
    .await
    .unwrap()
    .unwrap();
    let fresh =
        orch_engine::binding::model_parameters_for(&after, Some("openai-responses")).unwrap();
    let wire =
        serde_json::to_vec(&json!({"model":"frozen-model","input":"fixture","temperature":0.4}))
            .unwrap();
    assert!(outgoing("openai-responses", &frozen, &wire).is_ok());
    assert!(outgoing("openai-responses", &fresh, &wire).is_err());
}

#[tokio::test]
async fn model_group_membership_uses_the_scoped_configured_default_without_a_database_eight_cap() {
    let world = world().await;
    let c = company(&world).await;
    let p = provider(&world, &c.boss, c.acme, None).await;
    let groups = (0..9)
        .map(|number| format!("group-{number}"))
        .collect::<Vec<_>>();
    let problem = create(
        &world,
        &c.boss,
        p.base.id,
        "default-limited",
        None,
        Some(groups.clone()),
    )
    .await
    .problem();
    assert_eq!(problem.code, ErrorCode::RequestInvalid);
    world
        .set_at(
            orch_settings::Target::Node(c.acme),
            "providers.model_groups_max",
            json!(9),
        )
        .await;
    let model = create(
        &world,
        &c.boss,
        p.base.id,
        "raised-limit",
        None,
        Some(groups.clone()),
    )
    .await
    .ok()
    .into_full()
    .unwrap();
    assert_eq!(model.groups, groups);
    assert_eq!(
        current(&world, &c.boss, p.base.id).await.model_groups_max,
        Some(9)
    );
    world
        .set_at(
            orch_settings::Target::Node(c.acme),
            "providers.model_groups_max",
            json!(0),
        )
        .await;
    let rows = world
        .app
        .call(ProviderModelList, &[&p.base.id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(
        rows[0].groups, groups,
        "changing a default never rewrites membership"
    );
    let problem = create(
        &world,
        &c.boss,
        p.base.id,
        "none-now",
        None,
        Some(words(&["one"])),
    )
    .await
    .problem();
    assert_eq!(problem.code, ErrorCode::RequestInvalid);
    create(&world, &c.boss, p.base.id, "ungrouped", None, None)
        .await
        .ok();
    let edited = world
        .app
        .call(ProviderModelUpdate, &[&p.base.id, &model.id])
        .token(&c.boss.token)
        .raw_body(&json!({"groups":groups[..8],"display_name":"Existing membership edited"}))
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(edited.groups, groups[..8]);
}

#[tokio::test]
async fn malformed_group_or_endpoint_schemas_and_foreign_management_change_no_provider_state() {
    let world = world().await;
    let c = company(&world).await;
    let p = provider(&world, &c.boss, c.acme, None).await;
    for body in [
        json!({"model_group_options":{"Bad Group":{"request_options":{}}}}),
        json!({
            "model_group_options": {
                "frontier": {
                    "request_options": {"openai-responses": {"type":"not-a-type"}}
                }
            }
        }),
        json!({"upstreams":[{"protocol":"openai-responses","base_url":"https://api.example.com/v1","options":{"request_options":{"openai-responses":{"$ref":"https://example.invalid/schema.json"}}}}]}),
    ] {
        let mut body = body;
        body["expected_revision"] = json!(p.revision);
        let error = world
            .app
            .call(ProviderUpdate, &[&p.base.id])
            .token(&c.boss.token)
            .raw_body(&body)
            .send()
            .await
            .problem();
        assert_eq!(error.code, ErrorCode::RequestInvalid);
        assert_eq!(
            current(&world, &c.boss, p.base.id).await.revision,
            p.revision
        );
    }
    let denied=world.app.call(ProviderUpdate,&[&p.base.id]).token(&c.stranger.token)
        .raw_body(&json!({"expected_revision":p.revision,"model_group_options":{"frontier":{"request_options":{}}}})).send().await;
    assert!(!denied.status.is_success());
    let after = current(&world, &c.boss, p.base.id).await;
    assert_eq!(after.revision, p.revision);
    assert!(after.model_group_options.is_empty());
}
