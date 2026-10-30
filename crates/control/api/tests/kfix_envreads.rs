//! Environment cards, topic summaries and job progress agree on exact rights, current work and distinct reach.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;

use orch_api_contract::binding::NewEnvironmentAccess;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{
    Environment, EnvironmentGet, EnvironmentReachGet, EnvironmentSyncGet, EnvironmentSyncGetQuery,
    EnvironmentSyncJob, MachineEnvironmentCreate, MachineEnvironmentCreateBody,
};
use orch_api_contract::nodes::{Node, NodeGet};
use orch_api_contract::topics::TopicEnvironmentList;
use orch_api_contract::workspace::JobGet;
use orch_db::queries::access::{PageKey, RowWrite, write_row};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::jobs::{self, NewJob, kind};
use orch_types::{Id, new_id};
use places::Who;
use resources::{Company, company, name_person};
use serde_json::json;
use support::World;

struct Scene {
    world: World,
    people: Company,
    machine: Id,
    topic: Id,
    thread: Id,
    environment: Environment,
}

async fn register(
    world: &World,
    people: &Company,
    machine: Id,
    topic: Id,
    name: &str,
) -> Environment {
    world
        .app
        .call(MachineEnvironmentCreate, &[&machine])
        .token(&people.admin.token)
        .body(&MachineEnvironmentCreateBody {
            data_sources: None,
            home_id: people.acme,
            name: name.to_owned(),
            root: format!("/srv/ada/{name}"),
            home_node_id: Some(topic),
            remote_write: None,
            terminal: None,
            access: Some(NewEnvironmentAccess::OnlyMe),
        })
        .send()
        .await
        .ok()
}

async fn private(world: &World, environment: Id, owner: Id) {
    let mut conn = world.conn().await;
    let page = PageKey::new("environment", environment, "live");
    // Live Any rows cut inherited defaults, including the separate Ports content door.
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
        write_row(
            &mut conn,
            &RowWrite {
                page: &page,
                profile: "any",
                permission,
                inherit: false,
                locked: false,
            },
            Some(owner),
            world.now(),
        )
        .await
        .unwrap();
    }
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
            // The fixture starts enrollment's actual Owners page, not only the row's first-owner label.
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
        let environment = register(&world, &people, machine, topic, "main").await;
        private(&world, environment.base.id, people.admin.id()).await;
        let thread = new_id();
        {
            let mut conn = world.conn().await;
            insert_node(
                &mut conn,
                thread,
                "thread",
                Some(topic),
                "Visible work",
                people.boss.id(),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE threads SET environment_id = $2 WHERE node_id = $1")
                .bind(thread)
                .bind(environment.base.id)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        Self {
            world,
            people,
            machine,
            topic,
            thread,
            environment,
        }
    }

    async fn job(&self, word: &str, finished: bool) -> Id {
        let mut conn = self.world.conn().await;
        let progress = json!({"phase":"image","percent":25,"phases":[],"files_done":0,"files_total":null,
            "bytes_done":0,"bytes_total":null,"rate_bps":null,"eta_seconds":null,"message":"hidden/file.txt","log":"hidden-build-output"});
        let row = jobs::insert(
            &mut conn,
            &NewJob {
                id: new_id(),
                kind: word,
                node_id: None,
                environment_id: (word != kind::EXPORT).then_some(self.environment.base.id),
                provider_id: None,
                plan: &json!({}),
                progress: &progress,
                created_by: Some(self.people.admin.id()),
            },
            self.world.now(),
        )
        .await
        .unwrap();
        if finished {
            jobs::finish(
                &mut conn,
                row.id,
                "succeeded",
                &json!({"private":"result"}),
                None,
                None,
                self.world.now(),
            )
            .await
            .unwrap();
        }
        row.id
    }

    async fn read(&self, who: &Who) -> Environment {
        self.world
            .app
            .call(EnvironmentGet, &[&self.environment.base.id])
            .token(&who.token)
            .send()
            .await
            .ok()
    }
}

#[tokio::test]
async fn a_thread_reader_sees_only_the_current_jobs_safe_progress_and_never_finished_history() {
    let scene = Scene::new().await;
    sqlx::query("UPDATE environments SET config=config || jsonb_build_object('env.data_sources',$2::jsonb) WHERE id=$1")
        .bind(scene.environment.base.id)
        .bind(json!({"workspace":{"kind":"existing_directory","path":"/private/workspace"},"home":{"kind":"made_by_orch"}}))
        .execute(&scene.world.db.pool).await.unwrap();
    assert!(scene.read(&scene.people.admin).await.data_sources.is_some());
    let old = scene.job(kind::ENVIRONMENT_PROVISION, true).await;
    let current = scene.job(kind::ENVIRONMENT_RECREATE, false).await;
    let environment = scene.read(&scene.people.viewer).await;
    assert!(environment.workspace_root.is_empty());
    assert!(
        environment.data_sources.is_none(),
        "a thread reader never learns private directory selections"
    );
    assert_eq!(environment.job.as_ref().unwrap().id, current);
    let sync = scene
        .world
        .app
        .call(EnvironmentSyncGet, &[&scene.environment.base.id])
        .token(&scene.people.viewer.token)
        .query(&EnvironmentSyncGetQuery { since: Some(0) })
        .send()
        .await
        .ok();
    assert_eq!(sync.jobs.len(), 1);
    assert!(
        matches!(&sync.jobs[0], EnvironmentSyncJob::Reference(job) if job.id == current && job.percent == 25)
    );
    let wire = serde_json::to_value(&sync).unwrap();
    for field in ["log", "result", "created_by", "progress"] {
        assert!(
            wire["jobs"][0].get(field).is_none(),
            "{field} is not progress-only"
        );
    }
    let job = scene
        .world
        .app
        .call(JobGet, &[&current])
        .token(&scene.people.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(job.progress.percent, 25);
    assert!(
        job.log.is_none()
            && job.result.is_none()
            && job.created_by.is_none()
            && job.progress.message.is_none()
    );
    let refused = scene
        .world
        .app
        .call(JobGet, &[&old])
        .token(&scene.people.viewer.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
    let full = scene
        .world
        .app
        .call(EnvironmentSyncGet, &[&scene.environment.base.id])
        .token(&scene.people.admin.token)
        .query(&EnvironmentSyncGetQuery { since: Some(0) })
        .send()
        .await
        .ok();
    assert!(
        full.jobs
            .iter()
            .any(|job| matches!(job, EnvironmentSyncJob::Full(job) if job.id == old))
    );
}

#[tokio::test]
async fn any_environment_content_door_reads_its_job_but_never_another_persons_export() {
    let scene = Scene::new().await;
    let job = scene.job(kind::ENVIRONMENT_RECREATE, false).await;
    name_person(
        &scene.world,
        "environment",
        scene.environment.base.id,
        "browser.view",
        scene.people.viewer.id(),
    )
    .await;
    let full = scene
        .world
        .app
        .call(JobGet, &[&job])
        .token(&scene.people.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(full.log.as_deref(), Some("hidden-build-output"));
    let exported = scene.job(kind::EXPORT, false).await;
    assert_eq!(
        scene
            .world
            .app
            .call(JobGet, &[&exported])
            .token(&scene.people.viewer.token)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    scene
        .world
        .app
        .call(JobGet, &[&exported])
        .token(&scene.people.admin.token)
        .send()
        .await
        .ok();
    scene
        .world
        .app
        .call(JobGet, &[&job])
        .token(&scene.people.stranger.token)
        .send()
        .await
        .problem();
}

#[tokio::test]
async fn a_topic_and_its_environment_cards_agree_on_a_bound_thread_cut_and_hide_unrelated_environments()
 {
    let scene = Scene::new().await;
    let hidden = register(
        &scene.world,
        &scene.people,
        scene.machine,
        scene.topic,
        "hidden",
    )
    .await;
    private(&scene.world, hidden.base.id, scene.people.admin.id()).await;
    let cards = scene
        .world
        .app
        .call(TopicEnvironmentList, &[&scene.topic])
        .token(&scene.people.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].base.id, scene.environment.base.id);
    assert!(cards[0].workspace_root.is_empty() && cards[0].facts == json!({}));
    let node = scene
        .world
        .app
        .call(NodeGet, &[&scene.topic])
        .token(&scene.people.viewer.token)
        .send()
        .await
        .ok();
    let Node::Topic(topic) = node else {
        panic!("expected a topic");
    };
    assert_eq!(topic.environments, vec![cards[0].base.clone()]);
    assert!(
        !topic
            .environments
            .iter()
            .any(|environment| environment.id == hidden.base.id)
    );
    let all = scene
        .world
        .app
        .call(TopicEnvironmentList, &[&scene.topic])
        .token(&scene.people.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn explicit_topic_bindings_are_not_limited_to_the_first_global_resource_page() {
    let scene = Scene::new().await;
    // One beyond the public page maximum reproduces a truncation through the global resource list.
    let count = usize::try_from(orch_api_contract::common::MAX_LIMIT).unwrap() + 1;
    for index in 1..count {
        register(
            &scene.world,
            &scene.people,
            scene.machine,
            scene.topic,
            &format!("env-{index:03}"),
        )
        .await;
    }
    let cards = scene
        .world
        .app
        .call(TopicEnvironmentList, &[&scene.topic])
        .token(&scene.people.admin.token)
        .send()
        .await
        .ok();
    assert_eq!(cards.len(), count);
    let node = scene
        .world
        .app
        .call(NodeGet, &[&scene.topic])
        .token(&scene.people.admin.token)
        .send()
        .await
        .ok();
    let Node::Topic(topic) = node else {
        panic!("expected a topic");
    };
    assert_eq!(topic.environments.len(), count);
}

#[tokio::test]
async fn summary_reach_counts_the_complete_audience_once_per_person() {
    let scene = Scene::new().await;
    name_person(
        &scene.world,
        "environment",
        scene.environment.base.id,
        "files.view",
        scene.people.viewer.id(),
    )
    .await;
    name_person(
        &scene.world,
        "environment",
        scene.environment.base.id,
        "browser.view",
        scene.people.viewer.id(),
    )
    .await;
    name_person(
        &scene.world,
        "node",
        scene.thread,
        "thread.interact",
        scene.people.viewer.id(),
    )
    .await;
    let reach = scene
        .world
        .app
        .call(EnvironmentReachGet, &[&scene.environment.base.id])
        .token(&scene.people.admin.token)
        .send()
        .await
        .ok();
    let environment = scene.read(&scene.people.admin).await;
    assert_eq!(
        environment.reach,
        i64::try_from(reach.people.len()).unwrap() + reach.hidden
    );
    assert_eq!(
        reach
            .people
            .iter()
            .filter(|person| person.user.id == scene.people.viewer.id())
            .count(),
        1
    );
    assert!(environment.reach > 1, "owners alone are not the audience");
}
