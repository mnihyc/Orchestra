use super::*;
use orch_api_contract::settings::{
    NodeSettingDelete, NodeSettingDeleteQuery, NodeSettingSet, NodeSettingSetBody, SettingScope,
    ThreadParameters,
};
use orch_types::Id;

async fn read(s: &Scene, thread: Id) -> ThreadParameters {
    s.rig
        .world
        .app
        .call(ThreadParameterGet, &[&thread])
        .token(&s.ada.token)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn provider_wire_choices_use_the_existing_thread_setting_revision_source_and_independent_reset()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    sqlx::query("UPDATE provider_models SET parameters=$2 WHERE provider_id=$1 AND model_name=$3")
        .bind(s.on.provider)
        .bind(json!({"request_options":{
            "openai-responses":{"type":"object","properties":{
                "mode":{"type":"string","enum":["a","b"]},
                "region":{"const":"local"},"model":{"type":"string"}
            }},
            "native":{"type":"object","properties":{"native_only":{"type":"string"}}}
        }}))
        .bind(MODEL)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    assert!(
        read(&s, thread)
            .await
            .parameters
            .iter()
            .all(|row| !row.key.starts_with("provider_options:"))
    );
    sqlx::query("UPDATE providers SET kind='api',harness_kind=NULL,agent_id=NULL,protocols=ARRAY['openai-responses'],arrangements=ARRAY['separated','key_separated'],upstreams=$2 WHERE id=$1")
        .bind(s.on.provider)
        .bind(json!([{"protocol":"openai-responses","base_url":"https://model.example.test/v1","options":{}}]))
        .execute(s.rig.db.pool()).await.unwrap();
    let thread = s.thread().await.base.id;
    let initial = read(&s, thread).await;
    let row = initial
        .parameters
        .iter()
        .find(|row| row.key == "provider_options:openai-responses")
        .unwrap();
    assert_eq!(row.source.scope, SettingScope::Default);
    assert_eq!(row.value, None);
    assert!(row.writable);
    assert_eq!(
        row.apply,
        orch_api_contract::settings::ParameterApply::NextTurn
    );
    assert_eq!(
        row.def["provider_constraint"]["properties"]["mode"]["enum"],
        json!(["a", "b"])
    );
    assert_eq!(
        row.def["provider_constraint"]["properties"]["region"]["const"],
        "local"
    );
    assert!(
        row.def["provider_constraint"]["properties"]
            .get("model")
            .is_none()
    );
    assert!(
        initial
            .parameters
            .iter()
            .all(|row| row.key != "provider_options:native")
    );

    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "agent.params",
            json!({
                "provider_options:openai-responses":{"mode":"a"},"native_kept":"from the place"
            }),
        )
        .await;
    let row = read(&s, thread)
        .await
        .parameters
        .into_iter()
        .find(|row| row.key == "provider_options:openai-responses")
        .unwrap();
    assert_eq!(row.source.scope, SettingScope::Topic);
    assert_eq!(row.value, Some(json!({"mode":"a"})));
    let changed = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&thread, &"agent.params"])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value: json!({"provider_options:openai-responses":{"mode":"b"},"another_choice":true}),
            expected_revision: Some(0),
            locked: None,
        })
        .send()
        .await
        .full_setting()
        .ok();
    let row = read(&s, thread)
        .await
        .parameters
        .into_iter()
        .find(|row| row.key == "provider_options:openai-responses")
        .unwrap();
    assert_eq!(row.source.scope, SettingScope::Thread);
    assert_eq!(row.value, Some(json!({"mode":"b"})));
    assert_eq!(
        changed.value.as_ref().unwrap()["native_kept"],
        "from the place"
    );
    let revision = changed.own.as_ref().unwrap().revision;
    let stale = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&thread, &"agent.params"])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value: json!({"provider_options:openai-responses":{"mode":"a"}}),
            expected_revision: Some(0),
            locked: None,
        })
        .send()
        .await;
    assert_eq!(stale.status, 409);
    assert_eq!(
        read(&s, thread)
            .await
            .parameters
            .into_iter()
            .find(|row| row.key == "provider_options:openai-responses")
            .unwrap()
            .value,
        Some(json!({"mode":"b"}))
    );
    // Removing just this ordinary member restores its inherited source and keeps another choice.
    let reset = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&thread, &"agent.params"])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value: json!({"another_choice":true}),
            expected_revision: Some(revision),
            locked: None,
        })
        .send()
        .await
        .full_setting()
        .ok();
    assert_eq!(reset.value.as_ref().unwrap()["another_choice"], true);
    let row = read(&s, thread)
        .await
        .parameters
        .into_iter()
        .find(|row| row.key == "provider_options:openai-responses")
        .unwrap();
    assert_eq!(row.value, Some(json!({"mode":"a"})));
    assert_eq!(row.source.scope, SettingScope::Topic);
    s.rig
        .world
        .app
        .call(NodeSettingDelete, &[&thread, &"agent.params"])
        .token(&s.ada.token)
        .query(&NodeSettingDeleteQuery {
            expected_revision: Some(reset.own.unwrap().revision),
        })
        .send()
        .await
        .full_setting()
        .ok();
}
