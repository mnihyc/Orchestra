//! A page's first screen in one request.
//!
//! The document's own starter asks for the browser session together with every native read the page shows first. The
//! session is restored as `/auth/session` restores it; then each read goes through the same browser route as a signed-in
//! request with the credential just issued, so its authorization, snapshot and output checks are the native ones. The
//! answer is one line of JSON for the session, then one line for each read as soon as it is ready (a slow read does not
//! hold back the others), named by its position in the query.
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::sign_in::session_restore;
use crate::state::App;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::response::Response;
use bytes::Bytes;
use futures::stream::{self, FuturesUnordered, StreamExt};
use http::{HeaderMap, HeaderValue, Request, header};
use http_body_util::BodyExt;
use orch_api_contract::auth::{AuthSessionOpen, AuthSessionOpenQuery};
use std::net::{IpAddr, SocketAddr};
use tower::ServiceExt;

/// What of the page's own request its reads see as theirs: the device's time zone (the shell's read keeps it) and the
/// address in view (the server keeps the place a person was last in from it).
const CARRIED: [&str; 2] = ["x-orch-timezone", "x-orch-view"];

pub(super) fn mount(registry: &mut Registry) {
    let routes = registry.take_page_reads();
    registry.on_raw(AuthSessionOpen, move |ctx, query, _| {
        open(ctx, query, routes.clone())
    });
}

async fn open(ctx: Ctx, query: AuthSessionOpenQuery, router: axum::Router) -> ApiResult<Response> {
    let app = ctx.app.clone();
    let remote = ctx.remote;
    let opening = ctx.opening.to_string();
    let mut carried = HeaderMap::new();
    for name in CARRIED {
        if let Some(value) = ctx.headers.get(name) {
            carried.insert(name, value.clone());
        }
    }
    let session = session_restore::restore(ctx).await?;
    let mut first = br#"{"session":"#.to_vec();
    serde_json::to_writer(&mut first, &session.body).map_err(ApiError::internal)?;
    first.extend_from_slice(b"}\n");
    let parts = FuturesUnordered::new();
    // A browser without a session reads nothing: the page it opens is for somebody signed in.
    if let Some(reader) = &session.body {
        let bearer = HeaderValue::from_str(&format!("Bearer {}", reader.access_token))
            .map_err(ApiError::internal)?;
        let opening = HeaderValue::from_str(&opening).map_err(ApiError::internal)?;
        for (index, path) in query.read.into_iter().enumerate() {
            let mut headers = carried.clone();
            headers.insert(header::AUTHORIZATION, bearer.clone());
            headers.insert("x-orch-opening", opening.clone());
            let router = router.clone();
            let app = app.clone();
            parts.push(async move {
                line(index, read(router, app, &path, headers, remote).await).await
            });
        }
    }
    let body = stream::once(async move { Ok::<_, axum::Error>(Bytes::from(first)) })
        .chain(parts.map(Ok::<_, axum::Error>));
    let mut reply = Response::new(Body::from_stream(body));
    reply.headers_mut().extend(session.headers);
    reply.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    reply
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(reply)
}

/// One read, through the browser's own route for it.
async fn read(
    router: axum::Router,
    app: App,
    path: &str,
    headers: HeaderMap,
    remote: Option<IpAddr>,
) -> ApiResult<Response> {
    if !path.starts_with('/') || path.starts_with("//") {
        return Err(refused());
    }
    let mut request = Request::get(path)
        .body(Body::empty())
        .map_err(|_| refused())?;
    request.extensions_mut().insert(app);
    *request.headers_mut() = headers;
    if let Some(ip) = remote {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(ip, 0)));
    }
    Ok(router
        .oneshot(request)
        .await
        .unwrap_or_else(|never| match never {}))
}

fn refused() -> ApiError {
    ApiError::invalid(
        "read",
        "a read is a path of the API below its base, with its query",
    )
}

/// The line of one read: its answer as the read gave it, or its refusal.
async fn line(index: usize, reply: ApiResult<Response>) -> Bytes {
    let mut out = format!(r#"{{"read":{index},"status":"#).into_bytes();
    match answered(reply).await {
        Ok((true, _, data)) => {
            out.extend_from_slice(br#""ready","data":"#);
            out.extend_from_slice(&data);
        }
        Ok((false, retry, problem)) => failed(&mut out, retry, &problem),
        Err(error) => {
            let (problem, retry) = error.into_parts_for(&orch_types::new_id().to_string());
            failed(
                &mut out,
                retry,
                &serde_json::to_vec(&problem).unwrap_or_else(|_| b"null".to_vec()),
            );
        }
    }
    out.extend_from_slice(b"}\n");
    Bytes::from(out)
}

fn failed(out: &mut Vec<u8>, retry: Option<u64>, problem: &[u8]) {
    out.extend_from_slice(br#""failed","retry_after":"#);
    out.extend_from_slice(
        retry
            .map_or_else(|| "null".to_owned(), |seconds| seconds.to_string())
            .as_bytes(),
    );
    out.extend_from_slice(br#","problem":"#);
    out.extend_from_slice(problem);
}

/// Whether the read succeeded, its retry hint, and its JSON on one line. Only JSON reads are a page's first screen.
async fn answered(reply: ApiResult<Response>) -> ApiResult<(bool, Option<u64>, Vec<u8>)> {
    let response = reply?;
    let json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.starts_with("application/json") || value.starts_with("application/problem+json")
        });
    if !json {
        return Err(refused());
    }
    let retry = response
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok()?.parse::<u64>().ok());
    let success = response.status().is_success();
    let mut data = response
        .into_body()
        .collect()
        .await
        .map_err(ApiError::internal)?
        .to_bytes()
        .to_vec();
    // A line ends at its newline. JSON keeps a newline only as space between its tokens, never inside a string.
    for byte in &mut data {
        if *byte == b'\n' || *byte == b'\r' {
            *byte = b' ';
        }
    }
    if data.iter().all(u8::is_ascii_whitespace) {
        data = b"null".to_vec();
    }
    Ok((success, retry, data))
}
