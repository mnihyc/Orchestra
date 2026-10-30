//! Text, tag and usage results.
//!
//! A summarizer returns text (a title or a description), or tags, and, where it called a model, what the call
//! used ([`UsageEvent`]). What the engine does with it is its own: it writes the result only if the field's
//! source is still automatic when it commits, so a person's edit always wins, and it makes each call a row of
//! the ledger. What a result has to be before it is written is a rule that does not depend on who made it
//! ([`SummaryResult::conform`]): a description is one line of at most 200 characters, a title at most 80, tags
//! a subset of the offered ones and at most the most a topic may have. Longer output is cut; malformed output
//! is discarded.

use orch_agent_contract::events::TokenUsage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::activity::cut;
use crate::descriptor::SummaryField;
use crate::summarizer::{HANDOFF_MAX_WORDS, SummaryLimits};

/// What one call of a summarizer used. The engine adds what it alone knows (the provider, the thread, the
/// person to whom it is attributed, the rate) when it writes the ledger row, with the origin `orch` and the
/// purpose `summary`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageEvent {
    /// The routing name of the model that answered.
    pub model: String,
    /// What the call used.
    pub tokens: TokenUsage,
    /// The upstream's id for the request.
    #[serde(default)]
    pub request_ref: Option<String>,
}

/// What a summarizer made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummaryResult {
    /// The text, for a title or a description.
    #[serde(default)]
    pub text: Option<String>,
    /// The tags, for the field `tags`: names among those offered.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// What the call used, where it used anything the ledger counts.
    #[serde(default)]
    pub usage: Option<UsageEvent>,
}

/// A result held to the rule, and whether it had to be discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conformed {
    /// What may be written: `text` or `tags` is `None` when the output was discarded. The usage is kept
    /// either way, since the call was made.
    pub result: SummaryResult,
    /// Whether the output was malformed and nothing is to be written.
    pub discarded: bool,
}

impl SummaryResult {
    /// A result with a text.
    #[must_use]
    pub fn text(text: &str) -> Self {
        Self {
            text: Some(text.to_owned()),
            ..Self::default()
        }
    }

    /// A result with tags.
    #[must_use]
    pub fn tags(tags: &[&str]) -> Self {
        Self {
            tags: Some(tags.iter().map(ToString::to_string).collect()),
            ..Self::default()
        }
    }

    /// Holds the result to the rule for `field`.
    ///
    /// A title or a description is one line: the first line that has anything in it, cut to
    /// `limits.max_chars` characters; no text is malformed. Tags are a subset of `tag_choices` (matched without
    /// regard to case, and returned as the choice is written), each once, at most `limits.max_tags`; a list of
    /// none is a valid answer ("none applies"), and no list is malformed. The usage is kept.
    #[must_use]
    pub fn conform(
        self,
        field: SummaryField,
        limits: &SummaryLimits,
        tag_choices: &[String],
    ) -> Conformed {
        let usage = self.usage;
        match field {
            SummaryField::Title | SummaryField::Description | SummaryField::Rollup => {
                let line = self
                    .text
                    .as_deref()
                    .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
                    .map(|line| cut(line, limits.max_chars).trim_end().to_owned());
                Conformed {
                    discarded: line.is_none(),
                    result: SummaryResult {
                        text: line,
                        tags: None,
                        usage,
                    },
                }
            }
            SummaryField::Tags => {
                let most = usize::try_from(limits.max_tags).unwrap_or(usize::MAX);
                let tags = self.tags.map(|given| {
                    let mut kept: Vec<String> = Vec::new();
                    for name in given {
                        let Some(choice) = tag_choices
                            .iter()
                            .find(|choice| choice.eq_ignore_ascii_case(name.trim()))
                        else {
                            continue;
                        };
                        if !kept.contains(choice) && kept.len() < most {
                            kept.push(choice.clone());
                        }
                    }
                    kept
                });
                Conformed {
                    discarded: tags.is_none(),
                    result: SummaryResult {
                        text: None,
                        tags,
                        usage,
                    },
                }
            }
            SummaryField::Handoff => {
                // Reject a malformed or oversized handoff whole. A character cut can change the
                // meaning of an exact path or command the next agent is told to use.
                let text = self
                    .text
                    .filter(|text| {
                        !text.trim().is_empty()
                            && text.chars().count()
                                <= usize::try_from(limits.max_chars).unwrap_or(usize::MAX)
                            && text.split_whitespace().count() <= HANDOFF_MAX_WORDS
                    })
                    .map(|text| text.trim().to_owned());
                Conformed {
                    discarded: text.is_none(),
                    result: SummaryResult {
                        text,
                        tags: None,
                        usage,
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarizer::{DESCRIPTION_MAX_CHARS, TITLE_MAX_CHARS};
    use serde_json::json;

    fn limits(field: SummaryField) -> SummaryLimits {
        SummaryLimits::for_field(field, 3)
    }

    fn used() -> UsageEvent {
        UsageEvent {
            model: "small".into(),
            tokens: TokenUsage {
                input_tokens: 400,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 20,
                reasoning_tokens: 0,
                total_tokens: 420,
            },
            request_ref: Some("resp_1".into()),
        }
    }

    #[test]
    fn a_description_is_one_line_of_at_most_200_characters_and_longer_output_is_cut() {
        let long = format!("Fixing the redirect after login. {}", "x".repeat(300));
        let conformed = SummaryResult::text(&long).conform(
            SummaryField::Description,
            &limits(SummaryField::Description),
            &[],
        );
        let text = conformed.result.text.unwrap();
        assert_eq!(text.chars().count(), DESCRIPTION_MAX_CHARS as usize);
        assert!(text.starts_with("Fixing the redirect"));
        assert!(!conformed.discarded);
        let two_lines = SummaryResult::text("\n  Working on the tests.  \nThen the docs.").conform(
            SummaryField::Description,
            &limits(SummaryField::Description),
            &[],
        );
        assert_eq!(
            two_lines.result.text.as_deref(),
            Some("Working on the tests."),
            "the first line that has anything in it"
        );
    }

    #[test]
    fn a_title_is_at_most_80_characters() {
        let conformed = SummaryResult::text(&"t".repeat(120)).conform(
            SummaryField::Title,
            &limits(SummaryField::Title),
            &[],
        );
        assert_eq!(
            conformed.result.text.unwrap().chars().count(),
            TITLE_MAX_CHARS as usize
        );
        let wide = SummaryResult::text(&"字".repeat(100)).conform(
            SummaryField::Title,
            &limits(SummaryField::Title),
            &[],
        );
        assert_eq!(
            wide.result.text.unwrap().chars().count(),
            80,
            "characters, not bytes"
        );
    }

    #[test]
    fn output_that_says_nothing_is_discarded_and_the_call_is_still_counted() {
        for empty in [SummaryResult::default(), SummaryResult::text("   \n  ")] {
            let with_usage = SummaryResult {
                usage: Some(used()),
                ..empty
            };
            let conformed =
                with_usage.conform(SummaryField::Title, &limits(SummaryField::Title), &[]);
            assert!(conformed.discarded);
            assert_eq!(conformed.result.text, None);
            assert_eq!(
                conformed.result.usage,
                Some(used()),
                "tokens were spent, so the ledger row is still written"
            );
        }
    }

    #[test]
    fn tags_are_a_subset_of_the_offered_ones_each_once_and_at_most_the_most_a_topic_may_have() {
        let choices: Vec<String> = ["backend", "Frontend", "docs", "infra"]
            .map(String::from)
            .to_vec();
        let given = SummaryResult::tags(&[
            "frontend", "BACKEND", "invented", "backend", "docs", "infra",
        ]);
        let conformed = given.conform(SummaryField::Tags, &limits(SummaryField::Tags), &choices);
        assert_eq!(
            conformed.result.tags.as_deref(),
            Some(["Frontend", "backend", "docs"].map(String::from).as_slice())
        );
        assert!(!conformed.discarded);
        let none = SummaryResult::tags(&[]).conform(
            SummaryField::Tags,
            &limits(SummaryField::Tags),
            &choices,
        );
        assert_eq!(none.result.tags, Some(vec![]), "none applies is an answer");
        assert!(!none.discarded);
        let nothing_offered = SummaryResult::tags(&["backend"]).conform(
            SummaryField::Tags,
            &limits(SummaryField::Tags),
            &[],
        );
        assert_eq!(nothing_offered.result.tags, Some(vec![]));
        let malformed = SummaryResult::default().conform(
            SummaryField::Tags,
            &limits(SummaryField::Tags),
            &choices,
        );
        assert!(malformed.discarded);
    }

    #[test]
    fn a_result_reads_as_a_summarizer_writes_it() {
        let result: SummaryResult = serde_json::from_value(json!({
            "text": "Fixing the login redirect", "tags": null,
            "usage": { "model": "small", "tokens": { "input_tokens": 400, "cached_input_tokens": 0, "output_tokens": 20,
                                                      "reasoning_tokens": 0, "total_tokens": 420 }, "request_ref": "resp_1" }
        }))
        .unwrap();
        assert_eq!(result.usage, Some(used()));
        let bare: SummaryResult = serde_json::from_value(json!({ "text": "x" })).unwrap();
        assert!(bare.tags.is_none() && bare.usage.is_none());
    }
}
