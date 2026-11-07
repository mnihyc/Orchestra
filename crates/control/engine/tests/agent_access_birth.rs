//! Creation choices are ordinary pure gifts, atomic with the new holder and bounded by its caller.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::World;
use orch_access_contract::permissions::Permission;
use orch_api_contract::nodes::{AgentAccessChoice, AgentAccessWhere, FirstInput};
use orch_api_contract::share::{ShareDocumentsLevel, ShareLevel, ShareParts};
use orch_api_contract::threads::ContentPart;
use orch_authz::reach::{Agent, AgentReach, Reach, Things};
use orch_authz::{Act, Caller, Policy, Principal, TokenGrant};
use orch_db::audit::Origin;
use orch_engine::binding::{Birth, BirthAgentAccess, CreateThreadError, NewThread};
use orch_engine::sharing::{Record, Subject};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time, new_id};
use serde_json::json;
use std::collections::BTreeSet;

fn choice(where_: AgentAccessWhere, documents: Option<ShareDocumentsLevel>) -> AgentAccessChoice {
    AgentAccessChoice {
        where_,
        parts: ShareParts {
            documents,
            ..ShareParts::default()
        },
    }
}

fn new(world: &World, name: &str) -> NewThread {
    NewThread {
        topic: world.topic,
        name: name.into(),
        agent: world.agent,
        provider: world.provider,
        environment: world.environment,
        directory: "web".into(),
        arrangement: None,
        model: Some(common::MODEL.into()),
        effort: None,
    }
}

async fn make(world: &World, name: &str, agent_access: Option<AgentAccessChoice>) -> Id {
    world
        .engine()
        .create_thread_with(
            &Caller::session(world.ada),
            new(world, name),
            Birth {
                agent_access: agent_access.into(),
                ..Birth::default()
            },
        )
        .await
        .unwrap()
        .thread
}

async fn shares(world: &World, thread: Id) -> Vec<Record> {
    world
        .engine()
        .shares_of_thread(&Caller::session(world.ada), &Origin::default(), thread)
        .await
        .unwrap()
        .shares
}

async fn reach(world: &World, thread: Id, nodes: &[Id]) -> Reach {
    let mut conn = world.pool.acquire().await.unwrap();
    let agent = Agent::of(&mut conn, thread).await.unwrap().unwrap();
    Reach::load(
        &mut conn,
        agent,
        AgentReach::None,
        Time::new(world.clock.now()),
        &Things {
            nodes: nodes.to_vec(),
            ..Things::default()
        },
    )
    .await
    .unwrap()
}

async fn gift_count(world: &World, thread: Id) -> i64 {
    world
        .count(
            "SELECT count(*) FROM permission_entries WHERE holder_kind='agents' AND holder_id=$1 AND removed_at IS NULL",
            thread,
        )
        .await
}

#[tokio::test]
async fn a_creation_preset_becomes_area_only_entries_and_parent_changes_do_not_rewrite_them() {
    let world = World::observed().await;
    let expected = choice(AgentAccessWhere::Topic, Some(ShareDocumentsLevel::Edit));
    let first = make(&world, "Accept the creation preset", None).await;
    let original = shares(&world, first).await;
    assert_eq!(original.len(), 1);
    assert_eq!(original[0].subject, Subject::Node(world.topic));
    assert_eq!(original[0].to, first);
    assert_eq!(original[0].by, Some(world.ada));
    assert!(original[0].inside);
    assert_eq!(original[0].parts, expected.parts);
    let permissions: BTreeSet<&str> = original[0]
        .grants
        .iter()
        .map(|row| row.permission.as_str())
        .collect();
    assert_eq!(permissions, BTreeSet::from(["doc.view", "doc.edit"]));
    assert!(
        original[0]
            .grants
            .iter()
            .all(|row| row.created_by == Some(world.ada))
    );
    let standing = reach(&world, first, &[first, world.topic]).await;
    assert!(standing.documents_of(world.topic).write.is_some());
    assert!(!standing.may_download_node_chat(world.topic));
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                first
            )
            .await,
        0
    );
    assert_eq!(
        world.commands.issued_of("turn.start"),
        0,
        "a gift is not an instruction"
    );

    world
        .set(
            world.topic,
            keys::THREADS_AGENT_ACCESS,
            json!({"where":"nothing", "parts":{}}),
        )
        .await;
    let kept = shares(&world, first).await;
    assert_eq!(
        kept[0].grants, original[0].grants,
        "editing a preset never rewrites existing entries"
    );
    let second = make(&world, "Nothing from the changed preset", None).await;
    assert_eq!(gift_count(&world, second).await, 0);
    let explicit = make(&world, "Confirm an exact choice", Some(expected)).await;
    assert_eq!(shares(&world, explicit).await[0].parts, expected.parts);
    world.shutdown().await;
}

#[tokio::test]
async fn this_thread_gives_associated_documents_and_chat_without_siblings_or_an_own_conversation_grant()
 {
    let world = World::observed().await;
    let sibling = make(
        &world,
        "An unrelated sibling",
        Some(choice(AgentAccessWhere::Nothing, None)),
    )
    .await;
    let mut selected = choice(AgentAccessWhere::Thread, Some(ShareDocumentsLevel::Edit));
    selected.parts.chat = Some(ShareLevel::Write);
    let thread = make(&world, "Only this thread's material", Some(selected)).await;
    let records = shares(&world, thread).await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].subject, Subject::Node(thread));
    assert!(!records[0].inside);
    assert_eq!(records[0].parts, selected.parts);
    assert!(
        records[0]
            .grants
            .iter()
            .all(|row| row.object_id == thread && row.page == "live")
    );
    assert!(
        records[0]
            .grants
            .iter()
            .all(|row| !row.permission.starts_with("thread.")
                && !row.permission.ends_with(".export"))
    );
    let standing = reach(&world, thread, &[thread, sibling, world.topic]).await;
    assert!(
        standing.conversation(thread, false).read.is_some(),
        "its own conversation is intrinsic"
    );
    assert!(standing.documents_of(thread).write.is_some());
    assert!(standing.node_chat(thread).write.is_some());
    assert!(standing.documents_of(world.topic).read.is_none());
    assert!(standing.node_chat(world.topic).read.is_none());
    assert!(standing.conversation(sibling, false).read.is_none());
    assert!(standing.conversation(sibling, false).write.is_none());
    world.shutdown().await;
}

#[tokio::test]
async fn an_unavailable_omitted_preset_falls_back_but_the_same_explicit_choice_refuses_atomically()
{
    let world = World::observed().await;
    let bea = world.person("bea").await;
    world.give(bea, "developer").await;
    let requested = choice(AgentAccessWhere::Project, Some(ShareDocumentsLevel::Manage));
    world
        .set(world.topic, keys::THREADS_AGENT_ACCESS, json!(requested))
        .await;
    let caller = Caller::session(bea);
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = Principal::load(&mut conn, caller.clone(), Time::new(world.clock.now()))
        .await
        .unwrap();
    assert!(
        orch_authz::nodes::check(
            &mut conn,
            &principal,
            world.topic,
            Permission::TopicStartThreads,
            Act::Use,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    assert!(
        !orch_authz::nodes::check(
            &mut conn,
            &principal,
            world.project,
            Permission::DocManage,
            Act::Use,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    drop(conn);
    let made = world
        .engine()
        .create_thread_with(
            &caller,
            new(&world, "Create without a nongiveable preset"),
            Birth::default(),
        )
        .await
        .unwrap();
    assert_eq!(gift_count(&world, made.thread).await, 0);
    let rejected_id = new_id();
    let result = world
        .engine()
        .create_thread_as(
            &caller,
            new(&world, "An explicit nongiveable contribution"),
            Birth {
                agent_access: BirthAgentAccess::Exact(requested),
                first_input: Some(FirstInput {
                    content: vec![ContentPart::Text {
                        text: "Must not escape the refusal".into(),
                    }],
                    client_id: "refused-gift-input".into(),
                }),
                ..Birth::default()
            },
            Some(rejected_id),
        )
        .await;
    assert!(
        matches!(result, Err(CreateThreadError::AgentAccess(_))),
        "{result:?}"
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM nodes WHERE id=$1", rejected_id)
            .await,
        0
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                rejected_id
            )
            .await,
        0
    );
    assert_eq!(gift_count(&world, rejected_id).await, 0);
    assert_eq!(world.commands.issued_of("turn.start"), 0);
    world.shutdown().await;
}

#[tokio::test]
async fn a_creation_gift_keeps_token_scope_and_subtree_instead_of_using_the_owners_session() {
    let world = World::observed().await;
    let requested = choice(AgentAccessWhere::Topic, Some(ShareDocumentsLevel::Edit));
    let scopes = [
        "topic.start_threads",
        "topic.view",
        "thread.view",
        "access.manage",
        "doc.view",
    ]
    .map(str::to_owned);
    let caller = Caller::token(
        world.ada,
        TokenGrant::from_stored(&scopes, Some(world.topic)).unwrap(),
    );
    let omitted = world
        .engine()
        .create_thread_with(
            &caller,
            new(&world, "A clipped preset is optional"),
            Birth::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        gift_count(&world, omitted.thread).await,
        0,
        "even an earlier readable row of a partial attempt is rolled back"
    );
    let id = new_id();
    let result = world
        .engine()
        .create_thread_as(
            &caller,
            new(&world, "A token cannot lend missing Edit"),
            Birth {
                agent_access: Some(requested).into(),
                ..Birth::default()
            },
            Some(id),
        )
        .await;
    assert!(
        matches!(result, Err(CreateThreadError::AgentAccess(_))),
        "{result:?}"
    );
    assert_eq!(gift_count(&world, id).await, 0);
    assert_eq!(
        world
            .count("SELECT count(*) FROM nodes WHERE id=$1", id)
            .await,
        0
    );

    let subtree = Caller::token(
        world.ada,
        TokenGrant::from_stored(&["*".into()], Some(world.topic)).unwrap(),
    );
    let id = new_id();
    let result = world
        .engine()
        .create_thread_as(
            &subtree,
            new(&world, "A topic token cannot lend its parent"),
            Birth {
                agent_access: Some(choice(
                    AgentAccessWhere::Project,
                    Some(ShareDocumentsLevel::Edit),
                ))
                .into(),
                ..Birth::default()
            },
            Some(id),
        )
        .await;
    assert!(
        matches!(result, Err(CreateThreadError::AgentAccess(_))),
        "{result:?}"
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM nodes WHERE id=$1", id)
            .await,
        0
    );
    world.shutdown().await;
}

#[tokio::test]
async fn a_new_holder_its_gift_and_first_input_share_the_outer_transaction() {
    let world = World::observed().await;
    let id = new_id();
    let mut tx = world.pool.begin().await.unwrap();
    let created = world
        .engine()
        .create_thread_in_transaction(
            &mut tx,
            &Caller::session(world.ada),
            &new(&world, "Owned by the outer transaction"),
            &Birth {
                agent_access: Some(choice(
                    AgentAccessWhere::Topic,
                    Some(ShareDocumentsLevel::Edit),
                ))
                .into(),
                first_input: Some(FirstInput {
                    content: vec![ContentPart::Text {
                        text: "Only after commit".into(),
                    }],
                    client_id: "outer-transaction-input".into(),
                }),
                ..Birth::default()
            },
            id,
            world.clock.now(),
        )
        .await
        .unwrap();
    assert_eq!(created.thread, id);
    let gifts: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE holder_kind='agents' AND holder_id=$1 AND removed_at IS NULL").bind(id).fetch_one(&mut *tx).await.unwrap();
    assert!(gifts > 0);
    let inputs: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(inputs, 1);
    assert_eq!(
        world
            .count("SELECT count(*) FROM nodes WHERE id=$1", id)
            .await,
        0,
        "neither holder nor input is visible before commit"
    );
    assert_eq!(world.commands.issued_of("turn.start"), 0);
    tx.rollback().await.unwrap();
    assert_eq!(
        world
            .count("SELECT count(*) FROM nodes WHERE id=$1", id)
            .await,
        0
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM thread_inputs WHERE thread_id=$1", id)
            .await,
        0
    );
    assert_eq!(gift_count(&world, id).await, 0);
    world.shutdown().await;
}

#[tokio::test]
async fn invalid_nothing_areas_and_this_thread_timeline_are_refused_without_a_holder() {
    let world = World::observed().await;
    let mut thread = choice(AgentAccessWhere::Thread, None);
    thread.parts.timeline = Some(ShareLevel::Write);
    for invalid in [
        choice(AgentAccessWhere::Nothing, Some(ShareDocumentsLevel::Edit)),
        thread,
    ] {
        let id = new_id();
        let result = world
            .engine()
            .create_thread_as(
                &Caller::session(world.ada),
                new(&world, "An invalid area combination"),
                Birth {
                    agent_access: Some(invalid).into(),
                    ..Birth::default()
                },
                Some(id),
            )
            .await;
        assert!(
            matches!(result, Err(CreateThreadError::AgentAccess(_))),
            "{result:?}"
        );
        assert_eq!(
            world
                .count("SELECT count(*) FROM nodes WHERE id=$1", id)
                .await,
            0
        );
        assert_eq!(gift_count(&world, id).await, 0);
    }
    world.shutdown().await;
}
