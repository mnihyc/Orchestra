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
                acting: false,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_model_pool_switch_reviews_the_member_before_carrying_the_thread_history() {
    exercise_reviewed_pool_switch(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removed_member_leaves_the_reviewed_thread_and_pool_rotation_untouched() {
    exercise_reviewed_pool_switch(true).await;
}

async fn exercise_reviewed_pool_switch(remove_before_confirming: bool) {
    use orch_api_contract::pools::PoolCreate;

    let w = Forks::new().await;
    let who = &w.s.ada;
    let thread = w.thread_with_history("Review the next account").await;
    // The two accounts run the same profile. A switch may never trade that profile or its workspace.
    sqlx::query("UPDATE providers SET agent_id=$1 WHERE id=$2")
        .bind(w.s.on.agent)
        .bind(w.provider)
        .execute(w.s.rig.db.pool())
        .await
        .unwrap();
    let leaf = json!({"kind":"model","backend":{"route":"generation.model",
        "offering":{"provider":w.provider,"offering":MODEL},"config":{}}});
    let pool = w.s.rig.world.app.call(PoolCreate, &[]).token(&who.token)
        .raw_body(&json!({"name":"Reviewed accounts","description":"","home_id":who.person.personal_org,"access":"only_me",
            "definition":{"kind":"model","enabled":true,"policy":{"type":"round_robin"},"lend_members":false,
            "config":{},"members":[{"id":orch_types::new_id(),"enabled":true,"rule":{"type":"single","resource":leaf}}]}}))
        .send().await.ok();
    let target = json!({"type":"pool","kind":"model","id":pool.id});
    let mut body = json!(w.to_second());
    body["to"] = json!({"target":target});
    let mut review = None;
    for _ in 0..2 {
        let plan =
            w.s.rig
                .world
                .app
                .call(ThreadForkPreflight, &[&thread])
                .token(&who.token)
                .raw_body(&body)
                .send()
                .await
                .ok()
                .into_full()
                .unwrap();
        assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
        assert_eq!(plan.to.agent.id, w.s.on.agent);
        assert_eq!(plan.to.provider.id, w.provider);
        assert_eq!(plan.to.model, MODEL);
        assert!(
            matches!(&plan.to.environment, orch_api_contract::forks::ForkPlanTarget::Existing(env) if env.id == w.s.on.environment)
        );
        assert_eq!(w.count("SELECT count(*) FROM pool_choices").await, 0);
        assert_eq!(w.count("SELECT count(*) FROM pool_rotation").await, 0);
        assert_eq!(
            w.read(who, thread).await.binding.unwrap().provider.id,
            w.s.on.provider
        );
        review = Some(plan);
    }
    let review = review.unwrap();
    if remove_before_confirming {
        sqlx::query("UPDATE pool_members SET enabled=false WHERE pool_id=$1")
            .bind(pool.id)
            .execute(w.s.rig.db.pool())
            .await
            .unwrap();
        let problem = w.carry_out(who, thread, &review).await.problem();
        assert!(
            matches!(
                problem.code,
                ErrorCode::PreflightExpired | ErrorCode::PlanBlocked
            ),
            "{problem:?}"
        );
        assert_eq!(w.count("SELECT count(*) FROM pool_choices").await, 0);
        assert_eq!(w.count("SELECT count(*) FROM pool_rotation").await, 0);
        assert_eq!(
            w.read(who, thread).await.binding.unwrap().provider.id,
            w.s.on.provider
        );
        return;
    }
    let job = w.carry_out(who, thread, &review).await.ok();
    let done = w.ended(who, job.id).await;
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    let selected: Vec<(Json, Json)> = sqlx::query_as(
        "SELECT c.target,c.leaf FROM threads t JOIN segment_pool_bindings b ON b.segment_id=t.current_segment_id JOIN pool_choices c ON c.id=b.choice_id WHERE t.node_id=$1 AND c.state='selected' AND c.actor_id=$2",
    )
    .bind(thread)
    .bind(who.id())
    .fetch_all(w.s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(selected, vec![(target, leaf)]);
    assert_eq!(w.count("SELECT count(*) FROM pool_choices").await, 1);
}
