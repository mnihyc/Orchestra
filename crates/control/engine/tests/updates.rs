//! Runtime updates keep volumes and ownership, hold all threads and restore the old unit on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::World;
use hosting::{ScriptedHost, bind, running, serve, template, wait_over};
use orch_api_contract::environment::UpdatePreflightReason;
use orch_authz::{Caller, Principal};
use orch_db::queries::{environments::hosting as envs, jobs};
use orch_engine::updates::{Target, UpdateError};
use orch_link_contract::commands::{CommandError, CommandKind, ErrorCode};
use orch_types::{Id, Time};
use serde_json::json;

async fn draft(
    world: &World,
    environment: Id,
    reason: UpdatePreflightReason,
) -> orch_engine::updates::Draft {
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = Principal::load(
        &mut conn,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    world
        .engine()
        .draft_update(
            &mut conn,
            &principal,
            Target::Environment(environment),
            reason,
            None,
        )
        .await
        .unwrap()
}

/// The template's image changes after the unit was made from it: the unit is out of date, and an update
/// has something to apply (a unit made from the template as it is now is current).
async fn newer_image(world: &World, template: Id) {
    sqlx::query(
        "UPDATE env_templates SET spec = jsonb_set(spec,'{image}','\"busybox:newer\"') WHERE id = $1",
    )
    .bind(template)
    .execute(world.pool.pool())
    .await
    .unwrap();
}

async fn queue(world: &World, draft: orch_engine::updates::Draft) -> jobs::JobRow {
    let mut tx = world.pool.begin().await.unwrap();
    let row = orch_engine::jobs::recreate::queue(&mut tx, draft, None, world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    row
}

#[tokio::test]
async fn a_verified_template_change_names_both_choices_once_without_claiming_an_ordinary_recreate()
{
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "template-trace", 16_384).await;
    let first = template(&world).await;
    // The unit's connector stays online: both replacements are attached and verified through it.
    let (environment, _, _unit) = running(&world, &host, first, "Template history").await;
    let next = template(&world).await;
    sqlx::query("UPDATE environments SET config=jsonb_set(config,'{env.template_id}',$2::jsonb) WHERE id=$1")
        .bind(environment).bind(json!(next)).execute(world.pool.pool()).await.unwrap();
    let job = queue(
        &world,
        draft(&world, environment, UpdatePreflightReason::Update).await,
    )
    .await;
    assert_eq!(wait_over(&world, job.id).await.0, "succeeded");
    world.engine().kick();
    let mut conn = world.pool.acquire().await.unwrap();
    let events:Vec<(Option<Id>,serde_json::Value)> = sqlx::query_as("SELECT actor_id,details FROM audit_log WHERE action='environment.template_changed' AND object_id=$1")
        .bind(environment).fetch_all(&mut *conn).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, Some(world.ada));
    assert_eq!(events[0].1["from_template"], json!(first));
    assert_eq!(events[0].1["to_template"], json!(next));
    assert_eq!(
        envs::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .template_id,
        next
    );
    drop(conn);
    let again = queue(
        &world,
        draft(&world, environment, UpdatePreflightReason::Recreate).await,
    )
    .await;
    assert_eq!(wait_over(&world, again.id).await.0, "succeeded");
    let count:i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE action='environment.template_changed' AND object_id=$1")
        .bind(environment).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(count, 1, "recreating the same template is its own act");
}

#[tokio::test]
async fn a_failed_restriction_preserves_files_and_blocks_generic_start_after_the_job_releases() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "network-guard", 16_384).await;
    let template = template(&world).await;
    let (environment, old, _unit) = running(&world, &host, template, "Main").await;
    let before = envs::runtime(&mut world.pool.acquire().await.unwrap(), old)
        .await
        .unwrap()
        .unwrap();
    let thread = world.thread("Work").await;
    bind(&world, thread, environment).await;
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE env_templates SET spec = jsonb_set(spec, '{network}', '{\"mode\":\"bridge\",\"egress\":\"restricted\"}'::jsonb) WHERE id = $1")
        .bind(template).execute(&mut *conn).await.unwrap();
    drop(conn);
    host.refuse(
        CommandKind::NodeRuntimeCreate,
        CommandError::new(ErrorCode::Unsupported, "restriction not supported here"),
    );
    let row = queue(
        &world,
        draft(&world, environment, UpdatePreflightReason::Update).await,
    )
    .await;
    let (state, _, _) = wait_over(&world, row.id).await;
    assert_eq!(state, "failed");
    let mut conn = world.pool.acquire().await.unwrap();
    let current = envs::live_runtime(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.id, old);
    assert_eq!(current.state, "stopped");
    assert_eq!(current.volumes, before.volumes);
    assert!(
        jobs::changes::thread_hold(&mut conn, thread)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        orch_engine::network::blocked_thread(&mut conn, thread)
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        orch_engine::environments::start(
            &mut conn,
            world.clock.now(),
            environment,
            Some(world.ada)
        )
        .await,
        Err(orch_engine::environments::StartError::Unavailable(_))
    ));
    assert!(
        !host
            .calls()
            .iter()
            .any(|call| call.kind == CommandKind::NodeRuntimeStart
                && call.body["runtime_id"] == json!(old)),
        "rollback must not send a Start for the old open unit"
    );
    world.engine().stop();
}

#[tokio::test]
async fn disabling_a_home_stops_its_managed_unit_once_and_keeps_every_owned_volume() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "home-ending", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Kept files").await;
    let before = envs::runtime(&mut world.pool.acquire().await.unwrap(), runtime)
        .await
        .unwrap()
        .unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    let (affected, _) = world
        .engine()
        .end_home_resources_in(
            &mut tx,
            world.root,
            None,
            world.ada,
            Time::new(world.clock.now()),
            &orch_db::audit::Origin::default(),
        )
        .await
        .unwrap();
    assert!(affected.contains(&environment));
    tx.commit().await.unwrap();
    world.engine().kick();
    host.wait_for(CommandKind::NodeRuntimeStop, 1).await;
    common::eventually("the host confirms the disabled unit's stop", || async {
        envs::runtime(&mut world.pool.acquire().await.unwrap(), runtime)
            .await
            .unwrap()
            .unwrap()
            .state
            == "stopped"
    })
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let after = envs::runtime(&mut conn, runtime).await.unwrap().unwrap();
    assert_eq!(after.volumes, before.volumes);
    assert_eq!(
        envs::environment(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .status,
        "disabled"
    );
    assert_eq!(host.count(CommandKind::NodeRuntimeRemove), 0);
    assert_eq!(host.count(CommandKind::NodeRuntimeStart), 0);
    assert_eq!(host.count(CommandKind::NodeRuntimeStop), 1);
    world.engine().stop();
}

#[tokio::test]
async fn a_freshly_provisioned_unit_is_current_and_does_not_need_an_immediate_update() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    let made = hosting::make(&world, template, "Main").await;
    host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let waiting = envs::live_runtime(&mut world.pool.acquire().await.unwrap(), made.id)
        .await
        .unwrap()
        .unwrap();
    let making = envs::environment(&mut world.pool.acquire().await.unwrap(), made.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(making.status, "provisioning");
    let observed = world
        .engine()
        .pending_runtime_updates(&mut world.pool.acquire().await.unwrap(), &[waiting.id])
        .await
        .unwrap();
    assert!(
        observed[&waiting.id].is_empty(),
        "verification of a new unit is still in progress, not behind"
    );
    let _unit = hosting::enroll(&world, host.started().unwrap()).await;
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert!(
        draft(&world, made.id, UpdatePreflightReason::Update)
            .await
            .updates
            .is_empty()
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let live = envs::live_runtime(&mut conn, made.id)
        .await
        .unwrap()
        .unwrap();
    let recorded = orch_db::queries::jobs::changes::definition(&mut conn, live.id)
        .await
        .unwrap()
        .unwrap();
    let builds: Vec<orch_engine::hosting::ToolBuild> = serde_json::from_value(
        orch_db::queries::jobs::changes::tools_of(&mut conn, live.id)
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    // Units made by an earlier build used catalogue order rather than semantic tool identity.
    let legacy = orch_types::Hash::of(
        json!([
            live.driver,
            recorded["spec"],
            recorded["resources"],
            builds
                .iter()
                .rev()
                .map(|build| (&build.name, &build.version))
                .collect::<Vec<_>>()
        ])
        .to_string()
        .as_bytes(),
    );
    sqlx::query("UPDATE runtimes SET spec_hash=$2 WHERE id=$1")
        .bind(live.id)
        .bind(legacy.as_bytes().as_slice())
        .execute(&mut *conn)
        .await
        .unwrap();
    let current = world
        .engine()
        .pending_runtime_updates(&mut conn, &[live.id])
        .await
        .unwrap();
    assert!(
        current[&live.id].is_empty(),
        "a fresh unit is current in the page observation as well as preflight"
    );
    drop(conn);
    assert!(
        draft(&world, made.id, UpdatePreflightReason::Update)
            .await
            .updates
            .is_empty()
    );
    world.engine().stop();
}

#[test]
fn a_catalogue_reorder_is_not_a_runtime_update_but_a_build_change_is() {
    use orch_engine::hosting::ToolBuild;
    use orch_engine::updates::definition_hash;
    let build = |name: &str| ToolBuild {
        name: name.into(),
        version: "1.0.0".into(),
        url: "unused".into(),
        sha256: "0".repeat(64),
    };
    let tools = vec![build("orch-connector"), build("orch-exec"), build("fake")];
    let mut reordered = tools.clone();
    reordered.reverse();
    let spec = json!({"image":"example@sha256:fixed"});
    let resources = json!({"memory_mb":1024});
    let before = definition_hash("docker", &spec, &resources, &tools);
    assert_eq!(
        before,
        definition_hash("docker", &spec, &resources, &reordered)
    );
    reordered[0].version = "1.1.0".into();
    assert_ne!(
        before,
        definition_hash("docker", &spec, &resources, &reordered)
    );
    assert_ne!(
        before,
        definition_hash("docker", &spec, &json!({"memory_mb":2048}), &tools)
    );
    let first =
        json!({"image":"example", "env":{"B":"b", "A":"a"}, "command":["run", "one", "two"]});
    let reordered_keys =
        json!({"command":["run", "one", "two"], "env":{"A":"a", "B":"b"}, "image":"example"});
    assert_eq!(
        definition_hash(
            "docker",
            &first,
            &json!({"cpu_millis":1000,"memory_mb":1024}),
            &tools
        ),
        definition_hash(
            "docker",
            &reordered_keys,
            &json!({"memory_mb":1024,"cpu_millis":1000}),
            &tools
        )
    );
    let reordered_argv =
        json!({"image":"example", "env":{"B":"b", "A":"a"}, "command":["run", "two", "one"]});
    assert_ne!(
        definition_hash("docker", &first, &resources, &tools),
        definition_hash("docker", &reordered_argv, &resources, &tools)
    );
}

struct PinnedBuilds;

impl PinnedBuilds {
    fn listed(
        requests: &[orch_engine::hosting::HarnessRequest],
    ) -> Result<Vec<orch_engine::hosting::ToolBuild>, orch_engine::hosting::HostingError> {
        let mut builds = hosting::listed_tools(
            &requests
                .iter()
                .map(|request| request.kind.clone())
                .collect::<Vec<_>>(),
        );
        for request in requests {
            if let Some(version) = &request.version {
                if !["1.0.0", "1.1.0"].contains(&version.as_str()) {
                    return Err(orch_engine::hosting::HostingError::Failed(format!(
                        "no listed build of {} at {version}",
                        request.kind
                    )));
                }
                if let Some(build) = builds.iter_mut().find(|build| build.name == request.kind) {
                    build.version.clone_from(version);
                    build.url = format!("http://tools.test/{}-{version}", request.kind);
                    build.sha256 = "1".repeat(64);
                }
            }
        }
        Ok(builds)
    }
}

#[async_trait::async_trait]
impl orch_engine::hosting::HostServices for PinnedBuilds {
    fn observed_tools(
        &self,
        _os: &str,
        _arch: &str,
        requests: &[orch_engine::hosting::HarnessRequest],
    ) -> Option<Vec<orch_engine::hosting::ToolBuild>> {
        Self::listed(requests).ok()
    }
    async fn enrollment(
        &self,
        conn: &mut orch_db::Conn,
        ask: &orch_engine::hosting::EnrollmentAsk,
    ) -> Result<orch_engine::hosting::Enrollment, orch_engine::hosting::HostingError> {
        orch_engine::hosting::HostServices::enrollment(&hosting::Services, conn, ask).await
    }

    async fn tools(
        &self,
        conn: &mut orch_db::Conn,
        os: &str,
        arch: &str,
        kinds: &[String],
    ) -> Result<Vec<orch_engine::hosting::ToolBuild>, orch_engine::hosting::HostingError> {
        orch_engine::hosting::HostServices::tools(&hosting::Services, conn, os, arch, kinds).await
    }

    async fn requested_tools(
        &self,
        _conn: &mut orch_db::Conn,
        _os: &str,
        _arch: &str,
        requests: &[orch_engine::hosting::HarnessRequest],
    ) -> Result<Vec<orch_engine::hosting::ToolBuild>, orch_engine::hosting::HostingError> {
        Self::listed(requests)
    }
}

#[tokio::test]
async fn a_profile_pin_is_provisioned_updated_and_unavailable_pins_block_without_commands() {
    let world = World::new().await;
    world
        .engine()
        .attach_host_services(std::sync::Arc::new(PinnedBuilds));
    let host = ScriptedHost::on(&world, "pinned-build", 16_384).await;
    let template = hosting::template_of(&world, &["orch-exec"], &["fake"]).await;
    sqlx::query("UPDATE agents SET profile = '{\"harness_version\":\"1.1.0\"}' WHERE id = $1")
        .bind(world.agent)
        .execute(world.pool.pool())
        .await
        .unwrap();
    // This default is scoped to org/project/topic, not an environment's own config.
    world
        .set(world.root, "agent.default_agent_id", json!(world.agent))
        .await;
    let made = hosting::make(&world, template, "Pinned").await;
    host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let _unit = hosting::enroll(&world, host.started().unwrap()).await;
    assert_eq!(wait_over(&world, made.job.id).await.0, "succeeded");
    let calls = host.calls();
    assert!(
        calls
            .iter()
            .any(|call| call.kind == CommandKind::NodeToolEnsure
                && call.body["name"] == "fake"
                && call.body["version"] == "1.1.0"
                && call.body["sha256"] == "1".repeat(64))
    );
    assert!(
        !calls
            .iter()
            .any(|call| call.kind == CommandKind::NodeToolEnsure
                && call.body["name"] == "fake"
                && call.body["version"] == "1.0.0")
    );
    assert!(
        draft(&world, made.id, UpdatePreflightReason::Update)
            .await
            .updates
            .is_empty()
    );

    sqlx::query("UPDATE agents SET profile = '{\"harness_version\":\"1.0.0\"}' WHERE id = $1")
        .bind(world.agent)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let changed = draft(&world, made.id, UpdatePreflightReason::Update).await;
    assert!(changed.updates.iter().any(|u| u.name == "fake"
        && u.from.as_deref() == Some("1.1.0")
        && u.to.as_deref() == Some("1.0.0")));
    let live = envs::live_runtime(&mut world.pool.acquire().await.unwrap(), made.id)
        .await
        .unwrap()
        .unwrap();
    let replaced = live.id;
    let pending = world
        .engine()
        .pending_runtime_updates(&mut world.pool.acquire().await.unwrap(), &[live.id])
        .await
        .unwrap();
    assert_eq!(
        pending[&live.id], changed.updates,
        "the runtime count and replacement preflight use the same changes"
    );
    let job = queue(&world, changed).await;
    assert_eq!(wait_over(&world, job.id).await.0, "succeeded");
    assert!(
        draft(&world, made.id, UpdatePreflightReason::Update)
            .await
            .updates
            .is_empty()
    );
    let live = envs::live_runtime(&mut world.pool.acquire().await.unwrap(), made.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        world
            .engine()
            .pending_runtime_updates(&mut world.pool.acquire().await.unwrap(), &[live.id])
            .await
            .unwrap()[&live.id]
            .is_empty()
    );

    sqlx::query("UPDATE agents SET profile = '{\"harness_version\":\"unlisted\"}' WHERE id = $1")
        .bind(world.agent)
        .execute(world.pool.pool())
        .await
        .unwrap();
    // The reaper takes the replaced unit away after the update, on its own schedule: what the drafts below
    // send is counted once that is done, apart from it.
    common::eventually("the replaced unit to be taken away", || async {
        envs::runtime(&mut world.pool.acquire().await.unwrap(), replaced)
            .await
            .unwrap()
            .is_some_and(|runtime| runtime.state == "removed")
    })
    .await;
    let before = host.calls().len();
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = Principal::load(
        &mut conn,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    assert!(
        matches!(world.engine().draft_update(&mut conn, &principal, Target::Environment(made.id),
        UpdatePreflightReason::Update, None).await,
        Err(UpdateError::Blocked(blockers)) if blockers.iter().any(|b| b.code == "build_unavailable" && b.message.contains("unlisted")))
    );
    assert_eq!(host.calls().len(), before);
    let same = world
        .engine()
        .draft_update(
            &mut conn,
            &principal,
            Target::Environment(made.id),
            UpdatePreflightReason::Recreate,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        same.recipe
            .tools
            .iter()
            .find(|t| t.name == "fake")
            .unwrap()
            .version,
        "1.0.0",
        "recreate keeps its recorded build even after the profile's pin changed"
    );
    world.engine().stop();
}

#[tokio::test]
async fn an_update_keeps_the_same_volumes_owner_and_environment_and_retires_only_the_old_unit() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    let (environment, old, _unit) = running(&world, &host, template, "Main").await;
    newer_image(&world, template).await;
    let before = envs::runtime(&mut world.pool.acquire().await.unwrap(), old)
        .await
        .unwrap()
        .unwrap();
    let prepared = draft(&world, environment, UpdatePreflightReason::Update).await;
    assert!(!prepared.updates.is_empty());
    let row = queue(&world, prepared).await;
    let (state, _, error) = wait_over(&world, row.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let mut conn = world.pool.acquire().await.unwrap();
    let after = envs::live_runtime(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    let env = envs::environment(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(after.id, old);
    assert_eq!(after.volumes, before.volumes);
    assert_eq!(env.owner_id, world.ada);
    assert_eq!(env.generation, 2);
    assert!(after.successor_of.is_none());
    assert_eq!(env.status, "running");
    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE environment_id = $1 AND action = 'environment.update_applied'")
        .bind(environment).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(applied, 1);
    assert_eq!(
        envs::runtime(&mut conn, old).await.unwrap().unwrap().state,
        "retired"
    );
    drop(conn);
    assert!(
        draft(&world, environment, UpdatePreflightReason::Update)
            .await
            .updates
            .is_empty()
    );
    world.engine().stop();
}

#[tokio::test]
async fn a_failed_replacement_restores_the_old_unit_generation_files_and_pending_inputs() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    let (environment, old, _unit) = running(&world, &host, template, "Main").await;
    let thread = world.thread("Work").await;
    bind(&world, thread, environment).await;
    host.refuse(
        CommandKind::NodeRuntimeCreate,
        CommandError::new(ErrorCode::Conflict, "the new unit cannot start"),
    );
    let row = queue(
        &world,
        draft(&world, environment, UpdatePreflightReason::Update).await,
    )
    .await;
    let (state, _, error) = wait_over(&world, row.id).await;
    assert_eq!(state, "failed");
    assert_eq!(error.unwrap()["undone"], true);
    let mut conn = world.pool.acquire().await.unwrap();
    let env = envs::environment(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((env.status.as_str(), env.generation), ("running", 1));
    assert_eq!(
        envs::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .id,
        old
    );
    assert!(
        jobs::changes::thread_hold(&mut conn, thread)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(host.calls().last().unwrap().body["runtime_id"], json!(old));
    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE environment_id = $1 AND action = 'environment.update_applied'")
        .bind(environment).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(applied, 0);
    world.engine().stop();
}

#[tokio::test]
async fn the_old_unit_restarts_even_when_removing_a_failed_candidate_must_be_retried() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    // The old unit's connector stays online: the restarted unit is attached again through it.
    let (environment, old, _unit) = running(&world, &host, template, "Main").await;
    host.refuse(
        CommandKind::NodeRuntimeCreate,
        CommandError::new(ErrorCode::Conflict, "candidate cannot start"),
    );
    host.refuse(
        CommandKind::NodeRuntimeRemove,
        CommandError::new(ErrorCode::Conflict, "host cannot remove it yet"),
    );
    let row = queue(
        &world,
        draft(&world, environment, UpdatePreflightReason::Update).await,
    )
    .await;
    let (state, _, error) = wait_over(&world, row.id).await;
    assert_eq!(state, "failed");
    assert_eq!(error.unwrap()["undone"], false);
    let mut conn = world.pool.acquire().await.unwrap();
    let env = envs::environment(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((env.status.as_str(), env.generation), ("running", 1));
    assert_eq!(
        envs::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .id,
        old
    );
    let row = jobs::job(&mut conn, row.id).await.unwrap().unwrap();
    let next: Id = serde_json::from_value(row.plan["cleanup_pending"].clone()).unwrap();
    let candidate = envs::runtime(&mut conn, next).await.unwrap().unwrap();
    assert_eq!(candidate.state, "retired");
    assert!(candidate.remove_at.is_some());
    assert!(
        host.calls()
            .iter()
            .any(|c| c.kind == CommandKind::NodeRuntimeStart && c.body["runtime_id"] == json!(old))
    );
    world.engine().stop();
}

#[tokio::test]
async fn one_hold_serves_updates_switches_and_topic_moves_and_rejects_an_overlapping_job() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    let (environment, _old, _unit) = running(&world, &host, template, "Main").await;
    let thread = world.thread("Work").await;
    bind(&world, thread, environment).await;
    let prepared = draft(&world, environment, UpdatePreflightReason::Update).await;
    let mut tx = world.pool.begin().await.unwrap();
    let row =
        orch_engine::jobs::recreate::queue(&mut tx, prepared.clone(), None, world.clock.now())
            .await
            .unwrap();
    assert_eq!(
        jobs::forks::lock_of(&mut tx, thread)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
    assert_eq!(
        orch_db::queries::hierarchy::threads_held_in_topic(&mut tx, world.topic)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        jobs::forks::environment_is_being_changed(&mut tx, environment)
            .await
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
    assert!(matches!(
        orch_engine::jobs::recreate::queue(&mut tx, prepared, None, world.clock.now()).await,
        Err(UpdateError::Held(_))
    ));
    tx.rollback().await.unwrap();
    world.engine().stop();
}

#[tokio::test]
async fn a_person_who_does_not_manage_an_environment_cannot_plan_its_replacement() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Main").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let stranger = orch_db::queries::resources::testing::insert_user(&mut conn, "visitor")
        .await
        .unwrap();
    let principal = Principal::load(
        &mut conn,
        Caller::session(stranger),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    assert!(matches!(
        world
            .engine()
            .draft_update(
                &mut conn,
                &principal,
                Target::Environment(environment),
                UpdatePreflightReason::Update,
                None
            )
            .await,
        Err(UpdateError::Denied)
    ));
    world.engine().stop();
}

#[tokio::test]
async fn a_recreate_uses_the_recorded_definition_even_after_the_template_was_changed() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Main").await;
    let first = queue(
        &world,
        draft(&world, environment, UpdatePreflightReason::Update).await,
    )
    .await;
    assert_eq!(wait_over(&world, first.id).await.0, "succeeded");
    sqlx::query(
        "UPDATE env_templates SET spec = jsonb_set(spec,'{image}','\"another:tag\"') WHERE id = $1",
    )
    .bind(template)
    .execute(world.pool.pool())
    .await
    .unwrap();
    let same = draft(&world, environment, UpdatePreflightReason::Recreate).await;
    assert!(same.updates.is_empty());
    assert_eq!(same.recipe.definition["image"], "busybox:latest");
    let changed = draft(&world, environment, UpdatePreflightReason::Update).await;
    assert_eq!(changed.recipe.definition["image"], "another:tag");
    assert!(!changed.updates.is_empty());
    sqlx::query("UPDATE environments SET config = $2 WHERE id = $1")
        .bind(environment)
        .bind(json!({"env.template_id":orch_types::new_id()}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let retained = draft(&world, environment, UpdatePreflightReason::Recreate).await;
    assert_eq!(retained.recipe.template, template);
    assert_eq!(retained.recipe.definition["image"], "busybox:latest");
    world.engine().stop();
}

#[test]
fn a_release_requirement_compares_versions_numerically_and_unknown_versions_are_not_guessed() {
    assert!(orch_engine::updates::update_required(
        Some("0.9.1"),
        "0.10.0"
    ));
    assert!(!orch_engine::updates::update_required(
        Some("0.10.1-build"),
        "0.9.9"
    ));
    assert!(!orch_engine::updates::update_required(None, "1.0.0"));
}

#[tokio::test]
async fn a_supported_mount_change_is_planned_and_storage_transfer_is_blocked_before_any_unit_stops()
{
    use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
    use orch_authz::ownership::{NewAccess, start_resource};
    use orch_db::queries::resources::mounts::{self as mounts, LocationRow, MountRow};
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _) = running(&world, &host, template, "Main").await;
    let mount = orch_types::new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(jsonb_set(drivers,'{docker,mounts}','\"placed\"'::jsonb),'{docker,private_views}','true'::jsonb),storages=$2 WHERE id=$1")
        .bind(host.id).bind(json!([{"name":"fast","default":true,"capacity_gb":100,"free_gb":80,"private_views":true}]))
        .execute(&mut *conn).await.unwrap();
    mounts::insert(
        &mut conn,
        &MountRow {
            id: mount,
            name: "updates-data".into(),
            description: String::new(),
            org_id: Some(world.root),
            home_id: Some(world.root),
            owner_id: Some(world.ada),
            target: "/data/updates".into(),
            read_only: true,
            enabled: true,
            default_fate: "as_is".into(),
            revision: 1,
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Mount.into(),
            id: mount,
        },
        world.ada,
        NewAccess::OnlyMe,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    mounts::set_location(
        &mut conn,
        &LocationRow {
            mount_id: mount,
            machine_id: host.machine,
            host_id: Some(host.id),
            host_name: Some("build".into()),
            kind: "path".into(),
            source: "/srv/updates".into(),
            storage: None,
            data_kind: None,
            created_by: world.ada,
            created_at: world.clock.now(),
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET config = $2 WHERE id = $1")
        .bind(environment)
        .bind(json!({"env.mounts":{(mount.to_string()):"private"}}))
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    // Mount attachment is served now. A real usable mount is reviewed; an invented id
    // cannot prove a mount change, and the former blanket unavailable blocker is obsolete.
    let mounts_only = draft(&world, environment, UpdatePreflightReason::Update).await;
    assert!(
        mounts_only.blockers.is_empty(),
        "{:?}",
        mounts_only.blockers
    );
    assert!(
        mounts_only
            .updates
            .iter()
            .any(|change| change.name == "Mount attachments")
    );
    let resolved = orch_engine::mounts::resolve(
        &mut world.pool.acquire().await.unwrap(),
        environment,
        world.clock.now(),
    )
    .await
    .unwrap();
    let actual = resolved
        .iter()
        .find(|entry| entry.view.mount.id == mount)
        .unwrap()
        .spec
        .as_ref()
        .unwrap();
    assert_eq!(actual["source"], "/srv/updates");
    assert_eq!(actual["mode"], "private");
    assert!(
        actual["layer"]["name"]
            .as_str()
            .unwrap()
            .starts_with("orch-over-")
    );
    assert_eq!(
        host.count(CommandKind::NodeRuntimeStop),
        0,
        "review itself has no effects"
    );
    sqlx::query("UPDATE environments SET config = $2 WHERE id = $1")
        .bind(environment)
        .bind(json!({"env.mounts":{(mount.to_string()):"private"},"env.storage":{"workspace":"another-storage"}}))
        .execute(world.pool.pool()).await.unwrap();
    let prepared = draft(&world, environment, UpdatePreflightReason::Update).await;
    assert!(
        !prepared
            .blockers
            .iter()
            .any(|b| b.code == "mount_recreate_unavailable")
    );
    assert!(
        prepared
            .blockers
            .iter()
            .any(|b| b.code == "storage_transfer_unavailable")
    );
    let mut tx = world.pool.begin().await.unwrap();
    assert!(matches!(
        orch_engine::jobs::recreate::queue(&mut tx, prepared, None, world.clock.now()).await,
        Err(UpdateError::Blocked(_))
    ));
    tx.rollback().await.unwrap();
    assert_eq!(host.count(CommandKind::NodeRuntimeStop), 0);
    world.engine().stop();
}

#[tokio::test]
async fn a_plain_recreate_does_not_label_a_new_program_as_the_unavailable_original_build() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    let (environment, old, _) = running(&world, &host, template, "Main").await;
    let definition: orch_types::Json =
        sqlx::query_scalar("SELECT spec FROM env_templates WHERE id = $1")
            .bind(template)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    sqlx::query("INSERT INTO jobs (id,kind,environment_id,state,plan) VALUES ($1,'environment_provision',$2,'succeeded',$3)")
        .bind(orch_types::new_id()).bind(environment)
        .bind(json!({"runtime":old,"created_spec":{"spec":definition,"resources":{},"tools":[{"name":"orch-connector","version":"unavailable-original"}]}}))
        .execute(world.pool.pool()).await.unwrap();
    let prepared = draft(&world, environment, UpdatePreflightReason::Recreate).await;
    assert!(
        prepared
            .blockers
            .iter()
            .any(|b| b.code == "tool_build_unavailable")
    );
    assert_eq!(prepared.recipe.tools[0].version, "0.1.0");
    let mut tx = world.pool.begin().await.unwrap();
    assert!(matches!(
        orch_engine::jobs::recreate::queue(&mut tx, prepared, None, world.clock.now()).await,
        Err(UpdateError::Blocked(_))
    ));
    tx.rollback().await.unwrap();
    assert_eq!(host.count(CommandKind::NodeRuntimeStop), 0);
    world.engine().stop();
}

#[tokio::test]
async fn a_replacement_preserves_the_native_session_in_the_new_segment() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    let (environment, old, unit) = running(&world, &host, template, "Main").await;
    let thread = world.thread("Work").await;
    bind(&world, thread, environment).await;
    let before = world.segment_of(thread).await;
    sqlx::query("UPDATE segments SET environment_id = $2, env_generation = 1, native_session_ref = 'kept-session' WHERE id = $1").bind(before).bind(environment).execute(world.pool.pool()).await.unwrap();
    let job = queue(
        &world,
        draft(&world, environment, UpdatePreflightReason::Update).await,
    )
    .await;
    assert_eq!(wait_over(&world, job.id).await.0, "succeeded");
    let after = world.segment_of(thread).await;
    assert_ne!(before, after);
    let mut conn = world.pool.acquire().await.unwrap();
    let segment = orch_db::queries::threads::segment(&mut conn, after)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(segment.native_session_ref.as_deref(), Some("kept-session"));
    assert_eq!(segment.env_generation, 2);
    drop(conn);
    common::eventually(
        "the replaced unit is removed before its successor works",
        || async {
            envs::runtime(&mut world.pool.acquire().await.unwrap(), old)
                .await
                .unwrap()
                .is_some_and(|runtime| runtime.state == "removed")
        },
    )
    .await;
    let connector =
        orch_db::queries::resources::machine(&mut world.pool.acquire().await.unwrap(), unit.id)
            .await
            .unwrap()
            .unwrap();
    assert!(
        connector.is_active(),
        "removing the old unit keeps its successor's connector"
    );
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            common::queued("Continue"),
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert!(
        start.text.contains("The runtime was recreated"),
        "{}",
        start.text
    );
    common::eventually("runtime notices are consumed by the next turn", || async {
        let notices: orch_types::Json =
            sqlx::query_scalar("SELECT pending_notices FROM threads WHERE node_id = $1")
                .bind(thread)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        notices.as_array().unwrap().is_empty()
    })
    .await;
    world.engine().stop();
}

#[tokio::test]
async fn an_update_warns_a_running_agent_by_steer_and_an_expired_wait_changes_no_unit() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    let (environment, old, _unit) = running(&world, &host, template, "Main").await;
    let thread = world.thread("Work").await;
    bind(&world, thread, environment).await;
    let (turn, _) = world.running_turn(thread, "Work on this").await;
    assert!(
        !world
            .engine()
            .recyclable(environment, 0, true)
            .await
            .unwrap()
    );
    let mut prepared = draft(&world, environment, UpdatePreflightReason::Update).await;
    prepared.recipe.options.wait_minutes = 0;
    let job = queue(&world, prepared).await;
    let (state, _, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "failed");
    assert_eq!(error.unwrap()["code"], "thread.not_idle");
    assert!(world.seen.all().iter().any(|command| {
        matches!(command, orch_link_contract::commands::Command::TurnSteer(body)
            if body.turn_id == turn.turn_id && body.text.contains("recreated shortly"))
    }));
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(
        envs::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .id,
        old
    );
    assert_eq!(host.count(CommandKind::NodeRuntimeStop), 0);
    assert_eq!(world.turn_state(turn.turn_id).await, "running");
    world.engine().stop();
}

#[tokio::test]
async fn cancelling_a_replacement_before_commit_restores_the_old_unit() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    let (environment, old, _unit) = running(&world, &host, template, "Main").await;
    host.added
        .machine
        .handler
        .hold(CommandKind::NodeImageEnsure);
    let job = queue(
        &world,
        draft(&world, environment, UpdatePreflightReason::Update).await,
    )
    .await;
    host.wait_began(CommandKind::NodeImageEnsure, 1).await;
    assert!(world.engine().cancel_job(job.id, world.ada).await.unwrap());
    let (state, _, error) = wait_over(&world, job.id).await;
    host.added.machine.handler.release();
    assert_eq!(state, "cancelled", "{error:?}");
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(
        envs::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .id,
        old
    );
    assert_eq!(
        envs::environment(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .generation,
        1
    );
    world.engine().stop();
}

#[tokio::test]
async fn only_an_owners_on_start_policy_updates_a_stopped_unit_and_a_failure_starts_the_old_one() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build", 16_384).await;
    let template = template(&world).await;
    let (environment, old, _unit) = running(&world, &host, template, "Main").await;
    newer_image(&world, template).await;
    sqlx::query("UPDATE environments SET status = 'stopped', config = '{\"env.update\":\"on_start\"}' WHERE id = $1").bind(environment).execute(world.pool.pool()).await.unwrap();
    sqlx::query("UPDATE runtimes SET state = 'stopped' WHERE id = $1")
        .bind(old)
        .execute(world.pool.pool())
        .await
        .unwrap();
    host.refuse(
        CommandKind::NodeRuntimeCreate,
        CommandError::new(ErrorCode::Conflict, "cannot make the newer unit"),
    );
    let started = world
        .engine()
        .start_environment(environment, None)
        .await
        .unwrap();
    let orch_engine::environments::Starting::Job(job) = started else {
        panic!("an update was queued")
    };
    assert_eq!(job.kind, "environment_recreate");
    assert_eq!(wait_over(&world, job.id).await.0, "failed");
    let mut conn = world.pool.acquire().await.unwrap();
    let env = envs::environment(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(env.status, "running");
    assert_eq!(env.generation, 1);
    let told: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'environment.update'",
    )
    .bind(world.ada)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(told, 1);
    assert_eq!(
        envs::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .id,
        old
    );
    world.engine().stop();
}
