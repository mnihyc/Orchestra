//! A rows-only refresh preserves every visible row while avoiding a thread view's extra database statements.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;
mod support;

use realtime_support::Rig;
use support::Statements;

#[tokio::test]
async fn rows_only_refresh_has_identical_wire_rows_and_fewer_statements_for_a_threads_readers() {
    let rig = Rig::with_time_held().await;
    let thread = rig.thread("Refresh cost").await;
    let turn = rig.send(thread, "Show the visible rows").await;
    rig.begin_message(thread, &turn, "reply").await;
    rig.finish_turn(thread, &turn, "reply", "Completed text")
        .await;
    rig.server.engine.stop();
    let engine = &rig.server.engine;
    // Warm both reads before counting so pool initialization does not decide the comparison.
    engine.sync(thread, None, Some(0), 1).await.unwrap();
    engine.rows_since(thread, 0).await.unwrap();
    let mut full_cost = 0;
    let mut rows_cost = 0;
    // A page followed by many readers repeats these projections; each comparison holds its wire output.
    for _ in 0..16 {
        let before = Statements::now();
        let full = engine.sync(thread, None, Some(0), 1).await.unwrap();
        full_cost += Statements::now() - before;
        let before = Statements::now();
        let rows = engine.rows_since(thread, 0).await.unwrap();
        rows_cost += Statements::now() - before;
        assert_eq!(rows.items, full.items);
        assert!(rows.items.iter().any(|item| matches!(&item.payload, orch_api_contract::threads::ItemPayload::AgentMessage(message) if message.text == "Completed text")));
        assert_eq!(rows.turns, full.turns);
        assert_eq!(rows.inputs, full.inputs);
        assert_eq!(rows.segments, full.segments);
        assert_eq!(rows.seq, full.seq);
        assert_eq!(rows.complete, full.complete);
    }
    eprintln!("refresh statements for sixteen readers: whole={full_cost}, rows={rows_cost}");
    assert!(rows_cost > 0, "statement counting must be active");
    assert!(
        rows_cost < full_cost,
        "only adopt the projection where it saves work: whole={full_cost}, rows={rows_cost}"
    );
    rig.server.stop().await;
}
