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
        let full = [
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
            plan:&json!({"mount_id":mount,"copy":{"review":review,"by":actor},"private_marker":"original native recipe","active_command":{"machine":target}}),
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
    assert!(
        !wire.contains("Copy destination"),
        "reading a copy job does not disclose a machine without its own read"
    );
    let named = key(
        &world,
        people.boss.id(),
        vec![
            rule("MountCopyLatest", "mount", mount, &["read"]),
            rule("MountCopyLatest", "job", own.unwrap(), &["read"]),
            rule("MountCopyLatest", "machine", target, &["read"]),
        ],
    )
    .await;
    let named_job = world
        .app
        .call(MountCopyLatest, &[&mount])
        .token(&named)
        .send()
        .await
        .ok()
        .unwrap();
    assert!(
        serde_json::to_string(&named_job)
            .unwrap()
            .contains("Copy destination")
    );
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

#[tokio::test]
async fn a_mount_copy_review_uses_its_homes_configured_time_instead_of_a_fixed_quarter_hour() {
    let world = World::new().await;
    let (people, mount, source, target) = scene(&world).await;
    let peer = answer_plans(&world).await;
    for seconds in [30_i64, 10_800] {
        sqlx::query("INSERT INTO settings(node_id,key,value) VALUES($1,'mount.copy_review_secs',$2) ON CONFLICT(node_id,key) DO UPDATE SET value=excluded.value")
            .bind(people.acme).bind(json!(seconds)).execute(&world.db.pool).await.unwrap();
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
        assert_eq!(
            plan.expires_at.get(),
            world.now() + chrono::Duration::seconds(seconds)
        );
    }
    peer.abort();
}

async fn review_copy(
    world: &World,
    people: &resources::Company,
    mount: Id,
    source: Id,
    target: Id,
) -> MountCopyPlan {
    let peer = answer_plans(world).await;
    world.app.app.engine.start();
    let plan = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
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
            if plan.copy.as_ref().is_some_and(|copy| copy.measured) {
                break plan;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await;
    world.app.app.engine.stop();
    peer.abort();
    let _ = peer.await;
    let plan = plan.expect("the native count must finish before confirming a copy");
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let measured = plan.copy.as_ref().unwrap();
    assert_eq!((measured.file_count, measured.total_bytes), (2, 4096));
    plan
}
fn accepted_copy(value: orch_api_contract::workspace::JobCreationResponse) -> Id {
    match value {
        orch_api_contract::workspace::JobCreationResponse::Full(job) => job.id,
        orch_api_contract::workspace::JobCreationResponse::Receipt(receipt) => receipt.job_id,
    }
}

#[tokio::test]
async fn a_reviewed_copy_is_one_durable_job_even_when_confirmation_is_repeated() {
    let world = World::new().await;
    let (people, mount, source, target) = scene(&world).await;
    let plan = review_copy(&world, &people, mount, source, target).await;
    let body = MountCopyCreateBody {
        preflight_id: plan.preflight_id,
        notify: Some(false),
    };
    let mut accepted = Vec::new();
    for _ in 0..2 {
        accepted.push(accepted_copy(
            world
                .app
                .call(MountCopyCreate, &[&mount])
                .token(&people.boss.token)
                .body(&body)
                .send()
                .await
                .ok(),
        ));
    }
    assert_eq!(accepted[0], accepted[1]);
    let row = orch_db::queries::jobs::job(&mut *world.conn().await, accepted[0])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, "mount_copy");
    assert_eq!(row.created_by, Some(people.boss.id()));
    assert_eq!(row.plan["mount_id"], json!(mount));
    assert_eq!(row.plan["target_machine_id"], json!(target));
    assert_eq!(row.plan["notification_operation"], json!(plan.preflight_id));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='mount_copy'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let locations =
        orch_db::queries::resources::mounts::locations(&mut *world.conn().await, &[mount])
            .await
            .unwrap();
    assert_eq!(
        locations.len(),
        1,
        "queueing cannot publish a location before its actual native copy"
    );
}

#[tokio::test]
async fn copy_confirmation_needs_its_own_mount_and_both_machine_grants() {
    let world = World::new().await;
    let (people, mount, source, target) = scene(&world).await;
    let plan = review_copy(&world, &people, mount, source, target).await;
    for operation in ["MountCopyCreate", "MountCopyPreflight", "MountDelete"] {
        let full = [
            rule(operation, "mount", mount, &["write"]),
            rule(operation, "machine", source, &["manage"]),
            rule(operation, "machine", target, &["manage"]),
        ];
        let omissions = if operation == "MountCopyCreate" {
            vec![Some(0), Some(1), Some(2)]
        } else {
            vec![None]
        };
        for omitted in omissions {
            let token = key(
                &world,
                people.boss.id(),
                full.iter()
                    .enumerate()
                    .filter(|(i, _)| Some(*i) != omitted)
                    .map(|(_, r)| r.clone())
                    .collect(),
            )
            .await;
            let reply = world
                .app
                .call(MountCopyCreate, &[&mount])
                .token(&token)
                .body(&MountCopyCreateBody {
                    preflight_id: plan.preflight_id,
                    notify: Some(false),
                })
                .send_bytes()
                .await;
            assert_eq!(reply.status, http::StatusCode::FORBIDDEN);
        }
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='mount_copy'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn recreating_a_reviewed_location_invalidates_confirmation_before_any_copy_mutation() {
    let world = World::new().await;
    let (people, mount, source, target) = scene(&world).await;
    let plan = review_copy(&world, &people, mount, source, target).await;
    let mut conn = world.conn().await;
    let original = orch_db::queries::resources::mounts::locations(&mut conn, &[mount])
        .await
        .unwrap()
        .pop()
        .unwrap();
    orch_db::queries::resources::mounts::remove_location(&mut conn, mount, source)
        .await
        .unwrap();
    orch_db::queries::resources::mounts::set_location(&mut conn, &original)
        .await
        .unwrap();
    drop(conn);
    let reply = world
        .app
        .call(MountCopyCreate, &[&mount])
        .token(&people.boss.token)
        .body(&MountCopyCreateBody {
            preflight_id: plan.preflight_id,
            notify: Some(false),
        })
        .send_bytes()
        .await;
    assert_eq!(reply.status, http::StatusCode::CONFLICT);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='mount_copy'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let writes:i64=sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind IN ('node.volume.ensure','node.mount.export','node.mount.import')")
        .fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(writes, 0);
}

#[tokio::test]
async fn a_copy_uses_the_same_current_mount_for_ordinary_job_progress_and_cancellation() {
    use orch_api_contract::workspace::{JobCancel, JobGet};
    let world = World::new().await;
    let (people, mount, source, target) = scene(&world).await;
    let plan = review_copy(&world, &people, mount, source, target).await;
    let job = accepted_copy(
        world
            .app
            .call(MountCopyCreate, &[&mount])
            .token(&people.boss.token)
            .body(&MountCopyCreateBody {
                preflight_id: plan.preflight_id,
                notify: Some(false),
            })
            .send()
            .await
            .ok(),
    );
    let shown = world
        .app
        .call(JobGet, &[&job])
        .token(&people.boss.token)
        .send()
        .await
        .ok();
    assert_eq!(
        shown.id, job,
        "the common progress reader must follow this actual Copy job"
    );
    for (operation, action) in [
        ("JobGet.ordinary", "read"),
        ("JobCancel.lifecycle", "manage"),
    ] {
        let missing = key(
            &world,
            people.boss.id(),
            vec![rule(operation, "job", job, &[action])],
        )
        .await;
        let status = if action == "read" {
            world
                .app
                .call(JobGet, &[&job])
                .token(&missing)
                .send_bytes()
                .await
                .status
        } else {
            world
                .app
                .call(JobCancel, &[&job])
                .token(&missing)
                .send_bytes()
                .await
                .status
        };
        assert_eq!(
            status,
            http::StatusCode::FORBIDDEN,
            "the Job identity does not replace its mount source"
        );
        let allowed = key(
            &world,
            people.boss.id(),
            vec![
                rule(operation, "job", job, &[action]),
                rule(operation, "mount", mount, &[action]),
            ],
        )
        .await;
        let status = if action == "read" {
            world
                .app
                .call(JobGet, &[&job])
                .token(&allowed)
                .send_bytes()
                .await
                .status
        } else {
            world
                .app
                .call(JobCancel, &[&job])
                .token(&allowed)
                .send_bytes()
                .await
                .status
        };
        assert_eq!(
            status,
            http::StatusCode::OK,
            "the common Job act keeps its own declared native mount"
        );
    }
    let row = orch_db::queries::jobs::job(&mut *world.conn().await, job)
        .await
        .unwrap()
        .unwrap();
    assert!(row.cancel_requested());
}
