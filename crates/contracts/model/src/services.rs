//! Project captured native generations through the shared provider capability codec.
use orch_provider_contract::{SourceError, SourceErrorCode};
use orch_types::Json;

/// Canonical credential-free endpoint metadata shared by an auxiliary issuer and its keeping runtime.
pub fn generation_endpoint(
    protocol: &str,
    address: &str,
    options: &Json,
) -> Result<orch_provider_contract::capabilities::Endpoint, SourceError> {
    let address = url::Url::parse(address).map_err(|_| {
        SourceError::new(
            SourceErrorCode::Unsupported,
            "the generation endpoint address is invalid",
        )
    })?;
    let options = crate::endpoint::EndpointOptions::parse(options).map_err(|_| {
        SourceError::new(
            SourceErrorCode::Unsupported,
            "the generation endpoint options are invalid",
        )
    })?;
    Ok(orch_provider_contract::capabilities::Endpoint {
        interface: protocol.into(),
        address: address.to_string(),
        credential_role: (options.auth != Some(crate::endpoint::AuthMode::None))
            .then(|| "model".into()),
        options: serde_json::to_value(options).map_err(|_| {
            SourceError::new(
                SourceErrorCode::Unsupported,
                "the generation endpoint options could not be read",
            )
        })?,
        ..Default::default()
    })
}

/// Feed captured model results through the same declared codec as an API result.
/// A declaration may explicitly select the full envelope to read observed stream objects.
pub fn project_model_result(
    binding: &orch_provider_contract::capabilities::Binding,
    input: &Json,
    output: &crate::generation::GenerationOutput,
    limit: usize,
) -> Result<Json, SourceError> {
    if output.stop != crate::generation::GenerationStop::Complete {
        return Err(SourceError::new(
            SourceErrorCode::Unavailable,
            "the auxiliary model did not complete this capability request",
        ));
    }
    let envelope = &output.native_result;
    let response = envelope.get("response").ok_or_else(|| {
        SourceError::new(
            SourceErrorCode::Unsupported,
            "this generation interface did not report native results",
        )
    })?;
    let selected = if binding
        .extensions
        .get("auxiliary")
        .and_then(|auxiliary| auxiliary.get("result_envelope"))
        .and_then(Json::as_bool)
        == Some(true)
    {
        envelope
    } else {
        response
    };
    orch_provider_contract::capabilities::project_result(binding, input, selected, limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_same_result_codec_reads_native_responses_or_explicit_stream_envelopes_without_executing_them()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut output: crate::generation::GenerationOutput = serde_json::from_value(json!({
            "message":{"role":"assistant","content":[],"continuation":[]},"stop":"complete",
            "native_result":{"response":{"results":[{"title":"Final source","url":"https://final.example.invalid/"}]},"frames":[{"results":[{"title":"Stream source","url":"https://stream.example.invalid/"}]}]}
        }))?;
        let mut binding = orch_provider_contract::capabilities::Binding {
            result: json!({"collections":[{"pointer":"/results/*","item":{"title":{"from_result":"/title"},"url":{"from_result":"/url"}}}]}),
            ..orch_provider_contract::capabilities::Binding::default()
        };
        assert_eq!(
            project_model_result(&binding, &json!({}), &output, 10)?[0]["url"],
            "https://final.example.invalid/"
        );
        binding
            .extensions
            .insert("auxiliary".into(), json!({"result_envelope":true}));
        binding.result["collections"][0]["pointer"] = json!("/frames/*/results/*");
        assert_eq!(
            project_model_result(&binding, &json!({}), &output, 10)?[0]["url"],
            "https://stream.example.invalid/"
        );
        output.stop = crate::generation::GenerationStop::Limit;
        assert!(project_model_result(&binding, &json!({}), &output, 10).is_err());
        output.stop = crate::generation::GenerationStop::Complete;
        output.native_result = Json::Null;
        assert!(project_model_result(&binding, &json!({}), &output, 10).is_err());
        Ok(())
    }
}
