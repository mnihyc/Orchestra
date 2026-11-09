//! Handing a resource or a document over to another person, who is its first owner from then on.
//!
//! One of the owners or an admin of the place the thing is in names the person in the Owners row and
//! says that they are first; the person who was first stays an owner unless the same request takes
//! them out. It is the engine's change of the Owners row, in one transaction, audited as the passing
//! on of the kind of thing it is, and the answer is the thing as its own request reads it. A node is
//! handed over by its own request; a kind that has no page here is answered plainly as not there.

mod grouping;
pub(crate) mod operation;

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::access::subject::{self, Named};
use crate::routes::nodes::invalid;
use crate::sign_in::origin;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::ownership::{ObjectTransfer, TransferBody};
use orch_authz::pages::ChangeError;
use orch_authz::things::Thing;
use orch_authz::transfer::{TransferError, hand_over_prepared_fenced, prepare_hand_over};
use orch_db::DbError;
use orch_db::queries::documents as documents_rows;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_with_operation(ObjectTransfer, operation::object, transfer);
    registry
        .on_with_operation(
            orch_api_contract::resource_groups::ObjectHomeSet,
            crate::key_operations::native::effects::prepare::<grouping::HomeMove>,
            grouping::home,
        )
        .on_with_operation(
            orch_api_contract::resource_groups::ObjectTagsSet,
            crate::key_operations::native::effects::prepare::<grouping::Tags>,
            grouping::tags,
        );
}

/// The kinds a transfer answers for: those whose own request reads the thing back.
fn require_transfer_kind(kind: ObjectKind) -> ApiResult<()> {
    match kind {
        ObjectKind::Provider
        | ObjectKind::Agent
        | ObjectKind::Environment
        | ObjectKind::Host
        | ObjectKind::Machine
        | ObjectKind::Template
        | ObjectKind::Mount
        | ObjectKind::McpServer
        | ObjectKind::Skill
        | ObjectKind::Document
        | ObjectKind::DocumentContainer => Ok(()),
        _ => Err(ApiError::new(
            ErrorCode::NotFound,
            "Ownership can be transferred here for providers, agent profiles, environments, hosts, machines, templates, mounts, tool servers, skills, and documents.",
        )),
    }
}

/// Makes another person the first owner of a resource or a document.
async fn transfer(
    ctx: Ctx,
    (): (),
    body: TransferBody,
) -> ApiResult<orch_api_contract::ownership::ObjectTransferResponse> {
    crate::middleware::authorization::projected_result(
        ctx.clone(),
        transfer_action(ctx, body),
        false,
    )
    .await
}
async fn transfer_action(
    ctx: Ctx,
    body: TransferBody,
) -> ApiResult<orch_api_contract::ownership::ObjectTransferResponse> {
    let Named { thing } = subject::named(&ctx)?;
    let Thing::Other(object) = thing else {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "a node is handed over by its own request",
        ));
    };
    require_transfer_kind(object.kind)?;
    let (mut tx, initial) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let early = ctx
        .for_operation_checkpoint(initial.user().clone(), initial.now().get())
        .with_admitted_caller(initial.principal().caller().clone())?;
    let (_, site) =
        orch_authz::things::site(&mut tx, thing)
            .await
            .map_err(|error| match error {
                orch_authz::Error::NotFound(_) => ApiError::not_found(),
                other => ApiError::internal(other),
            })?;
    let policies = subject::resource_policy(&mut tx, thing, &site).await?;
    let prepared = prepare_hand_over(
        &mut tx,
        initial.principal(),
        thing,
        body.user_id,
        body.keep_previous.unwrap_or(true),
        &policies,
    )
    .await;
    initial.check_deadline(&early)?;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return Err(transfer_failure(&mut tx, error).await),
    };
    if let Some(target) = prepared.mutation_target() {
        orch_authz::pages::lock_change_target(&mut tx, target)
            .await
            .map_err(crate::middleware::authorization::restoration_error)?;
    }
    // A no-op keeps its original lock-free prefix. The native factual comparison
    // below refuses a changed owner/no-op association before any new mutation.
    let (ctx, guard) = crate::middleware::transaction_guard::after_wait(&early, &mut tx).await?;
    let principal = guard.principal().clone();
    let (_, site) =
        orch_authz::things::site(&mut tx, thing)
            .await
            .map_err(|error| match error {
                orch_authz::Error::NotFound(_) => ApiError::not_found(),
                other => ApiError::internal(other),
            })?;
    let policies = subject::resource_policy(&mut tx, thing, &site).await?;
    let handed = match hand_over_prepared_fenced(
        &mut tx,
        &principal,
        prepared,
        &policies,
        &origin(&ctx),
        || guard.check_deadline(&ctx),
    )
    .await
    {
        Ok(handed) => handed,
        Err(orch_authz::pages::EffectFailure::Native(error)) => {
            guard.check_deadline(&ctx)?;
            return Err(transfer_failure(&mut tx, error).await);
        }
        Err(orch_authz::pages::EffectFailure::BeforeEffect(error)) => return Err(error),
    };
    if handed.changed && object.kind == ObjectKind::Document {
        documents_rows::touch(&mut tx, object.id).await?;
    }
    let receipt = operation::receipt(&ctx, handed.to, handed.changed)?;
    tx.commit().await.map_err(DbError::from)?;
    operation::object_reply(&ctx, &principal, object, receipt).await
}

/// Existing native handover failures retain the same code and factual transport details.
async fn transfer_failure(conn: &mut orch_db::Conn, error: TransferError) -> ApiError {
    match error {
        TransferError::Engine(error) => refusals::problem(conn, error).await,
        TransferError::NoSuchUser => invalid("user_id", "is no user"),
        TransferError::NotAnActiveMember => invalid("user_id", "is not an active member of the org this lies in"),
        TransferError::Change(error) => match *error {
            ChangeError::FirstOwnerStays { owner } => ApiError::new(ErrorCode::ResourceInUse, "this is registered from a person's machine, and keeps that machine's owner as its first owner").with("by", "machine").with("owner", owner),
            other => refusals::change_failed(conn, other).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_kind_refusals_name_the_kinds_the_request_answers_for() {
        for kind in [
            ObjectKind::Provider,
            ObjectKind::Agent,
            ObjectKind::Environment,
            ObjectKind::Host,
            ObjectKind::Machine,
            ObjectKind::Template,
            ObjectKind::Mount,
            ObjectKind::McpServer,
            ObjectKind::Skill,
            ObjectKind::Document,
            ObjectKind::DocumentContainer,
        ] {
            assert!(require_transfer_kind(kind).is_ok(), "{kind:?}");
        }
        // Place transfers have their own request; this resource guard carries
        // a factual answer even if a future caller reaches it directly.
        for kind in [
            ObjectKind::Org,
            ObjectKind::Project,
            ObjectKind::Topic,
            ObjectKind::Thread,
        ] {
            let Err(error) = require_transfer_kind(kind) else {
                panic!("{kind:?} has its own transfer request");
            };
            assert_eq!(error.code(), ErrorCode::NotFound);
            assert_eq!(
                error.problem.detail,
                "Ownership can be transferred here for providers, agent profiles, environments, hosts, machines, templates, mounts, tool servers, skills, and documents."
            );
        }
    }
}
