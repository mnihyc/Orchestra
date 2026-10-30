//! Atomic document management: only destructive deletion previews and confirms; move and restoration execute directly.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use common::World;
use orch_link_contract::commands::{OrchTool, OrchToolResult};
use orch_types::Id;
use serde_json::{Value, json};
use shared::*;
use std::time::Duration;

/// The rows of a share to manage: the agents see the documents, change them and manage them.
const MANAGE: [&str; 3] = ["doc.view", "doc.edit", "doc.manage"];
/// The rows of a share to edit: the same, but for the right to manage.
const EDIT: [&str; 2] = ["doc.view", "doc.edit"];

struct Setup {
    world: World,
    agent: Id,
    topic: Id,
    /// The path of the `docs` directory of the topic, ending in `/`.
    docs: String,
    plan: Id,
    notes: Id,
}

impl Setup {
    fn path(&self, name: &str) -> String {
        format!("{}{name}", self.docs)
    }
}

/// An agent, and a topic `Docs` with two documents, `Plan` and `Notes`, whose rows are shared with the agent.
async fn setup(world: World, permissions: &[&str]) -> Setup {
    let agent = world.thread("Tidy").await;
    let topic = world.topic("Docs").await;
    let plan = world
        .document_beside(topic, "Plan", &["The plan.", "Second block."])
        .await;
    let notes = world
        .document_beside(topic, "Notes", &["Some notes."])
        .await;
    for permission in permissions {
        share(&world, "node", topic, "live", permission, "any", agent).await;
    }
    let listing = ok(call(
        &world,
        agent,
        OrchTool::ListShared,
        json!({ "path": format!("id:{topic}") }),
    )
    .await);
    let docs = entry(&listing, "docs")["path"].as_str().unwrap().to_owned();
    Setup {
        world,
        agent,
        topic,
        docs,
        plan,
        notes,
    }
}

async fn with_documents(permissions: &[&str]) -> Setup {
    setup(a_world_whose_agent_takes_tools().await, permissions).await
}

async fn manage(s: &Setup, arguments: Value) -> OrchToolResult {
    manage_as(s, s.agent, arguments).await
}

async fn manage_as(s: &Setup, thread: Id, arguments: Value) -> OrchToolResult {
    call(&s.world, thread, OrchTool::ManageShared, arguments).await
}

/// The first call, answered: the plan.
async fn plan(s: &Setup, command: &str, paths: &[impl AsRef<str>]) -> Value {
    let paths: Vec<&str> = paths.iter().map(AsRef::as_ref).collect();
    ok(manage(s, json!({ "command": command, "paths": paths })).await)
}

fn word_of(plan: &Value) -> String {
    plan["confirm"].as_str().unwrap().to_owned()
}

/// A stale confirmation is not a new confirmable plan.
fn short_refusal(message: &str) {
    assert!(
        message.len() < 1_000,
        "a refusal fits the smallest answer bound"
    );
    assert!(!message.contains("\"confirm\""));
    assert!(!message.contains("\"affects\""));
    assert!(message.contains("without confirm"));
}

async fn count(world: &World, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&mut *world.pool.acquire().await.unwrap())
        .await
        .unwrap()
}

/// Everything a call that changes nothing must leave as it was: the documents with their sequences and places, and
/// how many lines of the audit trail, rows of pages, blocks and versions there are.
async fn fingerprint(world: &World) -> String {
    let documents: Vec<(Id, i64, bool, Id)> = sqlx::query_as(
        "SELECT id, doc_seq, deleted_at IS NULL, node_id FROM documents ORDER BY id",
    )
    .fetch_all(&mut *world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    let mut out = format!("{documents:?}");
    for table in [
        "audit_log",
        "permission_entries",
        "document_blocks",
        "document_versions",
        "document_files",
        "notifications",
    ] {
        out.push_str(&format!(
            " {table}={}",
            count(world, &format!("SELECT count(*) FROM {table}")).await
        ));
    }
    out
}

/// The lines of the audit trail about a document, by action: who did it, whether it is a line of the history, where.
async fn lines(
    world: &World,
    action: &str,
    document: Id,
) -> Vec<(String, Option<Id>, bool, Option<Id>)> {
    sqlx::query_as(
        "SELECT actor_kind, actor_id, history, node_id FROM audit_log WHERE action = $1 AND object_id = $2 ORDER BY at, id",
    )
    .bind(action)
    .bind(document)
    .fetch_all(&mut *world.pool.acquire().await.unwrap())
    .await
    .unwrap()
}

async fn deleted(world: &World, document: Id) -> bool {
    world
        .scalar::<bool>(
            "SELECT deleted_at IS NOT NULL FROM documents WHERE id = $1",
            document,
        )
        .await
}

async fn listing(s: &Setup, thread: Id, path: &str) -> Value {
    ok(call(
        &s.world,
        thread,
        OrchTool::ListShared,
        json!({ "path": path }),
    )
    .await)
}

/// A real mutable document container for recursive cases; the built-in docs area stays immutable.
async fn collection(s: &Setup) -> String {
    let folder = format!("{}collection/", s.docs);
    ok(manage(s, json!({"command":"create_folder","paths":[folder]})).await);
    ok(manage(
        s,
        json!({"command":"move","paths":[s.path("Plan.md"),s.path("Notes.md")],"to":folder}),
    )
    .await);
    folder
}

async fn create_in(s: &Setup, directory: &str, name: &str) {
    ok(call(&s.world,s.agent,OrchTool::EditDocument,
        json!({"command":"create","path":format!("{directory}{name}.md"),"markdown":"A small document."})).await);
}

// ---------------------------------------------------------------- what a share to manage shows

#[tokio::test]
async fn a_share_to_manage_lists_its_documents_as_manageable_and_a_share_to_edit_does_not() {
    let s = with_documents(&MANAGE).await;
    let docs = listing(&s, s.agent, &s.docs).await;
    for name in ["Plan.md", "Notes.md"] {
        let found = entry(&docs, name);
        assert_eq!(found["manageable"], true, "{name}");
        assert_eq!(found["writable"], true, "{name}");
        assert_eq!(found["readable"], true, "{name}");
    }
    let top = listing(&s, s.agent, &format!("id:{}", s.topic)).await;
    assert_ne!(
        entry(&top, "docs")["manageable"],
        true,
        "the built-in docs area is not itself a mutable object"
    );
    let e = with_documents(&EDIT).await;
    let docs = listing(&e, e.agent, &e.docs).await;
    assert!(entry(&docs, "Plan.md").get("manageable").is_none());
    assert_eq!(entry(&docs, "Plan.md")["writable"], true);
    let top = listing(&e, e.agent, &format!("id:{}", e.topic)).await;
    assert!(entry(&top, "docs").get("manageable").is_none());
}

// ---------------------------------------------------------------- the first call

#[tokio::test]
async fn the_first_call_changes_nothing_and_lists_what_would_be_affected_with_what_goes_with_it_and_a_word()
 {
    let s = with_documents(&MANAGE).await;
    s.world.live.open_by(s.plan, 2);
    let before = fingerprint(&s.world).await;
    let answer = plan(&s, "delete", &[&s.path("Plan.md")]).await;
    assert_eq!(fingerprint(&s.world).await, before, "nothing was changed");
    assert_eq!(answer["done"], false);
    assert_eq!(answer["expires_in"], 600);
    let word = word_of(&answer);
    assert_eq!(word.len(), 32, "128 bits: {word}");
    assert!(word.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(answer["refused"], json!([]));
    let affects = answer["affects"].as_array().unwrap();
    assert_eq!(affects.len(), 1);
    let item = &affects[0];
    assert_eq!(item["action"], "delete");
    assert_eq!(item["path"], json!(s.path("Plan.md")));
    assert_eq!(item["name"], "Plan.md");
    assert_eq!(item["id"], json!(s.plan));
    assert_eq!(item["blocks"], 2);
    assert_eq!(item["files"], 0);
    assert_eq!(item["open_by"], 2, "the people who have it open now");
    assert_eq!(item["shared_with"], 0);
    let versions: i64 = count(
        &s.world,
        &format!(
            "SELECT count(*) FROM document_versions WHERE document_id = '{}'",
            s.plan
        ),
    )
    .await;
    assert_eq!(item["versions"], versions);
    let until = item["until"]
        .as_str()
        .expect("until when it can be brought back");
    let until: chrono::DateTime<chrono::Utc> = until.parse().unwrap();
    let days = (until - chrono::Utc::now()).num_days();
    assert!(
        (29..=30).contains(&days),
        "the thirty days of the purge: {until}"
    );
}

#[tokio::test]
async fn the_builtin_docs_directory_is_immutable_and_overlapping_selections_are_refused() {
    let s = with_documents(&MANAGE).await;
    let before = fingerprint(&s.world).await;
    let answer = manage(&s, json!({"command":"delete","paths":[s.docs]})).await;
    assert_eq!(refused(&answer).0, "wrong_kind");
    assert_eq!(fingerprint(&s.world).await, before);
    // Overlapping selection is refused rather than silently changing what was selected.
    let before = fingerprint(&s.world).await;
    let overlap = manage(
        &s,
        json!({ "command": "delete", "paths": [s.path("Plan.md"), s.docs] }),
    )
    .await;
    assert_eq!(refused(&overlap).0, "invalid");
    assert!(refused(&overlap).1.contains("must not overlap"));
    assert_eq!(fingerprint(&s.world).await, before);
}

#[tokio::test]
async fn a_list_of_more_documents_than_can_be_read_and_confirmed_together_is_refused() {
    let s = with_documents(&MANAGE).await;
    let folder = collection(&s).await;
    for n in 0..orch_engine::tools::manage::AFFECTS_MAX {
        create_in(&s, &folder, &format!("Draft {n}")).await;
    }
    // The folder, Plan and Notes also consume the same operation budget.
    let refusal = manage(&s, json!({ "command": "delete", "paths": [folder] })).await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "limit");
    assert!(
        message.contains("50") && message.contains("limit"),
        "{message}"
    );
    assert!(!deleted(&s.world, s.plan).await);
}

// ---------------------------------------------------------------- the second call

#[tokio::test]
async fn a_management_plan_must_fit_whole_before_its_word_is_issued_and_a_smaller_confirm_bound_changes_nothing()
 {
    let s = with_documents(&MANAGE).await;
    let folder = collection(&s).await;
    for index in 0..25 {
        let name = format!("{}{index}", "中".repeat(180));
        create_in(&s, &folder, &name).await;
    }
    let before = fingerprint(&s.world).await;
    let first = manage(&s, json!({"command":"delete","paths":[folder]})).await;
    let (code, message) = refused(&first);
    assert_eq!(code, "too_large");
    assert!(message.contains("No confirmation word"));
    assert!(!message.contains("\"confirm\""));
    assert_eq!(fingerprint(&s.world).await, before);
    let complete = ok(manage(
        &s,
        json!({"command":"delete","paths":[folder],"max_bytes":40000}),
    )
    .await);
    assert_eq!(
        complete["affects"].as_array().unwrap().len(),
        28,
        "all twenty-seven documents and their real folder are present, never a confirmable prefix"
    );
    assert!(complete.to_string().len() <= 40_000);
    let word = word_of(&complete);
    let small = manage(
        &s,
        json!({"command":"delete","paths":[folder],"confirm":word,"max_bytes":1000}),
    )
    .await;
    assert_eq!(refused(&small).0, "too_large");
    assert_eq!(fingerprint(&s.world).await, before);
    ok(manage(
        &s,
        json!({"command":"delete","paths":[folder],"confirm":word,"max_bytes":40000}),
    )
    .await);
    assert!(
        deleted(&s.world, s.plan).await && deleted(&s.world, s.notes).await,
        "a too-small confirm neither acts nor spends the original readable plan"
    );
}

#[tokio::test]
async fn the_second_call_with_the_word_does_all_of_the_list_in_one_act_under_the_agents_name() {
    let s = with_documents(&MANAGE).await;
    let answer = plan(&s, "delete", &[&s.path("Plan.md"), &s.path("Notes.md")]).await;
    let done = ok(manage(
        &s,
        json!({ "command": "delete", "paths": [s.path("Plan.md"), s.path("Notes.md")], "confirm": word_of(&answer) }),
    )
    .await);
    assert_eq!(done["done"], true);
    let results = done["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    for result in results {
        assert_eq!(result["action"], "delete");
        assert!(result["until"].is_string());
    }
    for document in [s.plan, s.notes] {
        assert!(deleted(&s.world, document).await);
        let by: Option<Id> = s
            .world
            .scalar("SELECT deleted_by FROM documents WHERE id = $1", document)
            .await;
        assert_eq!(
            by, None,
            "nobody is named as having deleted what an agent did"
        );
        // Each act is in the document's history under the agent's name, and the place's Activity has it.
        let lines = lines(&s.world, "document.deleted", document).await;
        assert_eq!(
            lines,
            [("agent".to_owned(), Some(s.agent), true, Some(s.topic))]
        );
    }
}

#[tokio::test]
async fn nothing_is_destroyed_by_a_delete_and_a_restore_brings_back_what_there_was() {
    let s = with_documents(&MANAGE).await;
    s.world.add_paragraph(s.plan, "A third block.").await;
    let text = s.world.blocks_of(s.plan).await;
    let parts = |sql: &'static str| {
        let world = &s.world;
        let plan = s.plan;
        async move { world.count(sql, plan).await }
    };
    let blocks =
        parts("SELECT count(*) FROM document_blocks WHERE document_id = $1 AND removed_at IS NULL")
            .await;
    let versions = parts("SELECT count(*) FROM document_versions WHERE document_id = $1").await;
    assert!(
        blocks == 3 && versions >= 1,
        "{blocks} blocks, {versions} versions"
    );
    let first = plan(&s, "delete", &[&s.path("Plan.md")]).await;
    ok(manage(
        &s,
        json!({ "command": "delete", "paths": [s.path("Plan.md")], "confirm": word_of(&first) }),
    )
    .await);
    assert_eq!(
        parts("SELECT count(*) FROM document_blocks WHERE document_id = $1 AND removed_at IS NULL")
            .await,
        blocks,
        "its blocks stay until the purge"
    );
    assert_eq!(
        parts("SELECT count(*) FROM document_versions WHERE document_id = $1").await,
        versions,
        "and so do its versions"
    );
    // It is listed to the agent that may bring it back, marked as deleted, and can be neither read nor changed.
    let docs = listing(&s, s.agent, &s.docs).await;
    let gone = entry(&docs, "Plan.md");
    assert_eq!(gone["deleted"], true);
    assert_eq!(gone["manageable"], true);
    assert_eq!(gone["readable"], false);
    assert_eq!(gone["writable"], false);
    assert_eq!(entry(&docs, "Notes.md")["readable"], true);
    let read = call(
        &s.world,
        s.agent,
        OrchTool::ReadShared,
        json!({ "path": s.path("Plan.md") }),
    )
    .await;
    assert_eq!(
        refused(&read).0,
        "not_found",
        "what was deleted cannot be read"
    );
    // And restore brings it back, as it was.
    let done = ok(manage(
        &s,
        json!({ "command": "restore", "paths": [s.path("Plan.md")] }),
    )
    .await);
    assert_eq!(done["done"], true);
    assert!(
        done.get("confirm").is_none(),
        "restoration is one direct operation"
    );
    assert_eq!(done["results"][0]["action"], "restore");
    assert!(!deleted(&s.world, s.plan).await);
    assert_eq!(s.world.blocks_of(s.plan).await, text);
    let restored = lines(&s.world, "document.restored", s.plan).await;
    assert_eq!(restored.len(), 1);
    assert_eq!(
        (restored[0].0.as_str(), restored[0].1),
        ("agent", Some(s.agent))
    );
    let docs = listing(&s, s.agent, &s.docs).await;
    assert!(entry(&docs, "Plan.md").get("deleted").is_none());
}

// ---------------------------------------------------------------- a word that is no good acts for nobody

#[tokio::test]
async fn a_made_up_word_never_acts_and_a_short_refusal_requires_reading_a_new_structured_plan() {
    let s = with_documents(&MANAGE).await;
    let before = fingerprint(&s.world).await;
    let made_up = "0123456789abcdef0123456789abcdef";
    let refusal = manage(
        &s,
        json!({ "command": "delete", "paths": [s.path("Plan.md")], "confirm": made_up }),
    )
    .await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "not_confirmed");
    assert!(message.contains("made up"), "{message}");
    assert!(message.contains("Nothing was done"), "{message}");
    short_refusal(message);
    assert_eq!(fingerprint(&s.world).await, before);
    // A separate first call gives a complete plan, and only its word can act.
    let fresh = plan(&s, "delete", &[&s.path("Plan.md")]).await;
    let word = word_of(&fresh);
    assert_ne!(word, made_up);
    ok(manage(
        &s,
        json!({ "command": "delete", "paths": [s.path("Plan.md")], "confirm": word }),
    )
    .await);
    assert!(deleted(&s.world, s.plan).await);
}

#[tokio::test]
async fn a_word_is_good_once() {
    let s = with_documents(&MANAGE).await;
    let first = plan(&s, "delete", &[&s.path("Plan.md")]).await;
    let word = word_of(&first);
    let arguments = json!({ "command": "delete", "paths": [s.path("Plan.md")], "confirm": word });
    ok(manage(&s, arguments.clone()).await);
    let lines_before = lines(&s.world, "document.deleted", s.plan).await.len();
    let again = manage(&s, arguments).await;
    let (code, message) = refused(&again);
    assert_eq!(code, "not_confirmed");
    assert!(
        message.contains("Nothing was done"),
        "a spent word changes nothing: {message}"
    );
    assert_eq!(
        lines(&s.world, "document.deleted", s.plan).await.len(),
        lines_before,
        "a word used twice acts once"
    );
}

#[tokio::test]
async fn a_word_of_another_thread_acts_for_nobody_else_and_is_not_spent_by_the_attempt() {
    let s = with_documents(&MANAGE).await;
    let other = s.world.thread("Other").await;
    share(&s.world, "node", s.topic, "live", "doc.view", "any", other).await;
    share(&s.world, "node", s.topic, "live", "doc.edit", "any", other).await;
    share(
        &s.world,
        "node",
        s.topic,
        "live",
        "doc.manage",
        "any",
        other,
    )
    .await;
    let first = plan(&s, "delete", &[&s.path("Plan.md")]).await;
    let word = word_of(&first);
    let arguments = json!({ "command": "delete", "paths": [s.path("Plan.md")], "confirm": word });
    let stolen = manage_as(&s, other, arguments.clone()).await;
    assert_eq!(refused(&stolen).0, "not_confirmed");
    assert!(!deleted(&s.world, s.plan).await);
    // The thread it was given to still has it.
    ok(manage(&s, arguments).await);
    assert!(deleted(&s.world, s.plan).await);
}

#[tokio::test]
async fn a_word_older_than_the_time_it_is_good_for_acts_no_more() {
    let world = World::tuned(|config| config.manage_plan_kept = Duration::from_millis(400)).await;
    the_agent_takes_tools_again(&world).await;
    let s = setup(world, &MANAGE).await;
    let first = plan(&s, "delete", &[&s.path("Plan.md")]).await;
    assert_eq!(
        first["expires_in"], 0,
        "the time is the engine's own: here under a second"
    );
    tokio::time::sleep(Duration::from_millis(900)).await;
    let late = manage(
        &s,
        json!({ "command": "delete", "paths": [s.path("Plan.md")], "confirm": word_of(&first) }),
    )
    .await;
    assert_eq!(refused(&late).0, "not_confirmed");
    assert!(!deleted(&s.world, s.plan).await);
}

#[tokio::test]
async fn a_word_given_for_other_arguments_acts_for_none_and_stays_good_for_its_own() {
    let s = with_documents(&MANAGE).await;
    let first = plan(&s, "delete", &[&s.path("Plan.md")]).await;
    let word = word_of(&first);
    for other in [
        json!({ "command": "delete", "paths": [s.path("Notes.md")], "confirm": word }),
        json!({ "command": "delete", "paths": [s.path("Notes.md"), s.path("Plan.md")], "confirm": word }),
    ] {
        let refusal = manage(&s, other).await;
        let (code, message) = refused(&refusal);
        assert_eq!(code, "not_confirmed");
        assert!(message.contains("other arguments"), "{message}");
    }
    assert!(!deleted(&s.world, s.plan).await && !deleted(&s.world, s.notes).await);
    ok(manage(
        &s,
        json!({ "command": "delete", "paths": [s.path("Plan.md")], "confirm": word }),
    )
    .await);
    assert!(deleted(&s.world, s.plan).await && !deleted(&s.world, s.notes).await);
}

#[tokio::test]
async fn a_document_changed_between_the_two_calls_is_not_touched_and_the_act_is_a_new_plan() {
    let s = with_documents(&MANAGE).await;
    let first = plan(&s, "delete", &[&s.path("Plan.md"), &s.path("Notes.md")]).await;
    s.world
        .change_block(s.plan, "The plan", "The plan, rewritten meanwhile.")
        .await;
    let arguments = |word: &str| json!({ "command": "delete", "paths": [s.path("Plan.md"), s.path("Notes.md")], "confirm": word });
    let refusal = manage(&s, arguments(&word_of(&first))).await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "not_confirmed");
    assert!(message.contains("changed since the preview"), "{message}");
    assert!(
        !deleted(&s.world, s.plan).await && !deleted(&s.world, s.notes).await,
        "neither the document that changed nor the one that did not was touched"
    );
    short_refusal(message);
    let fresh = plan(&s, "delete", &[&s.path("Plan.md"), &s.path("Notes.md")]).await;
    ok(manage(&s, arguments(&word_of(&fresh))).await);
    assert!(deleted(&s.world, s.plan).await && deleted(&s.world, s.notes).await);
}

#[tokio::test]
async fn one_refused_item_keeps_the_rest_from_being_done() {
    let s = with_documents(&MANAGE).await;
    // A second topic whose document is shared for reading only.
    let reading = s.world.topic("Reading").await;
    let book = s.world.document_beside(reading, "Book", &["Pages."]).await;
    share(
        &s.world, "node", reading, "live", "doc.view", "any", s.agent,
    )
    .await;
    let listing = listing(&s, s.agent, &format!("id:{reading}")).await;
    let book_path = format!(
        "{}Book.md",
        entry(&listing, "docs")["path"].as_str().unwrap()
    );
    let paths = [s.path("Plan.md"), book_path.clone()];
    let before = fingerprint(&s.world).await;
    let refusal = manage(&s, json!({ "command": "delete", "paths": paths })).await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "not_manageable");
    assert!(message.contains("Nothing was changed"), "{message}");
    assert!(
        !message.contains("confirm") && !message.contains("affects"),
        "a refused selection never issues a partially confirmable plan: {message}"
    );
    assert_eq!(fingerprint(&s.world).await, before);
    assert!(
        !deleted(&s.world, s.plan).await,
        "all of it is done or none"
    );
    assert!(!deleted(&s.world, book).await);
}

#[tokio::test]
async fn a_call_said_twice_is_the_same_call_and_acts_once() {
    let s = with_documents(&MANAGE).await;
    let first = ok(call_as(
        &s.world,
        s.agent,
        OrchTool::ManageShared,
        json!({ "command": "delete", "paths": [s.path("Plan.md")] }),
        "plan-once",
    )
    .await);
    let again = ok(call_as(
        &s.world,
        s.agent,
        OrchTool::ManageShared,
        json!({ "command": "delete", "paths": [s.path("Plan.md")] }),
        "plan-once",
    )
    .await);
    assert_eq!(first, again, "the same word, for it is the same answer");
    let arguments =
        json!({ "command": "delete", "paths": [s.path("Plan.md")], "confirm": word_of(&first) });
    let done = ok(call_as(
        &s.world,
        s.agent,
        OrchTool::ManageShared,
        arguments.clone(),
        "act-once",
    )
    .await);
    let replayed = ok(call_as(
        &s.world,
        s.agent,
        OrchTool::ManageShared,
        arguments,
        "act-once",
    )
    .await);
    assert_eq!(
        done, replayed,
        "an act said twice is not refused for a word it used"
    );
    assert_eq!(lines(&s.world, "document.deleted", s.plan).await.len(), 1);
}

#[tokio::test]
async fn a_plan_does_not_outlive_a_restart_and_the_second_call_is_then_a_first_call_made_again() {
    let s = with_documents(&MANAGE).await;
    let first = plan(&s, "delete", &[&s.path("Plan.md")]).await;
    s.world.restart_api().await;
    the_agent_takes_tools_again(&s.world).await;
    let arguments =
        |word: &str| json!({ "command": "delete", "paths": [s.path("Plan.md")], "confirm": word });
    let refusal = manage(&s, arguments(&word_of(&first))).await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "not_confirmed");
    assert!(!deleted(&s.world, s.plan).await);
    short_refusal(message);
    let fresh = plan(&s, "delete", &[&s.path("Plan.md")]).await;
    ok(manage(&s, arguments(&word_of(&fresh))).await);
    assert!(deleted(&s.world, s.plan).await);
}

// ---------------------------------------------------------------- what an agent may not manage

#[tokio::test]
async fn an_agent_shared_a_place_to_edit_is_refused_as_not_manageable_and_told_what_a_person_can_do()
 {
    let s = with_documents(&EDIT).await;
    let refusal = manage(
        &s,
        json!({ "command": "delete", "paths": [s.path("Plan.md")] }),
    )
    .await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "not_manageable");
    assert!(
        message.contains("manage access") && message.contains("ask"),
        "{message}"
    );
    assert!(!deleted(&s.world, s.plan).await);
    // Nor does a word, made up or from anywhere, do anything for it.
    let with_word = manage(
        &s,
        json!({ "command": "delete", "paths": [s.path("Plan.md")], "confirm": "0123456789abcdef0123456789abcdef" }),
    )
    .await;
    assert_eq!(refused(&with_word).0, "not_confirmed");
    assert!(!deleted(&s.world, s.plan).await);
    // One that is shared to manage does each of the three.
    let m = with_documents(&MANAGE).await;
    let first = plan(&m, "delete", &[&m.path("Notes.md")]).await;
    ok(manage(
        &m,
        json!({ "command": "delete", "paths": [m.path("Notes.md")], "confirm": word_of(&first) }),
    )
    .await);
    assert!(deleted(&m.world, m.notes).await);
}

#[tokio::test]
async fn a_thread_a_chat_and_a_place_are_refused_whatever_was_shared() {
    let s = with_documents(&MANAGE).await;
    let repro = s.world.thread_in(s.topic, "Repro").await;
    let chat = chat_of(&s.world, s.topic).await;
    for permission in [
        "thread.view",
        "thread.interact",
        "chat.view",
        "chat.interact",
    ] {
        share(
            &s.world, "node", s.topic, "live", permission, "any", s.agent,
        )
        .await;
        share(&s.world, "node", repro, "live", permission, "any", s.agent).await;
    }
    share(
        &s.world,
        "node",
        repro,
        "live",
        "doc.manage",
        "any",
        s.agent,
    )
    .await;
    let top = listing(&s, s.agent, &format!("id:{}", s.topic)).await;
    let chat_path = entry(&top, "chat.md")["path"].as_str().unwrap().to_owned();
    let thread_dir = listing(&s, s.agent, &format!("id:{repro}")).await;
    let conversation = entry(&thread_dir, "conversation.md")["path"]
        .as_str()
        .unwrap()
        .to_owned();
    for path in [
        chat_path,
        conversation,
        format!("id:{repro}"),
        format!("id:{}", s.topic),
        format!("id:{chat}"),
    ] {
        for command in ["delete", "restore"] {
            let refusal = manage(&s, json!({ "command": command, "paths": [path] })).await;
            let (code, message) = refused(&refusal);
            assert_eq!(code, "wrong_kind", "{command} {path}");
            assert!(message.contains("managed by people"), "{path}: {message}");
        }
    }
    let moved = manage(
        &s,
        json!({ "command": "move", "paths": [format!("id:{repro}")], "to": s.docs }),
    )
    .await;
    assert_eq!(refused(&moved).0, "wrong_kind");
    assert!(!deleted(&s.world, s.plan).await);
}

#[tokio::test]
async fn what_the_agent_cannot_reach_is_not_found_as_everywhere_else() {
    let s = with_documents(&MANAGE).await;
    let hidden = s.world.topic("Private").await;
    s.world
        .document_beside(hidden, "Secret", &["Hidden."])
        .await;
    for path in [
        s.path("Missing.md"),
        "/nowhere/docs/Plan.md".to_owned(),
        format!("id:{}", hidden),
    ] {
        let refusal = manage(&s, json!({ "command": "delete", "paths": [path] })).await;
        assert_eq!(refused(&refusal).0, "not_found", "{path}");
    }
}

#[tokio::test]
async fn a_document_in_an_archived_place_is_refused_as_not_writable() {
    let s = with_documents(&MANAGE).await;
    s.world
        .exec(&format!(
            "UPDATE nodes SET archived_at = now() WHERE id = '{}'",
            s.topic
        ))
        .await;
    let refusal = manage(
        &s,
        json!({ "command": "delete", "paths": [s.path("Plan.md")] }),
    )
    .await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "not_writable");
    assert!(message.contains("archived"), "{message}");
    assert!(!deleted(&s.world, s.plan).await);
}

#[tokio::test]
async fn a_deleted_document_is_listed_to_whoever_may_bring_it_back_and_to_nobody_else() {
    let s = with_documents(&MANAGE).await;
    let editor = s.world.thread("Editor").await;
    share(&s.world, "node", s.topic, "live", "doc.view", "any", editor).await;
    share(&s.world, "node", s.topic, "live", "doc.edit", "any", editor).await;
    let first = plan(&s, "delete", &[&s.path("Plan.md")]).await;
    ok(manage(
        &s,
        json!({ "command": "delete", "paths": [s.path("Plan.md")], "confirm": word_of(&first) }),
    )
    .await);
    let seen = listing(&s, s.agent, &s.docs).await;
    assert_eq!(entry(&seen, "Plan.md")["deleted"], true);
    let unseen = listing(&s, editor, &s.docs).await;
    let names: Vec<&str> = unseen["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|each| each["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Notes.md"],
        "an agent that cannot bring it back is not told of it"
    );
    // A new document of the same title does not take the name of the one that was deleted, nor it its.
    let again = s
        .world
        .document_beside(s.topic, "Plan", &["A new plan."])
        .await;
    let seen = listing(&s, s.agent, &s.docs).await;
    let entries: Vec<(&str, bool)> = seen["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|each| each["kind"] == "document")
        .map(|each| {
            (
                each["name"].as_str().unwrap(),
                each.get("deleted").is_some(),
            )
        })
        .collect();
    assert!(entries.contains(&("Plan.md", false)), "{entries:?}");
    assert!(
        entries
            .iter()
            .any(|(name, gone)| *gone && name.starts_with("Plan~")),
        "the deleted one is named apart: {entries:?}"
    );
    // Bringing it back where a document of its name stands already is refused, and says how to go on.
    let paths: Vec<String> = seen["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|each| each.get("deleted").is_some())
        .map(|each| each["path"].as_str().unwrap().to_owned())
        .collect();
    let restore = manage(&s, json!({ "command": "restore", "paths": paths })).await;
    let (code, message) = refused(&restore);
    assert_eq!(code, "exists");
    assert!(
        message.contains("choose another name or destination"),
        "{message}"
    );
    assert!(!deleted(&s.world, again).await);
}

// ---------------------------------------------------------------- earlier versions

#[tokio::test]
async fn an_earlier_version_is_put_back_while_the_present_text_is_kept_as_a_version() {
    let s = with_documents(&MANAGE).await;
    s.world.add_paragraph(s.plan, "Added later.").await;
    let before = s.world.blocks_of(s.plan).await;
    assert_eq!(before.len(), 3);
    let history = format!("{}Plan.history/", s.docs);
    let versions = listing(&s, s.agent, &history).await;
    let version = versions["entries"][0]["path"].as_str().unwrap().to_owned();
    assert_eq!(versions["entries"][0]["manageable"], true);
    let kept: i64 = s
        .world
        .count(
            "SELECT count(*) FROM document_versions WHERE document_id = $1",
            s.plan,
        )
        .await;
    let done = ok(manage(&s, json!({ "command": "restore", "paths": [version] })).await);
    assert_eq!(done["done"], true);
    assert!(
        done.get("confirm").is_none(),
        "version restoration needs no confirmation call"
    );
    assert_eq!(done["results"][0]["blocks"], 2, "what the version holds");
    assert_eq!(done["results"][0]["action"], "put_back");
    assert_eq!(done["results"][0]["new_path"], json!(s.path("Plan.md")));
    assert_eq!(
        s.world.blocks_of(s.plan).await.len(),
        2,
        "the older text stands again"
    );
    let now_kept: i64 = s
        .world
        .count(
            "SELECT count(*) FROM document_versions WHERE document_id = $1",
            s.plan,
        )
        .await;
    assert_eq!(
        now_kept,
        kept + 1,
        "the text it replaced is a version of its own"
    );
    let lines = lines(&s.world, "document.version_restored", s.plan).await;
    assert_eq!(
        lines,
        [("agent".to_owned(), Some(s.agent), true, Some(s.topic))]
    );
    // A version is put back and neither deleted nor moved.
    let refusal = manage(&s, json!({ "command": "delete", "paths": [history] })).await;
    assert_eq!(refused(&refusal).0, "wrong_kind");
}

// ---------------------------------------------------------------- moving

#[tokio::test]
async fn a_document_is_moved_to_a_docs_directory_the_agent_may_make_documents_in_and_keeps_its_history()
 {
    let s = with_documents(&MANAGE).await;
    let archive = s.world.topic("Archive").await;
    share(
        &s.world, "node", archive, "live", "doc.view", "any", s.agent,
    )
    .await;
    share(
        &s.world, "node", archive, "live", "doc.edit", "any", s.agent,
    )
    .await;
    let listed = listing(&s, s.agent, &format!("id:{archive}")).await;
    let target = entry(&listed, "docs")["path"].as_str().unwrap().to_owned();
    assert_eq!(entry(&listed, "docs")["writable"], true);
    s.world.add_paragraph(s.plan, "Added later.").await;
    // Two people see the document by the place it lies in or by the place it goes to, and not by both: Bob gains it by
    // the move and Cai loses it.
    let (bob, cai) = (s.world.person("bob").await, s.world.person("cai").await);
    {
        let mut conn = s.world.pool.acquire().await.unwrap();
        for (node, person) in [(archive, bob), (s.topic, cai)] {
            orch_db::queries::access::testing::give_role(
                &mut conn,
                node,
                "user",
                person,
                "viewer",
                s.world.ada,
                None,
            )
            .await
            .unwrap();
        }
    }
    let owner: Id = s
        .world
        .scalar("SELECT owner_id FROM documents WHERE id = $1", s.plan)
        .await;
    let versions: i64 = s
        .world
        .count(
            "SELECT count(*) FROM document_versions WHERE document_id = $1",
            s.plan,
        )
        .await;
    let before = s.world.blocks_of(s.plan).await;
    let done = ok(manage(
        &s,
        json!({ "command": "move", "paths": [s.path("Plan.md")], "to": target }),
    )
    .await);
    assert_eq!(done["done"], true);
    assert!(done.get("confirm").is_none(), "move executes directly");
    let item = &done["results"][0];
    assert_eq!(item["action"], "move");
    assert_eq!(item["to"], json!(target));
    assert_eq!(
        (item["gaining"].as_i64(), item["losing"].as_i64()),
        (Some(1), Some(1)),
        "the result says how many people gain and lose it: Bob only where it goes, Cai only where it lies: {item}"
    );
    assert_eq!(
        done["results"][0]["new_path"],
        json!(format!("{target}Plan.md"))
    );
    let node: Id = s
        .world
        .scalar("SELECT node_id FROM documents WHERE id = $1", s.plan)
        .await;
    assert_eq!(node, archive);
    assert_eq!(s.world.blocks_of(s.plan).await, before);
    let still: Id = s
        .world
        .scalar("SELECT owner_id FROM documents WHERE id = $1", s.plan)
        .await;
    assert_eq!(still, owner, "it keeps its owners");
    assert_eq!(
        s.world
            .count(
                "SELECT count(*) FROM document_versions WHERE document_id = $1",
                s.plan
            )
            .await,
        versions,
        "and its history"
    );
    let moved = lines(&s.world, "document.moved", s.plan).await;
    assert_eq!(
        moved,
        [("agent".to_owned(), Some(s.agent), true, Some(archive))]
    );
    let listed = ok(call(
        &s.world,
        s.agent,
        OrchTool::ListShared,
        json!({"path":target,"ids":true}),
    )
    .await);
    assert_eq!(entry(&listed, "Plan.md")["id"], json!(s.plan));
}

#[tokio::test]
async fn a_move_to_a_place_the_agent_may_only_read_or_into_a_name_that_is_taken_is_refused() {
    let s = with_documents(&MANAGE).await;
    let reading = s.world.topic("Reading").await;
    s.world.document_beside(reading, "Old", &["Old."]).await;
    share(
        &s.world, "node", reading, "live", "doc.view", "any", s.agent,
    )
    .await;
    let listed = listing(&s, s.agent, &format!("id:{reading}")).await;
    let read_only = entry(&listed, "docs")["path"].as_str().unwrap().to_owned();
    let refusal = manage(
        &s,
        json!({ "command": "move", "paths": [s.path("Plan.md")], "to": read_only }),
    )
    .await;
    let (code, message) = refused(&refusal);
    assert_eq!(code, "not_writable");
    assert!(message.contains("doc.edit"), "{message}");
    // A name that is taken where it would go.
    let archive = s.world.topic("Archive").await;
    s.world
        .document_beside(archive, "Plan", &["Another plan."])
        .await;
    share(
        &s.world, "node", archive, "live", "doc.view", "any", s.agent,
    )
    .await;
    share(
        &s.world, "node", archive, "live", "doc.edit", "any", s.agent,
    )
    .await;
    let listed = listing(&s, s.agent, &format!("id:{archive}")).await;
    let target = entry(&listed, "docs")["path"].as_str().unwrap().to_owned();
    let before = fingerprint(&s.world).await;
    let conflict = manage(
        &s,
        json!({ "command": "move", "paths": [s.path("Plan.md"), s.path("Notes.md")], "to": target }),
    ).await;
    assert_eq!(refused(&conflict).0, "exists");
    assert_eq!(
        fingerprint(&s.world).await,
        before,
        "one collision refuses the complete direct move"
    );
    let notes_at: Id = s
        .world
        .scalar("SELECT node_id FROM documents WHERE id=$1", s.notes)
        .await;
    assert_eq!(
        notes_at, s.topic,
        "a non-conflicting sibling is not moved partially"
    );
    // To the place it is in already, and to a path that is no docs directory.
    let here = manage(
        &s,
        json!({ "command": "move", "paths": [s.path("Plan.md")], "to": s.docs }),
    )
    .await;
    assert_eq!(refused(&here).0, "invalid");
    let plain = manage(
        &s,
        json!({ "command": "move", "paths": [s.path("Plan.md")], "to": s.path("Notes.md") }),
    )
    .await;
    assert_eq!(refused(&plain).0, "wrong_kind");
}

#[tokio::test]
async fn copy_and_move_require_a_destination_and_other_commands_refuse_one() {
    let s = with_documents(&MANAGE).await;
    for command in ["create_folder", "delete", "restore"] {
        let with_to = manage(
            &s,
            json!({ "command": command, "paths": [s.path("Plan.md")], "to": s.docs }),
        )
        .await;
        assert_eq!(refused(&with_to).0, "invalid");
    }
    for command in ["copy", "move"] {
        let without = manage(
            &s,
            json!({ "command": command, "paths": [s.path("Plan.md")] }),
        )
        .await;
        let (code, message) = refused(&without);
        assert_eq!(code, "invalid");
        assert!(message.contains("copy or move needs \"to\""), "{message}");
    }
    let too_many = manage(
        &s,
        json!({ "command": "delete", "paths": vec![s.path("Plan.md"); 21] }),
    )
    .await;
    let (code, message) = refused(&too_many);
    assert_eq!(code, "invalid");
    assert!(message.contains("at most 20"), "{message}");
}

#[tokio::test]
async fn non_destructive_operations_refuse_confirmation_arguments_without_changing_anything() {
    let s = with_documents(&MANAGE).await;
    let before = fingerprint(&s.world).await;
    for command in ["create_folder", "copy", "move", "restore"] {
        let arguments = if matches!(command, "copy" | "move") {
            json!({ "command": command, "paths": [s.path("Plan.md")], "to": s.docs,
                "confirm": "0123456789abcdef0123456789abcdef" })
        } else if command == "create_folder" {
            json!({ "command": command, "paths": [s.path("new-folder/")],
                "confirm": "0123456789abcdef0123456789abcdef" })
        } else {
            json!({ "command": command, "paths": [s.path("Plan.md")],
                "confirm": "0123456789abcdef0123456789abcdef" })
        };
        let answer = manage(&s, arguments).await;
        assert_eq!(refused(&answer).0, "invalid");
        assert!(refused(&answer).1.contains("Only delete takes confirm"));
        assert_eq!(fingerprint(&s.world).await, before);
    }
}

// ---------------------------------------------------------------- who else it is shared with

#[tokio::test]
async fn the_plan_says_how_many_other_places_the_document_is_shared_with() {
    let s = with_documents(&MANAGE).await;
    let other = s.world.thread("Other").await;
    share(
        &s.world, "document", s.plan, "live", "doc.view", "any", other,
    )
    .await;
    let answer = plan(&s, "delete", &[&s.path("Plan.md"), &s.path("Notes.md")]).await;
    let shared = |name: &str| {
        answer["affects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == name)
            .unwrap()["shared_with"]
            .clone()
    };
    assert_eq!(shared("Plan.md"), 1, "the agents of Other reach it too");
    assert_eq!(shared("Notes.md"), 0);
    // A share that came after the plan makes the act another plan: it is part of what the delete takes it away from.
    let first = plan(&s, "delete", &[&s.path("Notes.md")]).await;
    let third = s.world.thread("Third").await;
    share(
        &s.world, "document", s.notes, "live", "doc.view", "any", third,
    )
    .await;
    let late = manage(
        &s,
        json!({ "command": "delete", "paths": [s.path("Notes.md")], "confirm": word_of(&first) }),
    )
    .await;
    assert_eq!(refused(&late).0, "not_confirmed");
    assert!(!deleted(&s.world, s.notes).await);
}
