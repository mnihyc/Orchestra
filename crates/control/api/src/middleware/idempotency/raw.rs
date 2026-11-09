//! Bounded incoming raw input and typed replay on the existing raw execution path.
use super::*;
use axum::{body::Body, response::Response};
use orch_blob_contract::range::BlobMeta;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawFormat {
    Bytes,
    Json,
}
/// Limits and non-credential input headers come from the native body decoder.
pub(crate) struct RawInputSpec {
    pub limit: i64,
    pub format: RawFormat,
    pub fields: Json,
}
pub(crate) struct RawCreation<Q, T> {
    pub kind: &'static str,
    pub input: for<'a> fn(&'a Ctx) -> BoxFuture<'a, ApiResult<RawInputSpec>>,
    pub replay: CreationReplay<Q, RawIncoming, T>,
}
pub(crate) type RawReplay<Q> =
    dyn Fn(Ctx, Q, RawIncoming, Created) -> BoxFuture<'static, ApiResult<Response>> + Send + Sync;
pub(crate) struct RawRegistered<Q> {
    pub input: for<'a> fn(&'a Ctx) -> BoxFuture<'a, ApiResult<RawInputSpec>>,
    pub replay: Arc<RawReplay<Q>>,
}
/// This is the retry's newly supplied input, not stored response content or authority.
pub(crate) struct RawIncoming {
    staged: crate::blobs::StagedBody,
    json: Option<Json>,
    fields: Json,
}
impl RawIncoming {
    pub(crate) async fn take(ctx: &Ctx, body: Body, spec: RawInputSpec) -> ApiResult<Self> {
        if spec.limit < 0
            || (spec.format == RawFormat::Json
                && spec.limit
                    > i64::try_from(orch_api_contract::common::MAX_JSON_BYTES).unwrap_or(i64::MAX))
        {
            return Err(ApiError::internal(
                "the native raw input limit does not match its representation",
            ));
        }
        let staged = ctx
            .app
            .blobs
            .stage_stream(body.into_data_stream(), spec.limit)
            .await
            .map_err(crate::uploads::stored_badly)?;
        let json = if spec.format == RawFormat::Json {
            Some(
                staged
                    .json()
                    .await
                    .map_err(|_| ApiError::invalid("body", "the body is not valid JSON"))?,
            )
        } else {
            None
        };
        Ok(Self {
            staged,
            json,
            fields: spec.fields,
        })
    }
    pub(crate) fn json(&self) -> Option<&Json> {
        self.json.as_ref()
    }
    pub(crate) fn meta(&self) -> BlobMeta {
        self.staged.meta()
    }
    pub(crate) fn fields(&self) -> &Json {
        &self.fields
    }
    pub(crate) async fn into_body(self) -> ApiResult<Body> {
        self.staged
            .body()
            .await
            .map_err(crate::uploads::stored_badly)
    }
    pub(super) fn fingerprint(&self, account: Id, key: &str) -> ApiResult<Hash> {
        let body = self.json.clone().unwrap_or_else(
            || serde_json::json!({"hash":self.meta().hash,"size":self.meta().size}),
        );
        let data = serde_json::json!({"json":self.json.is_some(),"body":canonical(body),"fields":canonical(self.fields.clone())});
        fingerprint(account, key, "orch.creation.raw.v1", data)
    }
}
impl<Q: Send + 'static, B, T: serde::Serialize + Send + 'static> From<RawCreation<Q, T>>
    for Registered<Q, B, T>
{
    fn from(value: RawCreation<Q, T>) -> Self {
        let replay = value.replay;
        Self {
            kind: value.kind,
            principal: None,
            replay: None,
            identity: None,
            command: None,
            resume: None,
            raw: Some(RawRegistered {
                input: value.input,
                replay: Arc::new(move |ctx, query, input, created| {
                    Box::pin(async move {
                        let value = replay(ctx, query, input, created).await?;
                        crate::routes::workspace::json_reply(StatusCode::OK, &value)
                    })
                }),
            }),
        }
    }
}
