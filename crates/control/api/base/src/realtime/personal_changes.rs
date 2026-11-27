//! Personal-state event values projected after commit without enlarging database notifications.

use super::Connection;

use super::Fail;

use super::Inner;

use crate::middleware::authentication::Caller;

use crate::registry::Ctx;

use orch_api_contract::routes::Endpoint;

use orch_realtime_contract::feeds::UserEvent;

use orch_types::Id;

use orch_types::Json;

use std::sync::Arc;
/// The notice already names its sole owner; delivery authorization is retaken before it is sent.
pub(crate) async fn project(
    inner: &Inner,
    connection: &Connection,
    mut event: UserEvent,
) -> Result<Option<UserEvent>, Fail> {
    let user = connection.user;
    if let UserEvent::Subscription {
        node_id,
        level,
        source,
        effective_level,
    } = &mut event
    {
        let mut conn = inner.db.acquire().await?;
        let Some(principal) =
            super::subscriptions::principal_for_connection(inner, &mut conn, connection).await?
        else {
            return Ok(None);
        };
        let visible = orch_authz::nodes::holding(
            &mut conn,
            &principal,
            &[*node_id],
            crate::middleware::authorization::VIEW,
            orch_authz::Act::Use,
            orch_authz::Policy::default(),
        )
        .await
        .map_err(Fail::other)?;
        if visible.is_empty() {
            return Ok(None);
        }
        // A notification is a wakeup, never a source snapshot that may overwrite a later choice.
        let row = orch_db::queries::hierarchy::subscriptions_on(&mut conn, user, &[*node_id])
            .await?
            .pop();
        let effective =
            orch_db::queries::notifications::subscribers::level_of(&mut conn, user, *node_id)
                .await?;
        *effective_level =
            Some(serde_json::from_value(Json::String(effective)).map_err(Fail::other)?);
        (*level, *source) = match row {
            Some(row) => (
                Some(serde_json::from_value(Json::String(row.level)).map_err(Fail::other)?),
                Some(serde_json::from_value(Json::String(row.source)).map_err(Fail::other)?),
            ),
            None => (None, None),
        };
    }
    if let UserEvent::State { key, value } = &mut event {
        let named = serde_json::to_value(key).map_err(Fail::other)?;
        let mut conn = inner.db.acquire().await?;
        *value = Some(
            orch_db::queries::identity::interface_state(&mut conn, user)
                .await?
                .into_iter()
                .find_map(|(key, value)| (named.as_str() == Some(key.as_str())).then_some(value))
                .unwrap_or(Json::Null),
        );
    }
    if let UserEvent::ContinuationReadState {
        thread_id,
        thread_display,
        ..
    } = &mut event
    {
        // One person's sessions may have different administrative reach. The sender's
        // presentation is never authority for a recipient, even on the personal feed.
        *thread_display = None;
        let mut conn = inner.db.acquire().await?;
        if let Some(principal) =
            super::subscriptions::principal_for_connection(inner, &mut conn, connection).await?
        {
            let readable = orch_authz::nodes::holding(
                &mut conn,
                &principal,
                &[*thread_id],
                crate::middleware::authorization::VIEW,
                orch_authz::Act::Use,
                orch_authz::Policy::default(),
            )
            .await
            .map_err(Fail::other)?;
            if !readable.is_empty() {
                *thread_display = orch_engine::views::thread_displays(
                    &mut conn,
                    &[*thread_id],
                    Some(principal.user()),
                    principal.now(),
                )
                .await
                .map_err(Fail::other)?
                .remove(&*thread_id);
            }
        }
    }
    if let UserEvent::Approvals { summary } = &mut event {
        let mut conn = inner.db.acquire().await?;
        if let Some(principal) =
            super::subscriptions::principal_for_connection(inner, &mut conn, connection).await?
        {
            *summary = Some(Box::new(
                crate::routes::notifications::inbox::summary_for(
                    &mut conn,
                    &principal,
                    inner.engine()?,
                    None,
                    inner.clock.now(),
                )
                .await
                .map_err(Fail::other)?,
            ));
        }
    }
    if let UserEvent::Enrollment { receipt, status } = &mut event {
        // The notification contains only the issuer's receipt. A live session gets the same
        // native permission cut as its authenticated status read, never another person's row.
        *status = None;
        if let Some(app) = inner.app() {
            let mut conn = inner.db.acquire().await?;
            let before = orch_db::queries::access::epoch(&mut conn).await?;
            if let Some((principal, authed)) =
                super::subscriptions::reader_for_connection(inner, &mut conn, connection).await?
            {
                let ctx = Ctx::for_tool(
                    app,
                    Caller::User(Arc::new(authed)),
                    orch_api_contract::resources::MachineEnrollmentStatusGet::ROUTE,
                    Id::nil(),
                )
                .with_admitted_caller(principal.caller().clone())
                .map_err(Fail::other)?;
                let projected = crate::routes::resources::machines::enrollment::show(
                    &ctx, &mut conn, &principal, receipt,
                )
                .await;
                let after = orch_db::queries::access::epoch(&mut conn).await?;
                match projected {
                    Ok(progress) if before == after => *status = Some(Box::new(progress)),
                    Ok(_) => {} // Authority moved: the active dialog repairs only this exact receipt.
                    Err(error) if error.problem.status == 403 || error.problem.status == 404 => {}
                    Err(error) => return Err(Fail::other(error)),
                }
            }
        }
    }
    Ok(Some(event))
}
