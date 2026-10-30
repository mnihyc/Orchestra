//! The memory that an archive is written in does not grow with the archive: a thousand documents and a few large files go through a thread that makes the zip a few pieces at a time, so the peak of the process stays where a small archive leaves it. This is the only test of its binary, because the peak is a figure of the whole process.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use async_trait::async_trait;
use bytes::Bytes;
use common::World;
use futures::StreamExt;
use orch_api_contract::exports::{ExportEverything, ExportParts, ExportRequest, ExportWhat};
use orch_api_contract::threads::{ExportFormat, Thread};
use orch_authz::Caller;
use orch_blob_contract::store::{BlobError, BlobStore, ByteStream};
use orch_blob_local::LocalBlobStore;
use orch_db::audit::Origin;
use orch_engine::exports::{DocumentForm, DocumentHow, Forms, FormsFault};
use orch_types::{Hash, Id, new_id};
use serde_json::json;
use shared::{Places, chat_of};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A long message, large enough that 640 turns and messages are forty MiB of
/// plain text. The old whole-range producer needs several copies of that text.
const MESSAGE_BYTES: usize = 32 * 1024;

fn long_message(n: u64) -> String {
    use std::fmt::Write as _;
    let mut text = format!("history-{n:04}\n");
    for byte in pseudo(n, MESSAGE_BYTES / 2) {
        write!(text, "{byte:02x}").unwrap();
    }
    text
}

/// Completed fixture turns keep one real seed turn's model, input and payload
/// shape; only their permanent numbers and message text differ. No harness
/// history is accumulated by the test while measuring the export process.
async fn grow_history(world: &World, thread: Id, chat: Id, from: i32, to: i32) {
    for ordinal in from..=to {
        let turn = new_id();
        sqlx::query(
            "INSERT INTO turns (id, thread_id, segment_id, ordinal, state, input_id, model, params,
                                started_at, completed_at, seq_updated)
             SELECT $2, thread_id, segment_id, $3, 'completed', input_id, model, params,
                    started_at, completed_at, $4 FROM turns WHERE thread_id = $1 AND ordinal = 1",
        )
        .bind(thread)
        .bind(turn)
        .bind(ordinal)
        .bind(i64::from(ordinal) + 10_000)
        .execute(world.pool.pool())
        .await
        .unwrap();
        let text = long_message(u64::try_from(ordinal).unwrap());
        sqlx::query(
            "INSERT INTO items (id, thread_id, segment_id, turn_id, kind, status, payload, seq_created, seq_updated)
             SELECT $2, thread_id, segment_id, $3, kind, 'completed', jsonb_set(payload, '{text}', to_jsonb($4::text)), $5, $5
               FROM items WHERE thread_id = $1 AND kind = 'agent_message' ORDER BY seq_created LIMIT 1",
        ).bind(thread).bind(new_id()).bind(turn).bind(&text).bind(i64::from(ordinal) + 10_000)
            .execute(world.pool.pool()).await.unwrap();
        shared::say_in_chat(world, chat, world.ada, &text).await;
    }
    sqlx::query("UPDATE threads SET next_seq = GREATEST(next_seq, $2) WHERE node_id = $1")
        .bind(thread)
        .bind(i64::from(to) + 10_002)
        .execute(world.pool.pool())
        .await
        .unwrap();
}

async fn chat_peak(world: &World, chat: Id) -> u64 {
    forget_the_peak();
    let (before, _) = memory();
    let mut conn = world.pool.acquire().await.unwrap();
    let (path, _) = world
        .engine()
        .chat_with_files(&mut conn, chat, ExportFormat::Json, (None, None))
        .await
        .unwrap();
    let (_, peak) = memory();
    std::fs::remove_file(path).unwrap();
    peak.saturating_sub(before)
}

/// How many bytes a document is written as: as much as a long page of notes.
const DOCUMENT_BYTES: usize = 16 * 1024;

/// How large each of the files that a chat carries is. The tests are built without optimisation, where deflating and
/// hashing run a hundred times slower than they do in a release, so the archive is as large as it takes to tell its
/// size from the memory it was written in, and no larger.
const FILE_BYTES: usize = 12 * 1024 * 1024;

/// How long the writing of an archive may take before the test gives up.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(900);

/// How large a piece is that the tests' files are made and put in the store in.
const PIECE: usize = 64 * 1024;

/// Bytes that do not compress, the same for the same seed: a generator of the xorshift kind, which is quick enough
/// to make a hundred mebibytes in a moment.
fn pseudo(seed: u64, length: usize) -> Vec<u8> {
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(length);
    while out.len() < length {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(length);
    out
}

/// A document written as `DOCUMENT_BYTES` that differ from every other document's.
struct Heavy {
    written: AtomicU64,
}

#[async_trait]
impl Forms for Heavy {
    async fn thread(
        &self,
        _: &mut orch_db::Conn,
        _: &orch_authz::Principal,
        _: Id,
        _: chrono::DateTime<chrono::Utc>,
    ) -> Result<Thread, FormsFault> {
        Err(FormsFault(
            "no thread is written as records here".to_owned(),
        ))
    }

    async fn document(
        &self,
        _: &mut orch_db::Conn,
        _: &orch_authz::Principal,
        document: Id,
        _: DocumentHow,
        _: chrono::DateTime<chrono::Utc>,
    ) -> Result<DocumentForm, FormsFault> {
        let n = self.written.fetch_add(1, Ordering::SeqCst);
        let low = u64::try_from(document.as_u128() & u128::from(u64::MAX)).unwrap_or(0);
        Ok(DocumentForm {
            bytes: pseudo(low ^ n, DOCUMENT_BYTES),
            losses: String::new(),
        })
    }
}

/// The resident set of this process and its peak, in bytes, as the kernel says.
fn memory() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let field = |name: &str| -> u64 {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| {
                rest.trim()
                    .trim_end_matches("kB")
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .unwrap_or(0)
            * 1024
    };
    (field("VmRSS:"), field("VmHWM:"))
}

/// Starts the peak over, so that it is the peak of what follows.
fn forget_the_peak() {
    std::fs::write("/proc/self/clear_refs", "5").unwrap();
}

/// A file of `length` bytes put in the store a piece at a time, and the message of the chat that carries it.
async fn attach(world: &World, store: &LocalBlobStore, chat: Id, name: &str, seed: u64) {
    let hasher = {
        let mut hasher = Hash::hasher();
        let mut left = FILE_BYTES;
        let mut at = 0_u64;
        while left > 0 {
            let piece = pseudo(seed + at, PIECE.min(left));
            hasher.update(&piece);
            left -= piece.len();
            at += 1;
        }
        hasher
    };
    let hash = Hash::finish(&hasher);
    let stream: ByteStream =
        futures::stream::unfold((FILE_BYTES, 0_u64), move |(left, at)| async move {
            if left == 0 {
                return None;
            }
            let piece = Bytes::from(pseudo(seed + at, PIECE.min(left)));
            Some((
                Ok::<Bytes, BlobError>(piece.clone()),
                (left - piece.len(), at + 1),
            ))
        })
        .boxed();
    store
        .put(&hash, Some(i64::try_from(FILE_BYTES).unwrap()), stream)
        .await
        .unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let seq: i64 = sqlx::query_scalar(
        "UPDATE chats SET next_seq = next_seq + 1, last_message_at = now() WHERE id = $1 RETURNING next_seq - 1",
    )
    .bind(chat)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chat_messages (id, chat_id, kind, author_id, content, seq_created, seq_updated, search_text)
         VALUES ($1, $2, 'message', $3, $4, $5, $5, $6)",
    )
    .bind(new_id())
    .bind(chat)
    .bind(world.ada)
    .bind(json!([{
        "type": "attachment", "blob": hash.to_string(), "name": name,
        "size": FILE_BYTES, "media_type": "application/octet-stream"
    }]))
    .bind(seq)
    .bind(name)
    .execute(&mut *conn)
    .await
    .unwrap();
}

/// What the peak of the resident set rose by while an archive was written, in bytes, and the size of the archive.
async fn written(world: &World, store: &LocalBlobStore, request: &ExportRequest) -> (u64, i64) {
    let ada = Caller::session(world.ada);
    forget_the_peak();
    let (before, _) = memory();
    let made = world
        .engine()
        .make_export(&ada, &Origin::default(), world.root, request)
        .await
        .unwrap();
    let id = made.job.id;
    let started = std::time::Instant::now();
    loop {
        let mut conn = world.pool.acquire().await.unwrap();
        let open = orch_db::queries::jobs::job(&mut conn, id)
            .await
            .unwrap()
            .unwrap()
            .is_open();
        drop(conn);
        if !open {
            break;
        }
        assert!(
            started.elapsed() < PATIENCE,
            "gave up waiting for the archive"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let (_, peak) = memory();
    let mut conn = world.pool.acquire().await.unwrap();
    let row = orch_db::queries::exports::get(&mut conn, made.export.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "ready", "{:?}", row.left_out);
    let hash: Hash = row.blob.as_deref().unwrap().parse().unwrap();
    assert_eq!(store.size(&hash).await.unwrap(), row.bytes);
    (peak.saturating_sub(before), row.bytes.unwrap())
}

#[tokio::test]
async fn an_archive_of_a_thousand_documents_and_large_files_is_written_in_memory_that_does_not_grow_with_it()
 {
    let world = World::new().await;
    let root = std::env::temp_dir().join(format!("orch-export-memory-{}", new_id()));
    let store = Arc::new(LocalBlobStore::open(root.clone()).await.unwrap());
    world.engine().attach_blobs(store.clone());
    let heavy = Arc::new(Heavy {
        written: AtomicU64::new(0),
    });
    world.engine().attach_forms(heavy.clone());

    // The small archive: a quarter of the documents and one file.
    let small_topic = world.topic("Small").await;
    let chat = chat_of(&world, small_topic).await;
    for at in 0..250 {
        world
            .document_beside(small_topic, &format!("Small {at:04}"), &["words"])
            .await;
    }
    attach(&world, &store, chat, "one.bin", 1).await;
    let request = |files: bool| ExportRequest {
        what: ExportWhat::Everything(ExportEverything::All),
        parts: None,
        format: None,
        detail: None,
        files: Some(files),
        versions: None,
        comments: None,
        from: None,
        to: None,
        package: None,
    };
    // A first archive warms what is made once (the pool of connections, the allocator's arenas).
    let _ = written(&world, &store, &request(true)).await;
    let (small, small_bytes) = written(&world, &store, &request(true)).await;

    // The large one: a thousand documents in all, and four files.
    for at in 250..1_000 {
        world
            .document_beside(small_topic, &format!("Small {at:04}"), &["words"])
            .await;
    }
    for (n, name) in ["two.bin", "three.bin", "four.bin"].iter().enumerate() {
        attach(&world, &store, chat, name, 10 + n as u64).await;
    }
    let (large, large_bytes) = written(&world, &store, &request(true)).await;
    let mib = |bytes: u64| bytes as f64 / 1_048_576.0;
    let size = |bytes: i64| bytes as f64 / 1_048_576.0;
    eprintln!(
        "archive of 250 documents and 1 file: {:.1} MiB written, the peak of the process rose by {:.1} MiB",
        size(small_bytes),
        mib(small)
    );
    eprintln!(
        "archive of 1000 documents and 4 files: {:.1} MiB written, the peak of the process rose by {:.1} MiB",
        size(large_bytes),
        mib(large)
    );
    assert!(
        large_bytes > 3 * small_bytes,
        "the large archive is much larger: {large_bytes} against {small_bytes}"
    );
    assert!(
        mib(large) <= mib(small) + 16.0,
        "four times the archive does not take four times the memory: {:.1} MiB against {:.1} MiB",
        mib(large),
        mib(small)
    );
    assert!(
        mib(large) < 24.0,
        "the peak stays far below the size of the archive ({:.1} MiB), which a writer that holds it would reach: {:.1} MiB",
        size(large_bytes),
        mib(large)
    );

    // The largest individual objects now grow, while the number of exported
    // objects stays fixed. This catches a producer that streams between objects
    // but still holds an entire conversation or chat in memory.
    let long_topic = world.topic("Long histories").await;
    let thread = shared::thread_in_topic(&world, long_topic, "Long writer").await;
    shared::talk(&world, thread, "Begin", "Ready").await;
    let long_chat = chat_of(&world, long_topic).await;
    grow_history(&world, thread, long_chat, 2, 160).await;
    let histories = ExportRequest {
        parts: Some(ExportParts {
            conversations: Some(true),
            chats: Some(true),
            documents: Some(false),
        }),
        ..request(false)
    };
    let _ = written(&world, &store, &histories).await;
    let (short_history, _) = written(&world, &store, &histories).await;
    let _ = chat_peak(&world, long_chat).await;
    let short_chat = chat_peak(&world, long_chat).await;
    grow_history(&world, thread, long_chat, 161, 640).await;
    let (long_history, _) = written(&world, &store, &histories).await;
    let long_chat_peak = chat_peak(&world, long_chat).await;
    eprintln!(
        "160 -> 640 turns/messages: Markdown archive peak {:.1} -> {:.1} MiB; JSON chat peak {:.1} -> {:.1} MiB",
        mib(short_history),
        mib(long_history),
        mib(short_chat),
        mib(long_chat_peak)
    );
    for (small, large) in [(short_history, long_history), (short_chat, long_chat_peak)] {
        assert!(
            large <= small + 16 * 1_048_576,
            "four times one history stays within a window's allowance: {small} -> {large}"
        );
        assert!(
            large < 24 * 1_048_576,
            "a window's peak stays below the forty MiB combined history: {large}"
        );
    }
    drop(store);
    let _ = std::fs::remove_dir_all(&root);
}
