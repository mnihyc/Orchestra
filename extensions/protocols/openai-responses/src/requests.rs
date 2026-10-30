//! Allowed paths and explicit model extraction.
//!
//! The Responses format has three requests worth relaying: creating a response (`POST /responses`, answered
//! as one JSON document or as a server-sent event stream), compacting a conversation (`POST
//! /responses/compact`) and listing models (`GET /models`). Every other request, such as fetching, deleting or
//! cancelling a stored response, or a file or batch call, is refused: a harness that talks through Orch has no
//! use for them, and each would reach state of the provider's account that the lease does not cover.
//!
//! A path is compared exactly, below the upstream base and without its query. A trailing slash, a
//! percent-encoded letter or a `..` segment is therefore a different path and is refused, so what reaches the
//! upstream is always one of the paths of the table and nothing that merely resolves to one.
//!
//! The model a request names is the member `model` of its JSON body, as it is written: the routing name, which
//! the lease compares as a string.

use http::Method;
use orch_model_contract::protocol::{OPENAI_RESPONSES_REQUESTS, is_allowed, model_in_body};

/// The path, below the upstream base, that lists models.
pub const MODELS_PATH: &str = "/models";

/// Whether a request of `method` to `path` below the upstream base is one the Responses format lets
/// through. The query is no part of the path.
#[must_use]
pub fn allowed(method: &Method, path: &str) -> bool {
    is_allowed(&OPENAI_RESPONSES_REQUESTS, method, path)
}

/// The model a request names in the member `model` of its JSON body, or `None` when the body names none
/// (a list request has no body at all). The path is not consulted: this format never puts the model there.
#[must_use]
pub fn model_of(path: &str, body: &[u8]) -> Option<String> {
    let _ = path;
    model_in_body(body)
}

/// Whether the request asks for the list of models. The forwarder answers it from the lease and never
/// forwards it, and it has no model to check.
#[must_use]
pub fn lists_models(method: &Method, path: &str) -> bool {
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    method == Method::GET && path == MODELS_PATH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_responses_compaction_and_the_model_list_pass() {
        assert!(allowed(&Method::POST, "/responses"));
        assert!(allowed(&Method::POST, "/responses/compact"));
        assert!(allowed(&Method::GET, "/models"));
        assert!(
            allowed(&Method::POST, "/responses?api-version=1"),
            "the query is no part of the path"
        );
    }

    #[test]
    fn everything_that_reaches_stored_state_or_another_api_is_refused() {
        assert!(!allowed(&Method::GET, "/responses/resp_1"));
        assert!(!allowed(&Method::DELETE, "/responses/resp_1"));
        assert!(!allowed(&Method::POST, "/responses/resp_1/cancel"));
        assert!(!allowed(&Method::GET, "/responses"), "the method counts");
        assert!(!allowed(&Method::POST, "/models"));
        assert!(!allowed(&Method::POST, "/chat/completions"));
        assert!(!allowed(&Method::POST, "/v1/messages"));
        assert!(!allowed(&Method::POST, "/files"));
        assert!(!allowed(&Method::POST, "/batches"));
    }

    #[test]
    fn a_path_that_only_resolves_to_an_allowed_one_is_refused() {
        for path in [
            "/responses/",
            "//responses",
            "/responses/.",
            "/models/../responses",
            "/respons%65s",
            "/Responses",
            "/responses%2Fcompact",
            "responses",
            "",
        ] {
            assert!(!allowed(&Method::POST, path), "{path:?}");
        }
    }

    #[test]
    fn the_model_is_the_member_of_the_body_exactly_as_it_is_written() {
        let body = br#"{"model":"openai/gpt-5.6-luna:free","input":"hi","stream":true}"#;
        assert_eq!(
            model_of("/responses", body).as_deref(),
            Some("openai/gpt-5.6-luna:free")
        );
        assert_eq!(
            model_of("/responses/compact", br#"{"model":" Mixed-Case "}"#).as_deref(),
            Some(" Mixed-Case "),
            "nothing is trimmed or lower-cased: the lease compares the string"
        );
    }

    #[test]
    fn a_body_that_names_no_usable_model_names_none() {
        assert_eq!(model_of("/responses", br#"{"input":"hi"}"#), None);
        assert_eq!(model_of("/responses", br#"{"model":null}"#), None);
        assert_eq!(model_of("/responses", br#"{"model":["a"]}"#), None);
        assert_eq!(model_of("/responses", b"not json"), None);
        assert_eq!(model_of("/models", b""), None);
    }

    #[test]
    fn the_model_list_request_is_recognised_with_or_without_a_query() {
        assert!(lists_models(&Method::GET, "/models"));
        assert!(lists_models(&Method::GET, "/models?limit=5"));
        assert!(!lists_models(&Method::POST, "/models"));
        assert!(!lists_models(&Method::GET, "/models/gpt-6"));
        assert!(!lists_models(&Method::GET, "/responses"));
    }
}
