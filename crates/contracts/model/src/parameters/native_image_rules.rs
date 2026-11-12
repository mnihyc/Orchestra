//! A stored closed native capability remains closed on the final wire request.
use super::*;

#[test]
fn a_closed_native_image_binding_refuses_the_final_request_before_any_transport()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    const PROTOCOL: &str = "openai-responses";
    let interfaces = orch_provider_contract::capabilities::protocol_interfaces()?;
    let binding = interfaces
        .get(PROTOCOL)
        .and_then(|interface| interface.native_tools.get("image_generation"))
        .ok_or("the actual image binding is absent")?
        .clone();
    let input = json!({
        "model":"fixture-image-model",
        "input":"A blue square",
        "stream":false,
        "service_tier":"default",
        "tools":[{"type":"image_generation"}]
    });
    let bytes = serde_json::to_vec(&input)?;
    let mut model = ModelParameters::default();
    model.native_tools.insert(
        PROTOCOL.into(),
        [("image_generation".into(), Some(binding))].into(),
    );
    let permitted = outgoing_at(PROTOCOL, "/responses", &model, &bytes)?;
    assert_eq!(serde_json::from_slice::<Json>(&permitted)?, input);
    model
        .native_tools
        .insert(PROTOCOL.into(), [("image_generation".into(), None)].into());
    assert!(outgoing_at(PROTOCOL, "/responses", &model, &bytes).is_err());
    let mut ordinary = input.clone();
    ordinary
        .as_object_mut()
        .ok_or("the request is an object")?
        .remove("tools");
    let ordinary_bytes = serde_json::to_vec(&ordinary)?;
    assert_eq!(
        serde_json::from_slice::<Json>(&outgoing_at(
            PROTOCOL,
            "/responses",
            &model,
            &ordinary_bytes
        )?)?,
        ordinary,
        "closing a hosted image tool keeps ordinary model generation available"
    );
    ordinary["tools"] =
        json!([{"type":"function","name":"image_generation","parameters":{"type":"object"}}]);
    let function_bytes = serde_json::to_vec(&ordinary)?;
    assert_eq!(
        serde_json::from_slice::<Json>(&outgoing_at(
            PROTOCOL,
            "/responses",
            &model,
            &function_bytes
        )?)?,
        ordinary,
        "a client function with that name is not the declared hosted tool"
    );
    Ok(())
}
