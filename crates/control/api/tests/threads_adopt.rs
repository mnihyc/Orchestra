//! A session a person began in a harness by hand taken in as a thread over HTTP: found by asking the harness, put in the folder of theirs that holds the directory it ran in, checked as any thread is for what the caller may use, taken in once with what it held before and the model it ran on, said before anything happens, and opened as one that was adopted when its first turn begins.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{
    AdoptPlan, FieldSource, ThreadAdopt, ThreadAdoptBody, ThreadAdoptPreview,
};
use orch_api_contract::resources::ProviderHarnessSessionList;
use orch_api_contract::threads::{
    AdoptHistoryMissing, AdoptModelReason, Item, ItemPayload, MarkerPayload, Thread,
    ThreadItemList, ThreadItemListQuery,
};
use orch_link_contract::commands::{
    AgentDiscoverSessionsResult, AgentReadSessionResult, AgentStartResult, Command, CommandKind,
    CommandResult, RecordRef, SessionMode, SessionOpenResult, SessionResyncResult,
};
use orch_settings::Target;
use orch_types::{Id, new_id};
use places::{Who, missing_permissions};
use rig::{MODEL, Scene};
use serde_json::json;
use std::sync::{Arc, Mutex};

/// What the harness holds: sessions by reference, each with a directory it ran in.
const SESSIONS: [(&str, &str, Option<&str>); 3] = [
    ("native-a", "/work/web", Some("Fix the login")),
    ("native-b", "/elsewhere/api", None),
    ("native-c", "/work", Some("The whole folder")),
];

/// The harness can say which sessions it holds, and the machine lists them.
async fn the_harness_lists_its_sessions(s: &Scene) {
    sqlx::query(
        "UPDATE agents SET descriptor = jsonb_set(descriptor, '{capabilities,discover_sessions}', 'true') WHERE id = $1",
    )
    .bind(s.on.agent)
    .execute(s.rig.db.pool())
    .await
    .unwrap();
    let mut declares = rig::descriptor();
    declares["capabilities"]["discover_sessions"] = json!(true);
    s.rig.machine.handler.on(CommandKind::AgentStart, move |_| {
        Ok(CommandResult::AgentStart(AgentStartResult {
            reused: false,
            harness_version: "1.0.0".to_owned(),
            descriptor: serde_json::from_value(declares.clone()).unwrap(),
        }))
    });
    s.rig
        .machine
        .handler
        .on(CommandKind::AgentDiscoverSessions, move |_| {
            Ok(CommandResult::AgentDiscoverSessions(
                AgentDiscoverSessionsResult {
                    sessions: serde_json::from_value(serde_json::to_value(SESSIONS
                        .iter()
                        .map(|(native, cwd, title)| {
                            json!({
                                "native_session_ref": native, "title": title, "cwd": cwd,
                                "updated_at": "2026-10-12T08:00:00.000Z", "adopted_thread_id": null
                            })
                        })
                        .collect::<Vec<_>>()).unwrap()).unwrap(),
                },
            ))
        });
}

/// A directory of Ada's machine, registered as an environment of hers.
async fn registered(s: &Scene, root: &str) -> Id {
    let environment = new_id();
    let mut conn = s.rig.db.acquire().await.unwrap();
    // The machine is Ada's, as enrolling it from her account makes it: its first owner, in its Owners row.
    sqlx::query("UPDATE machines SET owner_id = $2 WHERE id = $1")
        .bind(s.rig.machine_id)
        .bind(s.ada.id())
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO permission_entries (id, object_kind, object_id, page, permission, holder_kind, holder_id,
                                         created_by, source)
         VALUES ($1, 'machine', $2, 'live', 'access.own', 'user', $3, $3, 'creation')
         ON CONFLICT DO NOTHING",
    )
    .bind(new_id())
    .bind(s.rig.machine_id)
    .bind(s.ada.id())
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root,
                                   machine_id, generation, status)
         VALUES ($1, $7, $2, $3, 'connected', $4, $5, $6, 1, 'running')",
    )
    .bind(environment)
    .bind(s.ada.id())
    .bind(s.tree.org.base.id)
    .bind(s.tree.topic.base.id)
    .bind(root)
    .bind(s.rig.machine_id)
    .bind(format!("Folder {root}"))
    .execute(&mut *conn)
    .await
    .unwrap();
    orch_db::queries::access::insert_entry(
        &mut conn,
        &orch_db::queries::access::NewEntry {
            id: new_id(),
            page: &orch_db::queries::access::PageKey::new("environment", environment, "live"),
            profile: "any",
            permission: "access.own",
            holder_kind: "user",
            holder_id: Some(s.ada.id()),
            holder_role: None,
            options: &json!({}),
            expires_at: None,
            note: "",
            source: "creation",
            created_by: Some(s.ada.id()),
            at: s.rig.world.now(),
        },
    )
    .await
    .unwrap();
    environment
}

async fn adopt(
    s: &Scene,
    by: &Who,
    native: &str,
    environment: Option<Id>,
    name: Option<&str>,
) -> orch_api_core::testing::Reply<Thread> {
    s.rig
        .world
        .app
        .call(ThreadAdopt, &[&s.tree.topic.base.id])
        .token(&by.token)
        .body(&ThreadAdoptBody {
            provider_id: s.on.provider,
            agent_id: s.on.agent,
            native_session_ref: native.to_owned(),
            environment_id: environment,
            name: name.map(str::to_owned),
        })
        .send()
        .await
}

async fn directory_of(s: &Scene, thread: Id) -> String {
    sqlx::query_scalar("SELECT workspace_dir FROM threads WHERE node_id = $1")
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

async fn segment_ref(s: &Scene, thread: Id) -> Option<String> {
    sqlx::query_scalar("SELECT native_session_ref FROM segments WHERE thread_id = $1")
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

/// Whether the session of the thread waits to be opened by Orch (`pending`) or was (`opened`).
async fn adopted_state(s: &Scene, thread: Id) -> Option<String> {
    sqlx::query_scalar("SELECT import_report ->> 'adopted' FROM segments WHERE thread_id = $1")
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_session_begun_by_hand_becomes_a_thread_in_the_folder_that_holds_its_directory() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let work = registered(&s, "/work").await;
    let web = registered(&s, "/work/web").await;

    let thread = adopt(&s, &s.ada, "native-a", None, None).await.ok();
    assert_eq!(thread.base.owner.username, "ada", "it is the caller's");
    assert_eq!(
        thread.environment.as_ref().map(|each| each.id),
        Some(web),
        "the folder nearest to the directory holds it"
    );
    assert_eq!(
        directory_of(&s, thread.base.id).await,
        "",
        "the folder is the directory itself"
    );
    assert_eq!(thread.base.name, "Fix the login", "the session's own title");
    assert_eq!(thread.base.title_source, FieldSource::Auto);
    assert_eq!(
        segment_ref(&s, thread.base.id).await.as_deref(),
        Some("native-a")
    );
    assert_eq!(
        adopted_state(&s, thread.base.id).await.as_deref(),
        Some("pending"),
        "Orch has yet to open it"
    );

    // A session that ran in the folder itself, and one that ran below the folder of another.
    let whole = adopt(&s, &s.ada, "native-c", None, None).await.ok();
    assert_eq!(whole.environment.as_ref().map(|each| each.id), Some(work));
    assert_eq!(directory_of(&s, whole.base.id).await, "");

    // The machine's page lists it as adopted, by the thread.
    let listed = s
        .rig
        .world
        .app
        .call(ProviderHarnessSessionList, &[&s.on.provider, &s.on.agent])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let by_ref: Vec<(&str, Option<Id>)> = listed
        .iter()
        .map(|each| (each.native_session_ref.as_str(), each.adopted_thread_id))
        .collect();
    assert!(by_ref.contains(&("native-a", Some(thread.base.id))));
    assert!(by_ref.contains(&("native-b", None)));
}

#[tokio::test]
async fn a_directory_below_the_folder_is_the_directory_of_the_thread() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work").await;
    let thread = adopt(&s, &s.ada, "native-a", None, Some("Mine")).await.ok();
    assert_eq!(directory_of(&s, thread.base.id).await, "web");
    assert_eq!(thread.base.name, "Mine");
    assert_eq!(
        thread.base.title_source,
        FieldSource::User,
        "a name given is the person's"
    );
}

/// What a machine was asked when sessions were opened: how, and under which name.
type Opens = Arc<Mutex<Vec<(SessionMode, Option<String>)>>>;

/// The machine opens a session under the name it is given, and says what it was asked.
fn the_machine_opens_sessions_under_the_names_it_is_given(s: &Scene) -> Opens {
    let opened = Opens::default();
    let seen = Arc::clone(&opened);
    s.rig
        .machine
        .handler
        .on(CommandKind::SessionOpen, move |command| {
            let Command::SessionOpen(open) = command else {
                unreachable!("only session opens come here");
            };
            seen.lock()
                .unwrap()
                .push((open.mode, open.native_session_ref.clone()));
            Ok(CommandResult::SessionOpen(SessionOpenResult {
                native_session_ref: open
                    .native_session_ref
                    .clone()
                    .unwrap_or_else(|| "native-new".to_owned()),
                resumed: true,
                import_report: None,
                instruction_sources: Vec::new(),
                approval_reviewer: None,
            }))
        });
    s.rig.machine.handler.on(CommandKind::SessionResync, |_| {
        Ok(CommandResult::SessionResync(SessionResyncResult {
            items: 0,
        }))
    });
    opened
}

#[tokio::test]
async fn the_first_turn_opens_the_session_as_one_that_was_adopted_and_asks_nothing_of_its_history_again()
 {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work").await;
    let thread = adopt(&s, &s.ada, "native-a", None, None).await.ok();
    s.rig
        .world
        .set_at(Target::Node(thread.base.id), "agent.model", json!(MODEL))
        .await;
    let opened = the_machine_opens_sessions_under_the_names_it_is_given(&s);
    s.exchange(thread.base.id, 0, "carry on", "carried on")
        .await;
    assert_eq!(
        opened.lock().unwrap().as_slice(),
        [(SessionMode::Adopt, Some("native-a".to_owned()))],
        "the harness is asked to take the session up as one it did not begin for Orch"
    );
    assert_eq!(
        s.rig.commands.issued_of("session.resync"),
        0,
        "what the session held before was read when it was adopted, and is not asked for again"
    );
    assert_eq!(
        s.rig.commands.issued_of("agent.read_session"),
        1,
        "it was read once, when it was adopted"
    );
    assert_eq!(
        segment_ref(&s, thread.base.id).await.as_deref(),
        Some("native-a")
    );
    assert_eq!(
        adopted_state(&s, thread.base.id).await.as_deref(),
        Some("opened"),
        "what opens the session after this is a resume"
    );
}

/// The harness recorded `entries` of the sessions it holds, and says they ran on `model`: the record is put where
/// a connector would upload it, and the machine answers with it.
async fn the_harness_recorded(s: &Scene, model: Option<&str>, entries: &[serde_json::Value]) {
    let text: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
    // The machine uploads the record to the application's store, which the engine was given when the rig made it.
    let store = &s.rig.world.app.app.blobs;
    let kept = store
        .put_bytes(bytes::Bytes::from(text.into_bytes()))
        .await
        .unwrap();
    let report = json!({
        "fidelity": "structured", "items_total": entries.len(), "by_kind": {}, "dropped": [], "warnings": []
    });
    let model = model.map(str::to_owned);
    let count = i64::try_from(entries.len()).unwrap();
    s.rig
        .machine
        .handler
        .on(CommandKind::AgentReadSession, move |command| {
            let Command::AgentReadSession(read) = command else {
                unreachable!("only reads of a record come here");
            };
            Ok(CommandResult::AgentReadSession(AgentReadSessionResult {
                model: model.clone(),
                effort: Some("high".to_owned()),
                turns: 2,
                entries: if read.write { count } else { 0 },
                record: read.write.then_some(RecordRef {
                    blob: kept.hash,
                    size: kept.size,
                }),
                report: serde_json::from_value(report.clone()).unwrap(),
            }))
        });
}

/// Two turns of a session: what a person said and what the agent answered, and a command it ran.
fn two_turns() -> Vec<serde_json::Value> {
    vec![
        json!({ "entry": "said", "native_ref": "said:u1", "turn": 1, "text": "Fix the login", "left_out": 0,
                "at": "2026-10-11T09:00:00.000Z" }),
        json!({ "entry": "item", "native_ref": "msg-1", "turn": 1, "stream": "main", "parent_native_ref": null,
                "status": "completed", "at": "2026-10-11T09:00:05.000Z",
                "content": { "kind": "agent_message", "payload": { "text": "Fixed the handler.", "text_len": 18, "phase": "final" } } }),
        json!({ "entry": "said", "native_ref": "said:u2", "turn": 2, "text": "Thanks, run the tests", "left_out": 0,
                "at": "2026-10-11T09:10:00.000Z" }),
        json!({ "entry": "item", "native_ref": "msg-2", "turn": 2, "stream": "main", "parent_native_ref": null,
                "status": "completed", "at": "2026-10-11T09:10:09.000Z",
                "content": { "kind": "agent_message", "payload": { "text": "All 42 pass.", "text_len": 12, "phase": "final" } } }),
    ]
}

/// The items of a thread, oldest first.
async fn items_of(s: &Scene, thread: Id) -> Vec<Item> {
    s.rig
        .world
        .app
        .call(ThreadItemList, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadItemListQuery {
            after: Some(0),
            limit: Some(100),
            ..ThreadItemListQuery::default()
        })
        .send()
        .await
        .ok()
        .items
}

/// The line that says the session was adopted.
fn adopted_line(items: &[Item]) -> orch_api_contract::threads::MarkerAdopted {
    items
        .iter()
        .find_map(|item| match &item.payload {
            ItemPayload::Marker(MarkerPayload::Adopted(line)) => Some(line.clone()),
            _ => None,
        })
        .expect("a line says the session was adopted")
}

#[tokio::test]
async fn what_the_session_held_before_comes_with_it_once_as_items_from_the_session_before_orch() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
    registered(&s, "/work").await;
    let thread = adopt(&s, &s.ada, "native-a", None, None).await.ok().base.id;

    let items = items_of(&s, thread).await;
    let before: Vec<&Item> = items.iter().filter(|item| item.before_orch).collect();
    let said: Vec<String> = before
        .iter()
        .map(|item| match &item.payload {
            ItemPayload::UserMessage(message) => {
                format!("person: {}", serde_json::to_string(&message.parts).unwrap())
            }
            ItemPayload::AgentMessage(message) => format!("agent: {}", message.text),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(said.len(), 4, "{said:?}");
    assert!(
        said[0].starts_with("person:") && said[0].contains("Fix the login"),
        "{said:?}"
    );
    assert_eq!(said[1], "agent: Fixed the handler.");
    assert!(said[2].contains("run the tests"));
    assert_eq!(said[3], "agent: All 42 pass.");
    assert!(
        before.iter().all(|item| item.turn_id.is_none()),
        "they belong to no turn of the thread"
    );
    let person = before[0]
        .author
        .as_ref()
        .expect("a person's words have their author");
    assert_eq!(
        person.username, "ada",
        "the owner of the machine, who is the harness's user there"
    );
    assert!(
        before
            .iter()
            .skip(1)
            .step_by(2)
            .all(|item| item.author.is_none()),
        "what the agent said is nobody's"
    );
    assert!(
        before[0]
            .created_at
            .to_string()
            .starts_with("2026-10-11T09:00:00"),
        "when it was said: {}",
        before[0].created_at
    );
    let line = adopted_line(&items);
    assert_eq!((line.turns, line.items, line.history_missing), (2, 4, None));
    assert_eq!(line.native_session_ref, "native-a");

    // Asked again, nothing is taken in twice: the session has its thread.
    let again = adopt(&s, &s.ada, "native-a", None, None).await.problem();
    assert_eq!(again.code, ErrorCode::ConflictExists);
    assert_eq!(
        items_of(&s, thread)
            .await
            .iter()
            .filter(|item| item.before_orch)
            .count(),
        4
    );
}

#[tokio::test]
async fn the_thread_runs_on_the_model_the_session_ran_on_where_the_provider_offers_it() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
    registered(&s, "/work").await;
    let thread = adopt(&s, &s.ada, "native-a", None, None).await.ok();
    assert_eq!(thread.model.as_deref(), Some(MODEL));
    assert_eq!(
        thread.effort.as_deref(),
        Some("high"),
        "with the effort it ran with, which the model takes"
    );
    let line = adopted_line(&items_of(&s, thread.base.id).await);
    assert_eq!(line.session_model.as_deref(), Some(MODEL));
    assert_eq!(line.model_reason, None);
}

#[tokio::test]
async fn a_model_the_provider_does_not_offer_is_left_for_a_person_to_choose_and_the_line_says_why()
{
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    the_harness_recorded(&s, Some("gpt-elsewhere"), &two_turns()).await;
    registered(&s, "/work").await;
    let thread = adopt(&s, &s.ada, "native-a", None, None).await.ok();
    assert_eq!(thread.model, None, "a person chooses one");
    let line = adopted_line(&items_of(&s, thread.base.id).await);
    assert_eq!(line.session_model.as_deref(), Some("gpt-elsewhere"));
    assert_eq!(line.model_reason, Some(AdoptModelReason::NotOffered));
}

#[tokio::test]
async fn a_harness_that_cannot_read_what_it_recorded_still_gives_the_session_and_says_so() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work").await;
    let thread = adopt(&s, &s.ada, "native-a", None, None).await.ok();
    let items = items_of(&s, thread.base.id).await;
    assert!(items.iter().all(|item| !item.before_orch));
    let line = adopted_line(&items);
    assert_eq!(line.history_missing, Some(AdoptHistoryMissing::Unsupported));
    assert_eq!(line.model_reason, Some(AdoptModelReason::NotReported));
    assert_eq!(thread.model, None);
}

#[tokio::test]
async fn a_manager_retries_missing_adopted_history_once_without_sending_it_or_changing_the_model() {
    use orch_api_contract::threads::ThreadAdoptRetryHistory;
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work").await;
    a_history_read_fails(&s);
    let thread = adopt(&s, &s.ada, "native-a", None, None).await.ok();
    let id = thread.base.id;
    let before = adopted_line(&items_of(&s, id).await);
    assert_eq!(
        before.history_missing,
        Some(AdoptHistoryMissing::Unreadable)
    );
    the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
    let read_before = s.rig.commands.issued_of("agent.read_session");
    let tried = s
        .rig
        .world
        .app
        .call(ThreadAdoptRetryHistory, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        tried.model, thread.model,
        "recovering history does not silently choose a model"
    );
    let items = items_of(&s, id).await;
    assert_eq!(items.iter().filter(|item| item.before_orch).count(), 4);
    let after = adopted_line(&items);
    assert_eq!(after.history_missing, None);
    assert_eq!((after.turns, after.items), (2, 4));
    assert_eq!(after.model_reason, before.model_reason);
    assert!(
        items
            .iter()
            .filter(|item| item.before_orch)
            .all(|item| item.turn_id.is_none())
    );
    assert_eq!(s.rig.commands.issued_of("turn.start"), 0);
    assert_eq!(s.rig.commands.issued_of("session.open"), 0);
    s.rig
        .world
        .app
        .call(ThreadAdoptRetryHistory, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(
        items_of(&s, id).await.len(),
        items.len(),
        "the retry adds neither a second marker nor duplicate history"
    );
    assert_eq!(
        s.rig.commands.issued_of("agent.read_session"),
        read_before + 1,
        "already-recovered history needs no second native read"
    );
}

#[tokio::test]
async fn a_history_retry_needs_thread_manage_and_a_failed_retry_preserves_its_readable_reason() {
    use orch_api_contract::threads::ThreadAdoptRetryHistory;
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work").await;
    a_history_read_fails(&s);
    let id = adopt(&s, &s.ada, "native-a", None, None).await.ok().base.id;
    let viewer = s.rig.person("viewer").await;
    s.rig
        .lets_use("node", id, "thread.view", &viewer, json!({}))
        .await;
    let reads = s.rig.commands.issued_of("agent.read_session");
    let denied = s
        .rig
        .world
        .app
        .call(ThreadAdoptRetryHistory, &[&id])
        .token(&viewer.token)
        .send()
        .await;
    assert!(denied.body.is_err());
    assert_eq!(s.rig.commands.issued_of("agent.read_session"), reads);
    let before = items_of(&s, id).await;
    let failed = s
        .rig
        .world
        .app
        .call(ThreadAdoptRetryHistory, &[&id])
        .token(&s.ada.token)
        .send()
        .await;
    failed.ok();
    let after = items_of(&s, id).await;
    assert_eq!(after.len(), before.len());
    assert_eq!(
        adopted_line(&after).history_missing,
        Some(AdoptHistoryMissing::Unreadable)
    );
    assert!(
        adopted_line(&after)
            .history_error
            .as_deref()
            .is_some_and(|why| why.contains("The original session file could not be opened"))
    );
    assert_eq!(s.rig.commands.issued_of("turn.start"), 0);
}

#[tokio::test]
async fn a_retry_imports_the_original_history_and_keeps_later_session_work_out_of_the_before_orch_band()
 {
    use orch_api_contract::threads::ThreadAdoptRetryHistory;
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work").await;
    a_history_read_fails(&s);
    let id = adopt(&s, &s.ada, "native-a", None, None).await.ok().base.id;
    let mut record = two_turns();
    record.push(json!({"entry":"said","native_ref":"said:after-adoption","turn":3,"text":"Later work in the session","left_out":0,"at":"2026-10-12T09:00:00.000Z"}));
    the_harness_recorded(&s, Some(MODEL), &record).await;
    s.rig
        .world
        .app
        .call(ThreadAdoptRetryHistory, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let items = items_of(&s, id).await;
    assert_eq!(items.iter().filter(|item| item.before_orch).count(), 4);
    assert!(
        !serde_json::to_string(&items)
            .unwrap()
            .contains("Later work in the session")
    );
    assert_eq!(adopted_line(&items).turns, 2);
}

fn a_history_read_fails(s: &Scene) {
    s.rig
        .machine
        .handler
        .on(CommandKind::AgentReadSession, |_| {
            Err(orch_link_contract::commands::CommandError::new(
                orch_link_contract::commands::ErrorCode::Internal,
                "The original session file could not be opened",
            ))
        });
}

#[tokio::test]
async fn the_before_orch_band_says_the_native_failure_and_offline_machine_queues_no_read() {
    use orch_api_contract::threads::{
        AdoptHistoryState, ThreadAdoptHistoryGet, ThreadAdoptRetryHistory,
    };
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work").await;
    a_history_read_fails(&s);
    let id = adopt(&s, &s.ada, "native-a", None, None).await.ok().base.id;
    let status = s
        .rig
        .world
        .app
        .call(ThreadAdoptHistoryGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(status.state, AdoptHistoryState::Unreadable);
    assert!(status.can_read_again);
    assert!(
        status
            .reason
            .as_deref()
            .is_some_and(|why| why.contains("The original session file could not be opened"))
    );
    let viewer = s.rig.person("watcher").await;
    s.rig
        .lets_use("node", id, "thread.view", &viewer, json!({}))
        .await;
    let status = s
        .rig
        .world
        .app
        .call(ThreadAdoptHistoryGet, &[&id])
        .token(&viewer.token)
        .send()
        .await
        .ok()
        .unwrap();
    assert!(!status.can_read_again, "only a current manager has the act");
    let reads = s.rig.commands.issued_of("agent.read_session");
    let discoveries = s.rig.commands.issued_of("agent.discover_sessions");
    sqlx::query("UPDATE machines SET online=false WHERE id=$1")
        .bind(s.rig.machine_id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let status = s
        .rig
        .world
        .app
        .call(ThreadAdoptHistoryGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(status.state, AdoptHistoryState::Offline);
    assert!(!status.can_read_again);
    assert!(
        s.rig
            .world
            .app
            .call(ThreadAdoptRetryHistory, &[&id])
            .token(&s.ada.token)
            .send()
            .await
            .body
            .is_err()
    );
    assert_eq!(s.rig.commands.issued_of("agent.read_session"), reads);
    assert_eq!(
        s.rig.commands.issued_of("agent.discover_sessions"),
        discoveries,
        "an offline retry returns immediately rather than queueing work"
    );
}

#[tokio::test]
async fn an_adapter_without_a_session_reader_says_so_once_and_offers_no_retry() {
    use orch_api_contract::threads::{
        AdoptHistoryState, ThreadAdoptHistoryGet, ThreadAdoptRetryHistory,
    };
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work").await;
    let id = adopt(&s, &s.ada, "native-a", None, None).await.ok().base.id;
    let reads = s.rig.commands.issued_of("agent.read_session");
    let status = s
        .rig
        .world
        .app
        .call(ThreadAdoptHistoryGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(status.state, AdoptHistoryState::Unsupported);
    assert!(!status.can_read_again);
    assert!(
        s.rig
            .world
            .app
            .call(ThreadAdoptRetryHistory, &[&id])
            .token(&s.ada.token)
            .send()
            .await
            .body
            .is_err()
    );
    assert_eq!(
        s.rig.commands.issued_of("agent.read_session"),
        reads,
        "unsupported is a fact, not a repeatedly failing action"
    );
}

async fn plan(
    s: &Scene,
    native: &str,
    environment: Option<Id>,
) -> orch_api_core::testing::Reply<AdoptPlan> {
    s.rig
        .world
        .app
        .call(ThreadAdoptPreview, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&ThreadAdoptBody {
            provider_id: s.on.provider,
            agent_id: s.on.agent,
            native_session_ref: native.to_owned(),
            environment_id: environment,
            name: None,
        })
        .send()
        .await
}

#[tokio::test]
async fn the_plan_says_where_it_would_stand_the_model_and_the_history_before_anything_happens() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    the_harness_recorded(&s, Some(MODEL), &two_turns()).await;
    let work = registered(&s, "/work").await;

    let planned = plan(&s, "native-a", None).await.ok();
    assert_eq!(planned.environment.as_ref().map(|each| each.id), Some(work));
    assert_eq!(planned.directory.as_deref(), Some("web"));
    assert_eq!(planned.register_first, None);
    assert_eq!(planned.model.as_deref(), Some(MODEL));
    assert_eq!(planned.turns, 2);
    assert_eq!(planned.name, "Fix the login");
    assert_eq!(planned.adopted_thread_id, None);
    let threads: i64 =
        sqlx::query_scalar("SELECT count(*) FROM segments WHERE native_session_ref = 'native-a'")
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(threads, 0, "nothing was made");

    // A session whose folder is registered nowhere: the plan names the folder to register first.
    let elsewhere = plan(&s, "native-b", None).await.ok();
    assert_eq!(elsewhere.environment, None);
    assert_eq!(elsewhere.register_first.as_deref(), Some("/elsewhere/api"));

    // Once adopted, the plan names the thread that has it.
    let thread = adopt(&s, &s.ada, "native-a", None, None).await.ok();
    let after = plan(&s, "native-a", None).await.ok();
    assert_eq!(after.adopted_thread_id, Some(thread.base.id));
}

#[tokio::test]
async fn the_plan_tells_nothing_to_those_who_may_not_adopt() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work").await;
    let refused = s
        .rig
        .world
        .app
        .call(ThreadAdoptPreview, &[&s.tree.topic.base.id])
        .token(&s.cai.token)
        .body(&ThreadAdoptBody {
            provider_id: s.on.provider,
            agent_id: s.on.agent,
            native_session_ref: "native-a".to_owned(),
            environment_id: None,
            name: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(s.rig.commands.issued_of("agent.discover_sessions"), 0);
}

#[tokio::test]
async fn a_session_is_taken_in_by_one_thread() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work").await;
    let first = adopt(&s, &s.ada, "native-a", None, None).await.ok();
    let again = adopt(&s, &s.ada, "native-a", None, None).await.problem();
    assert_eq!(again.code, ErrorCode::ConflictExists);
    assert_eq!(again.extra["existing_id"], json!(first.base.id));
}

#[tokio::test]
async fn a_session_the_harness_does_not_hold_is_not_there_and_a_harness_that_cannot_list_is_said_to()
 {
    let s = Scene::new().await;
    registered(&s, "/work").await;
    let unsupported = adopt(&s, &s.ada, "native-a", None, None).await.problem();
    assert_eq!(unsupported.code, ErrorCode::CapabilityUnsupported);
    the_harness_lists_its_sessions(&s).await;
    let missing = adopt(&s, &s.ada, "native-nowhere", None, None)
        .await
        .problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
    let blank = adopt(&s, &s.ada, "  ", None, None).await.problem();
    assert_eq!(blank.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn without_an_environment_that_holds_the_directory_the_folder_is_to_be_registered_first() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let work = registered(&s, "/work").await;
    // The session ran in a directory no folder of the caller's holds.
    let none = adopt(&s, &s.ada, "native-b", None, None).await.problem();
    assert_eq!(none.code, ErrorCode::EnvironmentNoneUsable);
    assert_eq!(none.status, 400);

    // A folder that is named and does not hold the directory is no folder for it.
    let wrong = adopt(&s, &s.ada, "native-b", Some(work), None)
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::RequestInvalid);
    // The detail is the code's sentence; the fault places the folder and says where the session ran.
    let fault = wrong
        .extra
        .get("errors")
        .and_then(|faults| faults.get(0))
        .cloned()
        .unwrap_or_default();
    assert_eq!(fault["path"], "environment_id", "{fault}");
    assert!(
        fault["message"]
            .as_str()
            .is_some_and(|message| message.contains("/elsewhere")),
        "{fault}"
    );
}

#[tokio::test]
async fn what_making_a_thread_needs_is_asked_and_the_harness_tells_nothing_to_those_who_may_not_use_it()
 {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let work = registered(&s, "/work").await;
    // A viewer of the org, who may not drive anything in the topic, and a developer without the use
    // of the provider.
    for (who, name) in [
        (&s.cai, "a viewer"),
        (&s.dan, "a developer without the provider"),
    ] {
        let refused = adopt(&s, who, "native-a", None, None).await.problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied, "{name}");
        assert!(
            !missing_permissions(&refused).is_empty(),
            "{name}: {refused:?}"
        );
    }
    assert_eq!(
        s.rig.commands.issued_of("agent.discover_sessions"),
        0,
        "the machine was not asked for the sessions of somebody who may not use the provider"
    );
    // Somebody who may use the provider and manages no environment of the machine finds none to put it in,
    // and one who names an environment they do not manage is refused for it.
    let none = adopt(&s, &s.eve, "native-a", None, None).await.problem();
    assert_eq!(none.code, ErrorCode::EnvironmentNoneUsable);
    let not_theirs = adopt(&s, &s.eve, "native-a", Some(work), None)
        .await
        .problem();
    assert_eq!(not_theirs.code, ErrorCode::AuthzDenied);
    assert!(missing_permissions(&not_theirs).contains(&"environment.manage".to_owned()));
    let anonymous = s
        .rig
        .world
        .app
        .call(ThreadAdopt, &[&s.tree.topic.base.id])
        .body(&ThreadAdoptBody {
            provider_id: s.on.provider,
            agent_id: s.on.agent,
            native_session_ref: "native-a".to_owned(),
            environment_id: None,
            name: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(anonymous.code, ErrorCode::AuthRequired);
}

#[tokio::test]
async fn a_session_is_taken_in_without_a_name_under_one_of_its_own_or_a_plain_one() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work").await;
    registered(&s, "/elsewhere").await;
    let untitled = adopt(&s, &s.ada, "native-b", None, None).await.ok();
    assert_eq!(untitled.base.name, "Adopted session");
    assert_eq!(untitled.base.title_source, FieldSource::Auto);
}

#[tokio::test]
async fn two_requests_that_take_in_one_session_at_once_make_one_thread_and_refuse_the_other() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    registered(&s, "/work").await;
    let (first, second) = tokio::join!(
        adopt(&s, &s.ada, "native-a", None, None),
        adopt(&s, &s.ada, "native-a", None, None)
    );
    let (made, refused): (Vec<_>, Vec<_>) = [first.body, second.body]
        .into_iter()
        .partition(Result::is_ok);
    assert_eq!((made.len(), refused.len()), (1, 1), "one of them wins");
    let thread = made.into_iter().next().unwrap().unwrap();
    let problem = refused.into_iter().next().unwrap().unwrap_err();
    assert_eq!(problem.code, ErrorCode::ConflictExists);
    assert_eq!(problem.extra["existing_id"], json!(thread.base.id));
    let holding: i64 =
        sqlx::query_scalar("SELECT count(*) FROM segments WHERE native_session_ref = 'native-a'")
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(holding, 1, "the session is held by one segment");
}

#[tokio::test]
async fn adoption_does_not_reveal_an_environment_the_caller_cannot_use() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let work = registered(&s, "/work").await;

    let allowed = plan(&s, "native-a", Some(work)).await.ok();
    assert_eq!(allowed.environment.as_ref().map(|env| env.id), Some(work));
    assert_eq!(allowed.directory.as_deref(), Some("web"));

    for native in ["native-a", "native-b"] {
        let refused = s
            .rig
            .world
            .app
            .call(ThreadAdoptPreview, &[&s.tree.topic.base.id])
            .token(&s.eve.token)
            .body(&ThreadAdoptBody {
                provider_id: s.on.provider,
                agent_id: s.on.agent,
                native_session_ref: native.to_owned(),
                environment_id: Some(work),
                name: None,
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied, "{native}");
        assert!(
            !serde_json::to_string(&refused).unwrap().contains("/work"),
            "the other person's workspace root is not disclosed"
        );
    }
    let refused = adopt(&s, &s.eve, "native-b", Some(work), None)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(!serde_json::to_string(&refused).unwrap().contains("/work"));

    s.rig
        .lets_use("environment", work, "environment.use", &s.eve, json!({}))
        .await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(s.eve.id()),
        orch_types::Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    let manage = orch_authz::resources::check(
        &mut conn,
        &principal,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Environment,
            id: work,
        },
        orch_access_contract::permissions::Permission::EnvironmentManage,
        orch_authz::Policy::default(),
    )
    .await
    .unwrap();
    assert!(
        !manage.is_allowed(),
        "use alone must not require management"
    );
    drop(conn);
    let allowed = s
        .rig
        .world
        .app
        .call(ThreadAdoptPreview, &[&s.tree.topic.base.id])
        .token(&s.eve.token)
        .body(&ThreadAdoptBody {
            provider_id: s.on.provider,
            agent_id: s.on.agent,
            native_session_ref: "native-a".to_owned(),
            environment_id: Some(work),
            name: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(allowed.environment.as_ref().map(|env| env.id), Some(work));
}

#[tokio::test]
async fn equal_native_names_in_other_harnesses_do_not_name_or_block_their_threads() {
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};

    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let work = registered(&s, "/work").await;
    let org = places::make_org(&s.rig.world, &s.bea, "Other", "other").await;
    let project = places::make_project(&s.rig.world, &s.bea, org.base.id, "Other").await;
    let topic = places::make_topic(&s.rig.world, &s.bea, project.base.id, "Other").await;
    let tree = rig::Tree {
        org,
        project,
        topic,
    };
    let other = s.rig.resources(&s.bea, &tree).await;
    let thread = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&tree.topic.base.id])
        .token(&s.bea.token)
        .body(&CreateThread {
            name: Some("Private work".into()),
            binding: Some(CreateThreadBinding {
                agent_id: other.agent,
                provider_id: other.provider,
                arrangement: None,
            }),
            model: Some(MODEL.into()),
            environment: Some(EnvironmentChoice::Use(other.environment)),
            directory: Some("work".into()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok();
    sqlx::query("UPDATE segments SET native_session_ref='native-a' WHERE thread_id=$1")
        .bind(thread.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();

    let listed = s
        .rig
        .world
        .app
        .call(ProviderHarnessSessionList, &[&s.on.provider, &s.on.agent])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    let session = listed
        .iter()
        .find(|session| session.native_session_ref == "native-a")
        .unwrap();
    assert_eq!(session.adopted_thread_id, None);
    let preview = plan(&s, "native-a", Some(work)).await.ok();
    assert_eq!(preview.adopted_thread_id, None);
    assert_eq!(preview.session.adopted_thread_id, None);
    let adopted = adopt(&s, &s.ada, "native-a", Some(work), None).await.ok();
    assert_ne!(adopted.base.id, thread.base.id);
    let after = plan(&s, "native-a", Some(work)).await.ok();
    assert_eq!(after.adopted_thread_id, Some(adopted.base.id));
}

#[tokio::test]
async fn a_shared_harness_does_not_disclose_a_private_adopted_thread() {
    let s = Scene::new().await;
    the_harness_lists_its_sessions(&s).await;
    let work = registered(&s, "/work").await;
    let thread = s.thread().await;
    sqlx::query("UPDATE segments SET native_session_ref='native-a' WHERE thread_id=$1")
        .bind(thread.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let mut conn = s.rig.db.acquire().await.unwrap();
    let page = orch_db::queries::access::PageKey::new("node", thread.base.id, "live");
    for permission in ["thread.view", "thread.interact", "thread.manage"] {
        orch_db::queries::access::testing::set_row(
            &mut conn, &page, "any", permission, false, false,
        )
        .await
        .unwrap();
    }
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(s.eve.id()),
        orch_types::Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    let decision = orch_authz::nodes::check(
        &mut conn,
        &principal,
        thread.base.id,
        orch_access_contract::permissions::Permission::ThreadView,
        orch_authz::Act::Use,
        orch_authz::Policy::default(),
    )
    .await
    .unwrap();
    assert!(
        !decision.is_allowed(),
        "the thread is private to the other caller"
    );
    drop(conn);
    s.rig
        .lets_use("environment", work, "environment.use", &s.eve, json!({}))
        .await;

    let listed = s
        .rig
        .world
        .app
        .call(ProviderHarnessSessionList, &[&s.on.provider, &s.on.agent])
        .token(&s.eve.token)
        .send()
        .await
        .ok();
    assert_eq!(
        listed
            .iter()
            .find(|session| session.native_session_ref == "native-a")
            .unwrap()
            .adopted_thread_id,
        None
    );
    let preview = s
        .rig
        .world
        .app
        .call(ThreadAdoptPreview, &[&s.tree.topic.base.id])
        .token(&s.eve.token)
        .body(&ThreadAdoptBody {
            provider_id: s.on.provider,
            agent_id: s.on.agent,
            native_session_ref: "native-a".into(),
            environment_id: Some(work),
            name: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(preview.adopted_thread_id, None);
    assert_eq!(preview.session.adopted_thread_id, None);
    let duplicate = adopt(&s, &s.eve, "native-a", Some(work), None)
        .await
        .problem();
    assert_eq!(duplicate.code, ErrorCode::ConflictExists);
    assert!(!duplicate.extra.contains_key("existing_id"));
    assert_eq!(
        plan(&s, "native-a", Some(work))
            .await
            .ok()
            .adopted_thread_id,
        Some(thread.base.id),
        "the thread's owner still sees the existing thread"
    );
}
