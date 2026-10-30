//! Embeddings pass-through and the request that turns texts into vectors, with shared OpenAI authorization, discovery and usage.
//!
//! The Embeddings format is the one most servers of embedding models speak (OpenAI's own, and local servers
//! such as llama.cpp's). Orch uses it for its own requests: a batch of texts goes out with
//! `encoding_format: "float"`, and the answer's `data` is read back by each item's `index`, so that a server that
//! answers out of order still gives each text its own vector. Only creation and the model list pass a lease.

use bytes::Bytes;
use http::{Method, Response, StatusCode};
use orch_agent_contract::discovery::ModelSuggestion;
use orch_model_contract::credentials::WireError;
use orch_model_contract::protocol::{
    OPENAI_EMBEDDINGS, OPENAI_EMBEDDINGS_REQUESTS, WireProtocol, is_allowed, model_in_body,
    next_models_page,
};
use orch_model_contract::usage::UsageObserver;
use serde::Deserialize;

/// The Embeddings format of OpenAI.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAiEmbeddings;

impl OpenAiEmbeddings {
    /// A lease's visible models in the OpenAI model-list shape, without querying the upstream.
    #[must_use]
    pub fn models_response(&self, names: &[String]) -> Response<Bytes> {
        crate::models::list_response(names)
    }
}

/// One item of an answer's `data`.
#[derive(Deserialize)]
struct Item {
    index: Option<usize>,
    embedding: Vec<f32>,
}

/// The members of an answer that matter here.
#[derive(Deserialize)]
struct Answer {
    data: Vec<Item>,
}

impl WireProtocol for OpenAiEmbeddings {
    fn name(&self) -> &'static str {
        OPENAI_EMBEDDINGS
    }

    fn allowed(&self, method: &Method, path: &str) -> bool {
        is_allowed(&OPENAI_EMBEDDINGS_REQUESTS, method, path)
    }

    fn model_of(&self, _: &str, body: &[u8]) -> Option<String> {
        model_in_body(body)
    }

    fn deny(&self, status: StatusCode, code: &str, message: &str) -> Response<Bytes> {
        crate::authorization::deny(status, code, message)
    }

    fn usage_observer(&self) -> Box<dyn UsageObserver> {
        // The answer is one JSON document whose `usage` has `prompt_tokens` and `total_tokens`, as a plain
        // Chat Completions answer has.
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

    fn embedding_request(
        &self,
        model: &str,
        inputs: &[String],
        dimensions: Option<u32>,
    ) -> Option<(Method, &'static str, Bytes)> {
        let mut body = serde_json::json!({
            "model": model, "input": inputs, "encoding_format": "float",
        });
        if let Some(dimensions) = dimensions {
            body["dimensions"] = serde_json::json!(dimensions);
        }
        Some((Method::POST, "/embeddings", Bytes::from(body.to_string())))
    }

    fn embedding_vectors(&self, body: &[u8], inputs: usize) -> Result<Vec<Vec<f32>>, WireError> {
        let bad = |why: &str| WireError::Internal(format!("the embeddings answer {why}"));
        let answer: Answer =
            serde_json::from_slice(body).map_err(|_| bad("is not a list of vectors"))?;
        if answer.data.len() != inputs {
            return Err(bad("has another number of vectors than texts were sent"));
        }
        let mut vectors: Vec<Option<Vec<f32>>> = vec![None; inputs];
        for (position, item) in answer.data.into_iter().enumerate() {
            let at = item.index.unwrap_or(position);
            let slot = vectors
                .get_mut(at)
                .ok_or_else(|| bad("names a text that was not sent"))?;
            if slot.is_some() {
                return Err(bad("gives one text two vectors"));
            }
            if item.embedding.is_empty() || item.embedding.iter().any(|value| !value.is_finite()) {
                return Err(bad("has a vector without numbers"));
            }
            *slot = Some(item.embedding);
        }
        let vectors: Vec<Vec<f32>> = vectors.into_iter().flatten().collect();
        let length = vectors.first().map_or(0, Vec::len);
        if vectors.len() != inputs || vectors.iter().any(|vector| vector.len() != length) {
            return Err(bad("has vectors of different lengths"));
        }
        Ok(vectors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_embeddings_lease_allows_creation_and_models_and_nothing_else() {
        let protocol = OpenAiEmbeddings;
        assert!(protocol.allowed(&Method::POST, "/embeddings"));
        assert!(protocol.allowed(&Method::GET, "/models"));
        for (method, path) in [
            (Method::GET, "/embeddings"),
            (Method::POST, "/chat/completions"),
            (Method::POST, "/responses"),
            (Method::POST, "/embeddings/"),
        ] {
            assert!(!protocol.allowed(&method, path), "{method} {path}");
        }
        assert_eq!(
            protocol
                .model_of("/embeddings", br#"{"model":"local","input":["x"]}"#)
                .as_deref(),
            Some("local")
        );
    }

    #[test]
    fn a_batch_asks_for_numbers_and_a_length_only_when_one_is_set() {
        let protocol = OpenAiEmbeddings;
        let inputs = vec!["一".to_owned(), "two".to_owned()];
        let (method, path, body) = protocol
            .embedding_request("local", &inputs, Some(256))
            .unwrap();
        assert_eq!((method, path), (Method::POST, "/embeddings"));
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["input"], serde_json::json!(["一", "two"]));
        assert_eq!(body["encoding_format"], "float");
        assert_eq!(body["dimensions"], 256);
        let (_, _, plain) = protocol.embedding_request("local", &inputs, None).unwrap();
        let plain: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        assert!(plain.get("dimensions").is_none());
    }

    #[test]
    fn vectors_are_read_back_in_the_order_of_the_texts_and_a_wrong_answer_is_refused() {
        let protocol = OpenAiEmbeddings;
        let answer = br#"{"object":"list","model":"local","data":[
            {"object":"embedding","index":1,"embedding":[0.0,1.0]},
            {"object":"embedding","index":0,"embedding":[1.0,0.0]}],
            "usage":{"prompt_tokens":7,"total_tokens":7}}"#;
        assert_eq!(
            protocol.embedding_vectors(answer, 2).unwrap(),
            vec![vec![1.0, 0.0], vec![0.0, 1.0]]
        );
        let mut observer = protocol.usage_observer();
        observer.observe(answer);
        let usage = observer.usage().unwrap();
        assert_eq!(usage.tokens.input_tokens, 7);
        assert_eq!(usage.tokens.output_tokens, 0);
        assert!(
            protocol.embedding_vectors(answer, 3).is_err(),
            "a text without its vector"
        );
        assert!(
            protocol
                .embedding_vectors(
                    br#"{"data":[{"index":0,"embedding":[1.0]},{"index":0,"embedding":[1.0]}]}"#,
                    2
                )
                .is_err(),
            "one text, two vectors"
        );
        assert!(
            protocol
                .embedding_vectors(
                    br#"{"data":[{"index":0,"embedding":[1.0]},{"index":1,"embedding":[1.0,2.0]}]}"#,
                    2
                )
                .is_err(),
            "lengths differ"
        );
        assert!(protocol.embedding_vectors(b"not json", 1).is_err());
    }
}
