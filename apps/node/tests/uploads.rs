//! A Files upload cancelled after real native writes waits for the connector's stopped result and preserves the original target.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Fleet, Lab, NodeUnderTest, World, eventually};
use futures::TryStreamExt;
use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
use orch_api_contract::environment::ThroughThreadQuery;
use orch_api_contract::nodes::JobState;
use orch_api_contract::threads::{
    UploadSessionComplete, UploadSessionCreate, UploadSessionCreateBody, UploadSessionPut,
    UploadSessionPutQuery,
};
use orch_api_contract::workspace::{
    EnvironmentUploadWrite, EnvironmentUploadWriteBody, EnvironmentUploadWriteResult, Job,
    JobCancel, JobGet,
};
use orch_api_core::middleware::authentication::{ACCESS_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::{registry::Registry, testing::TestApp, thread_engine::EngineApplier};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_link_contract::commands::{EnvAttach, FsStat, IndexMode, SerialKey};
use orch_types::{Hash, Id, Time, new_id};
use std::sync::Arc;

async fn staged_files(lab: &Lab, unit: &str) -> Vec<(String, u64)> {
    let options = bollard::query_parameters::DownloadFromContainerOptionsBuilder::default()
        .path("/workspace")
        .build();
    let pieces = lab
        .docker
        .download_from_container(unit, Some(options))
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let bytes: Vec<u8> = pieces
        .iter()
        .flat_map(|piece| piece.iter().copied())
        .collect();
    let mut archive = tar::Archive::new(bytes.as_slice());
    archive
        .entries()
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            let path = entry.path().unwrap();
            let name = path.file_name()?.to_str()?;
            name.starts_with(orch_envhost::paths::TEMP_PREFIX)
                .then(|| (name.to_owned(), entry.header().size().unwrap()))
        })
        .collect()
}

async fn uploaded(app: &TestApp, token: &str, bytes: &[u8]) -> Hash {
    let session = app
        .call(UploadSessionCreate, &[])
        .token(token)
        .body(&UploadSessionCreateBody {
            name: "authored.txt".into(),
            size: bytes.len() as i64,
            media_type: Some("text/plain".into()),
        })
        .send()
        .await
        .ok();
    let mut offset = 0;
    for piece in bytes.chunks(session.chunk_bytes as usize) {
        let received = app
            .call(UploadSessionPut, &[&session.upload_id])
            .token(token)
            .query(&UploadSessionPutQuery { offset })
            .bytes(piece)
            .send()
            .await
            .ok();
        offset = received.received;
    }
    app.call(UploadSessionComplete, &[&session.upload_id])
        .token(token)
        .send()
        .await
        .ok()
        .blob
}

async fn pin_count(world: &World, hash: Hash) -> i32 {
    orch_db::queries::storage::blob(&mut world.db.acquire().await.unwrap(), &hash)
        .await
        .unwrap()
        .unwrap()
        .pin_count
}

async fn read_job(app: &TestApp, token: &str, id: Id) -> Job {
    app.call(JobGet, &[&id]).token(token).send().await.ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_dispatched_files_write_joins_the_real_connector_before_cancelled_and_keeps_the_authored_target()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let mut world = World::start(&lab.bridge).await;
    world
        .api
        .link
        .set_applier(Arc::new(EngineApplier::new(world.api.app.engine.clone())));
    let mut node = NodeUnderTest::start(&world, &lab).await;
    let fleet = Fleet::new(&world, &lab, &node);
    let _program = fleet.prepare().await;
    let unit = fleet.unit("upload-stop");
    fleet.make(&unit, None).await.unwrap();
    fleet.start(&unit).await.unwrap();
    let machine = world.wait_enrolled(&unit.hint).await;
    world.wait_online(machine, true).await;
    let original = "authored before the upload\n";
    lab.put_file(&unit.name, "/workspace", "authored.txt", original)
        .await;

    // Provisioning normally publishes this binding after the real connector enrolls.
    let mut conn = world.db.acquire().await.unwrap();
    sqlx::query("UPDATE runtimes SET machine_id=$2,state='running' WHERE id=$1")
        .bind(unit.runtime_id)
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET machine_id=$2,status='running',generation=1,remote_write=true WHERE id=$1")
        .bind(unit.environment_id).bind(machine).execute(&mut *conn).await.unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id: unit.environment_id,
        },
        world.user,
        NewAccess::Defaults,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let token = new_token(ACCESS_TOKEN_PREFIX);
    orch_db::queries::identity::testing::insert_session(
        &mut conn,
        new_id(),
        world.user,
        &token_hash(&token),
        world.clock.now() + chrono::Duration::days(1),
    )
    .await
    .unwrap();
    drop(conn);
    world
        .api
        .link
        .dispatcher()
        .run(
            machine,
            &SerialKey::Environment(unit.environment_id),
            &EnvAttach {
                resources: None,
                data_generation: Some(1),
                storage: None,
                data_sources: None,
                environment_id: unit.environment_id,
                workspace_root: None,
                ignore: Vec::new(),
                index: IndexMode::Full,
                mounts: Some(Vec::new()),
                variables: Default::default(),
                secrets: Default::default(),
            },
        )
        .await
        .unwrap();
    eventually("the physical machine's write policy", || async {
        let mut conn = world.db.acquire().await.unwrap();
        !orch_db::queries::environment_facts::refuses(
            &mut conn,
            unit.environment_id,
            "environment.remote_write",
        )
        .await
        .unwrap()
    })
    .await;
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::over(
        world.api.app.clone(),
        orch_types::ManualClock::new(world.clock.now()),
        registry,
    );
    world.api.app.engine.start();

    // More than two maximum link pieces forces an actual partial file before the held next range.
    let length = usize::try_from(orch_link_contract::content::MAX_CHUNK_BYTES * 2 + 17).unwrap();
    let bytes = vec![b'x'; length];
    let blob = uploaded(&app, &token, &bytes).await;
    let before = pin_count(&world, blob).await;
    assert_eq!(
        before, 0,
        "upload authority is separate from durable content pins"
    );
    assert!(
        orch_db::queries::uploads::owns(&mut world.db.acquire().await.unwrap(), world.user, &blob)
            .await
            .unwrap(),
        "the private upload owns its content"
    );
    world.api.download.hold(blob);
    let id = new_id();
    let job: Job = app
        .call(EnvironmentUploadWrite, &[&unit.environment_id, &id])
        .token(&token)
        .query(&ThroughThreadQuery::default())
        .body(&EnvironmentUploadWriteBody {
            path: "authored.txt".into(),
            blob,
            expected_hash: Some(Hash::of(original.as_bytes()).to_string()),
            upload_batch: new_id(),
        })
        .send()
        .await
        .ok();
    assert_eq!(job.id, id);
    world.api.download.entered().await;
    let staged = staged_files(&lab, &unit.name).await;
    assert_eq!(
        staged.len(),
        1,
        "a real native writer has its partial file: {staged:?}"
    );
    assert!(staged[0].1 > 0 && staged[0].1 < length as u64);
    assert_eq!(
        lab.read_file(&unit.name, "/workspace/authored.txt")
            .await
            .as_deref(),
        Some(original)
    );
    eventually("the command-bound partial write progress", || async {
        let current = read_job(&app, &token, id).await;
        current.state == JobState::Running
            && current.progress.percent < 100
            && current.progress.bytes_done > 0
            && current.progress.bytes_done < length as i64
    })
    .await;
    assert_eq!(pin_count(&world, blob).await, before + 1);
    let current = orch_db::queries::jobs::job(&mut world.db.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap();
    let command: Id = serde_json::from_value(current.plan["active_command"]["id"].clone()).unwrap();
    assert_eq!(
        current.plan["active_command"]["machine"],
        serde_json::json!(machine)
    );
    let stopping = app.call(JobCancel, &[&id]).token(&token).send().await.ok();
    assert_eq!(stopping.state, JobState::Running);
    assert!(stopping.progress.activity.unwrap().stopping);
    eventually(
        "the real connector's stop receipt and terminal job",
        || async { read_job(&app, &token, id).await.state == JobState::Cancelled },
    )
    .await;
    let acknowledged: bool =
        sqlx::query_scalar("SELECT stop_confirmed_at IS NOT NULL FROM commands WHERE id=$1")
            .bind(command)
            .fetch_one(&world.test_db.pool)
            .await
            .unwrap();
    assert!(
        acknowledged,
        "Cancelled is downstream of the authenticated native result"
    );
    assert!(staged_files(&lab, &unit.name).await.is_empty());
    assert_eq!(pin_count(&world, blob).await, before);
    let receipt = app
        .call(EnvironmentUploadWriteResult, &[&unit.environment_id, &id])
        .token(&token)
        .query(&ThroughThreadQuery::default())
        .send()
        .await;
    assert_eq!(receipt.status, axum::http::StatusCode::CONFLICT);

    // A source reply released after the stopped result cannot resume a writer or replace its target.
    world.api.download.release();
    world
        .api
        .link
        .dispatcher()
        .run(
            machine,
            &SerialKey::Environment(unit.environment_id),
            &FsStat {
                environment_id: unit.environment_id,
                mount: None,
                path: "authored.txt".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        lab.read_file(&unit.name, "/workspace/authored.txt")
            .await
            .as_deref(),
        Some(original)
    );
    assert!(staged_files(&lab, &unit.name).await.is_empty());
    let writes: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE environment_id=$1 AND action='workspace.file_written'")
        .bind(unit.environment_id).fetch_one(&world.test_db.pool).await.unwrap();
    assert_eq!(
        writes, 0,
        "an incomplete native file is never recorded as a completed write"
    );
    world.api.app.engine.stop();
    fleet.stop(&unit).await.unwrap();
    node.stop().await;
    world.stop().await;
}
