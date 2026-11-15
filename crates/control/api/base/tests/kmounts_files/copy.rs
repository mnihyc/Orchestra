//! A standalone data copy keeps its original mount and both physical machine operands.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{identity, resources::access_fixtures as fixtures};
use orch_types::{Json, new_id};

fn rule(operation: &str, kind: &str, id: Id, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":actions}]})
}

async fn key(world: &World, actor: Id, rules: Vec<Json>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: actor,
            name: "Reviewed mount data copy",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    token
}

async fn scene(world: &World) -> (resources::Company, Id, Id, Id) {
    let people = company(world).await;
    let mount = named_mount(world, &people.boss, people.acme, "copy-data")
        .await
        .base
        .id;
    let mut conn = world.conn().await;
    let mut machines = Vec::new();
    for name in ["Copy source", "Copy destination"] {
        let machine = fixtures::insert_persons_machine(&mut conn, people.boss.id(), name)
            .await
            .unwrap();
        sqlx::query("UPDATE machines SET online=true,facts=$2 WHERE id=$1")
            .bind(machine).bind(json!({"policy":{"environment.roots":["/srv"]},"environment_descriptor":{"name":"directory","lifecycle":"attach","data_storage":{"made_by_orch":"directory","existing_directory":true,"reason":null}},"storages":[]}))
            .execute(&mut *conn).await.unwrap();
        machines.push(machine);
    }
    orch_db::queries::resources::mounts::set_location(
        &mut conn,
        &orch_db::queries::resources::mounts::LocationRow {
            mount_id: mount,
            machine_id: machines[0],
            host_id: None,
            host_name: None,
            kind: "path".into(),
            source: "/srv/private-data".into(),
            storage: None,
            data_kind: None,
            created_by: people.boss.id(),
            created_at: world.now(),
        },
    )
    .await
    .unwrap();
    (people, mount, machines[0], machines[1])
}

#[tokio::test]
async fn a_copy_needs_its_own_operation_on_the_mount_and_both_machines_before_any_native_work() {
    let world = World::new().await;
    let (people, mount, source, target) = scene(&world).await;
    for operation in ["MountCopyPreflight", "MountLocationSet", "MountDelete"] {
        let full = vec![
            rule(operation, "mount", mount, &["write"]),
            rule(operation, "machine", source, &["manage"]),
            rule(operation, "machine", target, &["manage"]),
        ];
        let omissions = if operation == "MountCopyPreflight" {
            vec![Some(0), Some(1), Some(2)]
        } else {
            vec![None]
        };
        for omitted in omissions {
            let rules = full
                .iter()
                .enumerate()
                .filter(|(index, _)| Some(*index) != omitted)
                .map(|(_, rule)| rule.clone())
                .collect();
            let token = key(&world, people.boss.id(), rules).await;
            let reply = world
                .app
                .call(MountCopyPreflight, &[&mount])
                .token(&token)
                .body(&MountCopyPreflightBody {
                    source_machine_id: source,
                    target_machine_id: target,
                    into: None,
                })
                .send_bytes()
                .await;
            assert_eq!(
                reply.status,
                http::StatusCode::FORBIDDEN,
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
        }
    }
    let native: i64 = sqlx::query_scalar("SELECT count(*) FROM commands")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let copies: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind IN ('mount_copy','measurement')")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(native, 0);
    assert_eq!(copies, 0);
}

#[tokio::test]
async fn a_copy_rejects_caller_paths_and_volumes_instead_of_looking_up_a_different_source() {
    let world = World::new().await;
    let (people, mount, source, target) = scene(&world).await;
    let reply = world.app.call(MountCopyPreflight, &[&mount]).token(&people.boss.token)
        .raw_body(&json!({"source_machine_id":source,"target_machine_id":target,"source":"/srv/other","volume":"other-data"}))
        .send_bytes().await;
    assert_eq!(reply.status, http::StatusCode::BAD_REQUEST);
    let copies: i64 = sqlx::query_scalar("SELECT count(*) FROM preflights WHERE kind='mount_copy'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(copies, 0);
}

async fn answer_plans(world: &World) -> tokio::task::JoinHandle<()> {
    let pool = world.db.pool.clone();
    tokio::spawn(async move {
        loop {
            for (kind, result) in [
                (
                    "node.volume.carry-check",
                    json!({"available":true,"reason":null}),
                ),
                (
                    "node.mount.measure",
                    json!({"file_count":2,"total_bytes":4096}),
                ),
            ] {
                if sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE kind=$1 AND state IN ('queued','sent','accepted')")
                    .bind(kind).bind(result).execute(&pool).await.is_err() { return; }
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
}

#[tokio::test]
async fn a_copy_review_keeps_native_locations_private_without_its_own_read_and_queues_no_copy() {
    let world = World::new().await;
    let (people, mount, source, target) = scene(&world).await;
    let peer = answer_plans(&world).await;
    let token = key(
        &world,
        people.boss.id(),
        vec![
            rule("MountCopyPreflight", "mount", mount, &["write"]),
            rule("MountCopyPreflight", "machine", source, &["manage"]),
            rule("MountCopyPreflight", "machine", target, &["manage"]),
            rule("MountGet", "mount", mount, &["read"]),
        ],
    )
    .await;
    let reply = world
        .app
        .call(MountCopyPreflight, &[&mount])
        .token(&token)
        .body(&MountCopyPreflightBody {
            source_machine_id: source,
            target_machine_id: target,
            into: None,
        })
        .send_bytes()
        .await;
    peer.abort();
    assert_eq!(
        reply.status,
        http::StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(value["projection"], "receipt");
    assert_eq!(value["mount_id"], json!(mount));
    assert!(
        value.get("copy").is_none()
            && value.get("source").is_none()
            && value.get("notification").is_none()
    );
    let id: Id = serde_json::from_value(value["preflight_id"].clone()).unwrap();
    let row = orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, "mount_copy");
    assert_eq!(row.created_by, people.boss.id());
    let copied: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='mount_copy'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let mutations: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind IN ('node.mount.export','node.mount.import','node.volume.ensure')").fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(copied, 0);
    assert_eq!(mutations, 0);
}

#[tokio::test]
async fn a_mounts_manager_can_review_the_actual_source_and_destination_without_editing_the_mount() {
    let world = World::new().await;
    let (people, mount, source, target) = scene(&world).await;
    let peer = answer_plans(&world).await;
    let plan = world
        .app
        .call(MountCopyPreflight, &[&mount])
        .token(&people.boss.token)
        .body(&MountCopyPreflightBody {
            source_machine_id: source,
            target_machine_id: target,
            into: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    peer.abort();
    assert_eq!(plan.mount.id, mount);
    assert_eq!(plan.source.id, source);
    assert_eq!(plan.target.id, target);
    assert!(plan.copy.as_ref().is_some_and(|copy| copy.location.made));
    let locations =
        orch_db::queries::resources::mounts::locations(&mut *world.conn().await, &[mount])
            .await
            .unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].source, "/srv/private-data");
    assert_eq!(locations[0].machine_id, source);
}

#[tokio::test]
async fn reopening_a_copy_reads_only_this_persons_latest_job_and_never_exposes_its_private_recipe()
{
    let world = World::new().await;
    let (people, mount, source, target) = scene(&world).await;
    assert!(
        world
            .app
            .call(MountCopyLatest, &[&mount])
            .token(&people.boss.token)
            .send()
            .await
            .ok()
            .is_none()
    );
    let mut conn = world.conn().await;
    let review =
        orch_engine::mount_copies::location::Review::capture(&mut conn, mount, source, target)
            .await
            .unwrap();
    let mut own = None;
    for actor in [people.boss.id(), people.stranger.id()] {
        let row = orch_db::queries::jobs::insert(&mut conn,&orch_db::queries::jobs::NewJob {
            id:new_id(),kind:"mount_copy",node_id:None,environment_id:None,provider_id:None,
            plan:&json!({"mount_id":mount,"copy":{"review":review,"by":actor},"private_marker":"original native recipe"}),
            progress:&serde_json::to_value(orch_engine::jobs::runner::initial_progress(&[])).unwrap(),created_by:Some(actor),
        },world.now()).await.unwrap();
        if actor == people.boss.id() {
            own = Some(row.id);
        }
    }
    drop(conn);
    let only_mount = key(
        &world,
        people.boss.id(),
        vec![rule("MountCopyLatest", "mount", mount, &["read"])],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(MountCopyLatest, &[&mount])
            .token(&only_mount)
            .send_bytes()
            .await
            .status,
        http::StatusCode::FORBIDDEN
    );
    let read = key(
        &world,
        people.boss.id(),
        vec![
            rule("MountCopyLatest", "mount", mount, &["read"]),
            rule("MountCopyLatest", "job", own.unwrap(), &["read"]),
        ],
    )
    .await;
    let job = world
        .app
        .call(MountCopyLatest, &[&mount])
        .token(&read)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(Some(job.id), own);
    let wire = serde_json::to_string(&job).unwrap();
    assert!(!wire.contains("original native recipe") && !wire.contains("/srv/private-data"));
    let wrong = key(
        &world,
        people.boss.id(),
        vec![rule("MountGet", "mount", mount, &["read"])],
    )
    .await;
    assert_eq!(
        world
            .app
            .call(MountCopyLatest, &[&mount])
            .token(&wrong)
            .send_bytes()
            .await
            .status,
        http::StatusCode::FORBIDDEN
    );
}
