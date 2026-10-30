//! What a model can be run with at a provider, written on its entry over HTTP: the efforts, the speeds and access programs its managers enabled, the largest context a thread may be given, what a thread starts with of each, and the groups the model is in; each returned as written, refused at the member that is not consistent, and never enabled by accepting a suggestion.
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
    CreateProvider, ModelParameters, ModelSource, Provider, ProviderCreate, ProviderModel,
    ProviderModelCreate, ProviderModelCreateBody, ProviderModelList, ProviderModelSuggestionAccept,
    ProviderModelSuggestionAcceptBody, ProviderModelUpdate, ProviderModelUpdateBody, ServiceTier,
    Upstream,
};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_db::secrets::MasterKey;
use orch_types::Id;
use places::{Who, missing_permissions};
use resources::{answer_discovery, company, machine_forwarding_to, online};
use serde_json::json;
use support::World;

async fn world() -> World {
    install_master_key(MasterKey::new([7; 32]));
    World::new().await
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
            upstreams: vec![Upstream {
                protocol: "openai-responses".to_owned(),
                base_url: "https://api.example.com/v1".to_owned(),
                options: json!({}),
            }],
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

fn tier(id: &str) -> ServiceTier {
    ServiceTier {
        id: id.to_owned(),
        label: id.to_owned(),
    }
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// An entry that has each of the things a model can be run with, all consistent.
fn everything() -> ModelParameters {
    ModelParameters {
        efforts: Some(words(&["low", "high"])),
        default_effort: Some("high".into()),
        service_tiers: Some(vec![tier("fast"), tier("flex")]),
        default_service_tier: Some("fast".into()),
        access_programs: Some(words(&["blue", "red"])),
        default_access_program: Some("blue".into()),
        context_window: Some(272_000),
        context_max: Some(1_000_000),
        default_context: Some(272_000),
        input_images: Some(true),
        ..ModelParameters::default()
    }
}

async fn create(
    world: &World,
    by: &Who,
    provider: Id,
    name: &str,
    parameters: Option<ModelParameters>,
    groups: Option<Vec<String>>,
) -> orch_api_core::testing::Reply<ProviderModel> {
    world
        .app
        .call(ProviderModelCreate, &[&provider])
        .token(&by.token)
        .body(&ProviderModelCreateBody {
            model_name: name.to_owned(),
            display_name: None,
            parameters,
            groups,
        })
        .send()
        .await
}

#[tokio::test]
async fn an_entry_is_written_with_what_the_model_can_be_run_with_and_the_groups_it_is_in_and_read_back_as_written()
 {
    let world = world().await;
    let c = company(&world).await;
    let id = provider(&world, &c.boss, c.acme, None).await.base.id;
    let made = create(
        &world,
        &c.boss,
        id,
        "openai/gpt-6-sol",
        Some(everything()),
        Some(words(&["frontier", "vision", "frontier"])),
    )
    .await
    .ok();
    assert_eq!(made.parameters, everything());
    assert_eq!(made.groups, ["frontier", "vision"], "each group once");
    assert_eq!(made.source, ModelSource::Manual);

    let listed = world
        .app
        .call(ProviderModelList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(listed, std::slice::from_ref(&made));

    // Groups are changed as a whole, without touching what the model can be run with, and the other way round.
    let regrouped = world
        .app
        .call(ProviderModelUpdate, &[&id, &made.id])
        .token(&c.boss.token)
        .body(&ProviderModelUpdateBody {
            groups: Some(words(&["small"])),
            ..ProviderModelUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(regrouped.groups, ["small"]);
    assert_eq!(regrouped.parameters, everything());
    let narrowed = ModelParameters {
        context_max: Some(500_000),
        default_context: Some(272_000),
        ..everything()
    };
    let rewritten = world
        .app
        .call(ProviderModelUpdate, &[&id, &made.id])
        .token(&c.boss.token)
        .body(&ProviderModelUpdateBody {
            parameters: Some(narrowed.clone()),
            ..ProviderModelUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(rewritten.parameters, narrowed);
    assert_eq!(rewritten.groups, ["small"], "left as they were");
    let emptied = world
        .app
        .call(ProviderModelUpdate, &[&id, &made.id])
        .token(&c.boss.token)
        .body(&ProviderModelUpdateBody {
            groups: Some(Vec::new()),
            ..ProviderModelUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert!(emptied.groups.is_empty());

    // Only a manager writes them.
    let refused = create(&world, &c.viewer, id, "other", None, None)
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["provider.manage"]);
}

/// The problem that came of a body, with the member it names.
fn refused_at(reply: &orch_api_contract::common::Problem) -> String {
    assert_eq!(reply.code, ErrorCode::RequestInvalid, "{reply:?}");
    reply.extra["errors"][0]["path"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn an_entry_that_does_not_hold_together_is_refused_at_the_member_and_nothing_is_written() {
    let world = world().await;
    let c = company(&world).await;
    let id = provider(&world, &c.boss, c.acme, None).await.base.id;
    let broken: Vec<(&str, ModelParameters, Option<Vec<String>>)> = vec![
        (
            "parameters.default_effort",
            ModelParameters {
                default_effort: Some("max".into()),
                ..everything()
            },
            None,
        ),
        (
            "parameters.default_service_tier",
            ModelParameters {
                default_service_tier: Some("priority".into()),
                ..everything()
            },
            None,
        ),
        (
            "parameters.default_access_program",
            ModelParameters {
                default_access_program: Some("green".into()),
                ..everything()
            },
            None,
        ),
        (
            "parameters.context_max",
            ModelParameters {
                context_max: Some(100_000),
                default_context: None,
                ..everything()
            },
            None,
        ),
        (
            "parameters.default_context",
            ModelParameters {
                default_context: Some(2_000_000),
                ..everything()
            },
            None,
        ),
        (
            "parameters.default_context",
            ModelParameters {
                context_max: None,
                ..everything()
            },
            None,
        ),
        (
            "groups[0]",
            ModelParameters::default(),
            Some(words(&["Frontier"])),
        ),
        (
            "groups[0]",
            ModelParameters::default(),
            Some(words(&["-small"])),
        ),
        (
            "groups[0]",
            ModelParameters::default(),
            Some(words(&["a b"])),
        ),
        (
            "groups[0]",
            ModelParameters::default(),
            Some(vec!["g".repeat(41)]),
        ),
        (
            "groups",
            ModelParameters::default(),
            Some((0..9).map(|n| format!("g{n}")).collect()),
        ),
    ];
    for (member, parameters, groups) in broken {
        let problem = create(
            &world,
            &c.boss,
            id,
            "gpt-6-sol",
            Some(parameters.clone()),
            groups.clone(),
        )
        .await
        .problem();
        assert_eq!(refused_at(&problem), member, "{parameters:?} {groups:?}");
    }
    let none = world
        .app
        .call(ProviderModelList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert!(none.is_empty(), "nothing was written by any of them");

    // The same refusals when an entry is changed, and the entry stays as it was.
    let made = create(&world, &c.boss, id, "gpt-6-sol", Some(everything()), None)
        .await
        .ok();
    let change = |parameters: Option<ModelParameters>, groups: Option<Vec<String>>| {
        world
            .app
            .call(ProviderModelUpdate, &[&id, &made.id])
            .token(&c.boss.token)
            .body(&ProviderModelUpdateBody {
                parameters,
                groups,
                ..ProviderModelUpdateBody::default()
            })
    };
    // A default is not among what is enabled any more: the speed is switched off and its start is kept.
    let problem = change(
        Some(ModelParameters {
            service_tiers: Some(vec![tier("flex")]),
            ..everything()
        }),
        None,
    )
    .send()
    .await
    .problem();
    assert_eq!(refused_at(&problem), "parameters.default_service_tier");
    let problem = change(None, Some(words(&["NO"]))).send().await.problem();
    assert_eq!(refused_at(&problem), "groups[0]");
    let still = world
        .app
        .call(ProviderModelList, &[&id])
        .token(&c.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(still[0].parameters, everything());
    // Taking the start away first, then the speed, is the way.
    let ok = change(
        Some(ModelParameters {
            service_tiers: Some(vec![tier("flex")]),
            default_service_tier: None,
            ..everything()
        }),
        None,
    )
    .send()
    .await
    .ok();
    assert_eq!(ok.parameters.service_tiers, Some(vec![tier("flex")]));
}

fn discovered() -> serde_json::Value {
    json!({ "suggestions": [
        { "model_name": "gpt-6-sol", "display_name": "GPT-6 Sol", "listed": true,
          "parameters": {
              "efforts": ["low", "high"], "default_effort": "low", "context_window": 272000,
              "offered": { "service_tiers": [{ "id": "fast", "label": "Fast" }], "access_programs": ["early"],
                           "context_max": 1000000 } } },
        { "model_name": "gpt-6-luna", "display_name": "gpt-6-luna", "listed": true,
          "parameters": {
              "context_window": 272000, "context_max": 1000000, "default_context": 400000,
              "service_tiers": [{ "id": "fast", "label": "Fast" }], "default_service_tier": "fast",
              "access_programs": ["early"], "default_access_program": "early" } },
        { "model_name": "gpt-6-same", "display_name": "gpt-6-same", "listed": true,
          "parameters": { "context_window": 272000, "offered": { "context_max": 272000 } } }
    ] })
}

#[tokio::test]
async fn accepting_a_suggestion_puts_a_larger_context_a_speed_and_a_program_on_offer_and_enables_none()
 {
    let world = world().await;
    let c = company(&world).await;
    let machine = machine_forwarding_to(&world, c.boss.id(), &["https://api.example.com/v1"]).await;
    let id = provider(&world, &c.boss, c.acme, Some(machine))
        .await
        .base
        .id;
    online(&world, machine).await;
    let machine_says = answer_discovery(&world, machine, discovered());
    let added = world
        .app
        .call(ProviderModelSuggestionAccept, &[&id])
        .token(&c.boss.token)
        .body(&ProviderModelSuggestionAcceptBody {
            model_names: words(&["gpt-6-sol", "gpt-6-luna", "gpt-6-same"]),
        })
        .send()
        .await
        .ok();
    machine_says.await.unwrap();
    assert_eq!(added.provider.models.len(), 3);
    let added = added.added;
    let get = |name: &str| added.iter().find(|m| m.model_name == name).unwrap().clone();

    let sol = get("gpt-6-sol");
    assert_eq!(sol.parameters.context_window, Some(272_000));
    assert_eq!(
        sol.parameters.context_max, None,
        "a larger context is not open for being offered"
    );
    assert_eq!(sol.parameters.service_tiers, None);
    assert_eq!(sol.parameters.access_programs, None);
    let offered = sol.parameters.offered.clone().unwrap();
    assert_eq!(offered.context_max, Some(1_000_000));
    assert_eq!(
        offered.service_tiers,
        Some(vec![ServiceTier {
            id: "fast".into(),
            label: "Fast".into()
        }]),
        "the speed as its source names it"
    );
    assert_eq!(offered.access_programs, Some(words(&["early"])));
    assert!(sol.groups.is_empty());
    assert_eq!(sol.parameters.default_effort.as_deref(), Some("low"));

    // Whatever a source says a thread starts with, it is not taken: that is a manager's to write.
    let luna = get("gpt-6-luna");
    assert_eq!(
        (
            luna.parameters.context_max,
            luna.parameters.default_context,
            luna.parameters.default_service_tier,
            luna.parameters.default_access_program,
        ),
        (None, None, None, None)
    );
    assert_eq!(
        luna.parameters.offered.unwrap().context_max,
        Some(1_000_000)
    );

    // A context no larger than the window offers nothing.
    assert_eq!(get("gpt-6-same").parameters.offered, None);

    // A manager opens what they choose, and the offer stays beside it.
    let opened = world
        .app
        .call(ProviderModelUpdate, &[&id, &sol.id])
        .token(&c.boss.token)
        .body(&ProviderModelUpdateBody {
            parameters: Some(ModelParameters {
                context_max: Some(1_000_000),
                service_tiers: Some(vec![tier("fast")]),
                default_service_tier: Some("fast".into()),
                ..sol.parameters.clone()
            }),
            ..ProviderModelUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(opened.parameters.context_max, Some(1_000_000));
    assert_eq!(
        opened.parameters.default_service_tier.as_deref(),
        Some("fast")
    );
    assert_eq!(
        opened.parameters.offered.unwrap().context_max,
        Some(1_000_000),
        "what the source said it offers is kept"
    );
}
