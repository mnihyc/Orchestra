//! A machine's journal keeps the results of a day of commands on its disk, not in its memory: a connector that
//! lists a provider's models every hour holds no more after a day of listings than after the first, and writes
//! each down holding one copy of it beside the one it was given.
#![allow(
    unsafe_code,
    reason = "the case counts the bytes the process holds through the allocator interface"
)]
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use chrono::TimeZone;
use orch_link::journal::Journal;
use orch_link_contract::commands::{Cmd, ProviderDiscoverModels, SerialKey};
use orch_types::{Json, Time, new_id};
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// The system's allocator, counting the bytes it holds for this process.
struct Counting;

static HELD: AtomicUsize = AtomicUsize::new(0);
/// The most held at once since it was last set.
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// The count is the whole process's: the cases take turns.
static ALONE: Mutex<()> = Mutex::new(());

fn grew(by: usize) {
    let now = HELD.fetch_add(by, Ordering::Relaxed) + by;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

// SAFETY: every call goes to the system allocator unchanged; the count only adds and subtracts the sizes of the
// layouts the calls name.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's layout, as the caller gave it.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            grew(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's layout, as the caller gave it.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            grew(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: a pointer this allocator gave, with the layout it was given for.
        unsafe { System.dealloc(pointer, layout) };
        HELD.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: a pointer this allocator gave, with the layout it was given for, and the caller's new size.
        let moved = unsafe { System.realloc(pointer, layout, size) };
        if !moved.is_null() {
            grew(size);
            HELD.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn held() -> usize {
    HELD.load(Ordering::Relaxed)
}

/// An hour's answer to `provider.discover_models`: a vendor's models, each with the search tool its endpoint
/// offers, as a connector lists them.
fn listing(hour: usize) -> Json {
    let tool = json!({
        "request": {"container": "/plugins", "paths": ["/chat/completions"], "modes": {"live": {}},
            "options_schema": {"type": "object", "additionalProperties": false, "properties": {
                "engine": {"type": "string", "enum": ["auto", "native", "exa", "firecrawl", "parallel", "perplexity"]},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 25, "default": 5},
                "search_context_size": {"type": "string", "enum": ["low", "medium", "high"]},
                "allowed_domains": {"type": "array", "items": {"type": "string"}},
                "excluded_domains": {"type": "array", "items": {"type": "string"}}}}},
        "result": {"unique_by": "/url", "collections": (0..6).map(|n| json!({"pointer": format!("/choices/*/message/annotations/{n}"),
            "optional": true, "item": {"url": {"from_result": ["/url_citation/url", "/url"]},
                "title": {"from_result": ["/url_citation/title", "/title"]}}})).collect::<Vec<_>>()},
        "prices": {"per_call": "0.005", "included_results_per_call": 10, "per_additional_result": "0.001"},
    });
    json!({
        "source": {"supported": true},
        "suggestions": (0..200).map(|n| json!({
            "model_name": format!("vendor/model-{n}"),
            "display_name": format!("Model {n}, listed at hour {hour}"),
            "parameters": {"native_tools": {"openai-chat": {"web_search": tool.clone()}}},
        })).collect::<Vec<_>>(),
    })
}

/// The command that asks for an hour's listing.
fn listing_command(now: Time) -> Cmd {
    Cmd::new(
        new_id(),
        &SerialKey::None,
        Time::new(now.get() + chrono::Duration::minutes(5)),
        &ProviderDiscoverModels {
            max_report_bytes: 16 << 20,
            provider_id: new_id(),
            models: None,
        },
    )
    .unwrap()
}

#[test]
fn a_day_of_hourly_model_listings_leaves_the_journal_holding_no_more_than_after_the_first() {
    let _alone = ALONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = tempfile::tempdir().unwrap();
    let start = Time::new(chrono::Utc.with_ymd_and_hms(2026, 11, 22, 5, 13, 0).unwrap());
    let mut journal = Journal::open(directory.path(), Duration::from_secs(86_400), start).unwrap();
    let one = {
        let before = held();
        let listed = listing(0);
        let size = held() - before;
        drop(listed);
        size
    };
    let mut ids = Vec::new();
    let mut after_first = 0;
    for hour in 0..24 {
        let now = Time::new(start.get() + chrono::Duration::hours(hour));
        let command = listing_command(now);
        journal.accept(&command, now).unwrap();
        journal.started(command.id).unwrap();
        let answered = journal
            .finish(command.id, Ok(listing(usize::try_from(hour).unwrap())))
            .unwrap();
        drop(answered);
        ids.push(command.id);
        if hour == 0 {
            after_first = held();
        }
    }
    let grown = held().saturating_sub(after_first);
    assert!(
        grown < one / 4,
        "after a day of hourly listings the journal holds {grown} bytes more than after the first; one listing is \
         {one} bytes"
    );
    // Each is still answered whole when it is asked for again, from what was written down.
    for (hour, id) in ids.iter().enumerate() {
        let entry = journal.get(*id).unwrap();
        assert_eq!(
            entry.result().map(|result| &result.outcome),
            Some(&Ok(listing(hour)))
        );
    }
}

#[test]
fn a_listing_is_written_down_with_one_copy_of_it_held_beside_it() {
    let _alone = ALONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = tempfile::tempdir().unwrap();
    let now = Time::new(chrono::Utc.with_ymd_and_hms(2026, 11, 22, 5, 13, 0).unwrap());
    let mut journal = Journal::open(directory.path(), Duration::from_secs(86_400), now).unwrap();
    let command = listing_command(now);
    journal.accept(&command, now).unwrap();
    journal.started(command.id).unwrap();
    let listed = listing(0);
    let written = serde_json::to_vec(&listed).unwrap().len();
    let one = {
        let before = held();
        let copy = listed.clone();
        let size = held() - before;
        drop(copy);
        size
    };
    let before = held();
    PEAK.store(before, Ordering::Relaxed);
    let answered = journal.finish(command.id, Ok(listed)).unwrap();
    let peak = PEAK.load(Ordering::Relaxed) - before;
    drop(answered);
    // The staged entry is the one copy; the bytes written are the file's, grown as a vector grows.
    assert!(
        peak <= one + 2 * written + (64 << 10),
        "writing a listing down held {peak} bytes more at once; one copy of it is {one} bytes and its file {written}"
    );
}
