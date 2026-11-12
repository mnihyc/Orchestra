//! Only credential construction shared by the HTTP and real-connector Snapshot controls.
#![allow(
    dead_code,
    reason = "the independent HTTP and native test targets use different helper subsets"
)]

use crate::workspace::Site;
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::collections::BTreeSet;

pub(crate) struct Key {
    pub(crate) id: Id,
    pub(crate) secret: String,
    pub(crate) user: Id,
}

pub(crate) fn rule(operation: &str, kind: &str, id: Id, reach: &str, actions: &[&str]) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":reach},
        "allow":[{"operation":operation,"actions":actions}]})
}

pub(crate) fn read(operation: &str, environment: Id) -> Json {
    rule(operation, "environment", environment, "self", &["read"])
}

pub(crate) async fn scoped(site: &Site, user: Id, rules: Vec<Json>) -> Key {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty(), "valid typed fixture scope");
    let known = orch_api_contract::operations::catalogue()
        .unwrap()
        .into_iter()
        .flat_map(|operation| {
            operation
                .variants
                .into_iter()
                .map(|variant| variant.operation)
        })
        .collect::<BTreeSet<_>>();
    for rule in &scope.rules {
        for selection in &rule.allow {
            assert!(selection.operation == "*" || known.contains(&selection.operation));
        }
    }
    insert(site, user, &[], None, Some(json!(scope))).await
}

pub(crate) async fn legacy(site: &Site, user: Id, node: Option<Id>) -> Key {
    let permissions = vec!["files.view".to_owned()];
    orch_authz::TokenGrant::from_stored(&permissions, node)
        .expect("actual Permission words, rather than shorthand guard names");
    insert(site, user, &permissions, node, None).await
}

pub(crate) async fn legacy_writer(site: &Site, user: Id, node: Option<Id>) -> Key {
    let permissions = vec!["files.manage".to_owned()];
    orch_authz::TokenGrant::from_stored(&permissions, node)
        .expect("the existing Files management permission");
    insert(site, user, &permissions, node, None).await
}

async fn insert(
    site: &Site,
    user: Id,
    permissions: &[String],
    node: Option<Id>,
    scope: Option<Json>,
) -> Key {
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *site.world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Snapshot source boundary control",
            token_hash: &token_hash(&secret),
            scopes: permissions,
            node_id: node,
            at: site.world.now(),
            expires_at: Some(site.world.now() + chrono::Duration::hours(1)),
        },
        scope.as_ref(),
    )
    .await
    .unwrap();
    Key { id, secret, user }
}

pub(crate) async fn revoke(site: &Site, key: &Key) {
    assert!(
        identity::revoke_api_token(
            &mut *site.world.conn().await,
            key.id,
            key.user,
            site.world.now()
        )
        .await
        .unwrap(),
        "the ordinary key writer also publishes its identity change"
    );
}
