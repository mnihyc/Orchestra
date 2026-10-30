//! Opaque workspace source metadata follows the existing native full/limited environment doors.

mod places;
mod realtime_support;
mod resources;
mod rig;
mod support;

use orch_api_contract::{
    binding::NewEnvironmentAccess,
    common::ErrorCode,
    environment::{
        Environment, EnvironmentGet, EnvironmentSyncGet, EnvironmentSyncGetQuery,
        EnvironmentUpdate, MachineEnvironmentCreate, MachineEnvironmentCreateBody,
    },
};
use orch_db::{
    Db,
    queries::{
        access::{self, PageKey, RowWrite, write_row},
        hierarchy::testing::insert_node,
    },
    secrets::MasterKey,
};
use orch_link_contract::policy::LocalPolicy;
use orch_types::{Id, new_id};
use places::Who;
use resources::{Company, company};
use serde_json::{Value, json};
use support::World;

type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Scene {
    world: World,
    people: Company,
    environment: Id,
    thread: Id,
}

impl Scene {
    async fn new() -> Proof<Self> {
        // A standalone websocket fixture shares this deployment key too, outside every read cut.
        orch_api_core::routes::install_master_key(&MasterKey::new([7; 32]));
        let world = World::new().await;
        let people = company(&world).await;
        let project = places::make_project(&world, &people.boss, people.acme, "Workspace")
            .await
            .base
            .id;
        let topic = places::make_topic(&world, &people.boss, project, "Work")
            .await
            .base
            .id;
        let machine = {
            let mut conn = world.conn().await;
            let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
                &mut conn,
                people.admin.id(),
                "local-node",
            )
            .await?;
            // The fixture gives the machine its own page as native enrollment does; owner_id is descriptive.
            let policy = LocalPolicy {
                environment_roots: vec!["/srv/ada".into()],
                ..Default::default()
            };
            sqlx::query(
                "UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb) WHERE id=$1",
            )
            .bind(machine)
            .bind(serde_json::to_value(policy)?)
            .execute(&mut *conn)
            .await?;
            machine
        };
        let made = world
            .app
            .call(MachineEnvironmentCreate, &[&machine])
            .token(&people.admin.token)
            .body(&MachineEnvironmentCreateBody {
                data_sources: None,
                home_id: people.acme,
                name: "Private workspace".into(),
                root: "/srv/ada/first".into(),
                home_node_id: Some(topic),
                remote_write: None,
                terminal: None,
                access: Some(NewEnvironmentAccess::OnlyMe),
            })
            .send()
            .await
            .ok();
        let environment = made.base.id;
        let thread = new_id();
        {
            let mut conn = world.conn().await;
            let page = PageKey::new("environment", environment, "live");
            for permission in [
                "environment.view",
                "environment.manage",
                "files.view",
                "files.manage",
                "terminal.view",
                "terminal.manage",
                "browser.view",
                "browser.manage",
                "vnc.view",
                "vnc.manage",
                "ports.view",
                "ports.manage",
            ] {
                // Live rows use Any; managed/connected profiles select inherited defaults.
                write_row(
                    &mut conn,
                    &RowWrite {
                        page: &page,
                        profile: "any",
                        permission,
                        inherit: false,
                        locked: false,
                    },
                    Some(people.admin.id()),
                    world.now(),
                )
                .await?;
            }
            insert_node(
                &mut conn,
                thread,
                "thread",
                Some(topic),
                "Visible thread",
                people.admin.id(),
            )
            .await?;
            sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
                .bind(thread)
                .bind(environment)
                .execute(&mut *conn)
                .await?;
        }
        Ok(Self {
            world,
            people,
            environment,
            thread,
        })
    }

    async fn read(&self, who: &Who) -> Environment {
        self.world
            .app
            .call(EnvironmentGet, &[&self.environment])
            .token(&who.token)
            .send()
            .await
            .ok()
    }

    async fn sync(&self, who: &Who) -> Environment {
        self.world
            .app
            .call(EnvironmentSyncGet, &[&self.environment])
            .token(&who.token)
            .query(&EnvironmentSyncGetQuery { since: None })
            .send()
            .await
            .ok()
            .environment
    }

    async fn update(&self, body: Value) -> Environment {
        self.world
            .app
            .call(EnvironmentUpdate, &[&self.environment])
            .token(&self.people.admin.token)
            .raw_body(&body)
            .send()
            .await
            .ok()
    }
}

fn token(environment: &Environment) -> Proof<&str> {
    let value = environment
        .workspace_incarnation
        .as_deref()
        .ok_or("modern source projection omitted its incarnation")?;
    assert_eq!(value.len(), 69);
    assert!(value.starts_with("wsi1_"));
    Ok(value)
}

fn limited(environment: &Environment) {
    assert_eq!(environment.generation, 0);
    assert!(
        environment.workspace_root.is_empty()
            && environment.machine.is_none()
            && environment.host.is_none()
    );
    assert!(
        environment.data_sources.is_none()
            && environment
                .config
                .as_object()
                .is_some_and(|value| value.is_empty())
    );
    assert!(
        !environment
            .access
            .permissions
            .iter()
            .any(|permission| matches!(permission.as_str(), "environment.view" | "files.view"))
    );
    assert!(
        environment.access.permissions.is_empty(),
        "a thread-only reader holds no environment content door, including Ports"
    );
}

#[tokio::test]
async fn full_and_limited_native_http_sync_reads_share_only_an_opaque_current_source_identity()
-> Proof {
    let scene = Scene::new().await?;
    let full = scene.read(&scene.people.admin).await;
    let limited_get = scene.read(&scene.people.viewer).await;
    limited(&limited_get);
    let limited_sync = scene.sync(&scene.people.viewer).await;
    limited(&limited_sync);
    assert_eq!(token(&limited_get)?, token(&full)?);
    assert_eq!(token(&limited_sync)?, token(&full)?);
    let wire = serde_json::to_string(&limited_sync)?;
    assert!(!wire.contains(&full.workspace_root));
    if let Some(machine) = full.machine {
        assert!(!wire.contains(&machine.id.to_string()));
    }
    assert_eq!(
        token(&scene.sync(&scene.people.admin).await)?,
        token(&limited_get)?
    );
    Ok(())
}

#[tokio::test]
async fn an_unrelated_environment_update_keeps_the_token_but_workspace_selection_replaces_it()
-> Proof {
    let scene = Scene::new().await?;
    let before = scene.read(&scene.people.admin).await;
    let renamed = scene
        .update(json!({"name":"Renamed workspace","expected_revision":before.revision}))
        .await;
    assert!(renamed.revision > before.revision);
    assert_eq!(token(&renamed)?, token(&before)?);
    let selected=scene.update(json!({"data_sources":{"workspace":{"kind":"existing_directory","path":"/srv/ada/second"},
        "home":{"kind":"made_by_orch"}},"expected_revision":renamed.revision})).await;
    assert_ne!(token(&selected)?, token(&before)?);
    let home=scene.update(json!({"data_sources":{"workspace":{"kind":"existing_directory","path":"/srv/ada/second"},
        "home":{"kind":"existing_directory","path":"/srv/ada/home"}},"expected_revision":selected.revision})).await;
    assert_eq!(token(&home)?, token(&selected)?);
    let reader = scene.sync(&scene.people.viewer).await;
    limited(&reader);
    assert_eq!(token(&reader)?, token(&selected)?);
    Ok(())
}

#[tokio::test]
async fn forbidden_expired_and_revoked_readers_cannot_retrieve_an_incarnation_or_new_permission()
-> Proof {
    let scene = Scene::new().await?;
    let reader = &scene.people.stranger;
    let refused = scene
        .world
        .app
        .call(EnvironmentGet, &[&scene.environment])
        .token(&reader.token)
        .send()
        .await;
    assert_eq!(refused.problem().code, ErrorCode::AuthzDenied);
    let entry = {
        let mut conn = scene.world.conn().await;
        access::testing::add_entry(
            &mut conn,
            &PageKey::new("node", scene.thread, "live"),
            "any",
            "thread.view",
            ("user", Some(reader.id()), None),
            Some(scene.world.now() + chrono::Duration::seconds(1)),
            scene.world.now(),
        )
        .await?
    };
    let shown = scene.sync(reader).await;
    limited(&shown);
    token(&shown)?;
    scene.world.advance(chrono::Duration::seconds(2));
    let refused = scene
        .world
        .app
        .call(EnvironmentSyncGet, &[&scene.environment])
        .token(&reader.token)
        .query(&EnvironmentSyncGetQuery { since: None })
        .send()
        .await;
    assert_eq!(refused.problem().code, ErrorCode::AuthzDenied);
    let active = {
        let mut conn = scene.world.conn().await;
        assert!(
            access::remove_entry(&mut conn, entry, scene.people.admin.id(), scene.world.now())
                .await?
        );
        access::testing::add_entry(
            &mut conn,
            &PageKey::new("node", scene.thread, "live"),
            "any",
            "thread.view",
            ("user", Some(reader.id()), None),
            None,
            scene.world.now(),
        )
        .await?
    };
    let visible = scene.sync(reader).await;
    limited(&visible);
    assert_eq!(token(&visible)?, token(&shown)?);
    // Revoke the actual permission, leaving its native removed record and the source unchanged.
    let mut conn = scene.world.conn().await;
    assert!(
        access::remove_entry(
            &mut conn,
            active,
            scene.people.admin.id(),
            scene.world.now()
        )
        .await?
    );
    drop(conn);
    let refused = scene
        .world
        .app
        .call(EnvironmentGet, &[&scene.environment])
        .token(&reader.token)
        .send()
        .await;
    assert_eq!(refused.problem().code, ErrorCode::AuthzDenied);
    assert_eq!(
        token(&scene.read(&scene.people.admin).await)?,
        token(&shown)?
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_existing_limited_environment_feed_delivers_the_same_source_change_without_an_extra_read()
-> Proof {
    let scene = Scene::new().await?;
    let baseline = scene
        .world
        .app
        .call(EnvironmentSyncGet, &[&scene.environment])
        .token(&scene.people.viewer.token)
        .query(&EnvironmentSyncGetQuery { since: None })
        .send()
        .await
        .ok();
    let initial = baseline.environment;
    limited(&initial);
    let db = Db::from_pool(scene.world.db.pool.clone());
    let server = realtime_support::Server::over(
        &db,
        &scene.world.app.app.clock,
        realtime_support::quick(&scene.world.app.app.config),
    )
    .await;
    let feed = format!("env:{}", scene.environment);
    let mut client =
        realtime_support::Client::signed_in(server.addr, &scene.people.viewer.token).await;
    // A durable subscription continues the one HTTP cut; None starts at head, not a snapshot.
    client.sub(1, &feed, Some(baseline.seq)).await;
    let subscribed =
        environment_frame(&mut client, &feed, "subscribed", "subscribe from HTTP cut").await?;
    assert_eq!(subscribed["seq"], baseline.seq);
    assert_eq!(subscribed["mode"], "delta");
    let current = scene.read(&scene.people.admin).await;
    assert_eq!(token(&initial)?, token(&current)?);
    let renamed = scene
        .update(json!({"name":"Only metadata","expected_revision":current.revision}))
        .await;
    assert!(
        renamed.seq > baseline.seq,
        "a committed metadata update must advance the environment feed: before={} after={}",
        baseline.seq,
        renamed.seq
    );
    let frame = environment_frame(&mut client, &feed, "sync", "metadata rename").await?;
    assert_eq!(frame["prev"], baseline.seq);
    assert_eq!(frame["seq"], renamed.seq);
    let update: Environment = serde_json::from_value(frame["data"]["environment"].clone())?;
    limited(&update);
    assert_eq!(token(&update)?, token(&initial)?);
    let selected=scene.update(json!({"data_sources":{"workspace":{"kind":"existing_directory","path":"/srv/ada/current"},
        "home":{"kind":"made_by_orch"}},"expected_revision":renamed.revision})).await;
    assert!(
        selected.seq > renamed.seq,
        "a committed workspace selection must advance the same environment feed"
    );
    let frame = environment_frame(&mut client, &feed, "sync", "workspace selection").await?;
    assert_eq!(frame["prev"], renamed.seq);
    assert_eq!(frame["seq"], selected.seq);
    let update: Environment = serde_json::from_value(frame["data"]["environment"].clone())?;
    limited(&update);
    assert_ne!(token(&update)?, token(&initial)?);
    assert_eq!(token(&update)?, token(&selected)?);
    drop(client);
    server.stop().await;
    Ok(())
}

async fn environment_frame(
    client: &mut realtime_support::Client,
    feed: &str,
    op: &str,
    phase: &str,
) -> Proof<Value> {
    let frame = match client.hear_within(realtime_support::PATIENCE).await {
        Some(realtime_support::Heard::Frame(frame)) => frame,
        other => {
            return Err(
                format!("environment feed phase {phase}: expected {op}, got {other:?}").into(),
            );
        }
    };
    assert_eq!(
        frame["op"], op,
        "environment feed phase {phase}: id={} code={}",
        frame["id"], frame["code"]
    );
    assert_eq!(frame["id"], 1, "environment feed phase {phase}");
    assert_eq!(frame["feed"], feed, "environment feed phase {phase}");
    Ok(frame)
}
