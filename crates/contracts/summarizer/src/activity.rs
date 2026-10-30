//! Redacted input activity and previous values.
//!
//! What a summarizer is shown is the node's current title and description, and the thread's main-stream items
//! since the last summary: at most the latest `input_items`, without tool output, each cut to `input_chars`
//! characters, after redaction. The engine redacts the stored text (`redaction.*`) before it is handed over,
//! so a summarizer, built in or installed, never sees what the place masks. [`trim`] makes the list from the
//! items, and [`ActivityItem`] is one of them.

use orch_types::Time;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What kind of material an item is. Document text is distinct from messages, and a summarizer may weigh them differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// What a person said.
    UserMessage,
    /// What the agent said.
    AgentMessage,
    /// The agent's reasoning.
    Reasoning,
    /// The agent's plan.
    Plan,
    /// A tool call: its title, and never what it printed.
    ToolCall,
    /// Current text of a document being described.
    Document,
    /// An error.
    Error,
}

/// One piece of source material as a summarizer is shown it: redacted, and cut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActivityItem {
    /// What kind of step it is.
    pub kind: ActivityKind,
    /// Its text, after redaction and cut to the limit. For a tool call, its title.
    pub text: String,
    /// When it happened.
    #[serde(default)]
    pub at: Option<Time>,
}

/// Cuts `text` to at most `chars` characters, never in the middle of one.
#[must_use]
pub fn cut(text: &str, chars: u32) -> String {
    text.chars()
        .take(usize::try_from(chars).unwrap_or(usize::MAX))
        .collect()
}

/// The activity a summarizer is shown, from the items of the thread's main stream since the last summary,
/// oldest first: the latest `input_items` of them, each cut to `input_chars` characters. An item that has
/// no text left is dropped before the count, so that an empty step takes no place.
#[must_use]
pub fn trim(items: Vec<ActivityItem>, input_items: u32, input_chars: u32) -> Vec<ActivityItem> {
    let mut kept: Vec<ActivityItem> = items
        .into_iter()
        .map(|item| ActivityItem {
            text: cut(item.text.trim(), input_chars),
            ..item
        })
        .filter(|item| !item.text.is_empty())
        .collect();
    let most = usize::try_from(input_items).unwrap_or(usize::MAX);
    if kept.len() > most {
        kept.drain(..kept.len() - most);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(kind: ActivityKind, text: &str) -> ActivityItem {
        ActivityItem {
            kind,
            text: text.into(),
            at: None,
        }
    }

    #[test]
    fn a_summarizer_is_shown_the_latest_items_each_cut_to_the_limit() {
        let items: Vec<_> = (1..=60)
            .map(|n| {
                item(
                    ActivityKind::AgentMessage,
                    &format!("message {n} {}", "x".repeat(600)),
                )
            })
            .collect();
        let shown = trim(items, 50, 500);
        assert_eq!(shown.len(), 50, "at most the latest input_items");
        assert!(
            shown[0].text.starts_with("message 11 "),
            "the oldest ten are dropped"
        );
        assert!(shown.iter().all(|i| i.text.chars().count() == 500));
        assert!(shown[49].text.starts_with("message 60 "));
    }

    #[test]
    fn an_item_with_nothing_left_takes_no_place_and_a_cut_is_never_inside_a_character() {
        let items = vec![
            item(ActivityKind::UserMessage, "fix the login redirect"),
            item(ActivityKind::ToolCall, "   "),
            item(ActivityKind::Reasoning, "字字字字字"),
        ];
        let shown = trim(items, 2, 3);
        assert_eq!(
            shown.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(),
            ["fix", "字字字"]
        );
        assert_eq!(cut("añb", 2), "añ");
        assert_eq!(cut("abc", 0), "");
        assert!(trim(vec![], 50, 500).is_empty());
    }

    #[test]
    fn an_item_reads_as_the_engine_writes_it() {
        let item: ActivityItem = serde_json::from_value(
            json!({ "kind": "tool_call", "text": "cargo test", "at": "2026-10-11T10:00:00.000Z" }),
        )
        .unwrap();
        assert_eq!(item.kind, ActivityKind::ToolCall);
        let bare: ActivityItem =
            serde_json::from_value(json!({ "kind": "plan", "text": "1. read" })).unwrap();
        assert!(bare.at.is_none());
    }

    #[test]
    fn document_material_keeps_its_kind_across_the_producer_boundary() {
        let material = json!({"kind":"document","text":"How to configure redirects","at":null});
        let item: ActivityItem = serde_json::from_value(material.clone()).unwrap();
        assert_eq!(item.kind, ActivityKind::Document);
        assert_eq!(serde_json::to_value(item).unwrap(), material);
    }
}
