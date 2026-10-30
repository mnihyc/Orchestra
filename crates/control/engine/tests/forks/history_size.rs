//! A plan costs the same statements for a history of a few turns and for one of two hundred thousand, and the carry of such a history is the newest turns that fit.

use super::*;

/// How long the history at size is: the figure the owner's review asked for.
const TURNS: i64 = 200_000;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plan_costs_the_same_statements_for_a_short_history_and_for_two_hundred_thousand_turns() {
    let rig = Rig::new(
        Can {
            export_native: true,
            import_native: true,
            ..Can::default()
        },
        "fake2",
        Can {
            import_neutral: true,
            ..Can::default()
        },
    )
    .await;
    let short = rig.thread_with_history("A short history").await;
    let long = rig
        .thread_with_history("A history of two hundred thousand turns")
        .await;
    rig.world.grow_history(long, TURNS).await;
    let request = rig.to_other();
    // The plan of the short history is made twice: the second time nothing is warm that was not for the long one.
    let _ = rig.plan(short, &request).await;
    let (short_plan, short_cost) = diagnostics::counted(rig.plan(short, &request)).await;
    let (long_plan, long_cost) = diagnostics::counted(rig.plan(long, &request)).await;
    assert_eq!(short_plan.history.turns, 1);
    assert_eq!(long_plan.history.turns, TURNS);
    assert_eq!(
        long_plan.history.report.by_kind["user_message"].dropped
            + long_plan.history.report.by_kind["user_message"].kept
            + long_plan.history.report.by_kind["user_message"].converted,
        TURNS,
        "every item of the long history is counted"
    );
    // Too long to be told as items within what is taken of a window that is not known: it travels as a file, with the
    // last turns beside it, and the plan says so.
    assert_eq!(long_plan.history.method, HistoryMethod::FileHandoff);
    assert_eq!(short_plan.history.method, HistoryMethod::NeutralImport);
    assert!(
        long_plan
            .history
            .report
            .warnings
            .iter()
            .any(|warning| warning.contains("whole conversation as a file")),
        "{:?}",
        long_plan.history.report.warnings
    );
    // What the plan reads of the long history is the newest turns that fit the share: a few pages more than for the
    // short one, and no more, whatever the length.
    assert!(
        long_cost <= short_cost + 24,
        "{long_cost} statements for the long history, {short_cost} for the short one"
    );
}
