//! Transport-only checkpoints for an operation's already-produced HTTP response.

use crate::middleware::requests::ApiResult;

use axum::body::Body;

use axum::response::Response;

use http_body_util::BodyExt;

use http_body_util::StreamBody;

use std::future::Future;
type BodyError = Box<dyn std::error::Error + Send + Sync>;

/// Retain the response parts and original frames. The operation supplies all authority checks.
/// No connection or permission result is retained while waiting for the next upstream frame.
pub async fn checked<F, Fut>(response: Response, mut check: F) -> ApiResult<Response>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = ApiResult<()>> + Send + 'static,
{
    // This also protects metadata-only replies such as conditional 304 and unsatisfied 416.
    check().await?;
    let (parts, body) = response.into_parts();
    let frames = futures::stream::try_unfold((body, check), |(mut body, mut check)| async move {
        let Some(frame) = body.frame().await else {
            return Ok::<_, BodyError>(None);
        };
        let frame = frame.map_err(|error| Box::new(error) as BodyError)?;
        // The source may have waited. Check after it produces the frame, before exposing it.
        check()
            .await
            .map_err(|error| Box::new(error) as BodyError)?;
        Ok(Some((frame, (body, check))))
    });
    Ok(Response::from_parts(
        parts,
        Body::new(StreamBody::new(frames)),
    ))
}

#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::unwrap_used, reason = "transport controls fail by panicking")]
pub mod tests {

    use super::*;

    use crate::middleware::requests::ApiError;

    use bytes::Bytes;

    use http::HeaderMap;

    use http::HeaderValue;

    use http::StatusCode;

    use orch_api_contract::common::ErrorCode;

    use std::sync::Arc;

    use std::sync::atomic::AtomicBool;

    use std::sync::atomic::AtomicUsize;

    use std::sync::atomic::Ordering;

    use tokio::sync::Notify;
    #[cfg(any(test, feature = "test-support"))]
    async fn original_parts_data_trailers_and_upstream_errors_are_preserved() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut trailers = HeaderMap::new();
        trailers.insert("x-source-trailer", HeaderValue::from_static("exact"));
        let original = futures::stream::iter(vec![
            Ok::<_, std::io::Error>(hyper::body::Frame::data(Bytes::from_static(b"original"))),
            Ok(hyper::body::Frame::trailers(trailers.clone())),
            Err(std::io::Error::other("actual upstream failure")),
        ]);
        let mut response = Response::new(Body::new(StreamBody::new(original)));
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        response
            .headers_mut()
            .insert("etag", HeaderValue::from_static("original-tag"));
        let observed = calls.clone();
        let response = checked(response, move || {
            let calls = observed.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()["etag"], "original-tag");
        let mut body = response.into_body();
        assert_eq!(
            body.frame().await.unwrap().unwrap().into_data().unwrap(),
            Bytes::from_static(b"original")
        );
        assert_eq!(
            body.frame()
                .await
                .unwrap()
                .unwrap()
                .into_trailers()
                .unwrap(),
            trailers
        );
        assert!(
            body.frame()
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("actual upstream failure")
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "one header check and one per exposed successful frame; errors stay errors"
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_original_parts_data_trailers_and_upstream_errors_are_preserved()
    -> ::futures::future::LocalBoxFuture<'static, ()> {
        ::std::boxed::Box::pin(
            self::original_parts_data_trailers_and_upstream_errors_are_preserved(),
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn current_check_occurs_after_an_awaited_upstream_frame() {
        let reached = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let permitted = Arc::new(AtomicBool::new(true));
        let calls = Arc::new(AtomicUsize::new(0));
        let (notice, wait) = (reached.clone(), release.clone());
        let original = futures::stream::once(async move {
            notice.notify_one();
            wait.notified().await;
            Ok::<_, std::io::Error>(hyper::body::Frame::data(Bytes::from_static(b"not exposed")))
        });
        let observed = permitted.clone();
        let count = calls.clone();
        let response = checked(
            Response::new(Body::new(StreamBody::new(original))),
            move || {
                let permitted = observed.clone();
                let calls = count.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    if permitted.load(Ordering::SeqCst) {
                        Ok(())
                    } else {
                        Err(ApiError::new(
                            ErrorCode::AuthzDenied,
                            "the operation's current check refused",
                        ))
                    }
                }
            },
        )
        .await
        .unwrap();
        let read = async { response.into_body().frame().await };
        let change = async {
            reached.notified().await;
            permitted.store(false, Ordering::SeqCst);
            release.notify_one();
        };
        let (frame, ()) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tokio::join!(read, change)
        })
        .await
        .unwrap();
        assert!(matches!(frame, Some(Err(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_current_check_occurs_after_an_awaited_upstream_frame()
    -> ::futures::future::LocalBoxFuture<'static, ()> {
        ::std::boxed::Box::pin(self::current_check_occurs_after_an_awaited_upstream_frame())
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn metadata_only_replies_are_checked_before_returning_their_headers() {
        for status in [StatusCode::NOT_MODIFIED, StatusCode::RANGE_NOT_SATISFIABLE] {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = status;
            response
                .headers_mut()
                .insert("etag", HeaderValue::from_static("existing-content"));
            let result = checked(response, || async {
                Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the operation's current check refused",
                ))
            })
            .await;
            assert!(result.is_err());
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_metadata_only_replies_are_checked_before_returning_their_headers()
    -> ::futures::future::LocalBoxFuture<'static, ()> {
        ::std::boxed::Box::pin(
            self::metadata_only_replies_are_checked_before_returning_their_headers(),
        )
    }
}
