//! Native resource source/cursor/privacy cases. No model or summary worker is dispatched.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]
mod common;
mod shared;

use common::World;
use orch_access_contract::{
    key_scope::{KeyAction, KeyScope, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    operations::{FacetSpec, OperationSpec, TargetResolver},
    routes::Endpoint,
    summaries::{SummaryChatRange, SummaryTarget},
};
use orch_authz::{Caller, Principal, TokenGrant};
use orch_db::queries::{access, chats, documents};
use orch_engine::{
    operation_scope::Prepared,
    summaries::sources::{self, ReadScope, SourceError},
};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use shared::Places;

fn budget(value: u32) -> sources::Budget {
    sources::Budget::new(value, 0).unwrap()
}

#[tokio::test]
async fn native_chat_refreshes_keep_surviving_cuts_and_report_included_shortened_and_omitted_items()
{
    let world = World::new().await;
    world.engine().stop();
    let chat = shared::chat_of(&world, world.topic).await;
    let mut through = 0;
    for index in 0..3 {
        through = post(
            &world,
            chat,
            json!([{"type":"text","text":format!("source {index} {}","字".repeat(300))}]),
        )
        .await
        .1;
    }
    let target = SummaryTarget::Chat { id: chat };
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = owner(&world, &mut conn).await;
    let first = sources::materialize(
        &mut conn,
        &principal,
        target,
        None,
        Some(SummaryChatRange { after: 0, through }),
        budget(512),
    )
    .await
    .unwrap();
    assert_eq!(first.snapshot["context"]["facts"]["included_items"], 3);
    assert_eq!(first.snapshot["context"]["facts"]["shortened_items"], 3);
    assert_eq!(first.snapshot["context"]["facts"]["omitted_items"], 0);
    let cache = orch_db::queries::summaries::ensure(&mut conn, target, None, world.clock.now())
        .await
        .unwrap()
        .unwrap();
    // Persist the ACTUAL native source plan only; this fixture creates no model reply or usage.
    sqlx::query("UPDATE resource_summaries SET source_snapshot=$2 WHERE id=$1")
        .bind(cache.id)
        .bind(&first.snapshot)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    through = post(
        &world,
        chat,
        json!([{"type":"text","text":"New small item."}]),
    )
    .await
    .1;
    let mut conn = world.pool.acquire().await.unwrap();
    let second = sources::materialize(
        &mut conn,
        &principal,
        target,
        None,
        Some(SummaryChatRange { after: 0, through }),
        budget(512),
    )
    .await
    .unwrap();
    orch_db::queries::summaries::ensure(&mut conn, target, Some(world.ada), world.clock.now())
        .await
        .unwrap()
        .unwrap();
    let with_personal_row = sources::materialize(
        &mut conn,
        &principal,
        target,
        None,
        Some(SummaryChatRange { after: 0, through }),
        budget(512),
    )
    .await
    .unwrap();
    assert_eq!(
        second.snapshot, with_personal_row.snapshot,
        "an empty personal row cannot replace the shared cut plan"
    );
    assert_eq!(second.snapshot["context"]["facts"]["included_items"], 3);
    assert_eq!(second.snapshot["context"]["facts"]["shortened_items"], 2);
    assert_eq!(second.snapshot["context"]["facts"]["omitted_items"], 1);
    assert_eq!(&second.activity[..2], &first.activity[1..]);
    sqlx::query("UPDATE resource_summaries SET source_snapshot=$2 WHERE id=$1")
        .bind(cache.id)
        .bind(&second.snapshot)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    through = post(
        &world,
        chat,
        json!([{"type":"text","text":"Next small item."}]),
    )
    .await
    .1;
    let mut conn = world.pool.acquire().await.unwrap();
    let third = sources::materialize(
        &mut conn,
        &principal,
        target,
        None,
        Some(SummaryChatRange { after: 0, through }),
        budget(512),
    )
    .await
    .unwrap();
    let frozen = sources::context_plan(&third.snapshot).unwrap();
    sqlx::query("UPDATE resource_summaries SET source_snapshot=NULL WHERE id=$1")
        .bind(cache.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    let repeated = sources::materialize_with_plan(
        &mut conn,
        &principal,
        target,
        None,
        Some(SummaryChatRange { after: 0, through }),
        budget(512),
        frozen,
    )
    .await
    .unwrap();
    assert_eq!(
        third.snapshot, repeated.snapshot,
        "accepted cut positions survive a changed cache seed while words are reread"
    );
    assert_eq!(
        &third.activity[..second.activity.len()],
        second.activity.as_slice()
    );
    assert_eq!(third.snapshot["context"]["facts"]["included_items"], 4);
    assert_eq!(third.snapshot["context"]["facts"]["omitted_items"], 1);
    let calls: i64 = sqlx::query_scalar("SELECT count(*) FROM summary_calls")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(calls, 0, "source selection does not call a model");
}

async fn owner(world: &World, conn: &mut orch_db::Conn) -> Principal {
    Principal::load(
        conn,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap()
}

async fn private(world: &World, kind: &str, id: Id, permission: &str) {
    let mut conn = world.pool.acquire().await.unwrap();
    access::write_row(
        &mut conn,
        &access::RowWrite {
            page: &access::PageKey::new(kind, id, "live"),
            profile: "any",
            permission,
            inherit: false,
            locked: false,
        },
        Some(world.ada),
        world.clock.now(),
    )
    .await
    .unwrap();
}

async fn post(world: &World, chat: Id, content: Json) -> (Id, i64) {
    let mut tx = world.pool.begin().await.unwrap();
    chats::lock(&mut tx, chat).await.unwrap().unwrap();
    let seq = chats::next_seq(&mut tx, chat).await.unwrap().unwrap();
    let id = new_id();
    chats::insert_message(
        &mut tx,
        &chats::NewMessage {
            id,
            chat_id: chat,
            kind: chats::MESSAGE,
            author_id: Some(world.ada),
            author_thread_id: None,
            author_agent: None,
            content: &content,
            reply_to_id: None,
            mentions: &[],
            mentions_all: false,
            client_id: None,
            search_text: None,
            seq,
            at: world.clock.now(),
        },
    )
    .await
    .unwrap();
    chats::note_message(&mut tx, chat, seq, world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (id, seq)
}

#[tokio::test]
async fn two_real_readers_of_one_group_keep_separate_catch_up_ranges_and_unread_marks() {
    let world = World::new().await;
    world.engine().stop();
    let bob = world.person("catchup-reader").await;
    let chat = new_id();
    let mut tx = world.pool.begin().await.unwrap();
    chats::insert_chat(
        &mut tx,
        &chats::NewChat {
            id: chat,
            kind: chats::KIND_GROUP,
            org_id: world.root,
            direct_key: None,
            name: Some("Shared group, personal catch-ups"),
            created_by: world.ada,
            at: world.clock.now(),
        },
    )
    .await
    .unwrap()
    .unwrap();
    for person in [world.ada, bob] {
        let seq = chats::next_seq(&mut tx, chat).await.unwrap().unwrap();
        chats::add_member(
            &mut tx,
            &chats::NewMember {
                chat_id: chat,
                user_id: person,
                added_by: Some(world.ada),
                guest: false,
                seq,
                at: world.clock.now(),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let (_, first) = post(
        &world,
        chat,
        json!([{"type":"text","text":"First message"}]),
    )
    .await;
    let (_, second) = post(
        &world,
        chat,
        json!([{"type":"text","text":"Second message"}]),
    )
    .await;
    let (_, third) = post(
        &world,
        chat,
        json!([{"type":"text","text":"Third message"}]),
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    chats::mark_read(&mut conn, world.ada, chat, first)
        .await
        .unwrap();
    chats::mark_read(&mut conn, bob, chat, second)
        .await
        .unwrap();
    let ada = owner(&world, &mut conn).await;
    let bob_principal = Principal::load(
        &mut conn,
        Caller::session(bob),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let ada_range = sources::catch_up_range(&mut conn, &ada, chat)
        .await
        .unwrap();
    let bob_range = sources::catch_up_range(&mut conn, &bob_principal, chat)
        .await
        .unwrap();
    assert_eq!(
        ada_range,
        SummaryChatRange {
            after: first,
            through: third
        }
    );
    assert_eq!(
        bob_range,
        SummaryChatRange {
            after: second,
            through: third
        }
    );
    let target = SummaryTarget::Chat { id: chat };
    let a = sources::materialize(
        &mut conn,
        &ada,
        target,
        None,
        Some(ada_range),
        budget(32_000),
    )
    .await
    .unwrap();
    let b = sources::materialize(
        &mut conn,
        &bob_principal,
        target,
        None,
        Some(bob_range),
        budget(32_000),
    )
    .await
    .unwrap();
    assert_eq!(
        a.activity
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>(),
        ["Second message", "Third message"]
    );
    assert_eq!(
        a.snapshot["context"]["facts"],
        json!({"included_items":2,"shortened_items":0,"omitted_items":0})
    );
    assert_eq!(
        b.activity
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>(),
        ["Third message"]
    );
    assert_ne!(a.snapshot, b.snapshot);
    assert_eq!(a.identity.fact().object.id, Some(chat));
    assert_eq!(b.identity.fact().object.id, Some(chat));
    assert_eq!(
        chats::state(&mut conn, world.ada, chat)
            .await
            .unwrap()
            .unwrap()
            .last_read_seq,
        first
    );
    assert_eq!(
        chats::state(&mut conn, bob, chat)
            .await
            .unwrap()
            .unwrap()
            .last_read_seq,
        second
    );
    let model_inputs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM model_inputs WHERE purpose='summary'")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(model_inputs, 0, "source preparation is not model dispatch");
}

#[tokio::test]
async fn a_document_keeps_its_real_family_and_ancestry_and_reads_only_its_budget_without_writing() {
    let world = World::new().await;
    world.engine().stop();
    let doc = world
        .document_beside(
            world.topic,
            "Written plan",
            &["First paragraph.", "Second paragraph.", "Third paragraph."],
        )
        .await;
    private(&world, "document", doc, "doc.view").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = owner(&world, &mut conn).await;
    let target = SummaryTarget::Document { id: doc };
    let identity = sources::identify(&mut conn, target).await.unwrap();
    assert_eq!(identity.target(), target);
    assert_eq!(identity.place(), world.topic);
    assert_eq!(
        identity.fact().object.kind,
        KeyTargetKind::Object(ObjectKind::Document)
    );
    assert_eq!(identity.fact().object.id, Some(doc));
    assert!(
        identity
            .fact()
            .node_ancestors
            .iter()
            .any(|parent| parent.id == Some(world.topic))
    );
    let before = documents::get(&mut conn, doc).await.unwrap().unwrap();
    let material = sources::materialize(&mut conn, &principal, target, None, None, budget(14))
        .await
        .unwrap();
    assert_eq!(
        material
            .activity
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>(),
        ["Second paragraph.", "Third paragraph."]
    );
    assert!(!material.complete);
    assert_eq!(material.snapshot["context"]["facts"]["included_items"], 2);
    assert_eq!(material.snapshot["context"]["facts"]["omitted_items"], 1);
    assert_eq!(material.source_refs, [target]);
    assert!(material.inputs.is_empty());
    let after = documents::get(&mut conn, doc).await.unwrap().unwrap();
    assert_eq!(before, after);
    let work:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM resource_summary_requests)+(SELECT count(*) FROM model_inputs)")
        .fetch_one(&mut *conn).await.unwrap();
    assert_eq!(work, 0, "preparation creates no work or model input");
    assert!(matches!(
        sources::identify(&mut conn, SummaryTarget::Topic { id: doc }).await,
        Err(SourceError::Unavailable)
    ));
    drop(conn);
    let outsider = world.person("other").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let reader = Principal::load(
        &mut conn,
        Caller::session(outsider),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    assert!(matches!(
        sources::materialize(&mut conn, &reader, target, None, None, budget(32_000)).await,
        Err(SourceError::Unavailable)
    ));
}

#[tokio::test]
async fn a_scoped_reader_uses_the_same_declared_read_facts_and_cannot_omit_its_key_boundary() {
    const DOCUMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Document);
    const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);
    const SPEC: OperationSpec = OperationSpec::facets(&[
        FacetSpec::required(
            KeyAction::Read,
            &[DOCUMENT],
            TargetResolver::Resolved("summary_target"),
        ),
        FacetSpec::required(
            KeyAction::Read,
            &[THREAD],
            TargetResolver::ResolvedCollection("summary_sources"),
        ),
    ]);
    let world = World::new().await;
    world.engine().stop();
    let doc = world
        .document_beside(world.topic, "Plan", &["Exact current document text."])
        .await;
    let target = SummaryTarget::Document { id: doc };
    let mut conn = world.pool.acquire().await.unwrap();
    let identity = sources::identify(&mut conn, target).await.unwrap();
    let mut route = orch_api_contract::documents::DocumentGet::ROUTE;
    route.name = "SummarySourceReadControl";
    route.operation = Some(SPEC);
    let mut plan = Prepared::new(route, route.name).unwrap();
    plan.bind(0, DOCUMENT, std::slice::from_ref(identity.fact()))
        .unwrap();
    plan.bind_empty(1).unwrap();
    let policy: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"document","id":doc,"reach":"self"},
        "allow":[{"operation":route.name,"actions":["read"]}]
    }]}))
    .unwrap();
    let caller = Caller::token(
        world.ada,
        TokenGrant::from_stored_scope(&[], None, Some(policy), None).unwrap(),
    );
    let caller = plan.preparation_caller(&caller).unwrap();
    let principal = Principal::load(&mut conn, caller, Time::new(world.clock.now()))
        .await
        .unwrap();
    assert!(matches!(
        sources::materialize(&mut conn, &principal, target, None, None, budget(32_000)).await,
        Err(SourceError::Unavailable)
    ));
    let scope = ReadScope::new(&plan, 0, 1).unwrap();
    let material = sources::materialize(
        &mut conn,
        &principal,
        target,
        Some(&scope),
        None,
        budget(32_000),
    )
    .await
    .unwrap();
    assert_eq!(material.activity[0].text, "Exact current document text.");
    // A Write slot of a real operation can never be used as this source's Read role.
    const WRITE_SPEC: OperationSpec = OperationSpec::facets(&[FacetSpec::required(
        KeyAction::Write,
        &[DOCUMENT],
        TargetResolver::Resolved("summary_target"),
    )]);
    let mut write_route = route;
    write_route.operation = Some(WRITE_SPEC);
    let write = Prepared::new(write_route, write_route.name).unwrap();
    assert!(ReadScope::new(&write, 0, 0).is_err());
}

#[tokio::test]
async fn a_topic_excludes_private_children_even_for_their_owner_and_filters_before_the_item_budget()
{
    let world = World::new().await;
    world.engine().stop();
    let hidden = world
        .thread_in(world.topic, "Private incident detail")
        .await;
    let visible = world.thread_in(world.topic, "Public work").await;
    private(&world, "node", hidden, "thread.view").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = owner(&world, &mut conn).await;
    assert!(
        orch_authz::nodes::check(
            &mut conn,
            &principal,
            hidden,
            orch_access_contract::permissions::Permission::ThreadView,
            orch_authz::Act::Use,
            orch_authz::Policy::default()
        )
        .await
        .unwrap()
        .is_allowed(),
        "this proves exclusion is about topic audience, not the owner's personal access"
    );
    let material = sources::materialize(
        &mut conn,
        &principal,
        SummaryTarget::Topic { id: world.topic },
        None,
        None,
        budget(32_000),
    )
    .await
    .unwrap();
    assert_eq!(material.activity.len(), 1);
    assert!(material.activity[0].text.contains("Public work"));
    assert!(
        !serde_json::to_string(&material.snapshot)
            .unwrap()
            .contains(&hidden.to_string())
    );
    assert_eq!(
        material.source_refs,
        [
            SummaryTarget::Topic { id: world.topic },
            SummaryTarget::Thread { id: visible }
        ]
    );
    assert_eq!(material.inputs.len(), 1);
    assert_eq!(material.inputs[0].object.id, Some(visible));
    assert!(material.complete);
}

#[tokio::test]
async fn a_chat_keeps_its_pinned_unread_range_across_read_marks_and_new_posts_without_following_links()
 {
    let world = World::new().await;
    world.engine().stop();
    let chat = shared::chat_of(&world, world.topic).await;
    let (_, first) = post(
        &world,
        chat,
        json!([{"type":"text","text":"Already read."}]),
    )
    .await;
    let (_, second) = post(
        &world,
        chat,
        json!([{"type":"text","text":"The change is ready."}]),
    )
    .await;
    let (_,third)=post(&world,chat,json!([
        {"type":"text","text":"Please review it."},
        {"type":"item_ref","thread_id":new_id(),"item_id":new_id(),"text":"A private linked detail must not be dereferenced."}
    ])).await;
    let mut conn = world.pool.acquire().await.unwrap();
    chats::mark_read(&mut conn, world.ada, chat, first)
        .await
        .unwrap();
    let principal = owner(&world, &mut conn).await;
    let range = sources::catch_up_range(&mut conn, &principal, chat)
        .await
        .unwrap();
    assert_eq!(
        range,
        SummaryChatRange {
            after: first,
            through: third
        }
    );
    let target = SummaryTarget::Chat { id: chat };
    let before = sources::materialize(
        &mut conn,
        &principal,
        target,
        None,
        Some(range),
        budget(32_000),
    )
    .await
    .unwrap();
    assert_eq!(
        before
            .activity
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>(),
        ["The change is ready.", "Please review it."]
    );
    assert_eq!(
        chats::state(&mut conn, world.ada, chat)
            .await
            .unwrap()
            .unwrap()
            .last_read_seq,
        first,
        "summary preparation is no read mark"
    );
    chats::mark_read(&mut conn, world.ada, chat, third)
        .await
        .unwrap();
    drop(conn);
    post(
        &world,
        chat,
        json!([{"type":"text","text":"A later unrelated message."}]),
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = owner(&world, &mut conn).await;
    let after = sources::materialize(
        &mut conn,
        &principal,
        target,
        None,
        Some(range),
        budget(32_000),
    )
    .await
    .unwrap();
    assert_eq!(before.snapshot, after.snapshot);
    assert_eq!(before.activity, after.activity);
    let truncated =
        sources::materialize(&mut conn, &principal, target, None, Some(range), budget(8))
            .await
            .unwrap();
    assert!(!truncated.complete);
    assert_eq!(truncated.activity[0].text, "Please review it.");
    for range in [
        SummaryChatRange {
            after: -1,
            through: third,
        },
        SummaryChatRange {
            after: third,
            through: second,
        },
        SummaryChatRange {
            after: 0,
            through: i64::MAX,
        },
    ] {
        assert!(matches!(
            sources::materialize(
                &mut conn,
                &principal,
                target,
                None,
                Some(range),
                budget(32_000)
            )
            .await,
            Err(SourceError::Range)
        ));
    }
}

#[tokio::test]
async fn a_topic_includes_its_native_chat_but_excludes_an_owned_private_group_without_spending_its_budget()
 {
    let world = World::new().await;
    world.engine().stop();
    let mut tx = world.pool.begin().await.unwrap();
    let node_chat = match chats::chat_of_node(&mut tx, world.topic).await.unwrap() {
        Some(chat) => chat.id,
        None => {
            let id = new_id();
            sqlx::query("INSERT INTO chats(id,kind,node_id,org_id,created_by,created_at) VALUES($1,'node',$2,$3,$4,$5)").bind(id).bind(world.topic).bind(world.root).bind(world.ada).bind(world.clock.now()).execute(&mut *tx).await.unwrap();
            id
        }
    };
    let group = new_id();
    chats::insert_chat_at(
        &mut tx,
        &chats::NewChat {
            id: group,
            kind: chats::KIND_GROUP,
            org_id: world.root,
            direct_key: None,
            name: Some("Private group canary"),
            created_by: world.ada,
            at: world.clock.now(),
        },
        Some(world.topic),
    )
    .await
    .unwrap()
    .unwrap();
    let seq = chats::next_seq(&mut tx, group).await.unwrap().unwrap();
    chats::add_member(
        &mut tx,
        &chats::NewMember {
            chat_id: group,
            user_id: world.ada,
            added_by: Some(world.ada),
            guest: false,
            seq,
            at: world.clock.now(),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    post(
        &world,
        group,
        json!([{"type":"text","text":"Never put this private group into the rollup."}]),
    )
    .await;
    post(
        &world,
        node_chat,
        json!([{"type":"text","text":"The topic chat's current visible progress."}]),
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = owner(&world, &mut conn).await;
    assert!(
        sources::readable(&mut conn, &principal, SummaryTarget::Chat { id: group })
            .await
            .unwrap(),
        "the owner could read it directly; the topic boundary still excludes it"
    );
    let before = sources::materialize(
        &mut conn,
        &principal,
        SummaryTarget::Topic { id: world.topic },
        None,
        None,
        budget(32_000),
    )
    .await
    .unwrap();
    assert!(
        before
            .activity
            .iter()
            .any(|item| item.text.contains("current visible progress"))
    );
    assert_eq!(
        before.source_refs,
        vec![
            SummaryTarget::Topic { id: world.topic },
            SummaryTarget::Chat { id: node_chat }
        ]
    );
    assert!(
        !serde_json::to_string(&before.activity)
            .unwrap()
            .contains("private group")
    );
    post(&world,group,json!([{"type":"text","text":"Another private change must not mark the public text stale."}])).await;
    let after = sources::materialize(
        &mut conn,
        &principal,
        SummaryTarget::Topic { id: world.topic },
        None,
        None,
        budget(32_000),
    )
    .await
    .unwrap();
    assert!(sources::same_content(&before.snapshot, &after.snapshot));
    assert_eq!(before.source_refs, after.source_refs);
    assert_eq!(
        before.complete, after.complete,
        "private chatter never consumes the public source budget"
    );
}
