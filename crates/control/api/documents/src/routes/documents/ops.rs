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

use super::build;

use super::limits;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::nodes::invalid;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::FieldFault;

use orch_api_contract::common::Problem;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::documents::DocumentOpsCreate;

use orch_api_contract::documents::DocumentOpsCreateBody;

use orch_api_contract::documents::DocumentOpsCreateReply;

use orch_api_contract::documents::DocumentOpsCreateResponse;

use orch_api_contract::documents::DocumentOpsReceipt;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::DbError;

use orch_db::queries::documents::batch;

use orch_db::queries::documents::batch::Outcome;

use orch_documents::operations;

use orch_documents::operations::Refusal;

use orch_documents::versions::Writer;

use orch_types::Id;

use limits::DocSettings;

use limits::Limit;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_with_operation(DocumentOpsCreate, super::operation::resolve, create);
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
    refuse_projected(conn, principal, settings, outcome, policy, false, None).await
}

/// Refuses without constructing existing content when the admitted operation permits only a receipt.
async fn refuse_projected(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    settings: &DocSettings,
    outcome: Outcome,
    policy: Policy,
    receipt_only: bool,
    ctx: Option<&Ctx>,
) -> ApiError {
    match outcome {
        Outcome::Gone => ApiError::not_found(),
        Outcome::Stale { current } => {
            metrics::counter!("orch_document_ops_total","result"=>"conflict").increment(1);
            if receipt_only {
                return ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "a block the batch builds on was changed or removed meanwhile",
                );
            }
            let projected = match ctx {
                Some(ctx) => {
                    super::operation::read_back(ctx, conn, principal, &current, policy, false)
                        .await
                        .map(|value| value.map(|read| read.blocks))
                }
                None => build::blocks_for(conn, principal, &current, policy)
                    .await
                    .map(Some),
            };
            let refusal = ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "a block the batch builds on was changed or removed meanwhile",
            );
            match projected {
                Ok(Some(current)) => refusal.with("current", current),
                Ok(None) => refusal,
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
            too_large(
                conn,
                principal,
                settings,
                Limit::BlockBytes,
                policy,
                receipt_only,
            )
            .await
        }
        Outcome::TooDeep { .. } => {
            too_large(
                conn,
                principal,
                settings,
                Limit::Depth,
                policy,
                receipt_only,
            )
            .await
        }
        Outcome::TooManyBlocks { .. } => {
            too_large(
                conn,
                principal,
                settings,
                Limit::Blocks,
                policy,
                receipt_only,
            )
            .await
        }
        Outcome::TooLarge { .. } => {
            too_large(
                conn,
                principal,
                settings,
                Limit::Bytes,
                policy,
                receipt_only,
            )
            .await
        }
        Outcome::Applied(_) | Outcome::Repeated { .. } => {
            ApiError::internal("a batch that was kept is not refused")
        }
    }
}

async fn too_large(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    settings: &DocSettings,
    limit: Limit,
    policy: Policy,
    receipt_only: bool,
) -> ApiError {
    if receipt_only {
        limits::too_large_receipt(settings, limit)
    } else {
        limits::too_large(conn, principal, settings, limit, policy).await
    }
}

/// Applies a batch of operations to a document, whole or not at all.
async fn create(
    mut ctx: Ctx,
    (): (),
    body: DocumentOpsCreateBody,
) -> ApiResult<DocumentOpsCreateResponse> {
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
        return Err(too_large(
            &mut tx,
            &principal,
            &settings,
            Limit::Operations,
            policy,
            ctx.key_receipt_required(),
        )
        .await);
    }
    let rules = settings.version_rules(ctx.app.config.documents.version_window_secs);
    // The guarded resolver captured exactly the source names admitted at this checkpoint.
    // Applying that value cannot discover an unbound source after a concurrent content rename.
    let ops = guard
        .operation_data::<build::WrittenLabels>()?
        .apply(body.ops)?;
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
        Outcome::Repeated { seq } => {
            let touched = if ctx.key_receipt_required() {
                Vec::new()
            } else {
                batch::stamped_at(&mut tx, id, seq).await?
            };
            (seq, touched)
        }
        other => {
            return Err(refuse_projected(
                &mut tx,
                &principal,
                &settings,
                other,
                policy,
                ctx.key_receipt_required(),
                Some(&ctx),
            )
            .await);
        }
    };
    let full = match super::operation::read_back(&ctx, &mut tx, &principal, &touched, policy, true)
        .await?
    {
        None => None,
        Some(read) => Some(DocumentOpsCreateReply {
            document: read
                .document
                .ok_or_else(|| ApiError::internal("the full document readback has no reference"))?,
            blocks: read.blocks,
            seq,
        }),
    };
    tx.commit().await.map_err(DbError::from)?;
    if fresh {
        metrics::counter!("orch_document_ops_total","result"=>"applied").increment(1);
    }
    // A guarded commit may have narrowed the shared request/reply disclosure decision.
    Ok(match (full, ctx.key_receipt_required()) {
        (Some(full), false) => DocumentOpsCreateResponse::full(full),
        _ => DocumentOpsCreateResponse::Receipt(DocumentOpsReceipt {
            projection: ReceiptProjection::Receipt,
            document_id: id,
            seq,
        }),
    })
}
