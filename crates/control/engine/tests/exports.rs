//! The job that writes an archive of a place, on the engine alone: the zip it streams into the store and pins, what a person's access allows at the moment each batch is written (a role lost after the plan takes away what comes after it), a token reading within its scopes, a job that is stopped or fails leaving nothing behind, and the number of archives one person may have in the making.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use async_trait::async_trait;
use common::{World, eventually};
use futures::future::BoxFuture;
use orch_access_contract::decisions::Scopes;
use orch_api_contract::exports::{
    ExportEverything, ExportLeftOutWhat, ExportParts, ExportRequest, ExportWhat,
};
use orch_api_contract::threads::Thread;
use orch_authz::{Caller, TokenGrant};
use orch_blob_contract::store::{BlobStore, read_all, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_db::audit::Origin;
use orch_db::queries::exports as rows;
use orch_db::queries::jobs::{self as job_rows, JobRow};
use orch_engine::exports::{DocumentForm, DocumentHow, ExportError, Forms, FormsFault, Made};
use orch_types::{Id, new_id};
use serde_json::json;
use shared::{Places, chat_of, say_in_chat, talk};
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// What writes a document as its single export would, for these tests: a line that names it. A thread is
/// written as text only (the Markdown form), so the records of one are never asked of it.
#[derive(Default)]
struct Plain {
    /// How many documents were written.
    documents: AtomicUsize,
    /// Done once, before the first document is written.
    before_first: Mutex<Option<BoxFuture<'static, ()>>>,
    /// Whether it refuses to write a document.
    refuse: AtomicBool,
}

#[async_trait]
impl Forms for Plain {
    async fn thread(
        &self,
        _: &mut orch_db::Conn,
        _: &orch_authz::Principal,
        _: Id,
        _: chrono::DateTime<chrono::Utc>,
    ) -> Result<Thread, FormsFault> {
        Err(FormsFault(
            "these tests write conversations as text".to_owned(),
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
        self.documents.fetch_add(1, Ordering::SeqCst);
        let first = self.before_first.lock().unwrap().take();
        if let Some(first) = first {
            first.await;
        }
        if self.refuse.load(Ordering::SeqCst) {
            return Err(FormsFault("the document could not be written".to_owned()));
        }
        Ok(DocumentForm {
            bytes: format!("document {document}\n").into_bytes(),
            losses: String::new(),
        })
    }
}

struct Office {
    world: World,
    forms: Arc<Plain>,
    store: Arc<LocalBlobStore>,
    /// The directory the store keeps its files in, removed when the test is over.
    root: std::path::PathBuf,
}

impl Drop for Office {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn office(world: World) -> Office {
    let root = std::env::temp_dir().join(format!("orch-export-test-{}", new_id()));
    let store = Arc::new(LocalBlobStore::open(root.clone()).await.unwrap());
    world.engine().attach_blobs(store.clone());
    let forms = Arc::new(Plain::default());
    world.engine().attach_forms(forms.clone());
    Office {
        world,
        forms,
        store,
        root,
    }
}

fn everything() -> ExportRequest {
    ExportRequest {
        what: ExportWhat::Everything(ExportEverything::All),
        parts: None,
        format: None,
        detail: None,
        files: None,
        versions: None,
        comments: None,
        from: None,
        to: None,
        package: None,
    }
}

impl Office {
    async fn ask(&self, who: &Caller, place: Id) -> Result<Made, ExportError> {
        self.world
            .engine()
            .make_export(who, &Origin::default(), place, &everything())
            .await
    }

    async fn job(&self, id: Id) -> JobRow {
        let mut conn = self.world.pool.acquire().await.unwrap();
        job_rows::job(&mut conn, id).await.unwrap().unwrap()
    }

    async fn ended(&self, made: &Made) -> JobRow {
        let id = made.job.id;
        eventually("the job that writes the archive ends", || async move {
            !self.job(id).await.is_open()
        })
        .await;
        self.job(id).await
    }

    /// A gate that no failed archive can reach must report that failure instead of waiting forever.
    async fn document_started(&self, made: &Made, reached: &Notify) {
        let started = reached.notified();
        tokio::pin!(started);
        // Observe at the engine's own sweep cadence, under the existing fixture deadline.
        let poll = self.world.engine().config().sweep_every;
        tokio::time::timeout(common::PATIENCE, async {
            loop {
                tokio::select! {
                    () = &mut started => break,
                    () = tokio::time::sleep(poll) => {
                        let row = self.job(made.job.id).await;
                        assert!(row.is_open(), "export {} ended as {} before the document writer: {:?}", row.id, row.state, row.error);
                    }
                }
            }
        }).await.unwrap_or_else(|_| panic!("export {} did not reach the document writer within {:?}", made.job.id, common::PATIENCE));
    }

    /// The titles of the notices of a finished job that a person was given, oldest first.
    async fn titles_told(&self, user: Id) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT title FROM notifications WHERE user_id = $1 AND kind = 'job.finished' ORDER BY created_at",
        )
        .bind(user)
        .fetch_all(self.world.pool.pool())
        .await
        .unwrap()
    }

    async fn row(&self, id: Id) -> rows::ExportRow {
        let mut conn = self.world.pool.acquire().await.unwrap();
        rows::get(&mut conn, id).await.unwrap().unwrap()
    }

    /// What a ready archive holds, by path.
    async fn contents(&self, user: Id, id: Id) -> BTreeMap<String, Vec<u8>> {
        let (_, read) = self
            .world
            .engine()
            .open_export(&Origin::default(), user, id)
            .await
            .unwrap();
        let bytes = read_all(read.body).await.unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
        let mut held = BTreeMap::new();
        for at in 0..archive.len() {
            let mut file = archive.by_index(at).unwrap();
            let mut content = Vec::new();
            file.read_to_end(&mut content).unwrap();
            held.insert(file.name().unwrap().into_owned(), content);
        }
        held
    }
}

fn entry<'a>(held: &'a BTreeMap<String, Vec<u8>>, tail: &str) -> &'a [u8] {
    held.iter()
        .find(|(path, _)| path.ends_with(tail))
        .map(|(_, bytes)| bytes.as_slice())
        .unwrap_or_else(|| panic!("no entry ends in {tail}: {:?}", held.keys()))
}

fn has(held: &BTreeMap<String, Vec<u8>>, tail: &str) -> bool {
    held.keys().any(|path| path.ends_with(tail))
}

/// A thread that was talked to, the chat of the topic with a message in it, a document beside the topic.
async fn things(world: &World) -> (Id, Id, Id) {
    let writer = shared::thread_in_topic(world, world.topic, "Writer").await;
    talk(world, writer, "How is the login built?", "With a cookie.").await;
    let chat = chat_of(world, world.topic).await;
    say_in_chat(world, chat, world.ada, "Please keep this.").await;
    let document = world
        .document_beside(world.topic, "Handbook", &["Be kind.", "Be brief."])
        .await;
    (writer, chat, document)
}

#[tokio::test]
async fn the_job_writes_the_archive_into_the_store_pins_it_and_removing_it_lets_the_bytes_go() {
    let o = office(World::new().await).await;
    let (writer, _, document) = things(&o.world).await;
    let ada = Caller::session(o.world.ada);
    let made = o.ask(&ada, o.world.root).await.unwrap();
    assert_eq!(made.job.kind, "export");
    assert_eq!(made.export.state, "pending");
    let ended = o.ended(&made).await;
    assert_eq!(ended.state, "succeeded", "{:?}", ended.error);
    assert_eq!(
        ended.result.as_ref().unwrap()["export_id"],
        made.export.id.to_string()
    );

    let row = o.row(made.export.id).await;
    assert_eq!(row.state, "ready");
    let hash: orch_types::Hash = row.blob.as_deref().unwrap().parse().unwrap();
    assert_eq!(
        row.expires_at.unwrap() - row.ready_at.unwrap(),
        chrono::Duration::hours(24)
    );
    assert_eq!(
        o.store.size(&hash).await.unwrap(),
        row.bytes,
        "what the row says is what the store holds"
    );
    let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash = $1")
        .bind(hash.as_bytes().to_vec())
        .fetch_one(o.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(pins, 1, "pinned for as long as the archive is kept");
    let scratch = std::env::temp_dir().join(format!("export-{}.zip", made.job.id.simple()));
    assert!(!scratch.exists(), "the file it was written in is gone");

    // The person who asked is told, once.
    let told = || async {
        sqlx::query_as::<_, (String, String)>(
            "SELECT title, body FROM notifications WHERE user_id = $1 AND kind = 'job.finished'",
        )
        .bind(o.world.ada)
        .fetch_all(o.world.pool.pool())
        .await
        .unwrap()
    };
    eventually("the person who asked is told", || async {
        !told().await.is_empty()
    })
    .await;
    assert_eq!(
        told().await,
        [(
            "Acme: export finished".to_owned(),
            "The archive is ready to download.".to_owned()
        )]
    );

    let held = o.contents(o.world.ada, made.export.id).await;
    assert!(has(&held, "index.md") && has(&held, "export.json"));
    let conversation = String::from_utf8(entry(&held, "conversations/Writer.md").to_vec()).unwrap();
    assert!(conversation.contains("With a cookie."), "{conversation}");
    let chat = String::from_utf8(entry(&held, "Login/chat.md").to_vec()).unwrap();
    assert!(chat.contains("Please keep this."), "{chat}");
    assert_eq!(
        entry(&held, "docs/Handbook.md"),
        format!("document {document}\n").as_bytes(),
        "a document is what the forms of the API write"
    );
    let _ = writer;
    assert_eq!(o.forms.documents.load(Ordering::SeqCst), 1);

    // Removed: the row says so, the pin is let go, and the bytes are taken out of the store.
    o.world
        .engine()
        .remove_export(o.world.ada, made.export.id)
        .await
        .unwrap();
    assert_eq!(o.row(made.export.id).await.state, "expired");
    assert!(o.store.size(&hash).await.unwrap().is_none());
    let rows_left: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs WHERE hash = $1")
        .bind(hash.as_bytes().to_vec())
        .fetch_one(o.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(rows_left, 0);
    assert!(matches!(
        o.world
            .engine()
            .open_export(&Origin::default(), o.world.ada, made.export.id)
            .await,
        Err(ExportError::Gone)
    ));
}

#[tokio::test]
async fn what_is_lost_between_the_plan_and_the_writing_is_left_out_by_the_batch_it_would_have_been_written_in()
 {
    let o = office(World::new().await).await;
    // A hundred and thirty documents: a batch of a hundred is asked about at once, then the rest.
    for at in 0..130 {
        o.world
            .document_beside(o.world.topic, &format!("Note {at:03}"), &["words"])
            .await;
    }
    let cai = o.world.person("cai").await;
    let role = o.world.give(cai, "viewer").await;
    let caller = Caller::session(cai);
    let plan = o
        .world
        .engine()
        .plan_export(&caller, o.world.topic, &everything())
        .await
        .unwrap();
    assert_eq!(
        plan.counts.documents, 130,
        "a viewer holds the Export row as the role ships it"
    );
    // The role is taken away when the first document is written: the batch it is in is already allowed,
    // and the next is asked about again.
    let taking = {
        let pool = o.world.pool.clone();
        let now = chrono::Utc::now();
        let giver = o.world.ada;
        async move {
            let mut conn = pool.acquire().await.unwrap();
            orch_db::queries::access::testing::take_role_away(&mut conn, role, giver, now)
                .await
                .unwrap();
        }
    };
    *o.forms.before_first.lock().unwrap() = Some(Box::pin(taking));
    let made = o.ask(&caller, o.world.topic).await.unwrap();
    let ended = o.ended(&made).await;
    assert_eq!(ended.state, "succeeded", "{:?}", ended.error);
    let row = o.row(made.export.id).await;
    let held = o.contents(cai, made.export.id).await;
    let documents = held.keys().filter(|path| path.contains("/docs/")).count();
    assert_eq!(
        documents,
        100,
        "the first batch only: {:?}",
        held.keys().count()
    );
    assert_eq!(row.counts["documents"], 100);
    let left_out: Vec<orch_api_contract::exports::ExportLeftOut> =
        serde_json::from_value(row.left_out.clone()).unwrap();
    assert!(
        left_out.is_empty(),
        "what could no longer be read is not named anywhere: {left_out:?}"
    );
}

#[tokio::test]
async fn a_token_writes_within_its_scopes_and_what_it_lacks_is_counted_as_not_taken_away() {
    let o = office(World::new().await).await;
    things(&o.world).await;
    let scopes = |words: &[&str]| {
        Caller::token(
            o.world.ada,
            TokenGrant::new(Scopes::parse(words).unwrap(), None),
        )
    };
    // Everything but the Export row of documents.
    let token = scopes(&[
        "org.view",
        "project.view",
        "topic.view",
        "thread.view",
        "thread.export",
        "chat.view",
        "chat.export",
        "doc.view",
    ]);
    let made = o.ask(&token, o.world.root).await.unwrap();
    let ended = o.ended(&made).await;
    assert_eq!(ended.state, "succeeded", "{:?}", ended.error);
    let held = o.contents(o.world.ada, made.export.id).await;
    assert!(has(&held, "conversations/Writer.md") && has(&held, "Login/chat.md"));
    assert!(!has(&held, "Handbook.md"), "{:?}", held.keys());
    let row = o.row(made.export.id).await;
    let left_out: Vec<orch_api_contract::exports::ExportLeftOut> =
        serde_json::from_value(row.left_out).unwrap();
    assert_eq!(left_out.len(), 1);
    assert_eq!(left_out[0].what, ExportLeftOutWhat::NotExportable);
    assert_eq!(left_out[0].count, 1);
    // The same person with a session reads as themself: all of it.
    let whole = o.ask(&Caller::session(o.world.ada), o.world.root).await;
    // (one archive is being written at a time only where the limit says so)
    let whole = whole.unwrap();
    assert_eq!(o.ended(&whole).await.state, "succeeded");
    assert!(has(
        &o.contents(o.world.ada, whole.export.id).await,
        "Handbook.md"
    ));
}

#[tokio::test]
async fn a_job_that_is_stopped_leaves_neither_a_file_nor_an_archive() {
    let o = office(World::new().await).await;
    things(&o.world).await;
    let reached = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    {
        let (reached, release) = (reached.clone(), release.clone());
        *o.forms.before_first.lock().unwrap() = Some(Box::pin(async move {
            reached.notify_one();
            tokio::time::timeout(common::PATIENCE, release.notified())
                .await
                .expect("the test did not release the document writer");
        }));
    }
    let ada = Caller::session(o.world.ada);
    let made = o.ask(&ada, o.world.root).await.unwrap();
    o.document_started(&made, &reached).await;
    assert!(
        o.world
            .engine()
            .cancel_job(made.job.id, o.world.ada)
            .await
            .unwrap()
    );
    release.notify_one();
    let ended = o.ended(&made).await;
    assert_eq!(ended.state, "cancelled");
    let row = o.row(made.export.id).await;
    assert_eq!(row.state, "failed");
    assert!(row.blob.is_none());
    let scratch = std::env::temp_dir().join(format!("export-{}.zip", made.job.id.simple()));
    assert!(!scratch.exists(), "the file it was written in is gone");
    let pinned: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs WHERE pin_count > 0")
        .fetch_one(o.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(pinned, 0);
    assert!(matches!(
        o.world
            .engine()
            .open_export(&Origin::default(), o.world.ada, made.export.id)
            .await,
        Err(ExportError::Gone)
    ));
}

fn one_at_a_time(config: &mut orch_engine::config::EngineConfig) {
    config.exports_per_user = 1;
}

#[tokio::test]
async fn a_job_that_fails_is_undone_and_one_person_has_as_many_in_the_making_as_the_limit_says() {
    let o = office(World::tuned(one_at_a_time).await).await;
    things(&o.world).await;
    let ada = Caller::session(o.world.ada);
    // One is waiting in the middle of its writing: a second is refused, with the limit.
    let reached = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    {
        let (reached, release) = (reached.clone(), release.clone());
        *o.forms.before_first.lock().unwrap() = Some(Box::pin(async move {
            reached.notify_one();
            tokio::time::timeout(common::PATIENCE, release.notified())
                .await
                .expect("the test did not release the document writer");
        }));
    }
    o.forms.refuse.store(true, Ordering::SeqCst);
    let first = o.ask(&ada, o.world.root).await.unwrap();
    o.document_started(&first, &reached).await;
    let refused = o.ask(&ada, o.world.root).await.unwrap_err();
    assert!(
        matches!(refused, ExportError::TooManyAtOnce { limit: 1 }),
        "{refused:?}"
    );
    // Another person has a limit of their own.
    let cai = o.world.person("cai").await;
    o.world.give(cai, "viewer").await;
    let theirs = o.ask(&Caller::session(cai), o.world.topic).await.unwrap();
    release.notify_one();

    // The document cannot be written: the job fails, the row says so, and nothing is kept.
    let ended = o.ended(&first).await;
    assert_eq!(ended.state, "failed");
    let error = ended.error.unwrap();
    assert_eq!(error["code"], "internal");
    eventually("the person who asked is told that it failed", || async {
        o.titles_told(o.world.ada)
            .await
            .iter()
            .any(|title| title == "Acme: export failed")
    })
    .await;
    let row = o.row(first.export.id).await;
    assert_eq!(row.state, "failed");
    assert!(row.blob.is_none());
    let scratch = std::env::temp_dir().join(format!("export-{}.zip", first.job.id.simple()));
    assert!(!scratch.exists());
    // Cai's job fails as well, and is undone as well, before the test is over.
    assert_eq!(o.ended(&theirs).await.state, "failed");
    // The place is free again.
    o.forms.refuse.store(false, Ordering::SeqCst);
    let second = o.ask(&ada, o.world.root).await.unwrap();
    assert_eq!(o.ended(&second).await.state, "succeeded");
}

#[tokio::test]
async fn only_the_kinds_asked_for_are_written_and_a_place_with_nothing_to_take_away_is_refused() {
    let o = office(World::new().await).await;
    things(&o.world).await;
    let ada = Caller::session(o.world.ada);
    let request = ExportRequest {
        parts: Some(ExportParts {
            conversations: Some(false),
            chats: Some(true),
            documents: Some(false),
        }),
        ..everything()
    };
    let made = o
        .world
        .engine()
        .make_export(&ada, &Origin::default(), o.world.root, &request)
        .await
        .unwrap();
    assert_eq!(o.ended(&made).await.state, "succeeded");
    let held = o.contents(o.world.ada, made.export.id).await;
    assert!(has(&held, "Login/chat.md"));
    assert!(!has(&held, "Writer.md") && !has(&held, "Handbook.md"));
    // A topic of its own that holds nothing.
    let empty = o.world.topic("Empty").await;
    let refused = o.ask(&ada, empty).await.unwrap_err();
    assert!(
        matches!(&refused, ExportError::Blocked { by, .. } if by.code() == "export.nothing"),
        "{refused:?}"
    );
}

fn small_files(config: &mut orch_engine::config::EngineConfig) {
    config.export_file_max_bytes = 64;
}

/// Puts `content` in the store and records it, and says its hash.
async fn kept(o: &Office, content: &[u8]) -> orch_types::Hash {
    let hash = orch_types::Hash::of(content);
    let size = i64::try_from(content.len()).unwrap();
    o.store
        .put(
            &hash,
            Some(size),
            stream_of(vec![bytes::Bytes::copy_from_slice(content)]),
        )
        .await
        .unwrap();
    let mut conn = o.world.pool.acquire().await.unwrap();
    orch_db::queries::storage::register_blob(&mut conn, &hash, size, o.world.clock.now())
        .await
        .unwrap();
    hash
}

#[tokio::test]
async fn files_and_earlier_versions_go_in_when_they_are_asked_for_and_a_file_past_the_largest_is_left_out_and_counted()
 {
    let o = office(World::tuned(small_files).await).await;
    let (writer, chat, document) = things(&o.world).await;
    let small = kept(&o, b"notes of the meeting").await;
    let big = kept(&o, &[7_u8; 100]).await;
    let (small_size, big_size) = (20_i64, 100_i64);

    // A conversation whose first message carried both files, a message of the chat that carried both, a document that
    // holds both.
    let part = |hash: &orch_types::Hash, name: &str, size: i64| json!({ "type": "file", "path": format!(".orch/uploads/{name}"), "name": name, "size": size, "blob": hash.to_string() });
    sqlx::query(
        "UPDATE items SET payload = jsonb_set(payload, '{parts}', (payload -> 'parts') || $2::jsonb)
          WHERE thread_id = $1 AND kind = 'user_message'",
    )
    .bind(writer)
    .bind(json!([part(&small, "notes.txt", small_size), part(&big, "big.bin", big_size)]))
    .execute(o.world.pool.pool())
    .await
    .unwrap();
    for (hash, name, size) in [
        (&small, "notes.txt", small_size),
        (&big, "big.bin", big_size),
    ] {
        let seq: i64 = sqlx::query_scalar(
            "UPDATE chats SET next_seq = next_seq + 1 WHERE id = $1 RETURNING next_seq - 1",
        )
        .bind(chat)
        .fetch_one(o.world.pool.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_messages (id, chat_id, kind, author_id, content, seq_created, seq_updated, search_text)
             VALUES ($1, $2, 'message', $3, $4, $5, $5, $6)",
        )
        .bind(new_id())
        .bind(chat)
        .bind(o.world.ada)
        .bind(json!([{
            "type": "attachment", "blob": hash.to_string(), "name": name, "size": size,
            "media_type": "application/octet-stream"
        }]))
        .bind(seq)
        .bind(name)
        .execute(o.world.pool.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO document_files (id, document_id, blob, name, media_type, size, is_text, added_by)
             VALUES ($1, $2, $3, $4, 'application/octet-stream', $5, false, $6)",
        )
        .bind(new_id())
        .bind(document)
        .bind(hash.as_bytes().to_vec())
        .bind(name)
        .bind(size)
        .bind(o.world.ada)
        .execute(o.world.pool.pool())
        .await
        .unwrap();
    }
    // An earlier version of the document: a change cuts one.
    o.world.add_paragraph(document, "Be brief again.").await;

    let ada = Caller::session(o.world.ada);
    let request = ExportRequest {
        files: Some(true),
        versions: Some(true),
        ..everything()
    };
    let plan = o
        .world
        .engine()
        .plan_export(&ada, o.world.root, &request)
        .await
        .unwrap();
    assert_eq!(
        plan.counts.files, 3,
        "the small file of each of the three kinds of thing"
    );
    let too_large: i64 = plan
        .left_out
        .iter()
        .filter(|each| each.what == ExportLeftOutWhat::TooLarge)
        .map(|each| each.count)
        .sum();
    assert_eq!(too_large, 3, "the large file of each: {:?}", plan.left_out);
    let made = o
        .world
        .engine()
        .make_export(&ada, &Origin::default(), o.world.root, &request)
        .await
        .unwrap();
    let ended = o.ended(&made).await;
    assert_eq!(ended.state, "succeeded", "{:?}", ended.error);
    let held = o.contents(o.world.ada, made.export.id).await;
    assert_eq!(
        entry(&held, "conversations/Writer.files/0001/in-1-notes.txt"),
        b"notes of the meeting"
    );
    assert!(
        entry_with(&held, "Login/chat.files/", "notes.txt").is_some(),
        "{:?}",
        held.keys()
    );
    assert_eq!(
        entry(&held, "docs/Handbook.files/notes.txt"),
        b"notes of the meeting"
    );
    assert!(
        !has(&held, "big.bin") && !has(&held, "in-2-big.bin"),
        "nothing past the largest file goes in: {:?}",
        held.keys()
    );
    let history: Vec<&String> = held
        .keys()
        .filter(|path| path.contains("docs/Handbook.history/"))
        .collect();
    assert_eq!(history.len(), 1, "{history:?}");
    let version = String::from_utf8(held[history[0]].clone()).unwrap();
    assert!(
        version.contains("Be kind.") && !version.contains("Be brief again."),
        "{version}"
    );
    let row = o.row(made.export.id).await;
    assert_eq!(row.counts["files"], 3);
    let left_out: Vec<orch_api_contract::exports::ExportLeftOut> =
        serde_json::from_value(row.left_out).unwrap();
    assert_eq!(left_out.iter().map(|each| each.count).sum::<i64>(), 3);
    assert!(
        left_out
            .iter()
            .all(|each| each.what == ExportLeftOutWhat::TooLarge)
    );

    // Not asked for, none of it is there.
    let plain = o.ask(&ada, o.world.root).await.unwrap();
    assert_eq!(o.ended(&plain).await.state, "succeeded");
    let held = o.contents(o.world.ada, plain.export.id).await;
    assert!(
        held.keys()
            .all(|path| !path.contains(".files/") && !path.contains(".history/")),
        "{:?}",
        held.keys()
    );
}

/// The entry whose path starts with `head` and ends with `tail`.
fn entry_with<'a>(held: &'a BTreeMap<String, Vec<u8>>, head: &str, tail: &str) -> Option<&'a [u8]> {
    held.iter()
        .find(|(path, _)| path.contains(head) && path.ends_with(tail))
        .map(|(_, bytes)| bytes.as_slice())
}
