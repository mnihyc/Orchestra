//! What a thread may be put on, told to the person who manages it: every provider they can see with whether they may use it and why not, the profiles that run on it with every arrangement and whose decision it is when one is not allowed, the environments they manage, the defaults, and what a thread made here starts with.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod binding_support;
#[path = "binding_options/native_scopes.rs"]
mod native_scopes;
mod places;
mod rig;
mod support;
mod thread_history_scope;

use binding_support::{
    a_directory_on_a_machine, another_environment, another_provider, names, offering_plain,
};
use orch_api_contract::binding::{
    ArrangementType, BindingOptions, BindingProvider, EnvironmentUse, HarnessPlace,
    NodeBindingOptionsGet, NodeBindingOptionsGetQuery, ThreadBindingOptionsGet,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::settings::ModelOptionReason;
use orch_settings::Target;
use orch_types::Id;
use places::{Who, join};
use rig::{MODEL, Scene};
use serde_json::json;

#[tokio::test]
async fn a_successful_native_start_remembers_its_resolved_model_on_the_server_for_only_its_maker() {
    use orch_api_contract::personal::{
        MeModelRecentCreate, MeModelRecentList, MeStateGet, ModelRecent,
    };
    let s = Scene::new().await;
    let thread = s.thread().await;
    let state = s
        .rig
        .world
        .app
        .call(MeStateGet, &[])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let saved = &state["thread_setup"];
    assert_eq!(saved["place"], json!(s.tree.topic.base.id));
    assert_eq!(saved["picks"]["provider"], json!(s.on.provider));
    assert_eq!(saved["picks"]["agent"], json!(s.on.agent));
    assert_eq!(saved["picks"]["model"], json!(thread.model));
    assert_eq!(
        saved["picks"]["environment"],
        json!(thread.environment.unwrap().id)
    );
    let recent = s
        .rig
        .world
        .app
        .call(MeModelRecentList, &[])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(recent.recent[0].provider_id, s.on.provider);
    assert_eq!(recent.recent[0].model_name, MODEL);
    s.rig
        .world
        .app
        .call(MeModelRecentCreate, &[])
        .token(&s.ada.token)
        .body(&ModelRecent {
            provider_id: orch_types::new_id(),
            model_name: "another-choice".into(),
        })
        .send()
        .await
        .ok();
    s.say(
        &s.ada,
        thread.base.id,
        "Use the current model again",
        orch_api_contract::threads::Delivery::Queue,
        "reuse-current-model",
    )
    .await
    .ok();
    s.rig.turn_start(thread.base.id, 0).await;
    let used = s
        .rig
        .world
        .app
        .call(MeModelRecentList, &[])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(used.recent[0].provider_id, s.on.provider);
    assert_eq!(
        used.recent[0].model_name, MODEL,
        "native work updates recency without opening or changing the picker"
    );
    let private = s
        .rig
        .world
        .app
        .call(MeStateGet, &[])
        .token(&s.cai.token)
        .send()
        .await
        .ok();
    assert!(!private.contains_key("thread_setup"));
    assert!(
        s.rig
            .world
            .app
            .call(MeModelRecentList, &[])
            .token(&s.cai.token)
            .send()
            .await
            .ok()
            .recent
            .is_empty()
    );
}

async fn options_for(s: &Scene, who: &Who, thread: Id) -> BindingOptions {
    s.rig
        .world
        .app
        .call(ThreadBindingOptionsGet, &[&thread])
        .token(&who.token)
        .send()
        .await
        .ok()
}

fn provider_of(options: &BindingOptions, id: Id) -> &BindingProvider {
    options
        .providers
        .iter()
        .find(|each| each.provider.id == id)
        .expect("the provider is listed")
}

fn kinds(provider: &BindingProvider) -> Vec<ArrangementType> {
    provider.agents[0]
        .arrangements
        .iter()
        .map(|each| each.kind)
        .collect()
}

#[tokio::test]
async fn binding_options_show_resolved_source_names_and_the_place_that_supplies_the_repository() {
    let s = Scene::new().await;
    let org = s.tree.org.base.id;
    s.rig
        .world
        .set_at(
            Target::Node(org),
            "source.repository_url",
            json!("https://code.example.test/team/repository.git"),
        )
        .await;
    s.rig
        .world
        .set_at(Target::Node(org), "source.branch", json!("release/next"))
        .await;
    s.rig
        .world
        .set_at(
            Target::Node(org),
            "source.secret_name",
            // The name of a secret is written in capitals, digits and underscores.
            json!("REPOSITORY_TOKEN"),
        )
        .await;
    let thread = s.thread().await.base.id;
    let options = options_for(&s, &s.ada, thread).await;
    assert_eq!(
        options.source.repository.repository_url.as_deref(),
        Some("https://code.example.test/team/repository.git")
    );
    assert_eq!(
        options.source.repository.branch.as_deref(),
        Some("release/next")
    );
    assert_eq!(
        options.source.repository.secret_name.as_deref(),
        Some("REPOSITORY_TOKEN")
    );
    assert_eq!(
        options.source.from.as_ref().map(|place| place.id),
        Some(org)
    );
    assert_eq!(
        serde_json::to_value(&options.source).unwrap()["repository_url"],
        "https://code.example.test/team/repository.git"
    );
}

#[tokio::test]
async fn an_embedding_only_model_is_absent_from_a_threads_generation_choices() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    sqlx::query("UPDATE provider_models SET parameters=parameters || $2 WHERE provider_id=$1")
        .bind(s.on.provider)
        .bind(json!({"purposes":["embedding"]}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let options = options_for(&s, &s.ada, thread).await;
    assert!(
        provider_of(&options, s.on.provider)
            .models
            .iter()
            .all(|model| model.model_name != MODEL)
    );
}

#[tokio::test]
async fn the_manager_of_a_thread_is_told_the_providers_they_may_use_with_their_models_and_the_ways_an_agent_can_be_placed()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let options = options_for(&s, &s.ada, thread).await;
    assert_eq!(options.providers.len(), 1);
    let provider = provider_of(&options, s.on.provider);
    assert!(provider.usable && provider.reason.is_none());
    assert_eq!(provider.models.len(), 1);
    assert_eq!(provider.models[0].model_name, MODEL);
    assert!(provider.models[0].allowed);
    assert_eq!(
        provider.models[0].parameters.efforts.as_deref(),
        Some(&["low".to_owned(), "high".to_owned()][..]),
        "model and effort are chosen in one control"
    );
    assert_eq!(provider.agents.len(), 1);
    let agent = &provider.agents[0];
    assert_eq!(agent.agent.id, s.on.agent);
    assert!(agent.usable);

    // Together on the one machine is what a binding that names nothing takes; the way through
    // Orch's executor cannot be had while the environment offers nothing, and says so.
    assert_eq!(
        kinds(provider),
        [ArrangementType::Together, ArrangementType::Separated]
    );
    let together = &agent.arrangements[0];
    assert_eq!(together.rank, 1);
    assert!(together.available && together.permitted && together.reason.is_none());
    assert_eq!(
        together.isolation.harness_runs_in,
        HarnessPlace::Environment
    );
    let separated = &agent.arrangements[1];
    assert_eq!(separated.rank, 2);
    assert!(!separated.available);
    assert_eq!(separated.reason.as_deref(), Some("executor_missing"));
    assert!(
        separated
            .detail
            .as_deref()
            .is_some_and(|text| !text.is_empty()),
        "the dialog is given a sentence for the person"
    );
    assert_eq!(separated.isolation.harness_runs_in, HarnessPlace::Provider);
}

#[tokio::test]
async fn native_choices_stay_available_when_another_served_protocol_has_no_forwarder_and_fresh_learning_keeps_its_path()
 {
    use orch_agent_contract::arrangement::Arrangement;
    use orch_api_contract::binding::{CredentialPath, Metering, ModelEnforcement};
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    sqlx::query("UPDATE providers SET protocols=ARRAY['unused-translator'] WHERE id=$1")
        .bind(s.on.provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET roles=ARRAY['agent','environment'] WHERE id=$1")
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let options = options_for(&s, &s.ada, thread).await;
    let shown = provider_of(&options, s.on.provider);
    let native = shown
        .agents
        .iter()
        .find(|agent| agent.agent.id == s.on.agent)
        .unwrap()
        .arrangements
        .iter()
        .find(|choice| choice.kind == ArrangementType::Together)
        .unwrap();
    assert!(
        native.available && native.permitted,
        "the unrelated forwarding service does not disable the native choice: {native:?}"
    );
    assert_eq!(native.isolation.credential_path, CredentialPath::Native);
    assert_eq!(
        native.isolation.model_enforcement,
        ModelEnforcement::Adapter
    );
    assert_eq!(native.isolation.metering, Metering::Harness);
    // Setup can learn this selection before the older profile snapshot has its descriptor.
    let mut conn = s.rig.db.acquire().await.unwrap();
    let provider = orch_db::queries::resources::thread_engine::provider(&mut conn, s.on.provider)
        .await
        .unwrap()
        .unwrap();
    let mut agent = orch_db::queries::resources::thread_engine::agent(&mut conn, s.on.agent)
        .await
        .unwrap()
        .unwrap();
    agent.descriptor = json!({});
    let selected = Arrangement {
        kind: orch_agent_contract::descriptor::ArrangementType::Together,
        executor: None,
        tool_set: None,
        form: None,
        protocol: None,
    };
    assert_eq!(
        orch_engine::binding::selected_path(&selected, &provider, &agent),
        orch_agent_contract::credentials::CredentialPath::Native
    );
    assert_eq!(
        s.rig.commands.issued_of("turn.start"),
        0,
        "reading choices starts no turn"
    );
}

#[tokio::test]
async fn an_environment_that_offers_what_the_harness_can_work_through_makes_the_way_through_the_executor_available()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    offering_plain(&s, s.on.environment).await;
    let options = options_for(&s, &s.ada, thread).await;
    let agent = &provider_of(&options, s.on.provider).agents[0];
    let separated = &agent.arrangements[1];
    assert!(separated.available && separated.permitted);
    assert_eq!(separated.executor.as_deref(), Some("orch-exec"));
    assert_eq!(separated.tool_set.as_deref(), Some("plain"));
    assert_eq!(separated.rank, 2, "the adapter's order decides");
}

#[tokio::test]
async fn a_provider_the_caller_sees_but_may_not_use_is_listed_with_the_reason() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    // Dan may manage the thread, and is let look at the provider only.
    names(&s, "node", thread, "thread.manage", &s.dan, json!({})).await;
    names(
        &s,
        "provider",
        s.on.provider,
        "provider.view",
        &s.dan,
        json!({}),
    )
    .await;
    names(&s, "agent", s.on.agent, "agent.view", &s.dan, json!({})).await;
    let options = options_for(&s, &s.dan, thread).await;
    let provider = provider_of(&options, s.on.provider);
    assert!(!provider.usable);
    assert_eq!(provider.reason.as_deref(), Some("no_use"));
    assert!(!provider.agents[0].usable);
    assert_eq!(provider.agents[0].reason.as_deref(), Some("no_use"));
}

#[tokio::test]
async fn a_provider_nobody_named_the_caller_on_is_not_listed_at_all() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    // Bea is no member of the org: the thread's page names her, and nothing else does.
    names(&s, "node", thread, "thread.manage", &s.bea, json!({})).await;
    let options = options_for(&s, &s.bea, thread).await;
    assert!(
        options.providers.is_empty(),
        "what a person cannot see they are not told exists"
    );
    assert!(options.environments.is_empty());
}

#[tokio::test]
async fn a_switched_off_provider_is_listed_as_switched_off_and_its_owner_is_told_why_it_cannot_be_chosen()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    sqlx::query("UPDATE providers SET status = 'disabled' WHERE id = $1")
        .bind(s.on.provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let options = options_for(&s, &s.ada, thread).await;
    let provider = provider_of(&options, s.on.provider);
    assert!(!provider.usable);
    assert_eq!(provider.reason.as_deref(), Some("disabled"));
}

#[tokio::test]
async fn an_ask_only_share_names_its_purpose_beside_the_unavailable_thread_model() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let fay = s.rig.person("fay").await;
    let org = s.tree.org.base.id;
    join(&s.rig.world, org, org, &fay, "developer", &s.ada).await;
    names(&s, "node", thread, "thread.manage", &fay, json!({})).await;
    names(&s, "agent", s.on.agent, "agent.use", &fay, json!({})).await;
    names(
        &s,
        "provider",
        s.on.provider,
        "provider.use",
        &fay,
        json!({"purposes":["ask"]}),
    )
    .await;
    let options = options_for(&s, &fay, thread).await;
    let model = &provider_of(&options, s.on.provider).models[0];
    assert!(!model.allowed);
    assert_eq!(model.reason, Some(ModelOptionReason::SharedForAskOnly));
}

#[tokio::test]
async fn the_models_an_entry_leaves_out_are_listed_as_left_out_by_it() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let fay = s.rig.person("fay").await;
    let org = s.tree.org.base.id;
    join(&s.rig.world, org, org, &fay, "developer", &s.ada).await;
    names(&s, "node", thread, "thread.manage", &fay, json!({})).await;
    names(&s, "agent", s.on.agent, "agent.use", &fay, json!({})).await;
    names(
        &s,
        "provider",
        s.on.provider,
        "provider.use",
        &fay,
        json!({ "models": ["another-model"] }),
    )
    .await;
    let options = options_for(&s, &fay, thread).await;
    let model = &provider_of(&options, s.on.provider).models[0];
    assert!(!model.allowed);
    assert_eq!(model.reason, Some(ModelOptionReason::ExcludedByEntry));
}

#[tokio::test]
async fn a_provider_whose_entry_leaves_the_environment_out_is_listed_as_not_allowed_there() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let fay = s.rig.person("fay").await;
    let org = s.tree.org.base.id;
    join(&s.rig.world, org, org, &fay, "developer", &s.ada).await;
    names(&s, "node", thread, "thread.manage", &fay, json!({})).await;
    names(&s, "agent", s.on.agent, "agent.use", &fay, json!({})).await;
    let elsewhere = another_environment(&s, "Elsewhere", None).await;
    names(
        &s,
        "provider",
        s.on.provider,
        "provider.use",
        &fay,
        json!({ "where": [{ "kind": "environment", "id": elsewhere }] }),
    )
    .await;
    let options = options_for(&s, &fay, thread).await;
    let provider = provider_of(&options, s.on.provider);
    assert!(!provider.usable, "the thread runs in another environment");
    assert_eq!(provider.reason.as_deref(), Some("not_allowed_where"));
    // Its owner uses it through no entry that narrows.
    let for_ada = options_for(&s, &s.ada, thread).await;
    assert!(provider_of(&for_ada, s.on.provider).usable);
}

#[tokio::test]
async fn a_provider_whose_entry_names_the_environment_is_listed_as_usable_there() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let fay = s.rig.person("fay").await;
    let org = s.tree.org.base.id;
    join(&s.rig.world, org, org, &fay, "developer", &s.ada).await;
    names(&s, "node", thread, "thread.manage", &fay, json!({})).await;
    names(&s, "agent", s.on.agent, "agent.use", &fay, json!({})).await;
    names(
        &s,
        "provider",
        s.on.provider,
        "provider.use",
        &fay,
        json!({ "where": [{ "kind": "environment", "id": s.on.environment }] }),
    )
    .await;
    let options = options_for(&s, &fay, thread).await;
    let provider = provider_of(&options, s.on.provider);
    assert!(provider.usable && provider.reason.is_none(), "{provider:?}");
}

#[tokio::test]
async fn a_policy_of_the_place_that_leaves_models_out_is_told_as_a_policy() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.org.base.id),
            "models.allowed",
            json!(["something-else"]),
        )
        .await;
    let options = options_for(&s, &s.ada, thread).await;
    let model = &provider_of(&options, s.on.provider).models[0];
    assert_eq!(model.reason, Some(ModelOptionReason::ExcludedByPolicy));
}

#[tokio::test]
async fn an_arrangement_the_providers_owner_has_not_allowed_says_it_is_the_providers_decision() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    offering_plain(&s, s.on.environment).await;
    let (own_profile, closed) = another_provider(&s, "Closed", &["key_separated"]).await;
    let options = options_for(&s, &s.ada, thread).await;
    // The profile that holds the provider's sign-in: another profile would need a copy of the
    // credential before it could be separated from it at all.
    let agent = provider_of(&options, closed)
        .agents
        .iter()
        .find(|each| each.agent.id == own_profile)
        .expect("the provider's own profile is listed");
    let separated = &agent.arrangements[1];
    assert_eq!(separated.kind, ArrangementType::Separated);
    assert!(separated.available, "{separated:?}");
    assert!(!separated.permitted);
    assert_eq!(separated.reason.as_deref(), Some("arrangement_not_allowed"));
    assert_eq!(
        separated.not_allowed_by,
        Some(orch_api_contract::binding::NotAllowedBy::Provider)
    );
    assert!(separated.scope.is_none(), "only a policy has a place");
    // What cannot be taken comes after what can, whatever the adapter's order.
    assert_eq!(separated.rank, 2);
    assert_eq!(agent.arrangements[0].rank, 1);
}

#[tokio::test]
async fn an_entry_that_narrows_the_arrangements_says_it_is_the_entrys_decision_to_the_person_it_names()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    offering_plain(&s, s.on.environment).await;
    let fay = s.rig.person("fay").await;
    let org = s.tree.org.base.id;
    join(&s.rig.world, org, org, &fay, "developer", &s.ada).await;
    names(&s, "node", thread, "thread.manage", &fay, json!({})).await;
    names(&s, "agent", s.on.agent, "agent.use", &fay, json!({})).await;
    names(
        &s,
        "provider",
        s.on.provider,
        "provider.use",
        &fay,
        json!({ "arrangements": ["handed_over"] }),
    )
    .await;
    let for_fay = options_for(&s, &fay, thread).await;
    let separated = &provider_of(&for_fay, s.on.provider).agents[0].arrangements[1];
    assert!(!separated.permitted);
    assert_eq!(
        separated.not_allowed_by,
        Some(orch_api_contract::binding::NotAllowedBy::Entry)
    );
    // The owner uses the provider through no entry, and is held by none.
    let for_ada = options_for(&s, &s.ada, thread).await;
    assert!(provider_of(&for_ada, s.on.provider).agents[0].arrangements[1].permitted);
}

#[tokio::test]
async fn a_policy_that_leaves_an_arrangement_out_names_the_place_that_set_it() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    offering_plain(&s, s.on.environment).await;
    let org = s.tree.org.base.id;
    s.rig
        .world
        .set_at(Target::Node(org), "binding.allowed", json!(["together"]))
        .await;
    let options = options_for(&s, &s.ada, thread).await;
    let separated = &provider_of(&options, s.on.provider).agents[0].arrangements[1];
    assert!(separated.available && !separated.permitted);
    assert_eq!(
        separated.not_allowed_by,
        Some(orch_api_contract::binding::NotAllowedBy::Policy)
    );
    assert_eq!(
        separated.scope.as_ref().map(|place| place.id),
        Some(org),
        "the place that set the policy is told"
    );
    assert!(
        provider_of(&options, s.on.provider).agents[0].arrangements[0].permitted,
        "together is asked of the policy alone"
    );
}

#[tokio::test]
async fn the_machine_that_keeps_the_credential_has_a_list_of_its_own_that_is_asked_as_well() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    offering_plain(&s, s.on.environment).await;
    sqlx::query(
        "UPDATE machines SET facts = jsonb_set(facts, '{policy,provider.arrangements}', jsonb_build_array('key_separated'))
          WHERE id = $1",
    )
    .bind(s.rig.machine_id)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let options = options_for(&s, &s.ada, thread).await;
    let separated = &provider_of(&options, s.on.provider).agents[0].arrangements[1];
    assert!(!separated.permitted);
    assert_eq!(
        separated.not_allowed_by,
        Some(orch_api_contract::binding::NotAllowedBy::Machine),
        "the owner of the machine said no, whoever else says yes"
    );
}

#[tokio::test]
async fn what_the_settings_choose_when_nothing_is_said_is_told_with_whether_a_lock_holds_it() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let org = s.tree.org.base.id;
    let before = options_for(&s, &s.ada, thread).await;
    assert_eq!(before.defaults.provider_id, None);
    assert_eq!(
        serde_json::to_value(before.defaults.locks).unwrap(),
        json!({"provider":false,"agent":false,"model":false,"effort":false})
    );
    s.rig
        .world
        .set_at(
            Target::Node(org),
            "agent.default_provider_id",
            json!(s.on.provider),
        )
        .await;
    s.rig
        .world
        .set_at(
            Target::Node(org),
            "agent.default_agent_id",
            json!(s.on.agent),
        )
        .await;
    s.rig
        .world
        .set_at(Target::Node(org), "agent.model", json!(MODEL))
        .await;
    let after = options_for(&s, &s.ada, thread).await;
    assert_eq!(after.defaults.provider_id, Some(s.on.provider));
    assert_eq!(after.defaults.agent_id, Some(s.on.agent));
    assert_eq!(after.defaults.model_name.as_deref(), Some(MODEL));
}

#[tokio::test]
async fn the_environments_the_caller_manages_are_listed_with_where_they_are_used_the_topics_own_first()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let spare = another_environment(&s, "Spare", None).await;
    let options = options_for(&s, &s.ada, thread).await;
    let ids: Vec<Id> = options
        .environments
        .iter()
        .map(|each| each.environment.id)
        .collect();
    assert_eq!(
        ids,
        [s.on.environment, spare],
        "the one this topic's thread runs in comes first"
    );
    let own = &options.environments[0];
    assert_eq!(own.used, EnvironmentUse::Topic);
    assert_eq!(own.threads, 1);
    assert!(own.usable && own.reason.is_none());
    assert_eq!(
        own.directory, "web",
        "a thread joins the directory of the one it joins"
    );
    assert_eq!(own.topics.len(), 1);
    assert_eq!(own.topics[0].id, s.tree.topic.base.id);
    assert_eq!(own.topics_hidden, 0);
    let unused = &options.environments[1];
    assert_eq!(unused.used, EnvironmentUse::Unused);
    assert_eq!(unused.threads, 0);
    assert_eq!(unused.directory, "");
    assert!(!own.is_default && !unused.is_default);

    sqlx::query("INSERT INTO settings (node_id,key,value) VALUES ($1,'env.default_id',to_jsonb($2::uuid::text)) ON CONFLICT (node_id,key) DO UPDATE SET value=excluded.value")
        .bind(s.tree.topic.base.id)
        .bind(spare)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let options = options_for(&s, &s.ada, thread).await;
    assert_eq!(
        options.environments[0].environment.id, spare,
        "the topic's default comes before the rest"
    );
    assert!(options.environments[0].is_default);
}

#[tokio::test]
async fn a_failed_environment_is_not_offered_as_the_topics_default_even_when_the_setting_still_names_it()
 {
    let s = Scene::new().await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "env.default_id",
            json!(s.on.environment),
        )
        .await;
    sqlx::query("UPDATE environments SET status='failed' WHERE id=$1")
        .bind(s.on.environment)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let shown = s
        .rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .query(&NodeBindingOptionsGetQuery::default())
        .send()
        .await
        .ok();
    let environment = shown
        .environments
        .iter()
        .find(|row| row.environment.id == s.on.environment)
        .unwrap();
    assert_eq!(
        environment.environment.status,
        orch_api_contract::environment::EnvironmentStatus::Failed
    );
    assert!(!environment.usable);
    assert!(!environment.is_default);
    assert_eq!(environment.reason.as_deref(), Some("environment_failed"));
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT value FROM settings WHERE node_id=$1 AND key='env.default_id'")
            .bind(s.tree.topic.base.id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        stored,
        json!(s.on.environment),
        "a read must not clear or replace the person's default"
    );
    let refused = s
        .rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .query(&NodeBindingOptionsGetQuery {
            environment_id: Some(s.on.environment),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::EnvironmentNoneUsable,
        "an explicit stale choice cannot preview a different environment"
    );
}

/// What starting a thread in `node` is offered, read as Ada reads it before any thread exists.
async fn start_options_for(s: &Scene, node: Id) -> BindingOptions {
    s.rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&node])
        .token(&s.ada.token)
        .query(&NodeBindingOptionsGetQuery::default())
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_default_environment_set_on_the_project_is_the_default_of_its_topics_and_names_where_it_is_set()
 {
    let s = Scene::new().await;
    let spare = another_environment(&s, "Spare", None).await;
    let project = s.tree.project.base.id;
    let topic = s.tree.topic.base.id;
    s.rig
        .world
        .set_at(Target::Node(project), "env.default_id", json!(spare))
        .await;

    let in_topic = start_options_for(&s, topic).await;
    let row = in_topic
        .environments
        .iter()
        .find(|row| row.environment.id == spare)
        .unwrap();
    assert!(
        row.is_default,
        "the nearest place that sets the default decides, as for every other setting"
    );
    let named = in_topic.default_environment.as_ref().unwrap();
    assert_eq!(named.environment, spare);
    assert_eq!(
        named.from.as_ref().map(|node| node.id),
        Some(project),
        "the person is told which place above the topic set it"
    );
    assert!(named.reason.is_none());

    let in_project = start_options_for(&s, project).await;
    assert!(
        in_project
            .environments
            .iter()
            .find(|row| row.environment.id == spare)
            .unwrap()
            .is_default,
        "the first thread of a topic made in the project starts on the project's default"
    );

    s.rig
        .world
        .set_at(
            Target::Node(topic),
            "env.default_id",
            json!(s.on.environment),
        )
        .await;
    let own = start_options_for(&s, topic).await;
    let named = own.default_environment.as_ref().unwrap();
    assert_eq!(
        named.environment, s.on.environment,
        "the topic's own setting wins over its project's"
    );
    assert!(named.from.is_none(), "the topic's own needs no place named");
    assert!(
        own.environments
            .iter()
            .find(|row| row.environment.id == s.on.environment)
            .unwrap()
            .is_default
    );
    assert!(
        !own.environments
            .iter()
            .find(|row| row.environment.id == spare)
            .unwrap()
            .is_default
    );
}

#[tokio::test]
async fn a_default_environment_the_caller_cannot_use_is_named_with_why_and_is_not_offered_as_the_default()
 {
    let s = Scene::new().await;
    let project = s.tree.project.base.id;
    s.rig
        .world
        .set_at(
            Target::Node(project),
            "env.default_id",
            json!(s.on.environment),
        )
        .await;
    sqlx::query("UPDATE environments SET status='failed' WHERE id=$1")
        .bind(s.on.environment)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let shown = start_options_for(&s, s.tree.topic.base.id).await;
    let named = shown.default_environment.as_ref().unwrap();
    assert_eq!(named.environment, s.on.environment);
    assert_eq!(named.from.as_ref().map(|node| node.id), Some(project));
    assert_eq!(
        named.reason.as_deref(),
        Some("environment_failed"),
        "the dialog can say why it did not start on the default"
    );
    assert!(shown.environments.iter().all(|row| !row.is_default));

    sqlx::query("DELETE FROM settings WHERE node_id=$1 AND key='env.default_id'")
        .bind(project)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let cleared = start_options_for(&s, s.tree.topic.base.id).await;
    assert!(
        cleared.default_environment.is_none(),
        "no setting, no default named"
    );
}

#[tokio::test]
async fn an_environment_that_has_served_only_other_topics_gives_a_new_directory_named_after_this_one()
 {
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let elsewhere = another_environment(&s, "Billing box", Some(s.rig.machine_id)).await;
    let other_topic =
        places::make_topic(&s.rig.world, &s.ada, s.tree.project.base.id, "Billing").await;
    s.rig
        .world
        .app
        .call(ThreadCreate, &[&other_topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            name: Some("Invoices".to_owned()),
            binding: Some(CreateThreadBinding::provider(
                s.on.agent,
                s.on.provider,
                None,
            )),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::existing(elsewhere)),
            directory: Some("invoices".to_owned()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    let options = options_for(&s, &s.ada, thread).await;
    let listed = options
        .environments
        .iter()
        .find(|each| each.environment.id == elsewhere)
        .expect("listed");
    assert_eq!(
        listed.used,
        EnvironmentUse::Project,
        "another topic of the project"
    );
    assert_eq!(listed.threads, 1);
    assert_eq!(
        listed.directory, "login",
        "a directory named after the topic the thread is in"
    );
}

#[tokio::test]
async fn an_environment_whose_removal_is_scheduled_is_listed_as_not_to_be_chosen() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let doomed = another_environment(&s, "Doomed", None).await;
    sqlx::query("UPDATE environments SET remove_at = now() WHERE id = $1")
        .bind(doomed)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let options = options_for(&s, &s.ada, thread).await;
    let listed = options
        .environments
        .iter()
        .find(|each| each.environment.id == doomed)
        .expect("listed");
    assert!(!listed.usable);
    assert_eq!(listed.reason.as_deref(), Some("being_removed"));
}

#[tokio::test]
async fn visible_environments_without_manage_are_listed_but_not_offered_for_a_move() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let theirs = a_directory_on_a_machine(&s, "Ada's laptop").await;
    names(&s, "node", thread, "thread.manage", &s.eve, json!({})).await;
    let options = options_for(&s, &s.eve, thread).await;
    let ids: Vec<Id> = options
        .environments
        .iter()
        .map(|each| each.environment.id)
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&s.on.environment));
    assert!(ids.contains(&theirs));
    let other = options
        .environments
        .iter()
        .find(|row| row.environment.id == theirs)
        .unwrap();
    assert!(!other.usable);
    assert_eq!(other.reason.as_deref(), Some("environment_manage_required"));
}

#[tokio::test]
async fn an_environment_with_use_alone_can_start_work_but_cannot_receive_a_moved_thread() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let directory = a_directory_on_a_machine(&s, "Shared directory").await;
    names(&s, "node", thread, "thread.manage", &s.eve, json!({})).await;
    names(
        &s,
        "environment",
        directory,
        "environment.use",
        &s.eve,
        json!({}),
    )
    .await;

    let start = s
        .rig
        .world
        .app
        .call(NodeBindingOptionsGet, &[&s.tree.topic.base.id])
        .token(&s.eve.token)
        .query(&NodeBindingOptionsGetQuery::default())
        .send()
        .await
        .ok();
    let offered = start
        .environments
        .iter()
        .find(|row| row.environment.id == directory)
        .unwrap();
    assert!(offered.usable, "Use permits starting a new thread there");

    let moved = options_for(&s, &s.eve, thread).await;
    let offered = moved
        .environments
        .iter()
        .find(|row| row.environment.id == directory)
        .unwrap();
    assert!(!offered.usable, "moving an existing thread requires Manage");
    assert_eq!(
        offered.reason.as_deref(),
        Some("environment_manage_required")
    );
}

#[tokio::test]
async fn what_a_new_thread_starts_with_is_told_from_the_defaults_of_the_topic() {
    use orch_api_contract::access::{Holder, Role};
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let options = options_for(&s, &s.ada, thread).await;
    let roles = |holders: &[Holder]| -> Vec<Role> {
        holders
            .iter()
            .filter_map(|holder| match holder {
                Holder::Role { role, .. } => Some(*role),
                _ => None,
            })
            .collect()
    };
    // As the defaults ship: in an environment made from a template Viewers watch, Interactors drive
    // the agent and Developers set it up; on a person's machine everybody who can see it watches and
    // nobody but its owner drives it.
    let managed = &options.access.managed;
    assert_eq!(roles(&managed.view), [Role::Viewer]);
    assert_eq!(roles(&managed.interact), [Role::Interactor]);
    assert_eq!(roles(&managed.manage), [Role::Developer]);
    assert!(!managed.custom);
    let connected = &options.access.connected;
    assert_eq!(roles(&connected.view), [Role::Viewer]);
    assert!(connected.interact.is_empty() && connected.manage.is_empty());
    // An environment made now gives View to Viewers, Use to Interactors and Manage to Developers.
    assert_eq!(roles(&options.new_environment.view), [Role::Viewer]);
    assert_eq!(roles(&options.new_environment.interact), [Role::Interactor]);
    assert_eq!(roles(&options.new_environment.manage), [Role::Developer]);
}

#[tokio::test]
async fn somebody_who_has_no_machine_and_may_use_no_host_may_make_no_environment_and_somebody_with_a_machine_may()
 {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let options = options_for(&s, &s.ada, thread).await;
    assert!(!options.can_create.allowed);
    let mut conn = s.rig.db.acquire().await.unwrap();
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut conn,
        s.ada.id(),
        "laptop",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE machines SET roles = ARRAY['environment'], facts = jsonb_build_object('policy', jsonb_build_object('environment.roots', jsonb_build_array('/work'))) WHERE id = $1")
        .bind(machine).execute(&mut *conn).await.unwrap();
    drop(conn);
    let options = options_for(&s, &s.ada, thread).await;
    assert!(
        options.can_create.allowed,
        "a directory of a machine of their own is an environment they can make"
    );
}

#[tokio::test]
async fn somebody_who_does_not_manage_the_thread_is_refused_with_what_is_missing() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    for who in [&s.cai, &s.bea] {
        let problem = s
            .rig
            .world
            .app
            .call(ThreadBindingOptionsGet, &[&thread])
            .token(&who.token)
            .send()
            .await
            .problem();
        assert!(
            problem.status == 403 || problem.status == 404,
            "{} {}",
            problem.status,
            problem.code
        );
    }
    let viewer = s
        .rig
        .world
        .app
        .call(ThreadBindingOptionsGet, &[&thread])
        .token(&s.cai.token)
        .send()
        .await
        .problem();
    assert_eq!(viewer.code, ErrorCode::AuthzDenied);
    assert!(places::missing_permissions(&viewer).contains(&"thread.manage".to_owned()));
}

#[tokio::test]
async fn a_subscription_is_offered_to_its_first_owner_alone_unless_the_org_shares_subscriptions_of_its_harness()
 {
    let s = Scene::new().await;
    let org = s.tree.org.base.id;
    let eves = s.thread_by(&s.eve, "Eve's").await.base.id;
    let adas = s.thread().await.base.id;
    // Left unset, the policy shares every kind as the page of the provider says: Eve uses Ada's.
    assert!(provider_of(&options_for(&s, &s.eve, eves).await, s.on.provider).usable);
    // The org shares no kind: the subscription stays with Ada, whose it is first.
    s.rig
        .world
        .set_at(
            Target::Node(org),
            "providers.subscription_sharing",
            json!([]),
        )
        .await;
    let kept = options_for(&s, &s.eve, eves).await;
    let provider = provider_of(&kept, s.on.provider);
    assert!(!provider.usable);
    assert_eq!(provider.reason.as_deref(), Some("subscription_not_shared"));
    assert!(
        provider_of(&options_for(&s, &s.ada, adas).await, s.on.provider).usable,
        "the first owner is offered it still"
    );
    // The org shares the kind of this one: Eve is offered it again.
    s.rig
        .world
        .set_at(
            Target::Node(org),
            "providers.subscription_sharing",
            json!(["fake"]),
        )
        .await;
    assert!(provider_of(&options_for(&s, &s.eve, eves).await, s.on.provider).usable);
}

#[tokio::test]
async fn grouping_uses_the_same_native_arrangements_and_preserves_unavailable_choices() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let before = options_for(&s, &s.ada, thread).await;
    let original = provider_of(&before, s.on.provider);
    assert_eq!(
        original.owner.as_ref().map(|owner| owner.id),
        Some(s.ada.id())
    );
    assert_eq!(
        original.agents[0].owner.as_ref().map(|owner| owner.id),
        Some(s.ada.id())
    );

    assert_eq!(
        original.grouping.home.as_ref().map(|home| home.id),
        Some(s.tree.org.base.id)
    );
    assert_eq!(
        original.agents[0]
            .grouping
            .home
            .as_ref()
            .map(|home| home.id),
        Some(s.tree.org.base.id)
    );
    let available = original.agents[0]
        .arrangements
        .iter()
        .any(|choice| choice.available);
    assert_eq!(
        original.agents[0].grouping.availability[0].available,
        available
    );
    assert_eq!(
        original.agents[0].grouping.availability[0].machine_id,
        s.rig.machine_id
    );
    sqlx::query("UPDATE environments SET status='lost' WHERE id=$1")
        .bind(s.on.environment)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let after = options_for(&s, &s.ada, thread).await;
    let environment = after
        .environments
        .iter()
        .find(|row| row.environment.id == s.on.environment)
        .unwrap();
    assert_eq!(
        environment.grouping.home.as_ref().map(|home| home.id),
        Some(s.tree.topic.base.id)
    );
    assert!(!environment.usable);
    assert_eq!(environment.reason.as_deref(), Some("environment_lost"));
    assert!(!environment.grouping.availability[0].available);
    assert_eq!(
        environment.grouping.availability[0].reason.as_deref(),
        Some("environment_lost")
    );
}

#[tokio::test]
async fn a_translated_pairing_shows_its_actual_interfaces_and_limits_before_policy_can_close_it() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    offering_plain(&s, s.on.environment).await;
    sqlx::query("UPDATE providers SET kind='api',agent_id=NULL,harness_kind=NULL,key_machine_id=$2,
        protocols=ARRAY['anthropic-messages'],arrangements=ARRAY['separated'],
        upstreams=$3 WHERE id=$1")
        .bind(s.on.provider).bind(s.rig.machine_id)
        .bind(json!([{"protocol":"anthropic-messages","base_url":"https://controlled.invalid","options":{}}]))
        .execute(s.rig.db.pool()).await.unwrap();
    let options = options_for(&s, &s.ada, thread).await;
    let provider = provider_of(&options, s.on.provider);
    let profile = provider
        .agents
        .iter()
        .find(|profile| profile.agent.id == s.on.agent)
        .unwrap();
    let choice = profile
        .arrangements
        .iter()
        .find(|choice| choice.kind == ArrangementType::Separated)
        .unwrap();
    assert!(choice.available && choice.permitted, "{choice:?}");
    let capture = choice
        .isolation
        .translation
        .clone()
        .expect("the conversion is explicit at the choice");
    assert_eq!(capture.from, "openai-responses");
    assert_eq!(capture.to, "anthropic-messages");
    assert!(!capture.losses.approximated.is_empty());
    assert!(!capture.losses.dropped.is_empty());
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.org.base.id),
            "providers.translation",
            json!(false),
        )
        .await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.project.base.id),
            "providers.translation",
            json!(true),
        )
        .await;
    let closed = options_for(&s, &s.ada, thread).await;
    let profile = provider_of(&closed, s.on.provider)
        .agents
        .iter()
        .find(|profile| profile.agent.id == s.on.agent)
        .unwrap();
    let choice = profile
        .arrangements
        .iter()
        .find(|choice| choice.kind == ArrangementType::Separated)
        .unwrap();
    assert!(choice.available && !choice.permitted, "{choice:?}");
    assert_eq!(
        choice.not_allowed_by,
        Some(orch_api_contract::binding::NotAllowedBy::Policy)
    );
    assert_eq!(
        choice.scope.as_ref().map(|scope| scope.id),
        Some(s.tree.org.base.id),
        "the refusal names the false ancestor, not the nearer true contribution"
    );
    assert_eq!(
        capture.from, "openai-responses",
        "the earlier capture is not filled from today's policy"
    );
}

#[tokio::test]
async fn a_directory_choice_reads_updated_capacity_with_its_machine_view_permission() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let directory = a_directory_on_a_machine(&s, "Measured directory").await;
    names(
        &s,
        "machine",
        s.rig.machine_id,
        "machine.view",
        &s.ada,
        json!({}),
    )
    .await;
    for memory in [42_764_201_984_i64, 41_696_751_616] {
        sqlx::query("UPDATE machines SET facts=COALESCE(facts,'{}'::jsonb) || $2 WHERE id=$1")
            .bind(s.rig.machine_id)
            .bind(json!({"cpus":4,"memory_bytes":memory,"data_volume_bytes":128_000_000_000_i64,"capacity_refresh_secs":600}))
            .execute(&mut *s.rig.db.acquire().await.unwrap()).await.unwrap();
        let options = options_for(&s, &s.ada, thread).await;
        let shown = options
            .environments
            .iter()
            .find(|p| p.environment.id == directory)
            .unwrap();
        assert_eq!(shown.capacity.as_ref().unwrap().memory_bytes, Some(memory));
    }
}

#[tokio::test]
async fn each_creation_default_names_only_its_own_lock() {
    use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
    for key in [
        "agent.default_provider_id",
        "agent.default_agent_id",
        "agent.model",
        "agent.effort",
    ] {
        let s = Scene::new().await;
        let thread = s.thread().await.base.id;
        let value = match key {
            "agent.default_provider_id" => json!(s.on.provider),
            "agent.default_agent_id" => json!(s.on.agent),
            "agent.model" => json!(MODEL),
            _ => json!("high"),
        };
        s.rig
            .world
            .app
            .call(NodeSettingSet, &[&s.tree.org.base.id, &key])
            .token(&s.ada.token)
            .body(&NodeSettingSetBody {
                model_choice: None,
                value,
                locked: Some(true),
                expected_revision: None,
            })
            .send()
            .await
            .ok();
        let options = options_for(&s, &s.ada, thread).await;
        assert_eq!(
            serde_json::to_value(options.defaults.locks).unwrap(),
            json!({
                "provider":key=="agent.default_provider_id", "agent":key=="agent.default_agent_id",
                "model":key=="agent.model", "effort":key=="agent.effort"
            })
        );
    }
}

#[tokio::test]
async fn a_start_proposal_names_the_real_approval_replacement_and_changes_when_the_preference_changes()
 {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let mut conn = s.rig.db.acquire().await.unwrap();
    sqlx::query("UPDATE agents SET descriptor = jsonb_set(descriptor, '{approval_modes}', '[\"on_request\"]'), descriptor_overrides='{}' WHERE id=$1")
        .bind(s.on.agent).execute(&mut *conn).await.unwrap();
    drop(conn);
    s.rig
        .world
        .set_at(Target::Node(topic), "approval.mode", json!("auto"))
        .await;
    let first = start_options_for(&s, topic).await;
    let first = serde_json::to_value(&provider_of(&first, s.on.provider).agents[0]).unwrap();
    let resolution = &first["approval_resolution"];
    assert_eq!(resolution["preferred"], "auto");
    assert_eq!(resolution["effective"], "on_request");
    assert_eq!(resolution["reason"], "not_declared");
    assert_eq!(resolution["source"]["node"]["id"], json!(topic));
    s.rig
        .world
        .set_at(Target::Node(topic), "approval.mode", json!("on_request"))
        .await;
    let next = start_options_for(&s, topic).await;
    let next = serde_json::to_value(&provider_of(&next, s.on.provider).agents[0]).unwrap();
    assert_eq!(next["approval_resolution"]["effective"], "on_request");
    assert_eq!(
        next["approval_resolution"]["reason"],
        serde_json::Value::Null
    );
    assert_ne!(
        next["approval_resolution"]["revision"],
        resolution["revision"]
    );
    assert_eq!(
        resolution["preferred"], "auto",
        "a later proposal does not rewrite the first one"
    );
}

#[tokio::test]
async fn a_thread_created_without_model_or_approval_choices_uses_the_only_model_and_an_offered_mode()
 {
    use orch_agent_contract::descriptor::ApprovalMode;
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
    let s = Scene::new().await;
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "approval.mode",
            json!("auto"),
        )
        .await;
    let made = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            name: Some("Use the offered defaults".to_owned()),
            binding: Some(CreateThreadBinding::provider(
                s.on.agent,
                s.on.provider,
                None,
            )),
            environment: Some(EnvironmentChoice::existing(s.on.environment)),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(made.model.as_deref(), Some(MODEL));
    let mut conn = s.rig.db.acquire().await.unwrap();
    let resolved = orch_engine::resolved::load(&mut conn, made.base.id)
        .await
        .unwrap();
    assert_eq!(resolved.model.as_deref(), Some(MODEL));
    assert_eq!(resolved.approval_mode, ApprovalMode::Ask);
    let stored: Option<String> = sqlx::query_scalar("SELECT payload#>>'{data,model}' FROM items WHERE thread_id=$1 AND payload->>'marker'='created'")
        .bind(made.base.id).fetch_optional(&mut *conn).await.unwrap().flatten();
    assert_eq!(
        stored.as_deref(),
        Some(MODEL),
        "the accepted creation says which sole model it selected"
    );
}

#[tokio::test]
async fn an_omitted_model_keeps_the_places_choice_and_does_not_replace_an_explicit_or_disallowed_choice()
 {
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
    let s = Scene::new().await;
    let request = || CreateThread {
        name: Some("Keep a deliberate model choice".to_owned()),
        binding: Some(CreateThreadBinding::provider(
            s.on.agent,
            s.on.provider,
            None,
        )),
        environment: Some(EnvironmentChoice::existing(s.on.environment)),
        ..CreateThread::default()
    };
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "agent.model",
            json!(MODEL),
        )
        .await;
    let made = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&request())
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(made.model.as_deref(), Some(MODEL));
    let mut explicit = request();
    explicit.model = Some("not-offered".to_owned());
    let refused = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&explicit)
        .send()
        .await;
    assert!(
        !refused.status.is_success(),
        "an explicit choice is never replaced by the sole offered model"
    );
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.topic.base.id),
            "models.allowed",
            json!(["not-offered"]),
        )
        .await;
    let refused = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&request())
        .send()
        .await;
    assert!(
        !refused.status.is_success(),
        "choosing a default cannot bypass the place allowlist"
    );
}

#[tokio::test]
async fn placement_choices_show_the_room_on_the_current_machine_or_host() {
    let s = Scene::new().await;
    let directory = a_directory_on_a_machine(&s, "Shared workspace").await;
    let managed = another_environment(&s, "Hosted workspace", None).await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(s.ada.id()),
        Some(s.tree.org.base.id),
        "Placement host",
    )
    .await
    .unwrap();
    let at = s.rig.world.now();
    let sample = json!({"cpu_free_millis":1250,"memory_free_bytes":4096,"observed_at":orch_types::Time::new(at)});
    sqlx::query("UPDATE machines SET online=true, facts=jsonb_set(facts,'{room}',$2) WHERE id=$1")
        .bind(s.rig.machine_id)
        .bind(&sample)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET capacity=$2,load=$3,load_at=$4 WHERE id=$1")
        .bind(host)
        .bind(json!({"cpus":8,"cpu_millis":4500,"memory_mb":8192}))
        .bind(json!({"cpu_percent":125.0,"memory_used_mb":6144}))
        .bind(at)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online=true,session_started_at=NULL WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)")
        .bind(host).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,state)
                 SELECT $1,'environment',$2,'hosted-placement',id,'test',template_id,$4,'running' FROM environments WHERE id=$3")
        .bind(orch_types::new_id()).bind(host).bind(managed).bind(vec![0_u8])
        .execute(&mut *conn).await.unwrap();
    drop(conn);
    let read = |choices: &BindingOptions, id| {
        let choice = choices
            .environments
            .iter()
            .find(|row| row.environment.id == id)
            .unwrap();
        serde_json::to_value(choice).unwrap()["room"].clone()
    };
    let choices = start_options_for(&s, s.tree.topic.base.id).await;
    assert_eq!(read(&choices, directory), sample);
    assert_eq!(
        read(&choices, managed),
        json!({"cpu_free_millis":3250,"memory_free_bytes":2147483648_i64,"observed_at":orch_types::Time::new(at)})
    );
    sqlx::query("UPDATE machines SET facts=facts-'room' WHERE id=$1")
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET session_started_at=$2 WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)")
        .bind(host).bind(at + chrono::Duration::seconds(1)).execute(s.rig.db.pool()).await.unwrap();
    let later = start_options_for(&s, s.tree.topic.base.id).await;
    assert_eq!(read(&later, directory), serde_json::Value::Null);
    assert_eq!(
        read(&later, managed),
        serde_json::Value::Null,
        "a host's earlier session cannot supply its current room"
    );
}

#[tokio::test]
async fn a_lapsed_subscription_offers_sign_in_then_needs_a_model_only_after_renewal() {
    let s = Scene::without_supervisor().await;
    let thread = s.thread().await.base.id;
    sqlx::query(
        "UPDATE providers SET status='needs_setup', status_detail=NULL, facts=$2 WHERE id=$1",
    )
    .bind(s.on.provider)
    .bind(json!({"credential":{"yields":"token","expires_at":"2001-01-01T00:00:00Z"}}))
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let before = options_for(&s, &s.ada, thread).await;
    assert_eq!(
        serde_json::to_value(provider_of(&before, s.on.provider)).unwrap()["setup"],
        json!({"reason":"sign_in_lapsed","can_sign_in":true})
    );
    sqlx::query("UPDATE provider_models SET enabled=false WHERE provider_id=$1")
        .bind(s.on.provider)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let both = options_for(&s, &s.ada, thread).await;
    assert_eq!(
        serde_json::to_value(provider_of(&both, s.on.provider)).unwrap()["setup"]["reason"],
        "sign_in_lapsed"
    );
    sqlx::query("UPDATE providers SET facts=$2 WHERE id=$1")
        .bind(s.on.provider)
        .bind(json!({"credential":{"yields":"token","expires_at":"2099-01-01T00:00:00Z"}}))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let renewed = options_for(&s, &s.ada, thread).await;
    assert_eq!(
        serde_json::to_value(provider_of(&renewed, s.on.provider)).unwrap()["setup"],
        json!({"reason":"needs_model"})
    );
}

#[tokio::test]
async fn the_start_options_and_creation_agree_for_the_owner_and_a_member_using_admin_mode() {
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
    let s = Scene::new().await;
    let member = places::admin(&s.rig.world, "setup_reader").await;
    let org = s.tree.org.base.id;
    join(&s.rig.world, org, org, &member, "developer", &s.ada).await;
    let topic = s.tree.topic.base.id;
    let directory = a_directory_on_a_machine(&s, "Owner's directory").await;
    names(&s, "agent", s.on.agent, "agent.use", &member, json!({})).await;
    names(&s, "agent", s.on.agent, "agent.use", &s.dan, json!({})).await;
    names(
        &s,
        "provider",
        s.on.provider,
        "provider.view",
        &s.dan,
        json!({}),
    )
    .await;
    names(
        &s,
        "environment",
        directory,
        "environment.view",
        &s.dan,
        json!({}),
    )
    .await;
    for (who, granted) in [
        (&s.ada, true),
        (&s.dan, false),
        (&member, false),
        (&s.dan, true),
        (&member, true),
    ] {
        if who.id() != s.ada.id() && granted {
            names(
                &s,
                "provider",
                s.on.provider,
                "provider.use",
                who,
                json!({}),
            )
            .await;
            names(
                &s,
                "environment",
                directory,
                "environment.use",
                who,
                json!({}),
            )
            .await;
        }
        let options = s
            .rig
            .world
            .app
            .call(NodeBindingOptionsGet, &[&topic])
            .token(&who.token)
            .query(&NodeBindingOptionsGetQuery::default())
            .send()
            .await
            .ok();
        let provider = provider_of(&options, s.on.provider);
        let model = provider
            .models
            .iter()
            .find(|m| m.model_name == MODEL)
            .unwrap();
        let environment = options
            .environments
            .iter()
            .find(|row| row.environment.id == directory)
            .unwrap();
        assert_eq!(provider.usable, granted);
        assert_eq!(model.allowed, granted);
        assert_eq!(environment.usable, granted);
        if !granted {
            assert_eq!(provider.reason.as_deref(), Some("no_use"));
            assert_eq!(
                environment.reason.as_deref(),
                Some("environment_use_required")
            );
        }
        let selected = s
            .rig
            .world
            .app
            .call(NodeBindingOptionsGet, &[&topic])
            .token(&who.token)
            .query(&NodeBindingOptionsGetQuery {
                environment_id: Some(directory),
                ..Default::default()
            })
            .send()
            .await;
        let reply = s
            .rig
            .world
            .app
            .call(ThreadCreate, &[&topic])
            .token(&who.token)
            .body(&CreateThread {
                binding: Some(CreateThreadBinding::provider(
                    s.on.agent,
                    s.on.provider,
                    None,
                )),
                environment: Some(EnvironmentChoice::existing(directory)),
                model: Some(MODEL.into()),
                ..Default::default()
            })
            .send()
            .await;
        if granted {
            assert!(provider_of(&selected.ok(), s.on.provider).usable);
            let made = reply.ok().into_full().unwrap();
            assert_eq!(made.environment.unwrap().id, directory);
            assert_eq!(made.binding.unwrap().provider.id, s.on.provider);
        } else {
            let reason = selected.problem();
            assert_eq!(reason.code, ErrorCode::AuthzDenied);
            assert!(places::missing_permissions(&reason).contains(&"environment.use".to_owned()));
            let reason = reply.problem();
            assert_eq!(reason.code, ErrorCode::AuthzDenied);
            let missing = places::missing_permissions(&reason);
            assert!(missing.contains(&"provider.use".to_owned()));
            assert!(missing.contains(&"environment.use".to_owned()));
            assert!(
                reason.extra["missing"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|row| row["ask"]
                        .as_array()
                        .is_some_and(|people| !people.is_empty()))
            );
        }
    }
}
