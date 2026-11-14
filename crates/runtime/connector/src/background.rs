//! Volatile bounded native snapshots whose active transfers renew their idle deadline.

use orch_link_contract::background::{BackgroundReadLimits, NativeBackground};
use orch_link_contract::commands::{CommandError, ErrorCode};
use orch_types::Id;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

struct Held {
    thread: Id,
    segment: Id,
    until: Option<Instant>,
    value: NativeBackground,
    idle: Duration,
}

/// Native snapshots never enter the connector ledger, spool or command journal.
#[derive(Default)]
pub struct Reads {
    held: Mutex<HashMap<String, Held>>,
}
fn stale() -> CommandError {
    CommandError::new(
        ErrorCode::Conflict,
        "the native snapshot expired or its session changed; read it again",
    )
}
fn shorten(text: &mut String, most: usize) -> bool {
    let mut end = text.len().min(most);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let shortened = end < text.len();
    text.truncate(end);
    shortened
}

impl Reads {
    /// Transfers one bounded page of the same exact thread and native generation.
    pub fn page(
        &self,
        thread: Id,
        segment: Id,
        cursor: Option<&str>,
        initial: Option<NativeBackground>,
        limits: BackgroundReadLimits,
    ) -> Result<NativeBackground, CommandError> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        held.retain(|_, review| review.until.is_none_or(|until| until > now));
        let (reference, offset) = if let Some(cursor) = cursor {
            let (reference, offset) = cursor.rsplit_once('/').ok_or_else(stale)?;
            (
                reference.to_owned(),
                offset.parse::<usize>().map_err(|_| stale())?,
            )
        } else {
            let reference = orch_types::new_id().to_string();
            let mut value = initial.ok_or_else(stale)?;
            for command in &mut value.items {
                if let Some(preview) = &mut command.command {
                    command.command_truncated |= shorten(
                        preview,
                        usize::try_from(limits.preview_bytes).unwrap_or(usize::MAX),
                    );
                }
            }
            held.insert(
                reference.clone(),
                Held {
                    thread,
                    segment,
                    until: now.checked_add(Duration::from_secs(limits.idle_seconds.max(1))),
                    idle: Duration::from_secs(limits.idle_seconds.max(1)),
                    value,
                },
            );
            (reference, 0)
        };
        let review = held.get_mut(&reference).ok_or_else(stale)?;
        if review.thread != thread || review.segment != segment || offset > review.value.items.len()
        {
            return Err(stale());
        }
        review.until = now.checked_add(review.idle);
        let mut page = NativeBackground {
            supported: review.value.supported,
            as_of: review.value.as_of,
            state: review.value.state,
            reason: review.value.reason.clone(),
            items: Vec::new(),
            next_cursor: None,
            total: review.value.total,
        };
        let encode = |value: &NativeBackground| {
            serde_json::to_vec(value)
                .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))
        };
        let mut bytes = encode(&page)?
            .len()
            .saturating_add(reference.len())
            .saturating_add(usize::MAX.to_string().len() + 3);
        let target = usize::try_from(limits.page_bytes).unwrap_or(usize::MAX);
        let mut end = offset;
        while end < review.value.items.len()
            && end - offset < usize::try_from(limits.page_items.max(1)).unwrap_or(usize::MAX)
        {
            let command = &review.value.items[end];
            let size = serde_json::to_vec(command)
                .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?
                .len()
                .saturating_add(1);
            if !page.items.is_empty() && bytes.saturating_add(size) > target {
                break;
            }
            bytes = bytes.saturating_add(size);
            page.items.push(command.clone());
            end += 1;
        }
        if end < review.value.items.len() {
            page.next_cursor = Some(format!("{reference}/{end}"));
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_link_contract::background::{
        BackgroundPlace, BackgroundState, NativeBackgroundCommand,
    };
    fn roster(count: usize) -> NativeBackground {
        NativeBackground {
            supported: true,
            as_of: None,
            state: BackgroundState::Live,
            reason: None,
            items: (0..count)
                .map(|index| NativeBackgroundCommand {
                    id: index.to_string(),
                    native_item_ref: None,
                    turn_id: None,
                    command: Some("ordinary".into()),
                    command_truncated: false,
                    cwd: None,
                    place: BackgroundPlace::Environment,
                    since: None,
                    cpu_percent: None,
                    memory_bytes: None,
                })
                .collect(),
            next_cursor: None,
            total: Some(count as i64),
        }
    }
    #[test]
    fn seventeen_simultaneous_readers_each_receive_the_entire_roster_without_releasing_another() {
        let reads = Reads::default();
        let mut readers = Vec::new();
        for _ in 0..17 {
            let thread = orch_types::new_id();
            let segment = orch_types::new_id();
            let page = reads
                .page(
                    thread,
                    segment,
                    None,
                    Some(roster(2_501)),
                    BackgroundReadLimits::default(),
                )
                .unwrap();
            readers.push((thread, segment, page));
        }
        for (thread, segment, mut page) in readers {
            let mut ids = std::collections::HashSet::new();
            loop {
                assert_eq!(page.total, Some(2_501));
                for command in page.items {
                    assert!(ids.insert(command.id));
                }
                let Some(cursor) = page.next_cursor else {
                    break;
                };
                page = reads
                    .page(
                        thread,
                        segment,
                        Some(&cursor),
                        None,
                        BackgroundReadLimits::default(),
                    )
                    .unwrap();
            }
            assert_eq!(ids.len(), 2_501);
        }
    }

    #[test]
    fn a_large_utf8_roster_transfers_in_bounded_pages_without_changing_its_captured_count() {
        let reads = Reads::default();
        let thread = orch_types::new_id();
        let segment = orch_types::new_id();
        let items = (0..250)
            .map(|index| NativeBackgroundCommand {
                id: index.to_string(),
                native_item_ref: None,
                turn_id: None,
                command: Some("字".repeat(3_000)),
                command_truncated: false,
                cwd: None,
                place: BackgroundPlace::Environment,
                since: None,
                cpu_percent: None,
                memory_bytes: None,
            })
            .collect();
        let mut page = reads
            .page(
                thread,
                segment,
                None,
                Some(NativeBackground {
                    supported: true,
                    as_of: None,
                    state: BackgroundState::Live,
                    reason: None,
                    items,
                    next_cursor: None,
                    total: Some(250),
                }),
                BackgroundReadLimits::default(),
            )
            .unwrap();
        let mut count = 0;
        loop {
            assert!(
                serde_json::to_vec(&page).unwrap().len()
                    <= BackgroundReadLimits::default().page_bytes as usize
            );
            assert_eq!(page.total, Some(250));
            assert!(
                page.items
                    .iter()
                    .all(|command| command.command.as_ref().unwrap().len()
                        <= BackgroundReadLimits::default().preview_bytes as usize
                        && command.command_truncated)
            );
            count += page.items.len();
            let Some(cursor) = page.next_cursor else {
                break;
            };
            assert!(
                reads
                    .page(
                        orch_types::new_id(),
                        segment,
                        Some(&cursor),
                        None,
                        BackgroundReadLimits::default()
                    )
                    .is_err()
            );
            page = reads
                .page(
                    thread,
                    segment,
                    Some(&cursor),
                    None,
                    BackgroundReadLimits::default(),
                )
                .unwrap();
        }
        assert_eq!(count, 250);
    }
}
