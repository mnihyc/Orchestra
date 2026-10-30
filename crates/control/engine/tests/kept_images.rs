//! Reviewed replacements retain installed files on preserving drivers and clean only the exact succeeded or failed capture.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
mod hosting;

use common::{World, eventually};
use hosting::ScriptedHost;
use orch_api_contract::environment::UpdatePreflightReason;
use orch_authz::{Caller, Principal};
use orch_engine::updates::{Draft, Target};
use orch_link_contract::commands::{CommandError, CommandKind, ErrorCode};
use orch_types::{Id, Time, new_id};
use serde_json::json;

async fn fixture() -> (World, ScriptedHost, Id, Id, Id, hosting::Unit) {
    let world = World::new().await;
    hosting::serve(&world);
    let host = ScriptedHost::on(&world, "kept-driver", 16_384).await;
    let template = hosting::template(&world).await;
    let (environment, old, unit) =
        hosting::running(&world, &host, template, "Installed files").await;
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,recreate_preserves_files}','true') WHERE id=$1").bind(host.id).execute(world.pool.pool()).await.unwrap();
    (world, host, environment, old, template, unit)
}

async fn draft(world: &World, environment: Id, reason: UpdatePreflightReason) -> Draft {
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

async fn queue(world: &World, draft: Draft) -> Id {
    let mut tx = world.pool.begin().await.unwrap();
    let row = orch_engine::jobs::recreate::queue(&mut tx, draft, None, world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    row.id
}

#[tokio::test]
async fn a_same_image_replacement_reviews_the_recording_cost_and_creates_from_the_exact_kept_capture()
 {
    let (world, host, environment, old, _, _unit) = fixture().await;
    let reviewed = draft(&world, environment, UpdatePreflightReason::Recreate).await;
    assert!(reviewed.recipe.preserve_files);
    assert_eq!(reviewed.recipe.keep_estimate.unwrap().bytes, 64 << 20);
    assert!(
        reviewed
            .kept
            .iter()
            .any(|line| line.contains("installed programs"))
    );
    assert!(
        !reviewed
            .lost
            .iter()
            .any(|line| line.contains("outside persistent volumes"))
    );
    let job = queue(&world, reviewed).await;
    let (state, _, error) = hosting::wait_over(&world, job).await;
    assert_eq!(state, "succeeded", "{error:?}");
    // The capture is recorded in the job's plan; what the wait returns is its progress.
    let plan: serde_json::Value = world
        .scalar("SELECT plan FROM jobs WHERE id = $1", job)
        .await;
    let image = plan["kept_image"].clone();
    assert_eq!(image["capture"], job.to_string());
    let calls = host.calls();
    let stop = calls
        .iter()
        .position(|call| call.kind == CommandKind::NodeRuntimeStop)
        .unwrap();
    let keep = calls
        .iter()
        .position(|call| call.kind == CommandKind::NodeKeepFiles)
        .unwrap();
    let create = calls
        .iter()
        .position(|call| call.kind == CommandKind::NodeRuntimeCreate)
        .unwrap();
    assert!(stop < keep && keep < create);
    assert_eq!(calls[create].body["kept_image"], image);
    let mut conn = world.pool.acquire().await.unwrap();
    let current = orch_db::queries::environments::hosting::live_runtime(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(current.id, old);
    assert_eq!(current.kept_image, Some(image));
    world.engine().stop();
}

#[tokio::test]
async fn a_failed_successor_removes_its_capture_and_restores_the_original_unit() {
    let (world, host, environment, old, _, _unit) = fixture().await;
    host.refuse(
        CommandKind::NodeRuntimeCreate,
        CommandError::new(ErrorCode::Conflict, "candidate cannot start"),
    );
    let job = queue(
        &world,
        draft(&world, environment, UpdatePreflightReason::Recreate).await,
    )
    .await;
    let (state, _, _) = hosting::wait_over(&world, job).await;
    assert_eq!(state, "failed");
    eventually("failed kept capture cleanup", || async {
        host.count(CommandKind::NodeKeptRemove) > 0
    })
    .await;
    let removed = host
        .calls()
        .into_iter()
        .find(|call| call.kind == CommandKind::NodeKeptRemove)
        .unwrap();
    assert_eq!(removed.body["image"]["capture"], job.to_string());
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(
        orch_db::queries::environments::hosting::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .id,
        old
    );
    assert!(
        host.calls()
            .iter()
            .any(|call| call.kind == CommandKind::NodeRuntimeStart
                && call.body["runtime_id"] == old.to_string())
    );
    world.engine().stop();
}

#[tokio::test]
async fn a_new_image_is_reviewed_as_losing_installed_files_and_drops_the_old_environment_image_after_commit()
 {
    let (world, host, environment, old, template, _unit) = fixture().await;
    let capture = new_id();
    let image = json!({"system":"test","environment":environment,"generation":1,"capture":capture,"reference":format!("orch-keep-{environment}:1"),"bytes":64<<20,"total_bytes":72<<20,"layers":1});
    sqlx::query("UPDATE runtimes SET kept_image=$2 WHERE id=$1")
        .bind(old)
        .bind(image.clone())
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE env_templates SET spec=jsonb_set(spec,'{image}','\"busybox:changed\"') WHERE id=$1",
    )
    .bind(template)
    .execute(world.pool.pool())
    .await
    .unwrap();
    let reviewed = draft(&world, environment, UpdatePreflightReason::Update).await;
    assert!(!reviewed.recipe.preserve_files);
    assert!(
        reviewed
            .lost
            .iter()
            .any(|line| line.contains("/usr, /etc and /opt"))
    );
    let job = queue(&world, reviewed).await;
    let (state, _, error) = hosting::wait_over(&world, job).await;
    assert_eq!(state, "succeeded", "{error:?}");
    eventually("old image cleanup after an image change", || async {
        host.count(CommandKind::NodeKeptRemove) > 0
    })
    .await;
    assert_eq!(host.count(CommandKind::NodeKeepFiles), 0);
    let create = host
        .calls()
        .into_iter()
        .find(|call| call.kind == CommandKind::NodeRuntimeCreate)
        .unwrap();
    assert!(
        create
            .body
            .get("kept_image")
            .is_none_or(|image| image.is_null())
    );
    let removed = host
        .calls()
        .into_iter()
        .find(|call| call.kind == CommandKind::NodeKeptRemove)
        .unwrap();
    assert_eq!(removed.body["image"], image);
    world.engine().stop();
}

#[tokio::test]
async fn the_hourly_whitelist_retains_only_the_current_generation_and_does_not_reserve_reaped_history_forever()
 {
    let (world, host, environment, old, template, _unit) = fixture().await;
    let older = orch_driver_contract::kept::KeptImage {
        system: "test".into(),
        environment,
        generation: 1,
        capture: new_id(),
        reference: format!("orch-keep-{environment}:1"),
        bytes: 100,
        total_bytes: 200,
        layers: 1,
    };
    let current = orch_driver_contract::kept::KeptImage {
        generation: 2,
        capture: new_id(),
        reference: format!("orch-keep-{environment}:2"),
        bytes: 150,
        layers: 2,
        ..older.clone()
    };
    let mut conn = world.pool.acquire().await.unwrap();
    let previous = orch_db::queries::environments::hosting::runtime(&mut conn, old)
        .await
        .unwrap()
        .unwrap();
    let next = new_id();
    orch_db::queries::environments::changes::successor(
        &mut conn,
        &previous,
        next,
        "current-kept-unit",
        template,
        &previous.spec_hash,
        world.clock.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE runtimes SET state='removed',kept_image=$2 WHERE id=$1")
        .bind(old)
        .bind(json!(older))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE runtimes SET successor_of=NULL,state='running',kept_image=$2 WHERE id=$1")
        .bind(next)
        .bind(json!(current))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET generation=2 WHERE id=$1")
        .bind(environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    host.added
        .machine
        .publish(
            &orch_link_contract::streams::Stream::Node,
            &orch_link_contract::events::Event::NodeKeptImages(
                orch_link_contract::events::NodeKeptImages {
                    epoch: 7,
                    images: vec![older, current.clone()],
                },
            ),
        )
        .await;
    host.wait_for(CommandKind::NodeKeptSweep, 1).await;
    let sweep = host
        .calls()
        .into_iter()
        .find(|call| call.kind == CommandKind::NodeKeptSweep)
        .unwrap();
    let owners = sweep.body["owners"].as_array().unwrap();
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0]["capture"], current.capture.to_string());
    world.engine().stop();
}
