//! Real native Environment creation, independent copies and original queued source authority.
mod places;
mod resources;
mod support;
mod thread_history_scope;

use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
use orch_api_contract::{
    binding::{NewEnvironment, NewEnvironmentAccess},
    duplicates::{DuplicateDestination, EnvironmentDuplicate, EnvironmentDuplicateBody},
    environment::EnvironmentCreate,
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::{queries::resources::access_fixtures as fixtures, secrets::MasterKey};
use orch_types::{Id, Json, Time};
use serde_json::json;
use support::World;
use thread_history_scope::{Proof, key, ok, rule};

const OPERATION: &str = "EnvironmentDuplicate.managed";
const SECRET: &str = "synthetic-workspace-copy-credential";
struct Scene {
    world: World,
    company: resources::Company,
    source: Id,
    host: Id,
    template: Id,
}
impl Scene {
    async fn new() -> Proof<Self> {
        orch_api_core::routes::resources::sealing::install_master_key(MasterKey::new([7; 32]));
        let world = World::new().await;
        world.app.app.engine.stop();
        let company = resources::company(&world).await;
        let mut conn = world.conn().await;
        let host = fixtures::insert_resource(
            &mut conn,
            "host",
            Some(company.boss.id()),
            Some(company.acme),
            "Copy host",
        )
        .await?;
        let template = fixtures::insert_resource(
            &mut conn,
            "template",
            Some(company.boss.id()),
            Some(company.acme),
            "Copy template",
        )
        .await?;
        for (kind, id) in [
            (ResourceKind::Host, host),
            (ResourceKind::Template, template),
        ] {
            start_resource(
                &mut conn,
                ObjectRef {
                    kind: kind.into(),
                    id,
                },
                company.boss.id(),
                NewAccess::Defaults,
                Time::new(world.now()),
            )
            .await?;
        }
        sqlx::query("UPDATE hosts SET drivers=$2,capacity=$3,storages=$4 WHERE id=$1")
            .bind(host)
            .bind(json!({"docker":{"name":"docker","title":"Docker","spec_schemas":{"environment":{},"provider":{}},"runtime_classes":["runc"],"sources":["image"]}}))
            .bind(json!({"cpus":8,"memory_mb":16384,"disk_gb":500}))
            .bind(json!([{"name":"fast","default":true,"capacity_gb":500,"free_gb":300,"private_views":false}]))
            .execute(&mut *conn).await?;
        sqlx::query("UPDATE machines SET online=true,facts=jsonb_set(facts,'{policy}',COALESCE(facts->'policy','{}')||$2) WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)")
            .bind(host).bind(json!({"drivers":orch_link_contract::policy::MachineDrivers::managed()})).execute(&mut *conn).await?;
        sqlx::query("UPDATE env_templates SET driver='docker',spec=$2 WHERE id=$1")
            .bind(template).bind(json!({"image":"busybox:latest","runtime":"runc","user":{"name":"agent","uid":1000,"gid":1000},"resources":{"cpu_millis":2000,"memory_mb":2048,"pids":512,"disk_gb":20},"needs":{"cpu_millis":500,"memory_mb":1024,"disk_gb":5},"exec":{"offers":["orch-exec"],"orch_tools":true},"colocated_harnesses":[],"host_labels":[],"init":null}))
            .execute(&mut *conn).await?;
        drop(conn);
        let source = world
            .app
            .call(EnvironmentCreate, &[])
            .token(&company.boss.token)
            .body(&NewEnvironment {
                name: Some("Source workspace".into()),
                home_node_id: Some(company.acme),
                template_id: Some(template),
                host_id: Some(Some(host)),
                config: Some(json!({"env.variables":{"MODE":"copied"}})),
                access: Some(NewEnvironmentAccess::OnlyMe),
                ..Default::default()
            })
            .send()
            .await
            .ok()
            .into_full()
            .map_err(|_| "source creation returned only a receipt")?
            .base
            .id;
        // A stored native workspace source. Worker delivery itself is exercised by
        // the engine's actual held snapshot/materialization tests, never a fabricated ACK here.
        sqlx::query("UPDATE environments SET status='running',machine_id=(SELECT machine_id FROM hosts WHERE id=$2),workspace_root='/fixture/source-workspace' WHERE id=$1")
            .bind(source).bind(host).execute(&world.db.pool).await?;
        let mut conn = world.conn().await;
        orch_settings::secrets::put(
            &mut conn,
            &MasterKey::new([7; 32]),
            &orch_settings::secrets::Put {
                actor: orch_db::audit::Actor::User(company.boss.id()),
                owner: orch_settings::secrets::Owner::Environment(source),
                name: "API_KEY",
                value: SECRET,
                origin: &orch_db::audit::Origin::default(),
            },
            world.now(),
        )
        .await?;
        drop(conn);
        Ok(Self {
            world,
            company,
            source,
            host,
            template,
        })
    }
    async fn duplicate(
        &self,
        token: &str,
        body: &EnvironmentDuplicateBody,
    ) -> orch_api_core::testing::RawReply {
        self.world
            .app
            .call(EnvironmentDuplicate, &[&self.source])
            .token(token)
            .body(body)
            .send_bytes()
            .await
    }
    fn rules(&self) -> Vec<Json> {
        vec![
            rule(OPERATION, "environment", self.source, "self", "read"),
            json!({"target":{"kind":"org","id":self.company.acme,"reach":"self"},"resources":["environment"],"allow":[{"operation":OPERATION,"actions":["write"]}]}),
            rule(OPERATION, "template", self.template, "self", "read"),
            rule(OPERATION, "host", self.host, "self", "write"),
        ]
    }
    async fn plan(&self, id: Id) -> Proof<Json> {
        Ok(sqlx::query_scalar(
            "SELECT plan FROM jobs WHERE environment_id=$1 AND kind='environment_provision'",
        )
        .bind(id)
        .fetch_one(&self.world.db.pool)
        .await?)
    }
    async fn count(&self) -> Proof<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM environments WHERE org_id=$1 AND deleted_at IS NULL",
        )
        .bind(self.company.acme)
        .fetch_one(&self.world.db.pool)
        .await?)
    }

    async fn connected_source() -> Proof<(Self, Id)> {
        use orch_api_contract::environment::{
            MachineEnvironmentCreate, MachineEnvironmentCreateBody,
        };
        let mut scene = Self::new().await?;
        let mut conn = scene.world.conn().await;
        let machine =
            fixtures::insert_persons_machine(&mut conn, scene.company.boss.id(), "Copy laptop")
                .await?;
        drop(conn);
        let source = scene
            .world
            .app
            .call(MachineEnvironmentCreate, &[&machine])
            .token(&scene.company.boss.token)
            .body(&MachineEnvironmentCreateBody {
                name: "Connected source".into(),
                home_id: Some(scene.company.acme),
                home_node_id: None,
                root: String::new(),
                data_sources: Some(Default::default()),
                remote_write: None,
                terminal: None,
                access: None,
            })
            .send()
            .await
            .ok()
            .into_full()
            .map_err(|_| "the native registration returned no environment")?
            .base
            .id;
        sqlx::query("UPDATE environments SET status='running',workspace_root='/fixture/connected-source' WHERE id=$1")
            .bind(source).execute(&scene.world.db.pool).await?;
        scene.source = source;
        Ok((scene, machine))
    }
    fn connected_rules(&self, machine: Id) -> Vec<Json> {
        let operation = "EnvironmentDuplicate.connected";
        vec![
            rule(operation, "environment", self.source, "self", "read"),
            json!({"target":{"kind":"org","id":self.company.acme,"reach":"self"},"resources":["environment"],"allow":[{"operation":operation,"actions":["write"]}]}),
            rule(operation, "machine", machine, "self", "write"),
        ]
    }
}

#[tokio::test]
async fn a_connected_duplicate_keeps_its_real_machine_and_never_invents_a_template_or_runtime()
-> Proof {
    let (s, machine) = Scene::connected_source().await?;
    let key = key(&s.world, s.company.boss.id(), s.connected_rules(machine)).await?;
    let copied: Id = serde_json::from_value(
        ok(&s
            .duplicate(&key.secret, &EnvironmentDuplicateBody::default())
            .await)?["id"]
            .clone(),
    )?;
    let (placement,recipient,template,root,status,config):(String,Option<Id>,Option<Id>,String,String,Json)=sqlx::query_as(
        "SELECT placement,machine_id,template_id,workspace_root,status,config FROM environments WHERE id=$1"
    ).bind(copied).fetch_one(&s.world.db.pool).await?;
    assert_eq!(placement, "connected");
    assert_eq!(recipient, Some(machine));
    assert_eq!(template, None);
    assert_eq!(root, "");
    assert_eq!(status, "provisioning");
    assert_eq!(
        config["env.data_sources"],
        json!(orch_driver_contract::runtime::EnvironmentDataSources::default())
    );
    let plan = s.plan(copied).await?;
    assert_eq!(plan["mode"], "copy");
    assert_eq!(plan["creator_provision"]["machine"], json!(machine));
    assert!(plan["creator_provision"]["template"].is_null());
    assert!(plan["runtime"].is_null());
    assert_eq!(plan["initial_files"]["environment"], json!(s.source));
    assert_eq!(plan["initial_files"]["target_environment"], json!(copied));
    let mut conn = s.world.conn().await;
    let id: Id = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE environment_id=$1 AND kind='environment_provision'",
    )
    .bind(copied)
    .fetch_one(&mut *conn)
    .await?;
    let row = orch_db::queries::jobs::job(&mut conn, id)
        .await?
        .ok_or("the actual connected provision disappeared")?;
    let ready = orch_engine::jobs::provision::creator_checkpoint(
        &mut conn,
        &row,
        s.world.app.app.clock.as_ref(),
        &orch_engine::access::Unconnected,
    )
    .await?
    .ok_or("the connected creator checkpoint was not retained")?;
    ready.check(s.world.app.app.clock.as_ref())?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key.id)
        .bind(s.world.now())
        .execute(&mut *conn)
        .await?;
    assert!(
        orch_engine::jobs::provision::creator_checkpoint(
            &mut conn,
            &row,
            s.world.app.app.clock.as_ref(),
            &orch_engine::access::Unconnected
        )
        .await
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn connected_copy_needs_its_actual_source_home_and_machine_and_requires_an_explicit_placement_change()
-> Proof {
    let (s, machine) = Scene::connected_source().await?;
    let initial = s.count().await?;
    for missing in 0..3 {
        let mut rules = s.connected_rules(machine);
        rules.remove(missing);
        let key = key(&s.world, s.company.boss.id(), rules).await?;
        assert_eq!(
            s.duplicate(&key.secret, &EnvironmentDuplicateBody::default())
                .await
                .status,
            http::StatusCode::FORBIDDEN
        );
        assert_eq!(s.count().await?, initial);
    }
    let other = fixtures::insert_persons_machine(
        &mut *s.world.conn().await,
        s.company.stranger.id(),
        "Unshared recipient",
    )
    .await?;
    let other_key = key(&s.world, s.company.boss.id(), s.connected_rules(other)).await?;
    let response = s
        .duplicate(
            &other_key.secret,
            &EnvironmentDuplicateBody {
                machine_id: Some(other),
                ..Default::default()
            },
        )
        .await;
    assert!(!response.status.is_success());
    assert_eq!(s.count().await?, initial);
    // A refusal for the default connected placement does not fall through to managed creation.
    let key = key(&s.world, s.company.boss.id(), s.rules()).await?;
    assert_eq!(
        s.duplicate(&key.secret, &EnvironmentDuplicateBody::default())
            .await
            .status,
        http::StatusCode::FORBIDDEN
    );
    assert_eq!(s.count().await?, initial);
    let body = EnvironmentDuplicateBody {
        managed: Some(orch_api_contract::duplicates::EnvironmentDuplicateManaged {
            template_id: Some(s.template),
            host_id: Some(s.host),
        }),
        ..Default::default()
    };
    let copied: Id =
        serde_json::from_value(ok(&s.duplicate(&key.secret, &body).await)?["id"].clone())?;
    let placement: String = sqlx::query_scalar("SELECT placement FROM environments WHERE id=$1")
        .bind(copied)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(placement, "managed");
    Ok(())
}

#[tokio::test]
async fn a_native_workspace_duplicate_owns_fresh_data_and_keeps_its_original_operation_and_http_identity()
-> Proof {
    let s = Scene::new().await?;
    let key = key(&s.world, s.company.boss.id(), s.rules()).await?;
    let copied: Id = serde_json::from_value(
        ok(&s
            .duplicate(&key.secret, &EnvironmentDuplicateBody::default())
            .await)?["id"]
            .clone(),
    )?;
    assert_ne!(copied, s.source);
    assert_eq!(s.count().await?, 2);
    let plan = s.plan(copied).await?;
    let origin: orch_authz::CapturedOperation =
        serde_json::from_value(plan["creator_provision"]["origin"].clone())?;
    assert_eq!(origin.operation(), OPERATION);
    assert_eq!(origin.user(), s.company.boss.id());
    let raw: orch_engine::operation_scope::http_origin::HttpOrigin =
        serde_json::from_value(plan["duplicate_http_origin"].clone())?;
    assert_eq!(raw.origin().user(), origin.user());
    assert_eq!(raw.origin().operation(), OPERATION);
    assert!(
        matches!(raw.credential(), orch_engine::operation_scope::request_origin::Credential::Token {id, ..} if *id == key.id)
    );
    assert_eq!(plan["initial_files"]["environment"], json!(s.source));
    assert!(plan["initial_files"]["thread"].is_null());
    assert_eq!(plan["initial_files"]["target_environment"], json!(copied));
    assert!(plan["duplicate_http_origin"].is_object());
    assert!(!serde_json::to_string(&plan)?.contains(SECRET));
    let (owner, placement, root, config, status): (Id, String, String, Json, String) =
        sqlx::query_as(
            "SELECT owner_id,placement,workspace_root,config,status FROM environments WHERE id=$1",
        )
        .bind(copied)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(owner, s.company.boss.id());
    assert_eq!(placement, "managed");
    assert_ne!(root, "/fixture/source-workspace");
    assert_eq!(status, "provisioning");
    assert_eq!(config["env.variables"]["MODE"], "copied");
    assert_eq!(
        config["env.data_sources"],
        json!(orch_driver_contract::runtime::EnvironmentDataSources::default())
    );
    let sealed:Vec<u8> = sqlx::query_scalar("SELECT sealed FROM secrets WHERE owner_kind='environment' AND owner_id=$1 AND name='API_KEY'")
        .bind(copied).fetch_one(&s.world.db.pool).await?;
    let master = MasterKey::new([7; 32]);
    assert_eq!(
        master.open(
            &sealed,
            &orch_api_core::routes::resources::sealing::binding("environment", copied, "API_KEY")
        )?,
        SECRET.as_bytes()
    );
    assert!(
        master
            .open(
                &sealed,
                &orch_api_core::routes::resources::sealing::binding(
                    "environment",
                    s.source,
                    "API_KEY"
                )
            )
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn each_actual_duplicate_source_and_destination_is_required_before_a_new_environment_or_job()
-> Proof {
    let s = Scene::new().await?;
    for index in 0..4 {
        let mut rules = s.rules();
        rules.remove(index);
        let key = key(&s.world, s.company.boss.id(), rules).await?;
        let reply = s
            .duplicate(&key.secret, &EnvironmentDuplicateBody::default())
            .await;
        assert_eq!(
            reply.status,
            http::StatusCode::FORBIDDEN,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        assert_eq!(s.count().await?, 1);
    }
    let mut wrong = s.rules();
    for value in &mut wrong {
        value["allow"][0]["operation"] = json!("EnvironmentCreate");
    }
    let key = key(&s.world, s.company.boss.id(), wrong).await?;
    assert_eq!(
        s.duplicate(&key.secret, &EnvironmentDuplicateBody::default())
            .await
            .status,
        http::StatusCode::FORBIDDEN
    );
    assert_eq!(s.count().await?, 1);
    Ok(())
}

#[tokio::test]
async fn the_pending_native_copy_rechecks_its_real_http_key_and_frozen_workspace_after_waits()
-> Proof {
    let s = Scene::new().await?;
    let key = key(&s.world, s.company.boss.id(), s.rules()).await?;
    let copied: Id = serde_json::from_value(
        ok(&s
            .duplicate(&key.secret, &EnvironmentDuplicateBody::default())
            .await)?["id"]
            .clone(),
    )?;
    let mut conn = s.world.conn().await;
    let job_id: Id = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE environment_id=$1 AND kind='environment_provision'",
    )
    .bind(copied)
    .fetch_one(&mut *conn)
    .await?;
    let row = orch_db::queries::jobs::job(&mut conn, job_id)
        .await?
        .ok_or("missing actual copy provision")?;
    let checked = orch_engine::jobs::provision::creator_checkpoint(
        &mut conn,
        &row,
        s.world.app.app.clock.as_ref(),
        &orch_engine::access::Unconnected,
    )
    .await?
    .ok_or("missing native creator checkpoint")?;
    checked.check(s.world.app.app.clock.as_ref())?;
    assert_eq!(checked.principal().user(), s.company.boss.id());
    sqlx::query("UPDATE environments SET workspace_root='/fixture/replaced-source' WHERE id=$1")
        .bind(s.source)
        .execute(&mut *conn)
        .await?;
    assert!(
        orch_engine::jobs::provision::creator_checkpoint(
            &mut conn,
            &row,
            s.world.app.app.clock.as_ref(),
            &orch_engine::access::Unconnected
        )
        .await
        .is_err()
    );
    sqlx::query("UPDATE environments SET workspace_root='/fixture/source-workspace' WHERE id=$1")
        .bind(s.source)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key.id)
        .bind(s.world.now())
        .execute(&mut *conn)
        .await?;
    assert!(
        orch_engine::jobs::provision::creator_checkpoint(
            &mut conn,
            &row,
            s.world.app.app.clock.as_ref(),
            &orch_engine::access::Unconnected
        )
        .await
        .is_err()
    );
    let commands: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE body->>'job_id'=$1")
            .bind(job_id.to_string())
            .fetch_one(&mut *conn)
            .await?;
    assert_eq!(commands, 0, "source checks alone enqueue no native IO");
    Ok(())
}

#[tokio::test]
async fn a_viewers_native_copy_starts_without_secrets_or_the_original_page_and_a_creation_key_replays_once()
-> Proof {
    let s = Scene::new().await?;
    let viewer = &s.company.viewer;
    for permission in ["environment.view", "files.view"] {
        resources::name_person(&s.world, "environment", s.source, permission, viewer.id()).await;
    }
    resources::name_person(
        &s.world,
        "node",
        s.company.acme,
        "org.interact",
        viewer.id(),
    )
    .await;
    resources::name_person(&s.world, "host", s.host, "host.use", viewer.id()).await;
    resources::name_person(
        &s.world,
        "template",
        s.template,
        "template.use",
        viewer.id(),
    )
    .await;
    let key = key(&s.world, viewer.id(), s.rules()).await?;
    let header = orch_types::new_id().to_string();
    let mut copied = None;
    for _ in 0..2 {
        let reply = s
            .world
            .app
            .call(EnvironmentDuplicate, &[&s.source])
            .token(&key.secret)
            .header("idempotency-key", &header)
            .body(&EnvironmentDuplicateBody::default())
            .send_bytes()
            .await;
        let id: Id = serde_json::from_value(ok(&reply)?["id"].clone())?;
        if let Some(old) = copied {
            assert_eq!(id, old);
        } else {
            copied = Some(id);
        }
    }
    let id = copied.ok_or("no created environment")?;
    let secrets: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM secrets WHERE owner_kind='environment' AND owner_id=$1",
    )
    .bind(id)
    .fetch_one(&s.world.db.pool)
    .await?;
    assert_eq!(secrets, 0);
    let owner: Id = sqlx::query_scalar("SELECT owner_id FROM environments WHERE id=$1")
        .bind(id)
        .fetch_one(&s.world.db.pool)
        .await?;
    assert_eq!(owner, viewer.id());
    assert_eq!(s.count().await?, 2);
    let origin: orch_authz::CapturedOperation =
        serde_json::from_value(s.plan(id).await?["creator_provision"]["origin"].clone())?;
    assert_eq!(origin.user(), viewer.id());
    let body = EnvironmentDuplicateBody {
        destination: Some(DuplicateDestination::Node { id: s.company.acme }),
        expected_revision: Some(-1),
        ..Default::default()
    };
    assert_eq!(
        s.duplicate(&key.secret, &body).await.status,
        http::StatusCode::CONFLICT
    );
    assert_eq!(s.count().await?, 2);
    Ok(())
}

#[tokio::test]
async fn both_native_environment_placements_copy_to_another_org_and_recheck_its_source_policy()
-> Proof {
    for connected in [false, true] {
        let (s, machine) = if connected {
            let (s, machine) = Scene::connected_source().await?;
            (s, Some(machine))
        } else {
            (Scene::new().await?, None)
        };
        let destination = places::make_org(
            &s.world,
            &s.company.boss,
            "Workspace recipient",
            "workspace-recipient",
        )
        .await
        .base
        .id;
        let mut rules = match machine {
            Some(machine) => s.connected_rules(machine),
            None => s.rules(),
        };
        rules[1]["target"]["id"] = json!(destination);
        let token = key(&s.world, s.company.boss.id(), rules).await?;
        let reply = s
            .duplicate(
                &token.secret,
                &EnvironmentDuplicateBody {
                    destination: Some(DuplicateDestination::Node { id: destination }),
                    ..Default::default()
                },
            )
            .await;
        let made: Id = serde_json::from_value(ok(&reply)?["id"].clone())?;
        let row: Json = sqlx::query_scalar("SELECT jsonb_build_object('home',home_node_id,'org',org_id,'owner',owner_id,'placement',placement,'template',template_id,'machine',machine_id) FROM environments WHERE id=$1")
            .bind(made).fetch_one(&s.world.db.pool).await?;
        assert_eq!(row["home"], json!(destination));
        assert_eq!(row["org"], json!(destination));
        assert_eq!(row["owner"], json!(s.company.boss.id()));
        assert_eq!(
            row["placement"],
            json!(if connected { "connected" } else { "managed" })
        );
        if let Some(machine) = machine {
            assert_eq!(row["machine"], json!(machine));
            assert!(row["template"].is_null());
        }
        let mut tx = s.world.db.pool.begin().await?;
        let job_id: Id = sqlx::query_scalar(
            "SELECT id FROM jobs WHERE kind='environment_provision' AND environment_id=$1",
        )
        .bind(made)
        .fetch_one(&mut *tx)
        .await?;
        let job = orch_db::queries::jobs::job(&mut tx, job_id)
            .await?
            .ok_or("missing actual copy job")?;
        let ready = orch_engine::jobs::provision::creator_checkpoint(
            &mut tx,
            &job,
            s.world.app.app.clock.as_ref(),
            &orch_engine::access::Unconnected,
        )
        .await?
        .ok_or("missing actual creator authority")?;
        ready.check(s.world.app.app.clock.as_ref())?;
        tx.commit().await?;
        let mut tx = s.world.db.pool.begin().await?;
        orch_settings::write::write(
            &mut tx,
            &orch_settings::write::Write {
                actor: orch_db::audit::Actor::User(s.company.boss.id()),
                target: orch_settings::Target::Node(s.company.acme),
                key: orch_settings_contract::registry::keys::SHARING_ALLOW_COPY_OUT,
                value: json!(false),
                locked: Some(true),
                expected_revision: None,
                may_lock: true,
                origin: &Default::default(),
            },
            s.world.now(),
        )
        .await?;
        tx.commit().await?;
        let mut tx = s.world.db.pool.begin().await?;
        let job = orch_db::queries::jobs::job(&mut tx, job_id)
            .await?
            .ok_or("missing retained copy job")?;
        assert!(
            orch_engine::jobs::provision::creator_checkpoint(
                &mut tx,
                &job,
                s.world.app.app.clock.as_ref(),
                &orch_engine::access::Unconnected
            )
            .await
            .is_err()
        );
        tx.rollback().await?;
        let bypass = s
            .world
            .app
            .call(EnvironmentCreate, &[])
            .token(&s.company.boss.token)
            .body(&NewEnvironment {
                name: Some("Initial files obey copy policy".into()),
                home_node_id: Some(destination),
                template_id: Some(s.template),
                host_id: Some(Some(s.host)),
                files: Some(
                    orch_api_contract::binding::NewEnvironmentFiles::FromEnvironment {
                        from_environment: s.source,
                        selection: None,
                    },
                ),
                ..Default::default()
            })
            .send_bytes()
            .await;
        assert_eq!(bypass.status, http::StatusCode::FORBIDDEN, "{:?}", bypass);
        let in_destination: i64 =
            sqlx::query_scalar("SELECT count(*) FROM environments WHERE home_node_id=$1")
                .bind(destination)
                .fetch_one(&s.world.db.pool)
                .await?;
        assert_eq!(
            in_destination, 1,
            "an initial-file source is the same copy boundary, not another creation door"
        );
        let statuses: Vec<String> =
            sqlx::query_scalar("SELECT status FROM environments WHERE id=ANY($1) ORDER BY id")
                .bind(vec![s.source, made])
                .fetch_all(&s.world.db.pool)
                .await?;
        assert!(statuses.iter().any(|state| state == "running"));
        assert!(statuses.iter().any(|state| state == "provisioning"));
        let audit: Json = sqlx::query_scalar(
            "SELECT details FROM audit_log WHERE action='content.copied_out' AND object_id=$1",
        )
        .bind(s.source)
        .fetch_one(&s.world.db.pool)
        .await?;
        assert_eq!(audit["destination_org_id"], json!(destination));
        assert_eq!(audit["created_id"], json!(made));
    }
    Ok(())
}

#[tokio::test]
async fn both_owner_environment_moves_keep_native_placement_and_the_source_until_copied_files_commit()
-> Proof {
    use orch_api_contract::duplicates::{
        EnvironmentMoveOut, EnvironmentMoveOutBody, EnvironmentMoveOutPreview, MoveOutConfirmation,
        MoveOutDeletePlan,
    };
    for connected in [false, true] {
        let (s, machine) = if connected {
            let (s, m) = Scene::connected_source().await?;
            (s, Some(m))
        } else {
            (Scene::new().await?, None)
        };
        // The fixture's source was already marked running; retire its original fixture-only queued Make too.
        sqlx::query("UPDATE jobs SET state='succeeded' WHERE environment_id=$1 AND kind='environment_provision' AND state='queued'").bind(s.source).execute(&s.world.db.pool).await?;
        let destination = places::make_org(
            &s.world,
            &s.company.boss,
            "Move workspace recipient",
            "move-workspace-recipient",
        )
        .await
        .base
        .id;
        let copy = EnvironmentDuplicateBody {
            destination: Some(DuplicateDestination::Node { id: destination }),
            ..Default::default()
        };
        let class = if connected { "connected" } else { "managed" };
        let mut rules = Vec::new();
        for operation in [
            format!("EnvironmentMoveOutPreview.{class}"),
            format!("EnvironmentMoveOut.{class}"),
        ] {
            let mut one = if let Some(machine) = machine {
                s.connected_rules(machine)
            } else {
                s.rules()
            };
            one[1]["target"]["id"] = json!(destination);
            for rule in &mut one {
                for allow in rule["allow"]
                    .as_array_mut()
                    .ok_or("rules have no actions")?
                {
                    allow["operation"] = json!(operation);
                }
            }
            one.push(rule(&operation, "environment", s.source, "self", "manage"));
            rules.extend(one);
        }
        let token = key(&s.world, s.company.boss.id(), rules).await?;
        let preview = s
            .world
            .app
            .call(EnvironmentMoveOutPreview, &[&s.source])
            .token(&token.secret)
            .body(&copy)
            .send_bytes()
            .await;
        let plan: MoveOutDeletePlan = serde_json::from_value(ok(&preview)?)?;
        let reply = s
            .world
            .app
            .call(EnvironmentMoveOut, &[&s.source])
            .token(&token.secret)
            .body(&EnvironmentMoveOutBody {
                copy,
                delete_plan: MoveOutConfirmation {
                    revision: plan.revision,
                    stamp: plan.stamp,
                },
            })
            .send_bytes()
            .await;
        let body = ok(&reply)?;
        let target: Id = serde_json::from_value(body["id"].clone())?;
        assert_eq!(
            body,
            json!({"projection":"receipt","kind":"environment","id":target})
        );
        let row:Json=sqlx::query_scalar("SELECT jsonb_build_object('owner',owner_id,'home',home_node_id,'placement',placement,'machine',machine_id,'status',status) FROM environments WHERE id=$1").bind(target).fetch_one(&s.world.db.pool).await?;
        assert_eq!(row["owner"], json!(s.company.boss.id()));
        assert_eq!(row["home"], json!(destination));
        assert_eq!(row["placement"], class);
        assert_eq!(row["status"], "provisioning");
        if let Some(machine) = machine {
            assert_eq!(row["machine"], json!(machine));
        }
        let source:Json=sqlx::query_scalar("SELECT jsonb_build_object('deleted',deleted_at,'remove',remove_at,'status',status) FROM environments WHERE id=$1").bind(s.source).fetch_one(&s.world.db.pool).await?;
        assert!(source["deleted"].is_null() && source["remove"].is_null());
        assert_eq!(source["status"], "running");
        let plan = s.plan(target).await?;
        assert_eq!(
            plan["creator_provision"]["origin"]["operation"],
            format!("EnvironmentMoveOut.{class}")
        );
        assert_eq!(plan["environment_move_out"]["source"], json!(s.source));
        let mut tx = s.world.db.pool.begin().await?;
        let job = orch_db::queries::jobs::job(
            &mut tx,
            serde_json::from_value(plan["creator_provision"]["job"].clone())?,
        )
        .await?
        .ok_or("native job absent")?;
        let checked = orch_engine::jobs::provision::creator_checkpoint(
            &mut tx,
            &job,
            s.world.app.app.clock.as_ref(),
            &orch_engine::access::Unconnected,
        )
        .await?
        .ok_or("actual moving checkpoint absent")?;
        assert_eq!(
            checked.principal().caller().admitted_operation(),
            Some(format!("EnvironmentMoveOut.{class}").as_str())
        );
        checked.check(s.world.app.app.clock.as_ref())?;
        tx.rollback().await?;
    }
    Ok(())
}
#[tokio::test]
async fn environment_move_uses_the_same_native_busy_refusal_and_rejects_a_changed_delete_plan()
-> Proof {
    use orch_api_contract::{
        duplicates::{
            EnvironmentMoveOut, EnvironmentMoveOutBody, EnvironmentMoveOutPreview,
            MoveOutConfirmation, MoveOutDeletePlan,
        },
        environment::{EnvironmentDelete, EnvironmentDeleteQuery},
    };
    let s = Scene::new().await?;
    let destination = places::make_org(
        &s.world,
        &s.company.boss,
        "Reviewed environment recipient",
        "reviewed-environment-recipient",
    )
    .await
    .base
    .id;
    let copy = EnvironmentDuplicateBody {
        destination: Some(DuplicateDestination::Node { id: destination }),
        ..Default::default()
    };
    let native = s
        .world
        .app
        .call(EnvironmentDelete, &[&s.source])
        .token(&s.company.boss.token)
        .query(&EnvironmentDeleteQuery {
            confirm: "Source workspace".into(),
            at: None,
        })
        .send_bytes()
        .await;
    assert!(
        !native.status.is_success(),
        "the queued source Make blocks native Delete"
    );
    let preview = s
        .world
        .app
        .call(EnvironmentMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send_bytes()
        .await;
    assert_eq!(preview.problem().code, native.problem().code);
    sqlx::query("UPDATE jobs SET state='succeeded' WHERE environment_id=$1 AND kind='environment_provision' AND state='queued'").bind(s.source).execute(&s.world.db.pool).await?;
    let preview = s
        .world
        .app
        .call(EnvironmentMoveOutPreview, &[&s.source])
        .token(&s.company.boss.token)
        .body(&copy)
        .send_bytes()
        .await;
    let plan: MoveOutDeletePlan = serde_json::from_value(ok(&preview)?)?;
    sqlx::query("UPDATE environments SET revision=revision+1 WHERE id=$1")
        .bind(s.source)
        .execute(&s.world.db.pool)
        .await?;
    let reply = s
        .world
        .app
        .call(EnvironmentMoveOut, &[&s.source])
        .token(&s.company.boss.token)
        .body(&EnvironmentMoveOutBody {
            copy,
            delete_plan: MoveOutConfirmation {
                revision: plan.revision,
                stamp: plan.stamp,
            },
        })
        .send_bytes()
        .await;
    assert_eq!(reply.status, 409);
    assert_eq!(s.count().await?, 1);
    Ok(())
}

#[tokio::test]
async fn environment_move_keeps_ownership_separate_from_management_and_destination_creation()
-> Proof {
    use orch_api_contract::duplicates::EnvironmentMoveOutPreview;
    let s = Scene::new().await?;
    sqlx::query("UPDATE jobs SET state='succeeded' WHERE environment_id=$1 AND kind='environment_provision' AND state='queued'").bind(s.source).execute(&s.world.db.pool).await?;
    for permission in ["environment.view", "environment.manage", "files.view"] {
        resources::name_person(
            &s.world,
            "environment",
            s.source,
            permission,
            s.company.admin.id(),
        )
        .await;
    }
    let destination = places::make_org(
        &s.world,
        &s.company.admin,
        "Manager workspace",
        "manager-workspace",
    )
    .await
    .base
    .id;
    let reply = s
        .world
        .app
        .call(EnvironmentMoveOutPreview, &[&s.source])
        .token(&s.company.admin.token)
        .body(&EnvironmentDuplicateBody {
            destination: Some(DuplicateDestination::Node { id: destination }),
            ..Default::default()
        })
        .send_bytes()
        .await;
    assert_eq!(reply.status, 403, "{}", String::from_utf8_lossy(&reply.raw));
    assert_eq!(s.count().await?, 1);
    Ok(())
}
