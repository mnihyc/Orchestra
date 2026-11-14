//! Ask's own reserves over the chosen model's measured text and image estimate.

use crate::context_fill::{self, Family, TextKind};
use crate::error::{AskNoRoom, EngineError};
use orch_model_contract::generation::{GenerationInput, GenerationMessage, GenerationPart};
use orch_provider_contract::models::ModelParameters;

/// The model's effective Ask window and the room left for each kind of request.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Context tokens after applying the Ask-only policy.
    pub window: i64,
    /// Tokens reserved for every answer, including compaction.
    pub output: i64,
    /// Tokens reserved for this turn's read-tool results.
    pub tools: i64,
    /// The bytes of read-tool results one turn may hold for this model: what `tools` stands for, and never more
    /// than the configured bytes.
    pub tool_bytes: usize,
    family: Family,
}

impl Budget {
    /// Resolve only Ask's numeric fitting policy; model selection stays with the caller.
    ///
    /// The answer's reserve comes off the window first. Of what is left, a turn's tool results may take half at
    /// most, and never more than the configured bytes stand for, so that a window that states nothing, or a small
    /// one, still leaves the conversation its share instead of refusing every question.
    pub fn new(
        parameters: &ModelParameters,
        model: &str,
        context: Option<i64>,
        output: i64,
        tool_bytes: usize,
    ) -> Result<Self, EngineError> {
        let stated = parameters.context_window.filter(|window| *window > 0);
        let own = stated.unwrap_or(crate::summaries::UNKNOWN_WINDOW_TOKENS);
        let window = context.map_or(own, |limit| own.min(limit));
        if output <= 0 || window <= output {
            // The model is the cause when its own window leaves nothing, whatever the setting says.
            return Err(EngineError::AskContextNoRoom(if own <= output {
                AskNoRoom::Window {
                    model: model.to_owned(),
                    window: own,
                    output,
                    assumed: stated.is_none(),
                }
            } else {
                AskNoRoom::Setting { window, output }
            }));
        }
        let family = crate::token_density::of(model);
        let configured = context_fill::estimate_bytes(
            i64::try_from(tool_bytes).unwrap_or(i64::MAX),
            family,
            TextKind::Structured,
        );
        let tools = configured.min((window - output) / 2);
        let held = if tools >= configured {
            tool_bytes
        } else {
            usize::try_from(context_fill::bytes_within_tokens(
                tools,
                family,
                TextKind::Structured,
            ))
            .unwrap_or(0)
            .min(tool_bytes)
        };
        Ok(Self {
            window,
            output,
            tools,
            tool_bytes: held,
            family,
        })
    }

    /// History, instructions and tool schemas fit before new tool results arrive.
    pub fn fits(self, input: &GenerationInput) -> Result<bool, EngineError> {
        Ok(input_estimate(input, self.family)? <= self.window - self.output - self.tools)
    }

    /// A compaction call has no tools, but retains the same output reserve.
    pub fn fits_summary(self, input: &GenerationInput) -> Result<bool, EngineError> {
        Ok(input_estimate(input, self.family)? <= self.window - self.output)
    }

    /// Framing/instructions for a bounded summary call, using this same model's density.
    pub fn estimate(self, input: &GenerationInput) -> Result<i64, EngineError> {
        input_estimate(input, self.family)
    }

    /// One whole native message or prefix, including referenced image costs.
    pub fn material(self, messages: &[GenerationMessage]) -> Result<i64, EngineError> {
        material_estimate(messages, self.family)
    }
}

/// A tool reserve known by bytes uses the existing structured-text estimate.
#[must_use]
pub fn tokens(model: &str, bytes: usize) -> i64 {
    context_fill::estimate_bytes(
        i64::try_from(bytes).unwrap_or(i64::MAX),
        crate::token_density::of(model),
        TextKind::Structured,
    )
}

fn images(messages: &[GenerationMessage], family: Family) -> i64 {
    let count = messages
        .iter()
        .flat_map(|message| &message.content)
        .filter(|part| {
            matches!(
                part,
                GenerationPart::Image { .. } | GenerationPart::ImageRef { .. }
            )
        })
        .count();
    context_fill::estimate_images(i64::try_from(count).unwrap_or(i64::MAX), family)
}

fn estimation_messages(messages: &[GenerationMessage]) -> Vec<GenerationMessage> {
    messages
        .iter()
        .map(|message| GenerationMessage {
            role: message.role,
            content: message
                .content
                .iter()
                .map(|part| match part {
                    GenerationPart::Image {
                        media_type, name, ..
                    } => GenerationPart::Image {
                        media_type: media_type.clone(),
                        data: String::new(),
                        name: name.clone(),
                    },
                    other => other.clone(),
                })
                .collect(),
            continuation: message.continuation.clone(),
        })
        .collect()
}

fn material_estimate(messages: &[GenerationMessage], family: Family) -> Result<i64, EngineError> {
    let text = serde_json::to_string(&estimation_messages(messages))?;
    Ok(
        context_fill::estimate_text(&text, family, TextKind::Structured)
            .saturating_add(images(messages, family)),
    )
}

fn input_estimate(input: &GenerationInput, family: Family) -> Result<i64, EngineError> {
    let mut framed = input.clone();
    framed.messages = estimation_messages(&input.messages);
    Ok(context_fill::estimate_text(
        &serde_json::to_string(&framed)?,
        family,
        TextKind::Structured,
    )
    .saturating_add(images(&input.messages, family)))
}

/// Estimate the complete portable request with the already selected model's catalogue facts.
pub fn estimate(model: &str, input: &GenerationInput) -> Result<i64, EngineError> {
    input_estimate(input, crate::token_density::of(model))
}

/// Estimate a completed summary or its original material with the same model.
pub fn material(model: &str, messages: &[GenerationMessage]) -> Result<i64, EngineError> {
    material_estimate(messages, crate::token_density::of(model))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a deciding case fails by panicking")]
mod tests {
    use super::*;
    use serde_json::json;
    const MODEL: &str = "gpt-6-sol";

    #[test]
    fn unset_uses_the_models_window_and_an_ask_only_limit_can_only_reduce_it() {
        let model: ModelParameters = serde_json::from_value(
            json!({"context_window":200000,"context_max":300000,"default_context":50000}),
        )
        .unwrap();
        let ordinary = Budget::new(&model, MODEL, None, 4096, 262144).unwrap();
        assert_eq!(ordinary.window, 200000);
        assert_eq!(
            model.context_start(),
            Some(50000),
            "ordinary thread defaults are independent"
        );
        assert_eq!(ordinary.output, 4096);
        assert_eq!(ordinary.tools, tokens(MODEL, 262144));
        assert_eq!(
            Budget::new(&model, MODEL, Some(120000), 4096, 262144)
                .unwrap()
                .window,
            120000
        );
        assert_eq!(
            Budget::new(&model, MODEL, Some(300000), 4096, 262144)
                .unwrap()
                .window,
            200000
        );
        assert_eq!(model.context_window, Some(200000));
    }

    fn window_of(tokens: Option<i64>) -> ModelParameters {
        serde_json::from_value(match tokens {
            Some(tokens) => json!({ "context_window": tokens }),
            None => json!({}),
        })
        .unwrap()
    }

    #[test]
    fn a_model_whose_window_is_not_recorded_gets_a_budget_with_every_value_as_shipped() {
        // A model added to a provider by name states no window: Ask takes the assumed one.
        let budget = Budget::new(&window_of(None), MODEL, None, 4096, 262144)
            .expect("the shipped values leave room in the assumed window");
        assert_eq!(budget.window, crate::summaries::UNKNOWN_WINDOW_TOKENS);
        let after_answer = budget.window - budget.output;
        assert!(
            budget.tools <= after_answer / 2,
            "tool results take half at most"
        );
        assert!(
            after_answer - budget.tools >= after_answer / 2,
            "and the history keeps at least the other half"
        );
    }

    #[test]
    fn any_window_above_the_answers_reserve_gets_a_budget_and_the_reserve_is_a_share_of_it() {
        for window in [
            4097, 6000, 8000, 16000, 32000, 64000, 128000, 200000, 1_000_000,
        ] {
            let budget = Budget::new(&window_of(Some(window)), MODEL, None, 4096, 262144)
                .unwrap_or_else(|error| panic!("a window of {window} tokens: {error}"));
            let after_answer = window - 4096;
            assert!(
                budget.tools <= (after_answer / 2).max(0),
                "{window}: the tool reserve {} is more than half of {after_answer}",
                budget.tools
            );
            assert!(
                budget.tools <= tokens(MODEL, 262144),
                "{window}: and never more than the configured bytes stand for"
            );
            assert!(
                after_answer - budget.tools > 0,
                "{window}: the history keeps some room"
            );
        }
    }

    #[test]
    fn a_turns_tool_bytes_are_its_reserve_in_that_models_density_and_never_above_the_configured_ones()
     {
        for window in [4097, 8000, 16000, 32000, 64000, 128000, 200000, 400_000] {
            let budget = Budget::new(&window_of(Some(window)), MODEL, None, 4096, 262144).unwrap();
            assert!(budget.tool_bytes <= 262144, "{window}");
            assert!(
                tokens(MODEL, budget.tool_bytes) <= budget.tools,
                "{window}: a turn that holds {} bytes holds no more than its {} tokens",
                budget.tool_bytes,
                budget.tools
            );
        }
        let small = Budget::new(&window_of(None), MODEL, None, 4096, 262144).unwrap();
        assert!(
            small.tool_bytes < 262144,
            "a small window holds fewer bytes than the configured ones"
        );
        let large = Budget::new(&window_of(Some(400_000)), MODEL, None, 4096, 262144).unwrap();
        assert_eq!(large.tool_bytes, 262144);
    }

    #[test]
    fn on_a_large_window_the_reserve_is_what_the_configured_bytes_stand_for() {
        for window in [400_000, 1_000_000] {
            let budget = Budget::new(&window_of(Some(window)), MODEL, None, 4096, 262144).unwrap();
            assert_eq!(budget.tools, tokens(MODEL, 262144), "{window}");
            assert_eq!(budget.window, window);
        }
    }

    #[test]
    fn a_window_the_answers_reserve_fills_names_the_model_when_the_model_is_the_cause() {
        let known = Budget::new(&window_of(Some(4096)), MODEL, None, 4096, 262144).unwrap_err();
        assert!(matches!(
            known,
            EngineError::AskContextNoRoom(AskNoRoom::Window { assumed: false, .. })
        ));
        let reason = known.to_string();
        assert!(
            reason.contains(MODEL) && reason.contains("ask.max_output_tokens"),
            "{reason}"
        );
        assert!(
            !reason.contains("ask.turn_tool_bytes") && !reason.contains("ask.context_tokens"),
            "a setting that cannot help is not named: {reason}"
        );
        // The window is unknown and assumed: the sentence says so.
        let assumed = Budget::new(&window_of(None), MODEL, None, 40_000, 262144).unwrap_err();
        assert!(matches!(
            assumed,
            EngineError::AskContextNoRoom(AskNoRoom::Window { assumed: true, .. })
        ));
        let reason = assumed.to_string();
        assert!(
            reason.contains(MODEL)
                && reason.contains("not recorded")
                && reason.contains("ask.max_output_tokens"),
            "{reason}"
        );
        assert!(!reason.contains("ask.context_tokens"), "{reason}");
    }

    #[test]
    fn a_window_the_answers_reserve_fills_names_the_setting_when_ask_context_tokens_is_the_cause() {
        let error =
            Budget::new(&window_of(Some(200_000)), MODEL, Some(4096), 4096, 262144).unwrap_err();
        assert!(matches!(
            error,
            EngineError::AskContextNoRoom(AskNoRoom::Setting { .. })
        ));
        let reason = error.to_string();
        assert!(
            reason.contains("ask.context_tokens") && reason.contains("ask.max_output_tokens"),
            "{reason}"
        );
        assert!(
            !reason.contains("ask.turn_tool_bytes") && !reason.contains(MODEL),
            "{reason}"
        );
    }

    #[test]
    fn model_density_and_image_cost_replace_a_fixed_byte_ratio() {
        let mut input = GenerationInput {
            system: "说明上下文".repeat(100),
            tools: vec![],
            max_output_tokens: 4096,
            messages: vec![GenerationMessage {
                role: orch_model_contract::generation::GenerationRole::User,
                content: vec![GenerationPart::Text {
                    text: "代码与资料".repeat(100),
                }],
                continuation: vec![],
            }],
        };
        assert_ne!(
            estimate(MODEL, &input).unwrap(),
            estimate("claude-sonnet-5", &input).unwrap(),
            "Chinese context follows the selected model's measured characters per token"
        );
        assert_eq!(tokens(MODEL, 437), 100);
        assert_eq!(tokens("claude-sonnet-5", 292), 100);
        input.messages[0].content.push(GenerationPart::Image {
            media_type: "image/png".into(),
            name: "screenshot.png".into(),
            data: "a".repeat(256),
        });
        let small = estimate(MODEL, &input).unwrap();
        if let GenerationPart::Image { data, .. } = &mut input.messages[0].content[1] {
            *data = "b".repeat(1024 * 1024);
        }
        assert_eq!(
            small,
            estimate(MODEL, &input).unwrap(),
            "encoded pixels are one image, not a megabyte of text tokens"
        );
        assert!(small >= crate::token_density::of(MODEL).image_tokens);
    }
}
