//! Prepare independent commands together without delaying cancellation or stream acknowledgements.

use super::*;
use std::collections::VecDeque;

#[derive(Clone, PartialEq, Eq, Hash)]
enum Line {
    Key(String),
    Command(Id),
}

impl Line {
    fn of(command: &orch_link_contract::commands::Cmd) -> Self {
        command
            .key
            .as_ref()
            .map_or_else(|| Self::Command(command.id), |key| Self::Key(key.clone()))
    }
}

struct Pending {
    command: orch_link_contract::commands::Cmd,
    #[cfg(feature = "test-observers")]
    flight: orch_link::test_observers::Flight,
}

impl Link {
    pub(super) async fn down_loop(
        &self,
        entry: Arc<MachineEntry>,
        mut down: mpsc::UnboundedReceiver<DownFrame>,
        #[cfg(feature = "test-observers")] observed: orch_link::test_observers::ReceiverLifetime,
    ) {
        let mut pending = VecDeque::<Pending>::new();
        let mut active = HashSet::<Line>::new();
        let mut preparing = futures::stream::FuturesUnordered::<BoxFuture<'static, Line>>::new();
        loop {
            while preparing.len() < entry.command_preparations {
                let Some(position) = pending
                    .iter()
                    .position(|waiting| !active.contains(&Line::of(&waiting.command)))
                else {
                    break;
                };
                let Some(waiting) = pending.remove(position) else {
                    continue;
                };
                let Some(route) = entry.route() else {
                    #[cfg(feature = "test-observers")]
                    waiting.flight.processed();
                    continue;
                };
                let line = Line::of(&waiting.command);
                active.insert(line.clone());
                let link = self.clone();
                let recipient = Arc::clone(&entry);
                preparing.push(Box::pin(async move {
                    link.publish_command(recipient, route, waiting).await;
                    line
                }));
            }
            tokio::select! {
                // A busy command line cannot prevent Stop or another control frame being read.
                () = entry.retired.cancelled() => return,
                () = self.inner.shutdown.cancelled() => return,
                completed = preparing.next(), if !preparing.is_empty() => {
                    if let Some(line) = completed {
                        active.remove(&line);
                    }
                }
                frame = down.recv() => {
                    let Some(frame) = frame else { return };
                    #[cfg(feature = "test-observers")]
                    let flight = observed.selected(
                        frames::encode(&frame, usize::MAX).ok().map(|text| text.len()),
                    );
                    match frame {
                        DownFrame::Cmd(command) => pending.push_back(Pending {
                            command,
                            #[cfg(feature = "test-observers")]
                            flight,
                        }),
                        control => {
                            let Some(route) = entry.route() else {
                                #[cfg(feature = "test-observers")]
                                flight.processed();
                                continue;
                            };
                            let sent = self.send_current(&entry, &route, control, &[]);
                            #[cfg(feature = "test-observers")]
                            if matches!(sent, Ok(true)) {
                                flight.complete();
                            } else {
                                flight.processed();
                            }
                            #[cfg(not(feature = "test-observers"))]
                            let _ = sent;
                        }
                    }
                }
            }
        }
    }

    async fn publish_command(
        &self,
        entry: Arc<MachineEntry>,
        route: SessionRoute,
        waiting: Pending,
    ) {
        let Pending {
            command,
            #[cfg(feature = "test-observers")]
            flight,
        } = waiting;
        let id = command.id;
        let original = command.clone();
        let prepared = self
            .prepare(&entry, route.fingerprint.as_deref(), command)
            .await;
        let admitted = match prepared {
            Ok(command) => self.admit_wire_command(&entry, &original, command).await,
            Err(error) => Err(error),
        };
        let (command, callers) = match admitted {
            Ok(Some(command)) => command,
            Ok(None) => {
                #[cfg(feature = "test-observers")]
                flight.processed();
                return;
            }
            Err(error) => {
                tracing::warn!(%error, command = %id, "a command cannot be put on the wire");
                self.refuse_unaccepted_publication(&entry, id, &error);
                #[cfg(any(test, feature = "test-support"))]
                command_admission_tests::local_refused(id).await;
                #[cfg(feature = "test-observers")]
                flight.processed();
                return;
            }
        };
        // The last check and publication stay synchronous, with no database lease.
        // Preparing on another line never grants new route or caller authority.
        let sent = self.send_current(&entry, &route, DownFrame::Cmd(command), &callers);
        if let Err(error) = &sent {
            self.refuse_unaccepted_publication(&entry, id, error);
        }
        #[cfg(feature = "test-observers")]
        if matches!(sent, Ok(true)) {
            flight.complete();
        } else {
            flight.processed();
        }
    }
}
