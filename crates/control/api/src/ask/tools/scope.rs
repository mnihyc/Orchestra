//! Conversation scope applies to required sources before optional context labels.

use super::*;
use orch_access_contract::key_scope::KeyFacet;

pub(super) fn check(
    scope: &mut KeyScope,
    root: Id,
    conversation: Id,
    required: &[KeyFacet],
    response: &[KeyFacet],
    allow: &[KeyOperation],
) -> ApiResult<()> {
    // Optional reference names never admit an otherwise unrelated required resource.
    if !scope.allows(required) {
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
    if !scope.allows(response) {
        return Err(denied(
            "This source's context is outside this conversation's root organization.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_access_contract::key_scope::KeyResolvedTarget;

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

    #[test]
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
                &allow
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
                &allow
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
        )?;
        Ok(())
    }
}
