//! The inherited optional cache choice is carried only for a program that declares it.
use super::*;
use orch_agent_contract::session::PromptCacheTtl;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_native_result_wait_uses_the_registered_system_default_when_a_profile_leaves_it_unset()
{
    let world = World::new().await;
    let mut descriptor = common::descriptor();
    descriptor["x_turn_end_delays"] = json!({"version":1,"delays":[{
        "pointer":"/native_result_wait_ms","also_ms":0,"default_ms":2000,
        "default_setting":"agents.native_result_wait_ms"
    }]});
    world.declares(descriptor).await;
    world
        .set_system("agents.native_result_wait_ms", json!(500))
        .await;
    let thread = world.thread("Native result wait").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Start"))
        .await
        .unwrap();
    let _ = world.turn_start(thread, 0).await;
    let start = world
        .seen
        .of_kind(CommandKind::AgentStart)
        .into_iter()
        .find_map(|command| match command {
            Command::AgentStart(start) => Some(start),
            _ => None,
        })
        .expect("the actual process start");
    assert_eq!(start.profile.config["native_result_wait_ms"], 500);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_web_actions_wait_is_inherited_without_a_hard_upper_limit() {
    let world = World::new().await;
    for (chosen, expected) in [(None, 120), (Some(3600), 3600)] {
        if let Some(chosen) = chosen {
            world
                .set(world.project, "agents.web_action_wait_secs", json!(chosen))
                .await;
        }
        let thread = world.thread("Web action wait").await;
        world
            .engine()
            .submit_input(&Caller::session(world.ada), thread, queued("Start"))
            .await
            .unwrap();
        let _ = world.turn_start(thread, 0).await;
        let open = world
            .seen
            .of_kind(CommandKind::SessionOpen)
            .into_iter()
            .find_map(|command| match command {
                Command::SessionOpen(open) if open.thread_id == thread => Some(open),
                _ => None,
            })
            .expect("the actual session open command");
        assert_eq!(open.web_action_wait_secs, expected);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_session_open_carries_each_explicit_cache_lifetime_and_omits_unset_or_unsupported_values()
 {
    for (supported, ttl) in [
        (false, Some(PromptCacheTtl::OneHour)),
        (true, None),
        (true, Some(PromptCacheTtl::FiveMinutes)),
        (true, Some(PromptCacheTtl::OneHour)),
    ] {
        let world = World::new().await;
        let mut descriptor = common::descriptor();
        descriptor["capabilities"]["prompt_cache_ttl"] = json!(supported);
        world.declares(descriptor).await;
        if supported && ttl == Some(PromptCacheTtl::OneHour) {
            sqlx::query("UPDATE provider_models SET parameters=parameters || '{\"cache_lifetimes\":[\"1h\"]}'::jsonb WHERE provider_id=$1")
                .bind(world.provider).execute(world.pool.pool()).await.unwrap();
        }
        if let Some(ttl) = ttl {
            world
                .set(world.project, "agents.prompt_cache_ttl", json!(ttl))
                .await;
        }
        let thread = world.thread("Cache choice").await;
        world
            .engine()
            .submit_input(&Caller::session(world.ada), thread, queued("Start"))
            .await
            .unwrap();
        let _ = world.turn_start(thread, 0).await;
        let open = world
            .seen
            .of_kind(CommandKind::SessionOpen)
            .into_iter()
            .find_map(|command| match command {
                Command::SessionOpen(open) if open.thread_id == thread => Some(open),
                _ => None,
            })
            .expect("the actual session open command");
        assert_eq!(open.prompt_cache_ttl, ttl.filter(|_| supported));
        if !supported || ttl.is_none() {
            assert!(
                serde_json::to_value(&open)
                    .unwrap()
                    .get("prompt_cache_ttl")
                    .is_none(),
                "unset or unsupported adds no native cache field"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_one_hour_setting_cannot_open_a_lifetime_the_provider_has_not_enabled() {
    let world = World::new().await;
    let mut descriptor = common::descriptor();
    descriptor["capabilities"]["prompt_cache_ttl"] = json!(true);
    world.declares(descriptor).await;
    world
        .set(
            world.project,
            "agents.prompt_cache_ttl",
            json!(PromptCacheTtl::OneHour),
        )
        .await;
    let thread = world.thread("Bound native cache lifetime").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Start"))
        .await
        .unwrap();
    world.wait_told_blocked(thread, "model_not_allowed").await;
    assert_eq!(world.ran(CommandKind::SessionOpen), 0);
    assert_eq!(world.turn_starts(thread).len(), 0);
}
