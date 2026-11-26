//! Native prerequisites are resolved from the local durable journal, never by another API exchange.
use super::*;

enum Prerequisite {
    Ready,
    Waiting { id: Id },
    Failed(CommandError),
}

pub(super) enum Next {
    Run(Box<Queued>, Option<CommandError>),
    Wait(Option<Time>),
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
    let Some(orch_link_contract::commands::NativeTarget::Session(target)) = &opening.native_target
    else {
        return false;
    };
    if !target.coherent()
        || target.thread_id != thread
        || target.opening_id != Some(opening.id)
        || target.segment_id != operand(session, "segment_id")
        || input.native_target != opening.native_target
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
        if command
            .authority_until
            .is_some_and(|until| until <= self.system_now())
        {
            return Prerequisite::Failed(too_late());
        }
        let Some(first) = command.after else {
            return Prerequisite::Ready;
        };
        if first == command.id {
            return Prerequisite::Failed(invalid("a command cannot depend on itself"));
        }
        let journal = lock(&self.inner.journal);
        let Some(opening) = journal.get(first) else {
            return Prerequisite::Waiting { id: first };
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
        Prerequisite::Waiting { id: first }
    }

    pub(super) fn next_queued(&self, key: &str) -> Next {
        let mut state = lock(&self.inner.state);
        let mut examined = HashSet::new();
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
            let failure = if !examined.insert(front.cmd.id) {
                // Journal writes can still be pending for every command in the cycle.
                // Revisiting an in-memory queue head must release the lock and fail it,
                // rather than alternating predecessors until the runtime stops polling.
                Some(invalid("the command prerequisites contain a cycle"))
            } else {
                match self.prerequisite(&front.cmd) {
                    Prerequisite::Ready => None,
                    Prerequisite::Failed(error) => Some(error),
                    Prerequisite::Waiting { id, .. } => {
                        if let Some(position) = queue.iter().position(|queued| queued.cmd.id == id)
                        {
                            // Reconnect may replay the dependent first. Run its predecessor first,
                            // preserving each command's admission, cancellation and journal entry.
                            if let Some(prior) = queue.remove(position) {
                                queue.push_front(prior);
                            }
                            continue;
                        }
                        return Next::Wait(front.cmd.authority_until);
                    }
                }
            };
            let Some(queued) = queue.pop_front() else {
                continue;
            };
            if let Some(hand) = state.in_hand.get_mut(&queued.cmd.id) {
                hand.queued_under = None;
            }
            return Next::Run(Box::new(queued), failure);
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
                Prerequisite::Waiting { .. } => {
                    tokio::select! {
                        () = cancel.cancelled() => {}
                        () = &mut changed => {}
                        () = self.wait_for_expiry(command.authority_until) => return Err(too_late()),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_link_contract::commands::SessionClose;
    use orch_types::{ManualClock, new_id};

    #[test]
    fn a_cycle_received_before_journal_flush_yields_a_failure_instead_of_spinning()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let now: Time = "2026-10-16T00:00:00Z".parse()?;
        let server = CommandServer::new(
            Journal::open(directory.path(), Duration::from_secs(3600), now)?,
            Arc::new(ManualClock::new(now.get())),
            ServerOptions {
                roles: vec![Role::Agent],
                connected: true,
            },
        );
        let thread = new_id();
        let key = SerialKey::Thread(thread);
        let mut first = Cmd::new(
            new_id(),
            &key,
            Duration::from_secs(30),
            &SessionClose { thread_id: thread },
        )?;
        let mut second = Cmd::new(
            new_id(),
            &key,
            Duration::from_secs(30),
            &SessionClose { thread_id: thread },
        )?;
        first.after = Some(second.id);
        second.after = Some(first.id);
        let key = key.wire().ok_or("the actual thread key is absent")?;
        let (first_write, first_written) = oneshot::channel();
        let (second_write, second_written) = oneshot::channel();
        {
            let mut state = lock(&server.inner.state);
            state.queues.entry(key.clone()).or_default().extend([
                Queued {
                    cmd: first,
                    admitted: first_written,
                },
                Queued {
                    cmd: second,
                    admitted: second_written,
                },
            ]);
        }
        // Keep the journal empty and both admission receipts unresolved. Run the
        // synchronous selector off the timeout's thread so a regression fails in
        // fixture patience rather than blocking the entire async runtime.
        let (found, answer) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let failed = matches!(server.next_queued(&key), Next::Run(_,Some(error))
                if error.detail.contains("cycle"));
            let _ = found.send(failed);
        });
        assert!(
            answer
                .recv_timeout(Duration::from_secs(30))
                .map_err(|_| "queue selection spun while journal admission was pending")?
        );
        worker
            .join()
            .map_err(|_| "the queue-selection regression worker panicked")?;
        drop((first_write, second_write));
        Ok(())
    }
}
