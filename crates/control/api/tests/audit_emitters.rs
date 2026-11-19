//! Every public audit action is backed by a linked production writer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_access_contract::audit::AuditAction;
use std::collections::HashSet;

#[test]
fn every_catalogue_action_has_a_production_emitter_and_test_mentions_add_none() {
    let mut routes = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut routes);
    let actual: HashSet<_> = orch_db::audit::emitters::registered()
        .map(|writer| writer.action)
        .collect();
    let expected: HashSet<_> = AuditAction::ALL
        .iter()
        .map(|action| action.as_str())
        .collect();
    let mut missing: Vec<_> = expected.difference(&actual).copied().collect();
    let mut unknown: Vec<_> = actual.difference(&expected).copied().collect();
    missing.sort_unstable();
    unknown.sort_unstable();
    assert!(
        unknown.is_empty(),
        "production writers have unknown audit actions: {unknown:?}"
    );
    assert!(
        missing.is_empty(),
        "catalogue actions lack production writers: {missing:?}"
    );
    let test_mention = orch_db::audit::name!("a.test.mention.cannot.be.a.production.writer");
    assert!(!orch_db::audit::emitters::registered().any(|writer| writer.action == test_mention));
}
