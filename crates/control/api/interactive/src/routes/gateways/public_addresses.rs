//! Management publishes only a prepared address set under the native system operation gate.

use crate::{
    middleware::{requests::ApiResult, transaction_guard},
    registry::{Ctx, Registry},
};
use orch_api_contract::{common::ReceiptProjection, public_addresses as api};
use orch_db::{
    DbError,
    audit::{self, Actor, Entry},
};

pub(super) fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(
            api::PublicAddressGet,
            crate::key_operations::direct::declared,
            read,
        )
        .on_with_operation(
            api::PublicAddressUpdate,
            crate::key_operations::direct::declared,
            update,
        );
}

async fn read(ctx: Ctx, (): (), (): ()) -> ApiResult<api::PublicAddressSnapshot> {
    ctx.ordinary_system_admin()?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.ordinary_system_admin_with(guard.user(), guard.principal().caller())?;
    let snapshot = crate::public_addresses::read(&ctx.app, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(snapshot)
}

async fn update(
    ctx: Ctx,
    (): (),
    body: api::PublicAddressUpdateBody,
) -> ApiResult<api::PublicAddressUpdated> {
    ctx.ordinary_system_admin()?;
    let candidate = api::PublicAddresses {
        public_url: body.public_url,
        alternate_urls: body.alternate_urls,
    };
    // Check ordinary identity and this operation before any certificate preparation; no transaction crosses IO.
    {
        let (tx, guard) = transaction_guard::begin(&ctx).await?;
        ctx.ordinary_system_admin_with(guard.user(), guard.principal().caller())?;
        tx.commit().await.map_err(DbError::from)?;
    }
    let managed =
        std::env::var_os("ORCH_FRONTEND_HTTPS_BIND").is_some_and(|value| !value.is_empty());
    let options = crate::public_addresses::PrepareOptions::read(&ctx.app.db, managed).await?;
    let mut prepared =
        crate::public_addresses::prepare(&ctx.app, candidate, body.expected_revision, options)
            .await?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.ordinary_system_admin_with(guard.user(), guard.principal().caller())?;
    let snapshot = prepared
        .publish(
            &mut tx,
            Some(guard.user().id()),
            guard.now().get(),
            Some(ctx.request_id.as_str()),
        )
        .await?;
    let mut entry = Entry::new(
        Actor::User(guard.user().id()),
        orch_access_contract::audit::AuditAction::SystemSettingsUpdated.as_str(),
        "system",
        None,
        "Public addresses updated",
    );
    entry.details = serde_json::json!({"key":orch_db::queries::public_addresses::KEY,"revision":snapshot.revision,"public_url":snapshot.addresses.public_url,"alternate_urls":snapshot.addresses.alternate_urls});
    audit::append(
        &mut tx,
        &entry,
        &crate::sign_in::origin(&ctx),
        guard.now().get(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    if prepared.release().await.is_err() {
        tracing::warn!("the preparation connection closed after the public address commit");
    }
    Ok(api::PublicAddressUpdated {
        projection: ReceiptProjection::Receipt,
        revision: snapshot.revision,
        public_url: snapshot.addresses.public_url,
        alternate_urls: snapshot.addresses.alternate_urls,
        using_example: snapshot.using_example,
    })
}
