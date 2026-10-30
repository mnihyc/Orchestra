//! Importing a conversation that is larger than one page: a thread of a thousand items and a chat of twelve hundred messages go through a package, are read a page at a time from the copy of their record, come back in the order they were written with what they were made under and what they answered, are found again by a later import (which adds only what is new), and are taken back a thread and a chat at a time. And the plan of a package that holds many documents asks the database no oftener than that of one that holds a few.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod archive_world;
mod places;
mod rig;
mod support;

use archive_world::{Office, chat_of, everything, kept};
use orch_api_contract::imports::{
    Import, ImportGet, ImportOptions, ImportOutcome, ImportPlan, ImportPreflightRequest,
    ImportReportList, ImportReportListQuery, ImportReportRow, ImportRequest, ImportSource,
    ImportSourceFile, ImportState, ImportUndo, ImportUndoBody, ImportView, NodeImportCreate,
    NodeImportPreflight,
};
use orch_api_contract::nodes::JobState;
use orch_api_contract::package::PackageKind;
use orch_types::Id;
use places::Who;
use support::Statements;

/// How many items are added to the thread at its source: more than four pages of the importer's.
const ITEMS: i64 = 1_000;
/// How many messages are added to the chat at its source: more than two groups.
const MESSAGES: i64 = 1_200;

/// The zip an export of `place` as an Orch package writes, as Ada fetched it.
async fn package_of(o: &Office, place: Id) -> Vec<u8> {
    let mut request = everything();
    request.package = Some(true);
    let export = o.made(&o.s.ada, place, &request).await;
    let fetched = o.fetch(&o.s.ada, export.id).await;
    assert_eq!(fetched.status, 200);
    fetched.raw.to_vec()
}

async fn upload(o: &Office, by: &Who, name: &str, bytes: &[u8]) -> ImportSourceFile {
    let (hash, _) = kept(&o.s, bytes).await;
    let mut conn = o.s.rig.db.acquire().await.unwrap();
    orch_db::queries::uploads::remember(&mut conn, by.id(), &hash, o.s.rig.world.now())
        .await
        .unwrap();
    ImportSourceFile {
        name: name.to_owned(),
        blob: hash,
    }
}

fn asked(client: &str, file: &ImportSourceFile) -> ImportRequest {
    ImportRequest {
        client_id: client.to_owned(),
        source: ImportSource {
            files: vec![file.clone()],
            mode: None,
        },
        destination: Default::default(),
        options: ImportOptions::default(),
        choices: Default::default(),
    }
}

/// Asks for the import, waits for its job and reads the import as it ended.
async fn imported(o: &Office, by: &Who, place: Id, request: &ImportRequest) -> Import {
    let asked =
        o.s.rig
            .world
            .app
            .call(NodeImportCreate, &[&place])
            .token(&by.token)
            .body(request)
            .send()
            .await;
    assert_eq!(asked.status, 202, "{:?}", asked.body);
    let job = o.ended(by, asked.ok().id).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
    let id: Id = job.result.as_ref().unwrap()["import_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    o.s.rig
        .world
        .app
        .call(ImportGet, &[&id])
        .token(&by.token)
        .send()
        .await
        .ok()
}

/// What the dry run answers for `request` into `place`, for the part of the plan `view` asks for.
async fn planned(
    o: &Office,
    by: &Who,
    place: Id,
    request: &ImportRequest,
    view: ImportView,
) -> ImportPlan {
    let reply =
        o.s.rig
            .world
            .app
            .call(NodeImportPreflight, &[&place])
            .token(&by.token)
            .body(&ImportPreflightRequest {
                request: request.clone(),
                view,
            })
            .send()
            .await;
    assert_eq!(reply.status, 200, "{:?}", reply.body);
    reply.ok()
}

async fn report(o: &Office, by: &Who, import: Id) -> Vec<ImportReportRow> {
    o.s.rig
        .world
        .app
        .call(ImportReportList, &[&import])
        .token(&by.token)
        .query(&ImportReportListQuery {
            limit: Some(200),
            ..Default::default()
        })
        .send()
        .await
        .ok()
        .items
}

fn made_of(rows: &[ImportReportRow], kind: PackageKind) -> Id {
    rows.iter()
        .find(|row| row.kind == kind)
        .unwrap_or_else(|| panic!("a row for the {kind:?}: {rows:#?}"))
        .made
        .as_ref()
        .expect("it was made")
        .id
}

async fn undone(o: &Office, by: &Who, import: Id) {
    let job =
        o.s.rig
            .world
            .app
            .call(ImportUndo, &[&import])
            .token(&by.token)
            .body(&ImportUndoBody::default())
            .send()
            .await;
    let job = o.ended(by, job.ok().id).await;
    assert_eq!(job.state, JobState::Succeeded, "{:?}", job.error);
}

/// Adds `count` items to the thread `o.thread` as if the conversation had gone on: copies of its first agent message with texts of their own,
/// in the thread's sequence, and some of them made under the one before (a sub-agent's work under its call).
async fn grow_thread(o: &Office, count: i64, label: &str) {
    let thread = o.thread.base.id;
    let mut conn = o.s.rig.db.acquire().await.unwrap();
    let first: i64 = sqlx::query_scalar(
        "UPDATE threads SET next_seq = next_seq + $2 WHERE node_id = $1 RETURNING next_seq - $2",
    )
    .bind(thread)
    .bind(count)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "WITH g AS (SELECT n, gen_random_uuid() AS id FROM generate_series(1, $3::int) AS n),
              template AS (SELECT * FROM items WHERE thread_id = $1 AND kind = 'agent_message' ORDER BY seq_created LIMIT 1)
         INSERT INTO items (id, thread_id, segment_id, continuation_id, turn_id, parent_item_id, stream, kind, status, payload, seq_created, seq_updated)
         SELECT g.id, t.thread_id, t.segment_id, t.continuation_id, t.turn_id,
                CASE WHEN g.n % 10 = 0 THEN prior.id ELSE NULL END,
                t.stream, t.kind, 'completed',
                jsonb_set(t.payload, '{text}', to_jsonb($4::text || ' ' || g.n)),
                $2 + g.n, $2 + g.n
           FROM g CROSS JOIN template t LEFT JOIN g prior ON prior.n = g.n - 1",
    )
    .bind(thread)
    .bind(first)
    .bind(i32::try_from(count).unwrap())
    .bind(label)
    .execute(&mut *conn)
    .await
    .unwrap();
}

/// Adds `count` messages to the chat `chat` by Ada, every tenth an answer to the one five before it, in the chat's sequence.
async fn grow_chat(o: &Office, chat: Id, count: i64, label: &str) {
    let mut conn = o.s.rig.db.acquire().await.unwrap();
    let first: i64 = sqlx::query_scalar(
        "UPDATE chats SET next_seq = next_seq + $2 WHERE id = $1 RETURNING next_seq - $2",
    )
    .bind(chat)
    .bind(count)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "WITH g AS (SELECT n, gen_random_uuid() AS id FROM generate_series(1, $4::int) AS n)
         INSERT INTO chat_messages (id, chat_id, kind, author_id, content, search_text, reply_to_id, seq_created, seq_updated, created_at)
         SELECT g.id, $1, 'message', $2, jsonb_build_array(jsonb_build_object('type', 'text', 'text', $5::text || ' ' || g.n)),
                $5::text || ' ' || g.n, earlier.id, $3 + g.n, $3 + g.n, now() - make_interval(secs => ($4::int - g.n))
           FROM g LEFT JOIN g earlier ON g.n % 10 = 0 AND earlier.n = g.n - 5",
    )
    .bind(chat)
    .bind(o.s.ada.id())
    .bind(first)
    .bind(i32::try_from(count).unwrap())
    .bind(label)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE chats SET last_message_seq = GREATEST(last_message_seq, $2), last_message_at = now() WHERE id = $1",
    )
    .bind(chat)
    .bind(first + count)
    .execute(&mut *conn)
    .await
    .unwrap();
}

async fn items_of(o: &Office, thread: Id) -> Vec<String> {
    let mut conn = o.s.rig.db.acquire().await.unwrap();
    sqlx::query_scalar(
        "SELECT COALESCE(payload->>'text', kind) FROM items WHERE thread_id = $1 AND kind NOT IN ('marker', 'approval') ORDER BY seq_created, id",
    )
    .bind(thread)
    .fetch_all(&mut *conn)
    .await
    .unwrap()
}

async fn messages_of(o: &Office, chat: Id) -> Vec<String> {
    let mut conn = o.s.rig.db.acquire().await.unwrap();
    sqlx::query_scalar(
        "SELECT COALESCE(search_text, '') FROM chat_messages WHERE chat_id = $1 AND deleted_at IS NULL AND kind = 'message' ORDER BY seq_created, id",
    )
    .bind(chat)
    .fetch_all(&mut *conn)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_thread_of_a_thousand_items_goes_through_a_package_in_the_order_it_was_written_and_a_later_import_adds_only_what_is_new_and_takes_back_only_that()
 {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let project = o.s.tree.project.base.id;
    grow_thread(&o, ITEMS, "line").await;
    let at_source = items_of(&o, o.thread.base.id).await;
    assert!(i64::try_from(at_source.len()).unwrap() > ITEMS);

    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;
    let first = imported(&o, ada, project, &asked("history-one", &file)).await;
    assert_eq!(first.state, ImportState::Done, "{:?}", first.failure);
    let rows = report(&o, ada, first.id).await;
    let thread_row = rows
        .iter()
        .find(|row| row.kind == PackageKind::Thread)
        .expect("the thread");
    assert_eq!(
        thread_row.outcome,
        ImportOutcome::Created,
        "{:?}",
        thread_row.message
    );
    let made = made_of(&rows, PackageKind::Thread);
    // Every item came, in the order the source wrote them, under the one it was made under.
    assert_eq!(items_of(&o, made).await, at_source);
    let hung: i64 = {
        let mut conn = o.s.rig.db.acquire().await.unwrap();
        sqlx::query_scalar(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND parent_item_id IS NOT NULL",
        )
        .bind(made)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
    };
    assert!(
        hung >= ITEMS / 10 - 1,
        "the items made under another kept it: {hung}"
    );
    let carried = thread_row.made.as_ref().unwrap();
    assert_eq!(carried.kind, PackageKind::Thread);

    // The same package again finds the thread by its source and adds nothing.
    let again = imported(&o, ada, project, &asked("history-two", &file)).await;
    assert_eq!(again.state, ImportState::Done, "{:?}", again.failure);
    assert_eq!(
        items_of(&o, made).await,
        at_source,
        "nothing was added twice"
    );

    // The conversation goes on at its source, a package of it is imported again, and only the new items are added to the thread that is there.
    grow_thread(&o, 150, "later").await;
    let newer = upload(&o, ada, "topic-later.zip", &package_of(&o, o.topic()).await).await;
    let merged = imported(&o, ada, project, &asked("history-three", &newer)).await;
    assert_eq!(merged.state, ImportState::Done, "{:?}", merged.failure);
    let now_at_source = items_of(&o, o.thread.base.id).await;
    assert_eq!(items_of(&o, made).await, now_at_source);

    // Taking that import back takes out the items it added and leaves the thread and what the first import brought.
    undone(&o, ada, merged.id).await;
    assert_eq!(items_of(&o, made).await, at_source);

    // Taking the first back takes the thread itself.
    undone(&o, ada, first.id).await;
    let gone: bool = {
        let mut conn = o.s.rig.db.acquire().await.unwrap();
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM nodes WHERE id = $1")
            .bind(made)
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    };
    assert!(gone, "the thread the first import made is taken back");
}

#[tokio::test]
async fn an_imported_thread_opens_and_syncs_without_an_agent_then_gets_its_first_local_setup() {
    use orch_api_contract::binding::{
        EnvironmentChoice, ThreadBindingSet, ThreadBindingSetBody, ThreadEnvironmentSet,
        ThreadEnvironmentSetBody,
    };
    use orch_api_contract::threads::{
        ThreadOpeningGet, ThreadOpeningGetQuery, ThreadState, ThreadSyncGet, ThreadSyncGetQuery,
    };
    let o = Office::new().await;
    let ada = &o.s.ada;
    let file = upload(
        &o,
        ada,
        "conversation.zip",
        &package_of(&o, o.topic()).await,
    )
    .await;
    let done = imported(
        &o,
        ada,
        o.s.tree.project.base.id,
        &asked("read-then-set-up", &file),
    )
    .await;
    assert_eq!(done.state, ImportState::Done, "{:?}", done.failure);
    let thread = made_of(&report(&o, ada, done.id).await, PackageKind::Thread);
    let history = items_of(&o, thread).await;
    assert!(!history.is_empty());
    let opening =
        o.s.rig
            .world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&ada.token)
            .query(&ThreadOpeningGetQuery::default())
            .send()
            .await;
    assert_eq!(opening.status, 200, "{:?}", opening.body);
    let opening = opening.ok();
    assert!(opening.sync.thread.binding.is_none());
    assert!(opening.sync.thread.environment.is_none());
    assert!(!opening.sync.items.is_empty());
    assert!(
        opening
            .sync
            .items
            .iter()
            .filter(|item| !matches!(
                item.payload,
                orch_api_contract::threads::ItemPayload::Marker(_)
            ))
            .all(|item| item.before_orch)
    );
    assert!(!opening.sync.segments.is_empty());
    let imported_segments: Vec<Id> = opening
        .sync
        .segments
        .iter()
        .map(|segment| {
            assert!(segment.agent.is_none());
            assert!(segment.provider.is_none());
            assert!(segment.environment.is_none());
            assert!(segment.ended_at.is_some());
            segment.id
        })
        .collect();
    let sync =
        o.s.rig
            .world
            .app
            .call(ThreadSyncGet, &[&thread])
            .token(&ada.token)
            .query(&ThreadSyncGetQuery::default())
            .send()
            .await;
    assert_eq!(sync.status, 200, "{:?}", sync.body);
    assert_eq!(sync.ok().segments, opening.sync.segments);
    let unbound: i64 = {
        let mut conn = o.s.rig.db.acquire().await.unwrap();
        sqlx::query_scalar("SELECT count(*) FROM segments WHERE thread_id=$1 AND agent_id IS NULL AND provider_id IS NULL AND environment_id IS NULL")
            .bind(thread).fetch_one(&mut *conn).await.unwrap()
    };
    assert_eq!(unbound, i64::try_from(imported_segments.len()).unwrap());
    let placed =
        o.s.rig
            .world
            .app
            .call(ThreadEnvironmentSet, &[&thread])
            .token(&ada.token)
            .body(&ThreadEnvironmentSetBody {
                choice: EnvironmentChoice::Use(o.s.on.environment),
                directory: None,
            })
            .send()
            .await;
    assert_eq!(placed.status, 200, "{:?}", placed.body);
    let bound =
        o.s.rig
            .world
            .app
            .call(ThreadBindingSet, &[&thread])
            .token(&ada.token)
            .body(&ThreadBindingSetBody {
                agent_id: o.s.on.agent,
                provider_id: o.s.on.provider,
                arrangement: None,
                model: Some(rig::MODEL.to_owned()),
                effort: None,
            })
            .send()
            .await;
    assert_eq!(bound.status, 200, "{:?}", bound.body);
    let bound = bound.ok();
    assert_eq!(bound.binding.as_ref().unwrap().agent.id, o.s.on.agent);
    assert_eq!(bound.environment.as_ref().unwrap().id, o.s.on.environment);
    assert_eq!(bound.state, ThreadState::Idle);
    let opening =
        o.s.rig
            .world
            .app
            .call(ThreadOpeningGet, &[&thread])
            .token(&ada.token)
            .query(&ThreadOpeningGetQuery::default())
            .send()
            .await
            .ok();
    assert_eq!(opening.sync.thread.base.id, thread);
    for id in imported_segments {
        let segment = opening
            .sync
            .segments
            .iter()
            .find(|segment| segment.id == id)
            .unwrap();
        assert!(segment.agent.is_none());
        assert!(segment.provider.is_none());
        assert!(segment.environment.is_none());
    }
    assert!(opening.sync.segments.iter().any(|segment| {
        segment
            .agent
            .as_ref()
            .is_some_and(|agent| agent.id == o.s.on.agent)
    }));
    assert_eq!(
        items_of(&o, thread).await,
        history,
        "setup preserves every imported message"
    );
    let sync =
        o.s.rig
            .world
            .app
            .call(ThreadSyncGet, &[&thread])
            .token(&ada.token)
            .query(&ThreadSyncGetQuery::default())
            .send()
            .await;
    assert_eq!(sync.status, 200, "{:?}", sync.body);
    assert!(sync.ok().thread.binding.is_some());
}

#[tokio::test]
async fn a_chat_of_twelve_hundred_messages_goes_through_a_package_with_its_replies_and_a_later_import_adds_only_the_new_ones_and_takes_back_only_those()
 {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let project = o.s.tree.project.base.id;
    grow_chat(&o, o.topic_chat, MESSAGES, "message").await;
    let at_source = messages_of(&o, o.topic_chat).await;
    assert!(i64::try_from(at_source.len()).unwrap() > MESSAGES);

    let file = upload(&o, ada, "topic.zip", &package_of(&o, o.topic()).await).await;
    let first = imported(&o, ada, project, &asked("chat-one", &file)).await;
    assert_eq!(first.state, ImportState::Done, "{:?}", first.failure);
    let rows = report(&o, ada, first.id).await;
    let topic = made_of(&rows, PackageKind::Topic);
    let chat = chat_of(&o.s, topic).await;
    assert_eq!(
        messages_of(&o, chat).await,
        at_source,
        "every message came in order"
    );
    let answers: i64 = {
        let mut conn = o.s.rig.db.acquire().await.unwrap();
        sqlx::query_scalar(
            "SELECT count(*) FROM chat_messages WHERE chat_id = $1 AND reply_to_id IS NOT NULL",
        )
        .bind(chat)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
    };
    assert_eq!(
        answers,
        MESSAGES / 10,
        "each answer found the message it answered"
    );

    // The same package again adds nothing; a package of the chat grown since adds only the new messages to the chat that is there.
    let again = imported(&o, ada, project, &asked("chat-two", &file)).await;
    assert_eq!(again.state, ImportState::Done, "{:?}", again.failure);
    assert_eq!(
        messages_of(&o, chat).await,
        at_source,
        "nothing was added twice"
    );
    grow_chat(&o, o.topic_chat, 90, "newer").await;
    let newer = upload(&o, ada, "topic-newer.zip", &package_of(&o, o.topic()).await).await;
    let merged = imported(&o, ada, project, &asked("chat-three", &newer)).await;
    assert_eq!(merged.state, ImportState::Done, "{:?}", merged.failure);
    let now_at_source = messages_of(&o, o.topic_chat).await;
    assert_eq!(messages_of(&o, chat).await, now_at_source);

    // Taking that import back deletes the messages it added, ninety, and no other.
    undone(&o, ada, merged.id).await;
    assert_eq!(messages_of(&o, chat).await, at_source);
}

#[tokio::test]
async fn the_plan_of_a_package_asks_the_database_as_often_for_sixty_documents_as_for_ten_and_a_page_of_it_as_often_wherever_it_lies()
 {
    let o = Office::new().await;
    let ada = &o.s.ada;
    let project = o.s.tree.project.base.id;
    for n in 0..9 {
        Office::write_document(&o.s, ada, &format!("Note {n}"), "Some words.\n").await;
    }
    let few = upload(&o, ada, "few.zip", &package_of(&o, o.topic()).await).await;
    let request = asked("plan-few", &few);
    // The first plan of a request reads what is kept for the next (settings, the shape of the place); the one that is counted is the second.
    planned(&o, ada, project, &request, ImportView::default()).await;
    let before = Statements::now();
    let small = planned(&o, ada, project, &request, ImportView::default()).await;
    let small_statements = Statements::now() - before;
    assert!(
        small_statements > 0,
        "the statements of the plan are counted on this thread"
    );

    for n in 9..59 {
        Office::write_document(&o.s, ada, &format!("Note {n}"), "Some words.\n").await;
    }
    let many = upload(&o, ada, "many.zip", &package_of(&o, o.topic()).await).await;
    let request = asked("plan-many", &many);
    planned(&o, ada, project, &request, ImportView::default()).await;
    let before = Statements::now();
    let large = planned(&o, ada, project, &request, ImportView::default()).await;
    let large_statements = Statements::now() - before;
    assert!(
        large.counts.documents >= small.counts.documents + 50,
        "the second package holds fifty documents more: {} against {}",
        large.counts.documents,
        small.counts.documents
    );
    assert_eq!(
        small_statements, large_statements,
        "a plan asks for what it needs of the database by the kinds and the places of its objects, never for each object"
    );

    // A page of the plan costs the same whichever page it is: the plan is read from memory, a page of it is cut from the list.
    let first_page = Statements::now();
    let first = planned(
        &o,
        ada,
        project,
        &request,
        ImportView {
            limit: Some(10),
            ..ImportView::default()
        },
    )
    .await;
    let first_page = Statements::now() - first_page;
    assert_eq!(first.objects.items.len(), 10);
    let cursor = first.objects.next_cursor.clone().expect("more than a page");
    let later_page = Statements::now();
    let later = planned(
        &o,
        ada,
        project,
        &request,
        ImportView {
            limit: Some(10),
            cursor: Some(cursor),
            ..ImportView::default()
        },
    )
    .await;
    let later_page = Statements::now() - later_page;
    assert_eq!(later.objects.items.len(), 10);
    assert_eq!(
        first_page, later_page,
        "a page far from the first is no dearer than the first"
    );
}
