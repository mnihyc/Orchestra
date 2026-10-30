//! The scripted counterparts a suite drives an implementation against: a controller, an upstream, an environment.
//!
//! A controller consumes the contract's events rather than harness frames. The same checks therefore
//! apply to a built-in adapter and a program behind a bridge, with a scripted or a real model.

use std::time::Duration;

use futures::StreamExt;
use orch_agent_contract::adapter::{AdapterEvent, EventSink, EventStream};
use orch_agent_contract::events::{SessionEvent, TurnOutcome};
use orch_types::Id;

use crate::report::{Checked, Miss};

/// The observable stream of a harness, with all events retained for an assertion or evidence record.
pub struct Controller {
    stream: EventStream,
    /// Events received, in their original order and with their session routing.
    pub events: Vec<AdapterEvent>,
}

impl Controller {
    /// Makes the sink an adapter receives and the controller that observes it.
    #[must_use]
    pub fn new() -> (EventSink, Self) {
        let (sink, stream) = EventSink::channel();
        (
            sink,
            Self {
                stream,
                events: Vec::new(),
            },
        )
    }

    /// Waits for one turn to finish, checking its session, catalogue round-trip and start/end ordering.
    pub async fn completed(
        &mut self,
        thread: Id,
        segment: Id,
        turn: Id,
        within: Duration,
    ) -> Checked {
        tokio::time::timeout(within, self.until_completed(thread, segment, turn))
            .await
            .unwrap_or_else(|_| {
                Err(Miss::Failed(
                    "the turn never reported its completion".into(),
                ))
            })
    }

    async fn until_completed(&mut self, thread: Id, segment: Id, turn: Id) -> Checked {
        let mut began = false;
        while let Some(event) = self.stream.next().await {
            let (name, params) = event.to_wire().map_err(|error| error.to_string())?;
            let roundtrip =
                AdapterEvent::from_wire(name, params).map_err(|error| error.to_string())?;
            if roundtrip != event {
                return Err("the event cannot round-trip through the adapter catalogue".into());
            }
            self.events.push(event.clone());
            if let AdapterEvent::Session {
                thread_id,
                segment_id,
                event,
            } = event
            {
                if thread_id != thread || segment_id != segment {
                    return Err("an event was routed to another thread or segment".into());
                }
                match event {
                    SessionEvent::TurnStarted(started) => {
                        if started.turn_id != turn || began {
                            return Err(
                                "the adapter started another turn or started this turn twice"
                                    .into(),
                            );
                        }
                        began = true;
                    }
                    SessionEvent::TurnCompleted(completed) => {
                        if !began || completed.turn_id != turn {
                            return Err("completion did not follow the matching turn start".into());
                        }
                        if completed.status != TurnOutcome::Completed {
                            return Err(format!(
                                "the controlled turn ended {:?}",
                                completed.status
                            )
                            .into());
                        }
                        return Ok(());
                    }
                    SessionEvent::SessionLost(_) => {
                        return Err("the controlled session was lost".into());
                    }
                    _ => {}
                }
            }
        }
        Err("the adapter closed its stream before completing the turn".into())
    }
}
