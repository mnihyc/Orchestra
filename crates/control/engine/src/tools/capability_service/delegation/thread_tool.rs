//! The actual image-tool producer owns its accepted-result sink and publication guard.
use super::*;

pub(super) async fn publish(
    engine: &Engine,
    call: &CallContext,
    parent: &ParentLink,
    record: &Record,
    receipt: &codecs::Receipt,
) -> Result<(), EngineError> {
    let images = receipt
        .result
        .items
        .iter()
        .map(|item| {
            if item.family != "image" {
                return Err(EngineError::ToolCallIdentity);
            }
            let image: accepted::NativeImage = serde_json::from_value(item.payload.clone())?;
            if item.native_ref.as_ref() != Some(&image.native_ref) {
                return Err(EngineError::ToolCallIdentity);
            }
            Ok(image)
        })
        .collect::<Result<Vec<_>, EngineError>>()?;
    let details = json!({"helper":record.helper_info(),"images":images});
    let accepted = accepted::capture_native(
        call,
        &record.capability,
        receipt.command,
        parent.clone(),
        &record.input,
        images,
        details,
    )?;
    engine
        .retain_native_capability_result(call, &accepted)
        .await
}
