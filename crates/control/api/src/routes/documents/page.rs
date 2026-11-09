//! The permission page of a document, read and changed through the requests that serve the page of anything: who is named in which row, what each row follows, and the one change that names owners and holders at a choice.
//!
//! A document's page is its own and stores almost nothing: its Owners row
//! and whatever somebody wrote on it. Every other row follows the row of the
//! same name on the page of the node it is beside. Making it private cuts the
//! rows loose; resetting it makes them follow again. The engine decides who
//! may read and who may change the page and what a change does; what a
//! document adds is its home: the policy of the node it is beside on naming
//! somebody from outside the root org, and the stamp a change leaves on the
//! document, so that whoever has it open is told its page changed.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::access::page_operations;
use crate::routes::nodes::policy_at;
use crate::sign_in::origin;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::access as api;
use orch_authz::pages::{self, ChangeError};
use orch_authz::usable::ResourcePolicy;
use orch_db::DbError;
use orch_db::queries::documents::{self as rows};
use orch_types::Id;

fn object_of(ctx: &Ctx, id: Id) -> ObjectRef {
    ObjectRef {
        kind: if ctx.param_if_any("object") == Some("document-containers") {
            ObjectKind::DocumentContainer
        } else {
            ObjectKind::Document
        },
        id,
    }
}

/// Reads the page of a document: whoever may read the document or manage its permission page.
pub async fn get(ctx: &Ctx) -> ApiResult<api::PermissionPage> {
    crate::routes::access::operations::read_page(ctx).await
}

/// Changes the page of a document, as a whole or not at all. Who may is the
/// engine's, and it was asked before this ran. A change that changed
/// something stamps the document.
pub async fn update(
    ctx: &Ctx,
    body: &api::PageChange,
) -> ApiResult<api::PermissionPageWriteResponse> {
    let id = ctx.id("id")?;
    let (change, people) = crate::routes::notifications::person::page_change(body)?;
    let (current, mut tx, guard) = page_operations::action(ctx).await?;
    let ctx = &current;
    let principal = guard.principal().clone();
    let (_, row, _) = orch_authz::documents::site(&mut tx, id)
        .await
        .map_err(ApiError::internal)?;
    if row.deleted || row.kind != object_of(ctx, id).kind {
        return Err(ApiError::not_found());
    }
    let policy = policy_at(&mut tx, row.node).await?;
    if crate::routes::notifications::person::replay_page(ctx, &mut tx, &principal, body).await? {
        crate::routes::access::subject::require_page(
            &mut tx,
            &principal,
            orch_authz::things::Thing::Other(object_of(ctx, id)),
            policy,
        )
        .await?;
        let view = pages::read(
            &mut tx,
            &principal,
            orch_authz::things::Thing::Other(object_of(ctx, id)).target(),
            policy,
        )
        .await
        .map_err(ApiError::internal)?;
        let receipt = page_operations::acknowledged(ctx, view.revision)?;
        tx.commit().await.map_err(DbError::from)?;
        return page_operations::reply(ctx, &principal, receipt).await;
    }
    let home = ResourcePolicy {
        owners_name_outsiders: policy.owners_name_outsiders,
        shared_subscriptions: None,
    };
    let accepted_revision;
    crate::routes::access::link_origin::check(&mut tx, &principal, body.link_origin).await?;
    match pages::change_resource_under_fenced(
        &mut tx,
        &principal,
        object_of(ctx, id),
        &change,
        policy,
        &home,
        &origin(ctx),
        || guard.check_deadline(ctx),
    )
    .await
    {
        Ok(outcome) => {
            accepted_revision = outcome.revision;
            if outcome.changed {
                if row.kind == ObjectKind::Document {
                    rows::touch(&mut tx, id).await?;
                    // The tree row of the node it is beside counts the documents that follow that node's
                    // rows: made private, or following again, the count changes and the row is sent anew.
                    orch_db::queries::hierarchy::stamp(&mut tx, row.node).await?;
                } else if row.kind == ObjectKind::DocumentContainer {
                    rows::containers::touch(&mut tx, id, ctx.now).await?;
                }
            }
        }
        Err(pages::ChangeFailure::Native(ChangeError::Stale { current })) => {
            let page = page_operations::current(ctx, &mut tx, &principal).await?;
            let mut error = refusals::change_failed(&mut tx, ChangeError::Stale { current }).await;
            if let Some(page) = page {
                error = error.with("current", page);
            }
            return Err(error);
        }
        Err(pages::ChangeFailure::Native(error)) => {
            return Err(refusals::change_failed(&mut tx, error).await);
        }
        Err(pages::ChangeFailure::BeforeEffect(error)) => return Err(error),
    }
    if !people.is_empty() {
        crate::routes::notifications::person::tell(
            ctx,
            &mut tx,
            &principal,
            "documents",
            id,
            &people,
            body.notify_anchor,
            None,
        )
        .await?;
    }
    let receipt = page_operations::acknowledged(ctx, accepted_revision)?;
    crate::routes::notifications::person::keep_page(ctx, &mut tx, &principal, body).await?;
    tx.commit().await.map_err(DbError::from)?;
    page_operations::reply(ctx, &principal, receipt).await
}
