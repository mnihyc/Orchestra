//! Conversation scope applies to required sources before optional context labels.

use super::*;

use orch_access_contract::key_scope::KeyEmptyFacet;

use orch_access_contract::key_scope::KeyFacet;
pub(super) fn check(
    scope: &mut KeyScope,
    root: Id,
    conversation: Id,
    required: &[KeyFacet],
    response: &[KeyFacet],
    allow: &[KeyOperation],
    empty_required: Option<(&str, &[KeyEmptyFacet])>,
) -> ApiResult<()> {
    // Optional reference names never admit an otherwise unrelated required resource.
    let required_allowed = if required.is_empty() {
        empty_required
            .is_some_and(|(operation, required)| scope.allows_empty_operation(operation, required))
    } else {
        scope.allows(required)
    };
    if !required_allowed {
        return Err(denied(
            "This source belongs to another root organization. Ask in that organization.",
        ));
    }
    // Native optional references may accompany a required source in this root.
    // Contextual resource reads supply their separate native projection proof.
    let has_context = required.iter().any(|facet| {
        facet
            .target
            .node_path()
            .is_some_and(|path| path.contains(&root))
    });
    if has_context {
        for facet in response {
            if matches!(facet.target.object.kind, KeyTargetKind::Object(kind) if kind.resource().is_some())
            {
                scope.rules.push(KeyRule {
                    target: KeyTarget {
                        kind: facet.target.object.kind,
                        id: facet.target.object.id,
                        reach: KeyReach::SelfOnly,
                    },
                    resources: None,
                    allow: allow.to_vec(),
                });
            }
        }
    }
    // The conversation's real optional storage ancestor labels may accompany it.
    // They never admit another required read or an ancestor's subtree.
    for source in required.iter().filter(|facet| {
        facet.target.object.kind == KeyTargetKind::Object(ObjectKind::Thread)
            && facet.target.object.id == Some(conversation)
    }) {
        for ancestor in &source.target.node_ancestors {
            if ancestor.kind != KeyTargetKind::Object(ObjectKind::Thread) {
                scope.rules.push(KeyRule {
                    target: KeyTarget {
                        kind: ancestor.kind,
                        id: ancestor.id,
                        reach: KeyReach::SelfOnly,
                    },
                    resources: None,
                    allow: allow.to_vec(),
                });
            }
        }
    }
    if !response.is_empty() && !scope.allows(response) {
        return Err(denied(
            "This source's context is outside this conversation's root organization.",
        ));
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;

    use orch_access_contract::key_scope::KeyResolvedTarget;

    use orch_api_contract::routes::Endpoint;
    fn facet(kind: ObjectKind, id: Id, ancestors: Vec<KeyObjectRef>) -> KeyFacet {
        let kind = KeyTargetKind::Object(kind);
        KeyFacet {
            operation: "ReadContext.read".into(),
            action: KeyAction::Read,
            resource_kind: kind,
            target: KeyResolvedTarget {
                object: KeyObjectRef { kind, id: Some(id) },
                node_ancestors: ancestors,
                document_ancestors: vec![],
                credential_account: None,
            },
        }
    }

    fn root_scope(root: Id, operation: &str) -> (KeyScope, Vec<KeyOperation>) {
        let allow = vec![KeyOperation {
            operation: operation.into(),
            actions: vec![KeyAction::Read],
        }];
        let scope = KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Object(ObjectKind::Org),
                    id: Some(root),
                    reach: KeyReach::Subtree,
                },
                resources: None,
                allow: allow.clone(),
            }],
            ..Default::default()
        };
        (scope, allow)
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_complete_named_read_does_not_require_optional_metadata() -> ApiResult<()> {
        // EN: Ask > read this topic > open its source; no optional metadata is required.
        // 中文：问一问 > 读取本话题 > 打开来源；不要求存在可选元数据。
        let root = new_id();
        let conversation = new_id();
        let topic = facet(
            ObjectKind::Topic,
            new_id(),
            vec![KeyObjectRef {
                kind: KeyTargetKind::Object(ObjectKind::Org),
                id: Some(root),
            }],
        );
        let mut plan = orch_engine::operation_scope::Prepared::new(
            orch_api_contract::nodes::NodeGet::ROUTE,
            "NodeGet.read",
        )
        .map_err(ApiError::internal)?;
        plan.bind(0, topic.resource_kind, &[topic.target])
            .map_err(ApiError::internal)?;
        let required = plan.required_facets().map_err(ApiError::internal)?;
        let response = plan.response_facets().map_err(ApiError::internal)?;
        assert!(response.is_empty());
        assert!(plan.empty_requirements().is_none());
        let (scope, allow) = root_scope(root, "NodeGet.read");
        check(
            &mut scope.clone(),
            root,
            conversation,
            &required,
            &response,
            &allow,
            None,
        )?;
        let mut foreign = required;
        for facet in &mut foreign {
            facet.target.node_ancestors.clear();
        }
        assert!(
            check(
                &mut scope.clone(),
                root,
                conversation,
                &foreign,
                &[],
                &allow,
                None
            )
            .is_err()
        );
        assert!(
            check(
                &mut scope.clone(),
                root,
                conversation,
                &[],
                &[],
                &allow,
                None
            )
            .is_err()
        );
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_complete_named_read_does_not_require_optional_metadata()
    -> crate::middleware::requests::ApiResult<()> {
        self::a_complete_named_read_does_not_require_optional_metadata()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn an_empty_native_collection_keeps_its_real_context_and_declared_proof() -> ApiResult<()> {
        // EN: Ask > list providers shared here > revoke their shares > list again;
        // the real place remains checked and the native page is empty.
        // 中文：问一问 > 列出共享给此处的提供方 > 撤销共享 > 再次列出；
        // 仍检查真实位置，原生列表为空。
        use orch_api_contract::operations::TargetResolver;
        use orch_api_contract::resources::{ProviderList, RESOURCE_READ_CONTEXT};
        let root = new_id();
        let conversation = new_id();
        let context = facet(
            ObjectKind::Topic,
            new_id(),
            vec![KeyObjectRef {
                kind: KeyTargetKind::Object(ObjectKind::Org),
                id: Some(root),
            }],
        );
        let mut plan =
            orch_engine::operation_scope::Prepared::new(ProviderList::ROUTE, "ProviderList")
                .map_err(ApiError::internal)?;
        assert!(
            plan.empty_requirements().is_none(),
            "unresolved is not empty"
        );
        for index in 0..plan.variant().facets.len() {
            if plan.variant().facets[index].target
                == TargetResolver::ResolvedCollection(RESOURCE_READ_CONTEXT)
            {
                plan.bind(
                    index,
                    context.resource_kind,
                    std::slice::from_ref(&context.target),
                )
                .map_err(ApiError::internal)?;
            } else {
                plan.bind_empty(index).map_err(ApiError::internal)?;
            }
        }
        let required = plan.required_facets().map_err(ApiError::internal)?;
        let response = plan.response_facets().map_err(ApiError::internal)?;
        assert!(required.is_empty());
        assert_eq!(response.len(), 1);
        let empty = plan
            .empty_requirements()
            .ok_or_else(|| ApiError::internal("the native list has no empty proof"))?;
        let (scope, allow) = root_scope(root, "ProviderList");
        let proof = Some(("ProviderList", empty.as_slice()));
        check(
            &mut scope.clone(),
            root,
            conversation,
            &required,
            &response,
            &allow,
            proof,
        )?;
        assert!(
            check(
                &mut scope.clone(),
                root,
                conversation,
                &required,
                &response,
                &allow,
                None
            )
            .is_err()
        );
        let mut foreign = response;
        for facet in &mut foreign {
            facet.target.node_ancestors.clear();
        }
        assert!(
            check(
                &mut scope.clone(),
                root,
                conversation,
                &required,
                &foreign,
                &allow,
                proof
            )
            .is_err()
        );
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_an_empty_native_collection_keeps_its_real_context_and_declared_proof()
    -> crate::middleware::requests::ApiResult<()> {
        self::an_empty_native_collection_keeps_its_real_context_and_declared_proof()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn optional_context_cannot_promote_an_unrelated_required_resource() -> ApiResult<()> {
        // EN: Ask or a private thread > read this topic and its resource names;
        // a context label cannot authorize reading a foreign provider's contents.
        // 中文：问一问或私有会话 > 读取本话题及其资源名称；上下文标签不能授权
        // 读取其他范围中提供方的内容。
        let root = new_id();
        let conversation = new_id();
        let root_ref = KeyObjectRef {
            kind: KeyTargetKind::Object(ObjectKind::Org),
            id: Some(root),
        };
        let topic = facet(ObjectKind::Topic, new_id(), vec![root_ref]);
        let provider = facet(ObjectKind::Provider, new_id(), vec![]);
        let allow = vec![KeyOperation {
            operation: "ReadContext.read".into(),
            actions: vec![KeyAction::Read],
        }];
        let base = KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: root_ref.kind,
                    id: root_ref.id,
                    reach: KeyReach::Subtree,
                },
                resources: None,
                allow: allow.clone(),
            }],
            ..Default::default()
        };
        // A response's topic label is not a required in-root source.
        assert!(
            check(
                &mut base.clone(),
                root,
                conversation,
                std::slice::from_ref(&provider),
                &[topic.clone(), provider.clone()],
                &allow,
                None
            )
            .is_err()
        );
        // Even a real required topic cannot promote another required rich resource.
        assert!(
            check(
                &mut base.clone(),
                root,
                conversation,
                &[topic.clone(), provider.clone()],
                std::slice::from_ref(&provider),
                &allow,
                None
            )
            .is_err()
        );
        // The original capability to return native optional resource references remains.
        check(
            &mut base.clone(),
            root,
            conversation,
            &[topic],
            &[provider],
            &allow,
            None,
        )?;
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_optional_context_cannot_promote_an_unrelated_required_resource()
    -> crate::middleware::requests::ApiResult<()> {
        self::optional_context_cannot_promote_an_unrelated_required_resource()
    }
}
