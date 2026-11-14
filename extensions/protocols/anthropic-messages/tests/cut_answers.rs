//! A completion that the upstream cut short says why, in the upstream's own word, and whether the answer
//! ran into its output allowance.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_model_contract::protocol::{CompletionStop, WireProtocol};
use orch_protocol_anthropic_messages::AnthropicMessages;

#[test]
fn a_messages_answer_that_ran_out_of_output_says_so_and_a_whole_one_says_nothing() {
    let protocol = AnthropicMessages;
    let cut = br#"{"type":"message","stop_reason":"max_tokens","content":[{"type":"thinking","thinking":"..."},{"type":"text","text":"A part"}]}"#;
    assert!(protocol.completion_text(cut).is_err());
    assert_eq!(
        protocol.completion_stopped(cut),
        Some(CompletionStop {
            reason: "max_tokens".to_owned(),
            output_limit: true
        })
    );
    let refused = br#"{"type":"message","stop_reason":"refusal","content":[]}"#;
    assert_eq!(
        protocol.completion_stopped(refused),
        Some(CompletionStop {
            reason: "refusal".to_owned(),
            output_limit: false
        })
    );
    let whole =
        br#"{"type":"message","stop_reason":"end_turn","content":[{"type":"text","text":"Done"}]}"#;
    assert_eq!(protocol.completion_text(whole).unwrap(), "Done");
    assert_eq!(protocol.completion_stopped(whole), None);
}
