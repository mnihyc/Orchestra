//! Resource saves validate nested schemas before writing any field and refuse unavailable schema declarations.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod rig;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::*;
use orch_types::{Id, Json};
use resources::{Company, company};
use serde_json::json;
use support::World;

async fn reported(world: &World, c: &Company) {
    let mut conn = world.conn().await;
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(c.admin.id()),
        Some(c.acme),
        "schema-host",
    )
    .await
    .unwrap();
    let schema = json!({"$defs":{"source":{"type":"object","properties":{"image":{"type":"string"}},"required":["image"],"additionalProperties":false}},
        "type":"object","properties":{"source":{"$ref":"#/$defs/source"},"memory":{"type":"integer","minimum":1}},"required":["source"],"additionalProperties":false});
    sqlx::query("UPDATE hosts SET drivers=jsonb_build_object('schema-driver',$2::jsonb) WHERE id=$1")
        .bind(host).bind(json!({"name":"schema-driver","version":"1.0.0","spec_schemas":{"environment":schema,"provider":{"type":"object","properties":{"entrypoint":{"type":"string"}},"required":["entrypoint"],"additionalProperties":false}}}))
        .execute(&mut *conn).await.unwrap();
}

fn body(home: Id, spec: Json) -> TemplateCreateBody {
    TemplateCreateBody {
        name: "Validated".into(),
        description: None,
        home_id: Some(home),
        kind: None,
        driver: "schema-driver".into(),
        spec,
        access: None,
    }
}

#[tokio::test]
async fn a_nested_template_saves_and_an_invalid_update_changes_neither_name_revision_nor_recipe() {
    let world = World::new().await;
    let c = company(&world).await;
    reported(&world, &c).await;
    let valid = json!({"source":{"image":"base"},"memory":256});
    let made = world
        .app
        .call(TemplateCreate, &[])
        .token(&c.admin.token)
        .body(&body(c.acme, valid.clone()))
        .send()
        .await
        .ok();
    for spec in [
        json!({"source":{"image":false},"memory":256}),
        json!({"source":{"image":"base","unknown":"value-never-echoed"}}),
        json!({"source":{"image":"base"},"memory":0}),
    ] {
        let change = TemplateUpdateBody {
            name: Some("This must not save".into()),
            description: None,
            spec: Some(spec),
            enabled: Some(false),
            expected_revision: made.revision,
        };
        let refused = world
            .app
            .call(TemplateUpdate, &[&made.id])
            .token(&c.admin.token)
            .body(&change)
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
        assert!(
            refused.extra["errors"]
                .as_array()
                .unwrap()
                .iter()
                .all(|fault| fault["path"].as_str().unwrap().starts_with("spec/"))
        );
        assert!(
            !serde_json::to_string(&refused)
                .unwrap()
                .contains("value-never-echoed")
        );
        let current = world
            .app
            .call(TemplateGet, &[&made.id])
            .token(&c.admin.token)
            .send()
            .await
            .ok();
        assert_eq!(current.name, made.name);
        assert_eq!(current.revision, made.revision);
        assert_eq!(current.spec, valid);
        assert!(current.enabled);
    }
    let mut provider = body(c.acme, valid);
    provider.name = "Provider".into();
    provider.kind = Some(UnitKind::Provider);
    assert_eq!(
        world
            .app
            .call(TemplateCreate, &[])
            .token(&c.admin.token)
            .body(&provider)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let mut unknown = body(c.acme, json!({}));
    unknown.driver = "never-reported".into();
    assert_eq!(
        world
            .app
            .call(TemplateCreate, &[])
            .token(&c.admin.token)
            .body(&unknown)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::SourceOffline
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM env_templates WHERE name IN ('Validated','Provider')",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn profile_envelopes_validate_nested_attach_and_environment_values_before_saving() {
    let world = World::new().await;
    let c = company(&world).await;
    let made = world
        .app
        .call(OrgAgentCreate, &[&c.acme])
        .token(&c.admin.token)
        .body(&OrgAgentCreateBody {
            name: "Profile".into(),
            harness_kind: "fake".into(),
            profile: json!({"config":{"native":{"kept":true}},"env":{"MODE":"one"}}),
        })
        .send()
        .await
        .ok();
    let change = AgentUpdateBody {
        name: Some("Must not save".into()),
        profile: Some(json!({"env":{"MODE":{"private":"never-echo-this"}}})),
        descriptor_overrides: None,
        disabled: Some(true),
        expected_revision: made.revision,
    };
    let refused = world
        .app
        .call(AgentUpdate, &[&made.base.id])
        .token(&c.admin.token)
        .body(&change)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(
        !serde_json::to_string(&refused)
            .unwrap()
            .contains("never-echo-this")
    );
    let current = world
        .app
        .call(AgentGet, &[&made.base.id])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(current.base.name, made.base.name);
    assert_eq!(current.revision, made.revision);
    assert_eq!(current.profile, made.profile);
    assert!(!current.base.disabled);
    let bad = OrgAgentCreateBody {
        name: "Unknown nested member".into(),
        harness_kind: "fake".into(),
        profile: json!({"attach":{"mode":"spawn","unrecognized":true}}),
    };
    assert_eq!(
        world
            .app
            .call(OrgAgentCreate, &[&c.acme])
            .token(&c.admin.token)
            .body(&bad)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
}

struct Rules;

#[async_trait::async_trait]
impl orch_summarizer_contract::summarizer::Summarizer for Rules {
    fn describe(&self) -> orch_summarizer_contract::descriptor::SummarizerDescriptor {
        serde_json::from_value(json!({"name":"schema-rules","title":"Rules","fields":["title"],"config_schema":{
            "$defs":{"limits":{"type":"object","properties":{"maximum":{"type":"integer","minimum":1}},"required":["maximum"],"additionalProperties":false}},
            "type":"object","properties":{"limits":{"$ref":"#/$defs/limits"}},"required":["limits"],"additionalProperties":false
        }})).unwrap()
    }

    async fn summarize(
        &self,
        _: orch_summarizer_contract::summarizer::SummaryRequest,
    ) -> Result<
        orch_summarizer_contract::result::SummaryResult,
        orch_summarizer_contract::summarizer::SummarizeError,
    > {
        panic!("saving a configuration never invokes its summarizer")
    }
}

#[tokio::test]
async fn node_and_system_summarizer_settings_validate_nested_declared_configuration_before_save() {
    use orch_api_contract::admin::{AdminSettingList, AdminSettingSet, AdminSettingSetBody};
    use orch_api_contract::settings::{NodeSettingList, NodeSettingSet, NodeSettingSetBody};
    let world = World::new().await;
    let c = company(&world).await;
    world
        .app
        .app
        .engine
        .attach_summarizer(std::sync::Arc::new(Rules));
    let valid = json!({"kind":"schema-rules","limits":{"maximum":3}});
    let made = world
        .app
        .call(NodeSettingSet, &[&c.acme, &"auto.summarizer"])
        .token(&c.admin.token)
        .body(&NodeSettingSetBody {
            value: valid.clone(),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let revision = made.own.as_ref().unwrap().revision;
    for invalid in [
        json!({"kind":"schema-rules","limits":{"maximum":false}}),
        json!({"kind":"schema-rules","limits":{"maximum":3,"unknown":"do-not-echo"}}),
    ] {
        let refused = world
            .app
            .call(NodeSettingSet, &[&c.acme, &"auto.summarizer"])
            .token(&c.admin.token)
            .body(&NodeSettingSetBody {
                value: invalid,
                locked: Some(true),
                expected_revision: Some(revision),
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
        assert!(
            !serde_json::to_string(&refused)
                .unwrap()
                .contains("do-not-echo")
        );
        assert!(
            refused.extra["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|fault| fault["path"].as_str().unwrap().starts_with("value/limits"))
        );
    }
    let current = world
        .app
        .call(NodeSettingList, &[&c.acme])
        .token(&c.admin.token)
        .send()
        .await
        .ok();
    let current = current
        .iter()
        .find(|setting| setting.key == "auto.summarizer")
        .unwrap();
    assert_eq!(current.own, made.own);
    assert_eq!(current.value, Some(valid.clone()));
    let unavailable = world
        .app
        .call(NodeSettingSet, &[&c.acme, &"auto.summarizer"])
        .token(&c.admin.token)
        .body(&NodeSettingSetBody {
            value: json!({"kind":"no-such-producer","token":"do-not-echo"}),
            locked: None,
            expected_revision: Some(revision),
        })
        .send()
        .await
        .problem();
    assert_eq!(unavailable.code, ErrorCode::SourceOffline);
    assert!(
        !serde_json::to_string(&unavailable)
            .unwrap()
            .contains("do-not-echo")
    );
    let root = places::admin(&world, "system-schema-admin").await;
    let made = world
        .app
        .call(AdminSettingSet, &[&"auto.summarizer"])
        .token(&root.token)
        .body(&AdminSettingSetBody {
            value: valid,
            locked: Some(false),
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let refused = world
        .app
        .call(AdminSettingSet, &[&"auto.summarizer"])
        .token(&root.token)
        .body(&AdminSettingSetBody {
            value: json!({"kind":"schema-rules","limits":{"maximum":0}}),
            locked: Some(true),
            expected_revision: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    let current = world
        .app
        .call(AdminSettingList, &[])
        .token(&root.token)
        .send()
        .await
        .ok();
    assert_eq!(
        current
            .iter()
            .find(|setting| setting.key == "auto.summarizer")
            .unwrap()
            .own,
        made.own
    );
    let missing_model = world
        .app
        .call(AdminSettingSet, &[&"auto.summarizer"])
        .token(&root.token)
        .body(&AdminSettingSetBody {
            value: json!({"kind":"model","model_name":"small"}),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(
        missing_model.code,
        ErrorCode::RequestInvalid,
        "the built-in model uses its descriptor too"
    );
    let zero_budget = world.app.call(AdminSettingSet, &[&"auto.summarizer"]).token(&root.token)
        .body(&AdminSettingSetBody { value: json!({"kind":"model","provider_id":Id::nil(),"model_name":"small","max_output_tokens":0}), locked: None, expected_revision: None }).send().await.problem();
    assert_eq!(zero_budget.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn subscription_option_schemas_validate_nested_values_without_partially_changing_the_provider()
 {
    let scene = rig::Scene::new().await;
    let world = &scene.rig.world;
    let mut descriptor = rig::descriptor();
    descriptor["native_provider"]["options_schema"] = json!({"type":"object","properties":{"routing":{"type":"object","properties":{"region":{"type":"string","enum":["local","remote"]}},"required":["region"],"additionalProperties":false}},"required":["routing"],"additionalProperties":false});
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(scene.on.agent)
        .bind(descriptor)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let before = world
        .app
        .call(ProviderGet, &[&scene.on.provider])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    let valid = json!({"routing":{"region":"local"}});
    let made = world
        .app
        .call(ProviderUpdate, &[&scene.on.provider])
        .token(&scene.ada.token)
        .body(&ProviderUpdateBody {
            name: None,
            upstreams: None,
            config: Some(valid.clone()),
            arrangements: None,
            runtime_config: None,
            expected_revision: before.revision,
        })
        .send()
        .await
        .ok()
        .provider;
    for config in [
        json!({"routing":{"region":false}}),
        json!({"routing":{"region":"local","token":"do-not-echo"}}),
    ] {
        let refused = world
            .app
            .call(ProviderUpdate, &[&scene.on.provider])
            .token(&scene.ada.token)
            .body(&ProviderUpdateBody {
                name: Some("Must not save".into()),
                upstreams: None,
                config: Some(config),
                arrangements: None,
                runtime_config: None,
                expected_revision: made.revision,
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
        assert!(
            !serde_json::to_string(&refused)
                .unwrap()
                .contains("do-not-echo")
        );
        let current = world
            .app
            .call(ProviderGet, &[&scene.on.provider])
            .token(&scene.ada.token)
            .send()
            .await
            .ok();
        assert_eq!(current.base.name, made.base.name);
        assert_eq!(current.revision, made.revision);
        assert_eq!(current.config, valid);
    }
}
