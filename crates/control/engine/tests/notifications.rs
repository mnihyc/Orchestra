//! Who is told of what happens in a thread: the people who follow it at the level a kind takes and who may view it, the people who can decide an approval, nobody twice for one event, what the Inbox holds whether or not a route carries it, and the destinations of the nodes it lies in.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, command_asked, queued, turn_began, turn_ended};
use orch_access_contract::pages::{PageChange, Whole};
use orch_api_contract::approvals::{ApprovalDecision, HarnessOptionDecision};
use orch_authz::Caller;
use orch_db::queries::hierarchy;
use orch_db::queries::notifications::channels::{self, NewChannel, NewDestination};
use orch_db::queries::notifications::inbox::{self, PrefsChange};
use orch_db::queries::notifications::routes::{self, RouteChange};
use orch_engine::binding::{Birth, NewThread};
use orch_engine::notifications::{Event, Kind, job_finished, tell, tell_destinations};
use orch_link_contract::events::TurnOutcome;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::time::Duration;

/// A person who may view the topic and nothing more.
async fn viewer(world: &World, name: &str) -> Id {
    let person = world.person(name).await;
    world.give(person, "viewer").await;
    person
}

/// A person who may manage the threads of the topic, as the developers of a topic may as shipped.
async fn developer(world: &World, name: &str) -> Id {
    let person = world.person(name).await;
    world.give(person, "developer").await;
    person
}

/// `person` follows `node` at `level`, as they chose.
async fn follow(world: &World, person: Id, node: Id, level: &str) {
    let mut conn = world.pool.acquire().await.unwrap();
    hierarchy::put_subscription(&mut conn, person, node, level, "manual", world.clock.now())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_subscribed_member_of_a_large_group_role_or_everyone_holder_receives_the_event() {
    use orch_db::queries::access::{PageKey, testing::add_entry};
    // One beyond the refusal-preview page bound proves delivery crosses the page boundary.
    let count = orch_authz::holders::SCAN_MOST + 1;
    for source in ["group", "role", "everyone"] {
        let world = World::new().await;
        let thread = world.thread("All subscribers").await;
        let mut conn = world.pool.acquire().await.unwrap();
        let users: Vec<Id> = sqlx::query_scalar(
            "WITH made AS (INSERT INTO users (id, username, display_name, avatar_color)
               SELECT gen_random_uuid(), 'reader_' || number::text, 'Reader ' || number::text, 0
                 FROM generate_series(1, $1::bigint) number RETURNING id)
             INSERT INTO org_members (org_id, user_id, domain_id) SELECT $2, id, $2 FROM made RETURNING user_id",
        ).bind(count).bind(world.root).fetch_all(&mut *conn).await.unwrap();
        sqlx::query(
            "INSERT INTO subscriptions (user_id, node_id, level, source)
            SELECT unnest($1::uuid[]), $2, 'all', 'manual'",
        )
        .bind(&users)
        .bind(thread)
        .execute(&mut *conn)
        .await
        .unwrap();
        let group = new_id();
        sqlx::query(
            "INSERT INTO groups (id, org_id, name, created_by) VALUES ($1, $2, 'Subscribers', $3)",
        )
        .bind(group)
        .bind(world.root)
        .bind(world.ada)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO group_members (group_id, user_id) SELECT $1, unnest($2::uuid[])")
            .bind(group)
            .bind(&users)
            .execute(&mut *conn)
            .await
            .unwrap();
        let holder = match source {
            "group" => ("group", Some(group), None),
            "everyone" => ("everyone", None, None),
            _ => {
                sqlx::query("INSERT INTO node_roles (id, node_id, subject_kind, subject_id, role, created_by)
                    VALUES ($1, $2, 'group', $3, 'viewer', $4)")
                    .bind(new_id()).bind(world.root).bind(group).bind(world.ada).execute(&mut *conn).await.unwrap();
                ("role", None, Some("viewer"))
            }
        };
        add_entry(
            &mut conn,
            &PageKey::new("node", thread, "live"),
            "any",
            "thread.view",
            holder,
            None,
            world.clock.now(),
        )
        .await
        .unwrap();
        drop(conn);
        completed_turn(&world, thread, "Done.").await;
        let delivered: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM notifications WHERE kind = 'turn.completed'
            AND thread_id = $1 AND user_id = ANY($2)",
        )
        .bind(thread)
        .bind(&users)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
        assert_eq!(
            delivered, count,
            "every subscribed human behind {source} is told"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_input_mentions_a_current_reader_even_with_their_bell_off_and_leaves_hidden_people_untold()
 {
    let world = World::new().await;
    let thread = world.thread("A mention").await;
    let bob = viewer(&world, "bob").await;
    let hidden = world.person("hidden").await;
    follow(&world, bob, thread, "off").await;
    world.engine().submit_input(&Caller::session(world.ada), thread,
        queued("Please check this, @bob. @bob again; @hidden cannot read it. Nobody pings bob@example.com.")
    ).await.unwrap();
    assert_eq!(told_of(&world, bob, "mention").await, 1);
    assert_eq!(told_of(&world, hidden, "mention").await, 0);
    assert_eq!(told_of(&world, world.ada, "mention").await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provider_exhaustion_tells_managers_and_blocked_thread_followers_once_in_one_episode() {
    let world = World::new().await;
    let thread = world.thread("A provider limit").await;
    let bob = viewer(&world, "bob").await;
    follow(&world, bob, thread, "important").await;
    world
        .say_of_provider(orch_link_contract::events::Event::ProviderStatus(
            orch_link_contract::events::ProviderStatus {
                status: "exhausted".to_owned(),
                detail: Some("The plan is used up".to_owned()),
            },
        ))
        .await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Wait for the provider"),
        )
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_exhausted").await;
    assert_eq!(told_of(&world, bob, "provider.exhausted").await, 1);
    assert_eq!(told_of(&world, world.ada, "provider.exhausted").await, 1);
    world
        .say_of_provider_taken_in(orch_link_contract::events::Event::ProviderStatus(
            orch_link_contract::events::ProviderStatus {
                status: "exhausted".to_owned(),
                detail: Some("Still used up".to_owned()),
            },
        ))
        .await;
    assert_eq!(told_of(&world, world.ada, "provider.exhausted").await, 1);
}

/// What the Inbox of `person` holds of a kind, the oldest first: its title, its first line, whether it was read.
async fn inbox_of(world: &World, person: Id, kind: &str) -> Vec<(String, String, bool)> {
    sqlx::query_as(
        "SELECT title, body, read_at IS NOT NULL FROM notifications
          WHERE user_id = $1 AND kind = $2 ORDER BY inbox_seq, created_at",
    )
    .bind(person)
    .bind(kind)
    .fetch_all(world.pool.pool())
    .await
    .unwrap()
}

/// How many rows of a kind the Inbox of `person` holds.
async fn told_of(world: &World, person: Id, kind: &str) -> usize {
    inbox_of(world, person, kind).await.len()
}

/// The level a person has at a node by a row of their own there, and how it came about.
async fn subscription(world: &World, person: Id, node: Id) -> Option<(String, String)> {
    sqlx::query_as("SELECT level, source FROM subscriptions WHERE user_id = $1 AND node_id = $2")
        .bind(person)
        .bind(node)
        .fetch_optional(world.pool.pool())
        .await
        .unwrap()
}

/// What the outbox holds, the oldest first: the channel, the route, the destination and the kinds it tells.
async fn outbox(world: &World) -> Vec<(String, Option<Id>, Option<Id>, Vec<String>)> {
    let rows: Vec<(String, Option<Id>, Option<Id>, Json)> = sqlx::query_as(
        "SELECT channel, route_id, destination_id, payload FROM outbox ORDER BY created_at, id",
    )
    .fetch_all(world.pool.pool())
    .await
    .unwrap();
    rows.into_iter()
        .map(|(channel, route, destination, payload)| {
            let kinds = payload["notifications"]
                .as_array()
                .unwrap()
                .iter()
                .map(|each| each["kind"].as_str().unwrap().to_owned())
                .collect();
            (channel, route, destination, kinds)
        })
        .collect()
}

/// A turn on the thread that completes with `reply`.
async fn completed_turn(world: &World, thread: Id, reply: &str) {
    let nth = world.turn_starts(thread).len();
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Fix the login"))
        .await
        .unwrap();
    world.run_turn(thread, nth, reply).await;
    world.wait_state(thread, "idle").await;
    world.wait_followed_up(thread).await;
}

/// A turn on the thread that the harness reports as failed.
async fn failed_turn(world: &World, thread: Id) {
    let nth = world.turn_starts(thread).len();
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Fix the login"))
        .await
        .unwrap();
    let start = world.turn_start(thread, nth).await;
    world.say(thread, turn_began(start.turn_id)).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Failed))
        .await;
    world.wait_turn(start.turn_id, "failed").await;
    // The shipped catch-all recovery rule waits for a person after an unclassified failure.
    world.wait_blocked(thread, "error").await;
    let detail: Option<String> = world
        .scalar(
            "SELECT blocked_detail FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    assert_eq!(detail.as_deref(), Some("call_recovery"));
    world.wait_followed_up(thread).await;
}

fn option(id: &str) -> ApprovalDecision {
    ApprovalDecision::Option(HarnessOptionDecision {
        option_id: id.to_owned(),
        note: None,
    })
}

/// A channel of the system that tells through a webhook.
async fn system_webhook(world: &World) -> Id {
    let id = new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    channels::insert_channel(
        &mut conn,
        &NewChannel {
            id,
            org_id: None,
            user_id: None,
            kind: "webhook",
            name: "Hooks",
            config: &json!({ "url": "https://hooks.example/orch" }),
            templates: &json!({}),
            enabled: true,
            created_by: world.ada,
            at: world.clock.now(),
        },
    )
    .await
    .unwrap();
    id
}

/// `person` switched their route through `channel` on, telling `kinds` (all, where none are given) and waiting
/// `delay` minutes for what is unread, where it is given.
async fn switch_on(
    world: &World,
    person: Id,
    channel: Id,
    kinds: &[&str],
    delay: Option<i32>,
) -> Id {
    let kinds: Vec<String> = kinds.iter().map(|kind| (*kind).to_owned()).collect();
    let mut conn = world.pool.acquire().await.unwrap();
    routes::put_route(
        &mut conn,
        new_id(),
        person,
        channel,
        &RouteChange {
            enabled: Some(true),
            kinds: Some(kinds.as_slice()),
            unread_delay_minutes: delay.map(Some),
            min_interval_minutes: None,
        },
        world.clock.now(),
    )
    .await
    .unwrap()
    .id
}

/// A destination of a node that tells through a webhook.
async fn destination(world: &World, node: Id, level: &str, kinds: &[&str]) -> Id {
    let id = new_id();
    let kinds: Vec<String> = kinds.iter().map(|kind| (*kind).to_owned()).collect();
    let mut conn = world.pool.acquire().await.unwrap();
    channels::insert_destination(
        &mut conn,
        &NewDestination {
            id,
            node_id: node,
            channel: "webhook",
            name: "Room",
            config: &json!({ "url": "https://room.example/hook" }),
            templates: &json!({}),
            kinds: &kinds,
            level,
            enabled: true,
            created_by: world.ada,
            at: world.clock.now(),
        },
    )
    .await
    .unwrap();
    id
}

/// The call that starts a sub-agent, as the harness reports it, in the state `state`.
fn subagent_call(
    world: &World,
    state: orch_agent_contract::events::SubagentState,
) -> orch_link_contract::foreign::ItemPayload {
    use orch_agent_contract::events::{OutputRef, RawCall, ToolCallPayload, ToolPayload};
    orch_link_contract::foreign::ItemPayload::ToolCall(ToolCallPayload {
        tool: ToolPayload::Subagent {
            child_stream: "sub:reviewer".to_owned(),
            name: Some("reviewer".to_owned()),
            role: None,
            prompt: Some("Check the redirect.\nThen the cookie.".to_owned()),
            state,
            model: Some("deepseek/deepseek-v4".to_owned()),
        },
        title: "Review".to_owned(),
        started_at: orch_types::Time::new(world.clock.now()),
        duration_ms: None,
        raw: RawCall {
            name: "subagent".to_owned(),
            input: OutputRef::of("{}"),
            output: None,
            is_error: false,
        },
    })
}

/// A turn in which a sub-agent starts and then finishes, its call reported as `native_ref`.
async fn subagent_ran(world: &World, thread: Id, native_ref: &str) {
    use orch_agent_contract::events::SubagentState;
    use orch_link_contract::events::{
        EmittedItemKind, Event as LinkEvent, ItemCompleted, ItemStarted,
    };
    let nth = world.turn_starts(thread).len();
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Review the login"),
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, nth).await;
    world.say(thread, turn_began(start.turn_id)).await;
    world
        .say(
            thread,
            LinkEvent::ItemStarted(ItemStarted {
                turn_id: start.turn_id,
                native_ref: native_ref.to_owned(),
                stream: "main".to_owned(),
                parent_native_ref: None,
                kind: EmittedItemKind::ToolCall,
                payload: subagent_call(world, SubagentState::Running),
            }),
        )
        .await;
    world
        .say(
            thread,
            LinkEvent::ItemCompleted(ItemCompleted {
                native_ref: native_ref.to_owned(),
                status: orch_agent_contract::events::ItemStatus::Completed,
                payload: subagent_call(world, SubagentState::Completed),
                output_blob: None,
            }),
        )
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    world.wait_state(thread, "idle").await;
    world.wait_followed_up(thread).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sub_agent_that_ended_is_told_to_the_thread_s_followers_only_where_the_place_asks_for_it()
{
    let world = World::new().await;
    let thread = world.thread("Fix login").await;
    subagent_ran(&world, thread, "reviewer-1").await;
    assert_eq!(
        told_of(&world, world.ada, "subagent.ended").await,
        0,
        "off as shipped: the agent that started it is told by its program"
    );
    world
        .set(world.topic, "notify.subagents", json!(true))
        .await;
    subagent_ran(&world, thread, "reviewer-2").await;
    assert_eq!(
        inbox_of(&world, world.ada, "subagent.ended").await,
        [(
            "Fix login: reviewer finished".to_owned(),
            "Check the redirect.".to_owned(),
            false
        )],
        "the owner follows the thread at all; the row says what the sub-agent was asked first"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_completed_is_told_to_those_who_follow_the_thread_at_all_and_may_view_it() {
    let world = World::new().await;
    let thread = world.thread("Fix login").await;
    let bob = viewer(&world, "bob").await;
    let cyd = viewer(&world, "cyd").await;
    let dee = world.person("dee").await;
    follow(&world, bob, thread, "important").await;
    follow(&world, cyd, world.project, "all").await;
    follow(&world, dee, thread, "all").await;

    completed_turn(
        &world,
        thread,
        "Fixed the login page.\nNothing else changed.",
    )
    .await;

    let owners = inbox_of(&world, world.ada, "turn.completed").await;
    assert_eq!(
        owners,
        [(
            "Fix login: turn completed".to_owned(),
            "Fixed the login page.".to_owned(),
            false
        )],
        "the owner follows the thread at all; the row shows the first line of what the agent said"
    );
    assert_eq!(
        told_of(&world, cyd, "turn.completed").await,
        1,
        "a bell on the project is followed by the thread below it"
    );
    assert_eq!(
        told_of(&world, bob, "turn.completed").await,
        0,
        "important is not all"
    );
    assert_eq!(
        told_of(&world, dee, "turn.completed").await,
        0,
        "somebody who may not view the thread is told nothing of it, however they follow it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn watching_ones_own_success_is_quiet_but_away_authors_other_followers_failures_and_destinations_still_hear()
 {
    let world = World::new().await;
    let thread = world.thread("Quiet while here").await;
    let bob = viewer(&world, "bob").await;
    follow(&world, bob, thread, "all").await;
    let room = destination(&world, world.project, "all", &[]).await;
    world.live.watching(thread, world.ada, true);
    world.live.watching(thread, bob, true);

    completed_turn(&world, thread, "Already visible in the conversation.").await;
    assert_eq!(
        told_of(&world, world.ada, "turn.completed").await,
        0,
        "the input's author is already watching"
    );
    assert_eq!(
        told_of(&world, bob, "turn.completed").await,
        1,
        "watching is not a reason to suppress somebody else's turn"
    );
    assert!(
        outbox(&world).await.iter().any(|row| row.2 == Some(room)),
        "the team's destination still receives the event"
    );

    world.live.watching(thread, world.ada, false);
    completed_turn(&world, thread, "Ready when you return.").await;
    assert_eq!(
        told_of(&world, world.ada, "turn.completed").await,
        1,
        "the author who went away still gets the completion"
    );
    world.live.watching(thread, world.ada, true);
    failed_turn(&world, thread).await;
    assert_eq!(
        told_of(&world, world.ada, "turn.failed").await,
        1,
        "a failure still needs attention while watching"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_failed_is_told_at_important_too_and_one_the_person_stopped_is_told_to_nobody()
{
    let world = World::new().await;
    let thread = world.thread("Fix login").await;
    let bob = viewer(&world, "bob").await;
    follow(&world, bob, thread, "important").await;

    failed_turn(&world, thread).await;
    for person in [world.ada, bob] {
        let failed = inbox_of(&world, person, "turn.failed").await;
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, "Fix login: turn failed");
        assert_eq!(failed[0].1, "the turn failed");
    }
    assert_eq!(told_of(&world, world.ada, "turn.completed").await, 0);

    // The next turn is stopped by its person: they did it, and nobody is told.
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Again"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 1).await;
    world.say(thread, turn_began(start.turn_id)).await;
    world
        .engine()
        .interrupt(&Caller::session(world.ada), thread, start.turn_id)
        .await
        .unwrap();
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_state(thread, "idle").await;
    world.wait_followed_up(thread).await;
    assert_eq!(told_of(&world, world.ada, "turn.failed").await, 1);
    assert_eq!(told_of(&world, world.ada, "turn.completed").await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_who_switched_the_thread_off_is_not_told_whatever_a_place_above_says() {
    let world = World::new().await;
    let thread = world.thread("Fix login").await;
    let bob = viewer(&world, "bob").await;
    follow(&world, bob, world.project, "all").await;
    follow(&world, bob, thread, "off").await;
    follow(&world, world.ada, thread, "off").await;

    completed_turn(&world, thread, "Done.").await;
    failed_turn(&world, thread).await;

    for person in [world.ada, bob] {
        assert_eq!(told_of(&world, person, "turn.completed").await, 0);
        assert_eq!(told_of(&world, person, "turn.failed").await, 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_that_is_blocked_is_told_once_to_those_who_follow_it() {
    let world = World::new().await;
    let thread = world.thread("Fix login").await;
    let bob = viewer(&world, "bob").await;
    follow(&world, bob, thread, "important").await;
    world
        .exec("UPDATE environments SET status = 'stopped'")
        .await;

    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Fix it"))
        .await
        .unwrap();
    world.wait_blocked(thread, "environment_unavailable").await;
    common::meanwhile(Duration::from_millis(500)).await;

    for person in [world.ada, bob] {
        let blocked = inbox_of(&world, person, "thread.blocked").await;
        assert_eq!(
            blocked.len(),
            1,
            "the supervisor looking again tells nothing new"
        );
        assert_eq!(blocked[0].0, "Fix login is blocked");
        assert!(!blocked[0].1.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approval_notifies_the_eligible_turn_starter_while_other_managers_can_decide() {
    let world = World::new().await;
    let thread = world.thread("Careful").await;
    let always = developer(&world, "bob").await; // can decide, follows nothing
    let muted = developer(&world, "cyd").await; // can decide, follows nothing, asked for none
    let follows = developer(&world, "dee").await; // can decide, asked for none, follows at important
    let silenced = developer(&world, "eve").await; // can decide, but switched the thread off
    let reader = viewer(&world, "fay").await; // follows at all and cannot decide
    for person in [muted, follows] {
        let mut conn = world.pool.acquire().await.unwrap();
        inbox::put_prefs(
            &mut conn,
            person,
            &PrefsChange {
                approvals_always: Some(false),
                ..PrefsChange::default()
            },
        )
        .await
        .unwrap();
    }
    follow(&world, follows, world.project, "important").await;
    follow(&world, silenced, thread, "off").await;
    follow(&world, reader, thread, "all").await;

    let (start, _) = world.running_turn(thread, "Clean the build").await;
    world
        .say(thread, command_asked(start.turn_id, "req-1"))
        .await;
    world.wait_state(thread, "waiting_approval").await;

    let approval: Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id = $1", thread)
        .await;
    let eligible = world.engine().eligible_deciders(thread).await.unwrap();
    for person in [world.ada, always, muted, follows, silenced] {
        assert!(eligible.contains(&person), "{person} can decide");
    }
    assert!(!eligible.contains(&reader), "a reader cannot decide");
    for (person, told) in [
        (world.ada, true),
        (always, false),
        (muted, false),
        (follows, false),
        (silenced, false),
        (reader, false),
    ] {
        let rows = inbox_of(&world, person, "approval.requested").await;
        assert_eq!(rows.len(), usize::from(told), "{person}");
        if told {
            assert_eq!(rows[0].0, "Careful asks for approval");
            assert_eq!(rows[0].1, "Run a command");
        }
    }
    let linked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE kind = 'approval.requested' AND approval_id = $1",
    )
    .bind(approval)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(linked, 1, "the starter's row names the approval");

    // Once somebody has decided it nobody has anything left to do about it.
    world
        .engine()
        .decide_approval(&Caller::session(always), approval, &option("allow_once"))
        .await
        .unwrap();
    let rows = inbox_of(&world, world.ada, "approval.requested").await;
    assert!(rows[0].2, "the starter need not look at what was decided");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approval_that_the_harness_withdrew_is_read_for_everybody_too() {
    let world = World::new().await;
    let thread = world.thread("Careful").await;
    let (start, _) = world.running_turn(thread, "Clean the build").await;
    world
        .say(thread, command_asked(start.turn_id, "req-2"))
        .await;
    world.wait_state(thread, "waiting_approval").await;
    assert!(!inbox_of(&world, world.ada, "approval.requested").await[0].2);

    world
        .say(
            thread,
            orch_link_contract::events::Event::ApprovalClosed(
                orch_link_contract::events::ApprovalClosed {
                    request_ref: "req-2".to_owned(),
                },
            ),
        )
        .await;
    world.wait_state(thread, "running").await;
    assert!(inbox_of(&world, world.ada, "approval.requested").await[0].2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_told_twice_for_one_event_not_in_the_inbox_nor_in_the_outbox() {
    let world = World::new().await;
    let channel = system_webhook(&world).await;
    switch_on(&world, world.ada, channel, &[], None).await;
    let room = destination(&world, world.topic, "all", &[]).await;
    let event = Event {
        kind: Kind::Milestone,
        node: Some(world.topic),
        thread: None,
        approval: None,
        title: "Reached the first milestone".to_owned(),
        body: String::new(),
        data: json!({}),
        source: "milestone-1".to_owned(),
        actor: None,
        at: world.clock.now(),
    };
    for _ in 0..2 {
        let mut tx = world.pool.begin().await.unwrap();
        let told = tell(&mut tx, &event, &[world.ada], Some(world.root))
            .await
            .unwrap();
        tell_destinations(&mut tx, &event, false).await.unwrap();
        tx.commit().await.unwrap();
        let _ = told;
    }
    assert_eq!(told_of(&world, world.ada, "milestone").await, 1);
    let sent = outbox(&world).await;
    assert_eq!(
        sent.len(),
        2,
        "one for the route and one for the destination: {sent:?}"
    );
    assert_eq!(sent.iter().filter(|row| row.2 == Some(room)).count(), 1);
    // Another event of the same kind is another row.
    let another = Event {
        source: "milestone-2".to_owned(),
        ..event
    };
    let mut tx = world.pool.begin().await.unwrap();
    tell(&mut tx, &another, &[world.ada], Some(world.root))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(told_of(&world, world.ada, "milestone").await, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_notification_is_in_the_inbox_whether_or_not_a_route_carries_it() {
    let world = World::new().await;
    let thread = world.thread("Fix login").await;
    let channel = system_webhook(&world).await;
    let route = switch_on(&world, world.ada, channel, &["turn.failed"], None).await;

    completed_turn(&world, thread, "Done.").await;
    assert_eq!(told_of(&world, world.ada, "turn.completed").await, 1);
    assert!(
        outbox(&world).await.is_empty(),
        "the route tells failures only, and the Inbox was written all the same"
    );

    failed_turn(&world, thread).await;
    assert_eq!(told_of(&world, world.ada, "turn.failed").await, 1);
    let sent = outbox(&world).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1, Some(route));
    assert_eq!(sent[0].3, ["turn.failed"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_route_is_off_until_its_person_switches_it_on_and_one_that_waits_is_left_to_the_worker() {
    let world = World::new().await;
    let thread = world.thread("Fix login").await;
    let channel = system_webhook(&world).await;
    let bob = viewer(&world, "bob").await;
    follow(&world, bob, thread, "all").await;
    // Ada has made a route and not switched it on; Bob has one that waits for five minutes.
    {
        let mut conn = world.pool.acquire().await.unwrap();
        routes::put_route(
            &mut conn,
            new_id(),
            world.ada,
            channel,
            &RouteChange::default(),
            world.clock.now(),
        )
        .await
        .unwrap();
    }
    switch_on(&world, bob, channel, &[], Some(5)).await;

    completed_turn(&world, thread, "Done.").await;

    assert_eq!(told_of(&world, world.ada, "turn.completed").await, 1);
    assert_eq!(told_of(&world, bob, "turn.completed").await, 1);
    assert!(
        outbox(&world).await.is_empty(),
        "nothing leaves for a route that is off, and a route with a delay is served by the worker from the Inbox"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_destination_is_sent_each_kind_once_by_its_level_and_nothing_from_a_restricted_node_below_it()
 {
    let world = World::new().await;
    let thread = world.thread("Fix login").await;
    let at_project = destination(&world, world.project, "all", &[]).await;
    let at_topic = destination(&world, world.topic, "important", &[]).await;
    let only_failures = destination(&world, world.root, "all", &["turn.failed"]).await;

    completed_turn(&world, thread, "Done.").await;
    let sent = outbox(&world).await;
    assert_eq!(
        sent.iter().map(|row| row.2).collect::<Vec<_>>(),
        [Some(at_project)],
        "a completed turn is for level all, and the list of kinds of the third leaves it out"
    );

    failed_turn(&world, thread).await;
    let mut failed: Vec<Option<Id>> = outbox(&world)
        .await
        .iter()
        .skip(1)
        .map(|row| row.2)
        .collect();
    failed.sort();
    let mut every: Vec<Option<Id>> = [at_project, at_topic, only_failures]
        .iter()
        .map(|id| Some(*id))
        .collect();
    every.sort();
    assert_eq!(failed, every, "a failure is for important, and for all");

    // A thread whose page was cut loose is restricted: what happens in it is not for a room above it.
    let private = world
        .engine()
        .create_thread_with(
            &Caller::session(world.ada),
            NewThread {
                topic: world.topic,
                name: "Secret".to_owned(),
                agent: world.agent,
                provider: world.provider,
                environment: world.environment,
                directory: "web".to_owned(),
                arrangement: None,
                model: Some(common::MODEL.to_owned()),
                effort: None,
            },
            Birth {
                page: Some(PageChange {
                    whole: Some(Whole::Private),
                    ..PageChange::default()
                }),
                ..Birth::default()
            },
        )
        .await
        .unwrap()
        .thread;
    let before = outbox(&world).await.len();
    failed_turn(&world, private).await;
    assert_eq!(
        told_of(&world, world.ada, "turn.failed").await,
        2,
        "its people are told"
    );
    assert_eq!(
        outbox(&world).await.len(),
        before,
        "no destination is sent what happens in a restricted thread"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_destination_is_never_sent_what_people_said_or_what_is_addressed_to_one_person() {
    let world = World::new().await;
    destination(&world, world.root, "all", &[]).await;
    let mut tx = world.pool.begin().await.unwrap();
    for kind in [
        Kind::Mention,
        Kind::ChatMessage,
        Kind::AccessRequested,
        Kind::AccessDecided,
        Kind::JobFinished,
        Kind::Invitation,
    ] {
        let event = Event {
            kind,
            node: Some(world.topic),
            thread: None,
            approval: None,
            title: "Something".to_owned(),
            body: String::new(),
            data: json!({}),
            source: format!("{kind:?}"),
            actor: None,
            at: world.clock.now(),
        };
        assert!(
            tell_destinations(&mut tx, &event, true)
                .await
                .unwrap()
                .is_empty(),
            "{kind:?}"
        );
    }
    tx.commit().await.unwrap();
    assert!(outbox(&world).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owning_a_thread_and_sending_to_it_make_a_person_follow_it_unless_they_chose_otherwise() {
    let world = World::new().await;
    let bob = developer(&world, "bob").await;
    let thread = world.thread_by(bob, "Bob's thread").await;
    assert_eq!(
        subscription(&world, bob, thread).await,
        Some(("all".to_owned(), "auto".to_owned())),
        "whoever makes a thread owns it"
    );

    // Ada writes to Bob's thread and follows it from then on.
    assert_eq!(subscription(&world, world.ada, thread).await, None);
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("A word"))
        .await
        .unwrap();
    assert_eq!(
        subscription(&world, world.ada, thread).await,
        Some(("all".to_owned(), "auto".to_owned()))
    );

    // Bob switches it off; writing again does not switch it back on.
    follow(&world, bob, thread, "off").await;
    world
        .engine()
        .submit_input(&Caller::session(bob), thread, queued("Another"))
        .await
        .unwrap();
    assert_eq!(
        subscription(&world, bob, thread).await,
        Some(("off".to_owned(), "manual".to_owned()))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_service_identity_follows_nothing_and_is_told_nothing() {
    let world = World::new().await;
    let thread = world.thread("Fix login").await;
    let bot = developer(&world, "ci-bot").await;
    world
        .exec("UPDATE users SET kind = 'service', service_org_id = (SELECT node_id FROM orgs LIMIT 1) WHERE username = 'ci-bot'")
        .await;
    follow(&world, bot, thread, "all").await;

    completed_turn(&world, thread, "Done.").await;
    assert_eq!(told_of(&world, bot, "turn.completed").await, 0);
    assert_eq!(told_of(&world, world.ada, "turn.completed").await, 1);
    // And it does not follow what it writes to.
    world
        .engine()
        .submit_input(&Caller::session(bot), thread, queued("From a script"))
        .await
        .unwrap();
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM subscriptions WHERE user_id = $1 AND source = 'auto'",
    )
    .bind(bot)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_that_ended_is_told_to_whoever_asked_for_it_and_one_that_was_stopped_to_nobody() {
    let world = World::new().await;
    let thread = world.thread("Fix login").await;
    let bob = viewer(&world, "bob").await;
    let job = |state: &'static str, by: Id| {
        let world = &world;
        async move {
            let id = new_id();
            sqlx::query(
                "INSERT INTO jobs (id, kind, node_id, state, created_by, finished_at)
                 VALUES ($1, 'thread_fork', $2, $3, $4, now())",
            )
            .bind(id)
            .bind(thread)
            .bind(state)
            .bind(by)
            .execute(world.pool.pool())
            .await
            .unwrap();
            id
        }
    };
    let done = job("succeeded", bob).await;
    let stopped = job("cancelled", bob).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let told = job_finished(&mut conn, done, world.clock.now())
        .await
        .unwrap();
    assert_eq!(told.people, [bob]);
    assert!(
        job_finished(&mut conn, stopped, world.clock.now())
            .await
            .unwrap()
            .people
            .is_empty()
    );
    // Told again, it is the same row.
    assert!(
        job_finished(&mut conn, done, world.clock.now())
            .await
            .unwrap()
            .people
            .is_empty()
    );
    drop(conn);
    let rows = inbox_of(&world, bob, "job.finished").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "Fix login: fork finished");
    assert_eq!(
        told_of(&world, world.ada, "job.finished").await,
        0,
        "only its creator"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_export_keeps_its_link_in_the_completion_transaction_and_is_told_only_once() {
    let world = World::new().await;
    let export = new_id();
    let job = new_id();
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO jobs (id,kind,node_id,state,plan,created_by,finished_at,error)
         VALUES ($1,'export',$2,'failed',$3,$4,$5,$6)",
    )
    .bind(job)
    .bind(world.root)
    .bind(json!({"export":export}))
    .bind(world.ada)
    .bind(world.clock.now())
    .bind(json!({"code":"unavailable","message":"The archive could not be written."}))
    .execute(&mut *tx)
    .await
    .unwrap();
    orch_db::queries::exports::insert(
        &mut tx,
        export,
        world.ada,
        world.root,
        &json!({}),
        world.clock.now(),
    )
    .await
    .unwrap();
    orch_db::queries::exports::attach_job(&mut tx, export, job)
        .await
        .unwrap();
    assert!(
        orch_db::queries::exports::make_failed(&mut tx, export, &json!([]))
            .await
            .unwrap()
    );
    let told = job_finished(&mut tx, job, world.clock.now()).await.unwrap();
    assert_eq!(told.people, [world.ada]);
    let data: Json = sqlx::query_scalar(
        "SELECT data FROM notifications WHERE user_id=$1 AND kind='job.finished'",
    )
    .bind(world.ada)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(data["export_id"], json!(export));
    assert_eq!(data["path"], "/settings/exports");
    assert_eq!(data["state"], "failed");
    tx.commit().await.unwrap();

    // The post-commit hook may replay the same completion; it cannot replace the linked notice.
    let repeated = orch_engine::notifications::export_finished(
        &mut world.pool.acquire().await.unwrap(),
        job,
        export,
        world.ada,
        world.clock.now(),
    )
    .await
    .unwrap();
    assert!(repeated.people.is_empty());
    assert_eq!(told_of(&world, world.ada, "job.finished").await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_person_a_machine_belongs_to_is_told_once_when_it_drops_with_a_thread_running_on_it_and_not_otherwise()
 {
    let world = World::new().await;
    world
        .exec("UPDATE machines SET owner_id = (SELECT id FROM users WHERE username = 'ada')")
        .await;
    let online = |wanted: bool| {
        let world = &world;
        async move {
            let now: bool = world
                .scalar(
                    "SELECT online FROM machines WHERE id = $1",
                    world.machine_id,
                )
                .await;
            now == wanted
        }
    };
    // Nothing runs on it, and nobody has anything to look into.
    world.link_drops();
    common::eventually("the machine is away", || online(false)).await;
    common::meanwhile(Duration::from_millis(400)).await;
    assert_eq!(told_of(&world, world.ada, "machine.offline").await, 0);
    world.link_returns();
    common::eventually("the machine is back", || online(true)).await;

    // A thread runs on it, and the link drops.
    let thread = world.thread("Dropped").await;
    let (_start, _) = world.running_turn(thread, "A long job").await;
    world.link_drops();
    world.wait_blocked(thread, "agent_offline").await;
    common::eventually("its person is told", || async {
        told_of(&world, world.ada, "machine.offline").await == 1
    })
    .await;
    let told = inbox_of(&world, world.ada, "machine.offline").await;
    assert_eq!(
        told,
        [(
            "m1 went offline".to_owned(),
            "Dropped was running on it".to_owned(),
            false
        )]
    );

    // The same turn cut off again is the same news.
    world.link_returns();
    common::eventually("the machine is back", || online(true)).await;
    world.wait_state(thread, "running").await;
    world.link_drops();
    world.wait_blocked(thread, "agent_offline").await;
    common::meanwhile(Duration::from_millis(500)).await;
    assert_eq!(told_of(&world, world.ada, "machine.offline").await, 1);
}
