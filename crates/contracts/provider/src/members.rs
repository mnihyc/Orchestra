//! The members of a provider's facts, the four states each is in, and which members a provider's sources declare.
//!
//! Every member of a provider's facts ([`FactMember`]) is in exactly one of four states ([`MemberState`]):
//! unsupported (the source does not declare it and never told it), not read yet (declared, never told), empty
//! (told as `null` or `[]`) or told (told with a value). A told member is supported from then on, even where it
//! was not declared: a fact proves support. [`member_state`] decides it, the same function for the API and the
//! engine; the facts types and [`normalize`] are those of [`crate::facts`], re-exported here.
//!
//! What a provider's sources declare is what its facts source lists in its descriptor, and what the keeper of
//! its credential always reports beside it: the harness of a sign-in tells who is signed in and how; Orch knows
//! of a key that it is one ([`declared`]).

pub use crate::facts::{
    ACCOUNT_BALANCE_ID, Balance, BalancePart, FactMember, KEY_SPEND_IDS, MemberState,
    PLAN_BALANCE_ID, ProviderFacts, Spend, SpendPeriod, facts_state, member_state, normalize,
};

use crate::source::SourceDescriptor;

/// What the keeper of a sign-in always reports: the harness that holds it tells who is signed in, and how.
pub const SIGN_IN_KEEPER: [FactMember; 2] = [FactMember::Identity, FactMember::Credential];

/// What the keeper of a key always reports: that the credential is a key.
pub const KEY_KEEPER: [FactMember; 1] = [FactMember::Credential];

/// The members a provider's facts are declared to have: those its facts source lists (none where it has no
/// facts source or the source implements no facts), and those the keeper of its credential always reports.
#[must_use]
pub fn declared(facts_source: Option<&SourceDescriptor>, keeper: &[FactMember]) -> Vec<FactMember> {
    let mut members: Vec<FactMember> = facts_source
        .map(|descriptor| descriptor.reports().to_vec())
        .unwrap_or_default();
    members.extend_from_slice(keeper);
    members.sort_unstable();
    members.dedup();
    members
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceDescriptorFacts;
    use serde_json::json;

    fn told_at() -> orch_types::Time {
        "2026-10-15T10:00:00.000Z".parse().unwrap()
    }

    #[test]
    fn a_program_sourced_sign_in_still_shows_who_is_signed_in() {
        let mut program = SourceDescriptor::new("acme-plan", "Acme plan", "Reads the Acme plan.");
        program.facts = Some(SourceDescriptorFacts {
            reports: vec![FactMember::Windows, FactMember::Balances],
            ..SourceDescriptorFacts::default()
        });
        let members = declared(Some(&program), &SIGN_IN_KEEPER);
        assert_eq!(
            members,
            [
                FactMember::Windows,
                FactMember::Balances,
                FactMember::Identity,
                FactMember::Credential
            ]
        );
        let facts = ProviderFacts::default();
        assert_eq!(
            member_state(FactMember::Identity, &facts, &members),
            MemberState::NotReadYet
        );
        assert_eq!(
            member_state(FactMember::Plan, &facts, &members),
            MemberState::Unsupported
        );
    }

    #[test]
    fn a_plain_key_tells_only_that_it_is_a_key_and_the_rest_is_unsupported() {
        let members = declared(None, &KEY_KEEPER);
        let facts: ProviderFacts = serde_json::from_value(json!({
            "credential": { "method": null, "yields": "key", "expires_at": null }
        }))
        .unwrap();
        let states = facts_state(&facts, &members);
        assert_eq!(states[&FactMember::Credential], MemberState::Told);
        for member in FactMember::ALL {
            if member != FactMember::Credential {
                assert_eq!(states[&member], MemberState::Unsupported, "{member}");
            }
        }
    }

    #[test]
    fn a_member_told_by_a_source_that_did_not_declare_it_is_told() {
        let mut facts = ProviderFacts {
            ordinary_usage_allowed: Some(false),
            ..ProviderFacts::default()
        };
        facts
            .field_times
            .insert("ordinary_usage_allowed".to_owned(), told_at());
        assert_eq!(
            member_state(FactMember::OrdinaryUsageAllowed, &facts, &[]),
            MemberState::Told
        );
    }
}
