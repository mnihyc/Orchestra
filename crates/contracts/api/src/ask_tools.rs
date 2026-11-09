//! Ask's tools derive their read catalogue from every variant's declared facets.

use crate::operations::{self, CatalogueError, Delegation, Operation, OperationVariant};
use crate::routes::{Route, Shape};
use orch_access_contract::key_scope::KeyAction;
use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Strict operation operands. Path, query and body keep their ordinary API schemas.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadParameters {
    #[serde(default)]
    pub path: BTreeMap<String, String>,
    #[serde(default)]
    pub query: BTreeMap<String, Json>,
    #[serde(default)]
    pub body: Option<Json>,
}

fn read_variant(variant: &OperationVariant) -> bool {
    !variant.facets.is_empty()
        && variant
            .facets
            .iter()
            .all(|facet| facet.action == KeyAction::Read)
}

fn read_operation(operation: &Operation) -> bool {
    operation.delegation == Delegation::Business
        && !operation.variants.is_empty()
        && operation.variants.iter().all(read_variant)
}

/// The unsuffixed endpoint remains available only when every variant reads.
pub fn readable(route: &Route) -> Result<bool, CatalogueError> {
    Ok(read_operation(&operations::for_route(route)?))
}

/// A mixed operation needs the exact name of an existing entirely Read variant.
pub fn allows_selection(route: &Route, selection: &str) -> Result<bool, CatalogueError> {
    let operation = operations::for_route(route)?;
    Ok(operation.delegation == Delegation::Business
        && ((selection == operation.operation_id && read_operation(&operation))
            || operation
                .variants
                .iter()
                .any(|variant| variant.operation == selection && read_variant(variant))))
}

/// Verify the native producer's actual variant at every current source checkpoint.
/// An operation name supplied to a tool never selects or overrides that producer.
pub fn matches_selection(
    route: &Route,
    requested: &str,
    selected: &str,
) -> Result<bool, CatalogueError> {
    let operation = operations::for_route(route)?;
    Ok(operation.delegation == Delegation::Business
        && operation
            .variants
            .iter()
            .any(|variant| variant.operation == selected && read_variant(variant))
        && (requested == selected
            || (requested == operation.operation_id && read_operation(&operation))))
}

/// General tools use the unchanged endpoint, or its sole eligible named variant.
/// Ambiguous selections require find_operation and the exact read tool instead.
pub fn default_selection(route: &Route) -> Result<Option<String>, CatalogueError> {
    let operation = operations::for_route(route)?;
    if read_operation(&operation) {
        return Ok(Some(operation.operation_id));
    }
    if operation.delegation != Delegation::Business {
        return Ok(None);
    }
    let mut allowed = operation.variants.into_iter().filter(read_variant);
    let first = allowed.next();
    Ok(match (first, allowed.next()) {
        (Some(variant), None) => Some(variant.operation),
        _ => None,
    })
}

/// The complete derived catalogue, without a manually maintained endpoint allowlist.
pub fn catalogue() -> Result<Vec<Operation>, CatalogueError> {
    crate::routes::all()
        .iter()
        .filter_map(|route| match operations::for_route(route) {
            Ok(mut operation) if operation.delegation == Delegation::Business => {
                operation.variants.retain(read_variant);
                (!operation.variants.is_empty()).then_some(Ok(operation))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

/// Publish ordinary request schemas, including definitions, to find_operation.
pub fn schemas(route: &Route) -> Json {
    let mut generator = schemars::SchemaGenerator::default();
    let mut shape = |value: Shape| match value {
        Shape::Json(write) => Json::from(write(&mut generator)),
        _ => Json::Null,
    };
    let query = shape(route.query);
    let body = shape(route.body);
    serde_json::json!({"path":route.parameters(),"query":query,"body":body,"$defs":generator.take_definitions(true)})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::{FacetSpec, OperationSpec, TargetResolver, VariantSpec};
    use crate::routes::Endpoint;
    use orch_access_contract::key_scope::KeyTargetKind;

    #[test]
    fn read_allowlist_is_derived_and_a_mutation_cannot_disguise_itself_as_get()
    -> Result<(), CatalogueError> {
        let routes = crate::routes::all();
        let allowed = catalogue()?;
        for operation in &allowed {
            let route = routes
                .iter()
                .find(|route| route.name == operation.operation_id)
                .ok_or_else(|| CatalogueError::MissingMetadata(operation.operation_id.clone()))?;
            for variant in &operation.variants {
                assert!(allows_selection(route, &variant.operation)?);
                assert!(matches_selection(
                    route,
                    &variant.operation,
                    &variant.operation
                )?);
                assert!(
                    variant
                        .facets
                        .iter()
                        .all(|facet| facet.action == KeyAction::Read)
                );
            }
        }
        const THREAD: &[KeyTargetKind] = &[KeyTargetKind::Object(
            orch_access_contract::permissions::ObjectKind::Thread,
        )];
        const MANAGE: &[FacetSpec] = &[FacetSpec::required(
            KeyAction::Manage,
            THREAD,
            TargetResolver::PermissionObject,
        )];
        let mut route = crate::nodes::NodeGet::ROUTE;
        route.operation = Some(OperationSpec::facets(MANAGE));
        assert!(!readable(&route)?);
        assert!(allowed.iter().any(|operation| operation.method == "POST"));
        Ok(())
    }

    #[test]
    fn named_read_variants_are_derived_without_admitting_their_manage_siblings()
    -> Result<(), CatalogueError> {
        for route in [
            crate::nodes::NodeGet::ROUTE,
            crate::threads::ThreadSyncGet::ROUTE,
            crate::threads::ThreadItemList::ROUTE,
            crate::documents::DocumentGet::ROUTE,
            crate::documents::DocumentVersionList::ROUTE,
        ] {
            let read = format!("{}.read", route.name);
            let inspect = format!("{}.inspect_deleted", route.name);
            // These genuine selections must remain entirely Read. Adding any
            // non-Read facet to one of them must fail this deciding assertion.
            assert!(allows_selection(&route, &read)?, "{read}");
            assert!(!readable(&route)?);
            assert!(!allows_selection(&route, route.name)?);
            assert!(!allows_selection(&route, &inspect)?);
            assert!(matches_selection(&route, &read, &read)?);
            assert!(!matches_selection(&route, &read, &inspect)?);
            assert!(!matches_selection(&route, &inspect, &read)?);
            assert_eq!(default_selection(&route)?, Some(read));
        }
        Ok(())
    }

    #[test]
    fn adding_a_non_read_facet_revokes_that_exact_named_selection() -> Result<(), CatalogueError> {
        const THREAD: &[KeyTargetKind] = &[KeyTargetKind::Object(
            orch_access_contract::permissions::ObjectKind::Thread,
        )];
        const CHANGED: &[FacetSpec] = &[
            FacetSpec::required(KeyAction::Read, THREAD, TargetResolver::PermissionObject),
            FacetSpec::required(KeyAction::Write, THREAD, TargetResolver::PermissionObject),
        ];
        let mut route = crate::threads::ThreadItemList::ROUTE;
        route.operation = Some(OperationSpec::variants(&[VariantSpec {
            name: "read",
            selector: "ordinary reader",
            facets: CHANGED,
        }]));
        assert!(!allows_selection(&route, "ThreadItemList.read")?);
        assert!(default_selection(&route)?.is_none());
        assert!(!matches_selection(
            &route,
            "ThreadItemList.read",
            "ThreadItemList.read"
        )?);
        Ok(())
    }

    #[test]
    fn operations_with_only_read_variants_keep_their_unsuffixed_tool_address()
    -> Result<(), CatalogueError> {
        const THREAD: &[KeyTargetKind] = &[KeyTargetKind::Object(
            orch_access_contract::permissions::ObjectKind::Thread,
        )];
        const READ: &[FacetSpec] = &[FacetSpec::required(
            KeyAction::Read,
            THREAD,
            TargetResolver::PermissionObject,
        )];
        let mut route = crate::threads::ThreadItemList::ROUTE;
        route.operation = Some(OperationSpec::variants(&[
            VariantSpec {
                name: "first",
                selector: "first reader",
                facets: READ,
            },
            VariantSpec {
                name: "second",
                selector: "second reader",
                facets: READ,
            },
        ]));
        assert!(readable(&route)?);
        assert!(allows_selection(&route, route.name)?);
        assert_eq!(default_selection(&route)?.as_deref(), Some(route.name));
        for name in ["ThreadItemList.first", "ThreadItemList.second"] {
            assert!(allows_selection(&route, name)?);
            assert!(matches_selection(&route, route.name, name)?);
        }
        assert!(!matches_selection(
            &route,
            "ThreadItemList.first",
            "ThreadItemList.second"
        )?);
        Ok(())
    }
}
