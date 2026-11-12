//! Canonical sharing variants retain actual name Read independently of spent effects.
use super::*;
use orch_access_contract::{
    key_scope::{KeyObjectRef, KeyScope},
    permissions::ObjectKind,
};
use orch_authz::TokenGrant;
use orch_types::Id;
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const VARIANTS: [&str; 6] = [
    "live",
    "live_viewers_read",
    "live_read_first",
    "live_read_first_viewers_read",
    "snapshot",
    "snapshot_comment",
];

fn target(kind: KeyTargetKind, id: u128) -> KeyResolvedTarget {
    KeyResolvedTarget {
        object: KeyObjectRef {
            kind,
            id: Some(Id::from_u128(id)),
        },
        node_ancestors: vec![],
        document_ancestors: vec![],
        credential_account: None,
    }
}
fn names(facet: &OperationFacet) -> bool {
    matches!(
        facet.target,
        TargetResolver::ResolvedCollection(
            "share_name_orgs" | "share_name_projects" | "share_name_topics" | "share_name_threads"
        )
    )
}
fn prepared(selection: &str, changed_name: bool) -> TestResult<(Prepared, Vec<usize>, Vec<usize>)> {
    let mut plan = Prepared::for_selection(selection)?;
    let mut content = vec![];
    let mut required = vec![];
    for (index, facet) in plan.variant.facets.clone().into_iter().enumerate() {
        if facet.usage == FacetUsage::ResponseContent {
            content.push(index);
        } else if names(&facet) {
            required.push(index);
        } else {
            continue;
        }
        let kind = *facet.resources.first().ok_or("facet family absent")?;
        plan.bind(
            index,
            kind,
            &[target(
                kind,
                100 + index as u128
                    + if changed_name && names(&facet) {
                        1000
                    } else {
                        0
                    },
            )],
        )?;
    }
    Ok((plan, content, required))
}
fn reader(facts: &[KeyFacet], except: Option<Id>) -> TestResult<Caller> {
    let mut rules = vec![];
    for fact in facts {
        let id = fact.target.object.id.ok_or("target identity absent")?;
        if Some(id) == except {
            continue;
        }
        let kind = match fact.target.object.kind {
            KeyTargetKind::Object(ObjectKind::Org) => "org",
            KeyTargetKind::Object(ObjectKind::Project) => "project",
            KeyTargetKind::Object(ObjectKind::Topic) => "topic",
            KeyTargetKind::Object(ObjectKind::Thread) => "thread",
            _ => return Err("unexpected canonical response family".into()),
        };
        rules.push(json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":fact.operation,"actions":["read"]}]}));
    }
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules}))?;
    assert!(scope.validate().is_empty());
    Ok(Caller::token(
        Id::from_u128(1),
        TokenGrant::from_stored_scope(&[], None, Some(scope), None)?,
    ))
}

#[test]
fn sharing_replay_reads_all_six_native_name_shapes_without_admitting_spent_work() -> TestResult {
    for variant in VARIANTS {
        let selection = format!("ShareAgentCreate.{variant}");
        let (plan, content, required) = prepared(&selection, false)?;
        assert_eq!(required.len(), 4);
        assert!(
            plan.required_facets().is_err(),
            "spent creation/control/copy inputs remain unresolved"
        );
        let actual = plan.response_read_facets_for(&content, &required)?;
        assert!(
            actual
                .iter()
                .all(|fact| fact.operation == selection && fact.action == KeyAction::Read)
        );
        let caller = reader(&actual, None)?;
        assert!(caller.admit_facets(&actual).is_ok());
        assert!(
            plan.admit(&caller, false).is_err(),
            "response Read never becomes complete effect admission"
        );
        let denied = reader(
            &actual,
            plan.facts[required[0]]
                .as_ref()
                .and_then(|facts| facts.first())
                .and_then(|fact| fact.target.object.id),
        )?;
        assert!(
            denied.admit_facets(&actual).is_err(),
            "name clipping is not a read permit"
        );
        assert!(
            plan.response_facets_for(&required).is_err(),
            "legacy response-only selector stays closed to Required roles"
        );
        for (index, facet) in plan
            .variant
            .facets
            .iter()
            .enumerate()
            .filter(|(_, facet)| facet.action != KeyAction::Read)
        {
            assert_eq!(facet.usage, FacetUsage::Required);
            assert!(plan.response_read_facets_for(&content, &[index]).is_err());
        }
        assert!(
            plan.response_read_facets_for(&content, &[required[0], required[0]])
                .is_err()
        );
        assert!(
            plan.response_read_facets_for(&content, &[usize::MAX])
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn name_read_publication_preserves_actual_sources_and_does_not_infer_missing_as_empty() -> TestResult
{
    for variant in VARIANTS {
        let selection = format!("ShareAgentCreate.{variant}");
        let (plan, content, required) = prepared(&selection, false)?;
        let (mut fresh, _, _) = prepared(&selection, false)?;
        assert!(fresh.covers_response_read_sources(&plan, &content, &required));
        let (changed, _, _) = prepared(&selection, true)?;
        assert!(!changed.covers_response_read_sources(&plan, &content, &required));
        let index = required[0];
        fresh.facts[index] = None;
        assert!(fresh.response_read_facets_for(&content, &required).is_err());
        assert!(!fresh.covers_response_read_sources(&plan, &content, &required));
        fresh.bind_empty(index)?;
        assert!(
            fresh.response_read_facets_for(&content, &required).is_ok(),
            "a genuinely selected empty name collection is factual"
        );
        assert!(
            !fresh.covers_response_read_sources(&plan, &content, &required),
            "a current empty name list cannot validate an older named body"
        );
        assert!(
            fresh.covers_response_sources(&plan, &content),
            "names are checked by their explicit additional Read selection"
        );
    }
    Ok(())
}
