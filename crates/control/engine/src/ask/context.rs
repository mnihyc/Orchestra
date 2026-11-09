//! Ask's own reserves over the chosen model's measured text and image estimate.

use crate::context_fill::{self, Family, TextKind};
use crate::error::EngineError;
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
    family: Family,
}

impl Budget {
    /// Resolve only Ask's numeric fitting policy; model selection stays with the caller.
    pub fn new(
        parameters: &ModelParameters,
        model: &str,
        context: Option<i64>,
        output: i64,
        tool_bytes: usize,
    ) -> Result<Self, EngineError> {
        let window = parameters
            .context_window
            .filter(|window| *window > 0)
            .unwrap_or(crate::summaries::UNKNOWN_WINDOW_TOKENS);
        let window = context.map_or(window, |limit| window.min(limit));
        let family = crate::token_density::of(model);
        let tools = context_fill::estimate_bytes(
            i64::try_from(tool_bytes).unwrap_or(i64::MAX),
            family,
            TextKind::Structured,
        );
        if output <= 0 || window.saturating_sub(output).saturating_sub(tools) <= 0 {
            return Err(EngineError::AskContextNoRoom);
        }
        Ok(Self {
            window,
            output,
            tools,
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

    #[test]
    fn a_window_equal_to_its_reserves_names_the_setting_instead_of_calling_a_provider() {
        let model: ModelParameters =
            serde_json::from_value(json!({"context_window":200000})).unwrap();
        let error = Budget::new(
            &model,
            MODEL,
            Some(4096 + tokens(MODEL, 262144)),
            4096,
            262144,
        )
        .unwrap_err();
        assert!(matches!(error, EngineError::AskContextNoRoom));
        let reason = error.to_string();
        assert!(
            reason.contains("ask.context_tokens")
                && reason.contains("ask.max_output_tokens")
                && reason.contains("ask.turn_tool_bytes")
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
