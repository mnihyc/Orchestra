//! A thread's settings as their writers change them: a model and an effort only as the writer may run them, a policy by whoever makes the rules there, where a thread starts by whoever manages its environment, and every change a line of the thread's history that names who made it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::auth::{ApiTokenCreateBody, MeTokenCreate};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::settings::{
    NodeSettingDelete, NodeSettingSet, NodeSettingSetBody, ResolvedSetting, ThreadParameterGet,
};
use orch_types::{Id, Json, new_id};
use places::{Who, missing_permissions};
use rig::{MODEL, Scene};
use serde_json::json;

async fn put(
    s: &Scene,
    who: &Who,
    node: Id,
    key: &str,
    value: Json,
) -> orch_api_core::testing::Reply<ResolvedSetting> {
    s.rig
        .world
        .app
        .call(NodeSettingSet, &[&node, &key])
        .token(&who.token)
        .body(&NodeSettingSetBody {
            value,
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
}

async fn take_back(
    s: &Scene,
    who: &Who,
    node: Id,
    key: &str,
) -> orch_api_core::testing::Reply<ResolvedSetting> {
    s.rig
        .world
        .app
        .call(NodeSettingDelete, &[&node, &key])
        .token(&who.token)
        .send()
        .await
}

async fn model_of(s: &Scene, thread: Id) -> Option<String> {
    s.rig
        .world
        .app
        .call(ThreadParameterGet, &[&thread])
        .token(&s.ada.token)
        .send()
        .await
        .ok()
        .model
        .value
}

/// A second model of the scene's provider, which takes the effort `low` alone.
async fn second_model(s: &Scene) {
    sqlx::query(
        "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, enabled, source, created_by)
         VALUES ($1, $2, 'fake-2', 'Fake Two', $3, true, 'manual', $4)",
    )
    .bind(new_id())
    .bind(s.on.provider)
    .bind(json!({ "efforts": ["low"], "default_effort": "low" }))
    .bind(s.ada.id())
    .execute(s.rig.db.pool())
    .await
    .unwrap();
}

/// The lines of a thread's history, oldest first, as they are stored.
async fn lines(s: &Scene, thread: Id) -> Vec<Json> {
    sqlx::query_scalar(
        "SELECT payload FROM items WHERE thread_id = $1 AND kind = 'marker' ORDER BY seq_created",
    )
    .bind(thread)
    .fetch_all(s.rig.db.pool())
    .await
    .unwrap()
}

/// The lines of one marker, as their data.
fn of_marker(lines: &[Json], marker: &str) -> Vec<Json> {
    lines
        .iter()
        .filter(|line| line["marker"] == marker)
        .map(|line| line["data"].clone())
        .collect()
}

#[tokio::test]
async fn a_writer_chooses_only_a_model_and_an_effort_they_may_run_on_the_threads_provider() {
    let s = Scene::new().await;
    second_model(&s).await;
    let thread = s.thread_by(&s.eve, "Eve's thread").await.base.id;
    // From now on Eve may run the first model of the provider alone.
    sqlx::query(
        "UPDATE permission_entries SET options = $2 WHERE holder_id = $1 AND permission = 'provider.use'",
    )
    .bind(s.eve.id())
    .bind(json!({ "models": [MODEL] }))
    .execute(s.rig.db.pool())
    .await
    .unwrap();

    let refused = put(&s, &s.eve, thread, "agent.model", json!("fake-2"))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["provider.use"]);
    assert_eq!(
        model_of(&s, thread).await.as_deref(),
        Some(MODEL),
        "nothing was written"
    );
    // An effort the model lists, on the model she may run: hers to choose.
    put(&s, &s.eve, thread, "agent.effort", json!("high"))
        .await
        .ok();

    // The provider's owner may run every model of it.
    put(&s, &s.ada, thread, "agent.model", json!("fake-2"))
        .await
        .ok();
    assert_eq!(model_of(&s, thread).await.as_deref(), Some("fake-2"));

    // The org names the second model for its threads; taking the thread's own model back would
    // have it run that, which Eve may not run.
    put(
        &s,
        &s.ada,
        s.tree.org.base.id,
        "agent.model",
        json!("fake-2"),
    )
    .await
    .ok();
    put(&s, &s.ada, thread, "agent.model", json!(MODEL))
        .await
        .ok();
    let refused = take_back(&s, &s.eve, thread, "agent.model").await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(model_of(&s, thread).await.as_deref(), Some(MODEL));

    // A model that nobody may run here is refused as such, whoever writes it.
    let refused = put(&s, &s.ada, thread, "agent.model", json!("fake-9"))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ModelNotAllowed);
    assert_eq!(refused.extra["reason"], "model");
}

#[tokio::test]
async fn a_policy_is_written_with_settings_manage_alone_and_where_a_thread_starts_by_whoever_manages_its_environment()
 {
    let s = Scene::new().await;
    let org = s.tree.org.base.id;
    let project = s.tree.project.base.id;
    // A token that may make the rules and nothing else: it sees nothing in the place, and writes
    // the policy all the same.
    let rules = s
        .rig
        .world
        .app
        .call(MeTokenCreate, &[])
        .token(&s.ada.token)
        .body(&ApiTokenCreateBody {
            name: "Rules only".to_owned(),
            scopes: Some(vec!["settings.manage".to_owned()]),
            node_id: Some(org),
            expires_at: None,
            proof: orch_api_contract::auth::SensitiveProof {
                password: Some(support::PASSWORD.to_owned()),
                ..Default::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .expect("the first answer carries the secret");
    let written = s
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&project, &"chat.enabled"])
        .token(&rules)
        .body(&NodeSettingSetBody {
            value: json!(false),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(written.value, Some(json!(false)));
    s.rig
        .world
        .app
        .call(NodeSettingDelete, &[&project, &"chat.enabled"])
        .token(&rules)
        .send()
        .await
        .ok();

    // Where a thread starts in its environment is the environment's to decide: Eve manages the
    // environment of Ada's thread and nothing of the thread, and decides it.
    let thread = s.thread().await.base.id;
    let written = put(&s, &s.eve, thread, "thread.cwd", json!("web/api"))
        .await
        .ok();
    assert_eq!(written.value, Some(json!("web/api")));
    // Cai sees the org and does not manage the environment (as shipped, its Developers do).
    let refused = put(&s, &s.cai, thread, "thread.cwd", json!("web"))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(refused.extra["required"], "environment.manage");
    let refused = take_back(&s, &s.cai, thread, "thread.cwd").await.problem();
    assert_eq!(refused.extra["required"], "environment.manage");
}

#[tokio::test]
async fn a_change_of_what_a_thread_runs_with_is_a_line_of_its_history_that_names_who_made_it() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.exchange(thread, 0, "First question", "First answer")
        .await;
    let before = lines(&s, thread).await.len();

    put(&s, &s.ada, thread, "agent.effort", json!("high"))
        .await
        .ok();
    put(
        &s,
        &s.ada,
        thread,
        "agent.params",
        json!({ "verbosity": "high" }),
    )
    .await
    .ok();
    put(&s, &s.ada, thread, "approval.mode", json!("ask"))
        .await
        .ok();
    put(
        &s,
        &s.ada,
        thread,
        "agent.instructions",
        json!("Answer in one line."),
    )
    .await
    .ok();
    // The same value again changes nothing, and says nothing.
    put(&s, &s.ada, thread, "approval.mode", json!("ask"))
        .await
        .ok();

    let all = lines(&s, thread).await;
    let new = &all[before..];
    let models = of_marker(new, "model_changed");
    assert_eq!(models.len(), 1, "{new:#?}");
    assert_eq!(models[0]["from"], MODEL);
    assert_eq!(models[0]["to"], MODEL);
    assert_eq!(models[0]["effort"], json!({ "from": "low", "to": "high" }));
    assert_eq!(models[0]["by"]["username"], "ada");

    let settings = of_marker(new, "setting_changed");
    let keys: Vec<&str> = settings
        .iter()
        .map(|data| data["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        [
            "agent.params.verbosity",
            "approval.mode",
            "agent.instructions"
        ],
        "one line for each change, none for a write that changed nothing"
    );
    assert_eq!(settings[0]["from"], Json::Null);
    assert_eq!(settings[0]["to"], "high");
    assert_eq!(settings[1]["to"], "ask");
    assert_eq!(
        (&settings[2]["from"], &settings[2]["to"]),
        (&Json::Null, &Json::Null),
        "the instructions are never put in a line"
    );
    assert!(settings.iter().all(|data| data["by"]["username"] == "ada"));
}

#[tokio::test]
async fn instructions_budget_a_write_that_takes_what_a_thread_is_told_past_the_limit_is_refused_whole()
 {
    let s = Scene::new().await;
    let org = s.tree.org.base.id;
    let project = s.tree.project.base.id;
    let topic = s.tree.topic.base.id;
    // Each place may say 20,000 characters; two of these, with their headings, fit what a thread may be told.
    let said = "x".repeat(15_000);
    put(&s, &s.ada, org, "agent.instructions", json!(said))
        .await
        .ok();
    put(&s, &s.ada, project, "agent.instructions", json!(said))
        .await
        .ok();
    // A third takes it past the limit: the write is refused, with the limit, how far, and where it was made.
    let refused = put(&s, &s.ada, topic, "agent.instructions", json!(said))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::SettingsInstructionsTooLarge);
    assert_eq!(refused.status, 413);
    assert_eq!(refused.extra["limit"], 40_000);
    assert!(
        refused.extra["characters"].as_u64().unwrap() > 40_000,
        "{refused:?}"
    );
    assert_eq!(refused.extra["scope"]["id"], json!(topic));
    // Nothing of it was kept: a shorter text is taken, and what the topic resolves to holds the two places above and it.
    let kept = put(
        &s,
        &s.ada,
        topic,
        "agent.instructions",
        json!("Answer briefly."),
    )
    .await
    .ok();
    let resolved = kept.value.unwrap();
    let resolved = resolved.as_str().unwrap();
    assert!(resolved.ends_with("Answer briefly."));
    assert_eq!(resolved.matches('x').count(), 30_000);
}
