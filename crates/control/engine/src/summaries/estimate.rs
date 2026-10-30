//! What a summary of a conversation will send to the summary model and cost, worked out before it is confirmed.
//!
//! The call is not made yet and the conversation is not written, so the figures are estimates: the input is the
//! newest turns in brief and an index line for each older one, as long as the conversation has material for, and never
//! more than the place's share of the model's window (`summary.handoff_input_share`).

use super::budget::input_budget;
use super::choice;
use crate::error::EngineError;
use orch_api_contract::forks::HistorySummaryEstimate;
use orch_db::Conn;
use orch_summarizer_model::descriptor::Config;
use orch_types::Id;

/// About how many bytes a turn takes in brief: the lines of what was asked, done and answered.
pub(crate) const BRIEF_TURN_BYTES: i64 = 500;
/// About how many bytes the index line of a turn takes.
pub(crate) const INDEX_LINE_BYTES: i64 = 200;
/// About how many tokens the instruction and the frame around the material take.
pub(crate) const FRAME_TOKENS: i64 = 250;

/// What a summary of `turns` turns of `thread` will send and cost, where a summary model is chosen for its place.
pub(crate) async fn handoff_estimate(
    conn: &mut Conn,
    thread: Id,
    turns: i64,
) -> Result<HistorySummaryEstimate, EngineError> {
    let none = HistorySummaryEstimate {
        available: false,
        window: None,
        share: 0,
        input_tokens: 0,
        output_tokens: 0,
        credits: None,
    };
    let Some(chosen) = choice(conn, thread).await? else {
        return Ok(none);
    };
    let Ok(config) = Config::read(&chosen.config)
        .map_err(|_| ())
        .and_then(|config| (chosen.kind == "model").then_some(config).ok_or(()))
    else {
        return Ok(none);
    };
    let budget = input_budget(conn, thread, config.provider_id, &config.model_name).await?;
    let wanted = turns
        .max(0)
        .saturating_mul(BRIEF_TURN_BYTES + INDEX_LINE_BYTES);
    let bytes = wanted.min(i64::try_from(budget.bytes).unwrap_or(i64::MAX));
    let input_tokens =
        (bytes / crate::fork_history::CARRY_BYTES_PER_TOKEN + FRAME_TOKENS).min(budget.tokens);
    let output_tokens = config.max_output_tokens;
    let own = crate::caps::own_rate(conn, config.provider_id, &config.model_name).await?;
    let book = crate::rates::RateBook::load(conn, thread).await?;
    let tokens = orch_db::usage::ledger::Tokens {
        input: input_tokens,
        cached_input: 0,
        cache_write: 0,
        output: output_tokens,
        reasoning: 0,
        total: input_tokens + output_tokens,
    };
    let priced = book
        .price(
            conn,
            own.as_ref(),
            &config.model_name,
            &tokens,
            Some(input_tokens),
            None,
        )
        .await?;
    Ok(HistorySummaryEstimate {
        available: true,
        window: budget.window,
        share: budget.share,
        input_tokens,
        output_tokens,
        credits: priced.credits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_figures_that_stand_for_a_turn_are_named_and_small() {
        const { assert!(BRIEF_TURN_BYTES > INDEX_LINE_BYTES) };
        assert_eq!(BRIEF_TURN_BYTES + INDEX_LINE_BYTES, 700);
    }
}
