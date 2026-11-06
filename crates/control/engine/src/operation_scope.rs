//! Shared operation metadata binding for HTTP, workers, and continuing work.
//!
//! Producers supply current typed facts for their actual effects. This module only validates
//! those facts against the existing catalogue and invokes the existing Caller restriction;
//! account authorization and business validation remain in the ordinary producers.

use orch_access_contract::key_scope::{
    KeyAction, KeyEmptyFacet, KeyFacet, KeyResolvedTarget, KeyTargetKind,
};
use orch_api_contract::operations::{
    self, CatalogueError, Delegation, FacetUsage, OperationFacet, OperationVariant, TargetResolver,
};
use orch_api_contract::routes::Route;
use orch_authz::Caller;

/// Stored-byte source witnesses shared by requests and continuing work.
pub mod blob_sources;
pub mod creator_files;
pub mod creators;
/// Current placement facts shared with the ordinary HTTP target resolver.
pub mod targets;

/// Bad server declarations remain distinguishable from a credential restriction refusal.
#[derive(Debug, thiserror::Error)]
pub enum BindingError {
    /// The shared catalogue is incomplete or inconsistent.
    #[error(transparent)]
    Catalogue(#[from] CatalogueError),
    /// A selection, binding or credential does not admit this actual operation.
    #[error("the key does not allow this operation")]
    Refused,
}

/// Server-owned facts for one current operation checkpoint; never serialized as authority.
#[derive(Clone)]
pub struct Prepared {
    route: Route,
    variant: OperationVariant,
    receipt: bool,
    facts: Vec<Option<Vec<KeyFacet>>>,
}

/// Preserve legacy endpoint admission in addition to the caller's complete policy ceiling.
#[must_use]
pub fn compatible_selection(caller: &Caller, variant: &OperationVariant) -> bool {
    !caller
        .grant()
        .is_some_and(|grant| grant.requires_legacy_compatibility() && !variant.legacy_tokens)
        && caller.permits_operation(&variant.operation)
}

/// Validate the declared business family without relabeling the actual creation parent.
pub fn facets(
    selection: &str,
    metadata: &OperationFacet,
    resource_kind: KeyTargetKind,
    targets: &[KeyResolvedTarget],
) -> Result<Vec<KeyFacet>, BindingError> {
    if !metadata.resources.contains(&resource_kind) {
        return Err(BindingError::Refused);
    }
    Ok(targets
        .iter()
        .cloned()
        .map(|target| KeyFacet {
            operation: selection.to_owned(),
            action: metadata.action,
            resource_kind,
            target,
        })
        .collect())
}

impl Prepared {
    /// Find an accepted selection in the canonical route registry. Splitting the catalogue's
    /// variant suffix is lookup only; `new` still requires an exact declared selection.
    pub fn for_selection(selection: &str) -> Result<Self, BindingError> {
        let name = selection
            .split_once('.')
            .map_or(selection, |(name, _)| name);
        let route = orch_api_contract::routes::all()
            .into_iter()
            .find(|route| route.name == name)
            .ok_or(BindingError::Refused)?;
        Self::new(route, selection)
    }

    /// The exact selection comes from the producer's intent, never an incoming key claim.
    pub fn new(route: Route, selection: &str) -> Result<Self, BindingError> {
        let operation = operations::for_route(&route)?;
        if operation.delegation != Delegation::Business {
            return Err(BindingError::Refused);
        }
        let variant = operation
            .variants
            .into_iter()
            .find(|variant| variant.operation == selection)
            .ok_or(BindingError::Refused)?;
        let facts = vec![None; variant.facets.len()];
        Ok(Self {
            route,
            variant,
            receipt: operation.receipt,
            facts,
        })
    }

    /// The same declaration the operation uses on every transport.
    #[must_use]
    pub fn variant(&self) -> &OperationVariant {
        &self.variant
    }

    /// Whether the existing operation has a typed receipt producer.
    #[must_use]
    pub fn receipt(&self) -> bool {
        self.receipt
    }

    /// A preparation cannot be installed on another operation's request.
    #[must_use]
    pub fn matches_route(&self, route: &Route) -> bool {
        self.route.name == route.name
            && self.route.method == route.method
            && self.route.path == route.path
    }

    /// Bind one complete nonempty slot; duplicate writes cannot replace previously checked facts.
    pub fn bind(
        &mut self,
        index: usize,
        kind: KeyTargetKind,
        resolved: &[KeyResolvedTarget],
    ) -> Result<(), BindingError> {
        let metadata = self
            .variant
            .facets
            .get(index)
            .ok_or(BindingError::Refused)?;
        let slot = self.facts.get_mut(index).ok_or(BindingError::Refused)?;
        if slot.is_some() || resolved.is_empty() {
            return Err(BindingError::Refused);
        }
        *slot = Some(facets(&self.variant.operation, metadata, kind, resolved)?);
        Ok(())
    }

    /// Bind a heterogeneous collection once, preserving each producer's stored business
    /// family independently of its governing target kind. Empty collections use `bind_empty`.
    pub fn bind_collection(
        &mut self,
        index: usize,
        groups: &[(KeyTargetKind, Vec<KeyResolvedTarget>)],
    ) -> Result<(), BindingError> {
        let metadata = self
            .variant
            .facets
            .get(index)
            .ok_or(BindingError::Refused)?;
        let slot = self.facts.get_mut(index).ok_or(BindingError::Refused)?;
        if slot.is_some()
            || !matches!(metadata.target, TargetResolver::ResolvedCollection(_))
            || groups.is_empty()
        {
            return Err(BindingError::Refused);
        }
        let mut bound = Vec::new();
        for (kind, targets) in groups {
            if targets.is_empty() {
                return Err(BindingError::Refused);
            }
            bound.extend(facets(&self.variant.operation, metadata, *kind, targets)?);
        }
        *slot = Some(bound);
        Ok(())
    }

    /// Only an explicitly declared collection can materialize as empty. Missing is never empty.
    pub fn bind_empty(&mut self, index: usize) -> Result<(), BindingError> {
        let metadata = self
            .variant
            .facets
            .get(index)
            .ok_or(BindingError::Refused)?;
        if !matches!(metadata.target, TargetResolver::ResolvedCollection(_)) {
            return Err(BindingError::Refused);
        }
        let slot = self.facts.get_mut(index).ok_or(BindingError::Refused)?;
        if slot.is_some() {
            return Err(BindingError::Refused);
        }
        *slot = Some(Vec::new());
        Ok(())
    }

    /// Scope-only predicate for effect-free source selection; ordinary visibility still binds.
    pub fn permits_binding(
        &self,
        caller: &Caller,
        index: usize,
        kind: KeyTargetKind,
        targets: &[KeyResolvedTarget],
    ) -> Result<bool, BindingError> {
        self.compatible(caller)?;
        let metadata = self
            .variant
            .facets
            .get(index)
            .ok_or(BindingError::Refused)?;
        if targets.is_empty() {
            return Err(BindingError::Refused);
        }
        Ok(caller.permits_facets(&facets(&self.variant.operation, metadata, kind, targets)?))
    }

    /// Checks both the exact current selection and any original legacy endpoint restriction.
    pub fn compatible(&self, caller: &Caller) -> Result<(), BindingError> {
        if compatible_selection(caller, &self.variant) {
            Ok(())
        } else {
            Err(BindingError::Refused)
        }
    }

    /// All actual required effects. An unresolved required slot always refuses.
    pub fn required_facets(&self) -> Result<Vec<KeyFacet>, BindingError> {
        self.selected_facets(|metadata| metadata.usage == FacetUsage::Required)
    }

    /// All actual content targets, after their ordinary response producer has resolved them.
    pub fn response_facets(&self) -> Result<Vec<KeyFacet>, BindingError> {
        self.selected_facets(|metadata| metadata.usage == FacetUsage::ResponseContent)
    }

    /// Fresh native facts must still cover every source that built an earlier body.
    /// Compare each canonical role's real identity/business family; current ancestry
    /// belongs to this new preparation's own admission, never the older snapshot.
    /// Additional freshly selected rows cannot substitute for an earlier source.
    pub fn covers_sources(&self, earlier: &Self) -> bool {
        self.variant.operation == earlier.variant.operation
            && self.facts.len() == earlier.facts.len()
            && self
                .facts
                .iter()
                .zip(&earlier.facts)
                .all(|(fresh, old)| match (fresh, old) {
                    (Some(fresh), Some(old)) => old.iter().all(|old| {
                        fresh.iter().any(|fresh| {
                            fresh.operation == old.operation
                                && fresh.action == old.action
                                && fresh.resource_kind == old.resource_kind
                                && fresh.target.object == old.target.object
                        })
                    }),
                    _ => false,
                })
    }

    /// Fresh required reads reused by the response checkpoint, without reusing earlier ancestry.
    pub fn required_reads(&self) -> Result<Vec<KeyFacet>, BindingError> {
        self.selected_facets(|metadata| {
            metadata.usage == FacetUsage::Required && metadata.action == KeyAction::Read
        })
    }

    fn selected_facets(
        &self,
        wanted: impl Fn(&OperationFacet) -> bool,
    ) -> Result<Vec<KeyFacet>, BindingError> {
        let mut selected = Vec::new();
        for (metadata, facts) in self.variant.facets.iter().zip(&self.facts) {
            if wanted(metadata) {
                selected.extend(facts.as_ref().ok_or(BindingError::Refused)?.iter().cloned());
            }
        }
        Ok(selected)
    }

    /// Some(true) is complete, Some(false) refused, None awaits actual response targets.
    /// HTTP disclosure state and response/error projection remain with the transport.
    #[must_use]
    pub fn response_coverage(&self, caller: &Caller) -> Option<bool> {
        if caller
            .grant()
            .is_none_or(|grant| grant.operation_scope().is_none())
        {
            return Some(true);
        }
        let mut pending = false;
        for (metadata, facts) in self.variant.facets.iter().zip(&self.facts) {
            if metadata.usage != FacetUsage::ResponseContent {
                continue;
            }
            match facts {
                None => pending = true,
                Some(facts) if !facts.is_empty() && !caller.permits_facets(facts) => {
                    return Some(false);
                }
                Some(_) => {}
            }
        }
        (!pending).then_some(true)
    }

    fn empty_requirements(&self) -> Option<Vec<KeyEmptyFacet>> {
        let mut required = Vec::new();
        for (metadata, facts) in self.variant.facets.iter().zip(&self.facts) {
            if metadata.usage != FacetUsage::Required {
                continue;
            }
            if !matches!(metadata.target, TargetResolver::ResolvedCollection(_))
                || !facts.as_ref().is_some_and(Vec::is_empty)
            {
                return None;
            }
            required.push(KeyEmptyFacet {
                action: metadata.action,
                resources: metadata.resources.clone(),
            });
        }
        (!required.is_empty()).then_some(required)
    }

    fn admit_facts(&self, caller: &Caller, facts: &[KeyFacet]) -> Result<Caller, BindingError> {
        let admitted = if facts.is_empty() {
            caller.admit_empty_operation(
                &self.variant.operation,
                &self.empty_requirements().ok_or(BindingError::Refused)?,
            )
        } else {
            caller.admit_facets(facts)
        };
        admitted
            .map(|admitted| admitted.into_caller())
            .map_err(|_| BindingError::Refused)
    }

    /// Admit the complete current operation. Optional response facts are included only when its
    /// ordinary response path will use them; receipt selection remains outside this binder.
    pub fn admit(&self, caller: &Caller, include_response: bool) -> Result<Caller, BindingError> {
        self.compatible(caller)?;
        let mut required = self.required_facets()?;
        // Even if response facts exist, a completely empty effect must retain its declared proof.
        if required.is_empty() {
            self.admit_facts(caller, &required)?;
        }
        if include_response {
            if self.response_coverage(caller) != Some(true) {
                return Err(BindingError::Refused);
            }
            required.extend(
                self.variant
                    .facets
                    .iter()
                    .zip(&self.facts)
                    .filter(|(metadata, _)| metadata.usage == FacetUsage::ResponseContent)
                    .flat_map(|(_, facts)| facts.iter().flatten().cloned()),
            );
        }
        self.admit_facts(caller, &required)
    }

    /// Partial admission only for the existing effect-free preparation phase. Final effect
    /// admission still requires every slot, so this caller must never commit business effects.
    pub fn preparation_caller(&self, caller: &Caller) -> Result<Caller, BindingError> {
        self.compatible(caller)?;
        let known: Vec<_> = self
            .variant
            .facets
            .iter()
            .zip(&self.facts)
            .filter(|(metadata, _)| metadata.usage == FacetUsage::Required)
            .flat_map(|(_, facts)| facts.iter().flatten().cloned())
            .collect();
        self.admit_facts(caller, &known)
    }
}

/// Validate transient empty evidence against the exact catalogue declaration.
pub fn checked_empty_for(route: &Route, caller: &Caller) -> Result<bool, BindingError> {
    let (Some(selection), Some(required)) =
        (caller.admitted_operation(), caller.admitted_empty_facets())
    else {
        return Ok(false);
    };
    let operation = operations::for_route(route)?;
    let Some(variant) = operation
        .variants
        .iter()
        .find(|variant| variant.operation == selection)
    else {
        return Ok(false);
    };
    let mut expected = Vec::new();
    for facet in variant
        .facets
        .iter()
        .filter(|facet| facet.usage == FacetUsage::Required)
    {
        if !matches!(facet.target, TargetResolver::ResolvedCollection(_)) {
            return Ok(false);
        }
        expected.push(KeyEmptyFacet {
            action: facet.action,
            resources: facet.resources.clone(),
        });
    }
    Ok(!expected.is_empty()
        && expected == required
        && compatible_selection(caller, variant)
        && caller.permits_empty_operation(selection, required))
}

#[cfg(test)]
mod collection_tests {
    #![allow(clippy::unwrap_used, reason = "test assertions fail by panicking")]

    use super::*;
    use orch_access_contract::key_scope::{KeyObjectRef, KeyScope};
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::operations::{FacetSpec, OperationSpec};
    use orch_api_contract::routes::{Access, Endpoint};
    use orch_types::Id;

    const TOPIC: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Topic);
    const MACHINE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Machine);

    fn plan(collection: bool) -> Prepared {
        const COLLECTION: OperationSpec = OperationSpec::facets(&[FacetSpec::required(
            KeyAction::Read,
            &[TOPIC, MACHINE],
            TargetResolver::ResolvedCollection("actual_rows"),
        )]);
        const SCALAR: OperationSpec = OperationSpec::facets(&[FacetSpec::required(
            KeyAction::Read,
            &[TOPIC, MACHINE],
            TargetResolver::Resolved("actual_row"),
        )]);
        let mut route = orch_api_contract::documents::DocumentOpsCreate::ROUTE;
        route.name = "MixedCollectionControl";
        route.access = Access::User;
        route.operation = Some(if collection { COLLECTION } else { SCALAR });
        Prepared::new(route, route.name).unwrap()
    }

    fn target(kind: KeyTargetKind, id: u128) -> KeyResolvedTarget {
        KeyResolvedTarget {
            object: KeyObjectRef {
                kind,
                id: Some(Id::from_u128(id)),
            },
            node_ancestors: Vec::new(),
            document_ancestors: Vec::new(),
            credential_account: None,
        }
    }

    #[test]
    fn collection_preserves_business_family_separately_from_governing_target() {
        let mut plan = plan(true);
        let topic = target(TOPIC, 1);
        let host = target(KeyTargetKind::Object(ObjectKind::Host), 2);
        plan.bind_collection(
            0,
            &[(TOPIC, vec![topic.clone()]), (MACHINE, vec![host.clone()])],
        )
        .unwrap();
        let actual = plan.required_facets().unwrap();
        assert_eq!(actual.len(), 2);
        assert_eq!(actual[0].resource_kind, TOPIC);
        assert_eq!(actual[0].target, topic);
        assert_eq!(actual[1].resource_kind, MACHINE);
        assert_eq!(actual[1].target, host);
        assert_eq!(actual[1].action, KeyAction::Read);
        assert_eq!(actual[1].operation, "MixedCollectionControl");
    }

    #[test]
    fn a_later_invalid_family_or_empty_group_cannot_leave_a_partial_binding() {
        for invalid in [
            (
                KeyTargetKind::Object(ObjectKind::Document),
                vec![target(TOPIC, 2)],
            ),
            (MACHINE, Vec::new()),
        ] {
            let mut plan = plan(true);
            assert!(
                plan.bind_collection(0, &[(TOPIC, vec![target(TOPIC, 1)]), invalid])
                    .is_err()
            );
            assert!(plan.required_facets().is_err());
            plan.bind_collection(0, &[(TOPIC, vec![target(TOPIC, 1)])])
                .unwrap();
            assert_eq!(plan.required_facets().unwrap().len(), 1);
        }
    }

    #[test]
    fn checked_empty_mixed_collection_accepts_one_real_family_but_keeps_every_policy_layer() {
        use serde_json::json;
        let rule = |operation: &str, action: &str| {
            json!({
                "target": {"kind":"topic", "id":Id::from_u128(1), "reach":"self"},
                "allow": [{"operation":operation, "actions":[action]}]
            })
        };
        let caller = |operation: &str, action: &str, layer: bool| {
            let mut value = json!({"rules":[rule(operation, action)]});
            if layer {
                value["constraints"] = json!([[rule(operation, "write")]]);
            }
            let scope: KeyScope = serde_json::from_value(value).unwrap();
            Caller::token(
                Id::from_u128(9),
                orch_authz::TokenGrant::from_stored_scope(&[], None, Some(scope), None).unwrap(),
            )
        };
        let mut plan = plan(true);
        plan.bind_empty(0).unwrap();
        let admitted = plan
            .admit(&caller("MixedCollectionControl", "read", false), false)
            .unwrap();
        assert!(checked_empty_for(&plan.route, &admitted).unwrap());
        assert!(
            plan.admit(&caller("MixedCollectionControl", "write", false), false)
                .is_err()
        );
        assert!(
            plan.admit(&caller("AnotherCollection", "read", false), false)
                .is_err()
        );
        assert!(
            plan.admit(&caller("MixedCollectionControl", "read", true), false)
                .is_err()
        );
    }

    #[test]
    fn collection_binding_keeps_scalar_duplicate_and_checked_empty_rules() {
        let group = [(TOPIC, vec![target(TOPIC, 1)])];
        assert!(plan(false).bind_collection(0, &group).is_err());
        let mut missing = plan(true);
        assert!(missing.bind_collection(0, &[]).is_err());
        assert!(missing.required_facets().is_err());
        missing.bind_empty(0).unwrap();
        assert!(missing.bind_collection(0, &group).is_err());
        assert!(missing.required_facets().unwrap().is_empty());
        let mut bound = plan(true);
        bound.bind_collection(0, &group).unwrap();
        assert!(bound.bind_collection(0, &group).is_err());
        assert!(bound.bind_empty(0).is_err());
    }

    #[test]
    fn publication_cannot_replace_an_earlier_native_source_with_an_allowed_new_row() {
        let mut earlier = plan(true);
        earlier
            .bind_collection(0, &[(MACHINE, vec![target(MACHINE, 1)])])
            .unwrap();
        let mut replacement = plan(true);
        replacement
            .bind_collection(0, &[(MACHINE, vec![target(MACHINE, 2)])])
            .unwrap();
        assert!(!replacement.covers_sources(&earlier));
        let mut preserved = plan(true);
        preserved
            .bind_collection(
                0,
                &[(MACHINE, vec![target(MACHINE, 1), target(MACHINE, 2)])],
            )
            .unwrap();
        assert!(preserved.covers_sources(&earlier));
        assert!(!earlier.covers_sources(&preserved));
        assert!(!plan(true).covers_sources(&earlier));
    }

    #[test]
    fn publication_identity_matching_never_reuses_an_earlier_ancestor_as_admission() {
        let old_home = target(TOPIC, 11).object;
        let new_home = target(TOPIC, 12).object;
        let mut old = target(KeyTargetKind::Object(ObjectKind::Host), 1);
        old.node_ancestors.push(old_home);
        let mut moved = old.clone();
        moved.node_ancestors = vec![new_home];
        let mut earlier = plan(true);
        earlier.bind_collection(0, &[(MACHINE, vec![old])]).unwrap();
        let mut fresh = plan(true);
        fresh.bind_collection(0, &[(MACHINE, vec![moved])]).unwrap();
        let scope: KeyScope = serde_json::from_value(serde_json::json!({"rules":[{
            "target":{"kind":"topic","id":old_home.id,"reach":"subtree"},
            "resources":["machine"],
            "allow":[{"operation":"MixedCollectionControl","actions":["read"]}]
        }]}))
        .unwrap();
        let caller = Caller::token(
            Id::from_u128(9),
            orch_authz::TokenGrant::from_stored_scope(&[], None, Some(scope), None).unwrap(),
        );
        assert!(
            fresh.covers_sources(&earlier),
            "it is still the actual same subject"
        );
        assert!(earlier.admit(&caller, false).is_ok());
        assert!(
            fresh.admit(&caller, false).is_err(),
            "only current placement admits publication"
        );
    }
}
