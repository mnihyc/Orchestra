//! Image wire declarations plug into the common registry. This codec performs no provider IO,
//! authorization, file write, price calculation or ledger insertion, and installs no executor.
#![forbid(unsafe_code)]
mod google;
mod input;
mod openai;
mod openrouter;
mod stream;

pub use input::{InputImage, has_inline_inputs, materialize_inline_inputs};
pub use stream::{ImageEvent, ImageStream};

use base64::Engine;
use orch_provider_contract::capabilities::{Binding, Declarations, project_result};
use orch_provider_contract::{SourceError, SourceErrorCode};
use orch_types::Json;

/// Capability-only extensions; absence of account facts does not require a fake facts source.
pub fn declarations() -> Result<Vec<(&'static str, Declarations)>, serde_json::Error> {
    Ok(vec![
        (
            "images.interfaces",
            serde_json::from_str(include_str!("interfaces.json"))?,
        ),
        ("images.openai", openai::declarations()?),
        ("images.google", google::declarations()?),
        ("images.openrouter", openrouter::declarations()?),
    ])
}

/// Existing invocation/output limits supplied by the admitted caller, never guessed from a prompt.
#[derive(Debug, Clone, Copy)]
pub struct DecodeLimits {
    /// Maximum completed images retained by this one call.
    pub images: usize,
    /// Maximum decoded bytes in an image.
    pub image_bytes: usize,
    /// Maximum decoded bytes across the call.
    pub total_bytes: usize,
}

/// Actual bytes for the existing blob and destination writer; this is not a delivered-file receipt.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    /// Original row order, including distinct identical outputs.
    pub ordinal: usize,
    /// Decoded provider output, never fetched from an arbitrary returned URL.
    pub bytes: Vec<u8>,
    /// Reported image media type, absent where the backend did not identify it.
    pub media_type: Option<String>,
    /// Provider's optional native image-operation identity, not authority evidence.
    pub native_ref: Option<String>,
    /// Provider's actual revised prompt, where reported.
    pub revised_prompt: Option<String>,
}
impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("ordinal", &self.ordinal)
            .field("bytes", &self.bytes.len())
            .field("media_type", &self.media_type)
            .finish_non_exhaustive()
    }
}

/// Output evidence for the original producer. Its ordinary writer owns blobs, delivery and usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Images {
    /// Actual image bytes; ImageEvent distinguishes a preview from the final binding.
    pub images: Vec<Image>,
    /// Unchanged actual usage object; missing stays null rather than a measured zero or price.
    pub usage: Json,
}

fn malformed(message: &str) -> SourceError {
    SourceError::new(SourceErrorCode::Malformed, message)
}

/// Decode a selected binding's normalized images. All shape selection uses the common projector.
/// A failure never refunds/erases already observed provider work; its caller retains that response.
pub fn decode(
    binding: &Binding,
    input: &Json,
    response: &Json,
    limits: DecodeLimits,
) -> Result<Images, SourceError> {
    let projected = project_result(binding, input, response, limits.images.saturating_add(1))?;
    let reported_format = if let Some(template) = binding.result.get("format") {
        let format = Binding {
            result: serde_json::json!({"page":template}),
            ..Default::default()
        };
        project_result(&format, input, response, 1)?
    } else {
        Json::Null
    };
    let images = decode_rows(
        &projected,
        limits,
        &reported_format,
        &binding.result["media_types"],
    )?;
    // `usage` is a declaration template, interpreted by the same projector, never a second meter.
    let usage = if let Some(template) = binding.result.get("usage") {
        let usage = Binding {
            result: serde_json::json!({"page":template}),
            ..Default::default()
        };
        project_result(&usage, input, response, 1)?
    } else {
        Json::Null
    };
    Ok(Images { images, usage })
}

/// Decode the common service runner's already projected final image rows.
/// Missing media type stays unknown; the native file writer may inspect actual bytes.
/// This grants no result custody, provider authority, billing or destination permission.
pub fn decode_projected(projected: &Json, limits: DecodeLimits) -> Result<Vec<Image>, SourceError> {
    decode_rows(projected, limits, &Json::Null, &Json::Null)
}

fn decode_rows(
    projected: &Json,
    limits: DecodeLimits,
    reported_format: &Json,
    media_types: &Json,
) -> Result<Vec<Image>, SourceError> {
    let rows = projected
        .as_array()
        .ok_or_else(|| malformed("the image binding returns an array"))?;
    if rows.is_empty() || rows.len() > limits.images {
        return Err(malformed(
            "the completed image count is outside the admitted output limit",
        ));
    }
    let mut total = 0usize;
    let mut images = Vec::with_capacity(rows.len());
    for (ordinal, row) in rows.iter().enumerate() {
        let encoded = row
            .get("b64_json")
            .and_then(Json::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| malformed("a completed image lacks encoded bytes"))?;
        // Standard padded base64 is the declared wire encoding. Check its exact decoded bound
        // before allocation; the decoder then rejects invalid padding/alphabet/trailing bits.
        if !encoded.len().is_multiple_of(4) {
            return Err(malformed("the image encoding is incomplete"));
        }
        let padding = encoded
            .bytes()
            .rev()
            .take_while(|byte| *byte == b'=')
            .count();
        let bytes = (encoded.len() / 4)
            .checked_mul(3)
            .and_then(|bytes| bytes.checked_sub(padding))
            .ok_or_else(|| malformed("the image encoding length is invalid"))?;
        total = total
            .checked_add(bytes)
            .ok_or_else(|| malformed("the image output is too large"))?;
        if padding > 2 || bytes == 0 || bytes > limits.image_bytes || total > limits.total_bytes {
            return Err(malformed(
                "the image bytes exceed the admitted output limit",
            ));
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| malformed("the image bytes are not valid base64"))?;
        let media_type = row
            .get("media_type")
            .filter(|value| !value.is_null())
            .map(|value| {
                let value = value
                    .as_str()
                    .filter(|value| {
                        value.starts_with("image/")
                            && value.len() > "image/".len()
                            && value.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || b"/-+._".contains(&byte)
                            })
                    })
                    .ok_or_else(|| malformed("the image reply has an invalid media type"))?;
                Ok::<_, SourceError>(value.to_owned())
            })
            .transpose()?;
        let format_type = row
            .get("format")
            .filter(|format| !format.is_null())
            .unwrap_or(reported_format)
            .as_str()
            .and_then(|format| media_types.get(format)?.as_str())
            .map(str::to_owned);
        let media_type = media_type.or(format_type);
        if media_type.as_deref().is_some_and(|value| {
            !value.starts_with("image/")
                || value.len() <= "image/".len()
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"/-+._".contains(&byte))
        }) {
            return Err(malformed(
                "the image format mapping has an invalid media type",
            ));
        }
        let optional_text = |key: &str| -> Result<Option<String>, SourceError> {
            row.get(key)
                .filter(|value| !value.is_null())
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| malformed("an image metadata member is not text"))
                })
                .transpose()
        };
        images.push(Image {
            ordinal,
            bytes: decoded,
            media_type,
            native_ref: optional_text("native_ref")?,
            revised_prompt: optional_text("revised_prompt")?,
        });
    }
    Ok(images)
}

#[cfg(test)]
mod tests;
