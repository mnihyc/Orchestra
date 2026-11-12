//! What a document may hold where it stands, as the settings of its node resolve it, and the refusals that say which limit was met and where it is set.
//!
//! A limit is a setting of the node the document is beside, so that an org, a
//! project or a topic says how large a document, one file and all its files
//! may be. A refusal names the limit, its value and the node whose setting it
//! is, when it is one node's and not the system's or the shipped default,
//! which is what lets a person know whom to ask.

use crate::middleware::requests::ApiError;

use crate::routes::nodes::view;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::documents::DOCUMENT_BLOCK_MAX_BYTES;

use orch_api_contract::documents::DOCUMENT_NESTING_MAX;

use orch_api_contract::documents::DOCUMENT_OPS_MAX;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_types::Id;

pub use orch_api_base::routes::documents::limits::Setting;

pub use orch_api_base::routes::documents::limits::DocSettings;

pub use orch_api_base::routes::documents::limits::Limit;
/// The value of a limit that the contract fixes, and no setting changes.
#[must_use]
pub fn fixed(limit: Limit) -> Option<i64> {
    match limit {
        Limit::BlockBytes => Some(DOCUMENT_BLOCK_MAX_BYTES),
        Limit::Depth => Some(i64::from(DOCUMENT_NESTING_MAX)),
        Limit::Operations => Some(i64::from(DOCUMENT_OPS_MAX)),
        _ => None,
    }
}
orch_api_base::__api_bind_920f34e543!();

/// The node `id` as the caller may know it, for a refusal to name.
async fn node_named(
    conn: &mut Conn,
    principal: &Principal,
    id: Option<Id>,
    policy: Policy,
) -> Option<orch_api_contract::nodes::NodeRef> {
    let id = id?;
    view::refs(conn, principal, &[id], policy)
        .await
        .ok()
        .and_then(|found| found.get(&id).cloned())
}

/// `403 docs.disabled`, naming the node whose setting switched documents off.
pub async fn disabled(
    conn: &mut Conn,
    principal: &Principal,
    settings: &DocSettings,
    policy: Policy,
) -> ApiError {
    let scope = node_named(conn, principal, settings.enabled_scope, policy).await;
    ApiError::new(ErrorCode::DocsDisabled, "documents are switched off here").with("scope", scope)
}

/// `413 docs.too_large`: which limit, its value, and the node that sets it.
pub async fn too_large(
    conn: &mut Conn,
    principal: &Principal,
    settings: &DocSettings,
    limit: Limit,
    policy: Policy,
) -> ApiError {
    let scope = node_named(conn, principal, settings.scope_of(limit), policy).await;
    too_large_receipt(settings, limit).with("scope", scope)
}

/// The same operational limit without reading names of the place that set it.
pub fn too_large_receipt(settings: &DocSettings, limit: Limit) -> ApiError {
    let value = settings.value_of(limit);
    ApiError::new(
        ErrorCode::DocsTooLarge,
        format!("a document may hold at most {value} {}", limit.what()),
    )
    .with("limit", limit.word())
    .with("value", value)
}
