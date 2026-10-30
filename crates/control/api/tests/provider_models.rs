//! The models of a provider over HTTP: adding, listing, changing and removing them with the two names each has, and discovery, asked of the machine that holds the key.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    CreateProvider, ModelParameters, ModelSource, Provider, ProviderCreate, ProviderGet,
    ProviderModel, ProviderModelCreate, ProviderModelCreateBody, ProviderModelDelete,
    ProviderModelList, ProviderModelSuggestionAccept, ProviderModelSuggestionAcceptBody,
    ProviderModelSuggestionList, ProviderModelUpdate, ProviderModelUpdateBody, ProviderStatus,
    ServiceTier, Upstream,
};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_db::secrets::MasterKey;
use orch_types::Id;
use places::{Who, missing_permissions};
use resources::{answer_discovery, company, machine_forwarding_to, name_person, online};
use serde_json::json;
use support::World;

async fn world() -> World {
    install_master_key(MasterKey::new([7; 32]));
    World::new().await
}

fn endpoint() -> Upstream {
    Upstream {
        protocol: "openai-responses".to_owned(),
        base_url: "https://api.example.com/v1".to_owned(),
        options: json!({}),
    }
}

async fn provider(world: &World, by: &Who, home: Id, key_machine: Option<Id>) -> Provider {
    world
        .app
        .call(ProviderCreate, &[])
        .token(&by.token)
        .body(&CreateProvider::Api {
            runtime_machine_id: None,
            name: "Team key".to_owned(),
            home_id: home,
            access: None,
            arrangements: None,
            runtime_config: None,
            upstreams: vec![endpoint()],
            api_key: key_machine.is_none().then(|| "sk-test".to_owned()),
            secrets: None,
            key_machine_id: key_machine,
            sources: None,
        })
        .send()
        .await
        .ok()
        .provider
}

async fn add(
    world: &World,
    by: &Who,
    provider: Id,
    name: &str,
    display: Option<&str>,
) -> ProviderModel {
    world
        .app
        .call(ProviderModelCreate, &[&provider])
        .token(&by.token)
        .body(&ProviderModelCreateBody {
            model_name: name.to_owned(),
            display_name: display.map(str::to_owned),
            parameters: None,
            groups: None,
        })
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn models_are_added_listed_changed_and_removed_by_whoever_manages_the_provider() {
    let world = world().await;
    let c = company(&world).await;
    let id = provider(&world, &c.boss, c.acme, None).await.base.id;
    let sol = world
        .app
        .call(ProviderModelCreate, &[&id])
        .token(&c.boss.token)
        .body(&ProviderModelCreateBody {
            model_name: "openai/gpt-6-sol".to_owned(),
            display_name: Some("  GPT-6 Sol (A) ".to_owned()),
            parameters: Some(ModelParameters {
                efforts: Some(vec!["low".into(), "high".into()]),
                default_effort: Some("low".into()),
                context_window: Some(400_000),
                ..ModelParameters::default()
            }),
            groups: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        sol.model_name, "openai/gpt-6-sol",
        "the routing name is taken as it is given"
    );
    assert_eq!(sol.display_name, "GPT-6 Sol (A)");
    assert!(sol.enabled);
    assert_eq!(sol.source, ModelSource::Manual);
    assert_eq!(
        sol.parameters.efforts.as_deref(),
        Some(&["low".to_owned(), "high".to_owned()][..])
    );
    let luna = add(&world, &c.boss, id, "gpt-6-luna", None).await;
    assert_eq!(luna.display_name, "", "the routing name is shown");
    // Anybody who holds any permission on the provider reads its models.
    name_person(&world, "provider", id, "provider.view", c.viewer.id()).await;
    let listed = world
        .app
        .call(ProviderModelList, &[&id])
        .token(&c.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(
        listed
            .iter()
            .map(|m| m.model_name.as_str())
            .collect::<Vec<_>>(),
        ["openai/gpt-6-sol", "gpt-6-luna"]
    );
    let stranger = world
        .app
        .call(ProviderModelList, &[&id])
        .token(&c.stranger.token)
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&stranger), ["provider.view"]);
    // Only a manager changes them.
    let body = ProviderModelUpdateBody {
        enabled: Some(false),
        ..ProviderModelUpdateBody::default()
    };
    let refused = world
        .app
        .call(ProviderModelUpdate, &[&id, &luna.id])
        .token(&c.viewer.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["provider.manage"]);
    let off = world
        .app
        .call(ProviderModelUpdate, &[&id, &luna.id])
        .token(&c.boss.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert!(!off.enabled);
    let renamed = world
        .app
        .call(ProviderModelUpdate, &[&id, &luna.id])
        .token(&c.boss.token)
        .body(&ProviderModelUpdateBody {
            display_name: Some("GPT-6 Luna".into()),
            ..ProviderModelUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(renamed.display_name, "GPT-6 Luna");
    assert_eq!(
        renamed.model_name, "gpt-6-luna",
        "a routing name is not changed"
    );
    let refused = world
        .app
        .call(ProviderModelDelete, &[&id, &luna.id])
        .token(&c.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["provider.manage"]);
    let removed = world
        .app
        .call(ProviderModelDelete, &[&id, &luna.id])
        .token(&c.boss.token)
        .send()
        .await;
    assert_eq!(removed.status, 204, "{:?}", removed.body);
    let left = world
        .app
        .call(ProviderModelList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(left.len(), 1);
    let gone = world
        .app
        .call(ProviderModelDelete, &[&id, &luna.id])
        .token(&c.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(gone.code, ErrorCode::NotFound);
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'provider.models_changed'",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(audited, 5, "added twice, changed twice, removed once");
}

#[tokio::test]
async fn two_models_of_one_provider_never_go_by_one_routing_name_or_show_under_one_name() {
    let world = world().await;
    let c = company(&world).await;
    let id = provider(&world, &c.boss, c.acme, None).await.base.id;
    let first = add(&world, &c.boss, id, "gpt-6-sol", Some("GPT-6 Sol (A)")).await;
    let call = |name: &str, display: Option<&str>| {
        world
            .app
            .call(ProviderModelCreate, &[&id])
            .token(&c.boss.token)
            .body(&ProviderModelCreateBody {
                model_name: name.to_owned(),
                display_name: display.map(str::to_owned),
                parameters: None,
                groups: None,
            })
    };
    let same_name = call("gpt-6-sol", None).send().await.problem();
    assert_eq!(same_name.code, ErrorCode::ConflictExists);
    assert_eq!(same_name.extra["existing_id"], json!(first.id));
    let same_display = call("gpt-6-sol-b", Some("gpt-6 sol (a)"))
        .send()
        .await
        .problem();
    assert_eq!(
        same_display.code,
        ErrorCode::ConflictExists,
        "a display name clashes whatever its case"
    );
    // Routing names differ by their letters: the one rule is the name as it is written.
    let upper = call("GPT-6-SOL", None).send().await.ok();
    assert_eq!(upper.model_name, "GPT-6-SOL");
    let second = add(&world, &c.boss, id, "gpt-6-luna", Some("GPT-6 Luna")).await;
    let clash = world
        .app
        .call(ProviderModelUpdate, &[&id, &second.id])
        .token(&c.boss.token)
        .body(&ProviderModelUpdateBody {
            display_name: Some("GPT-6 Sol (A)".into()),
            ..ProviderModelUpdateBody::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(clash.code, ErrorCode::ConflictExists);
    for bad in ["", "  ", "two\nlines"] {
        let invalid = call(bad, None).send().await.problem();
        assert_eq!(invalid.code, ErrorCode::RequestInvalid, "{bad:?}");
    }
    // What the endpoint last reported on offer stays when a manager writes what is enabled.
    sqlx::query("UPDATE provider_models SET parameters = jsonb_build_object('offered', jsonb_build_object('access_programs', jsonb_build_array('early'))) WHERE id = $1")
        .bind(first.id).execute(&world.db.pool).await.unwrap();
    let changed = world
        .app
        .call(ProviderModelUpdate, &[&id, &first.id])
        .token(&c.boss.token)
        .body(&ProviderModelUpdateBody {
            parameters: Some(ModelParameters {
                access_programs: Some(vec!["early".into()]),
                ..ModelParameters::default()
            }),
            ..ProviderModelUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        changed.parameters.access_programs,
        Some(vec!["early".to_owned()])
    );
    assert_eq!(
        changed.parameters.offered.unwrap().access_programs,
        Some(vec!["early".to_owned()])
    );
}

#[tokio::test]
async fn a_provider_is_ready_only_while_it_has_a_model_that_is_enabled() {
    let world = world().await;
    let c = company(&world).await;
    let id = provider(&world, &c.boss, c.acme, None).await.base.id;
    let status = || async {
        world
            .app
            .call(ProviderGet, &[&id])
            .token(&c.boss.token)
            .send()
            .await
            .ok()
            .base
            .status
    };
    assert_eq!(status().await, ProviderStatus::NeedsSetup);
    let model = add(&world, &c.boss, id, "gpt-6-sol", None).await;
    assert_eq!(status().await, ProviderStatus::Ready);
    world
        .app
        .call(ProviderModelUpdate, &[&id, &model.id])
        .token(&c.boss.token)
        .body(&ProviderModelUpdateBody {
            enabled: Some(false),
            ..ProviderModelUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(status().await, ProviderStatus::NeedsSetup);
    world
        .app
        .call(ProviderModelUpdate, &[&id, &model.id])
        .token(&c.boss.token)
        .body(&ProviderModelUpdateBody {
            enabled: Some(true),
            ..ProviderModelUpdateBody::default()
        })
        .send()
        .await
        .ok();
    world
        .app
        .call(ProviderModelDelete, &[&id, &model.id])
        .token(&c.boss.token)
        .send()
        .await;
    assert_eq!(status().await, ProviderStatus::NeedsSetup);
}

fn discovered() -> serde_json::Value {
    json!({ "suggestions": [
        { "model_name": "gpt-6-sol", "display_name": "GPT-6 Sol", "listed": true,
          "parameters": { "efforts": ["low", "high"], "context_window": 400000,
                          "service_tiers": [{ "id": "fast", "label": "Fast" }], "access_programs": ["early"] } },
        { "model_name": "gpt-6-luna", "display_name": "gpt-6-luna", "listed": true, "parameters": {} }
    ] })
}

#[tokio::test]
async fn a_program_source_lists_models_before_any_is_enabled_and_acceptance_returns_the_ready_provider()
 {
    let world = world().await;
    let c = company(&world).await;
    let machine = machine_forwarding_to(
        &world,
        c.boss.id(),
        &["https://api.example.com/v1", "https://gpu.example/v1"],
    )
    .await;
    let id = provider(&world, &c.boss, c.acme, Some(machine))
        .await
        .base
        .id;
    online(&world, machine).await;
    let mut descriptor = orch_provider_contract::SourceDescriptor::new(
        "gpu-box",
        "GPU box",
        "Lists this GPU service's models.",
    );
    descriptor.models = Some(Default::default());
    sqlx::query("UPDATE providers SET upstreams=$2,sources=$3,status='needs_setup',status_detail='it needs an enabled model' WHERE id=$1")
        .bind(id).bind(json!([{"protocol":"openai-responses","base_url":"https://gpu.example/v1","options":{"auth":"none"}}]))
        .bind(json!({"models":{"by":"program","name":"gpu-box","descriptor":descriptor}})).execute(&world.db.pool).await.unwrap();
    let report = json!({"source":{"descriptor":descriptor,"supported":true,"error":null},"suggestions":[
        {"model_name":"gpu-small","display_name":"GPU Small","listed":true,"parameters":{"context_window":32768}},
        {"model_name":"gpu-large","display_name":"GPU Large","listed":true,"parameters":{"context_window":131072}}]});
    let native = answer_discovery(&world, machine, report.clone());
    let suggestions = world
        .app
        .call(ProviderModelSuggestionList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    native.await.unwrap();
    assert_eq!(
        suggestions
            .iter()
            .map(|model| model.display_name.as_str())
            .collect::<Vec<_>>(),
        ["GPU Small", "GPU Large"]
    );
    let saved: i64 =
        sqlx::query_scalar("SELECT count(*) FROM provider_models WHERE provider_id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        saved, 0,
        "a source read suggests models without choosing them for the manager"
    );
    let native = answer_discovery(&world, machine, report);
    let accepted = world
        .app
        .call(ProviderModelSuggestionAccept, &[&id])
        .token(&c.boss.token)
        .body(&ProviderModelSuggestionAcceptBody {
            model_names: vec!["gpu-small".into(), "gpu-large".into()],
        })
        .send()
        .await
        .ok();
    native.await.unwrap();
    assert_eq!(accepted.added.len(), 2);
    assert!(accepted.added.iter().all(|model| model.enabled));
    assert_eq!(accepted.provider.models.len(), 2);
    assert_eq!(accepted.provider.base.status, ProviderStatus::Ready);
    assert!(accepted.provider.status_detail.is_none());
    assert_eq!(
        accepted
            .provider
            .facts
            .models
            .as_ref()
            .unwrap()
            .iter()
            .map(|model| model.display_name.as_str())
            .collect::<Vec<_>>(),
        ["GPU Small", "GPU Large"]
    );
    let requests: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT body FROM commands WHERE machine_id=$1 AND kind='provider.discover_models'",
    )
    .bind(machine)
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request["models"]["by"] == "program"
                && request["models"]["name"] == "gpu-box")
    );
}

#[tokio::test]
async fn discovery_asks_the_machine_that_holds_the_key_and_adds_nothing_by_itself() {
    let world = world().await;
    let c = company(&world).await;
    // A key kept on the machine of its maker, which forwards model calls to the endpoint.
    let machine = machine_forwarding_to(&world, c.boss.id(), &["https://api.example.com/v1"]).await;
    let made = provider(&world, &c.boss, c.acme, Some(machine)).await;
    let id = made.base.id;
    assert!(!made.key_set && made.key_machine.as_ref().unwrap().id == machine);
    let away = world
        .app
        .call(ProviderModelSuggestionList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(away.code, ErrorCode::ProviderUnavailable);
    online(&world, machine).await;
    let machine_says = answer_discovery(&world, machine, discovered());
    let listed = world
        .app
        .call(ProviderModelSuggestionList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    machine_says.await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|s| s.model_name.as_str())
            .collect::<Vec<_>>(),
        ["gpt-6-sol", "gpt-6-luna"]
    );
    assert_eq!(listed[0].display_name, "GPT-6 Sol");
    let none = world
        .app
        .call(ProviderModelList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert!(
        none.is_empty(),
        "suggestions never change the allowlist by themselves"
    );
    let refused = world
        .app
        .call(ProviderModelSuggestionList, &[&id])
        .token(&c.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["provider.manage"]);
}

#[tokio::test]
async fn a_provider_whose_key_the_server_holds_and_that_has_no_runtime_cannot_be_asked() {
    let world = world().await;
    let c = company(&world).await;
    let id = provider(&world, &c.boss, c.acme, None).await.base.id;
    let unavailable = world
        .app
        .call(ProviderModelSuggestionList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(unavailable.code, ErrorCode::ProviderUnavailable);
    let accept = world
        .app
        .call(ProviderModelSuggestionAccept, &[&id])
        .token(&c.boss.token)
        .body(&ProviderModelSuggestionAcceptBody {
            model_names: vec!["gpt-6-sol".into()],
        })
        .send()
        .await
        .problem();
    assert_eq!(accept.code, ErrorCode::ProviderUnavailable);
}

#[tokio::test]
async fn a_provider_on_a_key_the_server_holds_is_started_on_its_machine_before_that_is_asked_what_it_lists()
 {
    let world = world().await;
    let c = company(&world).await;
    let machine = machine_forwarding_to(&world, c.boss.id(), &["https://api.example.com/v1"]).await;
    // The provider runs on the machine, and the key is the server's: the machine holds nothing of it until
    // the provider is started there with the key sealed to it.
    let id = provider(&world, &c.boss, c.acme, None).await.base.id;
    sqlx::query("UPDATE providers SET machine_id = $2 WHERE id = $1")
        .bind(id)
        .bind(machine)
        .execute(&world.db.pool)
        .await
        .unwrap();
    online(&world, machine).await;
    // The responder acknowledges startup, then gives the model reply to the discovery command.
    let machine_says = answer_discovery(&world, machine, discovered());
    let listed = world
        .app
        .call(ProviderModelSuggestionList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    machine_says.await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|found| found.model_name.as_str())
            .collect::<Vec<_>>(),
        ["gpt-6-sol", "gpt-6-luna"]
    );
    let asked: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM commands WHERE machine_id = $1 ORDER BY created_at")
            .bind(machine)
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        asked,
        ["provider.start", "provider.discover_models"],
        "it was started where it sits before it was asked"
    );
}

#[tokio::test]
async fn accepting_two_reported_names_that_collide_refuses_the_batch_without_losing_either_name() {
    let world = world().await;
    let c = company(&world).await;
    let machine = machine_forwarding_to(&world, c.boss.id(), &["https://api.example.com/v1"]).await;
    let id = provider(&world, &c.boss, c.acme, Some(machine))
        .await
        .base
        .id;
    online(&world, machine).await;
    let machine_says = answer_discovery(
        &world,
        machine,
        json!({"suggestions":[
            {"model_name":"large-a","display_name":"Large model","listed":true,"parameters":{}},
            {"model_name":"large-b","display_name":"Large model","listed":true,"parameters":{}}
        ]}),
    );
    let refused = world
        .app
        .call(ProviderModelSuggestionAccept, &[&id])
        .token(&c.boss.token)
        .body(&ProviderModelSuggestionAcceptBody {
            model_names: vec!["large-a".into(), "large-b".into()],
        })
        .send()
        .await
        .problem();
    machine_says.await.unwrap();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert!(refused.extra["errors"].to_string().contains("model_names"));
    let saved = world
        .app
        .call(ProviderModelList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert!(
        saved.is_empty(),
        "a collision rolls back the whole batch instead of blanking a reported name"
    );
}

#[tokio::test]
async fn accepted_suggestions_are_added_with_what_they_offer_and_none_of_it_enabled() {
    let world = world().await;
    let c = company(&world).await;
    let machine = machine_forwarding_to(&world, c.boss.id(), &["https://api.example.com/v1"]).await;
    let id = provider(&world, &c.boss, c.acme, Some(machine))
        .await
        .base
        .id;
    online(&world, machine).await;
    let accept = |names: &[&str]| ProviderModelSuggestionAcceptBody {
        model_names: names.iter().map(|name| (*name).to_owned()).collect(),
    };
    let invalid = {
        let machine_says = answer_discovery(&world, machine, discovered());
        let reply = world
            .app
            .call(ProviderModelSuggestionAccept, &[&id])
            .token(&c.boss.token)
            .body(&accept(&["gpt-6-sol", "not-suggested"]))
            .send()
            .await
            .problem();
        machine_says.await.unwrap();
        reply
    };
    assert_eq!(invalid.code, ErrorCode::RequestInvalid);
    assert!(
        invalid.extra["errors"]
            .to_string()
            .contains("model_names[1]")
    );
    let machine_says = answer_discovery(&world, machine, discovered());
    let added = world
        .app
        .call(ProviderModelSuggestionAccept, &[&id])
        .token(&c.boss.token)
        .body(&accept(&["gpt-6-sol", "gpt-6-luna"]))
        .send()
        .await
        .ok();
    machine_says.await.unwrap();
    assert_eq!(
        added.provider.models.len(),
        2,
        "acceptance returns the complete native provider without another GET"
    );
    let added = added.added;
    assert_eq!(added.len(), 2);
    let sol = added.iter().find(|m| m.model_name == "gpt-6-sol").unwrap();
    assert_eq!(sol.source, ModelSource::Suggested);
    assert!(sol.enabled);
    assert_eq!(sol.display_name, "GPT-6 Sol");
    assert_eq!(
        sol.parameters.efforts.as_deref(),
        Some(&["low".to_owned(), "high".to_owned()][..])
    );
    assert_eq!(sol.parameters.context_window, Some(400_000));
    assert!(
        sol.parameters.service_tiers.is_none() && sol.parameters.access_programs.is_none(),
        "none enabled"
    );
    let offered = sol.parameters.offered.clone().unwrap();
    assert_eq!(
        offered.service_tiers,
        Some(vec![ServiceTier {
            id: "fast".into(),
            label: "Fast".into()
        }])
    );
    assert_eq!(offered.access_programs, Some(vec!["early".to_owned()]));
    let luna = added.iter().find(|m| m.model_name == "gpt-6-luna").unwrap();
    assert_eq!(
        luna.display_name, "gpt-6-luna",
        "a name reported by the source stays a name even when it equals the routing name"
    );
    let saved = world
        .app
        .call(ProviderModelList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    for expected in &added {
        let persisted = saved.iter().find(|model| model.id == expected.id).unwrap();
        assert_eq!(
            persisted.display_name, expected.display_name,
            "the next read and model picker keep the suggested name"
        );
    }
    // The provider has a key somewhere and a model: it is ready. Accepting again adds nothing.
    let refresh_pool = world.db.pool.clone();
    let refresh_answer = tokio::spawn(async move {
        // The page now has a no-turn fact read; this fixture answers it in addition to model discovery.
        loop {
            sqlx::query("UPDATE commands SET state = 'succeeded',result = $2 WHERE body ->> 'provider_id' = $1
                AND kind IN ('provider.start','provider.refresh') AND state IN ('queued','sent','accepted')")
                .bind(id.to_string()).bind(json!({"status":"ready","facts":{"key_usage_supported":false,"windows":[]}}))
                .execute(&refresh_pool).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    });
    let ready = world
        .app
        .call(ProviderGet, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    refresh_answer.abort();
    assert_eq!(ready.base.status, ProviderStatus::Ready);
    let machine_says = answer_discovery(&world, machine, discovered());
    let again = world
        .app
        .call(ProviderModelSuggestionAccept, &[&id])
        .token(&c.boss.token)
        .body(&accept(&["gpt-6-sol"]))
        .send()
        .await
        .ok();
    machine_says.await.unwrap();
    assert!(again.added.is_empty());
    assert_eq!(again.provider.models.len(), 2);
}
