//! A controller-owned public call uses the ordinary forwarder and its upstream accounting.

use super::{ProviderRole, internal};
use crate::leases::Leases;
use crate::roles::Call;
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use http_body_util::{BodyExt, Full};
use orch_envhost::blobs::{BlobWriter, RelaySource};
use orch_link_contract::commands::{
    CommandError, ErrorCode, LeaseCloseResult, LeaseMode, ProviderRelay, ProviderRelayHead,
};
use orch_model_contract::endpoint::HOP_BY_HOP;
use orch_types::{Clock, Hash, Id};
use std::sync::Arc;

fn failed(error: impl std::fmt::Display) -> CommandError {
    internal(error.to_string())
}

#[derive(Default)]
struct Input(BytesMut);
#[async_trait]
impl BlobWriter for Input {
    async fn received(&mut self) -> std::io::Result<i64> {
        i64::try_from(self.0.len()).map_err(std::io::Error::other)
    }
    async fn write(&mut self, offset: i64, bytes: Bytes) -> std::io::Result<()> {
        if i64::try_from(self.0.len()).map_err(std::io::Error::other)? != offset {
            return Err(std::io::Error::other(
                "the model request has another received prefix",
            ));
        }
        self.0.extend_from_slice(&bytes);
        Ok(())
    }
    async fn hash(&mut self) -> std::io::Result<Hash> {
        Ok(Hash::of(&self.0))
    }
    async fn discard(&mut self) -> std::io::Result<()> {
        self.0.clear();
        Ok(())
    }
}
struct Opened {
    leases: Arc<Leases>,
    lease: Id,
    clock: Arc<dyn Clock>,
}
impl Drop for Opened {
    fn drop(&mut self) {
        if let Err(error) = self.leases.close_checked(self.lease, self.clock.time()) {
            tracing::error!(%error,"a public model lease could not be closed");
        }
    }
}

impl ProviderRole {
    pub(super) async fn relay_call(
        &self,
        call: &Call,
        body: ProviderRelay,
    ) -> Result<LeaseCloseResult, CommandError> {
        if body.start.provider_id != body.lease.provider_id
            || body.lease.lease_id != call.context.id
            || body.lease.thread_id != call.context.id
            || body.lease.mode != LeaseMode::Forwarded
            || body.request_bytes < 0
            || u64::try_from(body.request_bytes).is_ok_and(|size| size > body.max_request_bytes)
            || body.lease.allowed_models.is_empty()
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "The public model call changed its admitted identity.",
            ));
        }
        if self.leases.knows(body.lease.lease_id) {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "This model call may already have reached its upstream; it was not repeated.",
            ));
        }
        let pipe = self.services.content.relay_for(
            call.context.id,
            body.relay_id,
            body.idle_secs,
            body.retry_ms,
        );
        let mut input = Input::default();
        tokio::select! {
            biased;
            _=call.context.cancel.cancelled()=>return Err(CommandError::new(ErrorCode::Conflict,"The model call was cancelled.")),
            result=pipe.receive(body.request_bytes,&mut input)=>result.map_err(|error|internal(error.to_string()))?,
        }
        if input.hash().await.map_err(failed)? != body.request_hash {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "The model request does not match its admitted digest.",
            ));
        }
        let provider = Arc::new(self.prepared_provider(call, &body.start)?.into_entry(
            body.start.provider_id,
            self.forwarder.protocols(),
            &self.services,
        )?);
        let lease = self
            .leases
            .open(&body.lease, self.services.clock.time())
            .map_err(failed)?;
        let _opened = Opened {
            leases: Arc::clone(&self.leases),
            lease: body.lease.lease_id,
            clock: Arc::clone(&self.services.clock),
        };
        let mut request = http::Request::builder()
            .method(http::Method::POST)
            .uri(&body.path)
            .body(Full::new(input.0.freeze()))
            .map_err(failed)?;
        for (name, value) in body.headers {
            let name = http::HeaderName::try_from(name).map_err(failed)?;
            let value = http::HeaderValue::try_from(value).map_err(failed)?;
            if matches!(
                name.as_str(),
                "accept" | "content-type" | "anthropic-version" | "anthropic-beta" | "openai-beta"
            ) {
                request.headers_mut().insert(name, value);
            }
        }
        let response = tokio::select! {
            biased;
            _=call.context.cancel.cancelled()=>return Err(CommandError::new(ErrorCode::Conflict,"The model call was cancelled.")),
            response=self.forwarder.serve_relay(request,lease,body.max_request_bytes,provider)=>response,
        };
        let (parts, mut response) = response.into_parts();
        let nominated = parts
            .headers
            .get_all(http::header::CONNECTION)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(|name| name.trim().to_ascii_lowercase())
            .collect::<std::collections::BTreeSet<_>>();
        let head = ProviderRelayHead {
            status: parts.status.as_u16(),
            headers: parts
                .headers
                .iter()
                .filter(|(name, _)| {
                    !HOP_BY_HOP.contains(&name.as_str())
                        && !matches!(
                            name.as_str(),
                            "content-length"
                                | "set-cookie"
                                | "authorization"
                                | "proxy-authorization"
                        )
                        && !nominated.contains(name.as_str())
                })
                .map(|(name, value)| (name.as_str().to_owned(), value.as_bytes().to_vec()))
                .collect(),
        };
        let head = serde_json::to_vec(&head).map_err(failed)?;
        let length = u64::try_from(head.len()).map_err(failed)?.to_be_bytes();
        let mut offset = body.request_bytes;
        tokio::select! {
            biased;
            _=call.context.cancel.cancelled()=>return Err(CommandError::new(ErrorCode::Conflict,"The model call was cancelled.")),
            result=async {
                offset=pipe.push_bytes(offset,Bytes::copy_from_slice(&length)).await.map_err(failed)?;
                offset=pipe.push_bytes(offset,Bytes::from(head)).await.map_err(failed)?;
                while let Some(frame)=response.frame().await{
                    let frame=frame.map_err(failed)?;
                    if let Ok(bytes)=frame.into_data(){
                        offset=pipe.push_bytes(offset,bytes).await.map_err(failed)?;
                    }
                }
                Ok::<(),CommandError>(())
            }=>result?,
        }
        let totals = self
            .leases
            .close_checked(body.lease.lease_id, self.services.clock.time())
            .map_err(failed)?;
        super::fitting_close_groups(
            call.context.id,
            LeaseCloseResult {
                requests: totals.requests,
                usage: totals.reported(),
                usage_groups: totals.usage_groups,
            },
            self.services.link.message_bytes(),
        )
        .map_err(failed)
    }
}
