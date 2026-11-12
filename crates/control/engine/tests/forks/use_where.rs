//! A switch or a fork into an environment that the entry of the person leaves out is blocked in its plan, with the access they lack, whether the environment exists or is still to be made on a host, and is planned as before where the entry lets work run there. The access double decides as the access engine does, so that a plan which asked without the place would pass here and fail the case.

use super::*;
use orch_api_contract::forks::{FileSelection, ForkEnvironment, ForkNewEnvironment};
use serde_json::json;

fn into(rig: &Rig, destination: Id, result: ForkResult) -> ForkRequest {
    ForkRequest {
        result,
        to: None,
        environment: Some(ForkEnvironment::Existing {
            environment_id: destination,
            files: None,
        }),
        ..rig.to_other()
    }
}

fn lacks_access(plan: &ForkPlan) -> bool {
    plan.blockers.iter().any(|blocker| blocker.code == "access")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_and_a_fork_into_an_environment_the_entry_leaves_out_are_blocked_in_the_plan() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    let destination = rig.second_environment().await;
    let thread = rig.thread_with_history("Out of the list").await;
    let source = rig.world.environment;
    for result in [ForkResult::Replace, ForkResult::New] {
        let request = into(&rig, destination, result);
        rig.world
            .access
            .use_only_in(rig.world.ada, &[source, destination]);
        assert!(
            !lacks_access(&rig.plan(thread, &request).await),
            "the entry names the destination: {result:?}"
        );
        rig.world.access.use_only_in(rig.world.ada, &[source]);
        let plan = rig.plan(thread, &request).await;
        assert!(lacks_access(&plan), "{result:?}: {:?}", plan.blockers);
    }
    // Staying where the thread is, the same entry is asked about the place the result runs in, which
    // is the place it already runs in.
    let staying = ForkRequest {
        environment: None,
        ..rig.to_other()
    };
    assert!(!lacks_access(&rig.plan(thread, &staying).await));
    rig.world.access.use_only_in(rig.world.ada, &[destination]);
    assert!(
        lacks_access(&rig.plan(thread, &staying).await),
        "an entry that leaves the source's own environment out blocks a plan that stays there"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_into_an_environment_to_be_made_on_a_host_is_held_to_the_hosts_the_entry_names() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    hosting::serve(&rig.world);
    let host = hosting::ScriptedHost::on(&rig.world, "named-host", 16_384).await;
    let template = hosting::template(&rig.world).await;
    let thread = rig.world.thread("Parent source").await;
    let mut request = rig.fork();
    request.environment = Some(ForkEnvironment::New {
        new: ForkNewEnvironment {
            template_id: Some(template),
            host_id: Some(Some(host.id)),
            files: Some(FileSelection::Nothing),
            ..ForkNewEnvironment::default()
        },
    });
    // The entry names the host, and an environment made on it later is covered.
    rig.world.access.use_through(
        rig.world.ada,
        vec![json!({ "where": [{ "kind": "host", "id": host.id }] })],
    );
    let plan = rig.plan(thread, &request).await;
    assert!(!lacks_access(&plan), "{:?}", plan.blockers);
    // The entry names another host, and the environment to be made on this one is outside it.
    rig.world.access.use_through(
        rig.world.ada,
        vec![json!({ "where": [{ "kind": "host", "id": orch_types::new_id() }] })],
    );
    let plan = rig.plan(thread, &request).await;
    assert!(lacks_access(&plan), "{:?}", plan.blockers);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_without_history_is_not_bound_or_placed_where_its_owners_entry_leaves_the_place_out()
 {
    use orch_engine::binding::{Rebind, RebindError, Replace};
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    let destination = rig.second_environment().await;
    let thread = rig.world.thread("Fresh").await;
    let caller = rig.caller();
    let ada = rig.world.ada;
    let source = rig.world.environment;
    // Put on another provider where it runs: the entry names the environment it runs in or not.
    let rebind = || Rebind {
        agent: rig.other_agent,
        provider: rig.other_provider,
        arrangement: None,
        model: Some(MODEL.to_owned()),
        effort: None,
    };
    rig.world.access.use_only_in(ada, &[destination]);
    let refused = rig
        .world
        .engine()
        .bind_thread(&caller, thread, rebind())
        .await;
    assert!(
        matches!(refused, Err(RebindError::Denied(_))),
        "the entry leaves out the environment the thread runs in: {refused:?}"
    );
    rig.world.access.use_only_in(ada, &[source]);
    let bound = rig
        .world
        .engine()
        .bind_thread(&caller, thread, rebind())
        .await;
    assert!(
        !matches!(bound, Err(RebindError::Denied(_))),
        "the entry names it: {bound:?}"
    );
    // Put into another environment: the first owner's entry has to name that one.
    let to = || Replace {
        environment: destination,
        directory: None,
    };
    let refused = rig.world.engine().place_thread(&caller, thread, to()).await;
    assert!(
        matches!(refused, Err(RebindError::Denied(_))),
        "the entry leaves out the environment it is put into: {refused:?}"
    );
    rig.world.access.use_only_in(ada, &[source, destination]);
    let placed = rig.world.engine().place_thread(&caller, thread, to()).await;
    assert!(
        !matches!(placed, Err(RebindError::Denied(_))),
        "the entry names both: {placed:?}"
    );
}
