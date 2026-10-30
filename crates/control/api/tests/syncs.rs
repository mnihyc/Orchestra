//! The state behind the feed of a topic and the feed of an environment over HTTP: the row itself, the threads and jobs and announcements of a topic, the jobs about an environment, from the start or from a number, what a caller may not see left out, and an answer that says it is complete only when it is.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod rig;
mod support;

use chrono::{DateTime, Utc};
use orch_api_contract::access::{AccessPreset, ThreadAccess};
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{
    Environment, EnvironmentSyncGet, EnvironmentSyncGetQuery, EnvironmentSyncGetReply,
    MachineEnvironmentCreate, MachineEnvironmentCreateBody,
};
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, JobState, ThreadCreate};
use orch_api_contract::topics::{
    AnnouncementCreateBody, NodeAnnouncementCreate, TopicSync, TopicSyncGet, TopicSyncGetQuery,
};
use orch_db::queries::jobs::{self as job_rows, NewJob, kind};
use orch_link_contract::policy::LocalPolicy;
use orch_types::{Id, new_id};
use places::Who;
use resources::company;
use rig::{MODEL, Scene};
use serde_json::json;
use support::World;

// ---------------------------------------------------------------- an environment

async fn world() -> World {
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([7; 32]),
    );
    orch_api_core::routes::settings::install_master_key(orch_db::secrets::MasterKey::new([7; 32]));
    World::new().await
}

/// A machine of `owner`'s whose local policy lets environments be under `/srv/ada`.
async fn machine(world: &World, owner: &Who) -> Id {
    let mut conn = world.conn().await;
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut conn,
        owner.id(),
        "laptop",
    )
    .await
    .unwrap();
    let policy = LocalPolicy {
        environment_roots: vec!["/srv/ada".to_owned()],
        ..LocalPolicy::default()
    };
    sqlx::query(
        "UPDATE machines SET facts = jsonb_build_object('policy', $2::jsonb) WHERE id = $1",
    )
    .bind(machine)
    .bind(serde_json::to_value(&policy).unwrap())
    .execute(&mut *conn)
    .await
    .unwrap();
    machine
}

/// The admin's environment, a directory of their machine.
async fn environment(world: &World) -> (resources::Company, Environment) {
    let c = company(world).await;
    let laptop = machine(world, &c.admin).await;
    let made = world
        .app
        .call(MachineEnvironmentCreate, &[&laptop])
        .token(&c.admin.token)
        .body(&MachineEnvironmentCreateBody {
            data_sources: None,
            home_id: c.acme,
            name: "Work".to_owned(),
            root: "/srv/ada/work".to_owned(),
            home_node_id: None,
            remote_write: None,
            terminal: None,
            access: None,
        })
        .send()
        .await
        .ok();
    (c, made)
}

async fn make_job(
    world: &World,
    environment: Option<Id>,
    thread: Option<Id>,
    word: &str,
    at: DateTime<Utc>,
) -> Id {
    let mut tx = world.db.pool.begin().await.unwrap();
    let row = job_rows::insert(
        &mut tx,
        &NewJob {
            id: new_id(),
            kind: word,
            node_id: thread,
            environment_id: environment,
            provider_id: None,
            plan: &json!({}),
            progress: &json!({}),
            created_by: None,
        },
        at,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    row.id
}

async fn end_job(world: &World, id: Id, at: DateTime<Utc>) {
    let mut conn = world.conn().await;
    job_rows::finish(&mut conn, id, "succeeded", &json!({}), None, None, at)
        .await
        .unwrap();
}

async fn sync_environment(
    world: &World,
    by: &Who,
    environment: Id,
    since: Option<i64>,
) -> orch_api_core::testing::Reply<EnvironmentSyncGetReply> {
    world
        .app
        .call(EnvironmentSyncGet, &[&environment])
        .token(&by.token)
        .query(&EnvironmentSyncGetQuery { since })
        .send()
        .await
}

fn ids_of<T: serde::Serialize>(jobs: &[T]) -> Vec<Id> {
    jobs.iter()
        .map(|job| {
            serde_json::from_value(serde_json::to_value(job).unwrap()["id"].clone()).unwrap()
        })
        .collect()
}

#[tokio::test]
async fn an_environments_state_is_the_environment_and_the_jobs_under_way_that_make_or_change_it() {
    let world = world().await;
    let (c, made) = environment(&world).await;
    let id = made.base.id;
    let now = world.now();
    let provision = make_job(&world, Some(id), None, kind::ENVIRONMENT_PROVISION, now).await;
    let recreate = make_job(&world, Some(id), None, kind::ENVIRONMENT_RECREATE, now).await;
    let moved = make_job(&world, Some(id), None, kind::ENVIRONMENT_MOVE, now).await;
    end_job(&world, moved, now).await;
    // A job about a thread's files is on the feed of the environment too, and is no part of its state.
    make_job(&world, Some(id), None, kind::SNAPSHOT, now).await;

    let reply = sync_environment(&world, &c.admin, id, None).await.ok();
    assert_eq!(reply.environment.base.id, id);
    assert_eq!(reply.since, None);
    assert!(reply.complete);
    assert_eq!(
        ids_of(&reply.jobs),
        [provision, recreate],
        "the jobs under way, in the order they were stamped"
    );
    assert_eq!(
        reply.seq, reply.environment.seq,
        "the answer ends where the environment says its feed stands"
    );
    assert!(reply.jobs.iter().all(|job| matches!(job, orch_api_contract::environment::EnvironmentSyncJob::Full(job) if job.environment_id == Some(id))));
}

#[tokio::test]
async fn from_a_number_on_the_jobs_are_those_stamped_after_it_whether_they_are_over_or_not() {
    let world = world().await;
    let (c, made) = environment(&world).await;
    let id = made.base.id;
    let now = world.now();
    let first = make_job(&world, Some(id), None, kind::ENVIRONMENT_PROVISION, now).await;
    let held = sync_environment(&world, &c.admin, id, None).await.ok();
    assert_eq!(ids_of(&held.jobs), [first]);

    // The job ends, and another begins, after the client's number.
    end_job(&world, first, now).await;
    let second = make_job(&world, Some(id), None, kind::ENVIRONMENT_REMOVE, now).await;
    let news = sync_environment(&world, &c.admin, id, Some(held.seq))
        .await
        .ok();
    assert_eq!(news.since, Some(held.seq));
    assert!(news.complete);
    assert_eq!(ids_of(&news.jobs), [first, second]);
    assert!(
        matches!(&news.jobs[0], orch_api_contract::environment::EnvironmentSyncJob::Full(job) if job.state == JobState::Succeeded)
    );
    assert!(news.seq > held.seq && news.environment.seq == news.seq);

    // Asked from where the feed stands, nothing is new but the environment is there.
    let nothing = sync_environment(&world, &c.admin, id, Some(news.seq))
        .await
        .ok();
    assert!(nothing.jobs.is_empty() && nothing.complete);
    assert_eq!(nothing.environment.base.id, id);
}

#[tokio::test]
async fn an_answer_that_says_it_is_complete_holds_every_job_stamped_after_the_number() {
    let world = world().await;
    let (c, made) = environment(&world).await;
    let id = made.base.id;
    let now = world.now();
    let mut all = Vec::new();
    for _ in 0..250 {
        all.push(make_job(&world, Some(id), None, kind::ENVIRONMENT_RECREATE, now).await);
    }
    let top = sync_environment(&world, &c.admin, id, None).await.ok();

    // More changed than a request holds: the answer says so and hands over none.
    let too_far = sync_environment(&world, &c.admin, id, Some(0)).await.ok();
    assert!(!too_far.complete);
    assert!(too_far.jobs.is_empty());
    assert_eq!(too_far.environment.base.id, id);

    // From a number near the end the answer is complete, and holds exactly what came after it.
    let near = sync_environment(&world, &c.admin, id, Some(top.seq - 40))
        .await
        .ok();
    assert!(near.complete);
    assert_eq!(
        ids_of(&near.jobs),
        &all[all.len() - 40..],
        "the last forty, none left out"
    );
}

#[tokio::test]
async fn a_number_above_the_feeds_own_is_no_place_to_go_on_from() {
    let world = world().await;
    let (c, made) = environment(&world).await;
    let id = made.base.id;
    make_job(
        &world,
        Some(id),
        None,
        kind::ENVIRONMENT_PROVISION,
        world.now(),
    )
    .await;
    let now_at = sync_environment(&world, &c.admin, id, None).await.ok();
    let restored = sync_environment(&world, &c.admin, id, Some(now_at.seq + 100))
        .await
        .ok();
    assert!(
        !restored.complete,
        "a database that was restored is read from the start"
    );
    assert_eq!(restored.since, Some(now_at.seq + 100));
    assert_eq!(restored.environment.base.id, id);
}

#[tokio::test]
async fn only_whoever_may_read_an_environment_has_its_state() {
    let world = world().await;
    let (c, made) = environment(&world).await;
    let stranger = places::person(&world, "sam").await;
    let refused = sync_environment(&world, &stranger, made.base.id, None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        sync_environment(&world, &c.admin, new_id(), None)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let anonymous = world
        .app
        .call(EnvironmentSyncGet, &[&made.base.id])
        .query(&EnvironmentSyncGetQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(anonymous.code, ErrorCode::AuthRequired);
    let negative = sync_environment(&world, &c.admin, made.base.id, Some(-1))
        .await
        .problem();
    assert_eq!(negative.code, ErrorCode::RequestInvalid);
}

// ---------------------------------------------------------------- a topic

async fn sync_topic(
    s: &Scene,
    by: &Who,
    since: Option<i64>,
) -> orch_api_core::testing::Reply<TopicSync> {
    s.rig
        .world
        .app
        .call(TopicSyncGet, &[&s.tree.topic.base.id])
        .token(&by.token)
        .query(&TopicSyncGetQuery { since })
        .send()
        .await
}

async fn announce(s: &Scene, title: &str) {
    s.rig
        .world
        .app
        .call(NodeAnnouncementCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&AnnouncementCreateBody {
            title: title.to_owned(),
            body: "words".to_owned(),
            category: None,
            severity: None,
            targets: None,
            delivery: None,
            fallback: None,
            wake_idle: None,
        })
        .send()
        .await
        .ok();
}

/// A thread of the topic made by `by` that only they read.
async fn private_thread(s: &Scene, by: &Who, name: &str) -> Id {
    s.rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&by.token)
        .body(&CreateThread {
            name: Some(name.to_owned()),
            access: Some(ThreadAccess::Preset(AccessPreset::Private)),
            binding: Some(CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: s.on.provider,
                arrangement: None,
            }),
            model: Some(MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
        .base
        .id
}

#[tokio::test]
async fn a_topics_state_is_the_topic_its_threads_the_jobs_about_them_and_its_announcements() {
    let s = Scene::new().await;
    let mine = s.thread().await;
    let eves = s.thread_by(&s.eve, "Review").await;
    let now = s.rig.world.now();
    let job = make_job(&s.rig.world, None, Some(mine.base.id), kind::SNAPSHOT, now).await;
    announce(&s, "Freeze").await;

    let reply = sync_topic(&s, &s.cai, None).await.ok();
    assert_eq!(reply.topic.base.id, s.tree.topic.base.id);
    assert_eq!(reply.seq, reply.topic.seq);
    assert!(reply.complete && reply.since.is_none());
    let mut threads: Vec<Id> = reply.threads.iter().map(|thread| thread.base.id).collect();
    threads.sort();
    let mut wanted = vec![mine.base.id, eves.base.id];
    wanted.sort();
    assert_eq!(threads, wanted, "a viewer of the topic sees its threads");
    assert_eq!(ids_of(&reply.jobs), [job]);
    assert_eq!(reply.announcements.len(), 1);
    assert_eq!(reply.announcements[0].title, "Freeze");
}

#[tokio::test]
async fn a_thread_the_caller_may_not_view_is_left_out_with_its_jobs() {
    let s = Scene::new().await;
    let open = s.thread().await;
    let closed = private_thread(&s, &s.eve, "Quiet").await;
    let now = s.rig.world.now();
    let about_open = make_job(&s.rig.world, None, Some(open.base.id), kind::SNAPSHOT, now).await;
    let about_closed = make_job(&s.rig.world, None, Some(closed), kind::SNAPSHOT, now).await;

    let cai = sync_topic(&s, &s.cai, None).await.ok();
    assert_eq!(
        cai.threads
            .iter()
            .map(|thread| thread.base.id)
            .collect::<Vec<_>>(),
        [open.base.id]
    );
    assert_eq!(ids_of(&cai.jobs), [about_open]);
    let eve = sync_topic(&s, &s.eve, None).await.ok();
    assert_eq!(eve.threads.len(), 2);
    let mut jobs = ids_of(&eve.jobs);
    jobs.sort();
    let mut both = vec![about_open, about_closed];
    both.sort();
    assert_eq!(
        jobs, both,
        "its maker sees the thread and what is done to it"
    );
    // Whoever may not view the topic has none of it.
    let refused = sync_topic(&s, &s.bea, None).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn from_a_number_on_a_topic_tells_the_announcements_and_the_jobs_after_it_and_all_its_threads()
 {
    let s = Scene::new().await;
    let thread = s.thread().await;
    announce(&s, "First").await;
    let held = sync_topic(&s, &s.ada, None).await.ok();
    assert_eq!(held.announcements.len(), 1);

    let now = s.rig.world.now();
    let job = make_job(
        &s.rig.world,
        None,
        Some(thread.base.id),
        kind::SNAPSHOT,
        now,
    )
    .await;
    end_job(&s.rig.world, job, now).await;
    announce(&s, "Second").await;
    let news = sync_topic(&s, &s.ada, Some(held.seq)).await.ok();
    assert!(news.complete && news.since == Some(held.seq));
    assert!(news.seq > held.seq);
    assert_eq!(
        news.announcements
            .iter()
            .map(|each| each.title.as_str())
            .collect::<Vec<_>>(),
        ["Second"]
    );
    assert_eq!(ids_of(&news.jobs), [job]);
    assert_eq!(news.jobs[0].state, JobState::Succeeded);
    assert_eq!(
        news.threads
            .iter()
            .map(|each| each.base.id)
            .collect::<Vec<_>>(),
        [thread.base.id],
        "a thread's own state is not stamped on the topic's number, so every thread is told again"
    );
    assert_eq!(news.topic.base.id, s.tree.topic.base.id);

    let nothing = sync_topic(&s, &s.ada, Some(news.seq)).await.ok();
    assert!(nothing.complete && nothing.announcements.is_empty() && nothing.jobs.is_empty());
}

#[tokio::test]
async fn an_answer_about_a_topic_that_says_it_is_complete_holds_every_announcement_after_the_number()
 {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let pool = &s.rig.world.db.pool;
    let start: i64 = sqlx::query_scalar("SELECT topic_seq FROM topics WHERE node_id = $1")
        .bind(topic)
        .fetch_one(pool)
        .await
        .unwrap();
    // Two hundred and fifty announcements, numbered after the topic's number as the engine does.
    sqlx::query(
        "INSERT INTO announcements (id, topic_id, from_user_id, category, title, body, severity, delivery,
                                    fallback, wake_idle, targets, topic_seq)
         SELECT gen_random_uuid(), $1, $2, 'notice', 'n' || i, 'x', 'info', 'next_turn', 'next_turn',
                false, 'all', $3 + i
           FROM generate_series(1, 250) AS i",
    )
    .bind(topic)
    .bind(s.ada.id())
    .bind(start)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE topics SET topic_seq = topic_seq + 250 WHERE node_id = $1")
        .bind(topic)
        .execute(pool)
        .await
        .unwrap();

    let first = sync_topic(&s, &s.ada, None).await.ok();
    assert_eq!(
        first.announcements.len(),
        20,
        "a first load holds the last twenty"
    );
    assert_eq!(first.announcements[0].title, "n250", "newest first");

    let too_far = sync_topic(&s, &s.ada, Some(start)).await.ok();
    assert!(!too_far.complete, "more than a request holds changed");
    assert!(too_far.announcements.is_empty());

    let near = sync_topic(&s, &s.ada, Some(first.seq - 30)).await.ok();
    assert!(near.complete);
    assert_eq!(near.announcements.len(), 30);
    assert_eq!(near.announcements[29].title, "n221");

    let restored = sync_topic(&s, &s.ada, Some(first.seq + 5)).await.ok();
    assert!(!restored.complete && restored.since == Some(first.seq + 5));
}

#[tokio::test]
async fn what_is_not_a_topic_has_no_topic_state() {
    let s = Scene::new().await;
    let thread = s.thread().await;
    for node in [thread.base.id, s.tree.project.base.id, new_id()] {
        let reply = s
            .rig
            .world
            .app
            .call(TopicSyncGet, &[&node])
            .token(&s.ada.token)
            .query(&TopicSyncGetQuery::default())
            .send()
            .await;
        assert_eq!(reply.status, 404, "{node}");
    }
    assert_eq!(
        sync_topic(&s, &s.ada, Some(-5)).await.problem().code,
        ErrorCode::RequestInvalid
    );
}
