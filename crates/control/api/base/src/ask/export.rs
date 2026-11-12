//! Bounded JSON export through the same owner/root-admin reader at every delivery.

use super::*;

use axum::body::Body;

use axum::response::Response;
pub(super) async fn export(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    let mut tx = ctx.app.db.begin().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let (row, inspecting, days) = opening_for(
        &ctx,
        &mut tx,
        &principal,
        ctx.id("id")?,
        true,
        &[Permission::ThreadView, Permission::ThreadExport],
    )
    .await?;
    let view = threads::thread(&mut tx, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?
        .active_continuation_id;
    let conversation = header(&row, inspecting, days)?;
    crate::routes::nodes::record(
        &ctx,
        &mut tx,
        orch_access_contract::audit::AuditAction::ThreadExported,
        crate::routes::nodes::Placed {
            id: row.id,
            org: row.root_org_id,
        },
        "Exported an Ask conversation",
        json!({"thread_id":row.id}),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let initial = format!(
        "{{\"conversation\":{},\"messages\":[",
        serde_json::to_string(&conversation).map_err(ApiError::internal)?
    );
    let stream = futures::stream::unfold(
        Some((ctx, row.id, view, row.seq, 0i64, false, Some(initial))),
        |state| async move {
            let (ctx, id, view, cut, mut after, mut comma, initial) = state?;
            let mut conn = match ctx.app.db.acquire().await {
                Ok(conn) => conn,
                Err(error) => {
                    return Some((
                        Err::<String, std::io::Error>(std::io::Error::other(error.to_string())),
                        None,
                    ));
                }
            };
            let principal = match ctx.operation_principal(&mut conn).await {
                Ok(principal) => principal,
                Err(error) => return Some((Err(std::io::Error::other(error.to_string())), None)),
            };
            if opening_for(
                &ctx,
                &mut conn,
                &principal,
                id,
                false,
                &[Permission::ThreadView, Permission::ThreadExport],
            )
            .await
            .is_err()
            {
                return Some((Err(std::io::Error::other("Ask export access ended")), None));
            }
            if let Some(initial) = initial {
                return Some((Ok(initial), Some((ctx, id, view, cut, after, comma, None))));
            }
            let items = match rows::items_in_view(&mut conn, id, view, Some(after), true, 200).await
            {
                Ok(items) => items,
                Err(error) => return Some((Err(std::io::Error::other(error.to_string())), None)),
            };
            if items.is_empty() || items.first().is_some_and(|item| item.seq_created > cut) {
                return Some((Ok("]}".into()), None));
            }
            let mut chunk = String::new();
            for item in items.into_iter().take_while(|item| item.seq_created <= cut) {
                after = item.seq_created;
                match item_message(&item) {
                    Ok(Some(message)) => {
                        if comma {
                            chunk.push(',');
                        }
                        comma = true;
                        match serde_json::to_string(&message) {
                            Ok(value) => chunk.push_str(&value),
                            Err(error) => {
                                return Some((Err(std::io::Error::other(error.to_string())), None));
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return Some((Err(std::io::Error::other(error.to_string())), None));
                    }
                }
            }
            Some((Ok(chunk), Some((ctx, id, view, cut, after, comma, None))))
        },
    );
    Response::builder()
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::CONTENT_DISPOSITION,
            "attachment; filename=ask-conversation.json",
        )
        .body(Body::from_stream(stream))
        .map_err(ApiError::internal)
}
