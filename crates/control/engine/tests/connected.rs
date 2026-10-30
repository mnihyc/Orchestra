//! A directory on a person's machine follows its machine: it is attached when the machine has a session, in every session, with what its home gives it, and stands offline when the machine is away; a turn that needs it waits for the attach instead of being blocked.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use chrono::Utc;
use common::{World, eventually, master_key, queued};
use hosting::{Unit, connector, directory, primary};
use orch_authz::Caller;
use orch_link_contract::commands::{CommandError, CommandKind as K, ErrorCode};
use orch_types::Id;
use serde_json::json;
use std::sync::Mutex;

/// The status of a directory, and what is said of it.
async fn status(world: &World, environment: Id) -> (String, Option<String>) {
    hosting::status(world, environment).await
}

/// Waits until a directory stands in a status, and says what was said of it when it was seen there.
async fn wait_status(world: &World, environment: Id, wanted: &str) -> Option<String> {
    let said = Mutex::new(None);
    eventually(&format!("the directory to be {wanted}"), || async {
        let (now, detail) = status(world, environment).await;
        if now == wanted {
            *said.lock().unwrap() = Some(detail);
        }
        now == wanted
    })
    .await;
    said.into_inner().unwrap().unwrap()
}

/// A person's laptop, away until a test brings it online, with a directory registered on it.
async fn laptop_with_directory(world: &World, roles: &[&str]) -> (Unit, Id) {
    let laptop = connector(world, "laptop", roles).await;
    hosting::physical::report(world, laptop.id, true, Some(true), "open").await;
    world.set_machine_online(laptop.id, false).await;
    let environment = directory(world, laptop.id, "Work", "/home/ada/work").await;
    (laptop, environment)
}

#[tokio::test]
async fn directory_commands_hold_readiness_skip_when_off_and_record_outcomes_without_output() {
    use orch_link_contract::commands::{Command, CommandResult, UnitAfterUpResult};
    for case in ["allowed", "off", "failed"] {
        let world = World::new().await;
        let (laptop, environment) = laptop_with_directory(&world, &["environment", "agent"]).await;
        let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
            "test",
            orch_driver_contract::descriptor::EnvironmentDataStorage {
                made_by_orch: None,
                existing_directory: true,
                reason: None,
            },
        );
        sqlx::query("UPDATE machines SET facts=jsonb_set(facts || $2,'{policy}',COALESCE(facts->'policy','{}') || $3) WHERE id=$1")
            .bind(laptop.id).bind(json!({"environment_descriptor":descriptor}))
            .bind(json!({"environment.after_up":case!="off"})).execute(world.pool.pool()).await.unwrap();
        sqlx::query(
            "UPDATE environments SET config=jsonb_set(config,'{env.after_up}',$2) WHERE id=$1",
        )
        .bind(environment)
        .bind(json!({"10-prepare":{"command":["prepare"]}}))
        .execute(world.pool.pool())
        .await
        .unwrap();
        laptop
            .added
            .machine
            .handler
            .on(K::UnitAfterUp, move |command| {
                let Command::UnitAfterUp(body) = command else {
                    unreachable!()
                };
                Ok(CommandResult::UnitAfterUp(UnitAfterUpResult {
                    name: body.name.clone(),
                    exit: Some(if case == "failed" { 7 } else { 0 }),
                    duration_ms: 1,
                    timed_out: false,
                    first_lines: "safe diagnostic".into(),
                }))
            });
        if case == "allowed" {
            laptop.hold(K::UnitAfterUp);
        }
        world.set_machine_online(laptop.id, true).await;
        if case == "allowed" {
            eventually("the directory setup command to be held", || async {
                laptop.began(K::UnitAfterUp) >= 1
            })
            .await;
            assert_ne!(status(&world, environment).await.0, "running");
            laptop.let_go();
        }
        let said = wait_status(
            &world,
            environment,
            if case == "failed" {
                "failed"
            } else {
                "running"
            },
        )
        .await;
        if case == "failed" {
            assert!(said.unwrap().contains("safe diagnostic"));
        }
        let history: Vec<serde_json::Value> = sqlx::query_scalar(
            "SELECT details FROM audit_log WHERE object_id=$1 AND action='unit.command_ran'",
        )
        .bind(environment)
        .fetch_all(world.pool.pool())
        .await
        .unwrap();
        if case == "off" {
            assert!(history.is_empty());
            assert_eq!(laptop.began(K::UnitAfterUp), 0);
            let skipped: serde_json::Value =
                sqlx::query_scalar("SELECT facts->'after_up' FROM environments WHERE id=$1")
                    .bind(environment)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap();
            assert_eq!(skipped["state"], "skipped");
            assert!(
                skipped["reason"]
                    .as_str()
                    .unwrap()
                    .contains("environment.after_up")
            );
        } else {
            assert_eq!(history.len(), 1);
            assert_eq!(history[0]["name"], "10-prepare");
            assert_eq!(history[0]["exit"], if case == "failed" { 7 } else { 0 });
            assert!(history[0].get("first_lines").is_none() && history[0].get("command").is_none());
        }
    }
}

#[tokio::test]
async fn a_directory_is_attached_when_its_machine_has_a_session_with_what_its_home_gives_it_and_then_it_runs()
 {
    let world = World::new().await;
    let (laptop, environment) = laptop_with_directory(&world, &["agent", "environment"]).await;
    // What the directory is given: a variable, a secret, and the executor its exec node offers.
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::settings::testing::set_environment_config(
        &mut conn,
        environment,
        &json!({
            "env.variables": { "EDITOR": "vi" },
            "env.secret_names": ["API_TOKEN"],
            "exec.offers": ["orch-exec"],
            "workspace.index": "live",
        }),
    )
    .await
    .unwrap();
    let sealed = master_key()
        .seal(
            b"tok-9f2",
            format!("secrets/environment/{environment}/API_TOKEN").as_bytes(),
        )
        .unwrap();
    let secret = orch_db::queries::settings::put_secret(
        &mut conn,
        "environment",
        environment,
        "API_TOKEN",
        &sealed,
        None,
        Utc::now(),
    )
    .await
    .unwrap();
    drop(conn);
    // The machine is away: the directory stays as it was registered.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(status(&world, environment).await.0, "none");
    assert!(
        laptop.kinds().is_empty(),
        "nothing is sent to a machine that has no session"
    );
    // The machine has a session and is held at the executor: the directory is not up before its
    // executors are.
    laptop.hold(K::ExecStart);
    world.set_machine_online(laptop.id, true).await;
    eventually("the executor to be asked for", || async {
        laptop.began(K::ExecStart) == 1
    })
    .await;
    assert_eq!(
        status(&world, environment).await.0,
        "none",
        "it is not running while its executors are being started"
    );
    laptop.let_go();
    wait_status(&world, environment, "running").await;
    assert_eq!(laptop.kinds(), [K::EnvAttach, K::ExecStart]);
    let attach = &laptop.calls_of(K::EnvAttach)[0].body;
    assert_eq!(attach["environment_id"], json!(environment));
    assert_eq!(
        attach["workspace_root"], "/home/ada/work",
        "the directory it was registered with"
    );
    assert!(
        attach["data_sources"].is_null(),
        "a registry default must not ask the connector to replace this registered directory"
    );
    assert_eq!(attach["variables"], json!({ "EDITOR": "vi" }));
    assert_eq!(
        attach["index"], "live",
        "the place chose a listing, not an index of the whole tree"
    );
    assert_eq!(
        attach["secrets"]["API_TOKEN"]["$secret"],
        json!(secret.id),
        "the secret goes as a reference the link seals, never as its value"
    );
    assert!(!attach.to_string().contains("tok-9f2"));
    assert_eq!(
        laptop.calls_of(K::ExecStart)[0].body["executor"],
        "orch-exec"
    );
    // What the connector said of the environment is kept, and the registered root is not replaced.
    let (os, root): (String, String) =
        sqlx::query_as("SELECT facts ->> 'os', workspace_root FROM environments WHERE id = $1")
            .bind(environment)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!((os.as_str(), root.as_str()), ("linux", "/home/ada/work"));
    // A session that goes on is not attached again.
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(laptop.count(K::EnvAttach), 1);
}

#[tokio::test]
async fn the_offers_of_an_exec_node_are_started_as_the_executors_that_carry_them_out() {
    let world = World::new().await;
    let (laptop, environment) = laptop_with_directory(&world, &["agent", "environment"]).await;
    // As the Default template offers: the tool sets of Orch's executor are no executors of their own.
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::settings::testing::set_environment_config(
        &mut conn,
        environment,
        &json!({ "exec.offers": ["codex-exec-server", "claude-code", "plain"] }),
    )
    .await
    .unwrap();
    drop(conn);
    world.set_machine_online(laptop.id, true).await;
    wait_status(&world, environment, "running").await;
    let started: Vec<String> = laptop
        .calls_of(K::ExecStart)
        .iter()
        .map(|call| call.body["executor"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(started, ["codex-exec-server", "orch-exec"]);
    assert_eq!(
        laptop.calls_of(K::EnvAttach)[0].body["index"],
        "full",
        "where nobody chose, the whole tree is indexed"
    );
}

#[tokio::test]
async fn a_machine_that_goes_away_leaves_its_directory_offline_and_in_its_next_session_it_is_attached_again()
 {
    let world = World::new().await;
    let (laptop, environment) = laptop_with_directory(&world, &["agent", "environment"]).await;
    world.set_machine_online(laptop.id, true).await;
    wait_status(&world, environment, "running").await;
    assert_eq!(laptop.count(K::EnvAttach), 1);
    world.set_machine_online(laptop.id, false).await;
    let said = wait_status(&world, environment, "offline").await;
    assert_eq!(said.as_deref(), Some("its machine is offline"));
    // The connector kept nothing across its restart: the next session attaches the directory anew.
    world.set_machine_online(laptop.id, true).await;
    wait_status(&world, environment, "running").await;
    assert_eq!(laptop.count(K::EnvAttach), 2);
    assert_eq!(status(&world, environment).await.1, None);
    for attach in laptop.calls_of(K::EnvAttach) {
        assert_eq!(attach.body["workspace_root"], "/home/ada/work");
        assert!(
            attach.body["data_sources"].is_null(),
            "the stored directory remains a borrowed root in every session"
        );
    }
}

#[tokio::test]
async fn explicit_own_and_inherited_data_choices_are_kept_even_when_they_equal_the_registry_default()
 {
    use orch_driver_contract::runtime::EnvironmentDataSources;
    use orch_settings_contract::{SourceScope, keys};
    for (own, chosen) in [
        (true, json!({})),
        (false, json!({})),
        (
            true,
            json!({"workspace":{"kind":"made_by_orch"},"home":{"kind":"made_by_orch"}}),
        ),
        (
            false,
            json!({"workspace":{"kind":"made_by_orch"},"home":{"kind":"made_by_orch"}}),
        ),
        (
            true,
            json!({"workspace":{"kind":"existing_directory","path":"/home/ada/selected-work"},"home":{"kind":"existing_directory","path":"/home/ada/selected-home"}}),
        ),
        (
            false,
            json!({"workspace":{"kind":"made_by_orch"},"home":{"kind":"existing_directory","path":"/home/ada/selected-home"}}),
        ),
    ] {
        let world = World::new().await;
        let (laptop, environment) = laptop_with_directory(&world, &["agent", "environment"]).await;
        if own {
            let mut conn = world.pool.acquire().await.unwrap();
            orch_db::queries::settings::testing::set_environment_config(
                &mut conn,
                environment,
                &json!({"env.data_sources":&chosen}),
            )
            .await
            .unwrap();
        } else {
            world
                .set(world.topic, keys::ENV_DATA_SOURCES, chosen.clone())
                .await;
        }
        let resolved = {
            let mut conn = world.pool.acquire().await.unwrap();
            orch_settings::resolve(
                &mut conn,
                orch_settings::Place::Environment(environment),
                keys::ENV_DATA_SOURCES,
            )
            .await
            .unwrap()
            .resolved
        };
        assert_eq!(
            resolved.source.scope,
            if own {
                SourceScope::EnvironmentOwn
            } else {
                SourceScope::Topic
            }
        );
        let expected: EnvironmentDataSources = serde_json::from_value(chosen).unwrap();
        world.set_machine_online(laptop.id, true).await;
        wait_status(&world, environment, "running").await;
        let calls = laptop.calls_of(K::EnvAttach);
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].body["data_sources"],
            serde_json::to_value(expected).unwrap(),
            "an explicit choice, including an empty choice and an independent home, must reach the connector"
        );
    }
}

#[tokio::test]
async fn a_session_that_replaced_another_before_anyone_saw_the_machine_go_is_attached_in_too() {
    let world = World::new().await;
    let (laptop, environment) = laptop_with_directory(&world, &["agent", "environment"]).await;
    world.set_machine_online(laptop.id, true).await;
    wait_status(&world, environment, "running").await;
    // The machine says it is here again: the database never saw it go, but the session is another one.
    world.set_machine_online(laptop.id, true).await;
    eventually("the second attach", || async {
        laptop.count(K::EnvAttach) == 2
    })
    .await;
    wait_status(&world, environment, "running").await;
}

#[tokio::test]
async fn an_attach_the_machine_refuses_fails_the_directory_with_what_it_said_and_is_tried_again_after_a_pause()
 {
    let world = World::new().await;
    let (laptop, environment) = laptop_with_directory(&world, &["agent", "environment"]).await;
    // The second try is held at the machine, so that the failed directory stands for as long as the
    // test looks at it, however long that takes on a busy machine.
    laptop.refuse_attach_then_hold(CommandError::new(
        ErrorCode::PolicyRootNotAllowed,
        "its owner offers no such directory",
    ));
    world.set_machine_online(laptop.id, true).await;
    let detail = wait_status(&world, environment, "failed").await.unwrap();
    assert!(detail.contains("its machine refused it"), "{detail}");
    assert!(detail.contains("policy.root_not_allowed"), "{detail}");
    assert!(
        detail.contains("its owner offers no such directory"),
        "{detail}"
    );
    // After a pause it is tried again, in the same session; the machine has not answered yet.
    eventually("the second try", || async {
        laptop.began(K::EnvAttach) == 2
    })
    .await;
    assert_eq!(status(&world, environment).await.0, "failed");
    // Its owner changed the policy meanwhile: the next try is let through.
    laptop.let_go();
    wait_status(&world, environment, "running").await;
    assert_eq!(laptop.count(K::EnvAttach), 2);
    assert_eq!(status(&world, environment).await.1, None);
}

#[tokio::test]
async fn a_machine_that_does_not_serve_environments_cannot_have_a_directory_attached() {
    let world = World::new().await;
    let (laptop, environment) = laptop_with_directory(&world, &["agent"]).await;
    world.set_machine_online(laptop.id, true).await;
    let detail = wait_status(&world, environment, "failed").await.unwrap();
    assert!(
        detail.contains("does not take the role of an environment"),
        "{detail}"
    );
    assert_eq!(
        laptop.count(K::EnvAttach),
        0,
        "nothing the machine cannot do is asked of it"
    );
}

#[tokio::test]
async fn a_turn_waits_for_the_attach_of_its_directory_and_runs_when_it_is_up() {
    let mut world = World::new().await;
    // The world's own machine is the person's machine: it holds the attach back for a while.
    let machine = primary(&world);
    hosting::physical::report(&world, machine.id, true, Some(true), "open").await;
    machine.hold(K::EnvAttach);
    let environment = directory(&world, machine.id, "Work", "/home/ada/work").await;
    hosting::physical::existing_thread_directory(&world, &machine, environment, "web");
    world.environment = environment;
    let thread = world.thread("work").await;
    let submitted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("hello"))
        .await
        .unwrap();
    // The thread says what it waits for; it is not blocked, and nothing starts in the directory.
    hosting::physical::wait(
        &world,
        environment,
        thread,
        "the thread to wait for its environment",
        || async { world.state(thread).await.2.as_deref() == Some("environment") },
    )
    .await;
    assert_eq!(world.state(thread).await.0, "starting");
    // The shared provider process can be up for discovery; directory sessions still wait for attach.
    assert_eq!(world.ran(K::SessionOpen), 0);
    assert_eq!(world.ran(K::TurnStart), 0);
    assert_eq!(machine.began(K::FsStat), 0);
    assert_eq!(machine.began(K::FsRecord), 0);
    assert_eq!(machine.began(K::FsMkdir), 0);
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
    assert_eq!(world.input_state(submitted.input.id).await.0, "pending");
    machine.let_go();
    hosting::physical::wait(
        &world,
        environment,
        thread,
        "the turn to be handed to the harness",
        || async { world.ran(K::TurnStart) == 1 },
    )
    .await;
    assert_eq!(status(&world, environment).await.0, "running");
    let start = world.turn_start(thread, 0).await;
    let input: Id = world
        .scalar("SELECT input_id FROM turns WHERE id=$1", start.turn_id)
        .await;
    assert_eq!(input, submitted.input.id);
    world.run_turn(thread, 0, "Hello!").await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test]
async fn a_directory_without_a_template_runs_with_its_machines_full_access_ceiling() {
    use orch_agent_contract::descriptor::ArrangementType;
    use orch_engine::binding::{CreateThreadError, NewThread};
    use orch_link_contract::commands::CommandResult;
    use orch_settings_contract::registry::keys;
    for machine_max in ["open", "workspace"] {
        let mut world = World::new().await;
        // This native profile runs at its actual subscription keeper, without a credential copy.
        assert_eq!(
            sqlx::query(
                "UPDATE providers SET key_machine_id=machine_id WHERE id=$1 AND kind='subscription'"
            )
            .bind(world.provider)
            .execute(world.pool.pool())
            .await
            .unwrap()
            .rows_affected(),
            1
        );
        let machine = primary(&world);
        hosting::physical::report(&world, machine.id, true, Some(true), machine_max).await;
        sqlx::query("UPDATE machines SET name='world-box' WHERE id=$1")
            .bind(machine.id)
            .execute(world.pool.pool())
            .await
            .unwrap();
        machine.hold(K::EnvAttach);
        machine.added.machine.handler.on(K::EnvAttach, |command| {
            let mut facts = hosting::facts();
            facts["executors"][0]["sandbox_modes"] = json!(["open"]);
            if let orch_link_contract::commands::Command::EnvAttach(attach) = command {
                facts["workspace_root"] = json!(attach.workspace_root);
                facts["generation"] = json!(attach.data_generation.unwrap_or(0));
            }
            Ok(CommandResult::EnvAttach(
                serde_json::from_value(facts).unwrap(),
            ))
        });
        let environment = directory(&world, machine.id, "Acme workspace", "/home/ada/work").await;
        hosting::physical::existing_thread_directory(&world, &machine, environment, "web");
        world.environment = environment;
        world
            .set(world.topic, keys::SANDBOX_MODE, json!("open"))
            .await;
        let request = || NewThread {
            topic: world.topic,
            name: "work".into(),
            agent: world.agent,
            provider: world.provider,
            environment,
            directory: "web".into(),
            arrangement: Some(ArrangementType::Separated),
            model: Some(common::MODEL.into()),
            effort: None,
        };
        // A template-free directory promises no remote executor before its real probe.
        // Together would instead select the fake harness's own Workspace-capable tools.
        let unprobed = world
            .engine()
            .create_thread(&Caller::session(world.ada), request())
            .await;
        assert!(
            matches!(
                unprobed,
                Err(CreateThreadError::Incompatible(
                    orch_agent_contract::arrangement::Incompatible::ExecutorMissing { .. }
                ))
            ),
            "an unknown executor cannot supply a separated binding before its probe"
        );
        assert_eq!(world.ran(K::SessionOpen), 0);
        assert_eq!(world.ran(K::TurnStart), 0);
        assert_eq!(
            world
                .count(
                    "SELECT count(*) FROM threads WHERE topic_id=$1",
                    world.topic
                )
                .await,
            0
        );
        machine.let_go();
        hosting::physical::attached(&world, environment).await;
        let created = world
            .engine()
            .create_thread(&Caller::session(world.ada), request())
            .await
            .unwrap();
        assert_eq!(created.arrangement.kind, ArrangementType::Separated);
        assert_eq!(created.arrangement.executor.as_deref(), Some("orch-exec"));
        let thread = created.thread;
        // The place explicitly permits Full access; a Workspace request must never be widened.
        world.set(thread, keys::SANDBOX_MODE, json!("open")).await;
        let mut conn = world.pool.acquire().await.unwrap();
        let binding = orch_db::queries::threads::thread(&mut conn, thread)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(binding.arrangement.as_deref(), Some("separated"));
        assert_eq!(binding.executor.as_deref(), Some("orch-exec"));
        let segment = orch_db::queries::threads::segment(&mut conn, created.segment)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(segment.environment_id, environment);
        assert_eq!(segment.arrangement, "separated");
        assert_eq!(segment.executor.as_deref(), Some("orch-exec"));
        let physical = orch_db::queries::environment_facts::of(&mut conn, environment)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(physical.machine_id, Some(machine.id));
        assert_eq!(physical.confinement(), "none");
        assert!(physical.attaches());
        assert_eq!(
            physical.policy.as_ref().unwrap()["agent.max_sandbox"],
            machine_max
        );
        let raw: serde_json::Value =
            sqlx::query_scalar("SELECT facts FROM environments WHERE id=$1")
                .bind(environment)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(raw["executors"][0]["sandbox_modes"], json!(["open"]));
        let template: Option<Id> =
            sqlx::query_scalar("SELECT template_id FROM environments WHERE id=$1")
                .bind(environment)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert!(template.is_none());
        assert_eq!(
            orch_settings::get::<String>(
                &mut conn,
                orch_settings::Place::Node(thread),
                keys::SANDBOX_MODE
            )
            .await
            .unwrap()
            .as_deref(),
            Some("open")
        );
        assert_eq!(
            orch_engine::sessions::widest_sandbox(&physical, None),
            if machine_max == "open" {
                orch_agent_contract::descriptor::SandboxMode::Open
            } else {
                orch_agent_contract::descriptor::SandboxMode::Workspace
            }
        );
        drop(conn);
        let sent = world
            .engine()
            .submit_input(&Caller::session(world.ada), thread, queued("hello"))
            .await
            .unwrap();
        hosting::physical::describe(
            &world,
            environment,
            thread,
            &format!("template-free ceiling {machine_max}"),
        )
        .await;
        if machine_max == "open" {
            hosting::physical::wait(
                &world,
                environment,
                thread,
                "the template-free directory turn to start",
                || async { !world.turn_starts(thread).is_empty() },
            )
            .await;
            assert_eq!(
                world.turn_starts(thread)[0].sandbox_mode,
                orch_link_contract::commands::SandboxMode::Open
            );
            let opened: Vec<_> = world
                .ordered()
                .into_iter()
                .filter_map(|(machine, command)| match command {
                    orch_link_contract::commands::Command::SessionOpen(body)
                        if body.thread_id == thread =>
                    {
                        Some((machine, body))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(opened.len(), 1);
            assert_eq!(opened[0].0, world.machine_id);
            assert_eq!(opened[0].1.exec.environment_id, environment);
            assert_eq!(opened[0].1.exec.executor.as_deref(), Some("orch-exec"));
            let Some(orch_link_contract::commands::TicketSlot::Asked(ticket)) =
                &opened[0].1.exec.ticket
            else {
                panic!("the executor must receive a ticket");
            };
            assert_eq!(ticket.ticket.more["environment_id"], json!(environment));
            assert_eq!(
                ticket.ticket.more["environment_generation"],
                json!(opened[0].1.exec.generation)
            );
            assert_eq!(
                opened[0].1.sandbox_mode,
                orch_link_contract::commands::SandboxMode::Open
            );
            let start = world.turn_start(thread, 0).await;
            let input: Id = world
                .scalar("SELECT input_id FROM turns WHERE id=$1", start.turn_id)
                .await;
            assert_eq!(input, sent.input.id);
            world.run_turn(thread, 0, "Hello!").await;
            world.wait_state(thread, "idle").await;
        } else {
            hosting::physical::wait(
                &world,
                environment,
                thread,
                "the template-free Workspace ceiling to block",
                || async { world.state(thread).await.0 == "blocked" },
            )
            .await;
            let (_, reason, _) = world.state(thread).await;
            assert_eq!(reason.as_deref(), Some("binding_invalid"));
            // World::state's third member is starting_step, not blocked_detail.
            let detail: Option<String> =
                sqlx::query_scalar("SELECT blocked_detail FROM threads WHERE node_id=$1")
                    .bind(thread)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap();
            let detail = detail.unwrap();
            assert!(detail.contains("cannot confine commands"), "{detail}");
            assert!(detail.contains("Workspace (machine policy)"), "{detail}");
            assert!(
                detail.contains("Allow Full access in this machine's policy"),
                "{detail}"
            );
            assert!(world.turn_starts(thread).is_empty());
            assert_eq!(world.ran(K::SessionOpen), 0);
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
        }
        world.engine().stop();
    }
}

#[tokio::test]
async fn a_colocated_harness_uses_its_own_workspace_sandbox_below_the_machine_ceiling() {
    use orch_agent_contract::descriptor::ArrangementType;
    use orch_engine::binding::NewThread;
    use orch_link_contract::commands::{Command, CommandResult, SandboxMode};
    let mut world = World::new().await;
    assert_eq!(
        sqlx::query(
            "UPDATE providers SET key_machine_id=machine_id WHERE id=$1 AND kind='subscription'"
        )
        .bind(world.provider)
        .execute(world.pool.pool())
        .await
        .unwrap()
        .rows_affected(),
        1
    );
    let machine = primary(&world);
    hosting::physical::report(&world, machine.id, true, Some(true), "workspace").await;
    machine.added.machine.handler.on(K::EnvAttach, |command| {
        let Command::EnvAttach(body) = command else {
            unreachable!()
        };
        let mut facts = hosting::facts();
        facts["executors"][0]["sandbox_modes"] = json!(["open"]);
        facts["workspace_root"] = json!(body.workspace_root);
        facts["generation"] = json!(body.data_generation.unwrap_or(0));
        Ok(CommandResult::EnvAttach(
            serde_json::from_value(facts).unwrap(),
        ))
    });
    let environment = directory(&world, machine.id, "Native harness", "/home/ada/work").await;
    hosting::physical::existing_thread_directory(&world, &machine, environment, "web");
    world.environment = environment;
    hosting::physical::attached(&world, environment).await;
    world
        .set(
            world.topic,
            orch_settings_contract::registry::keys::SANDBOX_MODE,
            json!("open"),
        )
        .await;
    let created = world
        .engine()
        .create_thread(
            &Caller::session(world.ada),
            NewThread {
                topic: world.topic,
                name: "Native confinement".into(),
                agent: world.agent,
                provider: world.provider,
                environment,
                directory: "web".into(),
                arrangement: Some(ArrangementType::Together),
                model: Some(common::MODEL.into()),
                effort: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(created.arrangement.kind, ArrangementType::Together);
    assert!(created.arrangement.executor.is_none());
    let mut conn = world.pool.acquire().await.unwrap();
    let physical = orch_db::queries::environment_facts::of(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        physical.policy.as_ref().unwrap()["agent.max_sandbox"],
        "workspace"
    );
    let raw: serde_json::Value = sqlx::query_scalar("SELECT facts FROM environments WHERE id=$1")
        .bind(environment)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(raw["executors"][0]["sandbox_modes"], json!(["open"]));
    assert_eq!(
        orch_settings::get::<String>(
            &mut conn,
            orch_settings::Place::Node(created.thread),
            orch_settings_contract::registry::keys::SANDBOX_MODE
        )
        .await
        .unwrap()
        .as_deref(),
        Some("open")
    );
    drop(conn);
    let sent = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            created.thread,
            queued("Keep the machine ceiling"),
        )
        .await
        .unwrap();
    hosting::physical::wait(
        &world,
        environment,
        created.thread,
        "native Workspace confinement under an Open request",
        || async { !world.turn_starts(created.thread).is_empty() },
    )
    .await;
    let start = world.turn_start(created.thread, 0).await;
    assert_eq!(start.sandbox_mode, SandboxMode::Workspace);
    let opened: Vec<_> = world
        .ordered()
        .into_iter()
        .filter_map(|(machine, command)| match command {
            Command::SessionOpen(body) if body.thread_id == created.thread => Some((machine, body)),
            _ => None,
        })
        .collect();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].0, machine.id);
    assert!(opened[0].1.exec.executor.is_none());
    assert_eq!(opened[0].1.exec.environment_id, environment);
    assert_eq!(opened[0].1.sandbox_mode, SandboxMode::Workspace);
    let input: Id = world
        .scalar("SELECT input_id FROM turns WHERE id=$1", start.turn_id)
        .await;
    assert_eq!(input, sent.input.id);
    world
        .run_turn(created.thread, 0, "Kept the narrower sandbox")
        .await;
    world.wait_state(created.thread, "idle").await;
    world.engine().stop();
}

#[tokio::test]
async fn a_turn_whose_directory_the_machine_refuses_fails_its_start_and_goes_on_when_a_later_try_is_let_through()
 {
    let mut world = World::new().await;
    let machine = primary(&world);
    hosting::physical::report(&world, machine.id, true, Some(true), "open").await;
    machine.hold(K::EnvAttach);
    // The refusal is followed by a try that the machine holds back, so that the thread stays blocked
    // for as long as the test looks at it.
    machine.refuse_attach_then_hold(CommandError::new(
        ErrorCode::PolicyRootNotAllowed,
        "its owner offers no such directory",
    ));
    let environment = directory(&world, machine.id, "Work", "/home/ada/work").await;
    hosting::physical::existing_thread_directory(&world, &machine, environment, "web");
    world.environment = environment;
    let thread = world.thread("work").await;
    let submitted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("hello"))
        .await
        .unwrap();
    hosting::physical::wait(
        &world,
        environment,
        thread,
        "the thread to wait for its environment",
        || async { world.state(thread).await.2.as_deref() == Some("environment") },
    )
    .await;
    machine.let_go();
    // The machine said no: the start ends, the input waits where it stood, and the thread says why.
    hosting::physical::wait(
        &world,
        environment,
        thread,
        "the attach refusal to block the retained input",
        || async {
            let (state, reason, _) = world.state(thread).await;
            state == "blocked" && reason.as_deref() == Some("environment_unavailable")
        },
    )
    .await;
    let detail: Option<String> =
        sqlx::query_scalar("SELECT blocked_detail FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(detail.unwrap().contains("policy.root_not_allowed"));
    assert_eq!(world.ran(K::TurnStart), 0);
    // The next try of the attach is let through, which frees the thread: the same input is the turn.
    machine.let_go();
    hosting::physical::wait(
        &world,
        environment,
        thread,
        "the turn to be handed to the harness",
        || async { world.ran(K::TurnStart) == 1 },
    )
    .await;
    assert_eq!(status(&world, environment).await.0, "running");
    let (input, _) = world.input_state(submitted.input.id).await;
    assert_eq!(input, "delivering");
}

#[tokio::test]
async fn a_turn_whose_directory_has_a_machine_that_is_away_is_blocked_and_goes_on_when_it_is_back()
{
    let world = World::new().await;
    let (laptop, environment) = laptop_with_directory(&world, &["agent", "environment"]).await;
    let thread = world.thread("work").await;
    sqlx::query("UPDATE threads SET environment_id = $2 WHERE node_id = $1")
        .bind(thread)
        .bind(environment)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("hello"))
        .await
        .unwrap();
    world.wait_blocked(thread, "environment_unavailable").await;
    assert_eq!(laptop.count(K::EnvAttach), 0);
    world.set_machine_online(laptop.id, true).await;
    eventually("the attach", || async { laptop.count(K::EnvAttach) == 1 }).await;
    wait_status(&world, environment, "running").await;
}
