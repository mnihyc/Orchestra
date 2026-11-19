//! Copies and imports use the ordinary guarded creation producer, with current source rights and truthful entry receipts.

use super::*;
use orch_api_contract::documents::NodeDocumentCopy;

// The ordinary client identity bound used by chat and document operation producers.
pub(super) const CLIENT_ID_MAX_CHARS: usize = 200;

pub(super) fn mount(registry: &mut Registry) {
    registry.on_with_operation(
        NodeDocumentCopy,
        crate::key_operations::native::effects::prepare::<super::copying::Copies>,
        super::copying::copy,
    );
}

pub(super) fn client_id(value: &str) -> ApiResult<()> {
    if value.trim().is_empty() || value.chars().count() > CLIENT_ID_MAX_CHARS {
        return Err(invalid(
            "client_id",
            "must be a nonempty identity of at most two hundred characters",
        ));
    }
    Ok(())
}

/// Pins the exact directory before its anchor's settings or ordinary creation material are read.
pub(super) async fn parent(
    ctx: &Ctx,
    tx: &mut Conn,
    principal: &Principal,
) -> ApiResult<document_tree_domain::Directory> {
    let id = ctx.id("id")?;
    if ctx.route.name == "DocumentContainerDocumentCreate" {
        let initial = rows::containers::get(tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        rows::containers::lock_scopes(tx, &[initial.node_id]).await?;
        authorization::require_document(
            tx,
            principal,
            id,
            Permission::DocCreate,
            Policy::default(),
        )
        .await?;
        let current = rows::containers::get(tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if current.node_id != initial.node_id {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the document directory moved; try again",
            ));
        }
        Ok(document_tree_domain::Directory::Container(id))
    } else {
        rows::containers::lock_scopes(tx, &[id]).await?;
        authorization::require(
            tx,
            principal,
            id,
            exact(Permission::DocCreate),
            Act::Use,
            Policy::default(),
        )
        .await?;
        Ok(document_tree_domain::Directory::NodeDocs(id))
    }
}

pub(super) async fn existing(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Option<Document>> {
    let Some(row) = rows::get(conn, id).await? else {
        return Ok(None);
    };
    if row.deleted_at.is_some() {
        return Err(ApiError::not_found());
    }
    let standing = build::standing(conn, principal, id, Policy::default()).await?;
    if !standing.holds(Permission::DocView) {
        return Err(ApiError::not_found());
    }
    Ok(Some(
        build::document(conn, principal, &row, &standing, ctx.now, Policy::default()).await?,
    ))
}
