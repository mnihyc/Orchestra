//! Credential restrictions on existing shared operations, never a source of account authority.
//!
//! Each rule keeps operation/action selections bound to one typed target. Rules within one
//! layer are alternatives; all constraint layers must agree. Actual object ancestry comes
//! from the shared operation resolver, never from a URL prefix or a submitted parent hint.

use crate::permissions::ObjectKind;
use crate::roles::PlaceRef;
use orch_types::Id;
use orch_types::fields::FieldError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Independent operation selectors, not a replacement for the human permission hierarchy.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    /// Read or disclose existing material.
    Read,
    /// Write or interact under the operation's existing authority.
    Write,
    /// Manage under the operation's existing authority.
    Manage,
}

impl KeyAction {
    /// All selectors exposed by the shared catalogue and preset UI.
    pub const ALL: [Self; 3] = [Self::Read, Self::Write, Self::Manage];
}

/// Existing identities without permission pages of their own. Their operation still uses its
/// ordinary authorization rule; listing a kind here confers no permission on that identity.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum KeyIdentityKind {
    /// A node, direct or group chat.
    Chat,
    /// A group.
    Group,
    /// An account.
    Account,
    /// An existing API key.
    Key,
    /// A browser session managed by an account operation.
    Session,
    /// A background job.
    Job,
    /// An export and its artifact.
    Export,
    /// An import.
    Import,
    /// An upload.
    Upload,
    /// An invitation.
    Invitation,
    /// An approval.
    Approval,
    /// A schedule.
    Schedule,
    /// A notification.
    Notification,
    /// System-level operations; no object identifier.
    System,
}

/// Typed identities understood by operation targets, reusing all existing permission-page kinds.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(untagged)]
pub enum KeyTargetKind {
    /// An existing permission-page object kind.
    Object(ObjectKind),
    /// An existing non-page identity.
    Identity(KeyIdentityKind),
}

impl KeyTargetKind {
    /// Whether the existing hierarchy can contain targets below this identity.
    #[must_use]
    pub fn supports_subtree(self) -> bool {
        matches!(self, Self::Object(kind) if kind.node().is_some() || kind == ObjectKind::DocumentContainer)
            || self == Self::Identity(KeyIdentityKind::Account)
    }

    /// Whether the target denotes the system rather than an object identifier.
    #[must_use]
    pub fn is_system(self) -> bool {
        self == Self::Identity(KeyIdentityKind::System)
    }
}

/// One actual identity, with no invented permission-page kind for a chat or credential.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct KeyObjectRef {
    /// Its existing identity kind.
    pub kind: KeyTargetKind,
    /// Absent only for the system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
}

/// Whether the target itself or the existing hierarchy beneath it is selected.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum KeyReach {
    /// This identity alone; unrelated or associated resources do not become targets.
    #[serde(rename = "self")]
    SelfOnly,
    /// The root and its actual current descendants.
    Subtree,
}

/// One target boundary in a key rule.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyTarget {
    /// The selected identity kind.
    pub kind: KeyTargetKind,
    /// Absent only for the system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// Self or the existing hierarchy beneath it.
    pub reach: KeyReach,
}

/// Explicit selections from the shared operation catalogue. No wildcard or future preset.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyOperation {
    /// Existing operation ID, including a shared variant where the catalogue defines it.
    pub operation: String,
    /// The selected facets of that operation, independently selected.
    pub actions: Vec<KeyAction>,
}

/// Alternatives inside a scope; its actions never detach from its target.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyRule {
    /// The target bound to these selections.
    pub target: KeyTarget,
    /// Optional resource-kind filter within this target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<Vec<KeyTargetKind>>,
    /// Explicit operation/action selections, already expanded from any UI preset.
    pub allow: Vec<KeyOperation>,
}

/// Snapshot of an existing credential place exclusion, retained when a key is issued.
/// This only narrows the policy; current refusal/lifetime checks remain independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum KeyPlaceExclusion {
    /// The issuing credential was refused in this existing place.
    Refused { place: PlaceRef },
    /// The issuing credential's life was outside this place's bound.
    Lifetime { place: PlaceRef, max_days: i64 },
}

impl KeyPlaceExclusion {
    fn excludes(self, target: &KeyResolvedTarget) -> bool {
        let place = match self {
            Self::Refused { place } | Self::Lifetime { place, .. } => place,
        };
        match place {
            PlaceRef::System => true,
            PlaceRef::Node(id) => target.node_path().is_some_and(|path| path.contains(&id)),
        }
    }
}

/// The selected rules plus retained issuance/acceptance ceilings. This only restricts a caller.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyScope {
    /// Union of the key's selected bound rules. Empty allows no operation.
    pub rules: Vec<KeyRule>,
    /// Additional bound-rule unions, each required independently. Child keys retain an
    /// issuing key's policy ceiling here; no live parent credential hierarchy is created.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<Vec<KeyRule>>,
    /// Server-generated issuance ceilings from existing refusal/lifetime policy. No live
    /// issuer-key dependency is retained, and current place policy still applies at use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_places: Vec<KeyPlaceExclusion>,
}

/// Facts resolved by an existing shared operation from its actual stored objects.
/// This has no Deserialize implementation: request bodies are not ancestry evidence.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyResolvedTarget {
    /// The actual governing business resource for this facet.
    pub object: KeyObjectRef,
    /// Current typed node ancestors, including the owning node for a resource beside it.
    pub node_ancestors: Vec<KeyObjectRef>,
    /// Current document-container ancestors, kept separate from the node hierarchy.
    pub document_ancestors: Vec<KeyObjectRef>,
    /// Current real Key.user_id or Session.user_id, as an Account reference. This relation
    /// exists only for credentials; it never scopes arbitrary documents or owned resources.
    pub credential_account: Option<KeyObjectRef>,
}

impl KeyResolvedTarget {
    /// The existing node path used for ordinary token refusal/lifetime checks. Document-folder
    /// ancestry is not substituted for node policy, and unrelated resources are never added.
    #[must_use]
    pub fn node_path(&self) -> Option<Vec<Id>> {
        let mut path: Vec<_> = self
            .node_ancestors
            .iter()
            .filter_map(|object| {
                matches!(object.kind, KeyTargetKind::Object(kind) if kind.node().is_some())
                    .then_some(object.id)
                    .flatten()
            })
            .collect();
        if matches!(self.object.kind, KeyTargetKind::Object(kind) if kind.node().is_some())
            && let Some(id) = self.object.id
            && !path.contains(&id)
        {
            path.push(id);
        }
        (!path.is_empty()).then_some(path)
    }
}

/// A real effect or disclosure declared by the shared operation. Compound calls need them all.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyFacet {
    /// Shared operation/variant, derived from the actual request.
    pub operation: String,
    /// Existing business-action facet.
    pub action: KeyAction,
    /// Declared business resource family. Creation can write a Document while its actual
    /// authorization target is the existing Topic or DocumentContainer parent.
    pub resource_kind: KeyTargetKind,
    /// Actual target and current ancestry, resolved on the server.
    pub target: KeyResolvedTarget,
}

/// A required catalogue facet whose collection has been explicitly resolved to zero targets.
/// Only the shared materialized operation may supply this evidence; no request deserializer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyEmptyFacet {
    /// The declared required action, even though it has no materialized targets.
    pub action: KeyAction,
    /// Its declared possible resource families; empty is not a resolved declaration.
    pub resources: Vec<KeyTargetKind>,
}

impl KeyRule {
    fn selects(&self, operation: &str, action: KeyAction, resource: KeyTargetKind) -> bool {
        self.resources
            .as_ref()
            .is_none_or(|kinds| kinds.contains(&resource))
            && self.allow.iter().any(|selection| {
                selection.operation == operation && selection.actions.contains(&action)
            })
    }

    fn allows(&self, facet: &KeyFacet) -> bool {
        self.selects(&facet.operation, facet.action, facet.resource_kind)
            && self.target.matches(&facet.target)
    }
}

impl KeyTarget {
    /// Checks typed current facts, never a related resource or a raw identifier coincidence.
    #[must_use]
    pub fn matches(&self, actual: &KeyResolvedTarget) -> bool {
        if self.kind.is_system() != self.id.is_none()
            || self.id.is_some_and(|id| id.is_nil())
            || (self.reach == KeyReach::Subtree && !self.kind.supports_subtree())
        {
            return false;
        }
        let selected = KeyObjectRef {
            kind: self.kind,
            id: self.id,
        };
        if selected == actual.object {
            return true;
        }
        self.reach == KeyReach::Subtree
            && self.kind.supports_subtree()
            && match self.kind {
                KeyTargetKind::Object(ObjectKind::DocumentContainer) => {
                    actual.document_ancestors.contains(&selected)
                }
                KeyTargetKind::Identity(KeyIdentityKind::Account) => {
                    matches!(
                        actual.object.kind,
                        KeyTargetKind::Identity(KeyIdentityKind::Key | KeyIdentityKind::Session)
                    ) && actual.credential_account == Some(selected)
                }
                _ => actual.node_ancestors.contains(&selected),
            }
    }
}

impl KeyScope {
    /// Every conjunctive layer, including the key's own rules.
    pub fn layers(&self) -> impl Iterator<Item = &[KeyRule]> {
        std::iter::once(self.rules.as_slice()).chain(self.constraints.iter().map(Vec::as_slice))
    }

    /// A preliminary operation-name check. Actual facets and targets are still required.
    #[must_use]
    pub fn names_operation(&self, operation: &str) -> bool {
        self.layers().all(|rules| {
            rules.iter().any(|rule| {
                rule.allow
                    .iter()
                    .any(|selected| selected.operation == operation && !selected.actions.is_empty())
            })
        })
    }

    /// True only when every actual facet has a matching bound rule in every policy layer.
    #[must_use]
    pub fn allows(&self, facets: &[KeyFacet]) -> bool {
        !facets.is_empty()
            && facets.iter().all(|facet| {
                !self
                    .excluded_places
                    .iter()
                    .any(|excluded| excluded.excludes(&facet.target))
                    && self
                        .layers()
                        .all(|rules| rules.iter().any(|rule| rule.allows(facet)))
            })
    }

    /// Checks only an explicitly empty materialized operation. The exact action and a common
    /// declared family must still be selected in every layer. Missing targets are not empty
    /// collections, and singleton/account/System operations cannot use this path.
    #[must_use]
    pub fn allows_empty_operation(&self, operation: &str, required: &[KeyEmptyFacet]) -> bool {
        !operation.is_empty()
            && !required.is_empty()
            && !self.excluded_places.iter().any(|excluded| {
                matches!(
                    excluded,
                    KeyPlaceExclusion::Refused {
                        place: PlaceRef::System
                    } | KeyPlaceExclusion::Lifetime {
                        place: PlaceRef::System,
                        ..
                    }
                )
            })
            && required.iter().all(|facet| {
                facet.resources.iter().any(|resource| {
                    self.layers().all(|rules| {
                        rules
                            .iter()
                            .any(|rule| rule.selects(operation, facet.action, *resource))
                    })
                })
            })
    }

    /// Intersects complete policies without creating an operation/target cross product.
    #[must_use]
    pub fn intersect(&self, current: &Self) -> Self {
        let mut result = self.clone();
        for layer in current.layers() {
            if layer != result.rules.as_slice()
                && !result.constraints.iter().any(|old| old == layer)
            {
                result.constraints.push(layer.to_vec());
            }
        }
        for exclusion in &current.excluded_places {
            if !result.excluded_places.contains(exclusion) {
                result.excluded_places.push(*exclusion);
            }
        }
        result
    }

    /// Issuance snapshots the invoking key's effective ceiling. Exact D AND subtree P remains
    /// conjunctive when D moves; later parent-key revocation is not a child-key revocation.
    #[must_use]
    pub fn attenuate(&self, rules: Vec<KeyRule>) -> Self {
        Self {
            rules,
            constraints: self.layers().map(<[KeyRule]>::to_vec).collect(),
            excluded_places: self.excluded_places.clone(),
        }
    }

    /// Structural checks. The shared catalogue separately validates operation names/actions.
    #[must_use]
    pub fn validate(&self) -> Vec<FieldError> {
        let mut faults = Vec::new();
        for (index, exclusion) in self.excluded_places.iter().enumerate() {
            let place = match exclusion {
                KeyPlaceExclusion::Refused { place }
                | KeyPlaceExclusion::Lifetime { place, .. } => place,
            };
            if place.node().is_some_and(|id| id.is_nil()) {
                faults.push(FieldError {
                    path: format!("excluded_places.{index}.place"),
                    message: "must name an existing place".into(),
                });
            }
        }
        for (layer_index, layer) in self.layers().enumerate() {
            let prefix = if layer_index == 0 {
                "rules".to_owned()
            } else {
                format!("constraints.{}", layer_index - 1)
            };
            for (index, rule) in layer.iter().enumerate() {
                let at = format!("{prefix}.{index}");
                let mut fault = |field: &str, message: &str| {
                    faults.push(FieldError {
                        path: format!("{at}.{field}"),
                        message: message.into(),
                    })
                };
                if rule.target.kind.is_system() != rule.target.id.is_none() {
                    fault(
                        "target.id",
                        "is required for an object and absent for system scope",
                    );
                }
                if rule.target.id.is_some_and(|id| id.is_nil()) {
                    fault("target.id", "must name an existing identity");
                }
                if rule.target.reach == KeyReach::Subtree && !rule.target.kind.supports_subtree() {
                    fault("target.reach", "this identity has no subtree");
                }
                for selection in &rule.allow {
                    if selection.operation.is_empty()
                        || selection.operation.contains('*')
                        || selection.operation.trim() != selection.operation
                    {
                        fault("allow", "operations must be explicit catalogue identifiers");
                    }
                    if selection.actions.is_empty() {
                        fault("allow", "an operation selection needs an action");
                    }
                }
            }
        }
        faults
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a contract control fails by panicking")]
mod tests {
    use super::*;
    use orch_types::new_id;
    use serde_json::json;

    fn object(kind: ObjectKind, id: Id) -> KeyObjectRef {
        KeyObjectRef {
            kind: KeyTargetKind::Object(kind),
            id: Some(id),
        }
    }

    fn rule(target: KeyObjectRef, reach: KeyReach, operation: &str, action: KeyAction) -> KeyRule {
        KeyRule {
            target: KeyTarget {
                kind: target.kind,
                id: target.id,
                reach,
            },
            resources: None,
            allow: vec![KeyOperation {
                operation: operation.into(),
                actions: vec![action],
            }],
        }
    }

    fn facet(target: KeyObjectRef, operation: &str, action: KeyAction) -> KeyFacet {
        KeyFacet {
            operation: operation.into(),
            action,
            resource_kind: target.kind,
            target: KeyResolvedTarget {
                object: target,
                node_ancestors: Vec::new(),
                document_ancestors: Vec::new(),
                credential_account: None,
            },
        }
    }

    #[test]
    fn action_operation_and_target_stay_bound_to_each_rule() {
        let a = object(ObjectKind::Document, new_id());
        let b = object(ObjectKind::Document, new_id());
        let scope = KeyScope {
            rules: vec![
                rule(a, KeyReach::SelfOnly, "DocumentGet", KeyAction::Read),
                rule(b, KeyReach::SelfOnly, "DocumentOpsCreate", KeyAction::Write),
            ],
            constraints: Vec::new(),
            excluded_places: Vec::new(),
        };
        assert!(scope.allows(&[facet(a, "DocumentGet", KeyAction::Read)]));
        assert!(scope.allows(&[facet(b, "DocumentOpsCreate", KeyAction::Write)]));
        assert!(!scope.allows(&[facet(a, "DocumentOpsCreate", KeyAction::Write)]));
        assert!(!scope.allows(&[facet(b, "DocumentGet", KeyAction::Read)]));
        assert!(!scope.allows(&[facet(b, "DocumentOpsCreate", KeyAction::Read)]));
        assert!(!scope.names_operation("DocumentOpsCreate.future_variant"));
    }

    #[test]
    fn compound_operations_require_every_actual_source_and_destination_facet() {
        let source = object(ObjectKind::Document, new_id());
        let destination = object(ObjectKind::Topic, new_id());
        let read = facet(source, "DocumentTreeOperate.copy", KeyAction::Read);
        let mut write = facet(destination, "DocumentTreeOperate.copy", KeyAction::Write);
        write.resource_kind = KeyTargetKind::Object(ObjectKind::Document);
        let mut scope = KeyScope {
            rules: vec![rule(
                source,
                KeyReach::SelfOnly,
                &read.operation,
                KeyAction::Read,
            )],
            constraints: Vec::new(),
            excluded_places: Vec::new(),
        };
        assert!(!scope.allows(&[read.clone(), write.clone()]));
        scope.rules.push(rule(
            destination,
            KeyReach::SelfOnly,
            &write.operation,
            KeyAction::Write,
        ));
        assert!(scope.allows(&[read, write]));
        assert!(!scope.allows(&[]));
    }

    #[test]
    fn document_creation_uses_its_declared_family_and_real_parent_identity() {
        let topic = object(ObjectKind::Topic, new_id());
        let mut selected = rule(
            topic,
            KeyReach::Subtree,
            "NodeDocumentCreate",
            KeyAction::Write,
        );
        selected.resources = Some(vec![KeyTargetKind::Object(ObjectKind::Document)]);
        let scope = KeyScope {
            rules: vec![selected],
            constraints: Vec::new(),
            excluded_places: Vec::new(),
        };
        let mut create = facet(topic, "NodeDocumentCreate", KeyAction::Write);
        assert!(!scope.allows(&[create.clone()]));
        create.resource_kind = KeyTargetKind::Object(ObjectKind::Document);
        assert!(scope.allows(&[create.clone()]));
        create.target.object.kind = KeyTargetKind::Object(ObjectKind::Document);
        assert!(
            !scope.allows(&[create]),
            "creation cannot relabel its parent as a document"
        );
    }

    #[test]
    fn node_and_document_ancestry_are_typed_and_exact_targets_have_no_implicit_relatives() {
        let id = new_id();
        let folder = object(ObjectKind::DocumentContainer, id);
        let topic = object(ObjectKind::Topic, id);
        let document = object(ObjectKind::Document, new_id());
        let scope = KeyScope {
            rules: vec![rule(
                folder,
                KeyReach::Subtree,
                "DocumentGet",
                KeyAction::Read,
            )],
            constraints: Vec::new(),
            excluded_places: Vec::new(),
        };
        let mut get = facet(document, "DocumentGet", KeyAction::Read);
        get.target.node_ancestors.push(topic);
        assert!(
            !scope.allows(&[get.clone()]),
            "equal UUIDs are not equal typed ancestors"
        );
        get.target.document_ancestors.push(folder);
        assert!(scope.allows(&[get]));
        let thread = object(ObjectKind::Thread, new_id());
        let exact = KeyScope {
            rules: vec![rule(
                thread,
                KeyReach::SelfOnly,
                "EnvironmentGet",
                KeyAction::Read,
            )],
            constraints: Vec::new(),
            excluded_places: Vec::new(),
        };
        let mut environment = facet(
            object(ObjectKind::Environment, new_id()),
            "EnvironmentGet",
            KeyAction::Read,
        );
        environment.target.node_ancestors.push(thread);
        assert!(!exact.allows(&[environment]));
    }

    #[test]
    fn attenuation_retains_exact_object_and_issuer_subtree_after_a_move() {
        let project = object(ObjectKind::Project, new_id());
        let elsewhere = object(ObjectKind::Project, new_id());
        let document = object(ObjectKind::Document, new_id());
        let issuer = KeyScope {
            rules: vec![rule(
                project,
                KeyReach::Subtree,
                "DocumentGet",
                KeyAction::Read,
            )],
            constraints: Vec::new(),
            excluded_places: Vec::new(),
        };
        let child = issuer.attenuate(vec![rule(
            document,
            KeyReach::SelfOnly,
            "DocumentGet",
            KeyAction::Read,
        )]);
        let mut get = facet(document, "DocumentGet", KeyAction::Read);
        get.target.node_ancestors.push(project);
        assert!(child.allows(&[get.clone()]));
        get.target.node_ancestors = vec![elsewhere];
        assert!(!child.allows(&[get]));
        assert_eq!(child.constraints, vec![issuer.rules]);
    }

    #[test]
    fn accepted_and_current_policies_intersect_without_cross_products_or_future_operations() {
        let document = object(ObjectKind::Document, new_id());
        let accepted = KeyScope {
            rules: vec![rule(
                document,
                KeyReach::SelfOnly,
                "DocumentGet",
                KeyAction::Read,
            )],
            constraints: Vec::new(),
            excluded_places: Vec::new(),
        };
        let current = KeyScope {
            rules: vec![rule(
                document,
                KeyReach::SelfOnly,
                "DocumentOpsCreate",
                KeyAction::Write,
            )],
            constraints: Vec::new(),
            excluded_places: Vec::new(),
        };
        let restricted = accepted.intersect(&current);
        assert!(!restricted.allows(&[facet(document, "DocumentGet", KeyAction::Read)]));
        assert!(!restricted.allows(&[facet(document, "DocumentOpsCreate", KeyAction::Write)]));
        assert!(!KeyScope::default().allows(&[facet(document, "DocumentGet", KeyAction::Read)]));
    }

    #[test]
    fn serialized_rules_support_exact_chat_and_reject_wildcards_and_fake_subtrees() {
        let chat = json!({"rules":[{"target":{"kind":"chat","id":new_id(),"reach":"self"},
            "allow":[{"operation":"ChatMessageCreate","actions":["write"]}]}]});
        let scope: KeyScope = serde_json::from_value(chat.clone()).unwrap();
        assert!(scope.validate().is_empty());
        assert_eq!(serde_json::to_value(&scope).unwrap(), chat);
        let mut wildcard = scope.clone();
        wildcard.rules[0].allow[0].operation = "*".into();
        assert!(!wildcard.validate().is_empty());
        let mut invalid = scope;
        invalid.rules[0].target.reach = KeyReach::Subtree;
        assert!(!invalid.validate().is_empty());
    }

    #[test]
    fn account_subtrees_match_only_actual_keys_and_sessions_owned_by_that_account() {
        let account = KeyObjectRef {
            kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
            id: Some(new_id()),
        };
        let other = KeyObjectRef {
            id: Some(new_id()),
            ..account
        };
        let subtree = KeyScope {
            rules: vec![rule(
                account,
                KeyReach::Subtree,
                "CredentialManage",
                KeyAction::Manage,
            )],
            ..KeyScope::default()
        };
        let exact_account = KeyScope {
            rules: vec![rule(
                account,
                KeyReach::SelfOnly,
                "CredentialManage",
                KeyAction::Manage,
            )],
            ..KeyScope::default()
        };
        assert!(subtree.validate().is_empty());
        for kind in [KeyIdentityKind::Key, KeyIdentityKind::Session] {
            let credential = KeyObjectRef {
                kind: KeyTargetKind::Identity(kind),
                id: Some(new_id()),
            };
            let mut request = facet(credential, "CredentialManage", KeyAction::Manage);
            assert!(
                !subtree.allows(&[request.clone()]),
                "membership needs a current stored owner fact"
            );
            request.target.credential_account = Some(account);
            assert!(subtree.allows(&[request.clone()]));
            assert!(
                !exact_account.allows(&[request.clone()]),
                "Account self is not an alias for credentials"
            );
            request.target.credential_account = Some(other);
            assert!(!subtree.allows(&[request]));
        }
        for kind in [
            ObjectKind::Document,
            ObjectKind::Environment,
            ObjectKind::Provider,
        ] {
            let mut unrelated = facet(
                object(kind, new_id()),
                "CredentialManage",
                KeyAction::Manage,
            );
            unrelated.target.credential_account = Some(account);
            assert!(
                !subtree.allows(&[unrelated]),
                "owning arbitrary resources creates no credential subtree"
            );
        }
    }

    #[test]
    fn current_account_facts_narrow_child_policy_without_changing_exact_key_identity() {
        let account = KeyObjectRef {
            kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
            id: Some(new_id()),
        };
        let key = KeyObjectRef {
            kind: KeyTargetKind::Identity(KeyIdentityKind::Key),
            id: Some(new_id()),
        };
        let issuer = KeyScope {
            rules: vec![rule(
                account,
                KeyReach::Subtree,
                "MeTokenDelete",
                KeyAction::Manage,
            )],
            ..KeyScope::default()
        };
        let exact = KeyScope {
            rules: vec![rule(
                key,
                KeyReach::SelfOnly,
                "MeTokenDelete",
                KeyAction::Manage,
            )],
            ..KeyScope::default()
        };
        let child = issuer.attenuate(exact.rules.clone());
        let mut request = facet(key, "MeTokenDelete", KeyAction::Manage);
        request.target.credential_account = Some(account);
        assert!(child.allows(&[request.clone()]));
        request.target.credential_account = Some(KeyObjectRef {
            id: Some(new_id()),
            ..account
        });
        assert!(
            !child.allows(&[request.clone()]),
            "the retained account ceiling follows current credential ownership"
        );
        assert!(
            exact.allows(&[request]),
            "exact identity remains exact; ordinary account authority is checked separately"
        );
    }
    #[test]
    fn checked_empty_still_requires_declared_actions_and_a_common_resource_family() {
        let account = KeyObjectRef {
            kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
            id: Some(new_id()),
        };
        let session = KeyTargetKind::Identity(KeyIdentityKind::Session);
        let key = KeyTargetKind::Identity(KeyIdentityKind::Key);
        let required = [KeyEmptyFacet {
            action: KeyAction::Manage,
            resources: vec![session],
        }];
        let mut scope = KeyScope {
            rules: vec![rule(
                account,
                KeyReach::Subtree,
                "MeSessionOthersDelete",
                KeyAction::Read,
            )],
            ..KeyScope::default()
        };
        assert!(scope.names_operation("MeSessionOthersDelete"));
        assert!(
            !scope.allows_empty_operation("MeSessionOthersDelete", &required),
            "a response-Read selection cannot stand for required Manage"
        );
        scope.rules[0].allow[0].actions = vec![KeyAction::Manage];
        scope.rules[0].resources = Some(vec![session]);
        assert!(scope.allows_empty_operation("MeSessionOthersDelete", &required));
        assert!(!scope.allows_empty_operation("Unknown", &required));
        assert!(!scope.allows_empty_operation("MeSessionOthersDelete", &[]));
        let mut ceiling = scope.rules[0].clone();
        ceiling.resources = Some(vec![key]);
        scope.constraints.push(vec![ceiling]);
        let alternatives = [KeyEmptyFacet {
            action: KeyAction::Manage,
            resources: vec![session, key],
        }];
        assert!(
            !scope.allows_empty_operation("MeSessionOthersDelete", &alternatives),
            "different layers cannot authorize different families by a cross product"
        );
        scope.constraints.clear();
        scope.excluded_places.push(KeyPlaceExclusion::Lifetime {
            place: PlaceRef::System,
            max_days: 1,
        });
        assert!(!scope.allows_empty_operation("MeSessionOthersDelete", &required));
    }
}
