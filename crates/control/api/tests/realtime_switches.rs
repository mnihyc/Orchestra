//! An open thread receives switch holds, progress, the new provider and its next turn without reloading or reconnecting.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use orch_api_contract::forks::{ForkCreateBody, ForkRequest};
use orch_authz::Caller;
use orch_db::queries::jobs::{self, forks};
use orch_engine::jobs::runner::{Phase, initial_progress};
use orch_types::{Id, Json, new_id};
use realtime_support::{Client, Heard, Mirror, PATIENCE, Rig};
use serde_json::json;
use std::time::{Duration, Instant};

struct Page {
    mirror: Mirror,
    thread: Json,
    completed: Vec<Id>,
}
impl Page {
    async fn follow(&mut self, client: &mut Client, done: impl Fn(&Self) -> bool) {
        let began = Instant::now();
        while !done(self) {
            assert!(
                began.elapsed() < PATIENCE,
                "an existing page missed committed switch facts: {}",
                self.thread
            );
            let Some(Heard::Frame(frame)) = client.hear_within(Duration::from_millis(200)).await
            else {
                continue;
            };
            assert!(
                !matches!(
                    frame["op"].as_str(),
                    Some("resync" | "feed_closed" | "error")
                ),
                "{frame}"
            );
            self.mirror.take(&frame);
            if frame["op"] == "sync" {
                if frame["data"]["thread"].is_object() {
                    self.thread = frame["data"]["thread"].clone();
                }
                for turn in frame["data"]["turns"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|turn| turn["state"] == "completed")
                {
                    self.completed
                        .push(turn["id"].as_str().unwrap().parse().unwrap());
                }
            }
        }
        assert_eq!(self.mirror.resyncs, 0);
        assert_eq!(self.mirror.gaps, 0);
    }
}

async fn opened(rig: &Rig, thread: Id) -> (Client, Page) {
    let before = rig.rows(thread).await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client
        .sub(1, &format!("thread:{thread}"), Some(before.seq))
        .await;
    client.expect("subscribed").await;
    client.expect("presence").await;
    (
        client,
        Page {
            mirror: Mirror::loaded(before.seq),
            thread: Json::Null,
            completed: Vec::new(),
        },
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_job_publishes_its_hold_progress_and_completion_on_the_threads_own_feed() {
    let rig = Rig::new().await;
    let thread = rig.thread("Watch the switch job").await;
    let (mut client, mut page) = opened(&rig, thread).await;
    let now = rig.clock.now();
    let job = new_id();
    // Another durable runner owns this fixture until well after the test. Only the real job
    // queries below drive its visible progress; the engine must not race to execute a fake recipe.
    let plan =
        json!({"result":"replace","runner":{"id":new_id(),"until":now+chrono::Duration::hours(1)}});
    let mut progress = initial_progress(&[Phase {
        name: "open",
        weight: 1,
    }]);
    let mut tx = rig.pool.begin().await.unwrap();
    forks::insert(
        &mut tx,
        &forks::NewForkJob {
            id: job,
            thread,
            environment: Some(rig.environment),
            batch: None,
            plan: &plan,
            progress: &serde_json::to_value(&progress).unwrap(),
            created_by: rig.ada,
        },
        now,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    page.follow(&mut client, |page| {
        page.thread["lock"]["id"] == job.to_string() && page.thread["lock"]["percent"] == 0
    })
    .await;
    progress.percent = 40;
    let mut tx = rig.pool.begin().await.unwrap();
    assert!(
        jobs::save_progress(
            &mut tx,
            job,
            &serde_json::to_value(&progress).unwrap(),
            rig.clock.now()
        )
        .await
        .unwrap()
    );
    tx.commit().await.unwrap();
    page.follow(&mut client, |page| page.thread["lock"]["percent"] == 40)
        .await;
    progress.percent = 100;
    let mut tx = rig.pool.begin().await.unwrap();
    assert!(
        jobs::finish(
            &mut tx,
            job,
            "succeeded",
            &serde_json::to_value(&progress).unwrap(),
            None,
            None,
            rig.clock.now()
        )
        .await
        .unwrap()
    );
    tx.commit().await.unwrap();
    page.follow(&mut client, |page| {
        page.thread.is_object() && page.thread["lock"].is_null()
    })
    .await;
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_switch_and_the_first_turn_after_it_reach_the_existing_thread_subscriber() {
    let rig = Rig::new().await;
    let thread = rig.thread("Watch the new provider").await;
    let other = new_id();
    let mut conn = rig.pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO providers (id,name,owner_id,org_id,home_id,kind,harness_kind,agent_id,machine_id,arrangements,status)
        VALUES ($1,'Other subscription',$2,$3,$3,'subscription','fake',$4,$5,ARRAY['separated'],'ready')")
        .bind(other).bind(rig.ada).bind(rig.root).bind(rig.agent).bind(rig.machine_id).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO provider_models (id,provider_id,model_name,display_name,parameters,enabled,source,created_by)
        SELECT $1,$2,model_name,display_name,parameters,enabled,source,created_by FROM provider_models WHERE provider_id=$3")
        .bind(new_id()).bind(other).bind(rig.provider).execute(&mut *conn).await.unwrap();
    // Its page is started as the rig's own provider's is: Ada is in its Owners row, and so uses it.
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ResourceKind::Provider.into(),
            id: other,
        },
        rig.ada,
        orch_authz::ownership::NewAccess::Defaults,
        orch_types::Time::new(rig.clock.now()),
    )
    .await
    .unwrap();
    drop(conn);
    let (mut client, mut page) = opened(&rig, thread).await;
    let caller = Caller::session(rig.ada);
    let request: ForkRequest = serde_json::from_value(
        json!({"result":"replace","history":{"mode":"none"},"to":{"provider_id":other}}),
    )
    .unwrap();
    let draft = rig
        .server
        .engine
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert!(draft.is_clear(), "{draft:?}");
    let plan = rig
        .server
        .engine
        .keep_fork_plan(&caller, draft)
        .await
        .unwrap();
    rig.server
        .engine
        .confirm_fork(
            &caller,
            thread,
            &ForkCreateBody {
                notify: None,
                preflight_id: plan.preflight_id,
                options: None,
            },
        )
        .await
        .unwrap();
    page.follow(&mut client, |page| {
        page.thread["binding"]["provider"]["id"] == other.to_string()
            && page.thread["state"] == "idle"
            && page.thread["lock"].is_null()
    })
    .await;
    let turn = rig.send(thread, "Use the new provider").await;
    rig.begin_message(thread, &turn, "after-switch").await;
    rig.finish_turn(thread, &turn, "after-switch", "The new provider answered")
        .await;
    page.follow(&mut client, |page| {
        page.completed.contains(&turn.turn_id)
            && page
                .mirror
                .text
                .values()
                .any(|text| text == "The new provider answered")
    })
    .await;
    assert_eq!(page.thread["binding"]["provider"]["id"], other.to_string());
    rig.server.stop().await;
}
