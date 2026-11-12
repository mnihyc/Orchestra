//! Request-local admission for the same Caller, after the shared operation resolves its facets.

use super::{Caller, Scopes, TokenGrant};
use crate::Error;
use orch_access_contract::key_scope::{
    KeyAction, KeyEmptyFacet, KeyFacet, KeyPlaceExclusion, KeyRule, KeyScope,
};
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::PlaceRef;
use orch_types::{Clock, Id, Time};
use std::collections::{BTreeSet, HashSet};

/// A credential restriction refusal, never a replacement for a resource permission decision.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyScopeRefusal {
    /// A shared operation has not provided its actual effects/targets.
    #[error("the shared operation has no resolved key-scope facets")]
    Unclassified,
    /// A real facet is outside the key's complete effective policy.
    #[error("the key scope does not cover this operation")]
    NotCovered {
        /// Safe operation identity; target references are projected separately by the caller.
        operation: String,
        /// The missing existing business-action facet.
        action: KeyAction,
    },
    /// A checked-empty operation still needs its declared action/resource selections.
    #[error("the key scope does not cover this empty operation")]
    EmptyNotCovered { operation: String },
    /// Restrictions belonging to different accounts cannot be merged into one acting account.
    #[error("credential boundaries belong to different accounts")]
    DifferentAccount,
    /// Delegation cannot outlive the invoking key's accepted/current deadline.
    #[error("the requested key lifetime exceeds the invoking credential")]
    Lifetime,
    /// A legacy boundary has no explicit grant of the newly available key-issuance operation.
    #[error("an operation-scoped key issuance requires an explicit operation policy")]
    LegacyIssuance,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Admission {
    Facets(Vec<KeyFacet>),
    Preparation(Vec<KeyFacet>),
    Resolved {
        operation: String,
        required: Vec<ResolvedFacetRole>,
        facets: Vec<KeyFacet>,
        empty: Option<Vec<KeyEmptyFacet>>,
    },
    Empty {
        operation: String,
        required: Vec<KeyEmptyFacet>,
    },
}

/// One canonical required role resolved by the shared operation builder, in declaration order.
/// This is current-checkpoint evidence only, never serialized with a captured credential.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedFacetRole {
    /// The canonical action, independent of the actual number of selected targets.
    pub action: KeyAction,
    /// The canonical allowed business families for that role.
    pub resources: Vec<orch_access_contract::key_scope::KeyTargetKind>,
    /// Every actual target for this role; empty only after a real collection selection.
    pub facets: Vec<KeyFacet>,
}

/// Evidence from the shared scope gate. Its Caller is still the original account and credential.
/// It authorizes no resource by itself; the existing authorizer checks ordinary rights next.
#[derive(Debug, Clone)]
#[must_use = "the admitted caller must enter the existing authorization and business path"]
pub struct AdmittedOperation {
    caller: Caller,
}

impl AdmittedOperation {
    /// The same caller, carrying only request-local scope admission.
    #[must_use]
    pub fn caller(&self) -> &Caller {
        &self.caller
    }

    /// Moves that caller into the existing Principal/operation implementation.
    #[must_use]
    pub fn into_caller(self) -> Caller {
        self.caller
    }
}

fn intersect_permissions(left: &Scopes, right: &Scopes) -> Scopes {
    Scopes::Only(
        Permission::ALL
            .iter()
            .copied()
            .filter(|permission| left.allow(*permission) && right.allow(*permission))
            .collect(),
    )
}

fn earlier(left: Option<Time>, right: Option<Time>) -> Option<Time> {
    match (left, right) {
        (Some(left), Some(right)) => Some(if left.get() <= right.get() {
            left
        } else {
            right
        }),
        (left, right) => left.or(right),
    }
}

impl TokenGrant {
    /// Loads the existing token's explicit mode. Rule-mode storage has no permissive legacy
    /// fallback: its old scope words and scalar subtree are empty, and no admission is implied.
    pub fn from_stored_scope(
        scopes: &[String],
        subtree: Option<Id>,
        operation_scope: Option<KeyScope>,
        expires_at: Option<Time>,
    ) -> Result<Self, Error> {
        let mut grant =
            Self::from_stored(scopes, subtree).map_err(|error| Error::Stored(error.to_string()))?;
        if let Some(scope) = operation_scope {
            if !scopes.is_empty() || subtree.is_some() || !scope.validate().is_empty() {
                return Err(Error::Stored(
                    "invalid or ambiguous operation-scoped key".into(),
                ));
            }
            grant.operation_scope = Some(scope);
        }
        grant.expires_at = expires_at;
        Ok(grant)
    }

    /// Explicit operation policy, including all retained conjunctive ceilings.
    #[must_use]
    pub fn operation_scope(&self) -> Option<&KeyScope> {
        self.operation_scope.as_ref()
    }

    /// Whether an old credential boundary also applies. The shared catalogue must preserve
    /// its original token-admission compatibility rather than enabling formerly refused acts.
    #[must_use]
    pub fn requires_legacy_compatibility(&self) -> bool {
        self.operation_scope.is_none() || self.legacy_ceiling.is_some()
    }

    /// The earliest accepted/current key deadline; absence means no recorded deadline.
    #[must_use]
    pub fn expires_at(&self) -> Option<Time> {
        self.expires_at
    }

    /// All actual credentials whose validity remains required for this caller. Issuer-key
    /// provenance of a child key is not added here: issuance captures policy, not live lineage.
    pub fn token_ids(&self) -> impl Iterator<Item = Id> + '_ {
        self.token
            .into_iter()
            .chain(self.other_tokens.iter().copied())
    }

    /// Every retained legacy node anchor; current target paths must satisfy all of them.
    pub fn subtrees(&self) -> impl Iterator<Item = Id> + '_ {
        self.subtree
            .into_iter()
            .chain(self.other_subtrees.iter().copied())
    }

    fn permission_ceiling(&self) -> Scopes {
        match &self.operation_scope {
            None => self.scopes.clone(),
            Some(_) => self.legacy_ceiling.clone().unwrap_or(Scopes::All),
        }
    }

    fn intersect(&self, current: &Self) -> Self {
        let operation_scope = match (&self.operation_scope, &current.operation_scope) {
            (Some(accepted), Some(current)) => Some(accepted.intersect(current)),
            (accepted, current) => accepted.as_ref().or(current.as_ref()).cloned(),
        };
        let ceiling =
            intersect_permissions(&self.permission_ceiling(), &current.permission_ceiling());
        let legacy =
            self.requires_legacy_compatibility() || current.requires_legacy_compatibility();
        let mut roots: Vec<_> = self
            .subtrees()
            .chain(current.subtrees())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let subtree = (!roots.is_empty()).then(|| roots.remove(0));
        let mut tokens: Vec<_> = self
            .token_ids()
            .chain(current.token_ids())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let token = (tokens.len() == 1).then(|| tokens.remove(0));
        let mut kept_out = self.kept_out.clone();
        for restriction in &current.kept_out {
            if !kept_out.contains(restriction) {
                kept_out.push(*restriction);
            }
        }
        let rule_mode = operation_scope.is_some();
        Self {
            // An old continuation serializer must not lose the operation policy and widen it.
            scopes: if rule_mode {
                Scopes::Only(BTreeSet::new())
            } else {
                ceiling.clone()
            },
            subtree,
            token,
            kept_out,
            operation_scope,
            legacy_ceiling: (rule_mode && legacy).then_some(ceiling),
            other_subtrees: roots,
            other_tokens: tokens,
            expires_at: earlier(self.expires_at, current.expires_at),
        }
    }
}

impl Caller {
    /// Whether the earliest retained original/current KEY deadline still permits a new
    /// effect/publication at the live clock. This synchronous check performs no IO and
    /// grants no permission, revocation proof, browser-session liveness or ACL freshness.
    /// Human Session origins have no recorded key deadline; their actual HTTP context,
    /// where relevant, remains the caller's existing responsibility.
    #[must_use]
    pub fn key_deadline_allows(&self, clock: &dyn Clock) -> bool {
        self.grant()
            .and_then(TokenGrant::expires_at)
            .is_none_or(|end| clock.time().get() < end.get())
    }

    /// Preliminary exact-name check. The catalogue supplies known variants; prefixes confer nothing.
    #[must_use]
    pub fn permits_operation(&self, operation: &str) -> bool {
        self.grant()
            .and_then(TokenGrant::operation_scope)
            .is_none_or(|scope| scope.names_operation(operation))
    }

    /// Scope-only decision. Existing resource authority and lifecycle rules must still pass.
    #[must_use]
    pub fn permits_facets(&self, facets: &[KeyFacet]) -> bool {
        self.grant().is_none_or(|grant| {
            grant
                .operation_scope()
                .is_none_or(|scope| scope.allows(facets))
                && facets.iter().all(|facet| {
                    grant
                        .kept_out_of(facet.target.node_path().as_deref())
                        .is_none()
                })
        })
    }

    /// Admits the complete server-resolved operation without replacing the caller. The caller
    /// performs legacy catalogue compatibility and ordinary authorization in the same shared gate.
    /// Facts are current-checkpoint evidence and must be re-resolved for later execution/emission.
    pub fn admit_facets(&self, facets: &[KeyFacet]) -> Result<AdmittedOperation, KeyScopeRefusal> {
        if facets.is_empty() {
            return Err(KeyScopeRefusal::Unclassified);
        }
        if self.grant().is_some() {
            for facet in facets {
                if !self.permits_facets(std::slice::from_ref(facet)) {
                    return Err(KeyScopeRefusal::NotCovered {
                        operation: facet.operation.clone(),
                        action: facet.action,
                    });
                }
            }
        }
        let mut caller = self.clone();
        caller.admission = Some(Admission::Facets(facets.to_vec()));
        Ok(AdmittedOperation { caller })
    }

    /// Common native input Read before the server can resolve a variant. The shared
    /// declaration binder checks that each candidate needs these identical Read roles.
    /// Even one allowed candidate remains unselected: no captured operation or complete
    /// required-role evidence can be obtained from this phase.
    pub fn admit_common_read(&self, facets: &[KeyFacet]) -> Result<Self, KeyScopeRefusal> {
        if facets
            .iter()
            .any(|facet| facet.action != KeyAction::Read || facet.operation.is_empty())
        {
            return Err(KeyScopeRefusal::Unclassified);
        }
        self.admit_native_preparation(facets)
    }

    /// Effect-free native discovery before the canonical producer knows a target.
    /// No exact operation, required role or checked-empty proof is supplied. Native
    /// selection must check each factual candidate before a complete effect gate.
    #[must_use]
    pub fn prepare_unresolved(&self) -> Self {
        let mut caller = self.clone();
        caller.admission = Some(Admission::Preparation(Vec::new()));
        caller
    }

    /// Canonical known native setup roles, without a selected business operation or
    /// complete required-role proof. The shared declaration binder owns their footprint.
    /// This transient caller cannot be captured as an operation or queue native commands.
    pub fn admit_native_preparation(&self, facets: &[KeyFacet]) -> Result<Self, KeyScopeRefusal> {
        if facets.is_empty() || facets.iter().any(|facet| facet.operation.is_empty()) {
            return Err(KeyScopeRefusal::Unclassified);
        }
        let mut caller = self.admit_facets(facets)?.into_caller();
        caller.admission = Some(Admission::Preparation(facets.to_vec()));
        Ok(caller)
    }

    /// Admits complete native bindings while retaining each required role, including actual
    /// empty collections. The shared builder supplies canonical roles and all selected facets;
    /// consumers must compare the roles with that same canonical declaration before using them.
    pub fn admit_resolved_operation(
        &self,
        operation: &str,
        required: &[ResolvedFacetRole],
        facets: &[KeyFacet],
    ) -> Result<AdmittedOperation, KeyScopeRefusal> {
        let selected = facets.iter().collect::<HashSet<_>>();
        if operation.is_empty()
            || required.is_empty()
            || facets.iter().any(|facet| facet.operation != operation)
            || required.iter().any(|role| {
                role.resources.is_empty()
                    || role.facets.iter().any(|facet| {
                        facet.operation != operation
                            || facet.action != role.action
                            || !role.resources.contains(&facet.resource_kind)
                            || !selected.contains(facet)
                    })
            })
        {
            return Err(KeyScopeRefusal::Unclassified);
        }
        let empty = required.iter().all(|role| role.facets.is_empty()).then(|| {
            required
                .iter()
                .map(|role| KeyEmptyFacet {
                    action: role.action,
                    resources: role.resources.clone(),
                })
                .collect::<Vec<_>>()
        });
        if let Some(empty) = &empty
            && !self.permits_empty_operation(operation, empty)
        {
            return Err(KeyScopeRefusal::EmptyNotCovered {
                operation: operation.into(),
            });
        }
        let mut caller = if facets.is_empty() {
            self.admit_empty_operation(
                operation,
                empty.as_deref().ok_or(KeyScopeRefusal::Unclassified)?,
            )?
            .into_caller()
        } else {
            self.admit_facets(facets)?.into_caller()
        };
        caller.admission = Some(Admission::Resolved {
            operation: operation.into(),
            required: required.to_vec(),
            facets: facets.to_vec(),
            empty,
        });
        Ok(AdmittedOperation { caller })
    }

    /// Request-local admitted facets, never a persisted target snapshot for later work.
    #[must_use]
    pub fn admitted_facets(&self) -> Option<&[KeyFacet]> {
        match self.admission.as_ref()? {
            Admission::Facets(facets) | Admission::Preparation(facets) => Some(facets),
            Admission::Resolved { facets, .. } => Some(facets),
            Admission::Empty { .. } => Some(&[]),
        }
    }

    /// Exact transient selection. A compound set with different selections has no single name.
    #[must_use]
    pub fn admitted_operation(&self) -> Option<&str> {
        match self.admission.as_ref()? {
            Admission::Preparation(_) => None,
            Admission::Empty { operation, .. } => Some(operation),
            Admission::Resolved { operation, .. } => Some(operation),
            Admission::Facets(facets) => facets
                .first()
                .filter(|first| {
                    facets
                        .iter()
                        .all(|facet| facet.operation == first.operation)
                })
                .map(|facet| facet.operation.as_str()),
        }
    }

    /// Declared required facets of an explicitly checked-empty operation, never a missing target.
    #[must_use]
    pub fn admitted_empty_facets(&self) -> Option<&[KeyEmptyFacet]> {
        match self.admission.as_ref()? {
            Admission::Empty { required, .. } => Some(required),
            Admission::Resolved { empty, .. } => empty.as_deref(),
            Admission::Facets(_) | Admission::Preparation(_) => None,
        }
    }

    /// Complete required role bindings supplied by the shared builder, never inferred from
    /// flattened targets. Missing/partial preparation and direct facets have no such proof.
    #[must_use]
    pub fn admitted_required_roles(&self) -> Option<&[ResolvedFacetRole]> {
        match self.admission.as_ref()? {
            Admission::Resolved { required, .. } => Some(required),
            Admission::Facets(_) | Admission::Preparation(_) | Admission::Empty { .. } => None,
        }
    }

    /// Scope-only check for required collections explicitly materialized as empty. The shared
    /// builder alone can establish emptiness; singleton/account/System targets cannot use it.
    #[must_use]
    pub fn permits_empty_operation(&self, operation: &str, required: &[KeyEmptyFacet]) -> bool {
        !operation.is_empty()
            && !required.is_empty()
            && required.iter().all(|facet| !facet.resources.is_empty())
            && self.grant().is_none_or(|grant| {
                grant.kept_out_of(None).is_none()
                    && grant
                        .operation_scope()
                        .is_none_or(|scope| scope.allows_empty_operation(operation, required))
            })
    }

    /// Retains exact selection and declared action/resource evidence for a checked-empty operation.
    /// It is not a substitute for an unresolved facet or ordinary account authorization.
    pub fn admit_empty_operation(
        &self,
        operation: &str,
        required: &[KeyEmptyFacet],
    ) -> Result<AdmittedOperation, KeyScopeRefusal> {
        if !self.permits_empty_operation(operation, required) {
            return Err(KeyScopeRefusal::EmptyNotCovered {
                operation: operation.into(),
            });
        }
        let mut caller = self.clone();
        caller.admission = Some(Admission::Empty {
            operation: operation.into(),
            required: required.to_vec(),
        });
        Ok(AdmittedOperation { caller })
    }

    pub(super) fn admission_is_current(&self) -> bool {
        self.admission
            .as_ref()
            .is_none_or(|admission| match admission {
                Admission::Facets(facets) => self.permits_facets(facets),
                // Discovery carries no selected targets to invalidate. Preserve its
                // transient class while refreshing exclusions; ordinary reach still
                // uses those exclusions and the complete gate still needs every role.
                Admission::Preparation(facets) => facets.is_empty() || self.permits_facets(facets),
                Admission::Resolved {
                    operation,
                    facets,
                    empty,
                    ..
                } => {
                    (facets.is_empty() || self.permits_facets(facets))
                        && empty.as_ref().is_none_or(|required| {
                            self.permits_empty_operation(operation, required)
                        })
                }
                Admission::Empty {
                    operation,
                    required,
                } => self.permits_empty_operation(operation, required),
            })
    }

    /// Intersects accepted/current credentials and discards old admission. A key anywhere in
    /// the intersection keeps the caller a key and cannot inherit browser As-admin state.
    pub fn intersect_restrictions(&self, current: &Self) -> Result<Self, KeyScopeRefusal> {
        if self.user() != current.user() {
            return Err(KeyScopeRefusal::DifferentAccount);
        }
        if self.native_machine() != current.native_machine()
            || self.native_transaction() != current.native_transaction()
            || self.owner_read_source() != current.owner_read_source()
        {
            return Err(KeyScopeRefusal::Unclassified);
        }
        if let Some(source) = self.owner_read_source() {
            return Ok(Self::owner_read(self.user(), source));
        }
        let mut result = match (self.grant(), current.grant()) {
            (None, None) => {
                Self::session(self.user()).with_admin_mode(self.as_admin() && current.as_admin())
            }
            (Some(accepted), Some(current)) => {
                Self::token(self.user(), accepted.intersect(current))
            }
            (Some(grant), None) | (None, Some(grant)) => Self::token(self.user(), grant.clone()),
        };
        result.machine = self.native_machine();
        result.native_transaction = self.native_transaction();
        Ok(result)
    }

    /// Reapply retained key ceilings to a freshly admitted native proof from the same actor.
    /// The supplied proof owns current typed facts and ordinary authorization. A changed ordinary
    /// permission/subtree boundary needs its domain's full re-preparation instead of this handoff.
    fn narrow_native_boundary(&self, proof: &Self) -> Result<Self, KeyScopeRefusal> {
        let narrowed = proof.intersect_restrictions(self)?;
        let unchanged_ordinary = match (narrowed.grant(), proof.grant()) {
            (None, None) => narrowed.as_admin() == proof.as_admin(),
            (Some(a), Some(b)) => {
                a.permission_ceiling() == b.permission_ceiling()
                    && a.requires_legacy_compatibility() == b.requires_legacy_compatibility()
                    && a.token_ids().collect::<BTreeSet<_>>()
                        == b.token_ids().collect::<BTreeSet<_>>()
                    && a.subtrees().collect::<BTreeSet<_>>()
                        == b.subtrees().collect::<BTreeSet<_>>()
                    && a.kept_out == b.kept_out
            }
            _ => false,
        };
        if !unchanged_ordinary {
            return Err(KeyScopeRefusal::Unclassified);
        }
        Ok(narrowed)
    }
    /// Retain a complete fresh native role proof; partial preparation stays partial.
    pub fn narrow_current_admission(&self, proof: &Self) -> Result<Self, KeyScopeRefusal> {
        let narrowed = self.narrow_native_boundary(proof)?;
        let operation = proof
            .admitted_operation()
            .ok_or(KeyScopeRefusal::Unclassified)?;
        let required = proof
            .admitted_required_roles()
            .ok_or(KeyScopeRefusal::Unclassified)?;
        let facets = proof
            .admitted_facets()
            .ok_or(KeyScopeRefusal::Unclassified)?;
        Ok(narrowed
            .admit_resolved_operation(operation, required, facets)?
            .into_caller())
    }

    /// Recheck a native source's actual phase while preserving its proof class.
    /// A preparation-only facet/empty proof never becomes complete required-role admission.
    pub fn narrow_source_admission(&self, proof: &Self) -> Result<Self, KeyScopeRefusal> {
        if proof.admitted_required_roles().is_some() {
            return self.narrow_current_admission(proof);
        }
        let narrowed = self.narrow_native_boundary(proof)?;
        proof
            .admitted_operation()
            .ok_or(KeyScopeRefusal::Unclassified)?;
        match proof
            .admission
            .as_ref()
            .ok_or(KeyScopeRefusal::Unclassified)?
        {
            Admission::Facets(facets) => Ok(narrowed.admit_facets(facets)?.into_caller()),
            Admission::Empty {
                operation,
                required,
            } => Ok(narrowed
                .admit_empty_operation(operation, required)?
                .into_caller()),
            Admission::Preparation(_) | Admission::Resolved { .. } => {
                Err(KeyScopeRefusal::Unclassified)
            }
        }
    }

    /// Reapply retained restrictions to an actual known native preparation footprint.
    /// Discovery without facts, complete effects and checked-empty proofs cannot enter it.
    pub fn narrow_preparation_admission(&self, proof: &Self) -> Result<Self, KeyScopeRefusal> {
        let Some(Admission::Preparation(facts)) = &proof.admission else {
            return Err(KeyScopeRefusal::Unclassified);
        };
        if facts.is_empty() {
            return Err(KeyScopeRefusal::Unclassified);
        }
        self.narrow_native_boundary(proof)?
            .admit_native_preparation(facts)
    }

    /// Captures the invoking operation policy as an issuance ceiling, without a live parent link.
    /// The shared key-creation producer still validates current account authority and request shape.
    pub fn attenuate_key_scope(
        &self,
        rules: Vec<KeyRule>,
        expires_at: Option<Time>,
    ) -> Result<KeyScope, KeyScopeRefusal> {
        let Some(grant) = self.grant() else {
            return Ok(KeyScope {
                rules,
                constraints: Vec::new(),
                excluded_places: Vec::new(),
            });
        };
        if grant
            .expires_at
            .is_some_and(|end| expires_at.is_none_or(|child| child.get() > end.get()))
        {
            return Err(KeyScopeRefusal::Lifetime);
        }
        let Some(scope) = grant.operation_scope() else {
            return Err(KeyScopeRefusal::LegacyIssuance);
        };
        if grant.legacy_ceiling.is_some() || grant.subtrees().next().is_some() {
            return Err(KeyScopeRefusal::LegacyIssuance);
        }
        let mut scope = scope.attenuate(rules);
        for excluded in &grant.kept_out {
            let excluded = match *excluded {
                super::KeptOut::Refused { org } => KeyPlaceExclusion::Refused {
                    place: PlaceRef::Node(org),
                },
                super::KeptOut::Lifetime { place, max_days } => KeyPlaceExclusion::Lifetime {
                    place: place.map_or(PlaceRef::System, PlaceRef::Node),
                    max_days,
                },
            };
            if !scope.excluded_places.contains(&excluded) {
                scope.excluded_places.push(excluded);
            }
        }
        Ok(scope)
    }
}

#[cfg(test)]
mod native_measurement_admission {
    use super::*;
    use orch_access_contract::{
        key_scope::{KeyObjectRef, KeyReach, KeyResolvedTarget, KeyTarget, KeyTargetKind},
        permissions::ObjectKind,
    };
    use orch_types::new_id;
    const OP: &str = "ThreadForkPreflight.fork";
    fn scoped(user: Id, token: Id, ids: &[Id]) -> Result<Caller, crate::Error> {
        let scope = KeyScope {
            rules: ids
                .iter()
                .map(|id| KeyRule {
                    target: KeyTarget {
                        kind: KeyTargetKind::Object(ObjectKind::Thread),
                        id: Some(*id),
                        reach: KeyReach::SelfOnly,
                    },
                    resources: None,
                    allow: vec![orch_access_contract::key_scope::KeyOperation {
                        operation: OP.into(),
                        actions: vec![KeyAction::Manage],
                    }],
                })
                .collect(),
            ..KeyScope::default()
        };
        Ok(Caller::token(
            user,
            TokenGrant::from_stored_scope(&[], None, Some(scope), None)?.for_token(token),
        ))
    }
    fn facet(id: Id) -> KeyFacet {
        let kind = KeyTargetKind::Object(ObjectKind::Thread);
        KeyFacet {
            operation: OP.into(),
            action: KeyAction::Manage,
            resource_kind: kind,
            target: KeyResolvedTarget {
                object: KeyObjectRef { kind, id: Some(id) },
                node_ancestors: vec![],
                document_ancestors: vec![],
                credential_account: None,
            },
        }
    }
    #[test]
    fn retained_limits_use_the_actual_fresh_roles_and_never_become_name_only_admission()
    -> Result<(), Box<dyn std::error::Error>> {
        let (user, token, first, second) = (new_id(), new_id(), new_id(), new_id());
        let old = scoped(user, token, &[first])?;
        let broad = scoped(user, token, &[first, second])?;
        let facts = vec![facet(first)];
        let roles = vec![ResolvedFacetRole {
            action: KeyAction::Manage,
            resources: vec![KeyTargetKind::Object(ObjectKind::Thread)],
            facets: facts.clone(),
        }];
        let current = broad
            .admit_resolved_operation(OP, &roles, &facts)?
            .into_caller();
        let retained = old.narrow_current_admission(&current)?;
        assert_eq!(retained.admitted_operation(), Some(OP));
        assert!(retained.permits_facets(&facts));
        assert!(!retained.permits_facets(&[facet(second)]));
        let moved = vec![facet(second)];
        let changed_roles = vec![ResolvedFacetRole {
            facets: moved.clone(),
            ..roles[0].clone()
        }];
        let changed = broad
            .admit_resolved_operation(OP, &changed_roles, &moved)?
            .into_caller();
        assert!(old.narrow_current_admission(&changed).is_err());
        assert!(
            old.narrow_current_admission(&broad).is_err(),
            "an operation name without fresh native roles proves nothing"
        );
        let flat = broad.admit_facets(&facts)?.into_caller();
        let phase = old.narrow_source_admission(&flat)?;
        assert!(
            phase.admitted_required_roles().is_none(),
            "a native preparation phase must not become a completed full-role proof"
        );
        assert!(!phase.permits_facets(&[facet(second)]));
        assert!(
            old.narrow_current_admission(&flat).is_err(),
            "preparation-only/direct facets are not a full native role proof"
        );
        let other = scoped(new_id(), token, &[first])?;
        assert!(other.narrow_current_admission(&current).is_err());
        Ok(())
    }
    #[test]
    fn a_known_preparation_handoff_keeps_its_class_and_never_captures_a_complete_operation()
    -> Result<(), Box<dyn std::error::Error>> {
        let (user, token, target, other) = (new_id(), new_id(), new_id(), new_id());
        let retained = scoped(user, token, &[target])?;
        let current = scoped(user, token, &[target, other])?;
        let actual = vec![facet(target)];
        let prepared = current.admit_native_preparation(&actual)?;
        let narrowed = retained.narrow_preparation_admission(&prepared)?;
        assert_eq!(narrowed.admitted_facets(), Some(actual.as_slice()));
        assert!(narrowed.admitted_operation().is_none());
        assert!(narrowed.admitted_required_roles().is_none());
        assert!(narrowed.admitted_empty_facets().is_none());
        assert!(crate::CapturedOperation::capture(&narrowed).is_err());
        assert!(!narrowed.permits_facets(&[facet(other)]));
        assert!(
            retained
                .narrow_preparation_admission(&current.prepare_unresolved())
                .is_err()
        );
        let foreign = current.admit_native_preparation(&[facet(other)])?;
        assert!(retained.narrow_preparation_admission(&foreign).is_err());
        let complete = current
            .admit_resolved_operation(
                OP,
                &[ResolvedFacetRole {
                    action: KeyAction::Manage,
                    resources: vec![KeyTargetKind::Object(ObjectKind::Thread)],
                    facets: actual.clone(),
                }],
                &actual,
            )?
            .into_caller();
        assert!(retained.narrow_preparation_admission(&complete).is_err());
        Ok(())
    }
    #[test]
    fn a_changed_ordinary_boundary_requires_its_domain_to_reprepare()
    -> Result<(), Box<dyn std::error::Error>> {
        let (user, token, target) = (new_id(), new_id(), new_id());
        let proof = scoped(user, token, &[target])?;
        let facts = vec![facet(target)];
        let roles = vec![ResolvedFacetRole {
            action: KeyAction::Manage,
            resources: vec![KeyTargetKind::Object(ObjectKind::Thread)],
            facets: facts.clone(),
        }];
        let proof = proof
            .admit_resolved_operation(OP, &roles, &facts)?
            .into_caller();
        let old = Caller::token(
            user,
            TokenGrant::from_stored(&["thread.view".into()], Some(new_id()))?.for_token(token),
        );
        assert!(old.narrow_current_admission(&proof).is_err());
        Ok(())
    }
}
