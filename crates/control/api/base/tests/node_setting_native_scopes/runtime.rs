//! Actual inherited resource application, durable queue waits and private original-operation proof.
use super::*;
use async_trait::async_trait;
use orch_api_core::testing::Machine;
use orch_link_contract::commands::{Cmd, CommandError, CommandKind};
use std::sync::Arc;

struct Host;
#[async_trait]
impl Machine for Host {
    async fn run(&self, command: &Cmd) -> Result<Json, CommandError> {
        assert_eq!(command.kind, "node.resources.apply");
        assert_eq!(command.body["resources"]["memory_mb"], 512);
        Ok(json!({}))
    }
}

struct Runtime {
    environment: Id,
    runtime: Id,
    machine: Id,
}
async fn runtime(scene: &Scene) -> Runtime {
    let mut conn = scene.world.conn().await;
    let host = resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        Some(scene.owner.id()),
        Some(scene.org),
        "Native setting host",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1")
        .bind(host).bind(json!({"test":{"name":"test","title":"Test","lifecycle":"unit","live_changes":{"resources":true,"mounts":false,"capabilities":[],"gateway_given":[]}}}))
        .execute(&mut *conn).await.unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let environment = resources::access_fixtures::insert_environment(
        &mut conn,
        scene.owner.id(),
        Some(scene.node),
        "Native setting unit",
        "managed",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
        scene.owner.id(),
        NewAccess::Defaults,
        Time::new(scene.world.now()),
    )
    .await
    .unwrap();
    let runtime = new_id();
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,state) SELECT $1,'environment',$2,'native-setting-unit',e.id,'test',e.template_id,'\\x00'::bytea,'running' FROM environments e WHERE e.id=$3")
        .bind(runtime).bind(host).bind(environment).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE environments SET status='running' WHERE id=$1")
        .bind(environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO commands(id,machine_id,kind,body,state,result,deadline_at) VALUES($1,$2,'node.runtime.create',$3,'succeeded','{}',$4)")
        .bind(new_id()).bind(machine).bind(json!({"runtime_id":runtime,"spec":{"resources":{"memory_mb":256}},"resources":{"memory_mb":256}})).bind(scene.world.now()+chrono::Duration::hours(1)).execute(&mut *conn).await.unwrap();
    Runtime {
        environment,
        runtime,
        machine,
    }
}

#[tokio::test]
async fn accepted_setting_or_clear_retains_its_exact_origin_through_the_actual_command_queue_wait()
{
    for clear in [false, true] {
        for mode in ["unchanged", "expired", "scope-lost", "home-changed"] {
            let scene = Scene::new().await;
            scene
                .world
                .set_at(
                    orch_settings::Target::Node(scene.org),
                    keys::ENV_RESOURCES,
                    json!({"memory_mb":512}),
                )
                .await;
            if clear {
                scene
                    .world
                    .set_at(
                        orch_settings::Target::Node(scene.node),
                        keys::ENV_RESOURCES,
                        json!({"memory_mb":128}),
                    )
                    .await;
            }
            let target = runtime(&scene).await;
            let other =
                places::make_project(&scene.world, &scene.owner, scene.org, "Another native home")
                    .await
                    .base
                    .id;
            let operation = if clear {
                "NodeSettingDelete"
            } else {
                "NodeSettingSet"
            };
            let secret = key(&scene.world, scene.owner.id(), scene.manage(operation)).await;
            let host = scene.world.app.play(target.machine, Arc::new(Host));
            let mut lock = scene.world.db.pool.begin().await.unwrap();
            let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *lock)
                .await
                .unwrap();
            // Ordinary planner reads proceed; the real parent FOR UPDATE scan has to wait.
            sqlx::query("LOCK TABLE jobs IN EXCLUSIVE MODE")
                .execute(&mut *lock)
                .await
                .unwrap();
            let app = scene.world.app.clone();
            let node = scene.node;
            let token = secret.secret;
            let request = tokio::spawn(async move {
                if clear {
                    let call = app
                        .call(NodeSettingDelete, &[&node, &keys::ENV_RESOURCES])
                        .token(&token)
                        .query(&NodeSettingDeleteQuery::default());
                    call.send_bytes().await
                } else {
                    let call = app
                        .call(NodeSettingSet, &[&node, &keys::ENV_RESOURCES])
                        .token(&token)
                        .body(&NodeSettingSetBody {
                            value: json!({"memory_mb":512}),
                            locked: None,
                            expected_revision: None,
                        });
                    call.send_bytes().await
                }
            });
            tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{
                assert!(!request.is_finished(),"the native runtime command must reach its actual parent queue wait");
                let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%FROM jobs WHERE id=$2 OR%' AND query LIKE '%FOR UPDATE%')")
                    .bind(blocker).fetch_one(&scene.world.db.pool).await.unwrap();
                if waiting {break;}tokio::task::yield_now().await;
            }}).await.expect("the actual native queue parent lock must be observed");
            let setting = settings::row(
                &mut *scene.world.conn().await,
                Some(scene.node),
                keys::ENV_RESOURCES,
            )
            .await
            .unwrap();
            assert_eq!(
                setting.is_none(),
                clear,
                "the original setting effect already committed once"
            );
            match mode {
                "expired" => scene.world.advance(chrono::Duration::hours(2)),
                "scope-lost" => {
                    let policy: KeyScope = serde_json::from_value(
                        json!({"rules":[rule(operation,"read","project",scene.node)]}),
                    )
                    .unwrap();
                    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                        .bind(secret.id)
                        .bind(json!(policy))
                        .execute(&mut *lock)
                        .await
                        .unwrap();
                }
                "home-changed" => {
                    sqlx::query("UPDATE environments SET home_node_id=$2 WHERE id=$1")
                        .bind(target.environment)
                        .bind(other)
                        .execute(&mut *lock)
                        .await
                        .unwrap();
                }
                _ => {}
            }
            lock.commit().await.unwrap();
            let body = checked(
                tokio::time::timeout(std::time::Duration::from_secs(30), request)
                    .await
                    .unwrap()
                    .unwrap(),
            );
            assert_eq!(body["projection"], "receipt");
            assert_eq!(body["key"], keys::ENV_RESOURCES);
            assert_eq!(
                settings::row(
                    &mut *scene.world.conn().await,
                    Some(scene.node),
                    keys::ENV_RESOURCES
                )
                .await
                .unwrap(),
                setting,
                "no accepted edit or clear is replayed"
            );
            let commands:Vec<(Id,Json,Option<Json>)>=sqlx::query_as("SELECT id,body,login_authority FROM commands WHERE kind='node.resources.apply' AND body->>'runtime_id'=$1::text")
                .bind(target.runtime).fetch_all(&scene.world.db.pool).await.unwrap();
            if mode == "unchanged" {
                assert_eq!(commands.len(), 1);
                assert_eq!(host.seen_of(CommandKind::NodeResourcesApply).len(), 1);
                let evidence = commands[0]
                    .2
                    .as_ref()
                    .expect("new setting commands retain their actual private origin");
                assert_eq!(evidence["kind"], "setting_apply");
                assert_eq!(evidence["authority"]["origin"]["operation"], operation);
                assert!(
                    commands[0].1.get("origin").is_none(),
                    "authority is never a connector body member"
                );
                let saved =
                    orch_db::queries::link::command(&mut *scene.world.conn().await, commands[0].0)
                        .await
                        .unwrap()
                        .unwrap();
                let evidence: orch_engine::commands::CommandAuthority =
                    serde_json::from_value(evidence.clone()).unwrap();
                assert!(evidence.requires_issue_checkpoint());
                let mut tx = scene.world.db.pool.begin().await.unwrap();
                assert!(
                    evidence
                        .current_callers(
                            &mut tx,
                            &saved,
                            scene.world.app.app.clock.as_ref(),
                            &orch_api_core::thread_access::AuthzAccess::new(
                                scene.world.app.app.clock.clone()
                            )
                        )
                        .await
                        .unwrap()
                        .is_some()
                );
                tx.rollback().await.unwrap();
            } else {
                assert!(
                    commands.is_empty(),
                    "current original setting authority closes before the first queue INSERT"
                );
                assert!(host.seen_of(CommandKind::NodeResourcesApply).is_empty());
            }
        }
    }
}
