//! The shared native source/destination/serialization prefix of a document move and its review.

use super::*;

use crate::key_operations::native;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::document_sources::MoveSource as Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;
const DOCUMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Document);

pub(super) struct Operands {
    pub source: DocumentRow,
    pub directory: document_tree_domain::Directory,
    pub principal: Principal,
    pub changes: bool,
    pub facts: Vec<native::Facts<Source>>,
}

pub(super) async fn facts(
    conn: &mut Conn,
    row: &DocumentRow,
    to: Id,
) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![
        native::Facts::one(
            Source::Document,
            DOCUMENT,
            read_operation::identity(conn, row.id).await?,
        ),
        native::Facts::one(
            Source::Destination,
            DOCUMENT,
            crate::key_operations::targets::node_identity(conn, to).await?,
        ),
    ])
}

pub(super) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    to: Id,
    declaration: &'static Declaration<Source>,
) -> ApiResult<Operands> {
    let initial = live(conn, ctx.id("id")?).await?;
    let known = native::bind_required(ctx, declaration, &facts(conn, &initial, to).await?)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    may_move(conn, &principal, Policy::default(), &initial, to).await?;
    rows::containers::lock_scopes(conn, &[initial.node_id, to]).await?;
    let row = live_locked(conn, initial.id).await?;
    if row.node_id != initial.node_id {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the source document moved; try again",
        ));
    }
    let mut nodes = vec![row.node_id, row.org_id, to];
    nodes.sort_unstable();
    nodes.dedup();
    let locked: Vec<Id> =
        sqlx::query_scalar("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR SHARE")
            .bind(&nodes)
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    if locked != nodes {
        return Err(ApiError::not_found());
    }
    let bound = facts(conn, &row, to).await?;
    let known = native::bind_required(ctx, declaration, &bound)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    let changes = may_move(conn, &principal, Policy::default(), &row, to)
        .await?
        .is_some();
    let directory = stored_directory(conn, &row).await?;
    Ok(Operands {
        source: row,
        directory,
        principal,
        changes,
        facts: bound,
    })
}
