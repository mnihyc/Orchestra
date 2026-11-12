//! Metadata passes retain names and references, while the one written renderer consumes one item at a time.
use super::*;
use crate::{
    views::{People, item_view, turn_view},
    written,
};
use orch_api_contract::threads::{ItemPayload, ToolPayload};
use std::collections::BTreeSet;
#[derive(Default)]
pub(crate) struct Presentation {
    pub approvals: BTreeSet<Id>,
    pub streams: BTreeSet<String>,
    pub first: Option<orch_types::Time>,
    pub last_plan: Option<usize>,
    pub author: Option<Id>,
    pub asked: String,
}
impl Bundle {
    pub(crate) fn authors(&self) -> Result<BTreeSet<Id>, EngineError> {
        let mut ids = BTreeSet::new();
        ids.extend(self.turn.interrupted_by);
        for row in self.items.iter() {
            ids.extend(row?.author_id);
        }
        Ok(ids)
    }
    pub(crate) fn presentation(&self, people: &People) -> Result<Presentation, EngineError> {
        let mut out = Presentation::default();
        let mut asked = false;
        for (position, row) in self.items.iter().enumerate() {
            let row = row?;
            let item = item_view(&row, people);
            if out.first.is_none() {
                out.first = Some(item.created_at);
            }
            match &item.payload {
                ItemPayload::Plan(_) => out.last_plan = Some(position),
                ItemPayload::Approval(approval) => {
                    out.approvals.insert(approval.approval_id);
                }
                ItemPayload::ToolCall(call) => {
                    if let ToolPayload::Subagent { child_stream, .. } = &call.tool {
                        out.streams.insert(child_stream.clone());
                    }
                }
                _ => {}
            }
            if !asked && row.kind == "user_message" && row.turn_id == Some(self.turn.id) {
                asked = true;
                out.author = row.author_id;
                let mut room = written::BRIEF_ASKED_CHARS;
                for text in row.payload["parts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|part| part["text"].as_str())
                {
                    for word in text.split_whitespace() {
                        if room == 0 {
                            break;
                        }
                        if !out.asked.is_empty() {
                            out.asked.push(' ');
                            room -= 1;
                        }
                        for ch in word.chars().take(room) {
                            out.asked.push(ch);
                            room -= 1;
                        }
                    }
                }
            }
        }
        Ok(out)
    }
    pub(crate) fn write(
        &self,
        people: &People,
        beside: &written::Beside,
        agent: &str,
        detail: written::Detail,
        info: &Presentation,
    ) -> Result<Text, EngineError> {
        let turn = turn_view(&self.turn, "", people)?;
        let mut text = Text::default();
        let mut writer = written::TurnWriter::new(
            |piece| text.push(piece),
            &turn,
            agent,
            detail,
            info.first,
            info.last_plan,
        )?;
        for item in self.items.iter() {
            writer.push(&item_view(&item?, people), beside)?;
        }
        writer.finish()?;
        Ok(text)
    }
}
