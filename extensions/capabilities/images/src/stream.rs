//! An invocation's decoded stream state. Durable acceptance, output blobs and billing stay with its caller.
use super::{DecodeLimits, Images, decode, malformed};
use orch_provider_contract::capabilities::Binding;
use orch_provider_contract::{SourceError, SourceErrorCode};
use orch_types::Json;
use serde_json::json;

/// A preview is never a completed image or another billable generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageEvent {
    /// Actual intermediate bytes, with a native preview index only where reported.
    Preview { index: Option<u64>, images: Images },
    /// The single completed result. The caller retains it before optional file delivery.
    Completed(Images),
}

/// Call-local once-only decoding; this is not a durable receipt, authority cache or ledger.
pub struct ImageStream {
    binding: Binding,
    input: Json,
    limits: DecodeLimits,
    completed: Option<Json>,
    failed: bool,
}

impl ImageStream {
    /// Keep this invocation's already selected binding, input and admitted output bounds.
    #[must_use]
    pub fn new(binding: Binding, input: Json, limits: DecodeLimits) -> Self {
        Self {
            binding,
            input,
            limits,
            completed: None,
            failed: false,
        }
    }

    /// The transport supplies one parsed SSE data object. Unknown progress events emit no image.
    /// Repeated identical final evidence emits nothing; another final cannot replace accepted work.
    pub fn accept(&mut self, event: Json) -> Result<Option<ImageEvent>, SourceError> {
        let result = self.advance(event);
        if result.is_err() && self.completed.is_none() {
            self.failed = true;
        }
        result
    }

    fn advance(&mut self, event: Json) -> Result<Option<ImageEvent>, SourceError> {
        if let Some(completed) = &self.completed {
            return if completed == &event {
                Ok(None)
            } else {
                Err(malformed(
                    "the image stream changed after its completed result",
                ))
            };
        }
        if self.failed {
            return Err(malformed("the image stream already failed"));
        }
        let declaration = self
            .binding
            .extensions
            .get("x_stream")
            .ok_or_else(|| malformed("the selected image binding has no stream declaration"))?;
        let Some(kind) = event.get("type").and_then(Json::as_str) else {
            return Ok(None);
        };
        let selected = ["partial", "completed", "error"]
            .into_iter()
            .find_map(|role| {
                declaration
                    .get(role)
                    .filter(|rule| rule.get("type").and_then(Json::as_str) == Some(kind))
                    .map(|rule| (role, rule))
            });
        let Some((role, rule)) = selected else {
            return Ok(None);
        };
        if role == "error" {
            return Err(SourceError::new(
                SourceErrorCode::Refused,
                "the image backend reported a failed generation",
            ));
        }
        let mut event_binding = self.binding.clone();
        let payload = if let Some(pointer) = rule.get("response").and_then(Json::as_str) {
            event
                .pointer(pointer)
                .ok_or_else(|| malformed("the final image stream event lacks its response"))?
        } else {
            let pointer = rule
                .get("bytes")
                .and_then(Json::as_str)
                .ok_or_else(|| malformed("the image stream declares its encoded-byte source"))?;
            let media = rule
                .get("media_type")
                .and_then(Json::as_str)
                .unwrap_or("/media_type");
            event_binding.result = json!({"page":[{
                "b64_json":{"from_response":pointer},
                "media_type":{"from_response":media,"optional":true}
            }]});
            if role == "completed"
                && let Some(pointer) = rule.get("usage").and_then(Json::as_str)
            {
                event_binding.result["usage"] = json!({"from_response":pointer,"optional":true});
            }
            &event
        };
        let images = decode(&event_binding, &self.input, payload, self.limits)?;
        if role == "completed" {
            self.completed = Some(event);
            return Ok(Some(ImageEvent::Completed(images)));
        }
        let index = rule
            .get("index")
            .and_then(Json::as_str)
            .and_then(|pointer| event.pointer(pointer))
            .map(|value| {
                value
                    .as_u64()
                    .ok_or_else(|| malformed("the native image preview index is invalid"))
            })
            .transpose()?;
        Ok(Some(ImageEvent::Preview { index, images }))
    }

    /// Close this invocation at transport EOF or the terminal sentinel, after all events.
    /// Missing final evidence is terminal; later frames cannot resurrect that invocation.
    /// Already accepted output remains complete even after a trailing transport failure.
    pub fn finish(&mut self) -> Result<(), SourceError> {
        if self.completed.is_some() {
            Ok(())
        } else {
            self.failed = true;
            Err(malformed(
                "the image stream ended without a completed result",
            ))
        }
    }
}
