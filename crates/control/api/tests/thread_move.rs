//! A reviewed move keeps the conversation, its side paths, held work and workspace while its topic changes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{
    Node, NodeActivityList, NodeActivityListQuery, ProjectChildCreate, ProjectCreateBody,
};
use orch_api_contract::thread_move::{
    ThreadMove, ThreadMoveBody, ThreadMovePlan, ThreadMovePreview, ThreadMovePreviewBody,
};
use orch_api_contract::threads::{
    Delivery, ItemPayload, MarkerPayload, Thread, ThreadInterrupt, ThreadInterruptBody, ThreadSync,
    ThreadSyncGet, ThreadSyncGetQuery,
};
use orch_link_contract::events::{ApprovalRequested, Event, TurnOutcome};
use orch_settings::Target;
use orch_types::{Id, new_id};
use rig::{Scene, turn_began, turn_ended};
use serde_json::json;

async fn preview(s: &Scene, thread: Id, to: Id) -> ThreadMovePlan {
    s.rig
        .world
        .app
        .call(ThreadMovePreview, &[&thread])
        .token(&s.ada.token)
        .body(&ThreadMovePreviewBody { parent_id: to })
        .send()
        .await
        .ok()
}

fn confirmation(plan: &ThreadMovePlan) -> ThreadMoveBody {
    ThreadMoveBody {
        parent_id: plan.movement.to.id,
        expected_revision: plan.expected_revision,
        preflight_id: plan.movement.preflight_id,
    }
}

async fn sync(s: &Scene, thread: Id) -> ThreadSync {
    s.rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
}

async fn destination(s: &Scene) -> Id {
    let folder = s
        .rig
        .world
        .app
        .call(ProjectChildCreate, &[&s.tree.project.base.id])
        .token(&s.ada.token)
        .body(&ProjectCreateBody {
            name: "Payments".to_owned(),
            description: None,
            access: None,
        })
        .send()
        .await
        .ok();
    places::make_topic(&s.rig.world, &s.ada, folder.base.id, "Refunds")
        .await
        .base
        .id
}

// This is a retained idle lane, not a substitute for the native mechanism's conformance tests.
async fn retained_side(s: &Scene, main: Id) -> Id {
    let side = new_id();
    sqlx::query("INSERT INTO nodes (id,kind,parent_id,org_id,path,name,owner_id,created_by) SELECT $2,'thread',id,org_id,path||$2::uuid,'An aside',owner_id,created_by FROM nodes WHERE id=$1")
        .bind(main).bind(side).execute(s.rig.db.pool()).await.unwrap();
    sqlx::query("INSERT INTO threads (node_id,topic_id,environment_id,workspace_dir,agent_id,provider_id,arrangement,side_of,side_reading) SELECT $2,topic_id,environment_id,workspace_dir,agent_id,provider_id,arrangement,node_id,'read_only' FROM threads WHERE node_id=$1")
        .bind(main).bind(side).execute(s.rig.db.pool()).await.unwrap();
    s.rig
        .lets_use("node", side, "access.own", &s.ada, json!({}))
        .await;
    side
}

#[tokio::test]
async fn a_move_keeps_the_same_history_workspace_and_stop_held_inputs_and_moves_retained_side_paths()
 {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    s.exchange(id, 0, "Find the bug", "The failing test is in web/login.ts")
        .await;
    s.say(&s.ada, id, "Fix it", Delivery::Queue, "running")
        .await
        .ok();
    let active = s.rig.turn_start(id, 1).await;
    s.rig.say(id, turn_began(active.turn_id)).await;
    s.rig.wait_turn(active.turn_id, "running").await;
    s.say(&s.ada, id, "Check the result", Delivery::Queue, "held")
        .await
        .ok();
    s.rig
        .world
        .app
        .call(ThreadInterrupt, &[&id])
        .token(&s.ada.token)
        .body(&ThreadInterruptBody {
            turn_id: active.turn_id,
        })
        .send()
        .await
        .ok();
    s.rig
        .say(id, turn_ended(active.turn_id, TurnOutcome::Interrupted))
        .await;
    s.rig.wait_turn(active.turn_id, "interrupted").await;
    s.rig.wait_state(id, "idle").await;
    let before = sync(&s, id).await;
    assert!(before.thread.queue.held);
    let side = retained_side(&s, id).await;
    let to = destination(&s).await;
    let home: Id = sqlx::query_scalar("SELECT home_node_id FROM environments WHERE id=$1")
        .bind(s.on.environment)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let opens = s.rig.commands.issued_of("session.open");
    let starts = s.rig.commands.issued_of("turn.start");
    let plan = preview(&s, id, to).await;
    assert!(plan.movement.blockers.is_empty());
    assert_eq!(plan.unchanged.owner.id, s.ada.id());
    assert_eq!(
        plan.unchanged.environment.as_ref().unwrap().id,
        s.on.environment
    );
    assert_eq!(plan.unchanged.directory, "web");
    assert_eq!(
        sync(&s, id).await.thread.topic_id,
        s.tree.topic.base.id,
        "review does not move anything"
    );
    let moved: Thread = s
        .rig
        .world
        .app
        .call(ThreadMove, &[&id])
        .token(&s.ada.token)
        .body(&confirmation(&plan))
        .send()
        .await
        .ok();
    assert_eq!(moved.base.id, id);
    assert_eq!(moved.topic_id, to);
    assert_eq!(moved.base.owner, before.thread.base.owner);
    assert_eq!(moved.environment, before.thread.environment);
    assert_eq!(moved.directory, before.thread.directory);
    assert_eq!(moved.binding, before.thread.binding);
    assert_eq!(moved.queue, before.thread.queue);
    let after = sync(&s, id).await;
    assert_eq!(
        after.inputs, before.inputs,
        "held authors, ids, content and order stay"
    );
    assert_eq!(after.turns, before.turns);
    for item in &before.items {
        assert!(
            after
                .items
                .iter()
                .any(|kept| kept.id == item.id && kept.payload == item.payload)
        );
    }
    assert!(after.items.iter().any(|item| matches!(&item.payload, ItemPayload::Marker(MarkerPayload::ThreadMoved(marker)) if marker.from.id == s.tree.topic.base.id && marker.to.id == to && marker.by.id == s.ada.id())));
    let Node::Thread(lane) = places::read(&s.rig.world, &s.ada, side).await else {
        panic!("the aside remains a thread");
    };
    assert_eq!(lane.side_of.as_ref().unwrap().id, id);
    assert_eq!(lane.topic_id, to);
    assert_eq!(lane.base.parent_id, Some(id));
    assert!(lane.base.path.iter().any(|place| place.id == to));
    let home_after: Id = sqlx::query_scalar("SELECT home_node_id FROM environments WHERE id=$1")
        .bind(s.on.environment)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(home_after, home);
    assert_eq!(s.rig.commands.issued_of("session.open"), opens);
    assert_eq!(s.rig.commands.issued_of("turn.start"), starts);
    assert_eq!(s.rig.commands.issued_of("provider.complete"), 0);
    for topic in [s.tree.topic.base.id, to] {
        let activity = s
            .rig
            .world
            .app
            .call(NodeActivityList, &[&topic])
            .token(&s.ada.token)
            .query(&NodeActivityListQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(
            activity
                .items
                .iter()
                .filter(|line| line.action.as_deref() == Some("thread.moved"))
                .count(),
            1,
            "one move is one act in each topic's Activity"
        );
    }
}

#[tokio::test]
async fn a_thread_with_an_unanswered_native_question_cannot_move() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.say(
        &s.ada,
        thread,
        "Ask a question",
        Delivery::Queue,
        "move-question",
    )
    .await
    .ok();
    let start = s.rig.turn_start(thread, 0).await;
    s.rig.say(thread, turn_began(start.turn_id)).await;
    s.rig.say(thread, Event::ApprovalRequested(ApprovalRequested {
        turn_id:start.turn_id, request_ref:"move-question".into(), title:"Choose a scope".into(),
        request:serde_json::from_value(json!({"request_kind":"user_input","options":[],"detail":{
            "questions":[{"id":"scope","header":"Scope","question":"Which scope?","multiple":false,"allow_other":true,"options":null}]
        }})).unwrap(),
    })).await;
    s.rig.wait_state(thread, "waiting_approval").await;
    let to = destination(&s).await;
    let plan = preview(&s, thread, to).await;
    assert!(
        plan.movement
            .blockers
            .iter()
            .any(|blocker| blocker.code == "thread.not_idle")
    );
    let attempted = s
        .rig
        .world
        .app
        .call(ThreadMove, &[&thread])
        .token(&s.ada.token)
        .body(&confirmation(&plan))
        .send()
        .await;
    assert!(!attempted.status.is_success());
    assert_eq!(sync(&s, thread).await.thread.topic_id, s.tree.topic.base.id);
    let state: String = sqlx::query_scalar(
        "SELECT state FROM approvals WHERE thread_id=$1 AND native_ref='move-question'",
    )
    .bind(thread)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(state, "pending");
}

#[tokio::test]
async fn a_move_asks_only_for_manage_of_the_thread_and_interact_of_the_destination() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let to = destination(&s).await;
    s.rig
        .lets_use("node", id, "thread.manage", &s.dan, json!({}))
        .await;
    let plan = s
        .rig
        .world
        .app
        .call(ThreadMovePreview, &[&id])
        .token(&s.dan.token)
        .body(&ThreadMovePreviewBody { parent_id: to })
        .send()
        .await
        .ok();
    let moved = s
        .rig
        .world
        .app
        .call(ThreadMove, &[&id])
        .token(&s.dan.token)
        .body(&confirmation(&plan))
        .send()
        .await
        .ok();
    assert_eq!(
        moved.topic_id, to,
        "Dan needs neither provider Use nor environment Manage"
    );
    let refusal = s
        .rig
        .world
        .app
        .call(ThreadMovePreview, &[&id])
        .token(&s.cai.token)
        .body(&ThreadMovePreviewBody {
            parent_id: s.tree.topic.base.id,
        })
        .send()
        .await
        .problem();
    assert_eq!(refusal.code, ErrorCode::AuthzDenied);
    assert_eq!(refusal.extra["required"], "thread.manage");
}

#[tokio::test]
async fn busy_side_lanes_and_settings_changed_since_the_review_refuse_the_whole_move() {
    let s = Scene::new().await;
    // This test writes a retained lane's state directly; no supervisor may settle that synthetic
    // state before the move request reads it. None of its acts require background execution.
    s.rig.engine.stop();
    let id = s.thread().await.base.id;
    let side = retained_side(&s, id).await;
    let to = destination(&s).await;
    let plan = preview(&s, id, to).await;
    s.rig
        .world
        .set_at(Target::Node(to), "approval.mode", json!("auto"))
        .await;
    let stale = s
        .rig
        .world
        .app
        .call(ThreadMove, &[&id])
        .token(&s.ada.token)
        .body(&confirmation(&plan))
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    sqlx::query("UPDATE threads SET state='starting',starting_step='files' WHERE node_id=$1")
        .bind(side)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let busy = preview(&s, id, to).await;
    assert!(
        busy.movement
            .blockers
            .iter()
            .any(|blocker| blocker.code == "thread.not_idle")
    );
    let refusal = s
        .rig
        .world
        .app
        .call(ThreadMove, &[&id])
        .token(&s.ada.token)
        .body(&confirmation(&busy))
        .send()
        .await
        .problem();
    assert_eq!(refusal.code, ErrorCode::ThreadNotIdle);
    assert_eq!(sync(&s, id).await.thread.topic_id, s.tree.topic.base.id);
    let side_only = s
        .rig
        .world
        .app
        .call(ThreadMovePreview, &[&side])
        .token(&s.ada.token)
        .body(&ThreadMovePreviewBody { parent_id: to })
        .send()
        .await
        .problem();
    assert_eq!(side_only.code, ErrorCode::NodeWrongParent);
}

#[tokio::test]
async fn a_review_cannot_be_reused_by_another_actor_or_after_it_expires() {
    let s = Scene::new().await;
    let id = s.thread().await.base.id;
    let to = destination(&s).await;
    s.rig
        .lets_use("node", id, "thread.manage", &s.dan, json!({}))
        .await;
    let plan = preview(&s, id, to).await;
    let other = s
        .rig
        .world
        .app
        .call(ThreadMove, &[&id])
        .token(&s.dan.token)
        .body(&confirmation(&plan))
        .send()
        .await
        .problem();
    assert_eq!(other.code, ErrorCode::PreflightExpired);
    s.rig.world.advance(chrono::Duration::minutes(16));
    let expired = s
        .rig
        .world
        .app
        .call(ThreadMove, &[&id])
        .token(&s.ada.token)
        .body(&confirmation(&plan))
        .send()
        .await
        .problem();
    assert_eq!(expired.code, ErrorCode::PreflightExpired);
    assert_eq!(sync(&s, id).await.thread.topic_id, s.tree.topic.base.id);
}
