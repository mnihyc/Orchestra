//! Volatile bounded native snapshots whose active transfers renew their idle deadline.

use orch_link_contract::background::{
    COMMAND_PREVIEW_BYTES, NativeBackground, PAGE_BYTES, SNAPSHOT_COMMANDS,
};
use orch_link_contract::commands::{CommandError, ErrorCode};
use orch_types::Id;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Thirty idle seconds bound use of an old native list while allowing active page transfers.
const IDLE: Duration = Duration::from_secs(30);
/// Sixteen maximum-size snapshots bound this connector's temporary roster memory.
const REVIEWS: usize = 16;
/// Replies use the person's largest roster page size.
const PAGE: usize = 200;
struct Held {
    thread: Id,
    segment: Id,
    until: Instant,
    value: NativeBackground,
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
    ) -> Result<NativeBackground, CommandError> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        held.retain(|_, review| review.until > now);
        let (reference, offset) = if let Some(cursor) = cursor {
            let (reference, offset) = cursor.rsplit_once('/').ok_or_else(stale)?;
            (
                reference.to_owned(),
                offset.parse::<usize>().map_err(|_| stale())?,
            )
        } else {
            if held.len() >= REVIEWS {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "too many native snapshots are being read; finish an earlier transfer",
                ));
            }
            let reference = orch_types::new_id().to_string();
            let mut value = initial.ok_or_else(stale)?;
            if value.items.len() > SNAPSHOT_COMMANDS {
                value.items.truncate(SNAPSHOT_COMMANDS);
                value.total = None;
                value.reason =
                    Some("the native list is partial; narrow it in the native program".to_owned());
            }
            for command in &mut value.items {
                if let Some(preview) = &mut command.command {
                    command.command_truncated |= shorten(preview, COMMAND_PREVIEW_BYTES);
                }
            }
            held.insert(
                reference.clone(),
                Held {
                    thread,
                    segment,
                    until: now + IDLE,
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
        review.until = now + IDLE;
        let mut page = review.value.clone();
        page.items.clear();
        page.next_cursor = None;
        let mut end = offset;
        while end < review.value.items.len() && end - offset < PAGE {
            page.items.push(review.value.items[end].clone());
            if serde_json::to_vec(&page)
                .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?
                .len()
                > PAGE_BYTES / 2
            {
                if page.items.len() == 1 {
                    return Err(CommandError::new(
                        ErrorCode::Internal,
                        "one native command exceeds the reply bound",
                    ));
                }
                page.items.pop();
                break;
            }
            end += 1;
        }
        if end < review.value.items.len() {
            page.next_cursor = Some(format!("{reference}/{end}"));
        } else {
            held.remove(&reference);
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
            )
            .unwrap();
        let mut count = 0;
        loop {
            assert!(serde_json::to_vec(&page).unwrap().len() <= PAGE_BYTES);
            assert_eq!(page.total, Some(250));
            assert!(
                page.items
                    .iter()
                    .all(|command| command.command.as_ref().unwrap().len()
                        <= COMMAND_PREVIEW_BYTES
                        && command.command_truncated)
            );
            count += page.items.len();
            let Some(cursor) = page.next_cursor else {
                break;
            };
            assert!(
                reads
                    .page(orch_types::new_id(), segment, Some(&cursor), None)
                    .is_err()
            );
            page = reads.page(thread, segment, Some(&cursor), None).unwrap();
        }
        assert_eq!(count, 250);
    }
}
