//! The files of an input are written into the thread's directory before its turn begins or its steer is sent: named by their paths in the text the agent reads, left out with the reason where the machine will not take them, shown as a step of the thread and, when large, as a job people can watch.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, eventually, queued};
use orch_api_contract::threads::{ContentPart, Delivery, ImageMediaType};
use orch_authz::Caller;
use orch_blob_contract::store::{BlobStore, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_db::queries::jobs::{self as jobs, NewJob, kind};
use orch_db::queries::storage;
use orch_engine::commands::Outgoing;
use orch_engine::inputs::InputRequest;
use orch_engine::manifest::Manifest;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, FsMaterialize,
    FsMaterializeResult, MaterializePurpose, SerialKey,
};
use orch_types::{Hash, Id, new_id};
use std::sync::{Arc, Mutex};

/// A store of content in a directory of its own, with the engine attached to it.
struct Files {
    store: Arc<LocalBlobStore>,
    root: std::path::PathBuf,
}

impl Files {
    async fn attach(world: &World) -> Self {
        let root = std::env::temp_dir().join(format!("orch-delivery-test-{}", new_id()));
        let store = Arc::new(LocalBlobStore::open(root.clone()).await.unwrap());
        world.engine().attach_blobs(store.clone());
        Self { store, root }
    }

    /// Puts content in the store and records it.
    async fn keep(&self, world: &World, bytes: &[u8]) -> Hash {
        let hash = Hash::of(bytes);
        self.store
            .put(
                &hash,
                Some(i64::try_from(bytes.len()).unwrap()),
                stream_of(vec![bytes::Bytes::copy_from_slice(bytes)]),
            )
            .await
            .unwrap();
        let mut conn = world.pool.acquire().await.unwrap();
        storage::register_blob(
            &mut conn,
            &hash,
            i64::try_from(bytes.len()).unwrap(),
            world.clock.now(),
        )
        .await
        .unwrap();
        orch_db::queries::uploads::remember(&mut conn, world.ada, &hash, world.clock.now())
            .await
            .unwrap();
        hash
    }

    /// The list a delivery was given, read from the store.
    async fn list(&self, hash: &Hash) -> Manifest {
        let read = self.store.get(hash, None).await.unwrap();
        let bytes = orch_blob_contract::store::read_all(read.body)
            .await
            .unwrap();
        Manifest::parse(&bytes).unwrap()
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// What the machine is asked to write, and how it answers.
#[derive(Default)]
struct Writes {
    asked: Mutex<Vec<FsMaterialize>>,
}

impl Writes {
    fn all(&self) -> Vec<FsMaterialize> {
        self.asked.lock().unwrap().clone()
    }
}

fn machine_writes(world: &World, answer: Result<FsMaterializeResult, CommandError>) -> Arc<Writes> {
    let writes = Arc::new(Writes::default());
    let kept = Arc::clone(&writes);
    world
        .machine
        .handler
        .on(CommandKind::FsMaterialize, move |command| {
            let Command::FsMaterialize(body) = command else {
                return Err(CommandError::new(ErrorCode::Internal, "not a delivery"));
            };
            kept.asked.lock().unwrap().push(body.clone());
            answer.clone().map(CommandResult::FsMaterialize)
        });
    writes
}

fn written(files: i64, bytes: i64) -> Result<FsMaterializeResult, CommandError> {
    Ok(FsMaterializeResult {
        files,
        bytes,
        skipped: 0,
    })
}

fn with_file(words: &str, path: &str, name: &str, blob: Hash, size: i64) -> InputRequest {
    InputRequest {
        plan: false,
        content: vec![
            ContentPart::Text {
                text: words.to_owned(),
            },
            ContentPart::File {
                path: path.to_owned(),
                name: name.to_owned(),
                size,
                blob: Some(blob),
            },
        ],
        delivery: Delivery::Queue,
        client_id: new_id().to_string(),
        bypass: None,
    }
}

/// The thread's directory as the agent is told of it.
async fn directory_of(world: &World, thread: Id) -> String {
    let dir: String = world
        .scalar(
            "SELECT workspace_dir FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    let root: String = world
        .scalar(
            "SELECT e.workspace_root FROM environments e JOIN threads t ON t.environment_id = e.id WHERE t.node_id = $1",
            thread,
        )
        .await;
    if dir.is_empty() {
        root
    } else {
        format!("{}/{dir}", root.trim_end_matches('/'))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_files_of_an_input_are_written_before_the_turn_begins_and_named_by_their_paths() {
    let world = World::new().await;
    let files = Files::attach(&world).await;
    let notes = b"remember the milk\n";
    let blob = files.keep(&world, notes).await;
    let writes = machine_writes(&world, written(1, 18));
    let thread = world.thread("With a file").await;
    let ada = Caller::session(world.ada);

    world
        .engine()
        .submit_input(
            &ada,
            thread,
            with_file(
                "Read this",
                ".orch/uploads/0199-notes.txt",
                "notes.txt",
                blob,
                18,
            ),
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let directory = directory_of(&world, thread).await;
    assert_eq!(
        common::words_after_where(&start.text),
        format!(
            "Read this\n\n<orch>Attached file: {directory}/.orch/uploads/0199-notes.txt (txt file; 18 bytes)</orch>"
        )
    );

    let asked = writes.all();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].purpose, MaterializePurpose::Delivery);
    assert_eq!(asked[0].target_root, format!("{directory}/.orch"));
    assert_eq!(asked[0].environment_id, world.environment);
    assert_eq!(asked[0].job_id, None, "a small delivery is no job");
    let list = files.list(&asked[0].manifest_blob).await;
    let entry = list
        .get("uploads/0199-notes.txt")
        .expect("the file is named by its place under .orch");
    assert_eq!(entry.hash, Some(blob));
    assert_eq!(entry.size, 18);

    // The files were written first: the turn begins after.
    let ran: Vec<CommandKind> = world
        .machine
        .handler
        .executions()
        .iter()
        .map(|run| run.kind)
        .collect();
    let at = |kind: CommandKind| ran.iter().position(|each| *each == kind).unwrap();
    assert!(
        at(CommandKind::FsMaterialize) < at(CommandKind::TurnStart),
        "{ran:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_without_files_asks_the_machine_to_write_nothing() {
    let world = World::new().await;
    let _files = Files::attach(&world).await;
    let writes = machine_writes(&world, written(0, 0));
    let thread = world.thread("Words only").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("Just words"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&start.text), "Just words");
    assert!(writes.all().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_machine_that_takes_no_delivered_files_holds_the_input_before_any_turn_until_its_author_recovers_it()
 {
    let world = World::new().await;
    let files = Files::attach(&world).await;
    let blob = files.keep(&world, b"data").await;
    let writes = machine_writes(
        &world,
        Err(CommandError::new(
            ErrorCode::PolicyDeliverFilesDisabled,
            "this machine takes no files",
        )),
    );
    let thread = world.thread("Refused").await;
    let ada = Caller::session(world.ada);
    let sent = world
        .engine()
        .submit_input(
            &ada,
            thread,
            with_file("See", ".orch/uploads/x-data.csv", "data.csv", blob, 4),
        )
        .await
        .unwrap();
    // Held with the file named and why, before a turn, a lease or a model request exists.
    world.wait_blocked(thread, "attachments").await;
    assert_eq!(
        world.input_state(sent.input.id).await,
        ("pending".to_owned(), Some("attachments".to_owned()))
    );
    let named: String = world
        .scalar(
            "SELECT attachment_issues->0->>'name' FROM thread_inputs WHERE id = $1",
            sent.input.id,
        )
        .await;
    assert_eq!(named, "data.csv");
    let why: String = world
        .scalar(
            "SELECT attachment_issues->0->>'message' FROM thread_inputs WHERE id = $1",
            sent.input.id,
        )
        .await;
    assert_eq!(
        why,
        "the owner of this machine does not accept delivered files"
    );
    assert!(world.turn_starts(thread).is_empty());
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id = $1", thread)
            .await,
        0
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM leases WHERE thread_id = $1", thread)
            .await,
        0
    );
    assert_eq!(writes.all().len(), 1, "it was asked, and refused");

    // A retry by its author asks again, and holds it again: the same input, still no turn.
    world
        .engine()
        .retry_input(&ada, thread, sent.input.id)
        .await
        .unwrap();
    eventually("the machine is asked again", || async {
        writes.all().len() == 2
    })
    .await;
    world.wait_blocked(thread, "attachments").await;
    assert!(world.turn_starts(thread).is_empty());

    // Sent without the file, it runs.
    world
        .engine()
        .edit_input(
            &ada,
            thread,
            sent.input.id,
            Some(vec![ContentPart::Text { text: "See".into() }]),
            None,
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&start.text), "See");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_whose_content_is_gone_holds_its_input_with_that_file_named_and_nothing_starts() {
    let world = World::new().await;
    let files = Files::attach(&world).await;
    let kept = files.keep(&world, b"still here").await;
    let lost = Hash::of(b"gone from the store");
    {
        let mut conn = world.pool.acquire().await.unwrap();
        storage::register_blob(&mut conn, &lost, 19, world.clock.now())
            .await
            .unwrap();
        orch_db::queries::uploads::remember(&mut conn, world.ada, &lost, world.clock.now())
            .await
            .unwrap();
    }
    let writes = machine_writes(&world, written(1, 10));
    let thread = world.thread("One lost").await;
    let ada = Caller::session(world.ada);
    let request = InputRequest {
        plan: false,
        content: vec![
            ContentPart::File {
                path: ".orch/uploads/a-kept.txt".into(),
                name: "kept.txt".into(),
                size: 10,
                blob: Some(kept),
            },
            ContentPart::File {
                path: ".orch/uploads/b-lost.txt".into(),
                name: "lost.txt".into(),
                size: 19,
                blob: Some(lost),
            },
        ],
        delivery: Delivery::Queue,
        client_id: new_id().to_string(),
        bypass: None,
    };
    let sent = world
        .engine()
        .submit_input(&ada, thread, request)
        .await
        .unwrap();
    world.wait_blocked(thread, "attachments").await;
    let named: String = world
        .scalar(
            "SELECT attachment_issues->0->>'name' FROM thread_inputs WHERE id = $1",
            sent.input.id,
        )
        .await;
    assert_eq!(
        named, "lost.txt",
        "the one file that cannot be had is named"
    );
    let issues = world
        .count(
            "SELECT jsonb_array_length(attachment_issues)::bigint FROM thread_inputs WHERE id = $1",
            sent.input.id,
        )
        .await;
    assert_eq!(issues, 1, "and only it");
    assert!(world.turn_starts(thread).is_empty());
    let asked = writes.all();
    assert_eq!(asked.len(), 1);
    let list = files.list(&asked[0].manifest_blob).await;
    assert_eq!(
        list.len(),
        1,
        "only what the store holds is asked of the machine"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_path_that_climbs_out_of_the_orch_directory_is_given_another_place() {
    let world = World::new().await;
    let files = Files::attach(&world).await;
    let blob = files.keep(&world, b"x").await;
    let writes = machine_writes(&world, written(1, 1));
    let thread = world.thread("Climbing").await;
    let ada = Caller::session(world.ada);
    let request = with_file("hi", "../../etc/cron.d/evil", "evil", blob, 1);
    let submitted = world
        .engine()
        .submit_input(&ada, thread, request)
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let directory = directory_of(&world, thread).await;
    assert!(
        start.text.contains(&format!(
            "Attached file: {directory}/.orch/uploads/{}/1-evil",
            submitted.input.id
        )),
        "{}",
        start.text
    );
    let list = files.list(&writes.all()[0].manifest_blob).await;
    assert!(
        list.entries()
            .iter()
            .all(|entry| entry.path.starts_with("uploads/"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn images_are_written_as_well_as_passed_and_forwarded_files_get_a_line_each() {
    let world = World::new().await;
    let files = Files::attach(&world).await;
    let png = files.keep(&world, b"\x89PNG not really").await;
    let writes = machine_writes(&world, written(1, 15));
    let thread = world.thread("With an image").await;
    let ada = Caller::session(world.ada);
    let request = InputRequest {
        plan: false,
        content: vec![ContentPart::Image {
            blob: png,
            media_type: ImageMediaType::Png,
            name: "shot.png".into(),
            path: None,
        }],
        delivery: Delivery::Queue,
        client_id: new_id().to_string(),
        bypass: None,
    };
    let _submitted = world
        .engine()
        .submit_input(&ada, thread, request)
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(start.images.len(), 1, "the model is shown the image");
    assert!(start.text.contains("image/png; 15 bytes"));
    let list = files.list(&writes.all()[0].manifest_blob).await;
    assert_eq!(
        list.get("attachments/shot.png")
            .and_then(|entry| entry.hash),
        Some(png),
        "and the agent's tools can open the original bytes"
    );
    let directory = directory_of(&world, thread).await;
    assert!(
        start
            .text
            .contains(&format!("{directory}/.orch/attachments/shot.png"))
    );
}

async fn image_delivery(model_native: bool, limit: Option<i64>, native_count: usize) {
    let world = World::new().await;
    let files = Files::attach(&world).await;
    let bytes = b"\x89PNG not really";
    let png = files.keep(&world, bytes).await;
    let writes = machine_writes(&world, written(1, 15));
    if !model_native {
        let mut conn = world.pool.acquire().await.unwrap();
        sqlx::query(
            "UPDATE provider_models SET parameters=jsonb_set(parameters,'{input_images}','false')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    if let Some(limit) = limit {
        world
            .set(
                world.topic,
                "files.native_image_bytes",
                serde_json::json!(limit),
            )
            .await;
    }
    let thread = world.thread("Image by path").await;
    let submitted = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            InputRequest {
                plan: false,
                content: vec![ContentPart::Image {
                    blob: png,
                    media_type: ImageMediaType::Png,
                    name: "shot.png".into(),
                    path: None,
                }],
                delivery: Delivery::Queue,
                client_id: new_id().to_string(),
                bypass: None,
            },
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(
        start.images.len(),
        native_count,
        "native images follow both the model's capability and the effective byte budget"
    );
    let path = ".orch/attachments/shot.png";
    assert!(
        start
            .text
            .contains(&format!("{path} (image/png; 15 bytes)")),
        "{}",
        start.text
    );
    assert!(!start.text.contains("omitted"));
    let list = files.list(&writes.all()[0].manifest_blob).await;
    assert_eq!(list.get("attachments/shot.png").unwrap().hash, Some(png));
    assert_eq!(world.count("SELECT jsonb_array_length(attachment_issues)::bigint FROM thread_inputs WHERE id=$1", submitted.input.id).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_without_native_images_receives_the_verified_path_kind_and_size() {
    image_delivery(false, None, 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_image_size_is_inclusive_and_zero_or_a_larger_file_uses_the_verified_path() {
    for (limit, native_count) in [(0, 0), (14, 0), (15, 1)] {
        image_delivery(true, Some(limit), native_count).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_thread_shows_the_step_of_its_files_while_they_are_written() {
    let world = World::new().await;
    let files = Files::attach(&world).await;
    let blob = files.keep(&world, b"slow").await;
    let writes = machine_writes(&world, written(1, 4));
    world.machine.handler.hold(CommandKind::FsMaterialize);
    let thread = world.thread("Waiting for files").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(
            &ada,
            thread,
            with_file("Go", ".orch/uploads/s-slow.txt", "slow.txt", blob, 4),
        )
        .await
        .unwrap();
    eventually("the step of the files", || async {
        world.state(thread).await.2.as_deref() == Some("files")
    })
    .await;
    assert!(
        world.turn_starts(thread).is_empty(),
        "the turn waits for its files"
    );
    world.machine.handler.release();
    let start = world.turn_start(thread, 0).await;
    assert!(start.text.contains("Attached file:"));
    assert_eq!(writes.all().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_large_delivery_is_a_job_people_can_watch_and_it_ends_with_the_command() {
    let world = World::new().await;
    let files = Files::attach(&world).await;
    let big = vec![7_u8; 9 * 1024 * 1024];
    let blob = files.keep(&world, &big).await;
    let writes = machine_writes(&world, written(1, 9 * 1024 * 1024));
    world.machine.handler.hold(CommandKind::FsMaterialize);
    let thread = world.thread("A large file").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(
            &ada,
            thread,
            with_file(
                "Big",
                ".orch/uploads/b-big.bin",
                "big.bin",
                blob,
                9 * 1024 * 1024,
            ),
        )
        .await
        .unwrap();
    eventually("the machine to be asked", || async {
        world.began(CommandKind::FsMaterialize) == 1
    })
    .await;
    let job: (Id, String, Option<Id>) = sqlx::query_as(
        "SELECT id, state, node_id FROM jobs WHERE kind = 'delivery' AND environment_id = $1",
    )
    .bind(world.environment)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        (job.1.as_str(), job.2),
        ("running", Some(thread)),
        "it is shown while it runs"
    );

    world.machine.handler.release();
    let start = world.turn_start(thread, 0).await;
    assert!(start.text.contains("Attached file:"));
    assert_eq!(
        writes.all()[0].job_id,
        Some(job.0),
        "the machine reports on the job"
    );
    eventually("the job to end", || async {
        let state: String = world
            .scalar("SELECT state FROM jobs WHERE id = $1", job.0)
            .await;
        state == "succeeded"
    })
    .await;
    let result: serde_json::Value = world
        .scalar("SELECT result FROM jobs WHERE id = $1", job.0)
        .await;
    assert_eq!(result["files"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_large_delivery_that_failed_keeps_its_input_held_with_the_file_and_reason() {
    let world = World::new().await;
    let files = Files::attach(&world).await;
    let big = vec![9_u8; 9 * 1024 * 1024];
    let blob = files.keep(&world, &big).await;
    machine_writes(
        &world,
        Err(CommandError::new(
            ErrorCode::PolicyDeliverFilesDisabled,
            "off",
        )),
    );
    let thread = world.thread("A large refusal").await;
    let ada = Caller::session(world.ada);
    let sent = world
        .engine()
        .submit_input(
            &ada,
            thread,
            with_file(
                "Big",
                ".orch/uploads/b-big.bin",
                "big.bin",
                blob,
                9 * 1024 * 1024,
            ),
        )
        .await
        .unwrap();
    world.wait_blocked(thread, "attachments").await;
    assert_eq!(
        world.input_state(sent.input.id).await,
        ("pending".to_owned(), Some("attachments".to_owned()))
    );
    let issue: serde_json::Value = world
        .scalar(
            "SELECT attachment_issues->0 FROM thread_inputs WHERE id=$1",
            sent.input.id,
        )
        .await;
    assert_eq!(issue["name"], "big.bin");
    assert_eq!(issue["code"], "delivery_failed");
    assert_eq!(
        issue["message"],
        "the owner of this machine does not accept delivered files"
    );
    assert!(
        world.turn_starts(thread).is_empty(),
        "a failed attachment is never silently delivered as text"
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id=$1", thread)
            .await,
        0
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM leases WHERE thread_id=$1", thread)
            .await,
        0
    );
    eventually("the job to end", || async {
        let count: i64 = world
            .count(
                "SELECT count(*) FROM jobs WHERE kind = 'delivery' AND state = 'failed' AND environment_id = $1",
                world.environment,
            )
            .await;
        count == 1
    })
    .await;
    let error: serde_json::Value = world
        .scalar(
            "SELECT error FROM jobs WHERE kind = 'delivery' AND environment_id = $1",
            world.environment,
        )
        .await;
    assert_eq!(error["code"], "policy.deliver_files_disabled");
    assert_eq!(error["undone"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_gets_its_files_written_before_it_is_sent() {
    let world = World::new().await;
    let files = Files::attach(&world).await;
    let blob = files.keep(&world, b"late addition").await;
    let writes = machine_writes(&world, written(1, 13));
    let thread = world.thread("Steering with a file").await;
    let (_start, _) = world.running_turn(thread, "Start").await;
    let ada = Caller::session(world.ada);
    let mut request = with_file(
        "Also this",
        ".orch/uploads/s-more.txt",
        "more.txt",
        blob,
        13,
    );
    request.delivery = Delivery::Steer;
    world
        .engine()
        .submit_input(&ada, thread, request)
        .await
        .unwrap();
    world.wait_ran(CommandKind::TurnSteer, 1).await;
    let Command::TurnSteer(body) = world.seen.of_kind(CommandKind::TurnSteer)[0].clone() else {
        panic!("a turn.steer")
    };
    let directory = directory_of(&world, thread).await;
    assert_eq!(
        common::words_after_where(&body.text),
        format!(
            "Also this\n\n<orch>Attached file: {directory}/.orch/uploads/s-more.txt (txt file; 13 bytes)</orch>"
        )
    );
    assert_eq!(writes.all().len(), 1);
}

// ---------------------------------------------------------------- jobs a process left behind

/// A delivery job as a process that is gone left it: queued, with the plan to write one list of files
/// by one command, and no lease on it (or the lease it is given).
async fn left_behind(world: &World, thread: Id, command: Id, lease_until: Option<&str>) -> Id {
    let id = new_id();
    let mut plan = serde_json::json!({
        "input_id": new_id(),
        "machine_id": world.machine_id,
        "command_id": command,
        "manifest_blob": Hash::of(b"a list"),
        "target_root": "/workspace/.orch",
    });
    if let Some(until) = lease_until {
        plan["runner"] = serde_json::json!({ "id": new_id(), "until": until });
    }
    let mut conn = world.pool.acquire().await.unwrap();
    jobs::insert(
        &mut conn,
        &NewJob {
            id,
            kind: kind::DELIVERY,
            node_id: Some(thread),
            environment_id: Some(world.environment),
            provider_id: None,
            plan: &plan,
            progress: &serde_json::to_value(orch_engine::jobs::runner::initial_progress(
                &orch_engine::delivery::PHASES,
            ))
            .unwrap(),
            created_by: None,
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    id
}

async fn state_of(world: &World, job: Id) -> String {
    world
        .scalar("SELECT state FROM jobs WHERE id = $1", job)
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_delivery_a_process_left_half_done_is_carried_on_with_the_command_it_had_sent() {
    let world = World::new().await;
    let writes = machine_writes(&world, written(2, 100));
    let thread = world.thread("A job left behind").await;
    let command = new_id();
    let ask = Outgoing::new(
        command,
        world.machine_id,
        SerialKey::Environment(world.environment),
        &FsMaterialize {
            work: None,
            environment_id: world.environment,
            job_id: None,
            manifest_blob: Hash::of(b"a list"),
            target_root: "/workspace/.orch".to_owned(),
            purpose: MaterializePurpose::Delivery,
        },
    )
    .unwrap();
    world.commands.as_engine().issue(ask).await.unwrap();
    world.commands.as_engine().wait(command).await.unwrap();

    let job = left_behind(&world, thread, command, None).await;
    eventually("the job to end with its command", || async {
        state_of(&world, job).await == "succeeded"
    })
    .await;
    let result: serde_json::Value = world
        .scalar("SELECT result FROM jobs WHERE id = $1", job)
        .await;
    assert_eq!(
        (result["files"].as_i64(), result["bytes"].as_i64()),
        (Some(2), Some(100))
    );
    assert_eq!(
        writes.all().len(),
        1,
        "a command that was sent is not run a second time"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_delivery_nothing_had_begun_is_sent_by_the_runner_that_finds_it() {
    let world = World::new().await;
    let writes = machine_writes(&world, written(1, 1));
    let thread = world.thread("A job that never began").await;
    let job = left_behind(&world, thread, new_id(), None).await;
    eventually("the job to end", || async {
        state_of(&world, job).await == "succeeded"
    })
    .await;
    assert_eq!(writes.all().len(), 1);
    assert_eq!(writes.all()[0].job_id, Some(job));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_delivery_another_process_holds_the_lease_on_is_left_alone_until_the_lease_runs_out() {
    let world = World::new().await;
    let writes = machine_writes(&world, written(1, 1));
    let thread = world.thread("A job still going").await;
    let job = left_behind(&world, thread, new_id(), Some("2999-01-01T00:00:00Z")).await;
    // Several sweeps go by.
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    assert_eq!(state_of(&world, job).await, "queued");
    assert!(writes.all().is_empty(), "nobody asked the machine");
    sqlx::query("UPDATE jobs SET plan = jsonb_set(plan, '{runner,until}', '\"2000-01-01T00:00:00Z\"') WHERE id = $1")
        .bind(job)
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually("the job to be taken over and to end", || async {
        state_of(&world, job).await == "succeeded"
    })
    .await;
    assert_eq!(writes.all().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_large_delivery_that_is_cancelled_holds_its_input_with_the_cancellation_reason() {
    let world = World::new().await;
    let files = Files::attach(&world).await;
    let big = vec![5_u8; 9 * 1024 * 1024];
    let blob = files.keep(&world, &big).await;
    machine_writes(&world, written(1, 9 * 1024 * 1024));
    world.machine.handler.hold(CommandKind::FsMaterialize);
    let thread = world.thread("A cancelled delivery").await;
    let ada = Caller::session(world.ada);
    let sent = world
        .engine()
        .submit_input(
            &ada,
            thread,
            with_file(
                "Big",
                ".orch/uploads/b-big.bin",
                "big.bin",
                blob,
                9 * 1024 * 1024,
            ),
        )
        .await
        .unwrap();
    eventually("the machine to be asked", || async {
        world.began(CommandKind::FsMaterialize) == 1
    })
    .await;
    let job: Id = world
        .scalar(
            "SELECT id FROM jobs WHERE kind = 'delivery' AND environment_id = $1",
            world.environment,
        )
        .await;
    assert!(
        world.engine().cancel_job(job, world.ada).await.unwrap(),
        "a delivery is cancelled until it has passed its commit"
    );
    world.wait_blocked(thread, "attachments").await;
    assert_eq!(
        world.input_state(sent.input.id).await,
        ("pending".to_owned(), Some("attachments".to_owned()))
    );
    let issue: serde_json::Value = world
        .scalar(
            "SELECT attachment_issues->0 FROM thread_inputs WHERE id=$1",
            sent.input.id,
        )
        .await;
    assert_eq!(issue["name"], "big.bin");
    assert_eq!(issue["code"], "delivery_failed");
    assert_eq!(issue["message"], "the delivery of the files was cancelled");
    assert!(world.turn_starts(thread).is_empty());
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id=$1", thread)
            .await,
        0
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM leases WHERE thread_id=$1", thread)
            .await,
        0
    );
    eventually("the job to end", || async {
        state_of(&world, job).await == "cancelled"
    })
    .await;
    world.machine.handler.release();
}
