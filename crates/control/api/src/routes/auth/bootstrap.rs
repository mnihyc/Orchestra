//! One shell read of the person, personal state, organizations, mode, favorites, chat state and inbox summary.

use crate::middleware::authorization;
use crate::middleware::requests::ApiResult;
use crate::registry::{Answer, Ctx};
use crate::routes::{chats, nodes, notifications::inbox, personal};
use crate::sign_in::people;
use orch_api_contract::auth::{AdminMode, MeAuthorityReply, MeBootstrapReply};
use orch_db::queries::identity;

/// Shares the existing projections on one connection and one principal. Nothing here calls a
/// machine, takes a second pooled connection or loads a tree, chat message or resource inventory.
pub(super) async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Answer<MeBootstrapReply>> {
    let authed = ctx.user()?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let now = principal.now().get();
    let zone = ctx
        .headers
        .get("x-orch-timezone")
        .and_then(|value| value.to_str().ok())
        .filter(|zone| zone.parse::<chrono_tz::Tz>().is_ok() && *zone != authed.user.timezone);
    let updated = if let Some(zone) = zone {
        if identity::report_device_timezone(&mut conn, authed.id(), zone, now).await? {
            identity::user_by_id(&mut conn, authed.id()).await?
        } else {
            None
        }
    } else {
        None
    };
    let person = people::me(&mut conn, updated.as_ref().unwrap_or(&authed.user), now).await?;
    let state = identity::interface_state(&mut conn, principal.user())
        .await?
        .into_iter()
        .collect();
    let admin_mode = AdminMode {
        as_admin: authed.as_admin,
        available: identity::admin_mode_available(&mut conn, principal.user(), now).await?,
    };
    // The rail keeps the person's spaces when mode changes; administrative reach is browsed
    // explicitly elsewhere and must not turn the bootstrap into every organization's inventory.
    let orgs = nodes::orgs_for(&mut conn, &principal, now, false).await?;
    let favorites = personal::favorites_for(&mut conn, &principal, now).await?;
    let chat_states = chats::states_for(&ctx, &mut conn, &principal).await?;
    let read_states = personal::read_states_for(&mut conn, principal.user(), None).await?;
    let subscriptions = personal::subscriptions_for(&mut conn, principal.user(), None).await?;
    let notification_summary =
        inbox::summary_for(&mut conn, &principal, &ctx.app.engine, None, now).await?;
    // Release the shell connection before the public supplier acquires its own.
    drop(conn);
    let meta = crate::routes::meta::meta(ctx.clone(), (), ()).await?;
    Ok(Answer::from(MeBootstrapReply {
        meta,
        person,
        state,
        orgs,
        admin_mode,
        favorites,
        chat_states,
        read_states,
        subscriptions,
        notification_summary,
    })
    .header(http::header::CACHE_CONTROL, "no-store"))
}

/// Retake the native authority suppliers after admission, without replaying shell preferences.
pub(super) async fn authority(ctx: Ctx, (): (), (): ()) -> ApiResult<Answer<MeAuthorityReply>> {
    let authed = ctx.user()?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let now = principal.now().get();
    let person = people::me(&mut conn, &authed.user, now).await?;
    let admin_mode = AdminMode {
        as_admin: authed.as_admin,
        available: identity::admin_mode_available(&mut conn, principal.user(), now).await?,
    };
    let orgs = nodes::orgs_for(&mut conn, &principal, now, false).await?;
    drop(conn);
    let meta = crate::routes::meta::meta(ctx.clone(), (), ()).await?;
    Ok(Answer::from(MeAuthorityReply {
        person,
        admin_mode,
        orgs,
        meta,
    })
    .header(http::header::CACHE_CONTROL, "no-store"))
}
