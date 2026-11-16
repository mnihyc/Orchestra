//! Native prerequisites are resolved from the local durable journal, never by another API exchange.
use super::*;

enum Prerequisite {
    Ready,
    Waiting { id: Id, missing: bool },
    Failed(CommandError),
}

pub(super) enum Next {
    Run(Queued, Option<CommandError>),
    Wait(Option<Duration>),
    Empty,
}

fn invalid(detail: &str) -> CommandError {
    CommandError::new(ErrorCode::Conflict, detail)
}

fn operand(body: &Json, member: &str) -> Option<Id> {
    body.get(member)?.as_str()?.parse().ok()
}

/// An input's opening must actually describe its own native lane, even on a malformed wire frame.
fn matching_opening(input: &Cmd, opening: &Cmd) -> bool {
    let fenced = input.kind == CommandKind::TurnStartFenced.as_str();
    if input.kind != CommandKind::TurnStart.as_str() && !fenced {
        return true;
    }
    let turn = if fenced {
        input.body.get("turn")
    } else {
        Some(&input.body)
    };
    let Some(thread) = turn.and_then(|body| operand(body, "thread_id")) else {
        return false;
    };
    if input.key.as_deref() != Some(thread.to_string().as_str()) || input.key != opening.key {
        return false;
    }
    if fenced && operand(&input.body, "thread_id") != Some(thread) {
        return false;
    }
    let session = if opening.kind == CommandKind::SessionOpen.as_str() {
        Some(&opening.body)
    } else if opening.kind == CommandKind::SessionOpenFenced.as_str()
        && operand(&opening.body, "thread_id") == Some(thread)
    {
        opening.body.get("session")
    } else {
        None
    };
    let Some(session) = session else {
        return false;
    };
    if operand(session, "thread_id") != Some(thread)
        || is_transport_rebind(&opening.kind, &opening.body)
    {
        return false;
    }
    if fenced {
        let segment = operand(&input.body, "segment_id");
        segment.is_some() && segment == operand(session, "segment_id")
    } else {
        true
    }
}

impl CommandServer {
    fn prerequisite(&self, command: &Cmd) -> Prerequisite {
        let Some(first) = command.after else {
            return Prerequisite::Ready;
        };
        if first == command.id {
            return Prerequisite::Failed(invalid("a command cannot depend on itself"));
        }
        let journal = lock(&self.inner.journal);
        let Some(opening) = journal.get(first) else {
            return if command.deadline <= self.system_now() {
                Prerequisite::Failed(too_late())
            } else {
                Prerequisite::Waiting {
                    id: first,
                    missing: true,
                }
            };
        };
        if !matching_opening(command, &opening.cmd) {
            return Prerequisite::Failed(invalid("the input names another native session opening"));
        }
        if let Step::Done { result } = &opening.step {
            return match &result.outcome {
                Ok(_) => Prerequisite::Ready,
                Err(error) => Prerequisite::Failed(error.clone()),
            };
        }
        let mut visited = HashSet::from([command.id]);
        let mut next = Some(first);
        while let Some(id) = next {
            if !visited.insert(id) {
                return Prerequisite::Failed(invalid("the command prerequisites contain a cycle"));
            }
            let Some(entry) = journal.get(id) else {
                break;
            };
            if matches!(entry.step, Step::Done { .. }) {
                break;
            }
            next = entry.cmd.after;
        }
        Prerequisite::Waiting {
            id: first,
            missing: false,
        }
    }

    fn arrival_wait(&self, command: &Cmd, missing: bool) -> Option<Duration> {
        missing.then(|| {
            (command.deadline.get() - self.system_now().get())
                .to_std()
                .unwrap_or_default()
        })
    }

    pub(super) fn next_queued(&self, key: &str) -> Next {
        let mut state = lock(&self.inner.state);
        loop {
            let Some(queue) = state.queues.get_mut(key) else {
                state.working.remove(key);
                return Next::Empty;
            };
            let Some(front) = queue.front() else {
                state.queues.remove(key);
                state.working.remove(key);
                return Next::Empty;
            };
            let failure = match self.prerequisite(&front.cmd) {
                Prerequisite::Ready => None,
                Prerequisite::Failed(error) => Some(error),
                Prerequisite::Waiting { id, missing } => {
                    if let Some(position) = queue.iter().position(|queued| queued.cmd.id == id) {
                        // Reconnect may replay the dependent first. Run its predecessor first,
                        // preserving each command's admission, cancellation and journal entry.
                        if let Some(prior) = queue.remove(position) {
                            queue.push_front(prior);
                        }
                        continue;
                    }
                    return Next::Wait(self.arrival_wait(&front.cmd, missing));
                }
            };
            let Some(queued) = queue.pop_front() else {
                continue;
            };
            if let Some(hand) = state.in_hand.get_mut(&queued.cmd.id) {
                hand.queued_under = None;
            }
            return Next::Run(queued, failure);
        }
    }

    pub(super) async fn wait_for_prerequisite(&self, command: &Cmd) -> Result<(), CommandError> {
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            let _ = changed.as_mut().enable();
            let cancel = lock(&self.inner.state)
                .in_hand
                .get(&command.id)
                .map(|hand| hand.cancel.clone());
            let Some(cancel) = cancel else {
                return Err(invalid("the command no longer owns its native queue"));
            };
            if cancel.is_cancelled() {
                return Err(invalid("cancelled before it started"));
            }
            match self.prerequisite(command) {
                Prerequisite::Ready => return Ok(()),
                Prerequisite::Failed(error) => return Err(error),
                Prerequisite::Waiting { missing, .. } => {
                    if let Some(until) = self.arrival_wait(command, missing) {
                        tokio::select! {
                            () = cancel.cancelled() => {}
                            () = &mut changed => {}
                            () = tokio::time::sleep(until) => {}
                        }
                    } else {
                        tokio::select! {
                            () = cancel.cancelled() => {}
                            () = &mut changed => {}
                        }
                    }
                }
            }
        }
    }
}
