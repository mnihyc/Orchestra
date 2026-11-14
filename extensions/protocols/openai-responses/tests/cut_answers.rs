//! A completion that the upstream cut short says why, in the upstream's own word, and whether the answer
//! ran into its output allowance.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_model_contract::protocol::{CompletionStop, WireProtocol};
use orch_protocol_openai_responses::{OpenAiChat, OpenAiResponses};

fn stop(reason: &str, output_limit: bool) -> Option<CompletionStop> {
    Some(CompletionStop {
        reason: reason.to_owned(),
        output_limit,
    })
}

#[test]
fn a_responses_answer_that_ran_out_of_output_says_so_and_a_whole_one_says_nothing() {
    let protocol = OpenAiResponses;
    let cut = br#"{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"reasoning","summary":[]},{"type":"message","content":[{"type":"output_text","text":"A part of the summary"}]}]}"#;
    assert!(protocol.completion_text(cut).is_err());
    assert_eq!(
        protocol.completion_stopped(cut),
        stop("max_output_tokens", true)
    );
    let filtered =
        br#"{"status":"incomplete","incomplete_details":{"reason":"content_filter"},"output":[]}"#;
    assert_eq!(
        protocol.completion_stopped(filtered),
        stop("content_filter", false)
    );
    let whole = br#"{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"Done"}]}]}"#;
    assert_eq!(protocol.completion_text(whole).unwrap(), "Done");
    assert_eq!(protocol.completion_stopped(whole), None);
}

#[test]
fn a_chat_answer_that_ran_out_of_output_says_so_and_a_whole_one_says_nothing() {
    let protocol = OpenAiChat;
    let cut = br#"{"choices":[{"finish_reason":"length","message":{"content":"A part"}}]}"#;
    assert!(protocol.completion_text(cut).is_err());
    assert_eq!(protocol.completion_stopped(cut), stop("length", true));
    let other = br#"{"choices":[{"finish_reason":"content_filter","message":{"content":null}}]}"#;
    assert_eq!(
        protocol.completion_stopped(other),
        stop("content_filter", false)
    );
    let whole = br#"{"choices":[{"finish_reason":"stop","message":{"content":"Done"}}]}"#;
    assert_eq!(protocol.completion_stopped(whole), None);
}
