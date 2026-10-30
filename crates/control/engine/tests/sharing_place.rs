//! A place is shared with agents in two ways, with what is in it and by itself, and both can stand for the same place; and nobody shares more than they hold: the share is what sharing a thing with a person is.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_api_contract::share::{
    ShareAgentUpdateBody, ShareDocumentsLevel, ShareLevel, ShareParts, ShareTo, ShareToThread,
    ShareWhat, ShareWhatNode, ShareWithAgents,
};
use orch_api_contract::threads::ShareMode;
use orch_authz::Caller;
use orch_db::audit::Origin;
use orch_db::queries::access::testing as access_testing;
use orch_db::queries::hierarchy::testing::insert_node;
use orch_engine::sharing::{Record, ShareError, Subject, live_id, live_id_alone};
use orch_link_contract::commands::OrchTool;
use orch_types::{Id, new_id};
use serde_json::json;
use shared::*;

/// Two places, one inside the other, a document in each, and an agent that reads neither until it is
/// shared: the project Knowledge, with the topic Inner in it.
struct Knowledge {
    world: common::World,
    reader: Id,
    outer: Id,
    outer_doc: Id,
    inner_doc: Id,
}

async fn knowledge() -> Knowledge {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let (outer, inner) = (new_id(), new_id());
    {
        let mut conn = world.pool.acquire().await.unwrap();
        insert_node(
            &mut conn,
            outer,
            "project",
            Some(world.root),
            "Knowledge",
            world.ada,
        )
        .await
        .unwrap();
        insert_node(&mut conn, inner, "topic", Some(outer), "Inner", world.ada)
            .await
            .unwrap();
        // Whoever makes a place is its first owner, named in its Owners row.
        for place in [outer, inner] {
            access_testing::name_owner(&mut conn, place, ("user", world.ada), world.clock.now())
                .await
                .unwrap();
        }
    }
    let outer_doc = world
        .document_beside(outer, "Outer notes", &["Outer text."])
        .await;
    let inner_doc = world
        .document_beside(inner, "Inner notes", &["Inner text."])
        .await;
    Knowledge {
        world,
        reader,
        outer,
        outer_doc,
        inner_doc,
    }
}

fn area(
    timeline: Option<ShareLevel>,
    chat: Option<ShareLevel>,
    documents: Option<ShareDocumentsLevel>,
) -> Option<ShareParts> {
    Some(ShareParts {
        timeline,
        chat,
        documents,
    })
}

fn live(
    place: Id,
    inside: Option<bool>,
    parts: Option<ShareParts>,
    to: Id,
    client: &str,
) -> ShareWithAgents {
    ShareWithAgents {
        read_first: None,
        download: None,
        what: ShareWhat::Node(ShareWhatNode { node_id: place }),
        inside,
        parts,
        mode: ShareMode::Live,
        to: ShareTo::Threads(vec![ShareToThread { thread_id: to }]),
        comment: None,
        expires_at: None,
        client_id: client.to_owned(),
    }
}

async fn share_by(
    k: &Knowledge,
    person: Id,
    request: &ShareWithAgents,
) -> Result<Vec<Record>, ShareError> {
    k.world
        .engine()
        .share_with_agents(&Caller::session(person), &Origin::default(), request)
        .await
}

async fn read_doc(k: &Knowledge, document: Id) -> orch_link_contract::commands::OrchToolResult {
    call(
        &k.world,
        k.reader,
        OrchTool::ReadShared,
        json!({ "path": format!("id:{document}") }),
    )
    .await
}

async fn entries_of_the_agents(k: &Knowledge, place: Id, page: &str) -> i64 {
    k.world
        .count(
            &format!(
                "SELECT count(*) FROM permission_entries WHERE object_id = $1 AND page = '{page}'
                  AND holder_kind = 'agents' AND removed_at IS NULL"
            ),
            place,
        )
        .await
}

async fn shares_of(k: &Knowledge, place: Id) -> Vec<Record> {
    k.world
        .engine()
        .shares_of(
            &Caller::session(k.world.ada),
            &Origin::default(),
            Subject::Node(place),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn a_place_shared_by_itself_opens_its_own_documents_and_nothing_inside_it() {
    let k = knowledge().await;
    assert_eq!(refused(&read_doc(&k, k.outer_doc).await).0, "not_found");
    let made = share_by(
        &k,
        k.world.ada,
        &live(
            k.outer,
            Some(false),
            area(
                None,
                Some(ShareLevel::Read),
                Some(ShareDocumentsLevel::Read),
            ),
            k.reader,
            "c1",
        ),
    )
    .await
    .unwrap();
    assert_eq!(made.len(), 1);
    assert!(!made[0].inside, "it is the place by itself");
    assert_eq!(made[0].id, live_id_alone("node", k.outer, k.reader));
    assert_eq!(made[0].parts.chat, Some(ShareLevel::Read));
    assert_eq!(made[0].parts.documents, Some(ShareDocumentsLevel::Read));
    assert_eq!(made[0].parts.timeline, None);
    assert!(
        made[0].grants.iter().all(|grant| grant.page == "live"),
        "the entries of its own page"
    );
    assert_eq!(
        entries_of_the_agents(&k, k.outer, "defaults").await,
        0,
        "none on its defaults page"
    );
    assert!(entries_of_the_agents(&k, k.outer, "live").await >= 2);

    // The agent reads the document written beside the outer place, and not the one beside the place inside it.
    let text = ok(read_doc(&k, k.outer_doc).await)["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(text.contains("Outer text."), "{text}");
    assert_eq!(
        refused(&read_doc(&k, k.inner_doc).await).0,
        "not_found",
        "what lies inside the place is not given with it"
    );

    // Its tree holds the outer place and what is beside it, and not the place inside.
    let (listing, _, _) = k
        .world
        .engine()
        .agent_tree(k.reader, Some("/"), 4)
        .await
        .unwrap()
        .expect("the tree of the agent");
    let names: Vec<String> = listing
        .entries
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    assert!(names.iter().any(|name| name == "Knowledge"), "{names:?}");
    assert!(
        names.iter().any(|name| name == "Outer notes.md"),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|name| name.contains("Inner")),
        "the place inside is no part of the tree: {names:?}"
    );
    let listing = ok(call(
        &k.world,
        k.reader,
        OrchTool::ListShared,
        json!({ "path": format!("id:{}", k.outer), "depth": 3 }),
    )
    .await);
    assert!(
        !paths(&listing).iter().any(|path| path.contains("Inner")),
        "{:?}",
        paths(&listing)
    );
}

#[tokio::test]
async fn a_place_shared_with_what_is_in_it_opens_the_documents_inside_as_well() {
    let k = knowledge().await;
    let made = share_by(
        &k,
        k.world.ada,
        &live(
            k.outer,
            None,
            area(None, None, Some(ShareDocumentsLevel::Read)),
            k.reader,
            "c1",
        ),
    )
    .await
    .unwrap();
    assert!(
        made[0].inside,
        "it is the place with what is in it unless the request says otherwise"
    );
    assert_eq!(made[0].id, live_id("node", k.outer, k.reader));
    assert!(entries_of_the_agents(&k, k.outer, "defaults").await >= 1);
    assert!(
        ok(read_doc(&k, k.outer_doc).await)["text"]
            .as_str()
            .unwrap()
            .contains("Outer text.")
    );
    assert!(
        ok(read_doc(&k, k.inner_doc).await)["text"]
            .as_str()
            .unwrap()
            .contains("Inner text."),
        "and what lies inside follows the defaults the share was written on"
    );
}

#[tokio::test]
async fn a_place_has_no_conversation_of_its_own_to_share_by_itself() {
    let k = knowledge().await;
    let refused = share_by(
        &k,
        k.world.ada,
        &live(
            k.outer,
            Some(false),
            area(Some(ShareLevel::Read), None, None),
            k.reader,
            "c1",
        ),
    )
    .await;
    let Err(ShareError::Invalid { field, .. }) = refused else {
        panic!("{refused:?}")
    };
    assert_eq!(field, "parts.timeline");
    assert_eq!(
        entries_of_the_agents(&k, k.outer, "live").await,
        0,
        "nothing was written"
    );
    // Left out, the areas of a place by itself are its chat and its documents, to read.
    let made = share_by(
        &k,
        k.world.ada,
        &live(k.outer, Some(false), None, k.reader, "c2"),
    )
    .await
    .unwrap();
    assert_eq!(made[0].parts.chat, Some(ShareLevel::Read));
    assert_eq!(made[0].parts.documents, Some(ShareDocumentsLevel::Read));
    assert_eq!(made[0].parts.timeline, None);
}

#[tokio::test]
async fn both_ways_can_stand_for_the_same_place_and_are_changed_and_taken_back_each_by_itself() {
    let k = knowledge().await;
    let alone = share_by(
        &k,
        k.world.ada,
        &live(
            k.outer,
            Some(false),
            area(
                None,
                Some(ShareLevel::Read),
                Some(ShareDocumentsLevel::Read),
            ),
            k.reader,
            "c1",
        ),
    )
    .await
    .unwrap()
    .remove(0);
    let inside = share_by(
        &k,
        k.world.ada,
        &live(
            k.outer,
            Some(true),
            area(Some(ShareLevel::Read), None, None),
            k.reader,
            "c2",
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        inside.len(),
        1,
        "the answer is the share that was asked for"
    );
    assert!(inside[0].inside);
    // The place and the agents are the same pair; the two are told apart, each by its own identifier.
    let listed = shares_of(&k, k.outer).await;
    assert_eq!(listed.len(), 2, "{listed:?}");
    let with_what_is_in_it = listed.iter().find(|record| record.inside).unwrap();
    let by_itself = listed.iter().find(|record| !record.inside).unwrap();
    assert_eq!(with_what_is_in_it.id, live_id("node", k.outer, k.reader));
    assert_eq!(by_itself.id, alone.id);
    assert_ne!(with_what_is_in_it.id, by_itself.id);
    assert_eq!(with_what_is_in_it.parts.timeline, Some(ShareLevel::Read));
    assert_eq!(with_what_is_in_it.parts.chat, None);
    assert_eq!(by_itself.parts.chat, Some(ShareLevel::Read));
    assert_eq!(by_itself.parts.documents, Some(ShareDocumentsLevel::Read));
    // Changing the one by itself takes its documents away, and leaves the other as it was.
    let changed = k
        .world
        .engine()
        .update_share(
            &Caller::session(k.world.ada),
            &Origin::default(),
            by_itself.id,
            &ShareAgentUpdateBody {
                read_first: None,
                parts: area(None, Some(ShareLevel::Read), None),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(changed.parts.documents, None);
    assert_eq!(changed.parts.chat, Some(ShareLevel::Read));
    assert!(!changed.inside);
    assert_eq!(refused(&read_doc(&k, k.outer_doc).await).0, "not_found");
    let listed = shares_of(&k, k.outer).await;
    assert_eq!(listed.len(), 2);
    assert_eq!(
        listed
            .iter()
            .find(|record| record.inside)
            .and_then(|record| record.parts.timeline),
        Some(ShareLevel::Read)
    );
    // Taking it back ends it, and the other stands.
    k.world
        .engine()
        .revoke_share(
            &Caller::session(k.world.ada),
            &Origin::default(),
            by_itself.id,
        )
        .await
        .unwrap();
    let listed = shares_of(&k, k.outer).await;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].inside);
    assert!(
        entries_of_the_agents(&k, k.outer, "defaults").await >= 1,
        "the entries inside stand"
    );
}

#[tokio::test]
async fn completely_overlapping_reaches_keep_their_own_lifetimes_and_retractions_in_both_orders() {
    for create_inside_first in [false, true] {
        for remove_inside_first in [false, true] {
            let k = knowledge().await;
            let parts = area(None, None, Some(ShareDocumentsLevel::Read));
            for (number, inside) in [create_inside_first, !create_inside_first]
                .into_iter()
                .enumerate()
            {
                let made = share_by(
                    &k,
                    k.world.ada,
                    &live(
                        k.outer,
                        Some(inside),
                        parts,
                        k.reader,
                        &format!("overlap-{number}"),
                    ),
                )
                .await
                .unwrap();
                assert_eq!(made.len(), 1);
                assert_eq!(
                    made[0].inside, inside,
                    "a covering share cannot replace the requested reach"
                );
            }
            let both = shares_of(&k, k.outer).await;
            assert_eq!(both.len(), 2, "both reaches have their own stored entries");
            let inside = both.iter().find(|share| share.inside).unwrap();
            let alone = both.iter().find(|share| !share.inside).unwrap();
            assert!(
                inside
                    .grants
                    .iter()
                    .all(|entry| entry.share_inside == Some(true))
            );
            assert!(
                alone
                    .grants
                    .iter()
                    .all(|entry| entry.share_inside == Some(false))
            );
            assert!(inside.grants.iter().all(|entry| {
                alone
                    .grants
                    .iter()
                    .all(|other| entry.entry_id != other.entry_id)
            }));
            assert!(
                ok(read_doc(&k, k.outer_doc).await)["text"]
                    .as_str()
                    .unwrap()
                    .contains("Outer text.")
            );
            assert!(
                ok(read_doc(&k, k.inner_doc).await)["text"]
                    .as_str()
                    .unwrap()
                    .contains("Inner text.")
            );

            let end = orch_types::Time::new(k.world.clock.now() + chrono::Duration::days(2));
            let first = if remove_inside_first { inside } else { alone };
            let second = if remove_inside_first { alone } else { inside };
            k.world
                .engine()
                .update_share(
                    &Caller::session(k.world.ada),
                    &Origin::default(),
                    first.id,
                    &ShareAgentUpdateBody {
                        read_first: None,
                        parts: None,
                        expires_at: Some(end),
                    },
                )
                .await
                .unwrap();
            let changed = shares_of(&k, k.outer).await;
            assert!(
                changed
                    .iter()
                    .find(|share| share.id == first.id)
                    .unwrap()
                    .grants
                    .iter()
                    .all(|entry| entry.expires_at == Some(end.get()))
            );
            assert!(
                changed
                    .iter()
                    .find(|share| share.id == second.id)
                    .unwrap()
                    .grants
                    .iter()
                    .all(|entry| entry.expires_at.is_none())
            );

            k.world
                .engine()
                .revoke_share(&Caller::session(k.world.ada), &Origin::default(), first.id)
                .await
                .unwrap();
            let kept = shares_of(&k, k.outer).await;
            assert_eq!(kept.len(), 1);
            assert_eq!(kept[0].id, second.id);
            assert!(
                ok(read_doc(&k, k.outer_doc).await)["text"]
                    .as_str()
                    .unwrap()
                    .contains("Outer text.")
            );
            if remove_inside_first {
                assert_eq!(refused(&read_doc(&k, k.inner_doc).await).0, "not_found");
            } else {
                assert!(
                    ok(read_doc(&k, k.inner_doc).await)["text"]
                        .as_str()
                        .unwrap()
                        .contains("Inner text.")
                );
            }
            k.world
                .engine()
                .revoke_share(&Caller::session(k.world.ada), &Origin::default(), second.id)
                .await
                .unwrap();
            assert!(shares_of(&k, k.outer).await.is_empty());
            assert_eq!(refused(&read_doc(&k, k.outer_doc).await).0, "not_found");
            assert_eq!(refused(&read_doc(&k, k.inner_doc).await).0, "not_found");
        }
    }
}

#[tokio::test]
async fn a_sharer_who_holds_a_place_by_a_role_shares_what_they_hold_and_no_more() {
    let k = knowledge().await;
    let sam = k.world.person("sam").await;
    {
        // Sam gives input to the agent's thread (an Interactor of its topic) and reads the outer place.
        k.world.give(sam, "interactor").await;
        let mut conn = k.world.pool.acquire().await.unwrap();
        access_testing::give_role(&mut conn, k.outer, "user", sam, "viewer", k.world.ada, None)
            .await
            .unwrap();
    }
    let reading = share_by(
        &k,
        sam,
        &live(
            k.outer,
            None,
            area(
                Some(ShareLevel::Read),
                Some(ShareLevel::Read),
                Some(ShareDocumentsLevel::Read),
            ),
            k.reader,
            "s1",
        ),
    )
    .await
    .unwrap();
    assert_eq!(reading[0].by, Some(sam), "every share says who gave it");
    assert!(
        reading[0]
            .grants
            .iter()
            .all(|grant| grant.created_by == Some(sam))
    );
    // They read; they do not write there, and so give the agents no writing.
    let writing = share_by(
        &k,
        sam,
        &live(
            k.outer,
            None,
            area(None, None, Some(ShareDocumentsLevel::Edit)),
            k.reader,
            "s2",
        ),
    )
    .await;
    let Err(ShareError::Denied(refusal)) = writing else {
        panic!("{writing:?}")
    };
    let lacking: Vec<_> = refusal
        .missing
        .iter()
        .flat_map(|entry| entry.permissions.iter().copied())
        .collect();
    assert!(
        lacking.contains(&orch_access_contract::permissions::Permission::DocEdit),
        "{lacking:?}"
    );
    // The place keeps sharing to its owners: Sam is refused what they could share, and cannot take back what they gave.
    k.world
        .set(k.world.root, "sharing.allow_holders", json!(false))
        .await;
    let refused_now = share_by(
        &k,
        sam,
        &live(
            k.outer,
            None,
            area(None, None, Some(ShareDocumentsLevel::Read)),
            k.reader,
            "s3",
        ),
    )
    .await;
    assert!(
        matches!(refused_now, Err(ShareError::Denied(_))),
        "{refused_now:?}"
    );
    let take = k
        .world
        .engine()
        .revoke_share(&Caller::session(sam), &Origin::default(), reading[0].id)
        .await;
    assert!(
        matches!(take, Err(ShareError::Page(_))),
        "a sharer takes back for as long as they could give it again: {take:?}"
    );
    // The owner shares and takes back all the same.
    let by_the_owner = share_by(
        &k,
        k.world.ada,
        &live(
            k.outer,
            Some(false),
            area(None, Some(ShareLevel::Write), None),
            k.reader,
            "a1",
        ),
    )
    .await
    .unwrap();
    assert_eq!(by_the_owner[0].by, Some(k.world.ada));
    // Where holders share again, Sam takes back their own and not the owner's.
    k.world
        .set(k.world.root, "sharing.allow_holders", json!(true))
        .await;
    let theirs = k
        .world
        .engine()
        .revoke_share(
            &Caller::session(sam),
            &Origin::default(),
            by_the_owner[0].id,
        )
        .await;
    assert!(
        matches!(theirs, Err(ShareError::Page(_))),
        "what somebody else gave stays: {theirs:?}"
    );
    k.world
        .engine()
        .revoke_share(&Caller::session(sam), &Origin::default(), reading[0].id)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_conversations_inside_a_place_are_given_in_the_forms_the_sharer_holds_them() {
    let k = knowledge().await;
    let sam = k.world.person("sam").await;
    k.world.give(sam, "interactor").await;
    {
        let mut conn = k.world.pool.acquire().await.unwrap();
        access_testing::give_role(
            &mut conn,
            k.outer,
            "user",
            sam,
            "interactor",
            k.world.ada,
            None,
        )
        .await
        .unwrap();
    }
    let made = share_by(
        &k,
        sam,
        &live(
            k.outer,
            None,
            area(Some(ShareLevel::Write), None, None),
            k.reader,
            "s1",
        ),
    )
    .await
    .unwrap();
    assert_eq!(made[0].parts.timeline, Some(ShareLevel::Write));
    let count = |permission: &'static str, profile: &'static str| {
        let (world, outer) = (&k.world, k.outer);
        async move {
            world
                .count(
                    &format!(
                        "SELECT count(*) FROM permission_entries WHERE object_id = $1 AND page = 'defaults'
                          AND permission = '{permission}' AND profile = '{profile}'
                          AND holder_kind = 'agents' AND removed_at IS NULL"
                    ),
                    outer,
                )
                .await
        }
    };
    assert_eq!(count("thread.view", "managed").await, 1);
    assert_eq!(
        count("thread.view", "connected").await,
        1,
        "an Interactor reads on a person's machine too"
    );
    assert_eq!(count("thread.interact", "managed").await, 1);
    assert_eq!(
        count("thread.interact", "connected").await,
        0,
        "and holds no right to drive a thread there, which the agents are not given"
    );
}
