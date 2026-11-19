//! A limit on one model of a provider, on a group of its models, on the whole of it and on one speed is a cap on the provider: it covers the models whose entries it names, a group is the managers' name for several of their entries and follows a model put into it or taken out, a group named on a cap of a place covers what each provider's managers put in a group of that name and nothing else, and a cap that names a speed covers a turn at that speed and not an ordinary one, in turns at once and in credits, with its use equal to the ledger's after a rebuild.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use chrono::Utc;
use common::{MODEL, World, queued, turn_ended};
use orch_authz::Caller;
use orch_db::usage::caps::{self as rows};
use orch_db::usage::ledger::{self, Attribution, NewEvent, Tokens};
use orch_engine::caps;
use orch_link_contract::events::TurnOutcome;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, new_id};
use serde_json::json;

const OTHER: &str = "fake-2";

/// The parameters of an adapter that publishes a speed, as the fake adapter declares it.
fn speed() -> Json {
    json!([{
        "key": "service_tier", "title": "Speed", "description": "A faster way of running a turn.",
        "group": "model", "type": "enum", "options": { "from_model": "service_tiers" },
        "default": { "from_model": "default_service_tier" },
        "scopes": ["org", "project", "topic", "thread"], "apply": "next_turn",
        "eligible_when": { "model_has": "service_tiers" }
    }])
}

/// Says that the agent publishes the speed. The machine of the tests answers `agent.start` with a descriptor
/// of its own that has no parameters, which the engine keeps; a harness that declares the same at every start
/// does not do that, so this is said again whenever a harness was started.
async fn declare(world: &World) {
    sqlx::query("UPDATE agents SET descriptor = jsonb_set(descriptor, '{parameters}', $1)")
        .bind(speed())
        .execute(world.pool.pool())
        .await
        .unwrap();
}

/// A world with two models on its provider, `fake-1` in the group `frontier` and `fake-2` in none, both with a
/// speed enabled; its agent publishes the speed.
async fn world() -> World {
    let world = World::new().await;
    declare(&world).await;
    sqlx::query(
        "UPDATE provider_models SET groups = ARRAY['frontier'],
                parameters = '{\"service_tiers\":[{\"id\":\"fast\",\"label\":\"Fast\"}]}'
          WHERE model_name = $1",
    )
    .bind(MODEL)
    .execute(world.pool.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO provider_models (id, provider_id, model_name, parameters, enabled, source, created_by)
         VALUES ($1, $2, $3, '{\"service_tiers\":[{\"id\":\"fast\",\"label\":\"Fast\"}]}', true, 'manual', $4)",
    )
    .bind(new_id())
    .bind(world.provider)
    .bind(OTHER)
    .bind(world.ada)
    .execute(world.pool.pool())
    .await
    .unwrap();
    world
}

/// A cap by hand: for everyone, shared, on a provider or a place, with the models and speeds it names.
struct Cap {
    provider: Option<Id>,
    node: Option<Id>,
    models: Vec<&'static str>,
    tiers: Vec<&'static str>,
    meter: &'static str,
    amount: i64,
}

impl Cap {
    fn running(amount: i64) -> Self {
        Self {
            provider: None,
            node: None,
            models: Vec::new(),
            tiers: Vec::new(),
            meter: "running",
            amount,
        }
    }

    fn of(meter: &'static str, amount: i64) -> Self {
        Self {
            meter,
            ..Self::running(amount)
        }
    }

    fn on_provider(self, provider: Id) -> Self {
        Self {
            provider: Some(provider),
            ..self
        }
    }

    fn in_place(self, node: Id) -> Self {
        Self {
            node: Some(node),
            ..self
        }
    }

    fn naming(self, models: &[&'static str]) -> Self {
        Self {
            models: models.to_vec(),
            ..self
        }
    }

    fn at(self, tiers: &[&'static str]) -> Self {
        Self {
            tiers: tiers.to_vec(),
            ..self
        }
    }

    async fn make(self, world: &World) -> Id {
        let id = new_id();
        let period = (self.meter != "running").then_some("day");
        sqlx::query(
            "INSERT INTO usage_caps (id, node_id, provider_id, subject_kind, split, models, tiers, meter, period,
                                     amount, at_limit, created_by)
             VALUES ($1, $2, $3, 'everyone', 'shared', $4, $5, $6, $7, $8, 'stop', $9)",
        )
        .bind(id)
        .bind(self.node)
        .bind(self.provider)
        .bind(self.models.iter().map(|m| (*m).to_owned()).collect::<Vec<_>>())
        .bind(self.tiers.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>())
        .bind(self.meter)
        .bind(period)
        .bind(self.amount)
        .bind(world.ada)
        .execute(world.pool.pool())
        .await
        .unwrap();
        id
    }
}

/// A thread that runs `model`, at the speed `tier` where it chose one.
async fn thread_of(world: &World, name: &str, model: &str, tier: Option<&str>) -> Id {
    let thread = world.thread(name).await;
    world.set(thread, keys::AGENT_MODEL, json!(model)).await;
    if let Some(tier) = tier {
        world
            .set(thread, keys::AGENT_PARAMS, json!({ "service_tier": tier }))
            .await;
    }
    thread
}

/// Sends an input to a thread.
async fn send(world: &World, thread: Id) {
    declare(world).await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
}

/// Whether the machine starts a turn or the cap explicitly holds it.
async fn started(world: &World, thread: Id) -> bool {
    common::eventually("the input to start or be held by its cap", || async {
        !world.turn_starts(thread).is_empty()
            || world.state(thread).await.1.as_deref() == Some("quota_exceeded")
    })
    .await;
    !world.turn_starts(thread).is_empty()
}

/// A turn that runs and holds its place in a cap.
async fn running(world: &World, thread: Id) -> orch_link_contract::commands::TurnStart {
    declare(world).await;
    let start = world.running_turn(thread, "Hold").await.0;
    declare(world).await;
    start
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_on_a_provider_that_names_one_model_leaves_its_other_models_alone() {
    let world = world().await;
    Cap::running(1)
        .on_provider(world.provider)
        .naming(&[MODEL])
        .make(&world)
        .await;
    let first = thread_of(&world, "First", MODEL, None).await;
    let second = thread_of(&world, "Second", MODEL, None).await;
    let other = thread_of(&world, "Other", OTHER, None).await;
    running(&world, first).await;
    send(&world, second).await;
    world.wait_blocked(second, "quota_exceeded").await;
    assert!(
        !started(&world, second).await,
        "a second turn of the model waits"
    );
    send(&world, other).await;
    world.turn_start(other, 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_on_a_provider_that_names_none_covers_the_whole_of_it() {
    let world = world().await;
    Cap::running(1)
        .on_provider(world.provider)
        .make(&world)
        .await;
    let first = thread_of(&world, "First", MODEL, None).await;
    let other = thread_of(&world, "Other", OTHER, None).await;
    running(&world, first).await;
    send(&world, other).await;
    world.wait_blocked(other, "quota_exceeded").await;
    assert!(!started(&world, other).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_that_names_a_group_covers_exactly_the_models_in_it_and_follows_a_model_put_in_or_taken_out()
 {
    let world = world().await;
    Cap::running(1)
        .on_provider(world.provider)
        .naming(&["@frontier"])
        .make(&world)
        .await;
    let in_the_group = thread_of(&world, "In", MODEL, None).await;
    let outside = thread_of(&world, "Out", OTHER, None).await;
    let another_in = thread_of(&world, "Another", MODEL, None).await;
    running(&world, in_the_group).await;

    // `fake-2` is in no group: it is not the cap's.
    send(&world, outside).await;
    world.turn_start(outside, 0).await;
    // Another turn of a model in the group waits.
    send(&world, another_in).await;
    world.wait_blocked(another_in, "quota_exceeded").await;

    // Taking the model out of the group takes it out of the cap: the turn that waited is let through.
    world
        .exec("UPDATE provider_models SET groups = '{}' WHERE model_name = 'fake-1'")
        .await;
    world.engine().room_may_have_been_made();
    world.turn_start(another_in, 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_put_into_the_group_is_covered_from_then_on() {
    let world = world().await;
    Cap::running(1)
        .on_provider(world.provider)
        .naming(&["@frontier"])
        .make(&world)
        .await;
    let first = thread_of(&world, "First", MODEL, None).await;
    let outside = thread_of(&world, "Out", OTHER, None).await;
    running(&world, first).await;
    world
        .exec("UPDATE provider_models SET groups = ARRAY['frontier','vision'] WHERE model_name = 'fake-2'")
        .await;
    send(&world, outside).await;
    world.wait_blocked(outside, "quota_exceeded").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_group_no_model_is_in_covers_nothing() {
    let world = world().await;
    Cap::running(1)
        .on_provider(world.provider)
        .naming(&["@nothing"])
        .make(&world)
        .await;
    let first = thread_of(&world, "First", MODEL, None).await;
    let second = thread_of(&world, "Second", MODEL, None).await;
    running(&world, first).await;
    send(&world, second).await;
    world.turn_start(second, 0).await;
}

/// Records that `person` used `tokens` of `model` at `tier` on `provider`, in the world's topic, counted
/// as a forwarder's request would be, with `credits` where there was a rate.
async fn used(
    world: &World,
    thread: Id,
    provider: Id,
    model: &str,
    tier: Option<&str>,
    tokens: i64,
    credits: Option<&str>,
) {
    let place = Attribution {
        org_id: world.root,
        project_id: world.project,
        topic_id: world.topic,
        thread_id: thread,
        segment_id: new_id(),
        turn_id: new_id(),
        provider_id: provider,
        entry_id: None,
        user_id: Some(world.ada),
        agent_id: Some(world.agent),
        harness_kind: Some("fake".to_owned()),
        environment_id: Some(world.environment),
        environment_kind: Some("managed".to_owned()),
        arrangement: Some("together".to_owned()),
        turn_model: model.to_owned(),
        effort: None,
        params: Json::Null,
    };
    let mut conn = world.pool.acquire().await.unwrap();
    ledger::record(
        &mut conn,
        &NewEvent {
            id: new_id(),
            at: Utc::now(),
            origin: ledger::origin::FORWARDER,
            scope: ledger::scope::REQUEST,
            counted: true,
            attribution: &place,
            model,
            tier,
            tokens: Tokens {
                input: tokens,
                cached_input: 0,
                cache_write: 0,
                output: 0,
                reasoning: 0,
                total: tokens,
            },
            context_tokens: None,
            request_ref: None,
            rate: None,
            rate_source: credits.map(|_| "provider"),
            credits,
        },
    )
    .await
    .unwrap();
}

/// A second provider beside the world's, with a model in the group `frontier` and another in none.
async fn another_provider(world: &World) -> Id {
    let id = new_id();
    sqlx::query(
        "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id,
                                machine_id, arrangements, status)
         VALUES ($1, 'Other subscription', $2, $3, $3, 'subscription', 'fake', $4, $5,
                 ARRAY['separated'], 'ready')",
    )
    .bind(id)
    .bind(world.ada)
    .bind(world.root)
    .bind(world.agent)
    .bind(world.machine_id)
    .execute(world.pool.pool())
    .await
    .unwrap();
    for (name, groups) in [("big", vec!["frontier"]), ("small", vec![])] {
        sqlx::query(
            "INSERT INTO provider_models (id, provider_id, model_name, groups, enabled, source, created_by)
             VALUES ($1, $2, $3, $4, true, 'manual', $5)",
        )
        .bind(new_id())
        .bind(id)
        .bind(name)
        .bind(groups.into_iter().map(str::to_owned).collect::<Vec<_>>())
        .bind(world.ada)
        .execute(world.pool.pool())
        .await
        .unwrap();
    }
    id
}

async fn standing_of(world: &World, cap: Id) -> caps::Standing {
    let mut conn = world.pool.acquire().await.unwrap();
    let cap = rows::get(&mut conn, cap).await.unwrap().unwrap();
    caps::standing(&mut conn, &cap, None, Utc::now())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_group_named_on_a_cap_of_a_place_covers_the_models_each_providers_managers_put_in_it_and_no_others()
 {
    let world = world().await;
    let thread = world.thread("Ledger").await;
    let other = another_provider(&world).await;
    let cap = Cap::of("tokens", 1_000)
        .in_place(world.project)
        .naming(&["@frontier"])
        .make(&world)
        .await;
    used(&world, thread, world.provider, MODEL, None, 100, None).await;
    used(&world, thread, other, "big", None, 30, None).await;
    used(&world, thread, other, "small", None, 5_000, None).await;
    used(&world, thread, world.provider, OTHER, None, 7_000, None).await;
    // `fake-1` here and `big` there are in the group their managers call `frontier`; the others are in none.
    let standing = standing_of(&world, cap).await;
    assert_eq!(standing.used.text(), "130");
    // The same name can be in a group at one provider and not at another.
    used(&world, thread, other, MODEL, None, 9_000, None).await;
    assert_eq!(
        standing_of(&world, cap).await.used.text(),
        "130",
        "a model of that name at a provider whose managers did not put it there is not covered"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_that_names_a_speed_covers_a_fast_turn_in_turns_at_once_and_not_an_ordinary_one() {
    let world = world().await;
    Cap::running(1)
        .on_provider(world.provider)
        .at(&["fast"])
        .make(&world)
        .await;
    let fast = thread_of(&world, "Fast", MODEL, Some("fast")).await;
    let another_fast = thread_of(&world, "Another fast", MODEL, Some("fast")).await;
    let ordinary = thread_of(&world, "Ordinary", MODEL, None).await;
    let start = running(&world, fast).await;
    assert_eq!(start.params["service_tier"], "fast");

    send(&world, another_fast).await;
    world.wait_blocked(another_fast, "quota_exceeded").await;
    send(&world, ordinary).await;
    world.turn_start(ordinary, 0).await;

    // When the fast turn ends the one that waited is let in.
    world.answer(fast, &start, "m", "Done.").await;
    world
        .say(fast, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    world.turn_start(another_fast, 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waiting_and_release_order_include_only_the_caps_models_and_service_tiers() {
    let world = world().await;
    let selected = Cap::running(0)
        .on_provider(world.provider)
        .naming(&["@frontier"])
        .at(&["fast"])
        .make(&world)
        .await;
    let temporary = Cap::running(0)
        .on_provider(world.provider)
        .make(&world)
        .await;
    let first = thread_of(&world, "First matching", MODEL, Some("fast")).await;
    let second = thread_of(&world, "Second matching", MODEL, Some("fast")).await;
    let other_model = thread_of(&world, "Other model", OTHER, Some("fast")).await;
    let ordinary = thread_of(&world, "Ordinary tier", MODEL, None).await;
    for thread in [first, second, other_model, ordinary] {
        send(&world, thread).await;
        world.wait_blocked(thread, "quota_exceeded").await;
    }
    declare(&world).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let cap = rows::get(&mut conn, selected).await.unwrap().unwrap();
    let waiting = caps::waiting_for(&mut conn, &cap, world.clock.now())
        .await
        .unwrap();
    assert_eq!(
        waiting.iter().map(|row| row.thread_id).collect::<Vec<_>>(),
        [first, second]
    );
    drop(conn);
    sqlx::query("UPDATE usage_caps SET enabled = false WHERE id = $1")
        .bind(temporary)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().cap_changed(temporary).await.unwrap();
    world.turn_start(other_model, 0).await;
    world.turn_start(ordinary, 0).await;
    assert!(world.turn_starts(first).is_empty() && world.turn_starts(second).is_empty());
    declare(&world).await;
    sqlx::query("UPDATE usage_caps SET amount = 1 WHERE id = $1")
        .bind(selected)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().cap_changed(selected).await.unwrap();
    let start = world.turn_start(first, 0).await;
    assert!(
        world.turn_starts(second).is_empty(),
        "the older matching input goes first"
    );
    world.say(first, common::turn_began(start.turn_id)).await;
    world
        .say(first, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    declare(&world).await;
    world.turn_start(second, 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_in_credits_that_names_a_speed_counts_the_use_at_that_speed_and_equals_the_ledger_after_a_rebuild()
 {
    let world = world().await;
    let thread = world.thread("Ledger").await;
    let cap = Cap::of("credits", 100)
        .on_provider(world.provider)
        .at(&["fast"])
        .make(&world)
        .await;
    used(
        &world,
        thread,
        world.provider,
        MODEL,
        Some("fast"),
        10,
        Some("12.500000"),
    )
    .await;
    used(
        &world,
        thread,
        world.provider,
        MODEL,
        Some("fast"),
        10,
        Some("2.250000"),
    )
    .await;
    used(
        &world,
        thread,
        world.provider,
        MODEL,
        None,
        10,
        Some("40.000000"),
    )
    .await;
    used(
        &world,
        thread,
        world.provider,
        OTHER,
        Some("flex"),
        10,
        Some("7.000000"),
    )
    .await;
    let before = standing_of(&world, cap).await;
    assert_eq!(
        before.used.text(),
        "14.75",
        "only what ran at the speed the cap names"
    );

    let from_the_ledger: String = sqlx::query_scalar(
        "SELECT COALESCE(sum(credits), 0)::text FROM usage_events WHERE counted AND tier = 'fast'",
    )
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        from_the_ledger.trim_end_matches('0').trim_end_matches('.'),
        "14.75"
    );
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::usage::rollup::rebuild(&mut tx, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        standing_of(&world, cap).await.used,
        before.used,
        "the same figure after the rollups were made again from the ledger"
    );

    // A cap that names a speed no use ran at has nothing in it.
    let never = Cap::of("credits", 100)
        .on_provider(world.provider)
        .at(&["priority"])
        .make(&world)
        .await;
    assert_eq!(standing_of(&world, never).await.used.text(), "0");
}
