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
async fn a_placement_is_asked_of_the_threads_first_owner_and_the_placer_is_told_the_place_alone() {
    let w = Forks::new().await;
    // Dan starts the thread and uses the provider through an entry that names the environment it runs in.
    let owner = &w.s.dan;
    w.s.rig
        .lets_use("agent", w.s.on.agent, "agent.use", owner, json!({}))
        .await;
    w.s.rig
        .lets_use(
            "environment",
            w.s.on.environment,
            "environment.use",
            owner,
            json!({}),
        )
        .await;
    w.s.rig
        .lets_use(
            "provider",
            w.s.on.provider,
            "provider.use",
            owner,
            json!({ "where": [{ "kind": "environment", "id": w.s.on.environment }] }),
        )
        .await;
    let thread = w.thread_with_history_by(owner, "Dan's thread").await;
    let destination = another_environment(&w.s, "Destination", Some(w.s.rig.machine_id)).await;
    let manager = &w.s.cai;
    for environment in [w.s.on.environment, destination] {
        names(
            &w.s,
            "environment",
            environment,
            "environment.manage",
            manager,
            json!({}),
        )
        .await;
    }
    let request = ForkRequest {
        to: None,
        environment: Some(ForkEnvironment::Existing {
            environment_id: destination,
            files: None,
        }),
        ..w.to_second()
    };
    // The manager does not become the one who runs the provider, so the thread's owner is asked, with the
    // place the thread would run in: their entry leaves it out, and the manager is told that and nothing of
    // the provider, its owner or who could give the entry.
    let blocked = w.plan(manager, thread, &request).await.ok();
    let access: Vec<_> = blocked
        .blockers
        .iter()
        .filter(|blocker| blocker.code == "access")
        .collect();
    assert_eq!(access.len(), 1, "{:?}", blocked.blockers);
    let missing = access[0].missing.as_ref().expect("what is missing");
    assert!(!missing.is_empty());
    for lacking in missing {
        assert!(
            lacking.object.name.is_none()
                && lacking.object.owner.is_none()
                && lacking.ask.is_empty(),
            "{lacking:?}"
        );
        assert_eq!(
            lacking
                .options
                .as_ref()
                .and_then(|options| options.r#where.as_ref())
                .map(|places| places.iter().map(|place| place.id).collect::<Vec<_>>()),
            Some(vec![destination]),
            "the place is what the manager is told"
        );
    }
    assert!(
        !serde_json::to_string(&blocked.blockers)
            .unwrap()
            .contains("Fake subscription"),
        "the provider is not named to a placer who is not its user"
    );
    // The entry names the destination too: the placement is planned.
    sqlx::query(
        "UPDATE permission_entries SET options = $3
          WHERE object_kind = 'provider' AND object_id = $1 AND holder_id = $2 AND permission = 'provider.use'",
    )
    .bind(w.s.on.provider)
    .bind(owner.id())
    .bind(json!({ "where": [
        { "kind": "environment", "id": w.s.on.environment },
        { "kind": "environment", "id": destination }
    ] }))
    .execute(w.s.rig.db.pool())
    .await
    .unwrap();
    let allowed = w.plan(manager, thread, &request).await.ok();
    assert!(allowed.blockers.is_empty(), "{:?}", allowed.blockers);
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
