//! Pure, declared inline image parameters; no source lookup, HTTP client, credentials or authority.
use base64::Engine;
use orch_provider_contract::{
    SourceError, SourceErrorCode,
    capabilities::{Binding, render_request},
};
use orch_types::Json;
use serde_json::json;
use std::collections::BTreeSet;

/// Actual bytes already read and verified by the consumer's native source boundary.
pub struct InputImage {
    /// A request-local label, never a local source path.
    pub name: String,
    /// Verified nonempty image bytes.
    pub bytes: Vec<u8>,
    /// Media type of those actual bytes.
    pub media_type: String,
}
impl std::fmt::Debug for InputImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputImage")
            .field("name", &self.name)
            .field("bytes", &self.bytes.len())
            .field("media_type", &self.media_type)
            .finish()
    }
}

fn refused(message: &str) -> SourceError {
    SourceError::new(SourceErrorCode::Refused, message)
}

/// Whether an installed binding declares this materializer, without enabling its offering.
#[must_use]
pub fn has_inline_inputs(binding: &Binding) -> bool {
    binding
        .extensions
        .get("x_input_images")
        .and_then(|value| value.get("encoding"))
        .and_then(Json::as_str)
        == Some("inline-json")
}

/// Materialize every supplied image into the selected declaration's JSON parameter.
/// The common renderer still owns the wire shape, and the shared keeper sends normal JSON.
pub fn materialize_inline_inputs(
    binding: &Binding,
    mut input: Json,
    images: &[InputImage],
    maximum_bytes: usize,
) -> Result<Json, SourceError> {
    if !has_inline_inputs(binding) || images.is_empty() {
        return Err(refused(
            "this image binding declares no inline image inputs",
        ));
    }
    let descriptor = &binding.extensions["x_input_images"];
    let field = descriptor
        .get("field")
        .and_then(Json::as_str)
        .filter(|field| !field.is_empty() && !matches!(*field, "model" | "prompt" | "n"))
        .ok_or_else(|| refused("the inline image parameter is not declared"))?;
    let template = descriptor
        .get("item")
        .ok_or_else(|| refused("the inline image item is not declared"))?;
    let object = input
        .as_object_mut()
        .ok_or_else(|| refused("inline image inputs require object parameters"))?;
    if object.contains_key(field) {
        return Err(refused(
            "an inline image parameter cannot override supplied data",
        ));
    }
    let template = Binding {
        request: json!({"body":template}),
        ..Binding::default()
    };
    let mut seen = BTreeSet::new();
    let mut used = serde_json::to_vec(object)
        .map_err(|_| refused("image parameters could not be encoded"))?
        .len();
    let mut rows = Vec::with_capacity(images.len());
    for image in images {
        if image.bytes.is_empty()
            || !image.media_type.starts_with("image/")
            || image.name.is_empty()
            || !seen.insert(&image.name)
        {
            return Err(refused(
                "every inline input needs distinct source-bound image bytes",
            ));
        }
        let encoded = image
            .bytes
            .len()
            .checked_add(2)
            .and_then(|size| size.checked_div(3))
            .and_then(|size| size.checked_mul(4))
            .and_then(|size| size.checked_add(image.media_type.len() + "data:;base64,".len()))
            .ok_or_else(|| refused("inline image input exceeds the request budget"))?;
        if used
            .checked_add(encoded)
            .is_none_or(|size| size > maximum_bytes)
        {
            return Err(refused("inline image input exceeds the request budget"));
        }
        let url = format!(
            "data:{};base64,{}",
            image.media_type,
            base64::engine::general_purpose::STANDARD.encode(&image.bytes)
        );
        let rendered = render_request(
            &template,
            &json!({"url":url,"media_type":image.media_type,"name":image.name}),
        )?;
        let row = rendered
            .get("body")
            .cloned()
            .ok_or_else(|| refused("the inline image item did not render"))?;
        used = used
            .checked_add(
                serde_json::to_vec(&row)
                    .map_err(|_| refused("the inline image item could not be encoded"))?
                    .len(),
            )
            .ok_or_else(|| refused("inline image input exceeds the request budget"))?;
        if used > maximum_bytes {
            return Err(refused("inline image input exceeds the request budget"));
        }
        rows.push(row);
    }
    object.insert(field.into(), Json::Array(rows));
    if serde_json::to_vec(&input)
        .map_err(|_| refused("image parameters could not be encoded"))?
        .len()
        > maximum_bytes
    {
        return Err(refused("inline image input exceeds the request budget"));
    }
    let _ = render_request(binding, &input)?;
    Ok(input)
}
