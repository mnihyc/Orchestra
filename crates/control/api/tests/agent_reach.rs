//! What the agents of an environment can reach, as each reader is let see it: its manager sees the secret names, mounts, profiles and a line for each thread they may view; a reader through a thread sees whether the network is in force and the lines of the threads they may view; nobody is told a value, a hidden thread or its arrangement, and a stranger is refused.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::binding::NewEnvironmentAccess;
use orch_api_contract::environment::{
    AgentReachOverview, Environment, EnvironmentAgentReachGet, MachineEnvironmentCreate,
    MachineEnvironmentCreateBody,
};
use orch_db::queries::access::{PageKey, RowWrite, write_row};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_types::{Id, new_id};
use places::Who;
use resources::{Company, company};
use support::World;

struct Scene {
    world: World,
    people: Company,
    environment: Environment,
    visible: Id,
    unbound: Id,
    hidden: Id,
}

/// A thread named `name` in `topic`, owned by `owner`, working in the environment with the arrangement given.
async fn thread_in(
    world: &World,
    topic: Id,
    name: &str,
    owner: Id,
    environment: Id,
    arrangement: Option<&str>,
) -> Id {
    let thread = new_id();
    let mut conn = world.conn().await;
    insert_node(&mut conn, thread, "thread", Some(topic), name, owner)
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET environment_id = $2, arrangement = $3 WHERE node_id = $1")
        .bind(thread)
        .bind(environment)
        .bind(arrangement)
        .execute(&mut *conn)
        .await
        .unwrap();
    thread
}

impl Scene {
    async fn new() -> Self {
        orch_api_core::routes::resources::sealing::install_master_key(
            orch_db::secrets::MasterKey::new([7; 32]),
        );
        orch_api_core::routes::settings::install_master_key(orch_db::secrets::MasterKey::new(
            [7; 32],
        ));
        let world = World::new().await;
        let people = company(&world).await;
        let project = places::make_project(&world, &people.boss, people.acme, "Web").await;
        let topic = places::make_topic(&world, &people.boss, project.base.id, "Work")
            .await
            .base
            .id;
        let machine = {
            let mut conn = world.conn().await;
            let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
                &mut conn,
                people.admin.id(),
                "laptop",
            )
            .await
            .unwrap();
            let policy = orch_link_contract::policy::LocalPolicy {
                environment_roots: vec!["/srv/ada".to_owned()],
                ..Default::default()
            };
            sqlx::query(
                "UPDATE machines SET facts = jsonb_build_object('policy', $2::jsonb) WHERE id = $1",
            )
            .bind(machine)
            .bind(serde_json::to_value(policy).unwrap())
            .execute(&mut *conn)
            .await
            .unwrap();
            machine
        };
        let environment: Environment = world
            .app
            .call(MachineEnvironmentCreate, &[&machine])
            .token(&people.admin.token)
            .body(&MachineEnvironmentCreateBody {
                data_sources: None,
                home_id: people.acme,
                name: "main".to_owned(),
                root: "/srv/ada/main".to_owned(),
                home_node_id: Some(topic),
                remote_write: None,
                terminal: None,
                access: Some(NewEnvironmentAccess::OnlyMe),
            })
            .send()
            .await
            .ok();
        let id = environment.base.id;
        {
            // Only its maker holds anything on its own page; the others reach it through a thread or not at all.
            let mut conn = world.conn().await;
            let page = PageKey::new("environment", id, "live");
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
                // The fifth door, whose View ships to Viewers like the others.
                "ports.view",
                "ports.manage",
            ] {
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
                .await
                .unwrap();
            }
            sqlx::query("UPDATE environments SET config = config || jsonb_build_object('env.secret_names', jsonb_build_array('DEPLOY_TOKEN', 'NPM_TOKEN')) WHERE id = $1")
                .bind(id).execute(&mut *conn).await.unwrap();
        }
        let visible = thread_in(
            &world,
            topic,
            "Visible work",
            people.boss.id(),
            id,
            Some("key_separated"),
        )
        .await;
        let unbound = thread_in(&world, topic, "Not run yet", people.boss.id(), id, None).await;
        // A thread of another org that works in the same environment, which neither reader may view.
        let other = places::make_org(&world, &people.stranger, "Other", "other").await;
        let elsewhere =
            places::make_project(&world, &people.stranger, other.base.id, "Elsewhere").await;
        let far = places::make_topic(&world, &people.stranger, elsewhere.base.id, "Far")
            .await
            .base
            .id;
        let hidden = thread_in(
            &world,
            far,
            "Hidden handover",
            people.stranger.id(),
            id,
            Some("handed_over"),
        )
        .await;
        Self {
            world,
            people,
            environment,
            visible,
            unbound,
            hidden,
        }
    }

    async fn reach(&self, who: &Who) -> AgentReachOverview {
        self.world
            .app
            .call(EnvironmentAgentReachGet, &[&self.environment.base.id])
            .token(&who.token)
            .send()
            .await
            .ok()
    }
}

#[tokio::test]
async fn its_manager_sees_the_secret_names_mounts_profiles_and_a_line_for_each_thread_they_may_view()
 {
    let scene = Scene::new().await;
    let overview = scene.reach(&scene.people.admin).await;
    assert_eq!(
        overview.secrets.names.as_deref(),
        Some(&["DEPLOY_TOKEN".to_owned(), "NPM_TOKEN".to_owned()][..])
    );
    assert!(
        overview.secrets.source.is_some(),
        "the place the names are set in is named"
    );
    let path = overview.home_path.as_ref().expect(
        "the admitted native overview carries its home ancestry without a separate tree read",
    );
    assert_eq!(
        path.last().map(|node| node.id),
        Some(scene.environment.home.id)
    );
    assert!(path.iter().any(|node| node.id == scene.people.acme));
    assert_eq!(
        overview.mounts.as_deref(),
        Some(&[][..]),
        "a reader of its files sees its mounts: none here"
    );
    assert_eq!(
        overview.browser_profiles.as_deref(),
        Some(&[][..]),
        "nothing was found yet, and nothing is made up"
    );
    let lines: Vec<(Id, &str)> = overview
        .credentials
        .iter()
        .map(|line| (line.thread.id, line.placement.as_str()))
        .collect();
    assert!(
        lines.contains(&(scene.visible, "key_separated")),
        "{lines:?}"
    );
    assert!(lines.contains(&(scene.unbound, "unbound")), "{lines:?}");
    assert!(
        lines.iter().all(|(thread, _)| *thread != scene.hidden),
        "a thread the reader may not view is not named"
    );
    let wire = serde_json::to_string(&overview).unwrap();
    assert!(
        !wire.contains("Hidden handover") && !wire.contains("handed_over"),
        "{wire}"
    );
}

#[tokio::test]
async fn a_reader_through_a_thread_sees_whether_its_network_is_in_force_and_its_threads_lines_and_nothing_of_the_rest()
 {
    let scene = Scene::new().await;
    let overview = scene.reach(&scene.people.viewer).await;
    assert_eq!(
        overview.secrets.names, None,
        "secret names are for whoever manages the environment"
    );
    assert_eq!(overview.secrets.source, None);
    assert_eq!(
        overview.home_path, None,
        "a reader admitted through a thread receives no additional home path"
    );
    assert_eq!(
        overview.mounts, None,
        "mounts are for whoever reads its files"
    );
    assert_eq!(
        overview.browser_profiles, None,
        "profiles are for whoever sees its browsers"
    );
    assert!(overview.network.allowed_hosts.is_empty() && overview.network.networks.is_empty());
    let threads: Vec<Id> = overview
        .credentials
        .iter()
        .map(|line| line.thread.id)
        .collect();
    assert!(threads.contains(&scene.visible), "{threads:?}");
    assert!(!threads.contains(&scene.hidden));
    let wire = serde_json::to_string(&overview).unwrap();
    assert!(
        !wire.contains("DEPLOY_TOKEN") && !wire.contains("handed_over"),
        "{wire}"
    );
}

#[tokio::test]
async fn somebody_who_neither_holds_the_environment_nor_sees_a_thread_of_it_is_refused() {
    let scene = Scene::new().await;
    let outsider = places::person(&scene.world, "outsider").await;
    let refused = scene
        .world
        .app
        .call(EnvironmentAgentReachGet, &[&scene.environment.base.id])
        .token(&outsider.token)
        .send()
        .await
        .problem();
    let wire = serde_json::to_string(&refused).unwrap();
    assert!(
        !wire.contains("DEPLOY_TOKEN") && !wire.contains("Visible work"),
        "{wire}"
    );
}
