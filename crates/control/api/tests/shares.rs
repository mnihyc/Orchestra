//! Sharing with agents over HTTP: a thread shared live with another thread's agent, listed from the three sides, changed and taken back; who may share and what they are told when they may not; a copy before anything can keep it; and the tree of an agent's reach as a person is shown it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate,
};
use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
use orch_api_contract::share::{
    AgentShareList, AgentShareListQuery, AgentShareSide, ShareAgentCreate, ShareAgentCreateReply,
    ShareAgentDelete, ShareAgentUpdate, ShareAgentUpdateBody, ShareDocumentsLevel, ShareLevel,
    ShareParts, ShareTo, ShareToThread, ShareWhat, ShareWhatNode, ShareWithAgents, SharedEntry,
    SharedEntryKind, SharedEntryVia, ThreadSharedGet, ThreadSharedPolicy, ThreadSharedTreeGet,
    ThreadSharedTreeGetQuery,
};
use orch_api_contract::threads::{ShareMode, Thread};
use orch_settings::Target;
use orch_types::Id;
use places::Who;
use rig::Scene;
use serde_json::json;

fn body(
    what: Id,
    parts: Option<ShareParts>,
    mode: ShareMode,
    to: Id,
    client: &str,
) -> ShareWithAgents {
    ShareWithAgents {
        read_first: None,
        download: None,
        what: ShareWhat::Node(ShareWhatNode { node_id: what }),
        inside: None,
        parts,
        mode,
        to: ShareTo::Threads(vec![ShareToThread { thread_id: to }]),
        comment: None,
        expires_at: None,
        client_id: client.to_owned(),
    }
}

fn timeline(level: ShareLevel) -> Option<ShareParts> {
    Some(ShareParts {
        timeline: Some(level),
        ..ShareParts::default()
    })
}

async fn create(
    s: &Scene,
    by: &Who,
    request: &ShareWithAgents,
) -> orch_api_core::testing::Reply<ShareAgentCreateReply> {
    s.rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&by.token)
        .body(request)
        .send()
        .await
}

/// The agents of the world take an MCP server, so that the tools can be shared.
async fn agents_take_tools(s: &Scene) {
    s.rig.tools_of_agents(json!("mcp")).await;
}

async fn thread_in(s: &Scene, by: &Who, topic: Id, name: &str) -> Thread {
    s.rig
        .world
        .app
        .call(ThreadCreate, &[&topic])
        .token(&by.token)
        .body(&CreateThread {
            name: Some(name.to_owned()),
            binding: Some(CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: s.on.provider,
                arrangement: None,
            }),
            model: Some(rig::MODEL.to_owned()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            directory: Some("web".to_owned()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
}

/// A scene with a thread that writes, and a thread whose agent is to be given it.
async fn two_threads() -> (Scene, Thread, Thread) {
    let s = Scene::new().await;
    agents_take_tools(&s).await;
    let shared = s.thread_by(&s.ada, "Builder").await;
    let reader = s.thread_by(&s.ada, "Reader").await;
    (s, shared, reader)
}

#[tokio::test]
async fn a_thread_shared_live_is_listed_from_every_side_changed_and_taken_back() {
    let (s, shared, reader) = two_threads().await;
    let (shared_id, reader_id) = (shared.base.id, reader.base.id);
    let made = create(
        &s,
        &s.ada,
        &body(
            shared_id,
            timeline(ShareLevel::Write),
            ShareMode::Live,
            reader_id,
            "c1",
        ),
    )
    .await;
    assert_eq!(made.status, http::StatusCode::CREATED);
    let made = made.ok();
    assert_eq!(made.shares.len(), 1);
    let share = &made.shares[0];
    assert_eq!(share.mode, ShareMode::Live);
    assert_eq!(share.parts.timeline, Some(ShareLevel::Write));
    assert_eq!(
        share.entries.len(),
        3,
        "the View row, the Interact row and the Export row"
    );
    assert!(
        share.download,
        "the agents may copy what is shared, as it comes"
    );
    assert_eq!(share.to.id, reader_id);
    assert_eq!(share.by.id, s.ada.id());

    // The three sides.
    let of = s
        .rig
        .world
        .app
        .call(AgentShareList, &[&"nodes", &shared_id])
        .token(&s.ada.token)
        .query(&AgentShareListQuery { side: None })
        .send()
        .await
        .ok();
    assert_eq!(of.len(), 1);
    assert_eq!(of[0].id, share.id);
    let to = s
        .rig
        .world
        .app
        .call(AgentShareList, &[&"nodes", &reader_id])
        .token(&s.ada.token)
        .query(&AgentShareListQuery {
            side: Some(AgentShareSide::To),
        })
        .send()
        .await
        .ok();
    assert_eq!(to.len(), 1, "what names the agents of that thread");
    let known = s
        .rig
        .world
        .app
        .call(ThreadSharedGet, &[&reader_id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(known.policy, ThreadSharedPolicy::None);
    assert_eq!(known.shares.len(), 1);

    // The tree the agent lists is the one the person is shown.
    let tree = s
        .rig
        .world
        .app
        .call(ThreadSharedTreeGet, &[&reader_id])
        .token(&s.ada.token)
        .query(&ThreadSharedTreeGetQuery {
            path: Some(format!("id:{}", s.tree.topic.base.id)),
            depth: Some(2),
        })
        .send()
        .await
        .ok();
    let conversation = tree
        .entries
        .iter()
        .find(|entry| entry.kind == SharedEntryKind::Conversation && entry.id == shared_id)
        .expect("the shared thread's conversation");
    assert!(
        conversation.readable && conversation.writable,
        "{conversation:?}"
    );
    assert_eq!(conversation.via, SharedEntryVia::Share);
    assert_eq!(
        conversation.shared_by.as_ref().map(|by| by.id),
        Some(s.ada.id())
    );
    assert!(!conversation.redacted);

    // Changed: only to read.
    let changed = s
        .rig
        .world
        .app
        .call(ShareAgentUpdate, &[&share.id])
        .token(&s.ada.token)
        .body(&ShareAgentUpdateBody {
            read_first: None,
            parts: timeline(ShareLevel::Read),
            expires_at: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(changed.parts.timeline, Some(ShareLevel::Read));
    assert_eq!(
        changed.entries.len(),
        2,
        "the View row and the Export row stay, the Interact row goes"
    );
    assert!(changed.download);

    // Taken back.
    let gone = s
        .rig
        .world
        .app
        .call(ShareAgentDelete, &[&share.id])
        .token(&s.ada.token)
        .send()
        .await;
    assert_eq!(gone.status, http::StatusCode::NO_CONTENT);
    let after = s
        .rig
        .world
        .app
        .call(AgentShareList, &[&"nodes", &shared_id])
        .token(&s.ada.token)
        .query(&AgentShareListQuery { side: None })
        .send()
        .await
        .ok();
    assert!(after.is_empty());
    let again = s
        .rig
        .world
        .app
        .call(ShareAgentDelete, &[&share.id])
        .token(&s.ada.token)
        .send()
        .await;
    assert_eq!(again.problem().code, ErrorCode::NotFound);
}

#[tokio::test]
async fn asking_again_with_the_same_value_answers_what_was_made_and_makes_no_more() {
    let (s, shared, reader) = two_threads().await;
    let request = body(
        shared.base.id,
        None,
        ShareMode::Live,
        reader.base.id,
        "same",
    );
    let first = create(&s, &s.ada, &request).await;
    assert_eq!(first.status, http::StatusCode::CREATED);
    let second = create(&s, &s.ada, &request).await;
    assert_eq!(second.status, http::StatusCode::OK);
    assert_eq!(second.ok().shares[0].id, first.ok().shares[0].id);
}

#[tokio::test]
async fn somebody_who_cannot_see_the_thing_finds_nothing_there() {
    let (s, shared, reader) = two_threads().await;
    let refused = create(
        &s,
        &s.bea,
        &body(shared.base.id, None, ShareMode::Live, reader.base.id, "c1"),
    )
    .await;
    assert_eq!(refused.problem().code, ErrorCode::NotFound);
}

#[tokio::test]
async fn somebody_who_sees_it_but_does_not_manage_its_page_is_told_what_is_missing() {
    let (s, shared, reader) = two_threads().await;
    let refused = create(
        &s,
        &s.cai,
        &body(shared.base.id, None, ShareMode::Live, reader.base.id, "c1"),
    )
    .await;
    let problem = refused.problem();
    assert_eq!(problem.code, ErrorCode::AuthzDenied);
    assert!(
        places::missing_permissions(&problem)
            .iter()
            .any(|each| each.ends_with(".interact") || each == "access.manage"),
        "{problem:?}"
    );
}

#[tokio::test]
async fn a_copy_waits_for_the_threads_next_turn_with_its_files_kept_and_pinned() {
    let (s, shared, reader) = two_threads().await;
    agents_take_tools(&s).await;
    create(
        &s,
        &s.ada,
        &body(
            shared.base.id,
            None,
            ShareMode::Snapshot,
            reader.base.id,
            "c1",
        ),
    )
    .await
    .ok();
    let (state, path, pins): (String, String, i32) = sqlx::query_as(
        "SELECT s.state, s.path, b.pin_count FROM agent_shares s JOIN blobs b ON b.hash = s.manifest_blob",
    )
    .fetch_one(&s.rig.world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        state, "pending",
        "nothing is written before the thread's next turn"
    );
    assert!(path.starts_with(".orch/shared/"), "{path}");
    assert_eq!(
        pins, 1,
        "the list of its files is held until it is written or taken back"
    );
}

#[tokio::test]
async fn a_copy_cannot_be_asked_for_writing_and_a_share_names_some_thread() {
    let (s, shared, reader) = two_threads().await;
    let writing = create(
        &s,
        &s.ada,
        &body(
            shared.base.id,
            timeline(ShareLevel::Write),
            ShareMode::Snapshot,
            reader.base.id,
            "c1",
        ),
    )
    .await;
    assert_eq!(writing.problem().code, ErrorCode::RequestInvalid);
    let nobody = create(
        &s,
        &s.ada,
        &ShareWithAgents {
            read_first: None,
            to: ShareTo::Threads(vec![]),
            ..body(shared.base.id, None, ShareMode::Live, reader.base.id, "c2")
        },
    )
    .await;
    assert_eq!(nobody.problem().code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_thread_of_a_topic_without_the_tools_cannot_be_shared_with() {
    let (s, shared, reader) = two_threads().await;
    s.rig.tools_of_agents(json!("none")).await;
    let refused = create(
        &s,
        &s.ada,
        &body(shared.base.id, None, ShareMode::Live, reader.base.id, "c1"),
    )
    .await;
    let problem = refused.problem();
    assert_eq!(problem.code, ErrorCode::ShareUnavailable);
    assert_eq!(problem.extra["reason"], "no_tools");
}

#[tokio::test]
async fn the_tree_a_person_is_shown_has_no_name_they_may_not_see_themself() {
    let (s, _, reader) = two_threads().await;
    let secret =
        places::make_topic(&s.rig.world, &s.ada, s.tree.project.base.id, "Hidden plans").await;
    let hidden = thread_in(&s, &s.ada, secret.base.id, "Whispers").await;
    create(
        &s,
        &s.ada,
        &body(hidden.base.id, None, ShareMode::Live, reader.base.id, "c1"),
    )
    .await
    .ok();
    // Bea may see the reader's topic and nothing else.
    places::give_role(&s.rig.world, s.tree.topic.base.id, &s.bea, "viewer", &s.ada).await;
    let tree = s
        .rig
        .world
        .app
        .call(ThreadSharedTreeGet, &[&reader.base.id])
        .token(&s.bea.token)
        .query(&ThreadSharedTreeGetQuery {
            path: None,
            depth: Some(4),
        })
        .send()
        .await
        .ok();
    let written = serde_json::to_string(&tree).unwrap();
    assert!(!written.contains("Hidden plans"), "{written}");
    assert!(!written.contains("Whispers"), "{written}");
    let place = tree
        .entries
        .iter()
        .find(|entry| entry.id == secret.base.id)
        .expect("the place is there, as an entry that has no name");
    assert!(place.redacted && place.name.is_none());
    assert_eq!(place.path, format!("id:{}", secret.base.id));
    // The same tree to Ada has the names.
    let full = s
        .rig
        .world
        .app
        .call(ThreadSharedTreeGet, &[&reader.base.id])
        .token(&s.ada.token)
        .query(&ThreadSharedTreeGetQuery {
            path: None,
            depth: Some(4),
        })
        .send()
        .await
        .ok();
    assert!(serde_json::to_string(&full).unwrap().contains("Whispers"));
}

#[tokio::test]
async fn a_tree_is_asked_for_one_to_four_levels_deep() {
    let (s, _, reader) = two_threads().await;
    for depth in [0_u32, 5] {
        let refused = s
            .rig
            .world
            .app
            .call(ThreadSharedTreeGet, &[&reader.base.id])
            .token(&s.ada.token)
            .query(&ThreadSharedTreeGetQuery {
                path: None,
                depth: Some(depth),
            })
            .send()
            .await;
        assert_eq!(
            refused.problem().code,
            ErrorCode::RequestInvalid,
            "depth {depth}"
        );
    }
    let _ = ShareDocumentsLevel::Read;
}

/// A document written beside `node` by Ada, and its identifier.
async fn document_beside(s: &Scene, node: Id, title: &str) -> Id {
    s.rig
        .world
        .app
        .call(NodeDocumentCreate, &[&node])
        .token(&s.ada.token)
        .body(&NodeDocumentCreateBody {
            title: title.to_owned(),
            blocks: None,
            markdown: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .base
        .id
}

/// `by` takes the share back.
async fn take_back(s: &Scene, by: &Who, share: Id) -> orch_api_core::testing::Reply<()> {
    s.rig
        .world
        .app
        .call(ShareAgentDelete, &[&share])
        .token(&by.token)
        .send()
        .await
}

/// What the agents of `thread` reach at and under `node`, as their tools list it to Ada.
async fn reached_under(s: &Scene, thread: Id, node: Id) -> Vec<SharedEntry> {
    s.rig
        .world
        .app
        .call(ThreadSharedTreeGet, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadSharedTreeGetQuery {
            path: Some(format!("id:{node}")),
            depth: Some(4),
        })
        .send()
        .await
        .ok()
        .entries
}

fn reads(entries: &[SharedEntry], document: Id) -> bool {
    entries.iter().any(|entry| {
        entry.id == document && entry.kind == SharedEntryKind::Document && entry.readable
    })
}

#[tokio::test]
async fn a_place_is_shared_by_itself_or_with_what_is_in_it_and_the_list_tells_which_is_which() {
    let (s, _, reader) = two_threads().await;
    let (project, topic, reader_id) =
        (s.tree.project.base.id, s.tree.topic.base.id, reader.base.id);
    // As shipped the agents of a topic write the documents beside it; here only shares give any.
    s.rig
        .world
        .set_at(
            Target::Node(s.tree.org.base.id),
            "sharing.agent_documents",
            json!("none"),
        )
        .await;
    let (outer, inner) = (
        document_beside(&s, project, "Outer").await,
        document_beside(&s, topic, "Inner").await,
    );
    let documents = Some(ShareParts {
        documents: Some(ShareDocumentsLevel::Read),
        ..ShareParts::default()
    });
    let chat = Some(ShareParts {
        chat: Some(ShareLevel::Read),
        ..ShareParts::default()
    });
    let share = |client: &str, inside: Option<bool>, parts: Option<ShareParts>| ShareWithAgents {
        read_first: None,
        inside,
        parts,
        ..body(project, None, ShareMode::Live, reader_id, client)
    };
    // A place has no conversation of its own, so there is none to share with the place by itself.
    let refused = create(
        &s,
        &s.ada,
        &share("c0", Some(false), timeline(ShareLevel::Read)),
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(refused.extra["errors"][0]["path"], "parts.timeline");
    // By itself: the place and the documents beside it, and nothing that lies inside.
    let alone = create(&s, &s.ada, &share("c1", Some(false), documents))
        .await
        .ok()
        .shares
        .remove(0);
    assert!(!alone.inside);
    let seen = reached_under(&s, reader_id, project).await;
    assert!(reads(&seen, outer), "{seen:?}");
    assert!(!reads(&seen, inner), "{seen:?}");
    // With what is inside, for the same agents but another area: both stand, each its own, and the
    // list says which is which.
    let with = create(&s, &s.ada, &share("c2", None, chat))
        .await
        .ok()
        .shares
        .remove(0);
    assert!(with.inside);
    assert_ne!(with.id, alone.id);
    let listed = s
        .rig
        .world
        .app
        .call(AgentShareList, &[&"nodes", &project])
        .token(&s.ada.token)
        .query(&AgentShareListQuery { side: None })
        .send()
        .await
        .ok();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|one| one.id == alone.id && !one.inside));
    assert!(listed.iter().any(|one| one.id == with.id && one.inside));
    let seen = reached_under(&s, reader_id, project).await;
    assert!(reads(&seen, outer) && !reads(&seen, inner), "{seen:?}");
    // Each is taken back by itself.
    let delete = |id: Id| take_back(&s, &s.ada, id);
    assert_eq!(delete(with.id).await.status, http::StatusCode::NO_CONTENT);
    let seen = reached_under(&s, reader_id, project).await;
    assert!(reads(&seen, outer) && !reads(&seen, inner), "{seen:?}");
    // The documents shared both ways are two shares of the place: the one asked for is the answer,
    // and the place itself is kept as its own reach beside the one with what is in it.
    let both = create(&s, &s.ada, &share("c3", None, documents))
        .await
        .ok()
        .shares
        .remove(0);
    assert!(both.inside);
    let seen = reached_under(&s, reader_id, project).await;
    assert!(reads(&seen, outer) && reads(&seen, inner), "{seen:?}");
    let again = create(&s, &s.ada, &share("c4", Some(false), documents))
        .await
        .ok()
        .shares
        .remove(0);
    assert!(!again.inside);
    assert_ne!(
        again.id, both.id,
        "the place itself is retained apart from what covers it"
    );
    assert_eq!(delete(both.id).await.status, http::StatusCode::NO_CONTENT);
    let seen = reached_under(&s, reader_id, project).await;
    assert!(reads(&seen, outer) && !reads(&seen, inner), "{seen:?}");
    assert_eq!(delete(again.id).await.status, http::StatusCode::NO_CONTENT);
    let seen = reached_under(&s, reader_id, project).await;
    assert!(!reads(&seen, outer) && !reads(&seen, inner), "{seen:?}");
}

#[tokio::test]
async fn somebody_who_holds_what_they_share_shares_it_and_takes_back_their_own_while_the_place_lets_them()
 {
    let (s, shared, reader) = two_threads().await;
    let (project, reader_id) = (s.tree.project.base.id, reader.base.id);
    let org = s.tree.org.base.id;
    let outer = document_beside(&s, project, "Outer").await;
    let by_dan = ShareWithAgents {
        read_first: None,
        download: None,
        inside: Some(false),
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..ShareParts::default()
        }),
        ..body(project, None, ShareMode::Live, reader_id, "d1")
    };
    // Dan is a Developer of the org: he holds the documents and does not manage the project's page.
    let made = create(&s, &s.dan, &by_dan).await.ok().shares.remove(0);
    assert_eq!(made.by.id, s.dan.id(), "the share says who gave it");
    assert!(
        made.entries
            .iter()
            .all(|entry| entry.created_by.as_ref().map(|by| by.id) == Some(s.dan.id())),
        "and so does each entry"
    );
    assert!(reads(&reached_under(&s, reader_id, project).await, outer));
    // Cai, a viewer of the org, is let interact with the thread whose agents are to be told. She
    // shares what she holds of the other thread, which is reading it, and is refused its writing,
    // with what she lacks.
    s.rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &reader_id])
        .token(&s.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: s.cai.id() }],
                choice: AccessChoice::FullInteract,
                export: None,
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let write = create(
        &s,
        &s.cai,
        &body(
            shared.base.id,
            timeline(ShareLevel::Write),
            ShareMode::Live,
            reader_id,
            "v1",
        ),
    )
    .await
    .problem();
    assert_eq!(write.code, ErrorCode::AuthzDenied);
    assert_eq!(write.extra["required"], "thread.interact");
    let read = create(
        &s,
        &s.cai,
        &body(
            shared.base.id,
            timeline(ShareLevel::Read),
            ShareMode::Live,
            reader_id,
            "v2",
        ),
    )
    .await
    .ok()
    .shares
    .remove(0);
    assert_eq!(read.by.id, s.cai.id());
    // Eve shares the topic by itself, which is another share: nobody takes back what another
    // shared unless they manage the page.
    let by_eve = ShareWithAgents {
        read_first: None,
        download: None,
        what: ShareWhat::Node(ShareWhatNode {
            node_id: s.tree.topic.base.id,
        }),
        client_id: "e1".to_owned(),
        ..by_dan.clone()
    };
    let eve_share = create(&s, &s.eve, &by_eve).await.ok().shares.remove(0);
    assert_ne!(eve_share.id, made.id);
    assert_eq!(
        take_back(&s, &s.dan, eve_share.id).await.problem().code,
        ErrorCode::AuthzDenied,
        "a sharer does not take back another's"
    );
    // Where the place keeps sharing to those who manage the page, Dan shares no more and cannot
    // take back what he gave; Ada, who manages it, takes back every share.
    s.rig
        .world
        .set_at(Target::Node(org), "sharing.allow_holders", json!(false))
        .await;
    let more = create(
        &s,
        &s.dan,
        &ShareWithAgents {
            read_first: None,
            download: None,
            what: ShareWhat::Node(ShareWhatNode { node_id: org }),
            inside: Some(false),
            parts: Some(ShareParts {
                documents: Some(ShareDocumentsLevel::Read),
                ..ShareParts::default()
            }),
            ..body(project, None, ShareMode::Live, reader_id, "d2")
        },
    )
    .await
    .problem();
    assert_eq!(more.code, ErrorCode::AuthzDenied);
    assert_eq!(more.extra["required"], "access.manage");
    assert_eq!(
        take_back(&s, &s.dan, made.id).await.problem().code,
        ErrorCode::AuthzDenied
    );
    for share in [made.id, eve_share.id, read.id] {
        assert_eq!(
            take_back(&s, &s.ada, share).await.status,
            http::StatusCode::NO_CONTENT
        );
    }
}
