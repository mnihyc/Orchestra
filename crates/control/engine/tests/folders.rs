//! What an agent announces to its project reaches the topics of the folders inside it, and passes over a folder that was made private.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
// A call is admitted only from the thread's running turn, which the shared fixture starts.
mod shared;

use chrono::Utc;
use common::{MODEL, World, queued};
use orch_authz::Caller;
use orch_db::queries::access::{PageKey, testing::set_row};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::usage::ledger::{self, Attribution, NewEvent, Tokens};
use orch_link_contract::commands::OrchTool;
use orch_link_contract::events::{Event, LeaseUsage};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use shared::{a_world_whose_agent_takes_tools, call};

fn sorted(mut ids: Vec<Id>) -> Vec<Id> {
    ids.sort();
    ids
}

async fn topics_told(world: &World, group: Id) -> Vec<Id> {
    sorted(
        sqlx::query_scalar("SELECT topic_id FROM announcements WHERE group_id = $1")
            .bind(group)
            .fetch_all(world.pool.pool())
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn an_agents_announcement_to_its_project_reaches_the_topics_of_the_folders_inside_it_and_passes_over_a_private_one()
 {
    let world = a_world_whose_agent_takes_tools().await;
    world
        .set(world.root, "broadcast.agent_scope", json!("project"))
        .await;
    let sender = world.thread("Build").await;
    let beside = world.topic("Elsewhere").await;
    // A folder in the project with a folder in it, a topic in each; and a private folder with a topic.
    let [checkout, payments, hidden, cart, refunds, secret] = std::array::from_fn(|_| new_id());
    {
        let mut conn = world.pool.acquire().await.unwrap();
        for (id, kind, parent, name) in [
            (checkout, "project", world.project, "Checkout"),
            (payments, "project", checkout, "Payments"),
            (hidden, "project", world.project, "Hidden"),
            (cart, "topic", checkout, "Cart"),
            (refunds, "topic", payments, "Refunds"),
            (secret, "topic", hidden, "Secret"),
        ] {
            insert_node(&mut conn, id, kind, Some(parent), name, world.ada)
                .await
                .unwrap();
        }
        // Hidden was made private: its own page and the defaults of what lies in it are cut loose.
        set_row(
            &mut conn,
            &PageKey::new("node", hidden, "live"),
            "any",
            "project.view",
            false,
            false,
        )
        .await
        .unwrap();
        set_row(
            &mut conn,
            &PageKey::new("node", hidden, "defaults"),
            "any",
            "topic.view",
            false,
            false,
        )
        .await
        .unwrap();
    }
    let result = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "To all", "audience": "project" }),
    )
    .await;
    assert!(result.error.is_none(), "{result:?}");
    let group: Id =
        serde_json::from_value(result.result.expect("a result")["announcement_id"].clone())
            .unwrap();
    assert_eq!(
        topics_told(&world, group).await,
        sorted(vec![world.topic, beside, cart, refunds]),
        "the project's topics and the folders' are told, and what lies in the private folder is not"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_on_a_project_covers_the_work_in_the_folders_inside_it_and_a_cap_on_a_folder_beside_does_not()
 {
    let world = World::new().await;
    let (folder, beside, cart) = (new_id(), new_id(), new_id());
    {
        let mut conn = world.pool.acquire().await.unwrap();
        for (id, kind, parent, name) in [
            (folder, "project", world.project, "Checkout"),
            (beside, "project", world.project, "Docs"),
            (cart, "topic", folder, "Cart"),
        ] {
            insert_node(&mut conn, id, kind, Some(parent), name, world.ada)
                .await
                .unwrap();
        }
    }
    // A thread in the topic of the folder.
    let thread = world
        .engine()
        .create_thread(
            &Caller::session(world.ada),
            orch_engine::binding::NewThread {
                topic: cart,
                name: "Build".to_owned(),
                agent: world.agent,
                provider: world.provider,
                environment: world.environment,
                directory: "web".to_owned(),
                arrangement: None,
                model: Some(MODEL.to_owned()),
                effort: None,
            },
        )
        .await
        .expect("a thread is created")
        .thread;
    // The folder's use is put down to the folder; a cap on the folder beside it has nothing to say
    // about it, and one on the project above it covers it.
    let place = Attribution {
        org_id: world.root,
        project_id: folder,
        topic_id: cart,
        thread_id: thread,
        segment_id: new_id(),
        turn_id: new_id(),
        provider_id: world.provider,
        entry_id: None,
        user_id: Some(world.ada),
        agent_id: Some(world.agent),
        harness_kind: Some("fake".to_owned()),
        environment_id: Some(world.environment),
        environment_kind: Some("managed".to_owned()),
        arrangement: Some("together".to_owned()),
        turn_model: MODEL.to_owned(),
        effort: None,
        params: Json::Null,
    };
    {
        let mut conn = world.pool.acquire().await.unwrap();
        ledger::record(
            &mut conn,
            &NewEvent {
                id: new_id(),
                at: Utc::now(),
                origin: ledger::origin::FORWARDER,
                scope: ledger::scope::REQUEST,
                counted: true,
                attribution: &place,
                model: MODEL,
                tier: None,
                tokens: Tokens {
                    input: 1_000,
                    cached_input: 0,
                    cache_write: 0,
                    output: 0,
                    reasoning: 0,
                    total: 1_000,
                },
                context_tokens: None,
                request_ref: None,
                rate: None,
                rate_source: None,
                credits: None,
            },
        )
        .await
        .unwrap();
    }
    world.cap(Some(beside), Some(10), true, &[], &[]).await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("First"))
        .await
        .unwrap();
    let first = world.run_turn(thread, 0, "Done.").await;
    assert_eq!(
        common::words_after_where(&first.text),
        "First",
        "a cap on another folder covers nothing here"
    );
    world.wait_state(thread, "idle").await;

    world
        .cap(Some(world.project), Some(1_000), true, &[], &[])
        .await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Second"))
        .await
        .unwrap();
    world.wait_blocked(thread, "quota_exceeded").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_use_of_a_thread_in_a_folder_is_put_down_to_the_folder_and_not_to_a_project_above_it() {
    let world = World::on_a_key("sk-orch-test-folders").await;
    let (folder, cart) = (new_id(), new_id());
    {
        let mut conn = world.pool.acquire().await.unwrap();
        insert_node(
            &mut conn,
            folder,
            "project",
            Some(world.project),
            "Checkout",
            world.ada,
        )
        .await
        .unwrap();
        insert_node(&mut conn, cart, "topic", Some(folder), "Cart", world.ada)
            .await
            .unwrap();
    }
    let thread = world
        .engine()
        .create_thread(
            &Caller::session(world.ada),
            orch_engine::binding::NewThread {
                topic: cart,
                name: "Build".to_owned(),
                agent: world.agent,
                provider: world.provider,
                environment: world.environment,
                directory: "web".to_owned(),
                arrangement: None,
                model: Some(MODEL.to_owned()),
                effort: None,
            },
        )
        .await
        .expect("a thread is created")
        .thread;
    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;
    world
        .say_of_provider(Event::LeaseUsage(LeaseUsage {
            lease_id: lease,
            model: MODEL.to_owned(),
            usage: serde_json::from_value(json!({
                "input_tokens": 1_000, "cached_input_tokens": 0, "output_tokens": 100,
                "reasoning_tokens": 0, "total_tokens": 1_100
            }))
            .unwrap(),
            request_ref: Some("resp_1".to_owned()),
        }))
        .await;
    let rows = world.wait_ledger(start.turn_id, 1).await;
    let row = rows.first().expect("a row of the ledger");
    assert_eq!(
        (row.org_id, row.project_id, row.topic_id),
        (world.root, folder, cart),
        "the project a row names is the one the topic lies in directly"
    );
}
