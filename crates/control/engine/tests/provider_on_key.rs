//! A turn on a provider that keeps a key reaches its model through the machine that keeps it: the provider is started there once, the session is opened on the forwarded path, a lease is opened before the turn and closed after it, and the key never passes through the engine in the clear.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{PROTOCOL, UPSTREAM, World, queued, turn_ended};
use orch_authz::Caller;
use orch_link_contract::commands::{
    Command, CommandKind, LeaseMode, Secret, SessionOpenProvider, TicketSlot,
};
use orch_link_contract::events::TurnOutcome;
use orch_link_contract::tunnels::TunnelKind;
use orch_types::Id;

/// A key that belongs to nobody and is recognizable wherever it might turn up.
const KEY: &str = "sk-orch-test-4f1d9c2e7b0a";

fn only<T>(mut found: Vec<T>) -> T {
    assert_eq!(found.len(), 1, "exactly one was expected");
    found.remove(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ready_key_fixture_defers_observation_without_disabling_its_normal_cadence() {
    use orch_db::queries::provider_facts::{ReadSchedule, due_reads};
    let world = World::on_a_key(KEY).await;
    let config = common::config().provider_facts;
    let schedule = || ReadSchedule {
        recent: config.recent,
        active: config.active,
        idle: config.idle,
        refused: config.refused,
    };
    let now = world.clock.now();
    let mut conn = world.pool.acquire().await.unwrap();
    let current = due_reads(&mut conn, now, schedule(), 1).await.unwrap();
    assert!(
        !current.contains(&world.provider),
        "turn tests begin with no unrelated observation due"
    );
    let later = due_reads(&mut conn, now + config.idle, schedule(), 1)
        .await
        .unwrap();
    assert!(
        later.contains(&world.provider),
        "the ordinary idle cadence still makes the same provider due"
    );
    assert!(
        world.ordered().is_empty(),
        "inspecting the schedule issues no native work"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_on_a_provider_on_a_key_starts_the_provider_opens_a_lease_and_closes_it_after_the_turn()
 {
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Fix login").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Fix the login page"),
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;

    // What the machine is asked, in this order, and nothing else.
    assert_eq!(
        world.ordered_kinds(),
        [
            CommandKind::ProviderStart,
            CommandKind::AgentStart,
            CommandKind::SessionOpen,
            CommandKind::LeaseOpen,
            CommandKind::TurnStart
        ]
    );
    assert!(
        world
            .ordered()
            .iter()
            .all(|(machine, _)| *machine == world.machine_id)
    );

    // The provider is started with its endpoints and with the key as a reference to its secret.
    let Command::ProviderStart(started) = only(world.seen.of_kind(CommandKind::ProviderStart))
    else {
        unreachable!()
    };
    assert_eq!(started.provider_id, world.provider);
    assert_eq!(started.protocols, [PROTOCOL]);
    assert_eq!(started.upstreams.len(), 1);
    assert_eq!(started.upstreams[0].protocol, PROTOCOL);
    assert_eq!(started.upstreams[0].base_url, UPSTREAM);
    let secret: Id = world
        .scalar(
            "SELECT id FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1 AND name = 'api_key'",
            world.provider,
        )
        .await;
    assert_eq!(started.credential, Some(Secret::Ref(secret)));
    assert!(started.secrets.is_empty());

    // The session is opened on the forwarded path, with the request for the ticket that opens the tunnel of
    // the lease, which the link signs when the frame is written.
    let lease: Id = world
        .scalar("SELECT id FROM leases WHERE thread_id = $1", thread)
        .await;
    let Command::SessionOpen(open) = only(world.seen.of_kind(CommandKind::SessionOpen)) else {
        unreachable!()
    };
    let SessionOpenProvider::Forwarded {
        provider_id,
        protocol,
        ticket,
    } = &open.provider
    else {
        panic!("the session was opened on {:?}", open.provider);
    };
    assert_eq!(
        (*provider_id, protocol.as_str()),
        (world.provider, PROTOCOL)
    );
    let TicketSlot::Asked(asked) = ticket else {
        panic!("a ticket is signed at send time, not before");
    };
    assert_eq!(asked.ticket.kind, TunnelKind::Llm);
    assert_eq!(
        asked.ticket.more["service"],
        format!("llm/{lease}").as_str()
    );
    assert_eq!(
        asked.ticket.more["dst"],
        world.machine_id.to_string().as_str()
    );
    assert_eq!(asked.ticket.more["thread_id"], thread.to_string().as_str());
    assert_eq!(
        asked.ticket.more["provider_id"],
        world.provider.to_string().as_str()
    );

    // The lease is the one pre-flight opened, for the model of the turn alone, with no allowance since no
    // cap is evaluated yet.
    let Command::LeaseOpen(lease_open) = only(world.seen.of_kind(CommandKind::LeaseOpen)) else {
        unreachable!()
    };
    assert_eq!(lease_open.lease_id, lease);
    assert_eq!(lease_open.provider_id, world.provider);
    assert_eq!(lease_open.thread_id, thread);
    assert_eq!(lease_open.mode, LeaseMode::Forwarded);
    assert_eq!(lease_open.protocol.as_deref(), Some(PROTOCOL));
    assert_eq!(lease_open.allowed_models, [common::MODEL]);
    assert!(lease_open.allowance.is_none());
    assert!(lease_open.recipient.is_none() && lease_open.form.is_none());
    assert_eq!(start.model, common::MODEL);

    // The key is in no command that was stored, and in no row of the database but its own, sealed.
    for (_, body) in world.commands.issued_bodies("provider.start") {
        let text = body.to_string();
        assert!(!text.contains(KEY), "{text}");
        assert!(text.contains("$secret"), "{text}");
    }
    assert!(world.tables_holding(KEY).await.is_empty());

    // The turn ends, and the machine is told that the lease is closed.
    world.say(thread, common::turn_began(start.turn_id)).await;
    world
        .say(thread, common::message_began(start.turn_id, "m1"))
        .await;
    world.say(thread, common::message_done("m1", "Done.")).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    world.wait_ran(CommandKind::LeaseClose, 1).await;
    assert_eq!(
        world.ordered_kinds(),
        [
            CommandKind::ProviderStart,
            CommandKind::AgentStart,
            CommandKind::SessionOpen,
            CommandKind::LeaseOpen,
            CommandKind::TurnStart,
            CommandKind::LeaseClose
        ]
    );
    let Command::LeaseClose(closed) = only(world.seen.of_kind(CommandKind::LeaseClose)) else {
        unreachable!()
    };
    assert_eq!(closed.lease_id, lease);
    let reason: Option<String> = world
        .scalar("SELECT close_reason FROM leases WHERE id = $1", lease)
        .await;
    assert_eq!(reason.as_deref(), Some("turn_ended"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_turn_opens_its_own_lease_and_does_not_start_the_provider_again() {
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Fix login").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("First"))
        .await
        .unwrap();
    let first = world.run_turn(thread, 0, "One.").await;
    world.wait_ran(CommandKind::LeaseClose, 1).await;
    world.wait_state(thread, "idle").await;

    world
        .engine()
        .submit_input(&ada, thread, queued("Second"))
        .await
        .unwrap();
    let second = world.run_turn(thread, 1, "Two.").await;
    world.wait_ran(CommandKind::LeaseClose, 2).await;

    assert_ne!(first.turn_id, second.turn_id);
    assert_eq!(world.ran(CommandKind::ProviderStart), 1);
    assert_eq!(world.ran(CommandKind::AgentStart), 1);
    assert_eq!(world.ran(CommandKind::SessionOpen), 1);
    assert_eq!(
        world.ordered_kinds()[6..],
        [
            CommandKind::LeaseOpen,
            CommandKind::TurnStart,
            CommandKind::LeaseClose
        ]
    );
    assert_eq!(world.commands.issued_of("provider.start"), 1);
    // Each turn had a lease of its own, and each is closed.
    let leases: i64 = world
        .count(
            "SELECT count(*) FROM leases WHERE thread_id = $1 AND closed_at IS NOT NULL",
            thread,
        )
        .await;
    assert_eq!(leases, 2);
}

// ---------------------------------------------------------------- the key on a machine of its own

use common::Added;
use orch_link_contract::commands::{CommandError, ErrorCode};
use std::time::Duration;

/// The key is kept on a machine that does nothing else, and the harness runs where the work is: the
/// arrangement is the one in which the key stays where it is and the harness is beside its tools.
async fn keep_the_key_elsewhere(world: &World) -> Added {
    let keeper = world.add_machine("keeper", &["provider"]).await;
    // The forwarder runs on the local keeper; only the agent runs in the work environment.
    // No absent managed runtime or server-owned key is part of this registration.
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("UPDATE providers SET machine_id=$2,key_machine_id=$2 WHERE id=$1")
        .bind(world.provider)
        .bind(keeper.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "DELETE FROM secrets WHERE owner_kind='provider' AND owner_id=$1 AND name='api_key'",
    )
    .bind(world.provider)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE env_templates SET spec=jsonb_set(spec,'{colocated_harnesses}','[\"fake\"]'::jsonb)
        WHERE id=(SELECT template_id FROM environments WHERE id=$1)",
    )
    .bind(world.environment)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    keeper
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_kept_on_another_machine_is_not_sent_and_each_command_goes_to_the_machine_it_is_for()
{
    let world = World::on_a_key(KEY).await;
    let keeper = keep_the_key_elsewhere(&world).await;
    let thread = world.thread("Two machines").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    world.turn_start(thread, 0).await;

    // The key's machine starts the provider (without the key, which it has) and holds the lease; the harness
    // and its session are where the work is.
    let machines: Vec<(Id, CommandKind)> = world
        .ordered()
        .iter()
        .map(|(machine, command)| (*machine, command.kind()))
        .collect();
    assert_eq!(
        machines,
        [
            (keeper.id, CommandKind::ProviderStart),
            (world.machine_id, CommandKind::AgentStart),
            (world.machine_id, CommandKind::SessionOpen),
            (keeper.id, CommandKind::LeaseOpen),
            (world.machine_id, CommandKind::TurnStart)
        ]
    );
    let Command::ProviderStart(started) = only(keeper.seen.of_kind(CommandKind::ProviderStart))
    else {
        unreachable!()
    };
    assert_eq!(
        started.credential, None,
        "the key is on that machine already"
    );
    // The ticket that opens the tunnel of the lease is for the machine that keeps the key.
    let lease: Id = world.open_lease_of(thread).await;
    let Command::SessionOpen(open) = only(world.seen.of_kind(CommandKind::SessionOpen)) else {
        unreachable!()
    };
    let SessionOpenProvider::Forwarded { ticket, .. } = &open.provider else {
        panic!("the session was opened on {:?}", open.provider);
    };
    let TicketSlot::Asked(asked) = ticket else {
        panic!("a ticket is signed at send time");
    };
    assert_eq!(
        asked.ticket.more["service"],
        format!("llm/{lease}").as_str()
    );
    assert_eq!(asked.ticket.more["dst"], keeper.id.to_string().as_str());
    assert_eq!(world.arrangement_of(thread).await, "key_separated");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_whose_machine_is_away_blocks_its_threads_and_they_go_on_when_it_returns() {
    let world = World::on_a_key(KEY).await;
    let keeper = keep_the_key_elsewhere(&world).await;
    let thread = world.thread("Away").await;
    world.set_machine_online(keeper.id, false).await;
    common::eventually("the provider stands offline", || async {
        let status: String = world
            .scalar("SELECT status FROM providers WHERE id = $1", world.provider)
            .await;
        status == "offline"
    })
    .await;

    let sent = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("While away"))
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_unavailable").await;
    let detail: Option<String> = world
        .scalar(
            "SELECT blocked_detail FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    assert_eq!(detail.as_deref(), Some("offline"));
    assert_eq!(world.input_state(sent.input.id).await.0, "pending");
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id=$1", thread)
            .await,
        0
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM leases WHERE thread_id=$1", thread)
            .await,
        0
    );
    assert_eq!(
        world.commands.issued_of("fs.record"),
        0,
        "an unavailable provider admits no recording work"
    );
    common::meanwhile(Duration::from_millis(400)).await;
    assert!(
        world.ordered().is_empty(),
        "nothing is asked of any machine meanwhile"
    );

    // The machine returns: the provider is ready again, and the input has its turn.
    world.set_machine_online(keeper.id, true).await;
    world.turn_start(thread, 0).await;
    let status: String = world
        .scalar("SELECT status FROM providers WHERE id = $1", world.provider)
        .await;
    assert_eq!(status, "ready");
    assert_eq!(keeper.seen.of_kind(CommandKind::ProviderStart).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_machine_that_does_not_take_the_role_of_a_provider_cannot_keep_the_key() {
    let world = World::on_a_key(KEY).await;
    let keeper = keep_the_key_elsewhere(&world).await;
    let thread = world.thread("No role").await;
    world
        .exec(&format!(
            "UPDATE machines SET roles = ARRAY['environment'] WHERE id = '{}'",
            keeper.id
        ))
        .await;
    let sent = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_unavailable").await;
    let detail: Option<String> = world
        .scalar(
            "SELECT blocked_detail FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    assert_eq!(
        detail.as_deref(),
        Some("the machine that keeps the key does not take the role of a provider")
    );
    assert_eq!(world.input_state(sent.input.id).await.0, "pending");
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id=$1", thread)
            .await,
        0
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM leases WHERE thread_id=$1", thread)
            .await,
        0
    );
    assert_eq!(
        world.commands.issued_of("fs.record"),
        0,
        "the provider role is checked before preparation"
    );
    common::meanwhile(Duration::from_millis(400)).await;
    assert!(world.ordered().is_empty());
}

// ---------------------------------------------------------------- a provider that cannot be started

async fn start_refusal_is_told(world: &World, thread: Id, refusal: &CommandError) {
    world.wait_told_blocked(thread, "binding_invalid").await;
    let detail:Option<String>=world.scalar(
        "SELECT data ->> 'detail' FROM notifications
         WHERE thread_id=$1 AND kind='thread.blocked' AND data ->> 'reason'='binding_invalid' LIMIT 1",
        thread,
    ).await;
    let expected = format!("provider.start: {}: {}", refusal.code, refusal.detail);
    assert_eq!(
        detail.as_deref(),
        Some(expected.as_str()),
        "followers receive the exact native refusal"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_machine_that_cannot_serve_the_provider_fails_the_turn_and_blocks_the_binding() {
    let world = World::on_a_key(KEY).await;
    let refusal = CommandError::new(
        ErrorCode::Unsupported,
        "this machine has no implementation of the protocol",
    );
    world.refuse(CommandKind::ProviderStart, vec![refusal.clone()]);
    let thread = world.thread("Cannot").await;
    let sent = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    start_refusal_is_told(&world, thread, &refusal).await;
    // Nothing after the start that failed was asked for, and the input does not wait for what cannot be.
    assert_eq!(world.ordered_kinds(), [CommandKind::ProviderStart]);
    assert_eq!(world.input_state(sent.input.id).await.0, "failed");
    let open: i64 = world
        .count(
            "SELECT count(*) FROM leases WHERE thread_id = $1 AND closed_at IS NULL",
            thread,
        )
        .await;
    assert_eq!(open, 0, "the lease of the turn that failed is closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_that_failed_for_a_reason_that_passes_is_asked_for_again_by_the_next_turn() {
    let world = World::on_a_key(KEY).await;
    world.refuse(
        CommandKind::ProviderStart,
        vec![CommandError::new(
            ErrorCode::OfflinePeer,
            "away for a moment",
        )],
    );
    let thread = world.thread("Passes").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    // The first start failed; the input waits, and the second turn starts the provider under the next attempt.
    world.turn_start(thread, 0).await;
    assert_eq!(world.ran(CommandKind::ProviderStart), 2);
    assert_eq!(world.commands.issued_of("provider.start"), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_machine_that_lost_the_provider_is_asked_to_start_it_again_once_and_the_lease_is_opened_again()
 {
    let world = World::on_a_key(KEY).await;
    world.refuse(
        CommandKind::LeaseOpen,
        vec![CommandError::new(
            ErrorCode::NotFound,
            "this machine holds neither the key nor the sign-in of that provider",
        )],
    );
    let thread = world.thread("Lost").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    world.turn_start(thread, 0).await;
    assert_eq!(
        world.ordered_kinds(),
        [
            CommandKind::ProviderStart,
            CommandKind::AgentStart,
            CommandKind::SessionOpen,
            CommandKind::LeaseOpen,
            CommandKind::ProviderStart,
            CommandKind::LeaseOpen,
            CommandKind::TurnStart
        ]
    );
}

// ---------------------------------------------------------------- a provider that is switched off

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_that_is_switched_off_is_stopped_once_its_running_turn_is_interrupted_and_its_threads_block()
 {
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Off").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;

    // As the routes switch a provider off.
    world
        .exec(
            "UPDATE providers SET status = 'disabled', revision = revision + 1, updated_at = now()",
        )
        .await;
    world.wait_ran(CommandKind::ProviderStop, 1).await;
    world.wait_turn(start.turn_id, "interrupted").await;
    let reason: Option<String> = world
        .scalar("SELECT close_reason FROM leases WHERE id = $1", lease)
        .await;
    assert_eq!(reason.as_deref(), Some("disabled"));
    let Command::ProviderStop(stopped) = only(world.seen.of_kind(CommandKind::ProviderStop)) else {
        unreachable!()
    };
    assert_eq!(stopped.provider_id, world.provider);
    world.wait_ran(CommandKind::TurnInterrupt, 1).await;

    // A new input waits for the provider, and the machine is not asked to stop it again.
    let sent = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Again"))
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_unavailable").await;
    let detail: Option<String> = world
        .scalar(
            "SELECT blocked_detail FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    assert_eq!(detail.as_deref(), Some("disabled"));
    common::meanwhile(Duration::from_millis(500)).await;
    assert_eq!(world.input_state(sent.input.id).await.0, "pending");
    assert_eq!(world.ran(CommandKind::ProviderStop), 1);
    assert_eq!(world.turn_starts(thread).len(), 1);

    // Switched on again, it runs the input; the machine still holds it as far as the link's rows say.
    world
        .exec("UPDATE providers SET status = 'ready', revision = revision + 1, updated_at = now()")
        .await;
    world.turn_start(thread, 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_that_is_deleted_is_stopped_and_its_threads_wait_with_that_reason() {
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Deleted").await;
    world
        .exec("UPDATE providers SET deleted_at = now(), status = 'disabled', revision = revision + 1, updated_at = now()")
        .await;
    world.wait_ran(CommandKind::ProviderStop, 1).await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_unavailable").await;
    let detail: Option<String> = world
        .scalar(
            "SELECT blocked_detail FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    assert_eq!(detail.as_deref(), Some("deleted"));
}

// ---------------------------------------------------------------- a lease that no turn holds

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lease_left_open_by_a_turn_that_is_over_is_closed_and_the_machine_is_told() {
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Left open").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;

    // The turn is over by something other than the engine's own end of a turn.
    world
        .exec("UPDATE turns SET state = 'completed', completed_at = now()")
        .await;
    common::eventually("the lease is closed", || async {
        let closed: Option<chrono::DateTime<chrono::Utc>> = world
            .scalar("SELECT closed_at FROM leases WHERE id = $1", lease)
            .await;
        closed.is_some()
    })
    .await;
    let reason: Option<String> = world
        .scalar("SELECT close_reason FROM leases WHERE id = $1", lease)
        .await;
    assert_eq!(reason.as_deref(), Some("turn_ended"));
    world.wait_ran(CommandKind::LeaseClose, 1).await;
    let Command::LeaseClose(closed) = only(world.seen.of_kind(CommandKind::LeaseClose)) else {
        unreachable!()
    };
    assert_eq!(closed.lease_id, lease);
    // It is closed once, however often the supervisor looks.
    common::meanwhile(Duration::from_millis(400)).await;
    assert_eq!(world.ran(CommandKind::LeaseClose), 1);
    let _ = start;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lease_that_was_never_opened_on_the_machine_is_not_closed_there() {
    let world = World::on_a_key(KEY).await;
    let refusal = CommandError::new(ErrorCode::Unsupported, "no");
    world.refuse(CommandKind::ProviderStart, vec![refusal.clone()]);
    let thread = world.thread("Never opened").await;
    let sent = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    start_refusal_is_told(&world, thread, &refusal).await;
    assert_eq!(world.input_state(sent.input.id).await.0, "failed");
    common::meanwhile(Duration::from_millis(500)).await;
    assert_eq!(world.ran(CommandKind::ProviderStart), 1);
    assert_eq!(world.ran(CommandKind::LeaseOpen), 0);
    assert_eq!(world.ran(CommandKind::LeaseClose), 0);
}
