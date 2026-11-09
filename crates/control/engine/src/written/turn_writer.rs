//! The one turn renderer accepts items incrementally, including from a private spool.
use super::*;
use std::collections::BTreeSet;

pub(crate) struct TurnWriter<'a, F, E>
where
    F: FnMut(&str) -> Result<(), E>,
{
    turn: &'a Turn,
    agent: &'a str,
    detail: Detail,
    last_plan: Option<usize>,
    position: usize,
    write: F,
    _error: std::marker::PhantomData<fn() -> E>,
    previous: Option<bool>,
    carried: usize,
    step: usize,
    asked: Option<(String, String)>,
    answered: Option<String>,
    changed: BTreeSet<String>,
}
impl<'a, F, E> TurnWriter<'a, F, E>
where
    F: FnMut(&str) -> Result<(), E>,
{
    pub(crate) fn new(
        mut write: F,
        turn: &'a Turn,
        agent: &'a str,
        detail: Detail,
        first: Option<Time>,
        last_plan: Option<usize>,
    ) -> Result<Self, E> {
        write(&heading(turn, first))?;
        write("\n")?;
        Ok(Self {
            turn,
            agent,
            detail,
            last_plan,
            position: 0,
            write,
            _error: std::marker::PhantomData,
            previous: None,
            carried: 0,
            step: 0,
            asked: None,
            answered: None,
            changed: BTreeSet::new(),
        })
    }
    fn emit(&mut self, text: String, line: bool) -> Result<(), E> {
        (self.write)(
            if self.previous.is_none() || line && self.previous == Some(true) {
                "\n"
            } else {
                "\n\n"
            },
        )?;
        (self.write)(if line {
            text.trim_end_matches('\n')
        } else {
            &text
        })?;
        self.previous = Some(line);
        Ok(())
    }
    pub(crate) fn push(&mut self, item: &Item, beside: &Beside) -> Result<(), E> {
        if !is_written(item) {
            return Ok(());
        }
        let position = self.position;
        self.position += 1;
        if self.detail == Detail::Brief {
            match &item.payload {
                ItemPayload::UserMessage(message) if self.asked.is_none() => {
                    let who = item
                        .author
                        .as_ref()
                        .map_or_else(|| "Orch".to_owned(), human);
                    let text = message
                        .parts
                        .iter()
                        .filter_map(|part| match part {
                            ContentPart::Text { text } => Some(text.as_str()),
                            ContentPart::Forwarded(copy) => Some(copy.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    self.asked = Some((who, opening(&text, BRIEF_ASKED_CHARS)));
                }
                ItemPayload::AgentMessage(message) if !message.text.trim().is_empty() => {
                    self.answered = Some(opening(&message.text, BRIEF_ANSWER_CHARS))
                }
                ItemPayload::ToolCall(call) => {
                    self.step += 1;
                    if let ToolPayload::FileChange { changes } = &call.tool {
                        self.changed
                            .extend(changes.iter().map(|change| change.path.clone()));
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        let turn = self.turn.ordinal;
        match &item.payload {
            ItemPayload::UserMessage(message) => {
                let who = item
                    .author
                    .as_ref()
                    .map_or_else(|| "Orch".to_owned(), human);
                let text = said_by_person(
                    &who,
                    message,
                    &files_in(beside.directory(), turn),
                    &mut self.carried,
                );
                self.emit(text, false)?;
            }
            ItemPayload::AgentMessage(message) => {
                if !message.text.trim().is_empty() {
                    self.emit(
                        format!("**{}:** {}", self.agent, message.text.trim()),
                        false,
                    )?;
                }
            }
            ItemPayload::Plan(_) => {
                if self.last_plan == Some(position)
                    && let Some(text) = plan_text(item)
                {
                    self.emit(text, false)?;
                }
            }
            ItemPayload::ToolCall(call) => {
                self.step += 1;
                let title = if call.title.trim().is_empty() {
                    kind_title(&call.tool)
                } else {
                    call.title.trim().to_owned()
                };
                let mut line = format!("- {title}{}", ending_of(item, call));
                if self.detail == Detail::Full {
                    let mut details = String::new();
                    step_details(&mut details, call, turn, self.step, beside);
                    if !details.is_empty() {
                        line.push('\n');
                        line.push_str(details.trim_end_matches('\n'));
                    }
                }
                self.emit(line, true)?;
            }
            ItemPayload::Approval(_) => {
                if self.detail == Detail::Full
                    && let Some(line) = approval_line(item, beside)
                {
                    self.emit(line, true)?;
                }
            }
            ItemPayload::Announcement(_) => {
                if let Some(text) = announcement_text(item) {
                    self.emit(text, false)?;
                }
            }
            ItemPayload::Marker(marker) => {
                if self.detail == Detail::Full {
                    self.emit(format!("- — {} —", marker_words(marker)), true)?;
                }
            }
            ItemPayload::Error(error) => {
                self.emit(format!("**Error:** {}", error.message.trim()), false)?
            }
            ItemPayload::Reasoning(_) => {}
        }
        Ok(())
    }
    pub(crate) fn finish(mut self) -> Result<(), E> {
        if self.detail == Detail::Brief {
            if let Some((who, text)) = self.asked.take() {
                (self.write)(&format!("\n**{who}:** {text}"))?;
            }
            if let Some(text) = self.answered.take() {
                (self.write)(&format!("\n**{}:** {text}", self.agent))?;
            }
            if self.step > 0 || !self.changed.is_empty() {
                let mut counts = plural(self.step, "step", "steps");
                if !self.changed.is_empty() {
                    let _ = write!(
                        counts,
                        " · {}",
                        plural(self.changed.len(), "file changed", "files changed")
                    );
                }
                (self.write)(&format!("\n{counts}"))?;
            }
            (self.write)("\n")?;
        } else if self.previous.is_some() {
            (self.write)("\n")?;
        }
        Ok(())
    }
}
