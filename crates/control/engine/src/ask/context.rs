//! What bounds an Ask request: the chosen model's window in tokens, counted with that model's measured text and
//! image estimate. Nothing is set aside in advance except the room the answer may take.

use crate::context_fill::{self, Family, TextKind};
use crate::error::EngineError;
use orch_model_contract::generation::{GenerationInput, GenerationMessage, GenerationPart};
use orch_provider_contract::models::ModelParameters;

/// The window a request is fitted to and the room the answer takes of it.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Context tokens: the model's own window, lowered by Ask's own setting. None where the model states no
    /// window and no setting does.
    pub window: Option<i64>,
    /// The most tokens an answer may take, and so the room the request is fitted to leave.
    pub output: i64,
    family: Family,
}

impl Budget {
    /// Resolve Ask's numeric fitting; model selection stays with the caller.
    ///
    /// The lower of the model's window and `context` (`ask.context_tokens`) bounds the request. A model that
    /// states none is not given a small one: nothing bounds the request until its provider refuses one, and
    /// that refusal is what makes the conversation shorter.
    ///
    /// The answer's most and the window constrain each other, so the one is derived from the other instead of
    /// refusing the question: an answer takes half of the window at most, and what is left is the conversation's.
    #[must_use]
    pub fn new(
        parameters: &ModelParameters,
        model: &str,
        context: Option<i64>,
        output: i64,
    ) -> Self {
        let window = [parameters.context_window, context]
            .into_iter()
            .flatten()
            .filter(|window| *window > 0)
            .min();
        let output = window
            .map_or(output, |window| output.min(window / 2))
            .max(1);
        Self {
            window,
            output,
            family: crate::token_density::of(model),
        }
    }

    /// The tokens a request may take: the window less the answer's most. None where nothing bounds it.
    #[must_use]
    pub fn room(self) -> Option<i64> {
        self.window.map(|window| window - self.output)
    }

    /// The tokens left after `input`, for the answer's tool calls and their results; none where nothing bounds it.
    pub fn room_after(self, input: &GenerationInput) -> Result<Option<i64>, EngineError> {
        let Some(room) = self.room() else {
            return Ok(None);
        };
        Ok(Some(room - input_estimate(input, self.family)?))
    }

    /// Whether the request, a summary call included, takes no more than the room.
    pub fn fits(self, input: &GenerationInput) -> Result<bool, EngineError> {
        Ok(self.room_after(input)?.is_none_or(|left| left >= 0))
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

/// The tokens a tool result's text takes in a request, where it is carried as a JSON string.
#[must_use]
pub fn result_tokens(model: &str, content: &str) -> i64 {
    context_fill::estimate_text(
        &serde_json::to_string(content).unwrap_or_default(),
        crate::token_density::of(model),
        TextKind::Structured,
    )
}

/// The longest start of `text`, cut at a character, that takes `tokens` tokens or fewer as prose of a model of no
/// family the catalogue knows: for a text, such as a help page, that is shortened before any model is chosen.
#[must_use]
pub fn within_tokens(text: &str, tokens: i64) -> &str {
    let family = crate::token_density::of("");
    let cost = |end: usize| context_fill::estimate_text(&text[..end], family, TextKind::Prose);
    if cost(text.len()) <= tokens {
        return text;
    }
    let ends: Vec<usize> = text
        .char_indices()
        .map(|(at, _)| at)
        .chain(std::iter::once(text.len()))
        .collect();
    // `fits` stands at an end known to fit (the start, of no tokens), `over` at one known to be too long.
    let (mut fits, mut over) = (0usize, ends.len() - 1);
    while over - fits > 1 {
        let middle = fits + (over - fits) / 2;
        if cost(ends[middle]) <= tokens {
            fits = middle;
        } else {
            over = middle;
        }
    }
    &text[..ends[fits]]
}

/// What one tool result takes of a request besides its text: the message and part that carry it.
pub fn result_overhead(model: &str, call_id: &str) -> Result<i64, EngineError> {
    material(
        model,
        &[GenerationMessage {
            role: orch_model_contract::generation::GenerationRole::User,
            content: vec![GenerationPart::ToolResult {
                call_id: call_id.to_owned(),
                content: String::new(),
                is_error: false,
                witness: Some(orch_types::Id::nil()),
            }],
            continuation: vec![],
        }],
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

    fn window_of(tokens: Option<i64>) -> ModelParameters {
        serde_json::from_value(match tokens {
            Some(tokens) => json!({ "context_window": tokens }),
            None => json!({}),
        })
        .unwrap()
    }

    fn input(text: &str) -> GenerationInput {
        GenerationInput {
            system: "Answer from the sources.".into(),
            tools: vec![],
            max_output_tokens: 4096,
            messages: vec![GenerationMessage {
                role: orch_model_contract::generation::GenerationRole::User,
                content: vec![GenerationPart::Text { text: text.into() }],
                continuation: vec![],
            }],
        }
    }

    #[test]
    fn unset_uses_the_models_window_and_an_ask_only_limit_can_only_reduce_it() {
        let model: ModelParameters = serde_json::from_value(
            json!({"context_window":200000,"context_max":300000,"default_context":50000}),
        )
        .unwrap();
        let ordinary = Budget::new(&model, MODEL, None, 4096);
        assert_eq!(ordinary.window, Some(200000));
        assert_eq!(
            model.context_start(),
            Some(50000),
            "ordinary thread defaults are independent"
        );
        assert_eq!(ordinary.output, 4096);
        assert_eq!(ordinary.room(), Some(200000 - 4096));
        assert_eq!(
            Budget::new(&model, MODEL, Some(120000), 4096).window,
            Some(120000)
        );
        assert_eq!(
            Budget::new(&model, MODEL, Some(300000), 4096).window,
            Some(200000)
        );
        assert_eq!(model.context_window, Some(200000));
    }

    #[test]
    fn a_model_that_states_no_window_is_not_given_a_small_one() {
        let budget = Budget::new(&window_of(None), MODEL, None, 4096);
        assert_eq!(budget.window, None);
        assert_eq!(budget.room(), None);
        assert_eq!(budget.output, 4096);
        let long = input(&"x".repeat(4_000_000));
        assert!(budget.fits(&long).unwrap(), "nothing bounds the request");
        assert_eq!(budget.room_after(&long).unwrap(), None);
        let set = Budget::new(&window_of(None), MODEL, Some(50_000), 4096);
        assert_eq!(
            set.window,
            Some(50_000),
            "a setting bounds it where the model states nothing"
        );
    }

    #[test]
    fn the_answers_most_is_derived_from_the_window_and_no_window_refuses_the_question() {
        for window in [
            2, 3, 100, 4096, 4097, 6000, 8192, 32_000, 200_000, 1_000_000,
        ] {
            let budget = Budget::new(&window_of(Some(window)), MODEL, None, 4096);
            assert!(budget.output >= 1, "{window}");
            assert!(budget.output <= 4096, "{window}: the setting is a most");
            assert!(
                budget.output <= window / 2,
                "{window}: the answer takes half of the window at most"
            );
            assert!(
                budget.room().unwrap() >= window / 2,
                "{window}: the conversation keeps at least the other half"
            );
        }
        let small = Budget::new(&window_of(Some(4096)), MODEL, None, 4096);
        assert_eq!((small.output, small.room()), (2048, Some(2048)));
        let large = Budget::new(&window_of(Some(200_000)), MODEL, None, 1024);
        assert_eq!(large.output, 1024, "a smaller most is kept");
        let lowered = Budget::new(&window_of(Some(200_000)), MODEL, Some(4096), 4096);
        assert_eq!(
            (lowered.window, lowered.output),
            (Some(4096), 2048),
            "a setting that lowers the window to the answer's most derives the answer, not a refusal"
        );
    }

    #[test]
    fn the_room_left_is_the_window_less_the_answer_and_the_request() {
        let budget = Budget::new(&window_of(Some(10_000)), MODEL, None, 4096);
        let request = input("A question");
        let used = budget.estimate(&request).unwrap();
        assert_eq!(
            budget.room_after(&request).unwrap(),
            Some(10_000 - 4096 - used)
        );
        assert!(budget.fits(&request).unwrap());
        assert!(!budget.fits(&input(&"x".repeat(100_000))).unwrap());
    }

    #[test]
    fn a_results_tokens_are_those_of_its_text_as_the_request_carries_it() {
        let plain = result_tokens(MODEL, &"a".repeat(1000));
        let quoted = result_tokens(MODEL, &"\"".repeat(1000));
        assert!(quoted > plain, "every quote is escaped in the request");
        assert!(result_overhead(MODEL, "call").unwrap() > 0);
    }

    #[test]
    fn a_text_is_cut_at_a_character_to_the_tokens_it_may_take() {
        let text = "word ".repeat(2000);
        let cut = within_tokens(&text, 100);
        assert!(cut.len() < text.len() && text.starts_with(cut));
        let family = crate::token_density::of("");
        assert!(context_fill::estimate_text(cut, family, TextKind::Prose) <= 100);
        assert!(context_fill::estimate_text(&text[..cut.len() + 8], family, TextKind::Prose) > 100);
        assert_eq!(within_tokens("short", 100), "short");
        let chinese = "来源😀".repeat(1000);
        let cut = within_tokens(&chinese, 50);
        assert!(!cut.is_empty() && chinese.starts_with(cut));
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
        let structured = |model: &str, bytes| {
            context_fill::estimate_bytes(
                bytes,
                crate::token_density::of(model),
                TextKind::Structured,
            )
        };
        assert_eq!(structured(MODEL, 437), 100);
        assert_eq!(structured("claude-sonnet-5", 292), 100);
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
