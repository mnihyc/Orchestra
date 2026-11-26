//! The HTTP reader owns its real native call until EOF, failure or disconnect.
use super::*;
use bytes::BytesMut;
use orch_engine::operation_scope::http_origin::HttpOrigin;
use orch_link_contract::commands::{ProviderRelay, ProviderRelayHead};
use std::sync::{Mutex, PoisonError};
use tokio::sync::oneshot;

pub(super) struct Exchange {
    ctx: Ctx,
    pub relay: Arc<crate::relay::Relay>,
    pub options: options::Options,
    pub command: Option<Id>,
    _held: Option<crate::transit::CommandTransfer>,
    source: Option<CallSource>,
    raw: Option<HttpOrigin>,
    sent: Option<oneshot::Receiver<ApiResult<()>>>,
    pending: Bytes,
    failure: Arc<Mutex<Option<ApiError>>>,
}
fn fail(relay: &crate::relay::Relay, failure: &Mutex<Option<ApiError>>, error: ApiError) {
    *failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
    relay.cancel();
}
impl Exchange {
    pub fn new(ctx: Ctx, relay: Arc<crate::relay::Relay>, options: options::Options) -> Self {
        Self {
            ctx,
            relay,
            options,
            command: None,
            _held: None,
            source: None,
            raw: None,
            sent: None,
            pending: Bytes::new(),
            failure: Arc::default(),
        }
    }
    pub fn hold(&mut self, held: crate::transit::CommandTransfer) {
        self._held = Some(held);
    }
    pub fn attach(&mut self, source: CallSource, raw: HttpOrigin) {
        self.source = Some(source);
        self.raw = Some(raw);
    }
    pub fn start(&mut self, input: Bytes) {
        let Some(id) = self.command else {
            return;
        };
        let (ready, sent) = oneshot::channel();
        self.sent = Some(sent);
        let (relay, failure, dispatcher, idle) = (
            Arc::clone(&self.relay),
            Arc::clone(&self.failure),
            self.ctx.app.link.dispatcher(),
            self.options.idle_secs,
        );
        tokio::spawn(async move {
            let waiting = dispatcher.wait_for::<ProviderRelay>(id);
            tokio::pin!(waiting);
            let outcome = tokio::select! {
                result=tokio::time::timeout(Duration::from_secs(idle), relay.offer(0,input))=> {
                    let accepted = result.map_err(|_|unavailable("The native connection did not accept the request before providers.relay_idle_secs."))
                        .and_then(|result|result.map(|_|()).map_err(|_|unavailable("The model request's memory exchange ended.")));
                    let continued = accepted.is_ok();
                    let _ = ready.send(accepted.clone());
                    if continued { waiting.await.map(|_|()).map_err(|error|unavailable(error.to_string())) }
                    else { accepted }
                },
                result=&mut waiting=> {
                    let error = result.err().map(|error|unavailable(error.to_string())).unwrap_or_else(||unavailable("The native call ended before accepting its complete input."));
                    let _ = ready.send(Err(error.clone()));
                    Err(error)
                },
            };
            match outcome {
                Ok(()) => relay.finish(),
                Err(error) => {
                    fail(&relay, &failure, error);
                    let _ = dispatcher.cancel(id).await;
                }
            }
        });
    }
    async fn current(&self) -> ApiResult<()> {
        if self.ctx.app.clock.time() >= self.options.until {
            return Err(unavailable("The model call exceeded jobs.io_timeout_secs."));
        }
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| ApiError::internal("the public call has no native source"))?;
        let raw = self
            .raw
            .as_ref()
            .ok_or_else(|| ApiError::internal("the public call has no HTTP origin"))?;
        let mut tx = self.ctx.app.db.begin().await?;
        let current = http_origin::current(&self.ctx, &mut tx, raw).await?;
        source
            .check(&mut tx, self.ctx.app.clock.as_ref(), raw)
            .await
            .map_err(continuing_error)?;
        http_origin::check(&self.ctx, &current)?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }
    async fn next(&mut self) -> ApiResult<Option<Bytes>> {
        self.current().await?;
        let bytes = if !self.pending.is_empty() {
            Some(std::mem::take(&mut self.pending))
        } else {
            tokio::time::timeout(
                Duration::from_secs(self.options.idle_secs),
                self.relay.next(),
            )
            .await
            .map_err(|_| {
                unavailable("The model response was silent beyond providers.relay_idle_secs.")
            })?
            .map_err(|_| {
                self.failure
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
                    .unwrap_or_else(|| {
                        unavailable(
                            "The native model exchange ended before its response completed.",
                        )
                    })
            })?
        };
        // The response source owns every delivered piece and the final EOF, after native IO waited.
        self.current().await?;
        Ok(bytes)
    }
    async fn exact(&mut self, length: usize) -> ApiResult<Bytes> {
        let mut bytes = BytesMut::with_capacity(length);
        while bytes.len() < length {
            let mut piece = self
                .next()
                .await?
                .ok_or_else(|| unavailable("The native model response head was incomplete."))?;
            let take = (length - bytes.len()).min(piece.len());
            bytes.extend_from_slice(&piece.split_to(take));
            self.pending = piece;
        }
        Ok(bytes.freeze())
    }
    pub async fn response(mut self) -> ApiResult<Response> {
        self.sent
            .take()
            .ok_or_else(|| ApiError::internal("the model call was not issued"))?
            .await
            .map_err(|_| unavailable("The model request producer ended."))??;
        let length = self.exact(8).await?;
        let length = u64::from_be_bytes(length.as_ref().try_into().map_err(ApiError::internal)?);
        if length > self.options.request_bytes {
            return Err(unavailable(
                "The model response head exceeds the relay's captured byte allowance.",
            ));
        }
        let head: ProviderRelayHead = serde_json::from_slice(
            &self
                .exact(usize::try_from(length).map_err(ApiError::internal)?)
                .await?,
        )
        .map_err(ApiError::internal)?;
        let mut reply = http::Response::builder().status(head.status);
        for (name, value) in head.headers {
            let name = http::HeaderName::try_from(name).map_err(ApiError::internal)?;
            if !matches!(
                name.as_str(),
                "content-length" | "set-cookie" | "authorization" | "proxy-authorization"
            ) && !orch_model_contract::endpoint::HOP_BY_HOP.contains(&name.as_str())
            {
                reply = reply.header(
                    name,
                    http::HeaderValue::try_from(value).map_err(ApiError::internal)?,
                );
            }
        }
        self.current().await?;
        let stream = futures::stream::try_unfold(self, |mut exchange| async move {
            exchange
                .next()
                .await
                .map(|piece| piece.map(|piece| (piece, exchange)))
        });
        reply
            .body(Body::from_stream(stream))
            .map_err(ApiError::internal)
    }
}
impl Drop for Exchange {
    fn drop(&mut self) {
        self.relay.cancel();
        let Some(id) = self.command else {
            return;
        };
        let (db, at) = (self.ctx.app.db.clone(), self.ctx.app.clock.now());
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Ok(mut conn) = db.acquire().await
                    && let Err(error) = rows::finish_call(&mut conn, id, at).await
                {
                    tracing::warn!(%error,%id,"the model call's reader could not be closed");
                }
            });
        }
    }
}
