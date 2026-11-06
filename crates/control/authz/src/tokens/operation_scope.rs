//! Request-local admission for the same Caller, after the shared operation resolves its facets.

use super::{Caller, Scopes, TokenGrant};
use crate::Error;
use orch_access_contract::key_scope::{
    KeyAction, KeyEmptyFacet, KeyFacet, KeyPlaceExclusion, KeyRule, KeyScope,
};
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::PlaceRef;
use orch_types::{Clock, Id, Time};
use std::collections::BTreeSet;

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
    Empty {
        operation: String,
        required: Vec<KeyEmptyFacet>,
    },
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
                kept_out.push(restriction.clone());
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

    /// Request-local admitted facets, never a persisted target snapshot for later work.
    #[must_use]
    pub fn admitted_facets(&self) -> Option<&[KeyFacet]> {
        match self.admission.as_ref()? {
            Admission::Facets(facets) => Some(facets),
            Admission::Empty { .. } => Some(&[]),
        }
    }

    /// Exact transient selection. A compound set with different selections has no single name.
    #[must_use]
    pub fn admitted_operation(&self) -> Option<&str> {
        match self.admission.as_ref()? {
            Admission::Empty { operation, .. } => Some(operation),
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
            Admission::Facets(_) => None,
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
        Ok(match (self.grant(), current.grant()) {
            (None, None) => {
                Self::session(self.user()).with_admin_mode(self.as_admin() && current.as_admin())
            }
            (Some(accepted), Some(current)) => {
                Self::token(self.user(), accepted.intersect(current))
            }
            (Some(grant), None) | (None, Some(grant)) => Self::token(self.user(), grant.clone()),
        })
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
