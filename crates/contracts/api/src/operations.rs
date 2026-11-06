//! Business operations declared by the shared HTTP contract.
//!
//! This is metadata, not another permission engine. A facet describes an existing effect
//! or disclosure. The operation resolves its actual typed targets and uses the caller's
//! ordinary authorization; a key can only restrict that caller. HTTP verbs never select
//! actions, and an unclassified declaration cannot enter the catalogue.

use orch_access_contract::key_scope::{
    KeyAction, KeyIdentityKind, KeyOperation, KeyScope, KeyTargetKind,
};
use orch_access_contract::permissions::{
    Area, Level, NodePermission, ObjectKind, Permission, PermissionName,
};
use schemars::JsonSchema;
use serde::Serialize;

use crate::routes::{Access, Route};

/// Whether a declaration is an operation a person can delegate to a credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Delegation {
    /// An ordinary shared operation, still subject to all existing authority checks.
    Business,
    /// A public observation with no authenticated authority to delegate.
    Public,
    /// Establishing credentials or redeeming an already authorized protocol continuation.
    AuthenticationProtocol,
    /// The browser's elevation and private-inspection override, which is never delegated.
    BrowserElevation,
}

/// How the operation obtains the governing target. These are resolver declarations,
/// never user-supplied target evidence or permission inferred from a URL prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TargetResolver {
    /// The authenticated account, including self-service actions without a path ID.
    CurrentAccount,
    /// System scope, which has no object ID and still requires ordinary authority.
    System,
    /// An explicitly named path parameter identifying the governing object.
    Path(&'static str),
    /// A direct existing identity whose kind is declared by the ordinary producer.
    /// The business resource family may differ from this target kind, notably at creation.
    TypedPath {
        /// The declared path parameter containing its existing identity.
        parameter: &'static str,
        /// The exact stored identity kind, never inferred from a URL or resource family.
        kind: KeyTargetKind,
    },
    /// The existing fixed-permission resolver supplies the governing object.
    PermissionObject,
    /// The shared operation resolves this role from its body, stored objects or results.
    Resolved(&'static str),
    /// A typed producer resolves every member of this collection. An explicitly empty
    /// collection is distinct from a missing binding and has no actual effects to admit.
    ResolvedCollection(&'static str),
}

/// Whether a facet is necessary for the effect or only for returning existing content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FacetUsage {
    /// Necessary to perform this variant, including source reads inherent in a copy.
    Required,
    /// Additional admission for existing content in a mutation response. A receipt does
    /// not require this facet; it must not be made a prerequisite of a write-only call.
    ResponseContent,
}

/// A static facet written beside an endpoint declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FacetSpec {
    /// Independent business-action selector.
    pub action: KeyAction,
    /// Resource families affected; creation can target a parent of a different kind.
    pub resources: &'static [KeyTargetKind],
    /// The operation's source of actual target identities.
    pub target: TargetResolver,
    /// Effect admission or optional disclosure admission.
    pub usage: FacetUsage,
}

impl FacetSpec {
    /// A facet necessary to perform an operation.
    #[must_use]
    pub const fn required(
        action: KeyAction,
        resources: &'static [KeyTargetKind],
        target: TargetResolver,
    ) -> Self {
        Self {
            action,
            resources,
            target,
            usage: FacetUsage::Required,
        }
    }

    /// Optional admission for returning existing content through this same operation.
    #[must_use]
    pub const fn response_content(
        resources: &'static [KeyTargetKind],
        target: TargetResolver,
    ) -> Self {
        Self {
            action: KeyAction::Read,
            resources,
            target,
            usage: FacetUsage::ResponseContent,
        }
    }
}

/// One server-derived variant of a compound operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VariantSpec {
    /// Stable suffix, for example `copy` in `DocumentTreeOperate.copy`.
    pub name: &'static str,
    /// The actual request or stored-object condition selecting this variant.
    pub selector: &'static str,
    /// All effects and optional disclosures of the variant.
    pub facets: &'static [FacetSpec],
}

/// Static metadata accepted by the `operation` endpoint-declaration suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationSpec {
    /// Exact selections historically reachable by permission-scoped credentials.
    /// This records compatibility only; explicit operation scopes use the same ordinary rules.
    LegacyTokens(&'static [&'static str], &'static OperationSpec),
    /// The shared producer implements a typed receipt alternative for this declaration.
    /// This changes response representation only, never actions, targets or authority.
    Receipt(&'static OperationSpec),
    /// One required facet and no declared existing-content response.
    Single(FacetSpec),
    /// One required facet with optional same-target content in the response.
    Returning(FacetSpec),
    /// Multiple facets under the endpoint's unchanged stable operation ID.
    Facets(&'static [FacetSpec]),
    /// Exact selectable variants; the unsuffixed endpoint ID is not an alias for them.
    Variants(&'static [VariantSpec]),
    /// No authenticated business authority to delegate.
    Public,
    /// Authentication or continuation mechanics, not a business grant.
    Protocol,
    /// A browser elevation/private-inspection override.
    BrowserElevation,
}

impl OperationSpec {
    /// Record existing runtime credential restrictions which Access alone cannot express.
    #[must_use]
    pub const fn legacy_tokens(
        selections: &'static [&'static str],
        operation: &'static Self,
    ) -> Self {
        Self::LegacyTokens(selections, operation)
    }
    /// Mark a producer whose declared JSON reply includes its typed receipt alternative.
    #[must_use]
    pub const fn receipt(operation: &'static Self) -> Self {
        Self::Receipt(operation)
    }
    /// Declare a simple action, with receipt-only output if it mutates state.
    #[must_use]
    pub const fn single(
        action: KeyAction,
        resources: &'static [KeyTargetKind],
        target: TargetResolver,
    ) -> Self {
        Self::Single(FacetSpec::required(action, resources, target))
    }

    /// Declare an action and an optional Read facet for existing response content.
    #[must_use]
    pub const fn returning(
        action: KeyAction,
        resources: &'static [KeyTargetKind],
        target: TargetResolver,
    ) -> Self {
        Self::Returning(FacetSpec::required(action, resources, target))
    }

    /// Declare several effects or disclosures under the endpoint's existing ID.
    #[must_use]
    pub const fn facets(facets: &'static [FacetSpec]) -> Self {
        Self::Facets(facets)
    }

    /// Declare a closed set of server-derived variants.
    #[must_use]
    pub const fn variants(variants: &'static [VariantSpec]) -> Self {
        Self::Variants(variants)
    }

    /// Public observations have no authenticated grant to delegate.
    #[must_use]
    pub const fn public() -> Self {
        Self::Public
    }

    /// Authentication and continuation protocols keep their existing proof requirements.
    #[must_use]
    pub const fn protocol() -> Self {
        Self::Protocol
    }

    /// Browser elevation/private inspection is never delegated to a key.
    #[must_use]
    pub const fn browser_elevation() -> Self {
        Self::BrowserElevation
    }
}

/// A facet displayed in the key editor and consumed by the shared operation resolver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct OperationFacet {
    /// Independent action selection; it does not alter the account's permissions.
    pub action: KeyAction,
    /// Supported business resource families.
    pub resources: Vec<KeyTargetKind>,
    /// Resolver for the actual target of this facet.
    pub target: TargetResolver,
    /// Required effect or optional response-content admission.
    pub usage: FacetUsage,
}

impl From<FacetSpec> for OperationFacet {
    fn from(facet: FacetSpec) -> Self {
        Self {
            action: facet.action,
            resources: facet.resources.to_vec(),
            target: facet.target,
            usage: facet.usage,
        }
    }
}

/// An exact selectable operation, including a suffix only for a compound variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct OperationVariant {
    /// Historical permission-token eligibility, not an additional explicit-key grant.
    pub legacy_tokens: bool,
    /// The exact value saved in `KeyOperation.operation`; never a wildcard.
    pub operation: String,
    /// Stable variant suffix; empty for a simple operation.
    pub name: String,
    /// Actual request/stored-object condition, derived by the server rather than the client.
    pub selector: String,
    /// All relevant business effects and optional disclosures.
    pub facets: Vec<OperationFacet>,
}

/// One canonical endpoint and its business capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Operation {
    /// Internal execution capability; the public reply schema describes the actual union.
    #[serde(skip)]
    pub receipt: bool,
    /// The existing Endpoint name, identical to OpenAPI `operationId`.
    pub operation_id: String,
    /// HTTP method from the route declaration.
    pub method: String,
    /// Route template from the route declaration, without any query or target values.
    pub path: String,
    /// Human-readable endpoint summary.
    pub title: String,
    /// Business, public, protocol or excluded browser elevation.
    pub delegation: Delegation,
    /// Exact selections supported now; future variants are not included in saved keys.
    pub variants: Vec<OperationVariant>,
}

/// An incomplete or inconsistent declaration. Generation and callers fail closed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogueError {
    /// A non-permission declaration needs explicit metadata.
    #[error("operation {0} has no business metadata")]
    MissingMetadata(String),
    /// The permission cannot supply a business facet without an explicit declaration.
    #[error("operation {operation} needs explicit metadata for permission {permission}")]
    UnclassifiedPermission {
        operation: String,
        permission: String,
    },
    /// A declaration cannot be offered as an unambiguous exact selection.
    #[error("operation {operation} has invalid metadata: {reason}")]
    Invalid { operation: String, reason: String },
}

fn invalid(route: &Route, reason: impl Into<String>) -> CatalogueError {
    CatalogueError::Invalid {
        operation: route.name.to_owned(),
        reason: reason.into(),
    }
}

fn permission_facet(route: &Route, key: &str) -> Result<OperationFacet, CatalogueError> {
    let unknown = || CatalogueError::UnclassifiedPermission {
        operation: route.name.to_owned(),
        permission: key.to_owned(),
    };
    let permission = key.parse::<PermissionName>().map_err(|_| unknown())?;
    let (action, resources) = match permission {
        PermissionName::OfNode(permission) => {
            let action = match permission {
                NodePermission::View => KeyAction::Read,
                NodePermission::Interact => KeyAction::Write,
                NodePermission::Manage | NodePermission::Archive => KeyAction::Manage,
            };
            (
                action,
                vec![
                    ObjectKind::Org,
                    ObjectKind::Project,
                    ObjectKind::Topic,
                    ObjectKind::Thread,
                ]
                .into_iter()
                .map(KeyTargetKind::Object)
                .collect(),
            )
        }
        PermissionName::Exact(permission) => {
            let action = match permission.level() {
                Level::View => KeyAction::Read,
                Level::Interact | Level::Edit => KeyAction::Write,
                Level::Manage | Level::Own => KeyAction::Manage,
                // Use can mean reading a native-session catalogue, allocating work, or a
                // bidirectional connection. Its actual endpoint must state its facets.
                Level::Use => return Err(unknown()),
                Level::Advanced => match permission {
                    Permission::DocCreate
                    | Permission::DocComment
                    | Permission::TopicStartThreads
                    | Permission::ProjectCreateTopics => KeyAction::Write,
                    Permission::ThreadExport | Permission::DocExport | Permission::ChatExport => {
                        KeyAction::Read
                    }
                    _ => return Err(unknown()),
                },
            };
            // A permission's allowed parent kinds are not necessarily its business family:
            // doc.edit on a Topic still edits a Document; chat.view on an Org reads a Chat.
            let resources = match permission.area() {
                // Folder operations declare their separate DocumentContainer family.
                // Ordinary document endpoints affect text, even when authorized at a node.
                Area::Doc => vec![KeyTargetKind::Object(ObjectKind::Document)],
                Area::Chat | Area::Directs => vec![KeyTargetKind::Identity(KeyIdentityKind::Chat)],
                _ => permission
                    .on()
                    .iter()
                    .copied()
                    .map(KeyTargetKind::Object)
                    .collect(),
            };
            (action, resources)
        }
    };
    Ok(OperationFacet {
        action,
        resources,
        target: TargetResolver::PermissionObject,
        usage: FacetUsage::Required,
    })
}

/// Describe one canonical route. Missing metadata never becomes a permissive default.
pub fn for_route(route: &Route) -> Result<Operation, CatalogueError> {
    let mut operation = Operation {
        operation_id: route.name.to_owned(),
        method: route.method.as_str().to_owned(),
        path: route.path.to_owned(),
        title: route.summary.trim().to_owned(),
        delegation: Delegation::Business,
        variants: Vec::new(),
        receipt: false,
    };
    let single = |facets: Vec<OperationFacet>| OperationVariant {
        legacy_tokens: route.access.tokens(),
        operation: route.name.to_owned(),
        name: String::new(),
        selector: String::new(),
        facets,
    };
    let mut declaration = route.operation;
    let mut legacy = None;
    loop {
        declaration = match declaration {
            Some(OperationSpec::Receipt(inner)) if !operation.receipt => {
                operation.receipt = true;
                Some(*inner)
            }
            Some(OperationSpec::LegacyTokens(selections, inner)) if legacy.is_none() => {
                legacy = Some(selections);
                Some(*inner)
            }
            Some(OperationSpec::Receipt(_) | OperationSpec::LegacyTokens(..)) => {
                return Err(invalid(route, "duplicate operation capability annotation"));
            }
            _ => break,
        };
    }
    // A sole empty wire reply is already a complete receipt. JSON/raw replies
    // still need an explicit, typed receipt producer before disclosure can narrow.
    let empty_reply =
        matches!(route.reply, crate::routes::Shape::None) && route.responses.is_empty();
    match declaration {
        Some(OperationSpec::LegacyTokens(..)) => {
            return Err(invalid(route, "unexpanded operation annotation"));
        }
        Some(OperationSpec::Receipt(_)) => {
            return Err(invalid(route, "receipt declarations cannot be nested"));
        }
        None => match route.access {
            Access::Permission(permission) => operation
                .variants
                .push(single(vec![permission_facet(route, permission)?])),
            _ => return Err(CatalogueError::MissingMetadata(route.name.to_owned())),
        },
        Some(OperationSpec::Single(facet)) => operation.variants.push(single(vec![facet.into()])),
        Some(OperationSpec::Returning(facet)) => operation.variants.push(single(vec![
            facet.into(),
            FacetSpec::response_content(facet.resources, facet.target).into(),
        ])),
        Some(OperationSpec::Facets(facets)) => operation
            .variants
            .push(single(facets.iter().copied().map(Into::into).collect())),
        Some(OperationSpec::Variants(variants)) => {
            for variant in variants {
                if variant.name.is_empty()
                    || !variant
                        .name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                {
                    return Err(invalid(
                        route,
                        "variant suffix must be nonempty letters, digits or underscores",
                    ));
                }
                if variant.selector.trim().is_empty() {
                    return Err(invalid(
                        route,
                        "variant needs its server-derived selection condition",
                    ));
                }
                operation.variants.push(OperationVariant {
                    legacy_tokens: route.access.tokens(),
                    operation: format!("{}.{}", route.name, variant.name),
                    name: variant.name.to_owned(),
                    selector: variant.selector.to_owned(),
                    facets: variant.facets.iter().copied().map(Into::into).collect(),
                });
            }
        }
        Some(OperationSpec::Public) => operation.delegation = Delegation::Public,
        Some(OperationSpec::Protocol) => operation.delegation = Delegation::AuthenticationProtocol,
        Some(OperationSpec::BrowserElevation) => {
            operation.delegation = Delegation::BrowserElevation
        }
    }
    if operation.delegation == Delegation::Business && operation.variants.is_empty() {
        return Err(invalid(route, "business operation has no variants"));
    }
    if operation.receipt && operation.delegation != Delegation::Business {
        return Err(invalid(
            route,
            "only business operations have mutation receipts",
        ));
    }
    operation.receipt |= operation.delegation == Delegation::Business && empty_reply;
    if let Some(selections) = legacy {
        if selections.iter().any(|id| {
            !operation
                .variants
                .iter()
                .any(|variant| variant.operation == *id)
        }) {
            return Err(invalid(
                route,
                "legacy compatibility names an unknown exact selection",
            ));
        }
        for variant in &mut operation.variants {
            variant.legacy_tokens = selections.contains(&variant.operation.as_str());
        }
    }
    let parameters = route.parameters();
    let mut selections = std::collections::HashSet::new();
    for variant in &operation.variants {
        if !selections.insert(&variant.operation) {
            return Err(invalid(route, "duplicate variant selection"));
        }
        if !variant
            .facets
            .iter()
            .any(|facet| facet.usage == FacetUsage::Required)
        {
            return Err(invalid(route, "variant has no required action"));
        }
        for facet in &variant.facets {
            if facet.resources.is_empty() {
                return Err(invalid(route, "facet has no business resource family"));
            }
            if facet.usage == FacetUsage::ResponseContent && facet.action != KeyAction::Read {
                return Err(invalid(route, "response content is a Read facet"));
            }
            match facet.target {
                TargetResolver::Path(parameter) | TargetResolver::TypedPath { parameter, .. }
                    if !parameters.contains(&parameter) =>
                {
                    return Err(invalid(
                        route,
                        format!("target parameter {parameter} is not declared"),
                    ));
                }
                TargetResolver::TypedPath {
                    kind: KeyTargetKind::Identity(kind),
                    ..
                } if !matches!(
                    kind,
                    KeyIdentityKind::Chat | KeyIdentityKind::Group | KeyIdentityKind::Account
                ) =>
                {
                    return Err(invalid(
                        route,
                        "typed path needs an existing shared identity reader; opaque credentials/jobs need their own producer",
                    ));
                }
                TargetResolver::Resolved(role) | TargetResolver::ResolvedCollection(role)
                    if role.is_empty()
                        || !role.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                        }) =>
                {
                    return Err(invalid(
                        route,
                        "resolved target role must be a stable snake_case identifier",
                    ));
                }
                TargetResolver::PermissionObject
                    if !matches!(route.access, Access::Permission(_)) =>
                {
                    return Err(invalid(
                        route,
                        "permission target has no fixed permission resolver",
                    ));
                }
                _ => {}
            }
        }
    }
    Ok(operation)
}

/// Exact historical compatibility for a permission-scoped or intersected legacy credential.
pub fn legacy_token_allowed(route: &Route, selection: &str) -> Result<bool, CatalogueError> {
    Ok(for_route(route)?
        .variants
        .iter()
        .any(|variant| variant.operation == selection && variant.legacy_tokens))
}

/// The complete HTTP operation catalogue, built only from the canonical route registry.
pub fn catalogue() -> Result<Vec<Operation>, CatalogueError> {
    let mut seen = std::collections::HashSet::new();
    crate::routes::all()
        .iter()
        .map(|route| {
            if !seen.insert(route.name) {
                return Err(invalid(route, "duplicate operation ID"));
            }
            for_route(route)
        })
        .collect()
}

/// Expand a reviewed preset into exact operation/action selections. The returned list is
/// what is saved, so later operations or variants cannot broaden an existing credential.
/// This does not inspect account authority or decide whether a request is allowed.
#[must_use]
pub fn expand_preset(
    operations: &[Operation],
    actions: &[KeyAction],
    resources: Option<&[KeyTargetKind]>,
) -> Vec<KeyOperation> {
    operations
        .iter()
        .filter(|operation| operation.delegation == Delegation::Business)
        .flat_map(|operation| &operation.variants)
        .filter_map(|variant| {
            let mut selected = Vec::new();
            for facet in &variant.facets {
                if actions.contains(&facet.action)
                    && resources.is_none_or(|resources| {
                        facet.resources.iter().any(|kind| resources.contains(kind))
                    })
                    && !selected.contains(&facet.action)
                {
                    selected.push(facet.action);
                }
            }
            (!selected.is_empty()).then(|| KeyOperation {
                operation: variant.operation.clone(),
                actions: selected,
            })
        })
        .collect()
}

/// Validate exact operation/action selections when issuing or changing a key. This
/// checks catalogue membership only; ordinary authority and attenuation remain in
/// their existing producers. Do not apply this to authentication of stored policies:
/// retired selections remain inert instead of invalidating an otherwise usable key.
pub fn validate_selections(
    scope: &KeyScope,
) -> Result<Vec<orch_types::fields::FieldError>, CatalogueError> {
    let operations = catalogue()?;
    let selections: std::collections::HashMap<_, _> = operations
        .iter()
        .filter(|operation| operation.delegation == Delegation::Business)
        .flat_map(|operation| &operation.variants)
        .map(|variant| (variant.operation.as_str(), &variant.facets))
        .collect();
    let layers = std::iter::once(("rules".to_owned(), scope.rules.as_slice())).chain(
        scope
            .constraints
            .iter()
            .enumerate()
            .map(|(index, rules)| (format!("constraints[{index}]"), rules.as_slice())),
    );
    let mut errors = Vec::new();
    for (prefix, rules) in layers {
        for (rule_index, rule) in rules.iter().enumerate() {
            for (selection_index, selection) in rule.allow.iter().enumerate() {
                let path = format!("{prefix}[{rule_index}].allow[{selection_index}]");
                let Some(facets) = selections.get(selection.operation.as_str()) else {
                    errors.push(orch_types::fields::FieldError { path: format!("{path}.operation"), message: "must name an exact selectable operation or variant in the current catalogue".into() });
                    continue;
                };
                for (action_index, action) in selection.actions.iter().enumerate() {
                    if !facets.iter().any(|facet| {
                        facet.action == *action
                            && rule.resources.as_ref().is_none_or(|kinds| {
                                kinds.iter().any(|kind| facet.resources.contains(kind))
                            })
                    }) {
                        errors.push(orch_types::fields::FieldError {
                            path: format!("{path}.actions[{action_index}]"),
                            message: "is not a declared action for this operation and selected resource families".into(),
                        });
                    }
                }
            }
        }
    }
    Ok(errors)
}

crate::endpoints! {
    /// The shared operation catalogue and its exact selectable action facets.
    OperationCatalogueGet: GET "/me/token-operations", Access::User, none, none, json Vec<Operation>, 200,
        operation OperationSpec::single(
            KeyAction::Read,
            &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
            TargetResolver::CurrentAccount,
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::Endpoint;

    #[test]
    fn issuance_rejects_unknown_names_ambiguous_parents_and_unsupported_actions_in_every_layer() {
        use orch_access_contract::key_scope::{KeyReach, KeyRule, KeyTarget};
        let rule = |operation: &str, action| KeyRule {
            target: KeyTarget {
                kind: KeyTargetKind::Identity(KeyIdentityKind::System),
                id: None,
                reach: KeyReach::SelfOnly,
            },
            resources: None,
            allow: vec![KeyOperation {
                operation: operation.into(),
                actions: vec![action],
            }],
        };
        let scope = KeyScope {
            rules: vec![
                rule("UnknownFutureRead", KeyAction::Read),
                rule("DocumentTreeOperate", KeyAction::Write),
                rule("MeAdminModeSet", KeyAction::Manage),
            ],
            constraints: vec![vec![rule("ThreadRawList", KeyAction::Read)]],
            ..KeyScope::default()
        };
        let errors = validate_selections(&scope).unwrap();
        assert_eq!(errors.len(), 4);
        assert_eq!(errors[3].path, "constraints[0][0].allow[0].actions[0]");
        let valid = KeyScope {
            rules: vec![
                rule("DocumentTreeOperate.copy", KeyAction::Write),
                rule("ChatMessageCreate", KeyAction::Read),
            ],
            ..KeyScope::default()
        };
        assert!(validate_selections(&valid).unwrap().is_empty());
    }

    #[test]
    fn provider_projection_sources_allow_read_without_inventing_group_management() {
        use orch_access_contract::key_scope::{KeyReach, KeyRule, KeyTarget};
        let group = KeyTargetKind::Identity(KeyIdentityKind::Group);
        let scope = |action| KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: group,
                    id: Some(orch_types::new_id()),
                    reach: KeyReach::SelfOnly,
                },
                resources: Some(vec![group]),
                allow: vec![KeyOperation {
                    operation: "ProviderUpdate".into(),
                    actions: vec![action],
                }],
            }],
            ..KeyScope::default()
        };
        assert!(
            validate_selections(&scope(KeyAction::Read))
                .unwrap()
                .is_empty()
        );
        let invalid = scope(KeyAction::Manage);
        assert_eq!(
            validate_selections(&invalid).unwrap()[0].path,
            "rules[0].allow[0].actions[0]"
        );
        let constrained = KeyScope {
            constraints: vec![invalid.rules],
            ..KeyScope::default()
        };
        assert_eq!(
            validate_selections(&constrained).unwrap()[0].path,
            "constraints[0][0].allow[0].actions[0]"
        );
    }

    #[test]
    fn every_canonical_route_has_valid_metadata_and_the_same_identity() {
        let routes = crate::routes::all();
        let operations = catalogue().expect("every canonical route must be classified");
        assert_eq!(operations.len(), routes.len());
        for (route, operation) in routes.iter().zip(&operations) {
            assert_eq!(operation.operation_id, route.name);
            assert_eq!(operation.method, route.method.as_str());
            assert_eq!(operation.path, route.path);
            assert_eq!(operation.title, route.summary.trim());
        }
    }

    #[test]
    fn unknown_metadata_and_permissions_fail_closed() {
        let mut route = OperationCatalogueGet::ROUTE;
        route.operation = None;
        for access in [
            Access::User,
            Access::Session,
            Access::Rule("new rule"),
            Access::Public,
            Access::Cookie,
        ] {
            route.access = access;
            assert!(matches!(
                for_route(&route),
                Err(CatalogueError::MissingMetadata(_))
            ));
        }
        route.access = Access::Permission("new.permission");
        assert!(matches!(
            for_route(&route),
            Err(CatalogueError::UnclassifiedPermission { .. })
        ));
    }

    #[test]
    fn actions_follow_business_meaning_not_transport() {
        let raw = for_route(&crate::threads::ThreadRawList::ROUTE).unwrap();
        assert_eq!(raw.variants[0].facets[0].action, KeyAction::Manage);
        let delete = for_route(&crate::documents::DocumentFileDelete::ROUTE).unwrap();
        assert_eq!(delete.variants[0].facets[0].action, KeyAction::Write);
        for route in [
            crate::resources::ProviderLoginStart::ROUTE,
            crate::resources::ProviderLoginInput::ROUTE,
            crate::resources::ProviderLoginGet::ROUTE,
            crate::resources::ProviderLoginCancel::ROUTE,
        ] {
            let operation = for_route(&route).unwrap();
            assert_eq!(operation.delegation, Delegation::Business);
            assert!(operation.variants.iter().all(|variant| {
                variant.facets.iter().any(|facet| {
                    facet.action == KeyAction::Manage && facet.usage == FacetUsage::Required
                })
            }));
        }
    }

    #[test]
    fn copy_declares_source_read_and_destination_write_under_one_exact_variant() {
        let operation = for_route(&crate::document_tree::DocumentTreeOperate::ROUTE).unwrap();
        let copy = operation
            .variants
            .iter()
            .find(|variant| variant.name == "copy")
            .unwrap();
        assert_eq!(copy.operation, "DocumentTreeOperate.copy");
        assert!(
            !operation
                .variants
                .iter()
                .any(|variant| variant.operation == operation.operation_id)
        );
        assert!(
            copy.facets
                .iter()
                .any(|facet| facet.action == KeyAction::Read
                    && facet.usage == FacetUsage::Required
                    && facet.target == TargetResolver::Resolved("sources"))
        );
        assert!(
            copy.facets
                .iter()
                .any(|facet| facet.action == KeyAction::Write
                    && facet.usage == FacetUsage::Required
                    && facet.target == TargetResolver::Resolved("destination"))
        );
    }

    #[test]
    fn mutation_readback_is_optional_and_belongs_to_the_same_selection() {
        let operation = for_route(&crate::chats::ChatMessageCreate::ROUTE).unwrap();
        let variant = &operation.variants[0];
        assert_eq!(variant.operation, "ChatMessageCreate");
        assert!(
            variant.facets.iter().any(
                |facet| facet.action == KeyAction::Write && facet.usage == FacetUsage::Required
            )
        );
        assert!(
            variant
                .facets
                .iter()
                .any(|facet| facet.action == KeyAction::Read
                    && facet.usage == FacetUsage::ResponseContent)
        );
        assert!(!variant.facets.iter().any(|facet| facet.action == KeyAction::Read
            && facet.usage == FacetUsage::Required));
    }

    #[test]
    fn malformed_targets_and_duplicate_variants_cannot_be_published() {
        const FACET: FacetSpec = FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
            TargetResolver::Path("missing"),
        );
        let mut route = OperationCatalogueGet::ROUTE;
        route.operation = Some(OperationSpec::Single(FACET));
        assert!(matches!(
            for_route(&route),
            Err(CatalogueError::Invalid { .. })
        ));
        const VARIANT: VariantSpec = VariantSpec {
            name: "same",
            selector: "same request",
            facets: &[FacetSpec::required(
                KeyAction::Read,
                &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
                TargetResolver::CurrentAccount,
            )],
        };
        route.operation = Some(OperationSpec::Variants(&[VARIANT, VARIANT]));
        assert!(matches!(
            for_route(&route),
            Err(CatalogueError::Invalid { .. })
        ));
    }

    #[test]
    fn browser_elevation_cannot_expand_into_a_key_preset() {
        let operation = for_route(&crate::auth::MeAdminModeSet::ROUTE).unwrap();
        assert_eq!(operation.delegation, Delegation::BrowserElevation);
        assert!(
            expand_preset(
                &[operation],
                &[KeyAction::Read, KeyAction::Write, KeyAction::Manage],
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn preset_expansion_saves_exact_current_selections_only() {
        let first = for_route(&OperationCatalogueGet::ROUTE).unwrap();
        let saved = expand_preset(std::slice::from_ref(&first), &[KeyAction::Read], None);
        let mut later = first;
        later.operation_id = "FutureOperation".to_owned();
        later.variants[0].operation = "FutureOperation".to_owned();
        let new = expand_preset(&[later], &[KeyAction::Read], None);
        assert_eq!(saved[0].operation, "OperationCatalogueGet");
        assert_eq!(new[0].operation, "FutureOperation");
        assert!(
            !saved
                .iter()
                .any(|selection| selection.operation == "FutureOperation")
        );
    }

    #[test]
    fn runtime_legacy_compatibility_is_per_variant_and_does_not_limit_explicit_presets() {
        const FACETS: &[FacetSpec] = &[FacetSpec::required(
            KeyAction::Read,
            &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
            TargetResolver::CurrentAccount,
        )];
        const BOTH: OperationSpec = OperationSpec::variants(&[
            VariantSpec {
                name: "old",
                selector: "old producer path",
                facets: FACETS,
            },
            VariantSpec {
                name: "new",
                selector: "new explicit path",
                facets: FACETS,
            },
        ]);
        let mut route = OperationCatalogueGet::ROUTE;
        route.operation = Some(OperationSpec::legacy_tokens(
            &["OperationCatalogueGet.old"],
            &BOTH,
        ));
        assert!(legacy_token_allowed(&route, "OperationCatalogueGet.old").unwrap());
        assert!(!legacy_token_allowed(&route, "OperationCatalogueGet.new").unwrap());
        assert!(!legacy_token_allowed(&route, "OperationCatalogueGet").unwrap());
        let operation = for_route(&route).unwrap();
        assert_eq!(
            expand_preset(&[operation], &[KeyAction::Read], None).len(),
            2
        );
    }

    #[test]
    fn legacy_annotations_cannot_name_unknown_or_future_selections() {
        const PLAIN: OperationSpec = OperationSpec::single(
            KeyAction::Read,
            &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
            TargetResolver::CurrentAccount,
        );
        let mut route = OperationCatalogueGet::ROUTE;
        route.operation = Some(OperationSpec::legacy_tokens(&["FutureOperation"], &PLAIN));
        assert!(for_route(&route).is_err());
        route.operation = Some(OperationSpec::legacy_tokens(&[], &PLAIN));
        assert!(!legacy_token_allowed(&route, "OperationCatalogueGet").unwrap());
    }
}
