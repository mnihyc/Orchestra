//! Declared multipart uses the same source-bound content reads and HTTP request budget.
use super::*;
use orch_model_contract::generation::GenerationPart;

fn refused(detail: &str) -> CommandError {
    CommandError::new(ErrorCode::Conflict, detail)
}
fn field(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !b"\"\\".contains(&byte))
}
impl Forwarder {
    pub(super) async fn materialize_service_body(
        &self,
        rendered: &Value,
        images: &[GenerationPart],
        command: Id,
    ) -> Result<(Vec<u8>, HeaderValue), CommandError> {
        match rendered
            .get("encoding")
            .and_then(Value::as_str)
            .unwrap_or("json")
        {
            "json" => {
                if !images.is_empty() {
                    return Err(refused(
                        "the declared JSON request has no input-image materializer",
                    ));
                }
                let body = rendered
                    .get("body")
                    .filter(|body| !body.is_null())
                    .map(serde_json::to_vec)
                    .transpose()
                    .map_err(|_| refused("the service body could not be encoded"))?
                    .unwrap_or_default();
                Ok((body, HeaderValue::from_static("application/json")))
            }
            "multipart" => {
                let values = rendered
                    .get("body")
                    .and_then(Value::as_object)
                    .ok_or_else(|| refused("a declared multipart body must be an object"))?;
                let mut parts = BTreeMap::new();
                let mut bytes = 0u64;
                for image in images {
                    let GenerationPart::ImageRef {
                        blob,
                        size,
                        media_type,
                        name,
                        ..
                    } = image
                    else {
                        return Err(refused(
                            "multipart inputs require actual source-bound image references",
                        ));
                    };
                    if *size <= 0
                        || !field(name)
                        || !field(media_type)
                        || !media_type.starts_with("image/")
                    {
                        return Err(refused("a multipart image reference is invalid"));
                    }
                    bytes = bytes
                        .checked_add(*size as u64)
                        .ok_or_else(|| refused("the multipart input is too large"))?;
                    if bytes > self.max_request_bytes {
                        return Err(CommandError::new(
                            ErrorCode::TooLarge,
                            "the multipart inputs exceed the request budget",
                        ));
                    }
                    let path = self
                        .services
                        .content
                        .fetch(command, *blob, *size, name)
                        .await?;
                    let content = tokio::fs::read(path)
                        .await
                        .map_err(|_| refused("a required multipart image is unavailable"))?;
                    if content.len() as i64 != *size
                        || orch_types::Hash::of(&content) != *blob
                        || parts
                            .insert(name.as_str(), (blob, media_type, content))
                            .is_some()
                    {
                        return Err(refused(
                            "a required multipart image changed or has a duplicate name",
                        ));
                    }
                }
                let boundary = format!("orch-{}", orch_types::new_id());
                let mut body = Vec::new();
                let mut used = std::collections::BTreeSet::new();
                for (name, value) in values {
                    if value.is_null() {
                        continue;
                    }
                    if !field(name) {
                        return Err(refused("a multipart field name is invalid"));
                    }
                    let array_name = rendered
                        .pointer("/array_fields")
                        .and_then(|fields| fields.get(name))
                        .and_then(Value::as_str);
                    let entries: Vec<&Value> = if array_name.is_some() {
                        value
                            .as_array()
                            .ok_or_else(|| refused("the multipart array field is invalid"))?
                            .iter()
                            .collect()
                    } else {
                        vec![value]
                    };
                    let sent_name = array_name.unwrap_or(name);
                    if !field(sent_name) {
                        return Err(refused("a multipart array field name is invalid"));
                    }
                    for value in entries {
                        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{sent_name}\"").as_bytes());
                        if let Some((blob, media, content)) =
                            value.as_str().and_then(|name| parts.get(name))
                        {
                            used.insert(value.as_str().unwrap_or_default());
                            body.extend_from_slice(
                                format!(
                                    "; filename=\"{blob}.bin\"\r\nContent-Type: {media}\r\n\r\n"
                                )
                                .as_bytes(),
                            );
                            body.extend_from_slice(content);
                        } else {
                            body.extend_from_slice(b"\r\n\r\n");
                            let text = value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| value.to_string());
                            body.extend_from_slice(text.as_bytes());
                        }
                        body.extend_from_slice(b"\r\n");
                        if body.len() as u64 > self.max_request_bytes {
                            return Err(CommandError::new(
                                ErrorCode::TooLarge,
                                "the multipart body exceeds the request budget",
                            ));
                        }
                    }
                }
                if used.len() != parts.len() || parts.is_empty() {
                    return Err(refused(
                        "the declared multipart request must consume every prepared input image",
                    ));
                }
                body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
                Ok((
                    body,
                    HeaderValue::from_str(&format!("multipart/form-data; boundary={boundary}"))
                        .map_err(|_| refused("the multipart boundary is invalid"))?,
                ))
            }
            _ => Err(CommandError::new(
                ErrorCode::Unsupported,
                "the declared service encoding is unavailable",
            )),
        }
    }
}
