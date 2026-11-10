//! Actual Snapshot custody facts shared by its stored mutations.

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::workspace_sources::Source;

use orch_db::Conn;

use orch_engine::archive::snapshot_authority::Source as SnapshotSource;
pub(super) async fn load(ctx: &Ctx, conn: &mut Conn) -> ApiResult<SnapshotSource> {
    SnapshotSource::load(conn, ctx.id("id")?)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub(super) async fn governor_facts(
    conn: &mut Conn,
    source: &SnapshotSource,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: source.governing_environment(),
        },
    )
    .await?;
    Ok(vec![native::Facts::one(
        Source::SnapshotGovernor,
        KeyTargetKind::Object(ObjectKind::Environment),
        target,
    )])
}

pub(super) async fn metadata_facts(
    conn: &mut Conn,
    source: &SnapshotSource,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut facts = governor_facts(conn, source).await?;
    let origin = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: source.row.environment_id,
        },
    )
    .await?;
    facts.push(native::Facts::one(
        Source::SnapshotOrigin,
        KeyTargetKind::Object(ObjectKind::Environment),
        origin,
    ));
    Ok(facts)
}
