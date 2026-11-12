//! A switch or a fork into an environment that the entry of the person leaves out is blocked in its plan, with the access they lack, and is planned as before where the entry lets work run there.

use super::*;
use orch_api_contract::forks::ForkEnvironment;

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
    for result in [ForkResult::Replace, ForkResult::New] {
        let request = into(&rig, destination, result);
        assert!(
            !lacks_access(&rig.plan(thread, &request).await),
            "the entry lets work run there: {result:?}"
        );
        rig.world.access.deny_where(rig.world.ada, destination);
        let plan = rig.plan(thread, &request).await;
        assert!(lacks_access(&plan), "{result:?}: {:?}", plan.blockers);
        rig.world.access.allow_where(rig.world.ada, destination);
    }
    // Staying where the thread is, the same entry is not asked about the place it left.
    rig.world.access.deny_where(rig.world.ada, destination);
    let staying = ForkRequest {
        environment: None,
        ..rig.to_other()
    };
    assert!(!lacks_access(&rig.plan(thread, &staying).await));
}
