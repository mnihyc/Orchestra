//! Content search uses the inventory's inherited decisions, with opt-in relevance and bounded pages.
//!
//! Best matches rank by the field that matched (a name or a title, a description, a heading, the rest), then by
//! how often the words occur there, then the newest first, for words in any script.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod chat_world;
mod desk;
mod places;
mod search_world;
mod support;

use desk::{Desk, paragraph};
use orch_access_contract::permissions::{Permission, PermissionName};
use orch_api_contract::{
    common::{ErrorCode, Page},
    documents::{DocumentUpdate, DocumentUpdateBody, NodeDocumentCreateBody},
    search::{SearchFound, SearchHit, SearchList, SearchListQuery, SearchSort},
};
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::queries::access::{PageKey, testing as access};
use orch_types::{Id, Time, new_id};
use places::Who;
use std::collections::HashSet;

fn query(q: &str, sort: SearchSort) -> SearchListQuery {
    serde_json::from_value(serde_json::json!({"q":q,"sort":sort,"limit":1})).unwrap()
}

async fn page(desk: &Desk, by: &Who, query: &SearchListQuery) -> Page<SearchHit> {
    desk.world
        .app
        .call(SearchList, &[])
        .token(&by.token)
        .query(query)
        .send()
        .await
        .ok()
}

fn identity(hit: &SearchHit) -> (String, Id, Option<Id>) {
    match &hit.found {
        SearchFound::Name { node, .. } => ("name".into(), node.id, None),
        SearchFound::Item { item, .. } => ("item".into(), item.id, None),
        SearchFound::ChatMessage { message, .. } => ("chat_message".into(), message.id, None),
        SearchFound::Document {
            document, block_id, ..
        } => ("document".into(), document.id, *block_id),
    }
}

#[tokio::test]
async fn retained_history_selection_does_not_reveal_whether_an_unreadable_thread_exists() {
    let desk = Desk::new().await;
    let stranger = desk.stranger("stranger").await;
    let (thread, _) =
        search_world::thread(&desk, &desk.ada, desk.topic, "Private conversation").await;
    for (continuation, revision) in [(Some(new_id()), None), (None, Some(1))] {
        let make_query = |scope| SearchListQuery {
            scope: Some(scope),
            continuation_id: continuation,
            projection_revision: revision,
            ..query("needle", SearchSort::Newest)
        };
        let hidden = desk
            .world
            .app
            .call(SearchList, &[])
            .token(&stranger.token)
            .query(&make_query(thread))
            .send()
            .await
            .problem();
        let missing = desk
            .world
            .app
            .call(SearchList, &[])
            .token(&stranger.token)
            .query(&make_query(new_id()))
            .send()
            .await
            .problem();
        assert_eq!(hidden.code, ErrorCode::NotFound);
        assert_eq!(hidden.code, missing.code);
        assert_eq!(hidden.status, missing.status);
        assert_eq!(hidden.detail, missing.detail);
        assert_eq!(hidden.extra, missing.extra);
        let wrong_kind = desk
            .world
            .app
            .call(SearchList, &[])
            .token(&desk.ada.token)
            .query(&make_query(desk.topic))
            .send()
            .await
            .problem();
        assert_eq!(wrong_kind.code, ErrorCode::RequestInvalid);
    }
    let visible = SearchListQuery {
        scope: Some(thread),
        ..query("needle", SearchSort::Newest)
    };
    let _ = page(&desk, &desk.ada, &visible).await;
}

#[tokio::test]
async fn relevance_pages_put_names_and_titles_before_newer_bodies_newest_first_and_bind_the_order()
{
    let desk = Desk::new().await;
    let named = places::make_topic(&desk.world, &desk.ada, desk.project, "Needle release")
        .await
        .base
        .id;
    desk.world.advance(chrono::Duration::minutes(1));
    let titled = desk.make(&desk.ada, "Needle release guide").await;
    desk.world.advance(chrono::Duration::minutes(1));
    let body = desk
        .make_body(
            &desk.ada,
            NodeDocumentCreateBody {
                title: "Notes".into(),
                blocks: Some(vec![paragraph("needle release in a newer block")]),
                markdown: None,
                access: None,
            },
        )
        .await
        .ok();
    let (thread, segment) =
        search_world::thread(&desk, &desk.ada, desk.topic, "Conversation").await;
    desk.world.advance(chrono::Duration::minutes(1));
    let newest = search_world::say(
        &desk,
        &desk.ada,
        thread,
        segment,
        "needle release in the newest message",
        desk.world.now(),
    )
    .await;
    let chronological = page(
        &desk,
        &desk.ada,
        &query("needle release", SearchSort::Newest),
    )
    .await;
    assert!(matches!(&chronological.items[0].found,SearchFound::Item {item,..} if item.id==newest));

    let initial = query("needle release", SearchSort::Relevance);
    let first = page(&desk, &desk.ada, &initial).await;
    let cursor = first.next_cursor.clone().unwrap();
    // A name and a title are one field, with the words as often in each: the newer of the two comes first.
    assert!(
        matches!(&first.items[0].found,SearchFound::Document {document,block_id:None,..} if document.id==titled.base.id)
    );
    let mut ask = initial.clone();
    let mut all = Vec::new();
    let mut finished = false;
    // Four matches; the finite guard makes a broken continuation fail instead of waiting forever.
    for _ in 0..6 {
        let next = page(&desk, &desk.ada, &ask).await;
        all.extend(next.items);
        if let Some(cursor) = next.next_cursor {
            ask.cursor = Some(cursor);
        } else {
            finished = true;
            break;
        }
    }
    assert!(finished);
    assert_eq!(all.len(), 4);
    assert_eq!(all.iter().map(identity).collect::<HashSet<_>>().len(), 4);
    assert!(
        matches!(&all[1].found,SearchFound::Name {node,href,..} if node.id==named && href==&format!("/topic/{named}"))
    );
    assert!(
        matches!(&all[2].found,SearchFound::Item {item,..} if item.id==newest),
        "two bodies with the words as often: the newer first"
    );
    let first_two: HashSet<_> = all[..2]
        .iter()
        .map(|hit| match &hit.found {
            SearchFound::Name { node, .. } => node.id,
            SearchFound::Document {
                document,
                block_id: None,
                ..
            } => document.id,
            _ => panic!("a name/title must precede any body"),
        })
        .collect();
    assert_eq!(first_two, HashSet::from([named, titled.base.id]));
    assert!(all.iter().any(|hit|matches!(&hit.found,SearchFound::Document {document,block_id:Some(_),..} if document.id==body.base.id)));
    let unordered = page(
        &desk,
        &desk.ada,
        &SearchListQuery {
            limit: Some(20),
            ..query("release needle", SearchSort::Relevance)
        },
    )
    .await;
    assert_eq!(
        unordered.items.iter().map(identity).collect::<HashSet<_>>(),
        all.iter().map(identity).collect::<HashSet<_>>()
    );
    for hit in &all {
        let characters: Vec<_> = hit.snippet.chars().collect();
        let marked: HashSet<String> = hit
            .matches
            .iter()
            .map(|(from, to)| {
                characters[*from as usize..*to as usize]
                    .iter()
                    .collect::<String>()
                    .to_lowercase()
            })
            .collect();
        assert_eq!(marked, HashSet::from(["needle".into(), "release".into()]));
    }
    assert!(
        page(
            &desk,
            &desk.ada,
            &query("\"release needle\"", SearchSort::Relevance)
        )
        .await
        .items
        .is_empty()
    );
    for changed in [
        SearchListQuery {
            sort: Some(SearchSort::Newest),
            cursor: Some(cursor.clone()),
            ..initial.clone()
        },
        SearchListQuery {
            q: "another query".into(),
            cursor: Some(cursor.clone()),
            ..initial.clone()
        },
    ] {
        let refused = desk
            .world
            .app
            .call(SearchList, &[])
            .token(&desk.ada.token)
            .query(&changed)
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
    }
}

/// A document with a title, an optional description, and blocks written as Markdown.
async fn written(desk: &Desk, title: &str, description: &str, markdown: &str) -> Id {
    let made = desk
        .make_body(
            &desk.ada,
            NodeDocumentCreateBody {
                title: title.into(),
                blocks: None,
                markdown: Some(markdown.into()),
                access: None,
            },
        )
        .await
        .ok();
    if !description.is_empty() {
        desk.world
            .app
            .call(DocumentUpdate, &[&made.base.id])
            .token(&desk.ada.token)
            .body(&DocumentUpdateBody {
                title: None,
                description: Some(description.into()),
            })
            .send()
            .await
            .ok();
    }
    desk.world.advance(chrono::Duration::minutes(1));
    made.base.id
}

fn document_of(hit: &SearchHit) -> Id {
    match &hit.found {
        SearchFound::Document { document, .. } => document.id,
        other => panic!("only documents were asked for: {other:?}"),
    }
}

#[tokio::test]
async fn best_matches_rank_title_description_heading_and_body_then_how_often_then_newest_in_any_script()
 {
    let desk = Desk::new().await;
    // Made oldest first, so that recency alone would give the reverse order.
    for (words, filler) in [("发布流程", "说明"), ("rollout plan", "notes")] {
        let title = written(&desk, &format!("{words} 一"), "", filler).await;
        let described = written(
            &desk,
            &format!("{filler} 二"),
            &format!("关于 {words} 的说明"),
            filler,
        )
        .await;
        let heading = written(
            &desk,
            &format!("{filler} 三"),
            "",
            &format!("# {words}\n\n{filler}"),
        )
        .await;
        let often = written(
            &desk,
            &format!("{filler} 四"),
            "",
            &format!("{words}，{words}，{words}"),
        )
        .await;
        let once = written(&desk, &format!("{filler} 五"), "", words).await;
        let ranked = page(
            &desk,
            &desk.ada,
            &SearchListQuery {
                kinds: Some("document".into()),
                limit: Some(20),
                ..query(words, SearchSort::Relevance)
            },
        )
        .await;
        let order: Vec<Id> = ranked.items.iter().map(document_of).collect();
        assert_eq!(
            order,
            vec![title, described, heading, often, once],
            "{words}: title, description, heading, then the body where the words occur most, then the newest"
        );
        // One page at a time keeps the same order and repeats nothing.
        let mut ask = SearchListQuery {
            kinds: Some("document".into()),
            ..query(words, SearchSort::Relevance)
        };
        let mut paged = Vec::new();
        for _ in 0..7 {
            let next = page(&desk, &desk.ada, &ask).await;
            paged.extend(next.items.iter().map(document_of));
            match next.next_cursor {
                Some(cursor) => ask.cursor = Some(cursor),
                None => break,
            }
        }
        assert_eq!(paged, order, "{words}: pages follow the ranking");
        let newest = page(
            &desk,
            &desk.ada,
            &SearchListQuery {
                kinds: Some("document".into()),
                limit: Some(20),
                ..query(words, SearchSort::Newest)
            },
        )
        .await;
        assert_eq!(
            newest.items.iter().map(document_of).next(),
            Some(once),
            "{words}: newest first is unchanged"
        );
    }
}

#[tokio::test]
async fn sql_search_keeps_live_custom_owner_locked_and_participation_decisions_equal_to_the_access_engine()
 {
    let desk = Desk::new().await;
    let reader = desk.member("reader", "viewer").await;
    let ordinary = search_world::thread(&desk, &desk.ada, desk.topic, "Projection ordinary").await;
    let owned = search_world::thread(&desk, &reader, desk.topic, "Projection owned").await;
    let private = search_world::thread(&desk, &desk.ada, desk.topic, "Projection private").await;
    search_world::cut_view(&desk, private.0).await;
    for (index, (thread, segment)) in [ordinary, owned, private].into_iter().enumerate() {
        search_world::say(
            &desk,
            &desk.ada,
            thread,
            segment,
            "projection needle",
            desk.world.now() + chrono::Duration::seconds(index as i64),
        )
        .await;
    }
    let ids = [ordinary.0, owned.0, private.0];
    for locked in [false, true] {
        if locked {
            let mut conn = desk.world.conn().await;
            access::set_row(
                &mut conn,
                &PageKey::new("node", desk.topic, "defaults"),
                "managed",
                "thread.view",
                false,
                true,
            )
            .await
            .unwrap();
        }
        for focus in [false, true] {
            let mut conn = desk.world.conn().await;
            let principal = Principal::load(
                &mut conn,
                Caller::session(reader.id()),
                Time::new(desk.world.now()),
            )
            .await
            .unwrap();
            let expected = orch_authz::nodes::holding(
                &mut conn,
                &principal,
                &ids,
                PermissionName::Exact(Permission::ThreadView),
                Act::Use,
                Policy::default(),
            )
            .await
            .unwrap()
            .into_iter()
            .filter(|held| !focus || held.participating)
            .map(|held| held.id)
            .collect::<HashSet<_>>();
            drop(conn);
            for sort in [SearchSort::Newest, SearchSort::Relevance] {
                let mut ask = SearchListQuery {
                    kinds: Some("user_message".into()),
                    participating: Some(focus),
                    ..query("projection needle", sort)
                };
                let mut actual = HashSet::new();
                let mut finished = false;
                for _ in 0..4 {
                    let result = page(&desk, &reader, &ask).await;
                    for hit in result.items {
                        match hit.found {
                            SearchFound::Item { thread, .. } => assert!(actual.insert(thread.id)),
                            _ => panic!("only the requested item kind"),
                        }
                    }
                    if let Some(cursor) = result.next_cursor {
                        ask.cursor = Some(cursor);
                    } else {
                        finished = true;
                        break;
                    }
                }
                assert!(finished);
                assert_eq!(
                    actual, expected,
                    "locked={locked}, focus={focus}, sort={sort:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_unscoped_content_page_and_a_name_page_do_not_expand_twenty_thousand_ordinary_thread_decisions()
 {
    let desk = Desk::new().await;
    let reader = desk.member("reader", "viewer").await;
    let (thread, segment) =
        search_world::thread(&desk, &desk.ada, desk.topic, "A conversation").await;
    let item = search_world::say(
        &desk,
        &desk.ada,
        thread,
        segment,
        "scale needle",
        desk.world.now(),
    )
    .await;
    // More than the retired whole-tree cap, all sharing one topic's existing decisions.
    let ids: Vec<_> = (0..20_001).map(|_| new_id()).collect();
    let mut conn = desk.world.conn().await;
    sqlx::query(
        "INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by)
        SELECT x.id,'thread',p.id,p.org_id,p.path||x.id,'Scale ordinary '||x.ord,$3,$3
          FROM unnest($1::uuid[]) WITH ORDINALITY x(id,ord) CROSS JOIN nodes p WHERE p.id=$2",
    )
    .bind(&ids)
    .bind(desk.topic)
    .bind(desk.ada.id())
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("INSERT INTO threads(node_id,topic_id) SELECT id,$2 FROM unnest($1::uuid[]) x(id)")
        .bind(&ids)
        .bind(desk.topic)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let body = page(
        &desk,
        &reader,
        &SearchListQuery {
            kinds: Some("user_message".into()),
            ..query("scale needle", SearchSort::Newest)
        },
    )
    .await;
    assert_eq!(body.items.len(), 1);
    assert!(matches!(&body.items[0].found,SearchFound::Item {item:found,..} if found.id==item));
    assert!(body.next_cursor.is_none());
    let names = page(
        &desk,
        &reader,
        &query("scale ordinary", SearchSort::Relevance),
    )
    .await;
    assert_eq!(names.items.len(), 1);
    assert!(matches!(&names.items[0].found,SearchFound::Name {node,..} if ids.contains(&node.id)));
    assert!(names.next_cursor.is_some());
    let mut conn = desk.world.conn().await;
    let principal = Principal::load(
        &mut conn,
        Caller::session(reader.id()),
        Time::new(desk.world.now()),
    )
    .await
    .unwrap();
    let projection = orch_authz::everything::project(&mut conn, &principal, None)
        .await
        .unwrap();
    let objects = projection.grants["objects"].as_object().unwrap();
    assert!(
        ids.iter().all(|id| !objects.contains_key(&id.to_string())),
        "ordinary threads remain inherited decisions, not an installation-sized allow list"
    );
}
