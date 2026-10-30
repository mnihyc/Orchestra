//! Allowed paths and explicit model extraction.
//!
//! The Messages format has three requests worth relaying: creating a message (`POST /v1/messages`, answered as
//! one JSON document or as a server-sent event stream), counting the tokens a message would take (`POST
//! /v1/messages/count_tokens`) and listing models (`GET /v1/models`). Every other request, such as a batch, a
//! file, a skill or a managed agent, is refused: each would reach state or spending of the provider's account
//! that the lease does not cover.
//!
//! A path is compared exactly, below the upstream base and without its query. The query matters to the
//! upstream (a harness may ask for `?beta=true`) and passes on with the request. A trailing slash, a
//! percent-encoded letter or a `..` segment makes a different path, which is refused, so what reaches the
//! upstream is always one of the paths of the table and nothing that merely resolves to one.
//!
//! The model a request names is the member `model` of its JSON body, as it is written: the routing name, which
//! the lease compares as a string. A request to count tokens names a model the same way.

use http::Method;
use orch_model_contract::protocol::{ANTHROPIC_MESSAGES_REQUESTS, is_allowed, model_in_body};

/// The path, below the upstream base, that lists models.
pub const MODELS_PATH: &str = "/v1/models";

/// Whether a request of `method` to `path` below the upstream base is one the Messages format lets
/// through. The query is no part of the path.
#[must_use]
pub fn allowed(method: &Method, path: &str) -> bool {
    is_allowed(&ANTHROPIC_MESSAGES_REQUESTS, method, path)
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
    fn the_requests_a_harness_needs_pass_with_whatever_query_it_adds() {
        assert!(allowed(&Method::POST, "/v1/messages"));
        assert!(allowed(&Method::POST, "/v1/messages?beta=true"));
        assert!(allowed(&Method::POST, "/v1/messages/count_tokens"));
        assert!(allowed(
            &Method::POST,
            "/v1/messages/count_tokens?beta=true"
        ));
        assert!(allowed(&Method::GET, "/v1/models"));
        assert!(allowed(&Method::GET, "/v1/models?limit=1000"));
    }

    #[test]
    fn everything_that_reaches_the_accounts_state_or_another_api_is_refused() {
        assert!(!allowed(&Method::POST, "/v1/messages/batches"));
        assert!(!allowed(&Method::GET, "/v1/messages/batches/msgbatch_1"));
        assert!(!allowed(&Method::POST, "/v1/files"));
        assert!(!allowed(&Method::POST, "/v1/skills"));
        assert!(!allowed(&Method::POST, "/v1/complete"));
        assert!(!allowed(&Method::GET, "/v1/models/claude-opus-5"));
        assert!(!allowed(&Method::GET, "/v1/messages"), "the method counts");
        assert!(!allowed(&Method::POST, "/v1/models"));
        assert!(!allowed(&Method::POST, "/responses"));
        assert!(
            !allowed(&Method::POST, "/messages"),
            "the base ends before /v1"
        );
    }

    #[test]
    fn a_path_that_only_resolves_to_an_allowed_one_is_refused() {
        for path in [
            "/v1/messages/",
            "//v1/messages",
            "/v1/messages/.",
            "/v1/models/../messages",
            "/v1/messag%65s",
            "/V1/messages",
            "/v1/messages%2Fcount_tokens",
            "v1/messages",
            "",
        ] {
            assert!(!allowed(&Method::POST, path), "{path:?}");
        }
    }

    #[test]
    fn the_model_is_the_member_of_the_body_exactly_as_it_is_written() {
        let body = br#"{"model":"anthropic/claude-haiku-5.5","max_tokens":16,"messages":[]}"#;
        assert_eq!(
            model_of("/v1/messages", body).as_deref(),
            Some("anthropic/claude-haiku-5.5")
        );
        assert_eq!(
            model_of("/v1/messages/count_tokens", br#"{"model":"claude-opus-5"}"#).as_deref(),
            Some("claude-opus-5"),
            "counting tokens names a model the same way"
        );
    }

    #[test]
    fn a_body_that_names_no_usable_model_names_none() {
        assert_eq!(model_of("/v1/messages", br#"{"messages":[]}"#), None);
        assert_eq!(model_of("/v1/messages", br#"{"model":3}"#), None);
        assert_eq!(model_of("/v1/messages", b"{"), None);
        assert_eq!(model_of("/v1/models", b""), None);
    }

    #[test]
    fn the_model_list_request_is_recognised_with_or_without_a_query() {
        assert!(lists_models(&Method::GET, "/v1/models"));
        assert!(lists_models(
            &Method::GET,
            "/v1/models?limit=1000&after_id=x"
        ));
        assert!(!lists_models(&Method::POST, "/v1/models"));
        assert!(!lists_models(&Method::GET, "/v1/models/claude-opus-5"));
        assert!(!lists_models(&Method::GET, "/models"));
    }
}
