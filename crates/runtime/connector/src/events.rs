//! What an adapter reports, as events of the link: which stream each goes on, and how the text that grows is bundled.
//!
//! An adapter says what happens in the neutral vocabulary of the agent contract. The link carries the
//! same vocabulary as events of streams. [`convert`] says which stream an event of the adapter goes on
//! (a session's, the harness process's, the provider's) and gives it the form the stream carries.
//! [`coalesce`] bundles the text of items that grows: deltas of one item that wait together in the
//! adapter's queue and follow each other are one delta, so that a harness that streams a word at a time
//! is not a word at a time on disk and on the wire. Nothing is held back to wait for more, and nothing
//! changes its place: a delta is merged only into the one before it.

use async_trait::async_trait;
use orch_agent_contract::adapter::AdapterEvent;
use orch_agent_contract::events::{ItemDelta, SessionEvent};
use orch_agent_contract::facts::HarnessStatus;
use orch_link::machine::MachineLink;
use orch_link::streams::PublishError;
use orch_link_contract::events::{self as link, Event};
use orch_link_contract::streams::Stream;
use orch_types::{Id, Json};

/// The most data a merged delta holds: sixteen kibibytes, a sixteenth of a frame at the largest frame
/// the link allows, so that one run of text never fills a frame.
pub const MAX_MERGED_DELTA_BYTES: usize = 16 * 1024;

/// Somewhere events are put. The link's spool is one; a test can be another.
#[async_trait]
pub trait Publisher: Send + Sync {
    /// Puts a durable event on a stream and returns once it is kept.
    async fn publish(&self, stream: Stream, event: Event) -> Result<(), PublishError>;
    /// Puts events on their streams in their order and returns once all are kept, each one's answer in
    /// its place. As given here one after the other; the link hands all over before it waits (C-turns).
    async fn publish_all(&self, events: Vec<(Stream, Event)>) -> Vec<Result<(), PublishError>> {
        let mut answers = Vec::with_capacity(events.len());
        for (stream, event) in events {
            answers.push(self.publish(stream, event).await);
        }
        answers
    }
    /// Raises a durable raw allocator's floor; a publisher without a spool has no allocator to raise.
    async fn raw_after(&self, _stream: Stream, _after: i64) -> Result<(), PublishError> {
        Ok(())
    }
}

#[async_trait]
impl Publisher for MachineLink {
    async fn raw_after(&self, stream: Stream, after: i64) -> Result<(), PublishError> {
        MachineLink::raw_after(self, &stream, after).await
    }
    async fn publish(&self, stream: Stream, event: Event) -> Result<(), PublishError> {
        MachineLink::publish(self, &stream, &event).await
    }
    async fn publish_all(&self, events: Vec<(Stream, Event)>) -> Vec<Result<(), PublishError>> {
        // All handed to the link first, in their order; then each answer (C-turns).
        let handed: Vec<_> = events
            .iter()
            .map(|(stream, event)| MachineLink::publish_now(self, stream, event))
            .collect();
        let mut answers = Vec::with_capacity(handed.len());
        for answer in handed {
            answers.push(answer.await);
        }
        answers
    }
}

/// Why an event of an adapter could not be given to the link.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    /// The event could not be written as JSON.
    #[error("the event cannot be written: {0}")]
    Write(#[from] serde_json::Error),
    /// The link does not know the event as the adapter made it.
    #[error("the link does not take the event: {0}")]
    Shape(#[from] orch_link_contract::commands::CatalogueError),
}

/// The stream an event of the harness process of `provider_id` and `agent_id` goes on, and the event as
/// the link carries it.
pub fn convert(
    provider_id: Id,
    agent_id: Id,
    event: &AdapterEvent,
) -> Result<(Stream, Event), ConvertError> {
    Ok(match event {
        AdapterEvent::ModelRequest(report) => (
            Stream::Session {
                thread_id: report.thread_id,
                segment_id: report.segment_id,
            },
            Event::ModelRequest(orch_link_contract::events::ModelRequest {
                turn_id: report.turn_id,
                request: report.request.clone(),
            }),
        ),
        AdapterEvent::Session {
            thread_id,
            segment_id,
            event,
        } => {
            let written = serde_json::to_value(event)?;
            let kind = event.kind();
            let body = written.get("body").cloned().unwrap_or(Json::Null);
            (
                Stream::Session {
                    thread_id: *thread_id,
                    segment_id: *segment_id,
                },
                Event::decode(kind, body)?,
            )
        }
        AdapterEvent::Login(state) => (
            Stream::Provider { provider_id },
            Event::ProviderLogin(state.clone()),
        ),
        AdapterEvent::Facts(facts) => (
            Stream::Provider { provider_id },
            Event::ProviderFacts(facts.clone()),
        ),
        AdapterEvent::Descriptor(descriptor) => (
            Stream::Harness {
                provider_id,
                agent_id,
            },
            Event::HarnessDescriptor(descriptor.as_ref().clone()),
        ),
        AdapterEvent::Status(status) => (
            Stream::Harness {
                provider_id,
                agent_id,
            },
            Event::HarnessStatus(harness_status(status)),
        ),
    })
}

/// A status of the harness process in the form the stream carries.
#[must_use]
pub fn harness_status(status: &HarnessStatus) -> link::HarnessStatus {
    link::HarnessStatus {
        status: serde_json::to_value(status.status)
            .ok()
            .and_then(|state| state.as_str().map(str::to_owned))
            .unwrap_or_default(),
        detail: status.detail.clone(),
        harness_version: status.harness_version.clone(),
        sessions_open: i64::from(status.sessions_open),
    }
}

fn delta_of(event: &AdapterEvent) -> Option<(Id, Id, &ItemDelta)> {
    match event {
        AdapterEvent::Session {
            thread_id,
            segment_id,
            event: SessionEvent::ItemDelta(delta),
        } => Some((*thread_id, *segment_id, delta)),
        _ => None,
    }
}

/// Merges the deltas of the events that wait together: a delta joins the one before it when both are of
/// the same item and field of the same session and it starts where the one before ends, as long as the
/// merged data stays within [`MAX_MERGED_DELTA_BYTES`]. Every other event stays where it is.
#[must_use]
pub fn coalesce(batch: Vec<AdapterEvent>) -> Vec<AdapterEvent> {
    let mut merged: Vec<AdapterEvent> = Vec::with_capacity(batch.len());
    for event in batch {
        let joined = match (merged.last_mut(), delta_of(&event)) {
            (
                Some(AdapterEvent::Session {
                    thread_id,
                    segment_id,
                    event: SessionEvent::ItemDelta(before),
                }),
                Some((thread, segment, delta)),
            ) if *thread_id == thread
                && *segment_id == segment
                && before.native_ref == delta.native_ref
                && before.field == delta.field
                && before.offset + before.len == delta.offset
                && before.data.len() + delta.data.len() <= MAX_MERGED_DELTA_BYTES =>
            {
                before.data.push_str(&delta.data);
                before.len += delta.len;
                true
            }
            _ => false,
        };
        if !joined {
            merged.push(event);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::events::{DeltaField, TurnCompleted, TurnOutcome};
    use orch_types::new_id;

    fn delta(thread: Id, segment: Id, item: &str, offset: i64, data: &str) -> AdapterEvent {
        AdapterEvent::Session {
            thread_id: thread,
            segment_id: segment,
            event: SessionEvent::ItemDelta(ItemDelta::text(item, DeltaField::Text, offset, data)),
        }
    }

    fn data_of(event: &AdapterEvent) -> (&str, i64, i64) {
        let (_, _, delta) = delta_of(event).unwrap();
        (delta.data.as_str(), delta.offset, delta.len)
    }

    #[test]
    fn deltas_that_wait_together_and_follow_each_other_become_one() {
        let (thread, segment) = (new_id(), new_id());
        let merged = coalesce(vec![
            delta(thread, segment, "item_1", 0, "Hel"),
            delta(thread, segment, "item_1", 3, "lo, "),
            delta(thread, segment, "item_1", 7, "wörld"),
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(data_of(&merged[0]), ("Hello, wörld", 0, 12));
    }

    #[test]
    fn a_delta_is_not_merged_across_another_event_another_item_or_a_gap() {
        let (thread, segment) = (new_id(), new_id());
        let completed = AdapterEvent::Session {
            thread_id: thread,
            segment_id: segment,
            event: SessionEvent::TurnCompleted(TurnCompleted {
                turn_id: new_id(),
                status: TurnOutcome::Completed,
                error: None,
            }),
        };
        let merged = coalesce(vec![
            delta(thread, segment, "item_1", 0, "a"),
            delta(thread, segment, "item_2", 0, "b"),
            delta(thread, segment, "item_2", 5, "gap"),
            delta(thread, segment, "item_2", 8, "c"),
            completed,
            delta(thread, segment, "item_2", 9, "d"),
            delta(thread, new_id(), "item_2", 10, "other session"),
        ]);
        let shapes: Vec<_> = merged
            .iter()
            .map(|event| delta_of(event).map(|(_, _, d)| d.data.clone()))
            .collect();
        assert_eq!(
            shapes,
            [
                Some("a".into()),
                Some("b".into()),
                Some("gapc".into()),
                None,
                Some("d".into()),
                Some("other session".into())
            ]
        );
    }

    #[test]
    fn a_merged_delta_never_passes_the_bound() {
        let (thread, segment) = (new_id(), new_id());
        let piece = "x".repeat(MAX_MERGED_DELTA_BYTES / 2 + 1);
        let merged = coalesce(vec![
            delta(thread, segment, "item_1", 0, &piece),
            delta(
                thread,
                segment,
                "item_1",
                i64::try_from(piece.len()).unwrap(),
                &piece,
            ),
        ]);
        assert_eq!(
            merged.len(),
            2,
            "two halves of the bound and a byte do not fit in one"
        );
    }

    #[test]
    fn a_session_event_goes_on_the_stream_of_its_session_in_the_form_the_link_carries() {
        let (provider, agent, thread, segment) = (new_id(), new_id(), new_id(), new_id());
        let event = delta(thread, segment, "item_1", 0, "Hi");
        let (stream, converted) = convert(provider, agent, &event).unwrap();
        assert_eq!(
            stream,
            Stream::Session {
                thread_id: thread,
                segment_id: segment
            }
        );
        let Event::ItemDelta(body) = converted else {
            panic!("{converted:?}")
        };
        assert_eq!(
            (body.native_ref.as_str(), body.offset, body.len),
            ("item_1", 0, 2)
        );
    }

    #[test]
    fn the_other_events_go_to_the_provider_and_to_the_harness_process() {
        let (provider, agent) = (new_id(), new_id());
        let login = AdapterEvent::Login(orch_agent_contract::login::LoginState::idle());
        assert_eq!(
            convert(provider, agent, &login).unwrap().0,
            Stream::Provider {
                provider_id: provider
            }
        );
        let status = AdapterEvent::Status(HarnessStatus {
            status: orch_agent_contract::facts::HarnessState::Error,
            detail: Some("it died".into()),
            harness_version: None,
            sessions_open: 2,
        });
        let (stream, event) = convert(provider, agent, &status).unwrap();
        assert_eq!(
            stream,
            Stream::Harness {
                provider_id: provider,
                agent_id: agent
            }
        );
        let Event::HarnessStatus(body) = event else {
            panic!()
        };
        assert_eq!((body.status.as_str(), body.sessions_open), ("error", 2));
    }
}
