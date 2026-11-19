//! The suite every summarizer passes: each field within its limits, with and without a model.
//!
//! A summarizer is driven through its contract's trait (`orch_summarizer_contract::summarizer::Summarizer`), so
//! the same cases serve a built-in one and an external one behind its plugin bridge:
//!
//! | Rule | The summarizer |
//! |------|----------------|
//! | `summarizer.descriptor` | declares a name, at least one field, each once, and a configuration schema that is an object |
//! | `summarizer.<field>` | for each field it declares, turns a short conversation into an answer that the rule for that field keeps: a line of text within the field's limit, or tags among the choices it was given, at most as many as allowed |
//! | `summarizer.undeclared` | refuses a field it does not declare with `unsupported`, never with a text |
//! | `summarizer.nothing` | given no activity, answers within the limits or skips; it does not fail |
//!
//! "With and without a model": a summarizer that needs one is checked against whatever its configuration
//! names; one that needs none (rules, templates) answers the same cases with nothing else running.

use std::time::Duration;

use orch_summarizer_contract::activity::{ActivityItem, ActivityKind};
use orch_summarizer_contract::descriptor::SummaryField;
use orch_summarizer_contract::summarizer::{
    SummarizeError, Summarizer, SummaryLimits, SummaryRequest,
};
use orch_types::Json;
use serde_json::json;

use crate::report::{Checked, Miss, Report, skip};

/// How long one summary may take: a model behind a slow provider answers a short request well within it.
pub const SUMMARY_WITHIN: Duration = Duration::from_secs(120);

/// How many tags a request allows.
const MAX_TAGS: u32 = 3;

/// The tags a request offers to choose from.
const TAG_CHOICES: [&str; 4] = ["bug", "login", "backend", "docs"];

/// Runs the summarizer's cases, with `config` as the rest of its `auto.summarizer` setting, into `report`.
pub async fn run(summarizer: &dyn Summarizer, config: &Json, report: &mut Report) {
    let descriptor = summarizer.describe();
    report.checked(
        "summarizer.descriptor",
        "It declares a name, at least one field, each once, and a configuration schema that is an object",
        descriptor.check().map_err(|faults| {
            faults
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
                .into()
        }),
    );
    for field in SummaryField::ALL {
        let rule = format!("summarizer.{}", field_name(field));
        let title = format!(
            "Asked for its {}, it answers what the rule for that field keeps",
            field_name(field)
        );
        if !descriptor.writes(field) {
            report.checked(&rule, &title, Err(skip("it does not declare that field")));
            continue;
        }
        report
            .run(
                &rule,
                &title,
                SUMMARY_WITHIN,
                field_case(summarizer, field, config),
            )
            .await;
    }
    let undeclared = SummaryField::ALL
        .into_iter()
        .find(|field| !descriptor.writes(*field));
    let title = "It refuses a field it does not declare with unsupported";
    match undeclared {
        Some(field) => {
            report
                .run(
                    "summarizer.undeclared",
                    title,
                    SUMMARY_WITHIN,
                    undeclared_case(summarizer, field, config),
                )
                .await;
        }
        None => report.checked(
            "summarizer.undeclared",
            title,
            Err(skip("it declares every field")),
        ),
    }
    match descriptor.fields.first().copied() {
        Some(field) => {
            report
                .run(
                    "summarizer.nothing",
                    "Given no activity, it answers within the limits or skips, and does not fail",
                    SUMMARY_WITHIN,
                    nothing_case(summarizer, field, config),
                )
                .await;
        }
        None => report.checked(
            "summarizer.nothing",
            "Given no activity, it answers within the limits or skips, and does not fail",
            Err(skip("it declares no field")),
        ),
    }
}

const fn field_name(field: SummaryField) -> &'static str {
    match field {
        SummaryField::Title => "title",
        SummaryField::Description => "description",
        SummaryField::Tags => "tags",
        SummaryField::Rollup => "rollup",
        SummaryField::Handoff => "handoff",
    }
}

fn request(
    field: SummaryField,
    activity: Vec<ActivityItem>,
    config: &Json,
) -> Result<SummaryRequest, Miss> {
    // The node a summary is for is named only; a summarizer reads nothing from Orch by it.
    let node = serde_json::from_value(json!({
        "id": "0199abcd-1234-7000-8000-00000000c0f1",
        "kind": "thread",
        "name": "Fix login",
        "redacted": false
    }))
    .map_err(|error| format!("the suite could not write the node of its request: {error}"))?;
    Ok(SummaryRequest {
        field,
        node,
        previous: String::new(),
        activity,
        tag_choices: TAG_CHOICES.iter().map(|tag| (*tag).to_owned()).collect(),
        limits: SummaryLimits::for_field(field, MAX_TAGS),
        config: config.clone(),
    })
}

fn conversation() -> Vec<ActivityItem> {
    let item = |kind, text: &str| ActivityItem {
        kind,
        text: text.to_owned(),
        at: None,
    };
    vec![
        item(
            ActivityKind::UserMessage,
            "The login form refuses a correct password after the session cookie expires.",
        ),
        item(
            ActivityKind::AgentMessage,
            "Found it: the backend compares the password against a stale cached hash.",
        ),
        item(ActivityKind::ToolCall, "Edit src/auth/session.rs"),
        item(
            ActivityKind::AgentMessage,
            "Fixed the cache invalidation in the login path; the tests pass.",
        ),
    ]
}

async fn field_case(summarizer: &dyn Summarizer, field: SummaryField, config: &Json) -> Checked {
    let asked = request(field, conversation(), config)?;
    let limits = asked.limits;
    let answer = summarizer
        .summarize(asked)
        .await
        .map_err(|error| format!("it failed: {error}"))?;
    match field {
        SummaryField::Title | SummaryField::Description | SummaryField::Rollup => {
            let text = answer
                .text
                .ok_or("it answered no text for a field of text")?;
            let line = text.trim();
            if line.is_empty() {
                return Err("its text is empty".into());
            }
            if line.contains('\n') {
                return Err("its text is more than one line".into());
            }
            let chars = u32::try_from(line.chars().count()).unwrap_or(u32::MAX);
            if chars > limits.max_chars {
                return Err(format!(
                    "its text is {chars} characters, over the limit of {}",
                    limits.max_chars
                )
                .into());
            }
            Ok(())
        }
        SummaryField::Handoff => {
            let text = answer.text.ok_or("it answered no handoff text")?;
            if text.trim().is_empty()
                || text.chars().count() > usize::try_from(limits.max_chars).unwrap_or(usize::MAX)
                || text.split_whitespace().count()
                    > orch_summarizer_contract::summarizer::HANDOFF_MAX_WORDS
            {
                return Err("its handoff is empty or exceeds the word or character bound".into());
            }
            Ok(())
        }
        SummaryField::Tags => {
            let tags = answer.tags.ok_or("it answered no list of tags")?;
            let most = usize::try_from(limits.max_tags).unwrap_or(usize::MAX);
            if tags.len() > most {
                return Err(
                    format!("it chose {} tags where {most} are allowed", tags.len()).into(),
                );
            }
            if let Some(stray) = tags.iter().find(|tag| {
                !TAG_CHOICES
                    .iter()
                    .any(|choice| choice.eq_ignore_ascii_case(tag.trim()))
            }) {
                return Err(format!("it chose {stray:?}, which was not among the choices").into());
            }
            Ok(())
        }
    }
}

async fn undeclared_case(
    summarizer: &dyn Summarizer,
    field: SummaryField,
    config: &Json,
) -> Checked {
    match summarizer
        .summarize(request(field, conversation(), config)?)
        .await
    {
        Err(SummarizeError::Unsupported(refused)) if refused == field => Ok(()),
        Err(SummarizeError::Unsupported(refused)) => Err(format!(
            "it refused {}, not the field it was asked for",
            field_name(refused)
        )
        .into()),
        Err(error) => Err(format!("it failed where unsupported was due: {error}").into()),
        Ok(_) => Err(format!(
            "it wrote its {}, which it does not declare",
            field_name(field)
        )
        .into()),
    }
}

async fn nothing_case(summarizer: &dyn Summarizer, field: SummaryField, config: &Json) -> Checked {
    let asked = request(field, Vec::new(), config)?;
    let limits = asked.limits;
    match summarizer.summarize(asked).await {
        Err(SummarizeError::Skipped(_)) => Ok(()),
        Err(error) => Err(format!("it failed on an empty conversation: {error}").into()),
        Ok(answer) => {
            let over = answer.text.as_deref().is_some_and(|text| {
                u32::try_from(text.trim().chars().count()).unwrap_or(u32::MAX) > limits.max_chars
            }) || answer.tags.as_ref().is_some_and(|tags| {
                tags.len() > usize::try_from(limits.max_tags).unwrap_or(usize::MAX)
            });
            if over {
                Err("its answer to an empty conversation is over the field's limits".into())
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use orch_summarizer_contract::descriptor::SummarizerDescriptor;
    use orch_summarizer_contract::result::SummaryResult;

    /// A summarizer with no model: a description from the last thing the agent said, tags by the words.
    struct Rules {
        /// What it writes as a description, to make it break the rule on purpose.
        description: Option<String>,
    }

    #[async_trait]
    impl Summarizer for Rules {
        fn describe(&self) -> SummarizerDescriptor {
            serde_json::from_value(json!({
                "name": "rules", "title": "By rules", "config_schema": {},
                "fields": ["description", "tags"]
            }))
            .unwrap()
        }

        async fn summarize(
            &self,
            request: SummaryRequest,
        ) -> Result<orch_summarizer_contract::result::SummaryResult, SummarizeError> {
            match request.field {
                SummaryField::Description => {
                    let latest = request
                        .activity
                        .iter()
                        .rev()
                        .find(|item| item.kind == ActivityKind::AgentMessage)
                        .ok_or_else(|| SummarizeError::Skipped("nothing was said".into()))?;
                    Ok(SummaryResult::text(
                        self.description.as_deref().unwrap_or(&latest.text),
                    ))
                }
                SummaryField::Tags => {
                    let words: String = request
                        .activity
                        .iter()
                        .map(|item| item.text.to_lowercase())
                        .collect();
                    let chosen: Vec<&str> = request
                        .tag_choices
                        .iter()
                        .map(String::as_str)
                        .filter(|tag| words.contains(*tag))
                        .take(usize::try_from(request.limits.max_tags).unwrap())
                        .collect();
                    Ok(SummaryResult::tags(&chosen))
                }
                field => Err(SummarizeError::Unsupported(field)),
            }
        }
    }

    #[tokio::test]
    async fn a_summarizer_that_keeps_to_its_contract_passes_every_case_it_declares() {
        let mut report = Report::new("summarizer \"rules\"");
        run(&Rules { description: None }, &json!({}), &mut report).await;
        assert!(report.passed(), "{report}");
        assert!(matches!(
            report.outcome("summarizer.title"),
            Some(crate::report::Outcome::Skipped(_))
        ));
        assert_eq!(
            report.outcome("summarizer.undeclared"),
            Some(&crate::report::Outcome::Passed)
        );
    }

    #[tokio::test]
    async fn a_description_over_its_limit_fails_the_rule_that_names_it() {
        let mut report = Report::new("summarizer \"long\"");
        let long = "x".repeat(400);
        run(
            &Rules {
                description: Some(long),
            },
            &json!({}),
            &mut report,
        )
        .await;
        assert!(!report.passed());
        assert_eq!(
            report
                .failures()
                .map(|case| case.rule.as_str())
                .collect::<Vec<_>>(),
            ["summarizer.description"]
        );
    }
}
