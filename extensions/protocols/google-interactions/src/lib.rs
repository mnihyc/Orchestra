//! Native stateless Gemini Interactions, including Google Search beside the caller's functions.

use bytes::Bytes;
use http::{Method, Response, StatusCode};
use orch_agent_contract::discovery::ModelSuggestion;
use orch_model_contract::credentials::WireError;
use orch_model_contract::endpoint::AuthMode;
use orch_model_contract::generation::{
    GenerationInput, GenerationMessage, GenerationPart, GenerationRole, GenerationStream,
};
use orch_model_contract::protocol::WireProtocol;
use orch_model_contract::usage::UsageObserver;
use orch_types::Json;
use serde_json::json;

mod generation;
#[cfg(test)]
mod tests;
mod usage;

/// Google's native Interactions interface. History is always supplied by Orch with storage off.
#[derive(Debug, Clone, Copy, Default)]
pub struct GoogleInteractions;

impl GoogleInteractions {
    /// The model listing exposed to a client contains exactly its current lease's models.
    #[must_use]
    pub fn models_response(&self, names: &[String]) -> Response<Bytes> {
        Response::new(Bytes::from(
            json!({"models":names.iter().map(|name| json!({"name":name})).collect::<Vec<_>>()} )
                .to_string(),
        ))
    }
}

impl WireProtocol for GoogleInteractions {
    fn name(&self) -> &'static str {
        orch_model_contract::protocol::GOOGLE_INTERACTIONS
    }
    fn allowed(&self, method: &Method, path: &str) -> bool {
        matches!(
            (method.as_str(), path.split('?').next().unwrap_or(path)),
            ("POST", "/interactions") | ("GET", "/models")
        )
    }
    fn model_of(&self, _path: &str, body: &[u8]) -> Option<String> {
        serde_json::from_slice::<Json>(body)
            .ok()?
            .get("model")?
            .as_str()
            .map(str::to_owned)
    }
    fn default_auth(&self) -> AuthMode {
        AuthMode::GoogleApiKey
    }
    fn deny(&self, status: StatusCode, code: &str, message: &str) -> Response<Bytes> {
        let mut response = Response::new(Bytes::from(
            json!({"error":{"code":status.as_u16(),"status":code,"message":message}}).to_string(),
        ));
        *response.status_mut() = status;
        response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        response
    }
    fn usage_observer(&self) -> Box<dyn UsageObserver> {
        Box::new(usage::GoogleUsage::default())
    }
    fn models_request(&self) -> Option<(Method, &'static str)> {
        Some((Method::GET, "/models"))
    }
    fn parse_models(&self, body: &[u8]) -> Vec<ModelSuggestion> {
        let Ok(reply) = serde_json::from_slice::<Json>(body) else {
            return Vec::new();
        };
        reply
            .get("models")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| {
                let name = model.get("name")?.as_str()?.strip_prefix("models/")?;
                let mut suggestion =
                    ModelSuggestion::named(name, model.get("displayName").and_then(Json::as_str));
                suggestion.parameters.context_window = model
                    .get("inputTokenLimit")
                    .and_then(Json::as_i64)
                    .filter(|size| *size > 0);
                let output = model
                    .get("outputTokenLimit")
                    .and_then(Json::as_i64)
                    .filter(|size| *size > 0);
                if output.is_some() {
                    suggestion.parameters.offered =
                        Some(orch_provider_contract::models::ModelOffered {
                            report: Some(orch_provider_contract::models::ModelReport {
                                output_max: output,
                                ..Default::default()
                            }),
                            ..Default::default()
                        });
                }
                Some(suggestion)
            })
            .collect()
    }
    fn next_models_request(
        &self,
        _current: &str,
        body: &[u8],
    ) -> Result<Option<String>, WireError> {
        let reply: Json =
            serde_json::from_slice(body).map_err(|error| WireError::Internal(error.to_string()))?;
        Ok(reply
            .get("nextPageToken")
            .and_then(Json::as_str)
            .filter(|token| !token.is_empty())
            .map(|token| {
                let query = url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("pageToken", token)
                    .finish();
                format!("/models?{query}")
            }))
    }
    fn generation_request(
        &self,
        model: &str,
        input: &GenerationInput,
        stream: bool,
    ) -> Option<(Method, &'static str, Bytes)> {
        generation::request(model, input, stream)
            .map(|body| (Method::POST, "/interactions", Bytes::from(body.to_string())))
    }
    fn generation_output(
        &self,
        body: &[u8],
    ) -> Result<orch_model_contract::generation::GenerationOutput, WireError> {
        let reply: Json =
            serde_json::from_slice(body).map_err(|error| WireError::Internal(error.to_string()))?;
        generation::output(reply)
    }
    fn generation_stream(&self) -> Option<Box<dyn GenerationStream>> {
        Some(Box::new(generation::GoogleStream::default()))
    }
    fn completion_request(
        &self,
        model: &str,
        system: &str,
        input: &str,
        max_output_tokens: i64,
    ) -> Option<(Method, &'static str, Bytes)> {
        self.generation_request(
            model,
            &GenerationInput {
                system: system.into(),
                messages: vec![GenerationMessage {
                    role: GenerationRole::User,
                    content: vec![GenerationPart::Text { text: input.into() }],
                    continuation: Vec::new(),
                }],
                tools: Vec::new(),
                max_output_tokens,
            },
            false,
        )
    }
    fn completion_text(&self, body: &[u8]) -> Result<String, WireError> {
        let answer = self.generation_output(body)?;
        if answer.stop != orch_model_contract::generation::GenerationStop::Complete {
            return Err(WireError::Internal(
                "Gemini did not complete its text answer".into(),
            ));
        }
        Ok(answer
            .message
            .content
            .iter()
            .filter_map(|part| match part {
                GenerationPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect())
    }
}
