//! Repository defaults follow directory existence, with the same write refusal on every machine.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::World;
use orch_api_contract::source::{
    RepositorySource, SourceDestination, SourceInfo, SourceKind, SourceState,
};
use orch_authz::Caller;
use orch_db::queries::{settings, threads};
use orch_engine::jobs::source::{self, Action, Plan};
use orch_link_contract::commands::{Command, CommandError, CommandKind, CommandResult, ErrorCode};
use orch_types::Time;
use serde_json::json;

struct Configured {
    thread: orch_types::Id,
    native: hosting::Unit,
    physical: orch_types::Id,
    runtime: Option<orch_types::Id>,
    _host: Option<hosting::ScriptedHost>,
}

async fn configured(world: &mut World, placement: &str) -> Configured {
    let (native, physical, runtime, host) = if placement == "connected" {
        let native = hosting::primary(world);
        hosting::physical::report(world, native.id, true, Some(true), "open").await;
        // A bare-directory executor explicitly reports Open only; this fixture asks for Open.
        native
            .added
            .machine
            .handler
            .on(CommandKind::EnvAttach, |command| {
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
        let directory = hosting::directory(
            world,
            native.id,
            "Source directory",
            "/home/ada/source-fixture",
        )
        .await;
        world.environment = directory;
        world.engine().kick();
        hosting::physical::attached(world, directory).await;
        let physical = native.id;
        (native, physical, None, None)
    } else {
        assert_eq!(placement, "managed");
        hosting::serve(world);
        let host = hosting::ScriptedHost::on(world, "source-host", 16_384).await;
        hosting::physical::report(world, host.machine, false, Some(true), "open").await;
        // The scripted Docker driver makes an isolated unit. The host fact, not the unit label,
        // supplies confinement, lifecycle and the environment-policy ceiling.
        let mut conn = world.pool.acquire().await.unwrap();
        let mut driver: serde_json::Value =
            sqlx::query_scalar("SELECT drivers->'docker' FROM hosts WHERE id=$1")
                .bind(host.id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        driver["version"] = json!("0.1.0");
        driver["confinement"] = json!("isolated");
        driver["process_scope"] = json!("all");
        driver["network_egress"] = json!(["open"]);
        driver["recreate"] = json!("replace");
        let driver: orch_driver_contract::descriptor::DriverDescriptor =
            serde_json::from_value(driver).unwrap();
        sqlx::query("UPDATE hosts SET drivers=jsonb_set(drivers,'{docker}',$2) WHERE id=$1")
            .bind(host.id)
            .bind(json!(driver))
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        let template = hosting::template(world).await;
        let made =
            hosting::make_with(world, template, "Source unit", Some(host.id), &json!({})).await;
        host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
        let runtime = host.started().unwrap();
        let native = hosting::enroll(world, runtime).await;
        let (state, _, error) = hosting::wait_over(world, made.job.id).await;
        assert_eq!(state, "succeeded", "{error:?}");
        world.environment = made.id;
        let physical = host.machine;
        (native, physical, Some(runtime), Some(host))
    };
    world
        .set(
            world.topic,
            orch_settings_contract::registry::keys::SANDBOX_MODE,
            json!("open"),
        )
        .await;
    let thread = world.thread("The topic's repository").await;
    let mut conn = world.pool.acquire().await.unwrap();
    settings::put(
        &mut conn,
        Some(world.topic),
        "source.repository_url",
        &settings::Stored {
            value: &json!("https://example.test/team/repo.git"),
            locked: false,
            by: Some(world.ada),
            at: world.clock.now(),
        },
        None,
    )
    .await
    .unwrap();
    let facts = orch_db::queries::environment_facts::of(&mut conn, world.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(facts.machine_id, Some(physical));
    assert_eq!(
        facts.descriptor.as_ref().unwrap()["lifecycle"],
        if runtime.is_some() { "unit" } else { "attach" }
    );
    assert_eq!(
        facts.confinement(),
        if runtime.is_some() {
            "isolated"
        } else {
            "none"
        }
    );
    assert_eq!(
        facts.policy.as_ref().unwrap()["environment.remote_write"],
        true
    );
    let (machine, generation, attached): (
        Option<orch_types::Id>,
        i32,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT machine_id,generation,attached_session FROM environments WHERE id=$1",
    )
    .bind(world.environment)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(machine, Some(native.id));
    assert_eq!(generation, if runtime.is_some() { 1 } else { 0 });
    if let Some(runtime) = runtime {
        let row = orch_db::queries::environments::hosting::runtime(&mut conn, runtime)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.host_id, host.as_ref().unwrap().id);
        assert_eq!(row.machine_id, Some(native.id));
        assert_eq!(row.generation, generation);
        assert_eq!(row.state, "running");
        assert_eq!(row.driver, "docker");
        assert_ne!(
            physical, native.id,
            "a unit connector does not supply its physical host's policy"
        );
    } else {
        assert!(attached.is_some());
    }
    drop(conn);
    assert_eq!(hosting::status(world, world.environment).await.0, "running");
    Configured {
        thread,
        native,
        physical,
        runtime,
        _host: host,
    }
}

#[tokio::test]
async fn a_topics_repository_is_the_default_on_every_machine_and_an_explicit_choice_overrides_it() {
    for placement in ["connected", "managed"] {
        let mut world = World::new().await;
        let fixture = configured(&mut world, placement).await;
        let thread = fixture.thread;
        for choice in [None, Some(false), Some(true)] {
            let mut tx = world.pool.begin().await.unwrap();
            let mut unit = orch_engine::unit::Unit::new(&mut tx, world.clock.now());
            source::freeze(
                &mut unit,
                &Caller::session(world.ada),
                thread,
                world.environment,
                "web",
                choice,
                None,
            )
            .await
            .unwrap();
            unit.finish().await.unwrap();
            tx.commit().await.unwrap();
            let shown = world.sync(thread).await.thread.source.unwrap();
            if choice == Some(false) {
                assert_eq!(shown.kind, SourceKind::Directory);
                assert_eq!(shown.state, SourceState::Ready);
                assert_eq!(shown.reason.as_deref(), Some("source_skipped"));
            } else {
                assert_eq!(shown.kind, SourceKind::Repository);
                assert_eq!(shown.state, SourceState::Pending);
                assert_eq!(
                    shown.repository_url.as_deref(),
                    Some("https://example.test/team/repo.git")
                );
            }
            assert_eq!(shown.directory, "web");
        }
        world.engine().stop();
    }
}

#[tokio::test]
async fn an_existing_directory_is_kept_even_without_write_permission_and_a_missing_one_obeys_both_write_gates()
 {
    for placement in ["connected", "managed"] {
        // Directory existence, the environment's switch and the machine's live refusal are independent.
        for (exists, writable, machine_allows) in [
            (true, false, false),
            (false, false, true),
            (false, true, false),
            (false, true, true),
        ] {
            let mut world = World::new().await;
            let fixture = configured(&mut world, placement).await;
            let thread = fixture.thread;
            sqlx::query("UPDATE environments SET remote_write=$2 WHERE id=$1")
                .bind(world.environment)
                .bind(writable)
                .execute(world.pool.pool())
                .await
                .unwrap();
            let now = Time::new(world.clock.now());
            fixture
                .native
                .added
                .machine
                .handler
                .on(CommandKind::FsStat, move |command| {
                    let Command::FsStat(body) = command else {
                        unreachable!()
                    };
                    if !exists {
                        return Err(CommandError::new(
                            ErrorCode::NotFound,
                            "the directory is missing",
                        ));
                    }
                    Ok(CommandResult::FsStat(
                        orch_api_contract::workspace::FileStat {
                            entry: orch_api_contract::workspace::DirEntry {
                                name: "web".into(),
                                path: body.path.clone(),
                                kind: orch_api_contract::workspace::DirEntryKind::Dir,
                                size: 0,
                                mtime: now,
                                mode: 0o755,
                                link_target: None,
                                cached: false,
                            },
                            hash: None,
                            media_type: "inode/directory".into(),
                            is_text: false,
                        },
                    ))
                });
            fixture
                .native
                .added
                .machine
                .handler
                .on(CommandKind::GitPrepare, move |command| {
                    if !machine_allows {
                        return Err(CommandError::new(
                            ErrorCode::PolicyRemoteWriteDisabled,
                            "environment.remote_write: this machine refuses remote writes",
                        ));
                    }
                    let Command::GitPrepare(body) = command else {
                        unreachable!()
                    };
                    assert_eq!(body.repository_url, "https://example.test/team/repo.git");
                    Ok(CommandResult::GitPrepare(SourceInfo {
                        kind: SourceKind::Repository,
                        directory: body.directory.clone(),
                        repository_url: Some(body.repository_url.clone()),
                        branch: Some("main".into()),
                        head: Some("checked-head".into()),
                        state: SourceState::Ready,
                        job: None,
                        reason: None,
                    }))
                });
            fixture
                .native
                .added
                .machine
                .handler
                .on(CommandKind::GitPrepareCleanup, |_| {
                    Ok(CommandResult::GitPrepareCleanup(
                        orch_link_contract::source::GitPrepareCleanupResult {
                            removed: true,
                            reason: None,
                        },
                    ))
                });
            let mut tx = world.pool.begin().await.unwrap();
            let row = threads::thread(&mut tx, thread).await.unwrap().unwrap();
            let environment =
                orch_db::queries::environments::hosting::environment(&mut tx, world.environment)
                    .await
                    .unwrap()
                    .unwrap();
            let frozen = RepositorySource {
                repository_url: Some("https://example.test/team/repo.git".into()),
                branch: None,
                secret_name: None,
            };
            let job = source::begin(
                &mut tx,
                Plan {
                    environment: world.environment,
                    generation: environment.generation,
                    machine: environment.machine_id.unwrap(),
                    thread: Some(thread),
                    caller: world.ada,
                    token: None,
                    action: Action::Prepare {
                        directory: row.workspace_dir,
                        source: frozen,
                        credential: None,
                        destination: SourceDestination::MissingDirectory,
                        effect_id: None,
                    },
                },
                world.clock.now(),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            world.engine().kick();
            let (state, _, error) = hosting::wait_over(&world, job.id).await;
            let shown = world.sync(thread).await.thread.source.unwrap();
            if exists {
                assert_eq!(state, "succeeded", "{error:?}");
                assert_eq!(shown.kind, SourceKind::Directory);
                assert_eq!(
                    fixture.native.began(CommandKind::GitPrepare),
                    0,
                    "an existing checkout is never fetched or replaced"
                );
            } else if writable && machine_allows {
                assert_eq!(state, "succeeded", "{error:?}");
                assert_eq!(shown.kind, SourceKind::Repository);
                assert_eq!(shown.head.as_deref(), Some("checked-head"));
                assert_eq!(fixture.native.began(CommandKind::GitPrepare), 1);
            } else {
                assert_eq!(state, "failed");
                assert_eq!(shown.state, SourceState::Failed);
                assert_eq!(
                    shown.reason.as_deref(),
                    Some("workspace.remote_write_disabled")
                );
                assert_eq!(
                    fixture.native.began(CommandKind::GitPrepare),
                    usize::from(writable)
                );
            }
            assert_eq!(
                fixture.native.began(CommandKind::FsMkdir),
                0,
                "a missing repository is never replaced with an empty directory"
            );
            world.engine().stop();
        }
    }
}

#[tokio::test]
async fn automatic_preparation_keeps_an_existing_directory_without_requiring_an_unused_source_credential()
 {
    let mut world = World::new().await;
    let fixture = configured(&mut world, "connected").await;
    let thread = fixture.thread;
    hosting::physical::report(&world, fixture.physical, true, Some(false), "open").await;
    let mut tx = world.pool.begin().await.unwrap();
    settings::put(
        &mut tx,
        Some(world.topic),
        "source.secret_name",
        &settings::Stored {
            value: &json!("source-auth"),
            locked: false,
            by: Some(world.ada),
            at: world.clock.now(),
        },
        None,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET remote_write=false WHERE id=$1")
        .bind(world.environment)
        .execute(&mut *tx)
        .await
        .unwrap();
    let mut unit = orch_engine::unit::Unit::new(&mut tx, world.clock.now());
    source::freeze(
        &mut unit,
        &Caller::session(world.ada),
        thread,
        world.environment,
        "web",
        None,
        None,
    )
    .await
    .unwrap();
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    let now = Time::new(world.clock.now());
    fixture
        .native
        .added
        .machine
        .handler
        .on(CommandKind::FsStat, move |_| {
            Ok(CommandResult::FsStat(
                orch_api_contract::workspace::FileStat {
                    entry: orch_api_contract::workspace::DirEntry {
                        name: "web".into(),
                        path: "web".into(),
                        kind: orch_api_contract::workspace::DirEntryKind::Dir,
                        size: 0,
                        mtime: now,
                        mode: 0o755,
                        link_target: None,
                        cached: false,
                    },
                    hash: None,
                    media_type: "inode/directory".into(),
                    is_text: false,
                },
            ))
        });
    fixture
        .native
        .added
        .machine
        .handler
        .on(CommandKind::FsRecord, |_| {
            Ok(CommandResult::FsRecord(
                orch_link_contract::source::FsRecordResult {
                    state: orch_link_contract::source::RecordState::Unavailable,
                    tree_ref: None,
                    repository_head: None,
                    manifest_blob: None,
                    files: 0,
                    bytes: 0,
                    expires_at: None,
                    reason: Some("not_ready".into()),
                },
            ))
        });
    fixture.native.hold(CommandKind::FsRecord);
    fixture
        .native
        .added
        .machine
        .handler
        .on(CommandKind::FsMkdir, |_| {
            Err(CommandError::new(
                ErrorCode::PolicyRemoteWriteDisabled,
                "the readonly directory refuses native writes",
            ))
        });
    let sent = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            common::queued("Use the existing checkout"),
        )
        .await
        .unwrap();
    // Recording is the natural boundary after source readiness and before turn-directory startup.
    hosting::physical::wait(
        &world,
        world.environment,
        thread,
        "the existing source to reach recording without a write",
        || async { fixture.native.began(CommandKind::FsRecord) == 1 },
    )
    .await;
    let shown = world.sync(thread).await.thread.source.unwrap();
    assert_eq!(shown.kind, SourceKind::Directory);
    assert_eq!(shown.state, SourceState::Ready);
    assert_eq!(fixture.native.began(CommandKind::GitPrepare), 0);
    assert_eq!(fixture.native.began(CommandKind::FsMkdir), 0);
    assert_eq!(world.began(CommandKind::SessionOpen), 0);
    assert_eq!(world.began(CommandKind::TurnStart), 0);
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
    assert_eq!(world.input_state(sent.input.id).await.0, "pending");
    fixture.native.let_go();
    hosting::physical::wait(
        &world,
        world.environment,
        thread,
        "the existing-directory source input to start",
        || async { !world.turn_starts(thread).is_empty() },
    )
    .await;
    let start = world.turn_start(thread, 0).await;
    assert_eq!(
        common::words_after_where(&start.text),
        "Use the existing checkout"
    );
    let input: orch_types::Id = world
        .scalar("SELECT input_id FROM turns WHERE id=$1", start.turn_id)
        .await;
    assert_eq!(input, sent.input.id);
    assert_eq!(fixture.native.began(CommandKind::GitPrepare), 0);
    world.engine().stop();
}

#[tokio::test]
async fn a_reported_physical_write_denial_or_missing_policy_stops_before_native_preparation_on_both_kinds()
 {
    for placement in ["connected", "managed"] {
        for reported in [Some(false), None] {
            let mut world = World::new().await;
            let fixture = configured(&mut world, placement).await;
            hosting::physical::report(
                &world,
                fixture.physical,
                fixture.runtime.is_none(),
                reported,
                "open",
            )
            .await;
            sqlx::query("UPDATE environments SET remote_write=true WHERE id=$1")
                .bind(world.environment)
                .execute(world.pool.pool())
                .await
                .unwrap();
            fixture
                .native
                .added
                .machine
                .handler
                .on(CommandKind::FsStat, |_| {
                    Err(CommandError::new(
                        ErrorCode::NotFound,
                        "no source directory",
                    ))
                });
            fixture
                .native
                .added
                .machine
                .handler
                .on(CommandKind::GitPrepare, |_| {
                    panic!("the controller must refuse before a native prepare")
                });
            fixture
                .native
                .added
                .machine
                .handler
                .on(CommandKind::GitPrepareCleanup, |_| {
                    Ok(CommandResult::GitPrepareCleanup(
                        orch_link_contract::source::GitPrepareCleanupResult {
                            removed: false,
                            reason: Some("no preparation was admitted".into()),
                        },
                    ))
                });
            let mut tx = world.pool.begin().await.unwrap();
            let facts = orch_db::queries::environment_facts::of(&mut tx, world.environment)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(facts.machine_id, Some(fixture.physical));
            assert_eq!(
                facts
                    .policy
                    .as_ref()
                    .and_then(|policy| policy.get("environment.remote_write"))
                    .and_then(serde_json::Value::as_bool),
                reported
            );
            let principal = orch_authz::Principal::load(
                &mut tx,
                Caller::session(world.ada),
                Time::new(world.clock.now()),
            )
            .await
            .unwrap();
            let manages = orch_authz::resources::check(
                &mut tx,
                &principal,
                orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Environment,
                    id: world.environment,
                },
                orch_access_contract::permissions::Permission::EnvironmentManage,
                orch_authz::Policy::default(),
            )
            .await
            .unwrap();
            assert!(
                matches!(
                    manages,
                    orch_access_contract::decisions::Decision::Allowed { .. }
                ),
                "the real registered owner remains authorized independently of physical write policy"
            );
            let env =
                orch_db::queries::environments::hosting::environment(&mut tx, world.environment)
                    .await
                    .unwrap()
                    .unwrap();
            let row = threads::thread(&mut tx, fixture.thread)
                .await
                .unwrap()
                .unwrap();
            let job = source::begin(
                &mut tx,
                Plan {
                    environment: world.environment,
                    generation: env.generation,
                    machine: fixture.native.id,
                    thread: Some(fixture.thread),
                    caller: world.ada,
                    token: None,
                    action: Action::Prepare {
                        directory: row.workspace_dir,
                        source: RepositorySource {
                            repository_url: Some("https://example.test/team/repo.git".into()),
                            branch: None,
                            secret_name: None,
                        },
                        credential: None,
                        destination: SourceDestination::MissingDirectory,
                        effect_id: None,
                    },
                },
                world.clock.now(),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            world.engine().kick();
            let (state, _, error) = hosting::wait_over(&world, job.id).await;
            assert_eq!(state, "failed", "{error:?}");
            assert_eq!(error.unwrap()["code"], "policy.remote_write_disabled");
            let shown = world.sync(fixture.thread).await.thread.source.unwrap();
            assert_eq!(shown.state, SourceState::Failed);
            assert_eq!(
                shown.reason.as_deref(),
                Some("workspace.remote_write_disabled")
            );
            assert_eq!(fixture.native.began(CommandKind::GitPrepare), 0);
            assert_eq!(fixture.native.began(CommandKind::FsMkdir), 0);
            assert_eq!(fixture.native.began(CommandKind::FsRecord), 0);
            assert_eq!(world.began(CommandKind::TurnStart), 0);
            assert_eq!(
                world
                    .count(
                        "SELECT count(*) FROM turns WHERE thread_id=$1",
                        fixture.thread
                    )
                    .await,
                0
            );
            assert_eq!(
                world
                    .count(
                        "SELECT count(*) FROM leases WHERE thread_id=$1",
                        fixture.thread
                    )
                    .await,
                0
            );
            world.engine().stop();
        }
    }
}

#[tokio::test]
async fn a_terminal_source_failure_blocks_without_retrying_and_explicit_retry_keeps_the_input() {
    use orch_link_contract::source::{FsRecordResult, RecordState};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    for refusal in [ErrorCode::PolicyRemoteWriteDisabled, ErrorCode::Internal] {
        let mut world = World::new().await;
        let fixture = configured(&mut world, "connected").await;
        let thread = fixture.thread;
        // This case reaches a native refusal after both controller write gates authorize it.
        sqlx::query("UPDATE environments SET remote_write=true WHERE id=$1")
            .bind(world.environment)
            .execute(world.pool.pool())
            .await
            .unwrap();
        fixture
            .native
            .added
            .machine
            .handler
            .on(CommandKind::FsStat, |_| {
                Err(CommandError::new(
                    ErrorCode::NotFound,
                    "the source directory does not exist",
                ))
            });
        let retry_allowed = Arc::new(AtomicBool::new(false));
        let permitted = retry_allowed.clone();
        let is_policy = refusal == ErrorCode::PolicyRemoteWriteDisabled;
        fixture
            .native
            .added
            .machine
            .handler
            .on(CommandKind::GitPrepare, move |command| {
                let Command::GitPrepare(body) = command else {
                    unreachable!()
                };
                assert_eq!(body.repository_url, "https://example.test/team/repo.git");
                if !permitted.load(Ordering::SeqCst) {
                    return Err(CommandError::new(
                        refusal,
                        "the native source preparation was refused",
                    ));
                }
                Ok(CommandResult::GitPrepare(SourceInfo {
                    kind: SourceKind::Repository,
                    directory: body.directory.clone(),
                    repository_url: Some(body.repository_url.clone()),
                    branch: Some("main".into()),
                    head: Some("retried-head".into()),
                    state: SourceState::Ready,
                    job: None,
                    reason: None,
                }))
            });
        fixture
            .native
            .added
            .machine
            .handler
            .on(CommandKind::GitPrepareCleanup, |_| {
                Ok(CommandResult::GitPrepareCleanup(
                    orch_link_contract::source::GitPrepareCleanupResult {
                        removed: true,
                        reason: None,
                    },
                ))
            });
        fixture
            .native
            .added
            .machine
            .handler
            .on(CommandKind::FsRecord, |_| {
                Ok(CommandResult::FsRecord(FsRecordResult {
                    state: RecordState::Unavailable,
                    tree_ref: None,
                    repository_head: None,
                    manifest_blob: None,
                    files: 0,
                    bytes: 0,
                    expires_at: None,
                    reason: Some("not_ready".into()),
                }))
            });
        let mut tx = world.pool.begin().await.unwrap();
        let mut unit = orch_engine::unit::Unit::new(&mut tx, world.clock.now());
        source::freeze(
            &mut unit,
            &Caller::session(world.ada),
            thread,
            world.environment,
            "web",
            None,
            None,
        )
        .await
        .unwrap();
        unit.finish().await.unwrap();
        tx.commit().await.unwrap();
        let sent = world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                common::queued("Keep this original input"),
            )
            .await
            .unwrap();
        world.wait_blocked(thread, "error").await;
        let shown = world.sync(thread).await.thread.source.unwrap();
        assert_eq!(shown.state, SourceState::Failed);
        assert_eq!(
            shown.reason.as_deref(),
            Some(if is_policy {
                "workspace.remote_write_disabled"
            } else {
                "workspace.source_failed"
            })
        );
        let job_id = shown.job.unwrap().id;
        assert_eq!(hosting::wait_over(&world, job_id).await.0, "failed");
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
        assert_eq!(world.input_state(sent.input.id).await.0, "pending");
        assert_eq!(fixture.native.began(CommandKind::GitPrepare), 1);
        assert_eq!(fixture.native.began(CommandKind::FsRecord), 0);
        assert_eq!(world.began(CommandKind::TurnStart), 0);
        for _ in 0..2 {
            assert!(matches!(
                world.engine().preflight(thread).await.unwrap(),
                orch_engine::preflight::Outcome::Blocked(_)
            ));
        }
        assert_eq!(
            fixture.native.began(CommandKind::GitPrepare),
            1,
            "ordinary preflight never retries the failed source"
        );
        let frozen: serde_json::Value = world
            .scalar("SELECT source FROM threads WHERE node_id=$1", thread)
            .await;
        let plan: Plan = serde_json::from_value(frozen["native_plan"].clone()).unwrap();
        retry_allowed.store(true, Ordering::SeqCst);
        let mut tx = world.pool.begin().await.unwrap();
        let retry = source::begin(&mut tx, plan, world.clock.now())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        world.engine().kick();
        assert_eq!(hosting::wait_over(&world, retry.id).await.0, "succeeded");
        hosting::physical::wait(
            &world,
            world.environment,
            thread,
            "the explicit source retry to resume its original input",
            || async { !world.turn_starts(thread).is_empty() },
        )
        .await;
        let start = world.turn_start(thread, 0).await;
        assert_eq!(
            common::words_after_where(&start.text),
            "Keep this original input"
        );
        let input: orch_types::Id = world
            .scalar("SELECT input_id FROM turns WHERE id=$1", start.turn_id)
            .await;
        assert_eq!(input, sent.input.id);
        assert_eq!(
            world
                .count(
                    "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                    thread
                )
                .await,
            1
        );
        assert_eq!(fixture.native.began(CommandKind::GitPrepare), 2);
        assert_eq!(
            world.sync(thread).await.thread.source.unwrap().state,
            SourceState::Ready
        );
        world.engine().stop();
    }
}
