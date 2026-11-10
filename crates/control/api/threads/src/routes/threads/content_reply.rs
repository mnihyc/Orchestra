//! The native thread header's actual protected sources, shared by completed controls.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::routes::nodes::content_sources;

use crate::routes::nodes::read_sources;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::nodes::Node;

use orch_api_contract::threads::Thread;

use orch_authz::Principal;

use orch_db::Conn;

use orch_types::Id;

use std::collections::BTreeMap;
pub(crate) type Groups = Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)>;

pub(crate) async fn prepare(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
) -> ApiResult<(content_sources::Prepared, Groups)> {
    let value = content_sources::prepare(conn, principal, thread).await?;
    let identities = read_sources::Sources::from_objects(
        value
            .objects
            .iter()
            .map(|&(kind, id)| ObjectRef { kind, id }),
    )
    .identity_targets(conn)
    .await?;
    let mut groups = BTreeMap::<KeyTargetKind, Vec<_>>::new();
    for &(kind, id) in &value.objects {
        groups.entry(KeyTargetKind::Object(kind)).or_default().push(
            identities
                .get(&ObjectRef { kind, id })
                .cloned()
                .ok_or_else(ApiError::not_found)?,
        );
    }
    Ok((value, groups.into_iter().collect()))
}

pub(crate) async fn render(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    value: &content_sources::Prepared,
) -> ApiResult<Thread> {
    match content_sources::render(ctx, conn, principal, thread, value).await? {
        Node::Thread(thread) => Ok(thread),
        _ => Err(ApiError::not_found()),
    }
}
