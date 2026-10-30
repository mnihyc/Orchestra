//! What is bound of the threads part of the contract: every request of a thread, an input, an approval, a draft, a pin, a thread's raw events and a session taken in as a thread is served.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_api_core::registry::Registry;
use std::collections::BTreeSet;

/// The requests of the threads part that this part serves, by the step of the work they came in.
const SERVED: [&str; 61] = [
    "ThreadCreate",
    "ThreadSyncGet",
    "ThreadOpeningGet",
    "ThreadItemList",
    "ThreadItemGet",
    "ThreadItemOutputGet",
    "ThreadOutlineGet",
    "ThreadSegmentList",
    "ThreadTurnList",
    "ThreadCatchupGet",
    "ThreadRead",
    "ThreadDraftGet",
    "ThreadDraftSet",
    "ThreadDraftDelete",
    "ThreadItemPin",
    "ThreadItemUnpin",
    "ThreadInputCreate",
    "ThreadInputList",
    "ThreadInputUpdate",
    "ThreadInputCancel",
    "ThreadInputRetry",
    "ThreadInterrupt",
    "ThreadQueueResume",
    "ThreadLimitsGet",
    "ThreadInputBypass",
    "ThreadInputRun",
    "ThreadTurnRetry",
    "ApprovalList",
    "ApprovalGet",
    "ApprovalDecide",
    "ApprovalCancel",
    "ThreadApprovalRuleList",
    "ThreadApprovalRuleDelete",
    "ThreadParameterGet",
    "ThreadExport",
    "ThreadBindingOptionsGet",
    "NodeBindingOptionsGet",
    "ThreadBindingSet",
    "ThreadEnvironmentSet",
    "ThreadBlobGet",
    "ThreadAttachmentCreate",
    "UploadCreate",
    "UploadSessionCreate",
    "UploadSessionPut",
    "UploadSessionComplete",
    "ThreadSideCreate",
    "ThreadSideList",
    "ThreadForkPreflight",
    "ThreadForkCreate",
    "ThreadForkNow",
    "ForkBatchPreflight",
    "ForkBatchCreate",
    "ForkBatchGet",
    "ForkBatchCancel",
    "ThreadAdopt",
    "ThreadAdoptRetryHistory",
    "ThreadAdoptHistoryGet",
    "ThreadRawList",
    "ThreadContinuationActivate",
    "ThreadPromptVersionCreate",
    "ThreadPromptVersionList",
];

#[test]
fn every_request_of_a_thread_is_served() {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let bound: BTreeSet<&str> = registry.bound().iter().map(|route| route.name).collect();
    for name in SERVED {
        assert!(bound.contains(name), "{name} is served");
    }
    // Nothing of the threads part of the contract is left out, except the requests of other parts
    // that happen to mention a thread.
    let declared: BTreeSet<&str> = orch_api_contract::threads::ROUTES
        .iter()
        .chain(orch_api_contract::approvals::ROUTES)
        .chain(orch_api_contract::binding::ROUTES)
        .chain(orch_api_contract::forks::ROUTES)
        .map(|route| route.name)
        .collect();
    let known: BTreeSet<&str> = SERVED.iter().copied().collect();
    let unplaced: Vec<_> = declared.difference(&known).collect();
    assert!(unplaced.is_empty(), "{unplaced:?} are not in the list");
}
