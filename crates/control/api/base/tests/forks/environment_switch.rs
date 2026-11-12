//! The public placement affordance and the existing native switch planner share one gate.

use super::*;
use binding_support::names;

#[tokio::test(flavor = "multi_thread")]
async fn a_reader_managing_both_environments_can_choose_placement_but_cannot_change_the_provider() {
    let w = Forks::new().await;
    let thread = w.thread_with_history("Reader chooses placement").await;
    let destination = another_environment(&w.s, "Destination", Some(w.s.rig.machine_id)).await;
    let reader = &w.s.cai;
    assert!(!w.read(reader, thread).await.can.place);
    let request = ForkRequest {
        to: None,
        environment: Some(ForkEnvironment::Existing {
            environment_id: destination,
            files: None,
        }),
        ..w.to_second()
    };
    names(
        &w.s,
        "environment",
        w.s.on.environment,
        "environment.manage",
        reader,
        json!({}),
    )
    .await;
    assert!(w.read(reader, thread).await.can.place);
    let blocked = w.plan(reader, thread, &request).await.ok();
    assert!(
        serde_json::to_string(&blocked.blockers)
            .unwrap()
            .contains("environment.manage")
    );
    names(
        &w.s,
        "environment",
        destination,
        "environment.manage",
        reader,
        json!({}),
    )
    .await;
    let allowed = w.plan(reader, thread, &request).await.ok();
    assert!(allowed.blockers.is_empty(), "{:?}", allowed.blockers);
    let binding = w.plan(reader, thread, &w.to_second()).await.ok();
    assert!(
        serde_json::to_string(&binding.blockers)
            .unwrap()
            .contains("thread.manage")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_side_thread_never_offers_placement_even_to_its_environment_manager() {
    let w = Forks::new().await;
    let parent = w.thread_with_history("Parent").await;
    let side =
        w.s.rig
            .world
            .app
            .call(ThreadSideCreate, &[&parent])
            .token(&w.s.ada.token)
            .body(&ThreadSideCreateBody {
                content: vec![ContentPart::Text {
                    text: "An aside".into(),
                }],
                client_id: "placement-aside".into(),
                to_turn: None,
            })
            .send()
            .await
            .ok();
    let side = side.into_full().unwrap();
    assert!(!w.read(&w.s.ada, side.thread.base.id).await.can.place);
}
