//! An explicit operation notice shares the sender's rolling allowance and activity record.
use crate::{
    middleware::requests::{ApiError, ApiResult},
    registry::Ctx,
};
use orch_access_contract::decisions::ObjectRef;
use orch_api_contract::common::ErrorCode;
use orch_authz::Principal;
use orch_db::{Conn, queries::notifications::operations};
use serde_json::json;

/// Charge the exact reviewed recipients inside the same transaction as the resource change.
pub async fn record_choice(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    governor: ObjectRef,
    kind: &str,
    people: usize,
) -> ApiResult<()> {
    orch_db::queries::chats::lock_named(conn, &format!("person.notify:{}", principal.user()))
        .await?;
    let (_, resource, _) = orch_authz::resources::site(conn, governor)
        .await
        .map_err(ApiError::internal)?;
    let place = resource
        .home
        .map_or(orch_settings::Place::System, orch_settings::Place::Node);
    let max = orch_settings::get::<i64>(
        conn,
        place,
        orch_settings_contract::registry::keys::NOTIFICATIONS_PERSON_RATE_PER_HOUR,
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or(0);
    let asked = i64::try_from(people).map_err(ApiError::internal)?;
    let since = ctx.now - chrono::Duration::hours(1);
    let used = operations::person_rate_used(conn, principal.user(), resource.root, since).await?;
    if max > 0 && used.saturating_add(asked) > max {
        let mut error = ApiError::new(
            ErrorCode::RateLimited,
            "The person notification rate is reached",
        )
        .with("limit", max)
        .with("asked", asked)
        .with(
            "setting",
            orch_settings_contract::registry::keys::NOTIFICATIONS_PERSON_RATE_PER_HOUR,
        );
        if let Some(at) = operations::person_rate_release(
            conn,
            principal.user(),
            resource.root,
            since,
            used.saturating_add(asked).saturating_sub(max),
        )
        .await?
        {
            let seconds = u64::try_from(
                (at + chrono::Duration::hours(1) - ctx.now)
                    .num_seconds()
                    .max(1),
            )
            .map_err(ApiError::internal)?;
            error = error.retry_after(seconds);
        }
        return Err(error);
    }
    let mut entry = orch_db::audit::Entry::new(
        orch_db::audit::Actor::User(principal.user()),
        orch_db::audit::name!("person.notified"),
        object.kind.as_str(),
        Some(object.id),
        "Notified affected people of a resource operation",
    );
    entry.org_id = resource.root;
    entry.node_id = resource.home;
    entry.details = json!({"count":asked,"operation_kind":kind});
    orch_db::audit::append(conn, &entry, &crate::sign_in::origin(ctx), ctx.now).await?;
    Ok(())
}
