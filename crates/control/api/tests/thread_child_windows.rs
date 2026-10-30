//! Busy child streams preserve the conversation's first window and page only their own history over HTTP.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a failed test panics"
)]
mod places;
mod rig;
mod support;

use orch_agent_contract::events::{
    AgentMessagePayload, AgentMessagePhase, SubagentState, ToolCallPayload, ToolPayload,
};
use orch_api_contract::threads::{
    Delivery, ItemPayload, OutputRef, RawCall, ThreadItemList, ThreadItemListQuery, ThreadSyncGet,
    ThreadSyncGetQuery,
};
use orch_link_contract::events::{EmittedItemKind, Event, ItemStarted};
use orch_link_contract::foreign::ItemPayload as NativePayload;
use rig::Scene;

const STREAM: &str = "sub:reviewer";
// More than the requested initial window, so an all-stream tail alone would lose its parent.
const CHILD_COUNT: i64 = 105;
const WINDOW: u32 = 100;

#[tokio::test]
async fn a_busy_child_keeps_its_parent_visible_and_its_history_flags_ignore_the_main_stream() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    assert!(
        s.say(
            &s.ada,
            thread,
            "Review the redirect",
            Delivery::Queue,
            "child-window"
        )
        .await
        .status
        .is_success()
    );
    let start = s.rig.turn_start(thread, 0).await;
    s.rig.say(thread, rig::turn_began(start.turn_id)).await;
    s.rig
        .say(
            thread,
            Event::ItemStarted(ItemStarted {
                turn_id: start.turn_id,
                native_ref: "reviewer".to_owned(),
                stream: "main".to_owned(),
                parent_native_ref: None,
                kind: EmittedItemKind::ToolCall,
                payload: NativePayload::ToolCall(ToolCallPayload {
                    tool: ToolPayload::Subagent {
                        child_stream: STREAM.to_owned(),
                        name: Some("reviewer".to_owned()),
                        role: Some("review".to_owned()),
                        prompt: Some("Check the redirect".to_owned()),
                        state: SubagentState::Running,
                        model: None,
                    },
                    title: "Review the redirect".to_owned(),
                    started_at: s.rig.world.now().into(),
                    duration_ms: None,
                    raw: RawCall {
                        name: "subagent".to_owned(),
                        input: OutputRef::of("{}"),
                        output: None,
                        is_error: false,
                    },
                }),
            }),
        )
        .await;
    for n in 0..CHILD_COUNT {
        let Event::ItemStarted(mut child) =
            rig::message_began(start.turn_id, &format!("review-{n}"))
        else {
            unreachable!()
        };
        child.stream = STREAM.to_owned();
        child.parent_native_ref = Some("reviewer".to_owned());
        child.payload = NativePayload::AgentMessage(AgentMessagePayload {
            text: "Checking the redirect.".to_owned(),
            text_len: 22,
            phase: Some(AgentMessagePhase::Final),
        });
        s.rig.say(thread, Event::ItemStarted(child)).await;
    }
    s.rig
        .say(
            thread,
            rig::message_began(start.turn_id, "main-after-review"),
        )
        .await;
    rig::eventually("all child items and the following main item are ingested", || async {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1 AND stream=$2")
            .bind(thread).bind(STREAM).fetch_one(s.rig.db.pool()).await.unwrap();
        let last: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM items WHERE thread_id=$1 AND native_ref='main-after-review')")
            .bind(thread).fetch_one(s.rig.db.pool()).await.unwrap();
        count == CHILD_COUNT && last
    }).await;

    let sync = s
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadSyncGetQuery {
            limit: Some(WINDOW),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let parent = sync.items.iter().find(|item| matches!(&item.payload, ItemPayload::ToolCall(call) if matches!(&call.tool, orch_api_contract::threads::ToolPayload::Subagent { .. })))
        .expect("child activity cannot evict its parent's row");
    assert_eq!(parent.stream, "main");
    assert!(
        sync.items
            .iter()
            .any(|item| matches!(&item.payload, ItemPayload::UserMessage(_)))
    );
    assert!(
        !sync.has_older,
        "omitted child history does not pretend the main conversation has earlier rows"
    );
    assert!(sync.items.len() <= usize::try_from(WINDOW * 2).unwrap());
    assert!(
        sync.items
            .iter()
            .filter(|item| item.stream == STREAM)
            .count()
            < usize::try_from(CHILD_COUNT).unwrap()
    );

    let all = s
        .rig
        .world
        .app
        .call(ThreadItemList, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadItemListQuery {
            after: Some(0),
            stream: Some(STREAM.to_owned()),
            limit: Some(500),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(all.items.len(), usize::try_from(CHILD_COUNT).unwrap());
    assert!(
        all.items
            .iter()
            .all(|item| item.stream == STREAM && item.parent_item_id == Some(parent.id))
    );
    assert!(
        !all.has_older,
        "the parent precedes the stream, not its own history"
    );
    assert!(
        !all.has_newer,
        "the main answer after a child is not more child activity"
    );

    let before = s
        .rig
        .world
        .app
        .call(ThreadItemList, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadItemListQuery {
            before: Some(all.items.first().unwrap().seq_created),
            stream: Some(STREAM.to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(before.items.is_empty());
    assert!(!before.has_older);
    assert!(before.has_newer);
    let after = s
        .rig
        .world
        .app
        .call(ThreadItemList, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadItemListQuery {
            after: Some(all.items.last().unwrap().seq_created),
            stream: Some(STREAM.to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(after.items.is_empty());
    assert!(after.has_older);
    assert!(!after.has_newer);
    let missing = s
        .rig
        .world
        .app
        .call(ThreadItemList, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadItemListQuery {
            after: Some(0),
            stream: Some("sub:absent".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(missing.items.is_empty() && !missing.has_older && !missing.has_newer);

    let main = s
        .rig
        .world
        .app
        .call(ThreadItemList, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadItemListQuery {
            before: Some(sync.seq + 1),
            stream: Some("main".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(main.items.iter().all(|item| item.stream == "main"));
    assert!(main.items.iter().any(|item| item.id == parent.id));
}
