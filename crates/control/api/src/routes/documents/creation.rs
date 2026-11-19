//! Copies and imports use the ordinary guarded creation producer, with current source rights and truthful entry receipts.

use super::*;
use orch_api_contract::documents::{NodeDocumentCopy, NodeDocumentCopyBody};

// The ordinary client identity bound used by chat and document operation producers.
pub(super) const CLIENT_ID_MAX_CHARS: usize = 200;

pub(super) fn mount(registry: &mut Registry) {
    registry.on(NodeDocumentCopy, copy);
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
    tx: &mut orch_db::Tx,
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

async fn copy(mut ctx: Ctx, (): (), body: NodeDocumentCopyBody) -> ApiResult<Document> {
    client_id(&body.client_id)?;
    let destination = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    authorization::require(
        &mut tx,
        &principal,
        destination,
        exact(Permission::DocCreate),
        Act::Use,
        Policy::default(),
    )
    .await?;
    let id = rows::copy::identity(principal.user(), destination, &body.client_id, "copy");
    if rows::containers::operation(&mut tx, "person", principal.user(), &id.to_string())
        .await?
        .is_none()
    {
        let source = live(&mut tx, body.source_document_id).await?;
        let standing = build::standing(&mut tx, &principal, source.id, Policy::default()).await?;
        if !standing.holds(Permission::DocView) {
            return Err(ApiError::not_found());
        }
        let settings = DocSettings::at(&mut tx, destination).await?;
        if !settings.enabled {
            return Err(limits::disabled(&mut tx, &principal, &settings, Policy::default()).await);
        }
    }
    let title = body
        .title
        .as_deref()
        .map(|title| clean_title("title", title))
        .transpose()?;
    let request = document_tree_domain::Request {
        command: document_tree_domain::Command::Copy,
        items: vec![document_tree_domain::Item::Document(
            body.source_document_id,
        )],
        to: Some(document_tree_domain::Directory::NodeDocs(destination)),
        name: None,
        request_ref: id.to_string(),
        version: None,
    };
    let copy = document_tree_domain::TextCopy {
        id,
        title,
        private: body.access == Some(NewDocumentAccess::Private),
    };
    let made = document_tree_domain::copy_text_in(
        &mut tx,
        document_tree_domain::Actor::Person(&principal),
        &request,
        &copy,
        Time::new(ctx.now),
        crate::routes::document_tree::options(&ctx),
    )
    .await;
    crate::routes::document_tree::result(&mut tx, made).await?;
    let made = existing(&ctx, &mut tx, &principal, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(made)
}
