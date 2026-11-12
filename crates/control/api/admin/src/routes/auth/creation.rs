//! Account creation replay is proved through the native username/password path.

use super::*;

use crate::middleware::idempotency::ProofCreation;

use orch_api_contract::auth::AccountCreatedReceipt;

use orch_api_contract::auth::SetupCreateBody;

use orch_api_contract::auth::SetupCreateReply;

use orch_db::idempotency::creation::Created;
pub(crate) fn setup_binding() -> ProofCreation<(), SetupCreateBody, SetupCreateReply> {
    ProofCreation {
        kind: "setup_account",
        principal: |ctx, body| Box::pin(account(ctx, body.username, body.password)),
        replay: |ctx, (), body, created| {
            Box::pin(async move {
                if account(ctx, body.username, body.password).await? != Some(created.id) {
                    return Err(crate::routes::meta::setup_settled());
                }
                Ok(SetupCreateReply::Receipt(receipt(created)))
            })
        },
    }
}
orch_api_base::__api_bind_2e1ba4f348!();

pub(super) fn register_binding() -> ProofCreation<(), AuthRegisterBody, AuthRegisterReply> {
    ProofCreation {
        kind: "registered_account",
        principal: |ctx, body| Box::pin(account(ctx, body.username, body.password)),
        replay: |ctx, (), body, created| {
            Box::pin(async move {
                if account(ctx, body.username, body.password).await? != Some(created.id) {
                    return Err(accounts::public_name_conflict());
                }
                Ok(AuthRegisterReply::AlreadyCreated(receipt(created)))
            })
        },
    }
}

fn receipt(created: Created) -> AccountCreatedReceipt {
    AccountCreatedReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        user_id: created.id,
        sign_in_required: true,
    }
}

async fn account(ctx: Ctx, username: String, password: String) -> ApiResult<Option<Id>> {
    let mut conn = ctx.app.db.acquire().await?;
    let row = identity::user_by_username(&mut conn, &username).await?;
    let who = throttle::Who::named(
        row.as_ref().map(|row| row.id),
        &throttle::normalize(&username),
    );
    // This is the existing counted proof, including its decoy for an unknown
    // username. It opens no session and sends no credential in its answer.
    let right = crate::sign_in::proofs::password_right(
        &ctx,
        &mut conn,
        &Limits::of(&ctx.app.config.auth),
        &who,
        row.as_ref(),
        &password,
        true,
    )
    .await?;
    Ok(row
        .filter(|row| right && row.status == "active")
        .map(|row| row.id))
}
