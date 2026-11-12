//! A completed fork is found before consuming its plan or creating another job.
use super::*;
use orch_api_contract::forks::{ForkNowBody, ThreadForkNow};
use orch_api_core::testing::RawReply;
use thread_history_scope::{Proof, key, rule};

fn value(reply: &RawReply) -> Json {
    assert_eq!(
        reply.status.as_u16(),
        202,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn side_header_replay_uses_current_child_read_or_its_original_identity_receipt_without_spent_parent_write()
-> Proof {
    let w = Forks::new().await;
    let parent = w.thread_with_history("One side retry").await;
    w.s.rig.world.app.app.engine.stop();
    let token = orch_types::new_id().to_string();
    let input = ThreadSideCreateBody {
        content: vec![ContentPart::Text {
            text: "An aside accepted once".into(),
        }],
        client_id: orch_types::new_id().to_string(),
        to_turn: None,
    };
    let send = |secret: String| {
        let app = w.s.rig.world.app.clone();
        let input = input.clone();
        let token = token.clone();
        async move {
            let call = app
                .call(ThreadSideCreate, &[&parent])
                .token(&secret)
                .header("Idempotency-Key", &token)
                .body(&input);
            call.send_bytes().await
        }
    };
    let first = send(w.s.ada.token.clone()).await;
    assert_eq!(
        first.status.as_u16(),
        201,
        "{}",
        String::from_utf8_lossy(&first.raw)
    );
    let first: Json = serde_json::from_slice(&first.raw)?;
    let _child: Id = serde_json::from_value(first["thread"]["id"].clone())?;
    for operation in ["ThreadSideCreate", "NodeGet"] {
        let actor = key(
            &w.s.rig.world,
            w.s.ada.id(),
            vec![
                rule(operation, "org", w.s.tree.org.base.id, "subtree", "read"),
                rule(operation, "machine", w.s.rig.machine_id, "self", "read"),
            ],
        )
        .await?;
        let replay = send(actor.secret).await;
        assert_eq!(
            replay.status.as_u16(),
            201,
            "{}",
            String::from_utf8_lossy(&replay.raw)
        );
        let replay: Json = serde_json::from_slice(&replay.raw)?;
        if operation == "ThreadSideCreate" {
            assert_eq!(replay["thread"]["id"], first["thread"]["id"]);
            assert_eq!(replay["input"]["id"], first["input"]["id"]);
        } else {
            assert_eq!(replay["projection"], "receipt");
            assert_eq!(replay["thread_id"], first["thread"]["id"]);
            assert_eq!(replay["input_id"], first["input"]["id"]);
            assert!(replay.get("input").is_none());
        }
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM threads WHERE side_of=$1")
        .bind(parent)
        .fetch_one(w.s.rig.db.pool())
        .await?;
    assert_eq!(count, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn immediate_fork_replays_one_native_job_and_reserved_thread_under_current_read_or_receipt()
-> Proof {
    let w = Forks::new().await;
    let source = w.thread_with_history("Idempotent immediate source").await;
    w.s.rig.world.app.app.engine.stop();
    let token = orch_types::new_id().to_string();
    let input = ForkNowBody {
        name: Some("One reserved fork".into()),
        ..Default::default()
    };
    let send = |secret: String| {
        let app = w.s.rig.world.app.clone();
        let token = token.clone();
        let input = input.clone();
        async move {
            let call = app
                .call(ThreadForkNow, &[&source])
                .token(&secret)
                .header("Idempotency-Key", &token)
                .body(&input);
            call.send_bytes().await
        }
    };
    let first = value(&send(w.s.ada.token.clone()).await);
    let job = first["job"]["id"].clone();
    let reserved = first["thread_id"].clone();
    assert!(job.is_string() && reserved.is_string());
    let repeated = value(&send(w.s.ada.token.clone()).await);
    assert_eq!(repeated["job"]["id"], job);
    assert_eq!(repeated["thread_id"], reserved);
    let actor = key(
        &w.s.rig.world,
        w.s.ada.id(),
        vec![
            rule("ThreadForkNow", "thread", source, "self", "read"),
            rule(
                "ThreadForkNow",
                "environment",
                w.s.on.environment,
                "self",
                "read",
            ),
            rule("ThreadForkNow", "provider", w.s.on.provider, "self", "read"),
            rule(
                "ThreadForkNow",
                "machine",
                w.s.rig.machine_id,
                "self",
                "read",
            ),
        ],
    )
    .await?;
    let read = value(&send(actor.secret).await);
    assert_eq!(
        read["job"]["id"], job,
        "the completed act needs no spent Manage"
    );
    let actor = key(
        &w.s.rig.world,
        w.s.ada.id(),
        vec![rule("NodeGet", "thread", source, "self", "read")],
    )
    .await?;
    let receipt = value(&send(actor.secret).await);
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["job_id"], job);
    assert_eq!(receipt["thread_id"], reserved);
    assert!(receipt.get("job").is_none());
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await,
        1
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn confirmed_fork_replay_finds_the_first_job_after_its_private_plan_was_consumed() -> Proof {
    let w = Forks::new().await;
    let source = w.thread_with_history("Idempotent confirmed source").await;
    let plan = w.plan(&w.s.ada, source, &w.fork()).await.ok();
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    w.s.rig.world.app.app.engine.stop();
    let token = orch_types::new_id().to_string();
    let input = ForkCreateBody {
        preflight_id: plan.preflight_id,
        notify: None,
        options: None,
    };
    let send = || {
        let app = w.s.rig.world.app.clone();
        let secret = w.s.ada.token.clone();
        let token = token.clone();
        let input = input.clone();
        async move {
            let call = app
                .call(ThreadForkCreate, &[&source])
                .token(&secret)
                .header("Idempotency-Key", &token)
                .body(&input);
            call.send_bytes().await
        }
    };
    let first = value(&send().await);
    let repeated = value(&send().await);
    assert_eq!(first["id"], repeated["id"]);
    assert!(first["id"].is_string());
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await,
        1
    );
    Ok(())
}
