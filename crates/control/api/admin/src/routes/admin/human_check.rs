//! The secret of the human check, which a system admin sets and nobody reads back, and the test of the service.
//!
//! Which service asks, with which keys, for which acts, is the system's settings (Signing in). The secret is not a
//! setting: hCaptcha's secret, reCAPTCHA's secret key or the API key of a Google Cloud project is sealed under the
//! system's key as it arrives, kept apart from every node, and only ever opened to ask the service about a token.
//! **Test** asks the service with a token that is not one and says whether it accepted the keys.

use super::action;

use super::admin_of;

use super::invalid;

use super::record;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::resources::sealing::binding;

use crate::routes::resources::sealing::master_key;

use crate::sign_in::human_check::SECRET_NAME;

use crate::sign_in::human_check::SECRET_OWNER;

use crate::sign_in::human_check::secret_owner_id;

use orch_access_contract::audit::AuditAction;

use orch_api_contract::admin::AdminHumanCheckGet;

use orch_api_contract::admin::AdminHumanCheckSecretDelete;

use orch_api_contract::admin::AdminHumanCheckSecretSet;

use orch_api_contract::admin::AdminHumanCheckTest;

use orch_api_contract::admin::HumanCheckSecretBody;

use orch_api_contract::admin::HumanCheckState;

use orch_api_contract::admin::HumanCheckTestReply;

use orch_api_contract::common::ErrorCode;

use orch_db::DbError;

use orch_db::queries::settings::delete_secret as delete_secret_row;

use orch_db::queries::settings::put_secret;

use orch_types::Time;

use serde_json::json;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(AdminHumanCheckGet, state_of_secret)
        .on(AdminHumanCheckSecretSet, set_secret)
        .on(AdminHumanCheckSecretDelete, delete_secret)
        .on(AdminHumanCheckTest, test);
}

/// Whether a secret is kept, and when it was last set; and what the web edge's content policy leaves the chosen
/// service, from a look made now.
async fn state(ctx: &Ctx) -> ApiResult<HumanCheckState> {
    let (row, policy) = {
        let mut conn = ctx.app.db.acquire().await?;
        let row = orch_db::queries::settings::secret(
            &mut conn,
            SECRET_OWNER,
            secret_owner_id(),
            SECRET_NAME,
        )
        .await?;
        let policy = crate::sign_in::human_check::policy(&mut conn).await?;
        (row, policy)
    };
    // The look is made with no connection held.
    let edge =
        crate::sign_in::human_check::edge_state(&ctx.app.config.system.public_url, &policy).await;
    Ok(HumanCheckState {
        secret_set: row.is_some(),
        secret_changed_at: row.map(|row| Time::new(row.rotated_at.unwrap_or(row.created_at))),
        edge,
    })
}

/// Says whether the secret is kept, and what the web edge allows.
async fn state_of_secret(ctx: Ctx, (): (), (): ()) -> ApiResult<HumanCheckState> {
    admin_of(&ctx)?;
    state(&ctx).await
}

/// Seals the secret as it arrives and keeps it, in place of any before.
async fn set_secret(
    mut ctx: Ctx,
    (): (),
    body: HumanCheckSecretBody,
) -> ApiResult<HumanCheckState> {
    admin_of(&ctx)?;
    let secret = body.secret.trim();
    if secret.is_empty() {
        return Err(invalid("secret", "must not be empty"));
    }
    if secret.len() > orch_settings::secrets::SECRET_MAX_BYTES {
        return Err(
            ApiError::new(ErrorCode::RequestTooLarge, "the secret is too large")
                .with("limit", orch_settings::secrets::SECRET_MAX_BYTES),
        );
    }
    let (mut tx, guard) = action(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    let sealed = master_key()?
        .seal(
            secret.as_bytes(),
            &binding(SECRET_OWNER, secret_owner_id(), SECRET_NAME),
        )
        .map_err(|_| ApiError::internal("a secret could not be sealed"))?;
    put_secret(
        &mut tx,
        SECRET_OWNER,
        secret_owner_id(),
        SECRET_NAME,
        &sealed,
        Some(admin),
        ctx.now,
    )
    .await?;
    let who = guard.user().user.display_name.clone();
    record(
        &ctx,
        &mut tx,
        admin,
        orch_db::audit::action!(AuditAction::AuthHumanCheckSecretChanged),
        None,
        &format!("{who} set the secret of the human check"),
        json!({ "set": true }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    state(&ctx).await
}

/// Takes the secret away.
async fn delete_secret(mut ctx: Ctx, (): (), (): ()) -> ApiResult<HumanCheckState> {
    admin_of(&ctx)?;
    let (mut tx, guard) = action(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    delete_secret_row(&mut tx, SECRET_OWNER, secret_owner_id(), SECRET_NAME).await?;
    let who = guard.user().user.display_name.clone();
    record(
        &ctx,
        &mut tx,
        admin,
        orch_db::audit::action!(AuditAction::AuthHumanCheckSecretChanged),
        None,
        &format!("{who} took the secret of the human check away"),
        json!({ "set": false }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    state(&ctx).await
}

/// Asks the service with a token that is not one: nothing is signed in and nothing changes.
async fn test(ctx: Ctx, (): (), (): ()) -> ApiResult<HumanCheckTestReply> {
    admin_of(&ctx)?;
    // The guard of the act is checked first, and nothing is held while the service is asked.
    {
        let (tx, _guard) = action(&ctx).await?;
        tx.commit().await.map_err(DbError::from)?;
    }
    crate::sign_in::human_check::test(&ctx).await
}
