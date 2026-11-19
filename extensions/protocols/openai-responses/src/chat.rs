//! Chat Completions pass-through with shared OpenAI authorization, discovery and usage observation.
//!
//! Request and reply bytes are never translated to Responses. Only creation and the lease's model
//! list are served; stored-completion management and every other account API remain outside a lease.

use bytes::Bytes;
use http::{Method, Response, StatusCode};
use orch_agent_contract::discovery::ModelSuggestion;
use orch_model_contract::credentials::WireError;
use orch_model_contract::protocol::{
    OPENAI_CHAT, OPENAI_CHAT_REQUESTS, WireProtocol, is_allowed, model_in_body, next_models_page,
};
use orch_model_contract::usage::UsageObserver;

/// The Chat Completions format, served independently of an agent's declared format support.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAiChat;

impl OpenAiChat {
    /// A lease's visible models in the OpenAI model-list shape, without querying the upstream.
    #[must_use]
    pub fn models_response(&self, names: &[String]) -> Response<Bytes> {
        crate::models::list_response(names)
    }
}

impl WireProtocol for OpenAiChat {
    fn name(&self) -> &'static str {
        OPENAI_CHAT
    }

    fn allowed(&self, method: &Method, path: &str) -> bool {
        is_allowed(&OPENAI_CHAT_REQUESTS, method, path)
    }

    fn model_of(&self, _: &str, body: &[u8]) -> Option<String> {
        model_in_body(body)
    }

    fn deny(&self, status: StatusCode, code: &str, message: &str) -> Response<Bytes> {
        crate::authorization::deny(status, code, message)
    }

    fn usage_observer(&self) -> Box<dyn UsageObserver> {
        Box::new(crate::usage::ResponsesUsage::chat())
    }

    fn models_request(&self) -> Option<(Method, &'static str)> {
        Some(crate::models::request())
    }

    fn parse_models(&self, body: &[u8]) -> Vec<ModelSuggestion> {
        crate::models::parse(body)
    }

    fn next_models_request(&self, current: &str, body: &[u8]) -> Result<Option<String>, WireError> {
        next_models_page(current, body, "after")
    }

    fn completion_request(
        &self,
        model: &str,
        system: &str,
        input: &str,
        max_output_tokens: i64,
    ) -> Option<(Method, &'static str, Bytes)> {
        Some((Method::POST, "/chat/completions", Bytes::from(serde_json::json!({
            "model": model, "messages": [{"role":"system","content":system},{"role":"user","content":input}],
            "max_tokens": max_output_tokens, "stream": false,
        }).to_string())))
    }

    fn completion_text(&self, body: &[u8]) -> Result<String, WireError> {
        let bad =
            || WireError::Internal("the response contains no completed text answer".to_owned());
        let reply: serde_json::Value = serde_json::from_slice(body).map_err(|_| bad())?;
        if reply.get("error").is_some_and(|error| !error.is_null()) {
            return Err(bad());
        }
        let choice = reply
            .get("choices")
            .and_then(serde_json::Value::as_array)
            .and_then(|choices| choices.first())
            .ok_or_else(bad)?;
        if choice
            .get("finish_reason")
            .and_then(serde_json::Value::as_str)
            != Some("stop")
        {
            return Err(bad());
        }
        choice
            .pointer("/message/content")
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
            .ok_or_else(bad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_lease_allows_creation_and_models_but_no_stored_state_or_other_api() {
        let protocol = OpenAiChat;
        assert!(protocol.allowed(&Method::POST, "/chat/completions?api-version=1"));
        assert!(protocol.allowed(&Method::GET, "/models"));
        for (method, path) in [
            (Method::GET, "/chat/completions"),
            (Method::DELETE, "/chat/completions/one"),
            (Method::POST, "/responses"),
            (Method::POST, "/files"),
            (Method::POST, "/chat/completions/"),
            (Method::POST, "/chat/../chat/completions"),
            (Method::POST, "/chat/complet%69ons"),
        ] {
            assert!(!protocol.allowed(&method, path), "{method} {path}");
        }
        assert_eq!(
            protocol
                .model_of("/chat/completions", br#"{"model":" Local/Model "}"#)
                .as_deref(),
            Some(" Local/Model ")
        );
    }

    #[test]
    fn a_chat_completion_uses_its_own_messages_and_reads_only_a_completed_text_answer() {
        let protocol = OpenAiChat;
        let (_, path, body) = protocol
            .completion_request("local", "Summarize", "Hello", 64)
            .unwrap();
        assert_eq!(path, "/chat/completions");
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["messages"][0]["content"], "Summarize");
        assert_eq!(body["stream"], false);
        assert_eq!(protocol.completion_text(br#"{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"Hello"}}]}"#).unwrap(),"Hello");
        assert!(
            protocol
                .completion_text(
                    br#"{"choices":[{"finish_reason":"length","message":{"content":"cut"}}]}"#
                )
                .is_err()
        );
    }
}
