//! The local harness key applies to an environment role without an implicit handover.

use orch_link_contract::policy::PolicyKey;
use orch_link_contract::session::Role;

#[test]
fn a_connected_environment_can_report_its_locally_confirmed_programs() {
    assert!(PolicyKey::AgentHarnesses.applies_to_roles(&[Role::Environment]));
}

#[test]
fn environment_handover_does_not_delegate_the_harness_program_key() {
    assert!(!PolicyKey::AgentHarnesses.handed_by(&["environment.*".into()]));
    assert!(PolicyKey::AgentHarnesses.handed_by(&["agent.harnesses".into()]));
}
