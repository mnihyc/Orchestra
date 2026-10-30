//! A batch of changes applied to a document, whole or not at all, and what a refused batch is answered with.
//!
//! The person's batch is applied by `orch-db` under the lock on the
//! document's row, by the rules of `orch-documents`; here it is read, sized,
//! and answered. A change written on an older state of a block is refused
//! with `conflict.stale_revision` and the blocks as they stand now, removed
//! ones with `removed: true`, so that the editor takes them as they are and
//! keeps what its person wrote beside them. Two people who change different
//! blocks never meet one. A batch sent twice, with the same `client_id` by
//! the same person, is applied once and answered as it was the first time.

use super::{build, limits};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::invalid;
use orch_api_contract::common::{ErrorCode, FieldFault, Problem};
use orch_api_contract::documents::{
    DocumentOpsCreate, DocumentOpsCreateBody, DocumentOpsCreateReply,
};
use orch_authz::{Policy, Principal};
use orch_db::DbError;
use orch_db::queries::documents::batch::{self, Outcome};
use orch_documents::operations::{self, Refusal};
use orch_documents::versions::Writer;
use orch_types::Id;

use limits::{DocSettings, Limit};

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(DocumentOpsCreate, create);
}

/// The answer to a batch that was not applied: what is wrong with it, which
/// limit it met and where that is set, or the blocks that were changed
/// meanwhile.
pub async fn refuse(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    settings: &DocSettings,
    outcome: Outcome,
    policy: Policy,
) -> ApiError {
    match outcome {
        Outcome::Gone => ApiError::not_found(),
        Outcome::Stale { current } => {
            metrics::counter!("orch_document_ops_total","result"=>"conflict").increment(1);
            match build::blocks_for(conn, principal, &current, policy).await {
                Ok(current) => ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "a block the batch builds on was changed or removed meanwhile",
                )
                .with("current", current),
                Err(error) => error,
            }
        }
        Outcome::Exists { id } => ApiError::new(
            ErrorCode::ConflictExists,
            "a block with that identifier exists already",
        )
        .with("id", id),
        Outcome::Invalid(faults) => Problem::invalid(
            faults
                .into_iter()
                .map(|fault| FieldFault {
                    path: fault.path,
                    message: fault.message,
                })
                .collect(),
        )
        .into(),
        Outcome::BlockTooLarge { .. } => {
            metrics::counter!("orch_document_ops_total","result"=>"too_large").increment(1);
            limits::too_large(conn, principal, settings, Limit::BlockBytes, policy).await
        }
        Outcome::TooDeep { .. } => {
            limits::too_large(conn, principal, settings, Limit::Depth, policy).await
        }
        Outcome::TooManyBlocks { .. } => {
            limits::too_large(conn, principal, settings, Limit::Blocks, policy).await
        }
        Outcome::TooLarge { .. } => {
            limits::too_large(conn, principal, settings, Limit::Bytes, policy).await
        }
        Outcome::Applied(_) | Outcome::Repeated { .. } => {
            ApiError::internal("a batch that was kept is not refused")
        }
    }
}

/// Applies a batch of operations to a document, whole or not at all.
async fn create(
    mut ctx: Ctx,
    (): (),
    body: DocumentOpsCreateBody,
) -> ApiResult<DocumentOpsCreateReply> {
    let id = ctx.id("id")?;
    let client: Id = body
        .client_id
        .parse()
        .map_err(|_| invalid("client_id", "must be a UUID"))?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let policy = Policy::default();
    let row = super::live(&mut tx, id).await?;
    let settings = DocSettings::at(&mut tx, row.node_id).await?;
    if let Err(Refusal::Invalid(_)) = operations::check_size(&body.ops) {
        metrics::counter!("orch_document_ops_total","result"=>"too_large").increment(1);
        return Err(
            limits::too_large(&mut tx, &principal, &settings, Limit::Operations, policy).await,
        );
    }
    let rules = settings.version_rules(ctx.app.config.documents.version_window_secs);
    let ops = build::label_written(&mut tx, &principal, body.ops).await?;
    let outcome = batch::apply_fenced(
        &mut tx,
        id,
        &Writer::User(principal.user()),
        Some(client),
        &ops,
        settings.batch_limits(),
        rules,
        ctx.now,
        body.expected_seq,
    )
    .await?;
    let fresh = matches!(&outcome, Outcome::Applied(_));
    let (seq, touched) = match outcome {
        Outcome::Applied(applied) => (applied.seq, applied.touched),
        Outcome::Repeated { seq } => (seq, batch::stamped_at(&mut tx, id, seq).await?),
        other => return Err(refuse(&mut tx, &principal, &settings, other, policy).await),
    };
    let row = super::live(&mut tx, id).await?;
    let listed = build::standing(&mut tx, &principal, id, policy).await?;
    let document = build::reference(&mut tx, &principal, &row, &listed, ctx.now, policy).await?;
    let blocks = build::blocks_for(&mut tx, &principal, &touched, policy).await?;
    tx.commit().await.map_err(DbError::from)?;
    if fresh {
        metrics::counter!("orch_document_ops_total","result"=>"applied").increment(1);
    }
    Ok(DocumentOpsCreateReply {
        document,
        blocks,
        seq,
    })
}
