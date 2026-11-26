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

/// Transformations performed by the native conformance rule, without rejected source text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConformanceFacts {
    /// Whitespace was normalized.
    pub flattened: bool,
    /// A multiline answer was reduced to its first nonempty line.
    pub first_line_only: bool,
    /// Characters removed by the chosen character allowance, after field formatting.
    pub removed_characters: u64,
    /// Supplied tags absent from the offered choices.
    pub unknown_tags: u64,
    /// Supplied tags already retained by this answer.
    pub duplicate_tags: u64,
    /// Known, nonduplicate tags beyond the chosen tag allowance.
    pub over_budget_tags: u64,
    /// An invalid handoff was rejected whole instead of keeping a command/path prefix.
    pub rejected_whole: bool,
}

impl ConformanceFacts {
    fn include(&mut self, earlier: &Self) {
        self.flattened |= earlier.flattened;
        self.first_line_only |= earlier.first_line_only;
        self.removed_characters = self
            .removed_characters
            .saturating_add(earlier.removed_characters);
        self.unknown_tags = self.unknown_tags.saturating_add(earlier.unknown_tags);
        self.duplicate_tags = self.duplicate_tags.saturating_add(earlier.duplicate_tags);
        self.over_budget_tags = self
            .over_budget_tags
            .saturating_add(earlier.over_budget_tags);
        self.rejected_whole |= earlier.rejected_whole;
    }
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
    /// Native observations survive a second in-process conformance boundary. Serialized producer
    /// replies cannot supply these facts: their words are held to the native rule when received.
    #[serde(skip)]
    pub conformance: Option<ConformanceFacts>,
}

/// A result held to the rule, and whether it had to be discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conformed {
    /// What may be written: `text` or `tags` is `None` when the output was discarded. The usage is kept
    /// either way, since the call was made.
    pub result: SummaryResult,
    /// Whether the output was malformed and nothing is to be written.
    pub discarded: bool,
    /// Actual transformations, kept separately from usage and from the conformed words.
    pub facts: ConformanceFacts,
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
        let previous = self.conformance.unwrap_or_default();
        let count = |text: &str| u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
        let mut made = match field {
            SummaryField::Summary => {
                let normalized = self
                    .text
                    .as_deref()
                    .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "));
                let paragraph = normalized
                    .as_deref()
                    .filter(|text| !text.is_empty())
                    .map(|text| cut(text, limits.max_chars).trim_end().to_owned())
                    .filter(|text| !text.is_empty());
                let facts = ConformanceFacts {
                    flattened: self
                        .text
                        .as_deref()
                        .zip(normalized.as_deref())
                        .is_some_and(|(raw, normalized)| raw != normalized),
                    removed_characters: normalized
                        .as_deref()
                        .map_or(0, count)
                        .saturating_sub(paragraph.as_deref().map_or(0, count)),
                    ..ConformanceFacts::default()
                };
                Conformed {
                    discarded: paragraph.is_none(),
                    facts,
                    result: SummaryResult {
                        text: paragraph,
                        tags: None,
                        usage,
                        conformance: None,
                    },
                }
            }
            SummaryField::Title | SummaryField::Description | SummaryField::Rollup => {
                let first = self
                    .text
                    .as_deref()
                    .and_then(|text| text.lines().find(|line| !line.trim().is_empty()));
                let formatted = first.map(str::trim);
                let line = formatted.map(|line| cut(line, limits.max_chars).trim_end().to_owned());
                let facts = ConformanceFacts {
                    flattened: first
                        .zip(formatted)
                        .is_some_and(|(raw, trimmed)| raw != trimmed),
                    first_line_only: first.is_some()
                        && self.text.as_deref().is_some_and(|text| text.contains('\n')),
                    removed_characters: formatted
                        .map_or(0, count)
                        .saturating_sub(line.as_deref().map_or(0, count)),
                    ..ConformanceFacts::default()
                };
                Conformed {
                    discarded: line.is_none(),
                    facts,
                    result: SummaryResult {
                        text: line,
                        tags: None,
                        usage,
                        conformance: None,
                    },
                }
            }
            SummaryField::Tags => {
                let most = usize::try_from(limits.max_tags).unwrap_or(usize::MAX);
                let mut facts = ConformanceFacts::default();
                let tags = self.tags.map(|given| {
                    let mut kept: Vec<String> = Vec::new();
                    for name in given {
                        let Some(choice) = tag_choices
                            .iter()
                            .find(|choice| choice.eq_ignore_ascii_case(name.trim()))
                        else {
                            facts.unknown_tags = facts.unknown_tags.saturating_add(1);
                            continue;
                        };
                        if kept.contains(choice) {
                            facts.duplicate_tags = facts.duplicate_tags.saturating_add(1);
                        } else if kept.len() >= most {
                            facts.over_budget_tags = facts.over_budget_tags.saturating_add(1);
                        } else {
                            kept.push(choice.clone());
                        }
                    }
                    kept
                });
                Conformed {
                    discarded: tags.is_none(),
                    facts,
                    result: SummaryResult {
                        text: None,
                        tags,
                        usage,
                        conformance: None,
                    },
                }
            }
            SummaryField::Handoff => {
                // Reject the whole answer: cutting an exact path or command changes its meaning.
                let text = self.text.filter(|text| {
                    !text.trim().is_empty()
                        && text.chars().count()
                            <= usize::try_from(limits.max_chars).unwrap_or(usize::MAX)
                        && text.split_whitespace().count() <= HANDOFF_MAX_WORDS
                });
                let facts = ConformanceFacts {
                    rejected_whole: text.is_none(),
                    flattened: text.as_deref().is_some_and(|text| text != text.trim()),
                    ..ConformanceFacts::default()
                };
                Conformed {
                    discarded: text.is_none(),
                    facts,
                    result: SummaryResult {
                        text: text.map(|text| text.trim().to_owned()),
                        tags: None,
                        usage,
                        conformance: None,
                    },
                }
            }
        };
        made.facts.include(&previous);
        made.result.conformance = Some(made.facts.clone());
        made
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarizer::{DESCRIPTION_MAX_CHARS, TITLE_MAX_CHARS};
    use serde_json::json;

    #[test]
    fn serialized_producer_claims_cannot_invent_native_transformation_counts() {
        let supplied: SummaryResult = serde_json::from_value(json!({
            "text": "ready", "conformance": {"removed_characters": 900, "rejected_whole": true}
        }))
        .unwrap();
        let made = supplied.conform(
            SummaryField::Summary,
            &SummaryLimits {
                max_chars: 100,
                max_tags: 0,
            },
            &[],
        );
        assert_eq!(made.facts, ConformanceFacts::default());
        assert_eq!(made.result.text.as_deref(), Some("ready"));
    }

    #[test]
    fn a_shortened_paragraph_keeps_its_actual_transformation_when_it_crosses_another_boundary() {
        let made = SummaryResult {
            text: Some("  alpha  beta\ngamma delta  ".into()),
            usage: Some(used()),
            ..SummaryResult::default()
        }
        .conform(
            SummaryField::Summary,
            &SummaryLimits {
                max_chars: 10,
                max_tags: 0,
            },
            &[],
        );
        assert_eq!(made.result.text.as_deref(), Some("alpha beta"));
        assert!(made.facts.flattened);
        assert_eq!(made.facts.removed_characters, 12);
        assert_eq!(made.result.usage, Some(used()));
        let facts = made.facts.clone();
        let again = made.result.conform(
            SummaryField::Summary,
            &SummaryLimits {
                max_chars: 1000,
                max_tags: 0,
            },
            &[],
        );
        assert_eq!(
            again.facts, facts,
            "the already cut result does not acquire a new origin from a later allowance"
        );
        assert_eq!(again.result.text.as_deref(), Some("alpha beta"));
    }

    #[test]
    fn title_and_tag_adjustments_record_the_decision_that_actually_dropped_each_part() {
        let title = SummaryResult::text("first line\nsecond line").conform(
            SummaryField::Title,
            &limits(SummaryField::Title),
            &[],
        );
        assert!(title.facts.first_line_only);
        assert_eq!(title.facts.removed_characters, 0);
        let tags = SummaryResult::tags(&["backend", "BACKEND", "invented", "docs", "infra"])
            .conform(
                SummaryField::Tags,
                &SummaryLimits {
                    max_chars: 80,
                    max_tags: 1,
                },
                &["backend".into(), "docs".into(), "infra".into()],
            );
        assert_eq!(tags.result.tags, Some(vec!["backend".into()]));
        assert_eq!(tags.facts.unknown_tags, 1);
        assert_eq!(tags.facts.duplicate_tags, 1);
        assert_eq!(tags.facts.over_budget_tags, 2);
    }

    #[test]
    fn a_rejected_handoff_keeps_the_usage_and_the_whole_rejection_without_retaining_a_command_prefix()
     {
        let answer = SummaryResult {
            text: Some("run /an/exact/command".into()),
            usage: Some(used()),
            ..SummaryResult::default()
        }
        .conform(
            SummaryField::Handoff,
            &SummaryLimits {
                max_chars: 8,
                max_tags: 0,
            },
            &[],
        );
        assert!(answer.discarded && answer.facts.rejected_whole);
        assert!(answer.result.text.is_none());
        assert_eq!(answer.result.usage, Some(used()));
        assert_eq!(
            answer.facts.removed_characters, 0,
            "a rejected whole is not a shortened valid handoff"
        );
    }

    #[test]
    fn a_summary_is_one_paragraph_and_its_requested_size_can_exceed_the_default() {
        let text = format!("{}\n\nNext step.", "Reviewed work. ".repeat(60));
        let limits = SummaryLimits {
            max_chars: 1200,
            max_tags: 0,
        };
        let made = SummaryResult::text(&text).conform(SummaryField::Summary, &limits, &[]);
        let paragraph = made.result.text.unwrap();
        assert!(!made.discarded && !paragraph.contains('\n'));
        assert!(
            paragraph.chars().count() > usize::try_from(crate::summarizer::SUMMARY_CHARS).unwrap()
        );
        assert!(paragraph.ends_with("Next step."));
        let smaller = SummaryResult::text(&paragraph).conform(
            SummaryField::Summary,
            &SummaryLimits {
                max_chars: 24,
                max_tags: 0,
            },
            &[],
        );
        let kept = smaller.result.text.unwrap();
        assert!(!kept.is_empty() && kept.chars().count() <= 24);
    }

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
