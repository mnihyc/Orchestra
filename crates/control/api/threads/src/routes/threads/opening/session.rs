//! The first document request renews the session and carries the native thread response.
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::{StreamExt, stream};
use http::{HeaderValue, Request, StatusCode, header};
use orch_api_contract::auth::{AuthSessionThreadOpen, AuthThreadOpening};
use orch_api_contract::routes::Endpoint;
use orch_api_contract::threads::ThreadOpeningGet;
use std::net::SocketAddr;
use tower::ServiceExt;

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::sign_in::session_restore;
use crate::state::App;

pub(super) fn mount(registry: &mut Registry, thread: axum::Router<App>) {
    registry.on_raw(AuthSessionThreadOpen, move |ctx, (), _| {
        open(ctx, thread.clone())
    });
}

async fn open(ctx: Ctx, thread: axum::Router<App>) -> ApiResult<Response> {
    let id = ctx.id("id")?;
    let app = ctx.app.clone();
    let remote = ctx.remote;
    let opening = ctx.opening;
    let session = session_restore::restore(ctx).await?;
    let Some(reader) = session.body else {
        let mut empty = axum::Json(None::<AuthThreadOpening>).into_response();
        empty.headers_mut().extend(session.headers);
        return Ok(empty);
    };

    // Enter the same browser endpoint as a signed-in navigation, with the credential
    // just issued to this cookie. Its snapshot and output checks run on the real body.
    let mut request = Request::get(
        ThreadOpeningGet::ROUTE
            .path
            .replace("{id}", &id.to_string()),
    )
    .header(
        header::AUTHORIZATION,
        format!("Bearer {}", reader.access_token),
    )
    .header("x-orch-opening", opening.to_string())
    .body(Body::empty())
    .map_err(ApiError::internal)?;
    if let Some(ip) = remote {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(ip, 0)));
    }
    let native = thread
        .with_state(app)
        .oneshot(request)
        .await
        .unwrap_or_else(|never| match never {});
    if native.status() == StatusCode::UNAUTHORIZED {
        return Ok(native);
    }

    let mut prefix = br#"{"session":"#.to_vec();
    serde_json::to_writer(&mut prefix, &reader).map_err(ApiError::internal)?;
    if native.status().is_success() {
        prefix.extend_from_slice(br#","opening":{"status":"ready","data":"#);
    } else {
        prefix.extend_from_slice(br#","opening":{"status":"failed","retry_after":"#);
        let delay = native
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
        serde_json::to_writer(&mut prefix, &delay).map_err(ApiError::internal)?;
        prefix.extend_from_slice(br#","problem":"#);
    }
    let body = stream::once(async move { Ok::<_, axum::Error>(Bytes::from(prefix)) })
        .chain(native.into_body().into_data_stream())
        .chain(stream::once(async { Ok(Bytes::from_static(b"}}")) }));
    let mut reply = Response::new(Body::from_stream(body));
    reply.headers_mut().extend(session.headers);
    reply.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(reply)
}
