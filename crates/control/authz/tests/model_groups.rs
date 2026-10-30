//! A list that says which models something covers names them by name, by family and by a group of the provider's own models: an entry's models, and the models a place allows, are matched against the groups the provider's managers put each model in.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission as P;
use orch_access_contract::roles::Role;
use orch_authz::Caller;
use orch_authz::usable::{self, Author, Policies, TurnAsk};
use orch_types::Id;
use serde_json::json;
use support::World;

/// A provider of `ada` with three models: `big` in the group `frontier`, `mid` in `frontier` and `vision`,
/// `small` in none; `bea` may use it through an entry that names `models`.
async fn setting(models: serde_json::Value) -> (World, Id, Id, Id) {
    let w = World::new().await;
    let (ada, bea) = (w.member("ada").await, w.member("bea").await);
    for person in [ada, bea] {
        w.give(w.project, person, Role::Interactor).await;
    }
    let provider = w.provider(ada, w.root, "Team Pro").await;
    for name in ["big", "mid", "small"] {
        w.model(provider, name, &[]).await;
    }
    for (name, groups) in [
        ("big", vec!["frontier"]),
        ("mid", vec!["frontier", "vision"]),
    ] {
        sqlx::query(
            "UPDATE provider_models SET groups = $1 WHERE provider_id = $2 AND model_name = $3",
        )
        .bind(groups.into_iter().map(str::to_owned).collect::<Vec<_>>())
        .bind(provider)
        .bind(name)
        .execute(&mut *w.conn().await)
        .await
        .unwrap();
    }
    w.put(
        "provider",
        provider,
        P::ProviderUse,
        Holder::User { id: bea },
        json!({ "models": models }),
    )
    .await;
    (w, ada, bea, provider)
}

/// Which of the three models a person runs on a thread, with the place's own list of models where there is one.
async fn runs(
    w: &World,
    who: Id,
    provider: Id,
    place_allows: Option<&[String]>,
) -> Vec<&'static str> {
    let principal = w.principal(Caller::session(who)).await;
    let mut conn = w.conn().await;
    let mut found = Vec::new();
    for model in ["big", "mid", "small"] {
        let decision = usable::check_turn(
            &mut conn,
            Author::Person(&principal),
            &TurnAsk {
                thread: w.thread,
                provider,
                profile: None,
                model,
                effort: None,
                arrangement: None,
                models_allowed: place_allows,
            },
            &Policies::default(),
        )
        .await
        .unwrap();
        if decision.is_allowed() {
            found.push(model);
        }
    }
    found
}

#[tokio::test]
async fn an_entry_that_names_a_group_lets_its_holder_run_the_models_the_providers_managers_put_in_it()
 {
    let (w, ada, bea, provider) = setting(json!(["@frontier"])).await;
    assert_eq!(
        runs(&w, bea, provider, None).await,
        ["big", "mid"],
        "the models of the group, and not the one that is in none"
    );
    assert_eq!(
        runs(&w, ada, provider, None).await,
        ["big", "mid", "small"],
        "whoever owns the provider is not limited by an entry"
    );
}

#[tokio::test]
async fn names_families_and_groups_are_one_kind_of_list_in_an_entry() {
    let (w, _, bea, provider) = setting(json!(["small", "@vision"])).await;
    assert_eq!(runs(&w, bea, provider, None).await, ["mid", "small"]);
    let (w, _, bea, provider) = setting(json!(["b*", "@nothing"])).await;
    assert_eq!(
        runs(&w, bea, provider, None).await,
        ["big"],
        "a family, and a group no model is in, which names nothing"
    );
    let (w, _, bea, provider) = setting(json!(["@Frontier"])).await;
    assert_eq!(
        runs(&w, bea, provider, None).await,
        ["big", "mid"],
        "the name of a group is read in lower case, as every pattern is"
    );
}

#[tokio::test]
async fn the_models_a_place_allows_may_name_a_group_as_well_and_both_lists_have_to_allow_a_model() {
    let (w, _, bea, provider) = setting(json!(["@frontier", "small"])).await;
    let vision = ["@vision".to_owned()];
    assert_eq!(runs(&w, bea, provider, Some(&vision)).await, ["mid"]);
    let family = ["s*".to_owned(), "@frontier".to_owned()];
    assert_eq!(
        runs(&w, bea, provider, Some(&family)).await,
        ["big", "mid", "small"]
    );
    let narrow = ["small".to_owned()];
    assert_eq!(runs(&w, bea, provider, Some(&narrow)).await, ["small"]);
}
