//! Management admits the same native updater once, under system administration.
use super::*;
use orch_api_contract::admin::{
    AdminServerUpdateGet, AdminServerUpdateStart, ServerUpdateAvailability, ServerUpdateStart,
};
use orch_db::queries::resources::access_fixtures as fixtures;
use orch_types::Id;

async fn offered() -> (World, admin_support::Cast, Site) {
    let site = Site::new().await;
    let mut config = ServerConfig::default();
    config.release = site.config.clone();
    let world = World::with(config).await;
    let cast = admin_support::cast(&world).await;
    world
        .app
        .call(AdminReleaseCheck, &[])
        .token(&cast.root.token)
        .send()
        .await
        .ok();
    (world, cast, site)
}
async fn bundled(world: &World, by: Id, docker: bool) -> Id {
    let mut conn = world.conn().await;
    let host = fixtures::insert_resource(&mut conn, "host", Some(by), None, "Server host")
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET bundled_with_server=true WHERE id=$1")
        .bind(host)
        .execute(&mut *conn)
        .await
        .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online=true,roles=ARRAY['node'],facts=jsonb_build_object('server_update',jsonb_build_object('supported',$2,'reason',CASE WHEN $2 THEN NULL ELSE 'Docker access is unavailable' END)) WHERE id=$1")
        .bind(machine).bind(docker).execute(&mut *conn).await.unwrap();
    machine
}

#[tokio::test]
async fn only_a_system_administrator_can_ask_the_bundled_node_to_update_the_server() {
    let (world, cast, _site) = offered().await;
    bundled(&world, cast.root.id(), true).await;
    let body = ServerUpdateStart {
        release: "1.2.3".into(),
    };
    for who in [&cast.boss, &cast.ada] {
        let problem = world
            .app
            .call(AdminServerUpdateStart, &[])
            .token(&who.token)
            .body(&body)
            .send()
            .await
            .problem();
        assert_eq!(problem.code, ErrorCode::AuthzDenied);
    }
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='server_update'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
    let accepted = world
        .app
        .call(AdminServerUpdateStart, &[])
        .token(&cast.root.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(accepted.kind, "server_update");
    assert_eq!(
        accepted.created_by.as_ref().map(|who| who.id),
        Some(cast.root.id())
    );
    assert!(accepted.environment_id.is_none());
}

#[tokio::test]
async fn another_press_joins_the_running_server_update_instead_of_launching_a_second_updater() {
    let (world, cast, _site) = offered().await;
    bundled(&world, cast.root.id(), true).await;
    let body = ServerUpdateStart {
        release: "1.2.3".into(),
    };
    let first = world
        .app
        .call(AdminServerUpdateStart, &[])
        .token(&cast.root.token)
        .body(&body)
        .send()
        .await
        .ok();
    let second = world
        .app
        .call(AdminServerUpdateStart, &[])
        .token(&cast.root.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(first.id, second.id);
    let read = world
        .app
        .call(AdminServerUpdateGet, &[])
        .token(&cast.root.token)
        .send()
        .await
        .ok();
    assert_eq!(read.job.unwrap().id, first.id);
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='server_update'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(jobs, 1);
}

#[tokio::test]
async fn management_names_a_missing_bundled_node_or_its_missing_docker_access_and_keeps_the_shell_command()
 {
    let (world, cast, _site) = offered().await;
    let absent = world
        .app
        .call(AdminServerUpdateGet, &[])
        .token(&cast.root.token)
        .send()
        .await
        .ok();
    assert_eq!(absent.availability, ServerUpdateAvailability::NoBundledNode);
    assert_eq!(absent.command, "./update.sh v1.2.3");
    bundled(&world, cast.root.id(), false).await;
    let denied = world
        .app
        .call(AdminServerUpdateGet, &[])
        .token(&cast.root.token)
        .send()
        .await
        .ok();
    assert_eq!(
        denied.availability,
        ServerUpdateAvailability::DockerUnavailable
    );
    let body = ServerUpdateStart {
        release: "1.2.3".into(),
    };
    assert_eq!(
        world
            .app
            .call(AdminServerUpdateStart, &[])
            .token(&cast.root.token)
            .body(&body)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::CapabilityUnsupported
    );
}

#[tokio::test]
async fn reading_an_available_server_release_never_installs_it_while_automatic_updates_are_off() {
    let (world, cast, _site) = offered().await;
    bundled(&world, cast.root.id(), true).await;
    for _ in 0..2 {
        let read = world
            .app
            .call(AdminServerUpdateGet, &[])
            .token(&cast.root.token)
            .send()
            .await
            .ok();
        assert_eq!(read.availability, ServerUpdateAvailability::Ready);
        assert!(read.job.is_none());
    }
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='server_update'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
}

#[tokio::test]
async fn a_rollback_observation_is_kept_for_the_returning_admin_and_another_node_cannot_supply_it()
{
    let (world, cast, _site) = offered().await;
    let machine = bundled(&world, cast.root.id(), true).await;
    let body = ServerUpdateStart {
        release: "1.2.3".into(),
    };
    let job = world
        .app
        .call(AdminServerUpdateStart, &[])
        .token(&cast.root.token)
        .body(&body)
        .send()
        .await
        .ok();
    let mut conn = world.conn().await;
    let host: Id = sqlx::query_scalar("SELECT id FROM hosts WHERE machine_id=$1")
        .bind(machine)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let row = orch_db::queries::jobs::job(&mut conn, job.id)
        .await
        .unwrap()
        .unwrap();
    let request: orch_engine::jobs::server_update::Request =
        serde_json::from_value(row.plan).unwrap();
    let command = orch_engine::commands::Outgoing::new(
        orch_types::new_id(),
        machine,
        orch_link_contract::commands::SerialKey::Machine,
        &orch_link_contract::commands::NodeServerUpdate {
            job_id: job.id,
            release: body.release.clone(),
            timeout_secs: request.prepare_timeout_secs,
        },
    )
    .unwrap()
    .by(Time::new(world.now() + Duration::seconds(60)));
    let source =
        orch_engine::jobs::server_update::command::Source::of(job.id, &request, &command).unwrap();
    let authority = serde_json::to_value(
        request
            .origin
            .command_source(
                &command,
                orch_engine::operation_scope::http_command_source::CommandEvidence::ServerUpdate(
                    source,
                ),
            )
            .unwrap(),
    )
    .unwrap();
    let key = command.key.wire();
    orch_db::queries::jobs::pass_commit(&mut conn, job.id, world.now())
        .await
        .unwrap();
    orch_db::queries::link::insert_command_with_authority(
        &mut conn,
        &orch_db::queries::link::NewCommand {
            id: command.id,
            machine_id: machine,
            kind: command.kind.as_str(),
            body: &command.body,
            serial_key: key.as_deref(),
            deadline_at: world.now() + Duration::seconds(60),
        },
        world.now(),
        Some(&authority),
    )
    .await
    .unwrap();
    let report = orch_link_contract::events::NodeServerUpdate {
        command_id: command.id,
        job_id: job.id,
        observation: orch_api_contract::admin::ServerUpdateObservation {
            stage: orch_api_contract::admin::ServerUpdateStage::RolledBack,
            reason: Some(orch_api_contract::admin::ServerUpdateFailure::HealthCheckFailed),
            from_version: Some("1.2.2".into()),
            to_version: "1.2.3".into(),
        },
    };
    orch_engine::jobs::server_update::report(
        &mut conn,
        host,
        orch_types::new_id(),
        &report,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    drop(conn);
    let before = world
        .app
        .call(AdminServerUpdateGet, &[])
        .token(&cast.root.token)
        .send()
        .await
        .ok();
    assert!(before.observation.is_none());
    let mut conn = world.conn().await;
    orch_engine::jobs::server_update::report(
        &mut conn,
        host,
        machine,
        &report,
        Time::new(world.now()),
    )
    .await
    .unwrap();
    drop(conn);
    for _ in 0..2 {
        let returned = world
            .app
            .call(AdminServerUpdateGet, &[])
            .token(&cast.root.token)
            .send()
            .await
            .ok();
        assert_eq!(returned.observation.as_ref(), Some(&report.observation));
        assert_eq!(
            returned.job.as_ref().unwrap().state,
            orch_api_contract::nodes::JobState::Failed
        );
        assert!(
            returned
                .job
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .undone
        );
        assert_eq!(returned.job.as_ref().map(|row| row.id), Some(job.id));
    }
}

#[tokio::test]
async fn the_native_server_update_keeps_the_original_administrator_at_delivery() {
    let (world, cast, _site) = offered().await;
    bundled(&world, cast.root.id(), true).await;
    let body = ServerUpdateStart {
        release: "1.2.3".into(),
    };
    world
        .set("server.update_prepare_timeout_secs", json!(120))
        .await;
    let job = world
        .app
        .call(AdminServerUpdateStart, &[])
        .token(&cast.root.token)
        .body(&body)
        .send()
        .await
        .ok();
    let mut tx = world.app.app.db.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    let row = orch_db::queries::jobs::job(&mut tx, job.id)
        .await
        .unwrap()
        .unwrap();
    let request: orch_engine::jobs::server_update::Request =
        serde_json::from_value(row.plan).unwrap();
    let outgoing = orch_engine::commands::Outgoing::new(
        orch_types::new_id(),
        request.machine,
        orch_link_contract::commands::SerialKey::Machine,
        &orch_link_contract::commands::NodeServerUpdate {
            job_id: job.id,
            release: body.release,
            timeout_secs: request.prepare_timeout_secs,
        },
    )
    .unwrap();
    let source =
        orch_engine::jobs::server_update::command::Source::of(job.id, &request, &outgoing).unwrap();
    let key = outgoing.key.wire();
    let native = orch_db::queries::link::NewCommand {
        id: outgoing.id,
        machine_id: outgoing.machine,
        kind: outgoing.kind.as_str(),
        body: &outgoing.body,
        serial_key: key.as_deref(),
        deadline_at: world.now() + Duration::seconds(30),
    };
    assert!(
        source
            .current_issue(&mut tx, &native, world.app.app.clock.as_ref())
            .await
            .unwrap()
            .is_some()
    );
    sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
        .bind(cast.root.id())
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        source
            .current_issue(&mut tx, &native, world.app.app.clock.as_ref())
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
}
