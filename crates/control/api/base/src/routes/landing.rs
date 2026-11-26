//! Continue a browser sign-in with the ordinary shell and the native opening it leads to.
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::state::App;
use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::response::Response;
use bytes::Bytes;
use futures::{StreamExt, stream};
use http::{HeaderValue, Request, StatusCode, header};
use http_body_util::BodyExt;
use orch_api_contract::auth::{MeBootstrapGet, MeLandingGet, MeLandingQuery};
use orch_api_contract::routes::Endpoint;
use orch_api_contract::threads::ThreadOpeningGet;
use orch_types::Id;
use serde::Deserialize;
use std::net::SocketAddr;
use tower::ServiceExt;

pub(super) fn mount(registry: &mut Registry) {
    let native = registry.take_landing_reads();
    registry.on_raw(MeLandingGet, move |ctx, query, _| {
        open(ctx, query, native.clone())
    });
}

async fn read(ctx: &Ctx, native: Router<App>, path: &str) -> ApiResult<Response> {
    let mut request = Request::get(path)
        .header("x-orch-opening", ctx.opening.to_string())
        .body(Body::empty())
        .map_err(ApiError::internal)?;
    for name in ["authorization", "x-orch-timezone"] {
        if let Some(value) = ctx.headers.get(name) {
            request.headers_mut().insert(name, value.clone());
        }
    }
    if let Some(ip) = ctx.remote {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(ip, 0)));
    }
    // The shell and the thread the sign-in leads to are that page's first screen (see `orch_db::pool::first_screen`).
    Ok(
        orch_db::pool::first_screen(native.with_state(ctx.app.clone()).oneshot(request))
            .await
            .unwrap_or_else(|never| match never {}),
    )
}

fn saved_thread(shell: &[u8]) -> ApiResult<Option<Id>> {
    #[derive(Deserialize)]
    struct Shell {
        state: serde_json::Value,
    }
    let shell: Shell = serde_json::from_slice(shell).map_err(ApiError::internal)?;
    let Some(place) = shell.state.pointer("/sidebar/last_place") else {
        return Ok(None);
    };
    let Some(id) = place["node_id"]
        .as_str()
        .and_then(|id| id.parse::<Id>().ok())
    else {
        return Ok(None);
    };
    let route = format!("/thread/{id}");
    Ok(place["route"]
        .as_str()
        .filter(|path| path.strip_suffix('/').unwrap_or(path) == route)
        .map(|_| id))
}

async fn open(ctx: Ctx, query: MeLandingQuery, native: Router<App>) -> ApiResult<Response> {
    let shell = read(&ctx, native.clone(), MeBootstrapGet::ROUTE.path).await?;
    if !shell.status().is_success() {
        return Ok(shell);
    }
    let bytes = shell
        .into_body()
        .collect()
        .await
        .map_err(ApiError::internal)?
        .to_bytes();
    let id = match query.thread {
        Some(id) => Some(id),
        None => saved_thread(&bytes)?,
    };
    let mut prefix = br#"{"shell":"#.to_vec();
    prefix.extend_from_slice(&bytes);
    prefix.extend_from_slice(br#","thread_id":"#);
    serde_json::to_writer(&mut prefix, &id).map_err(ApiError::internal)?;
    let body = if let Some(id) = id {
        let path = ThreadOpeningGet::ROUTE
            .path
            .replace("{id}", &id.to_string());
        let thread = read(&ctx, native, &path).await?;
        if thread.status() == StatusCode::UNAUTHORIZED {
            return Ok(thread);
        }
        if thread.status().is_success() {
            prefix.extend_from_slice(br#","thread":{"status":"ready","data":"#);
        } else {
            let delay = thread
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
            prefix.extend_from_slice(br#","thread":{"status":"failed","retry_after":"#);
            serde_json::to_writer(&mut prefix, &delay).map_err(ApiError::internal)?;
            prefix.extend_from_slice(br#","problem":"#);
        }
        Body::from_stream(
            stream::once(async move { Ok::<_, axum::Error>(Bytes::from(prefix)) })
                .chain(thread.into_body().into_data_stream())
                .chain(stream::once(async { Ok(Bytes::from_static(b"}}")) })),
        )
    } else {
        prefix.extend_from_slice(br#","thread":{"status":"omitted"}}"#);
        Body::from(prefix)
    };
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}
