//! Content search over HTTP: literal matches, access before matching, current content, filters, stable pages and scale.
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

use chat_world::{Room, text};
use desk::{Desk, paragraph, put};
use orch_api_contract::{
    chats::{ChatMessageDelete, ChatMessageUpdate, ChatMessageUpdateBody},
    common::{ErrorCode, Page},
    documents::{
        DocumentDelete, DocumentOp, DocumentUpdate, DocumentUpdateBody, NewDocumentAccess,
        NodeDocumentCreateBody,
    },
    nodes::{ProjectChildCreate, ProjectCreateBody},
    search::{
        NameSearchList, NameSearchListQuery, SearchFound, SearchHit, SearchList, SearchListQuery,
    },
};
use orch_types::{Id, Time, new_id};
use places::Who;
use search_world::{cut_view, say, thread};
use std::collections::HashSet;

fn query(text: &str) -> SearchListQuery {
    SearchListQuery {
        continuation_id: None,
        projection_revision: None,
        sort: None,
        participating: None,
        with_file: None,
        mentions_me: None,
        chat_id: None,
        document_id: None,
        q: text.to_owned(),
        scope: None,
        kinds: None,
        author_id: None,
        from: None,
        to: None,
        limit: None,
        cursor: None,
        regex: None,
        immediate: None,
        typing_age_ms: None,
        ..SearchListQuery::default()
    }
}

async fn find(desk: &Desk, by: &Who, query: &SearchListQuery) -> Page<SearchHit> {
    desk.world
        .app
        .call(SearchList, &[])
        .token(&by.token)
        .query(query)
        .send()
        .await
        .ok()
}

fn key(hit: &SearchHit) -> (String, Id, Option<Id>) {
    match &hit.found {
        SearchFound::Name { node, .. } => ("name".to_owned(), node.id, None),
        SearchFound::Item { item, .. } => ("item".to_owned(), item.id, None),
        SearchFound::ChatMessage { message, .. } => ("chat_message".to_owned(), message.id, None),
        SearchFound::Document {
            document, block_id, ..
        } => ("document".to_owned(), document.id, *block_id),
    }
}

#[tokio::test]
async fn injected_context_never_matches_and_old_echoes_are_repaired_without_losing_the_actual_answer()
 {
    use orch_engine::{
        ingest::{ItemSpec, add_item},
        unit::Unit,
    };
    let desk = Desk::new().await;
    let (thread, segment) = thread(&desk, &desk.ada, desk.topic, "A normal conversation").await;
    let hidden = "<orch>\nWhere you are\nhiddencontextneedle in a different thread\n</orch>";
    let mut ids = Vec::new();
    {
        let mut tx = desk.world.app.app.db.begin().await.unwrap();
        let mut unit = Unit::new(&mut tx, desk.world.now());
        for text in [
            hidden.to_owned(),
            format!("The visible answer.\n{hidden}\nActual conclusion."),
            "Progress <orch>hiddencontextneedle still streaming".to_owned(),
        ] {
            let payload =
                serde_json::json!({"text":text,"text_len":text.chars().count(),"phase":"final"});
            let item = add_item(
                &mut unit,
                &ItemSpec {
                    thread,
                    segment,
                    turn: None,
                    stream: "main",
                    parent: None,
                    kind: "agent_message",
                    status: "completed",
                    author: None,
                    payload: &payload,
                    native_ref: None,
                },
            )
            .await
            .unwrap()
            .unwrap();
            assert!(item.search_text.as_deref().is_none_or(|text| {
                !text.contains("hiddencontextneedle") && !text.contains("<orch>")
            }));
            assert_eq!(item.payload, payload, "diagnostic source remains intact");
            ids.push(item.id);
        }
        unit.finish().await.unwrap();
        tx.commit().await.unwrap();
    }
    assert!(
        find(&desk, &desk.ada, &query("hiddencontextneedle"))
            .await
            .items
            .is_empty()
    );
    let visible = find(&desk, &desk.ada, &query("visible answer")).await;
    assert_eq!(visible.items.len(), 1);
    assert!(!visible.items[0].snippet.contains("hiddencontextneedle"));
    // A saved world can predate the new writer; its stale index is refused before repair too.
    {
        let mut conn = desk.world.conn().await;
        sqlx::query("UPDATE items SET search_text=payload->>'text' WHERE id=ANY($1)")
            .bind(&ids)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    assert!(
        find(&desk, &desk.ada, &query("hiddencontextneedle"))
            .await
            .items
            .is_empty()
    );
    assert_eq!(
        desk.world
            .app
            .app
            .engine
            .repair_context_search()
            .await
            .unwrap(),
        3
    );
    let restored = find(&desk, &desk.ada, &query("visible answer")).await;
    assert_eq!(restored.items.len(), 1);
    assert!(matches!(&restored.items[0].found,SearchFound::Item {item,..} if item.id==ids[1]));
    assert!(!restored.items[0].snippet.contains("<orch>"));
    assert!(
        find(&desk, &desk.ada, &query("hiddencontextneedle"))
            .await
            .items
            .is_empty()
    );
    assert_eq!(
        desk.world
            .app
            .app
            .engine
            .repair_context_search()
            .await
            .unwrap(),
        0,
        "repair is idempotent"
    );
}

#[tokio::test]
async fn content_uses_the_name_query_grammar_and_highlights_each_unordered_term() {
    let desk = Desk::new().await;
    let document = desk
        .make_body(
            &desk.ada,
            NodeDocumentCreateBody {
                title: "Relevant notes".into(),
                blocks: Some(vec![paragraph("Fix redirect after login")]),
                markdown: None,
                access: None,
            },
        )
        .await
        .ok();
    let ask = SearchListQuery {
        document_id: Some(document.base.id),
        ..query("login redirect")
    };
    let found = find(&desk, &desk.ada, &ask).await;
    assert_eq!(found.items.len(), 1);
    let hit = &found.items[0];
    let chars: Vec<char> = hit.snippet.chars().collect();
    let marked: Vec<String> = hit
        .matches
        .iter()
        .map(|(from, to)| chars[*from as usize..*to as usize].iter().collect())
        .collect();
    assert_eq!(marked, ["redirect", "login"]);
    assert!(
        find(
            &desk,
            &desk.ada,
            &SearchListQuery {
                q: "\"login redirect\"".into(),
                ..ask.clone()
            }
        )
        .await
        .items
        .is_empty()
    );
    assert_eq!(
        find(
            &desk,
            &desk.ada,
            &SearchListQuery {
                q: "login \"redirect after\"".into(),
                ..ask
            }
        )
        .await
        .items
        .len(),
        1
    );
}

#[tokio::test]
async fn finding_inside_one_document_pages_only_its_current_readable_words_even_with_two_letters() {
    let desk = Desk::new().await;
    let bea = desk.member("bea", "viewer").await;
    let chosen = desk
        .make_body(
            &desk.ada,
            NodeDocumentCreateBody {
                title: "Release selected".into(),
                blocks: Some(vec![
                    paragraph("release first"),
                    paragraph("release second"),
                ]),
                markdown: None,
                access: Some(NewDocumentAccess::Private),
            },
        )
        .await
        .ok();
    desk.make_body(
        &desk.ada,
        NodeDocumentCreateBody {
            title: "Release outside".into(),
            blocks: Some(vec![paragraph("release outside this document")]),
            markdown: None,
            access: None,
        },
    )
    .await
    .ok();
    let ask = SearchListQuery {
        document_id: Some(chosen.base.id),
        limit: Some(1),
        ..query("re")
    };
    let mut cursor = None;
    let mut seen = HashSet::new();
    loop {
        let page = find(
            &desk,
            &desk.ada,
            &SearchListQuery {
                cursor: cursor.clone(),
                ..ask.clone()
            },
        )
        .await;
        assert_eq!(page.items.len(), 1);
        for hit in &page.items {
            assert!(
                matches!(&hit.found, SearchFound::Document { document, .. } if document.id == chosen.base.id)
            );
            assert!(
                seen.insert(key(hit)),
                "stepping to the next page never repeats a match"
            );
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        seen.len(),
        3,
        "the title and each matching block belong to this document alone"
    );
    let hidden = find(
        &desk,
        &bea,
        &SearchListQuery {
            q: "release".into(),
            ..ask.clone()
        },
    )
    .await;
    assert!(hidden.items.is_empty() && hidden.next_cursor.is_none());
    let refused = desk
        .world
        .app
        .call(SearchList, &[])
        .token(&bea.token)
        .query(&ask)
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::RequestInvalid,
        "an unreadable id does not grant a short-search scope"
    );
    desk.world
        .app
        .call(DocumentDelete, &[&chosen.base.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let deleted = find(
        &desk,
        &desk.ada,
        &SearchListQuery {
            q: "release".into(),
            ..ask
        },
    )
    .await;
    assert!(deleted.items.is_empty() && deleted.next_cursor.is_none());
}

#[tokio::test]
async fn finding_inside_a_direct_chat_searches_only_that_current_membership_and_never_deleted_words()
 {
    let room = Room::new().await;
    let chosen = room.direct(&room.ada, &room.bea).await.base.id;
    let other = room.direct(&room.ada, &room.cai).await.base.id;
    let first = room.say(&room.ada, chosen, "hi selected one").await;
    let second = room.say(&room.bea, chosen, "hi selected two").await;
    room.say(&room.ada, other, "hi outside").await;
    room.say(&room.ada, room.chat, "hi in the place instead")
        .await;
    let ask = SearchListQuery {
        chat_id: Some(chosen),
        ..query("hi")
    };
    let page = room
        .world
        .app
        .call(SearchList, &[])
        .token(&room.bea.token)
        .query(&ask)
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 2);
    assert!(page.items.iter().all(|hit| matches!(&hit.found, SearchFound::ChatMessage { chat, message, .. } if chat.id == chosen && [first.id, second.id].contains(&message.id))));
    let hidden = room
        .world
        .app
        .call(SearchList, &[])
        .token(&room.cai.token)
        .query(&SearchListQuery {
            q: "selected".into(),
            ..ask.clone()
        })
        .send()
        .await
        .ok();
    assert!(hidden.items.is_empty() && hidden.next_cursor.is_none());
    let refused = room
        .world
        .app
        .call(SearchList, &[])
        .token(&room.cai.token)
        .query(&ask)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    room.world
        .app
        .call(ChatMessageDelete, &[&chosen, &first.id])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let remaining = room
        .world
        .app
        .call(SearchList, &[])
        .token(&room.bea.token)
        .query(&ask)
        .send()
        .await
        .ok();
    assert_eq!(remaining.items.len(), 1);
    assert!(
        matches!(&remaining.items[0].found, SearchFound::ChatMessage { message, .. } if message.id == second.id)
    );
}

#[tokio::test]
async fn a_conversation_match_carries_the_item_and_its_nearest_folder_with_unicode_highlights() {
    let desk = Desk::new().await;
    let folder = desk
        .world
        .app
        .call(ProjectChildCreate, &[&desk.project])
        .token(&desk.ada.token)
        .body(&ProjectCreateBody {
            name: "Checkout".to_owned(),
            description: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .base
        .id;
    let topic = places::make_topic(&desk.world, &desk.ada, folder, "Payments")
        .await
        .base
        .id;
    let (thread, segment) = thread(&desk, &desk.ada, topic, "Release thread").await;
    let item = say(
        &desk,
        &desk.ada,
        thread,
        segment,
        "你好 Release 世界 release",
        desk.world.now(),
    )
    .await;
    let hits = find(&desk, &desk.ada, &query("RELEASE")).await;
    assert_eq!(hits.items.len(), 1);
    let hit = &hits.items[0];
    assert_eq!(hit.snippet, "你好 Release 世界 release");
    assert_eq!(hit.matches, [(3, 10), (14, 21)]);
    assert!(
        matches!(&hit.found,SearchFound::Item {item:row,thread:found,topic:place,project,..} if row.id==item && found.id==thread && place.id==topic && project.id==folder)
    );
    let scoped = find(
        &desk,
        &desk.ada,
        &SearchListQuery {
            scope: Some(thread),
            ..query("世界")
        },
    )
    .await;
    assert_eq!(scoped.items.len(), 1);
}

#[tokio::test]
async fn short_unscoped_queries_unknown_kinds_bad_cursors_limits_and_reversed_dates_are_refused() {
    let desk = Desk::new().await;
    let later = desk.world.now() + chrono::Duration::days(1);
    let cases = [
        query("x"),
        query("hi"),
        SearchListQuery {
            scope: Some(desk.project),
            ..query("hi")
        },
        SearchListQuery {
            kinds: Some("raw_call".to_owned()),
            ..query("hello")
        },
        SearchListQuery {
            cursor: Some("garbage".to_owned()),
            ..query("hello")
        },
        SearchListQuery {
            limit: Some(0),
            ..query("hello")
        },
        SearchListQuery {
            limit: Some(201),
            ..query("hello")
        },
        SearchListQuery {
            from: Some(Time::new(later)),
            to: Some(Time::new(desk.world.now())),
            ..query("hello")
        },
    ];
    for query in cases {
        assert_eq!(
            desk.world
                .app
                .call(SearchList, &[])
                .token(&desk.ada.token)
                .query(&query)
                .send()
                .await
                .problem()
                .code,
            ErrorCode::RequestInvalid
        );
    }
    assert!(
        find(
            &desk,
            &desk.ada,
            &SearchListQuery {
                scope: Some(desk.topic),
                ..query("hi")
            }
        )
        .await
        .items
        .is_empty()
    );
    let unsigned = desk
        .world
        .app
        .call(SearchList, &[])
        .query(&query("hello"))
        .send()
        .await;
    assert_eq!(unsigned.status, http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn kinds_authors_and_inclusive_dates_filter_what_was_written() {
    let desk = Desk::new().await;
    let bea = desk.member("bea", "viewer").await;
    let (thread, segment) = thread(&desk, &desk.ada, desk.topic, "Main").await;
    let old = say(
        &desk,
        &desk.ada,
        thread,
        segment,
        "release old",
        desk.world.now(),
    )
    .await;
    let later = desk.world.now() + chrono::Duration::days(1);
    let current = say(&desk, &bea, thread, segment, "release current", later).await;
    let exact = find(
        &desk,
        &desk.ada,
        &SearchListQuery {
            kinds: Some("user_message".to_owned()),
            author_id: Some(bea.id()),
            from: Some(Time::new(later)),
            to: Some(Time::new(later)),
            ..query("release")
        },
    )
    .await;
    assert_eq!(
        exact.items.iter().map(key).collect::<Vec<_>>(),
        [("item".to_owned(), current, None)]
    );
    let first = find(
        &desk,
        &desk.ada,
        &SearchListQuery {
            to: Some(Time::new(desk.world.now())),
            ..query("release")
        },
    )
    .await;
    assert_eq!(
        first.items.iter().map(key).collect::<Vec<_>>(),
        [("item".to_owned(), old, None)]
    );
    assert!(
        find(
            &desk,
            &desk.ada,
            &SearchListQuery {
                kinds: Some("agent_message".to_owned()),
                ..query("release")
            }
        )
        .await
        .items
        .is_empty()
    );
    let no_kinds = SearchListQuery {
        kinds: Some(String::new()),
        ..query("release")
    };
    let empty = find(&desk, &desk.ada, &no_kinds).await;
    assert!(empty.items.is_empty() && empty.next_cursor.is_none());
}

#[tokio::test]
async fn creator_editor_owner_and_edited_dates_narrow_documents_before_paging() {
    use orch_api_contract::search::{SearchDateField, SearchStreamEvent, SearchStreamList};

    let desk = Desk::new().await;
    let bea = desk.member("bea", "viewer").await;
    let selected = desk.make(&desk.ada, "saffron selected").await;
    let other = desk.make(&desk.ada, "saffron other").await;
    let created = desk.world.now() - chrono::Duration::days(3);
    let edited = desk.world.now() - chrono::Duration::days(1);
    sqlx::query(
        "UPDATE documents SET owner_id=$2,created_at=$3,updated_at=$4,updated_by=$2 WHERE id=$1",
    )
    .bind(selected.base.id)
    .bind(bea.id())
    .bind(created)
    .bind(edited)
    .execute(desk.world.app.app.db.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE documents SET updated_at=$2 WHERE id=$1")
        .bind(other.base.id)
        .bind(desk.world.now())
        .execute(desk.world.app.app.db.pool())
        .await
        .unwrap();
    let ask = SearchListQuery {
        kinds: Some("document".into()),
        created_by_id: Some(desk.ada.id()),
        edited_by_id: Some(bea.id()),
        owner_id: Some(bea.id()),
        date_field: Some(SearchDateField::Edited),
        from: Some(Time::new(edited)),
        to: Some(Time::new(edited)),
        limit: Some(1),
        ..query("saffron")
    };
    let page = find(&desk, &desk.ada, &ask).await;
    assert_eq!(page.items.len(), 1);
    assert!(
        matches!(&page.items[0].found, SearchFound::Document { document, .. } if document.id == selected.base.id)
    );
    assert!(page.next_cursor.is_none());
    let created_range = SearchListQuery {
        date_field: Some(SearchDateField::Created),
        ..ask.clone()
    };
    assert!(
        find(&desk, &desk.ada, &created_range)
            .await
            .items
            .is_empty()
    );

    let response = desk
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&desk.ada.token)
        .query(&SearchListQuery {
            immediate: Some(true),
            ..ask
        })
        .send_bytes()
        .await;
    assert_eq!(response.status, 200);
    let hits: Vec<_> = response
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(
            |line| match serde_json::from_slice::<SearchStreamEvent>(line).unwrap() {
                SearchStreamEvent::Hit { hit, .. } => Some(hit),
                _ => None,
            },
        )
        .collect();
    assert_eq!(hits.len(), 1);
    assert!(
        matches!(&hits[0].found, SearchFound::Document { document, .. } if document.id == selected.base.id)
    );
}

#[tokio::test]
async fn shared_filter_keeps_only_currently_readable_shares_before_the_limit() {
    use orch_api_contract::search::{SearchStreamEvent, SearchStreamList};

    let desk = Desk::new().await;
    let bea = desk.member("bea", "viewer").await;
    let private = |title: &str| NodeDocumentCreateBody {
        title: title.into(),
        blocks: None,
        markdown: None,
        access: Some(NewDocumentAccess::Private),
    };
    let shared = desk
        .make_body(&desk.ada, private("tamarind shared"))
        .await
        .ok();
    let _hidden = desk
        .make_body(&desk.ada, private("tamarind hidden"))
        .await
        .ok();
    let mut conn = desk.world.conn().await;
    let grant = orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("document", shared.base.id, "live"),
        "any",
        "doc.view",
        ("user", Some(bea.id()), None),
        None,
        desk.world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let ask = SearchListQuery {
        kinds: Some("document".into()),
        shared_with_me: Some(true),
        limit: Some(1),
        ..query("tamarind")
    };
    let page = find(&desk, &bea, &ask).await;
    assert_eq!(page.items.len(), 1);
    assert!(
        matches!(&page.items[0].found, SearchFound::Document { document, .. } if document.id == shared.base.id)
    );
    assert!(page.next_cursor.is_none());
    let response = desk
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&bea.token)
        .query(&SearchListQuery {
            immediate: Some(true),
            ..ask.clone()
        })
        .send_bytes()
        .await;
    assert_eq!(response.status, 200);
    let hits: Vec<_> = response
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(
            |line| match serde_json::from_slice::<SearchStreamEvent>(line).unwrap() {
                SearchStreamEvent::Hit { hit, .. } => Some(hit),
                _ => None,
            },
        )
        .collect();
    assert_eq!(hits.len(), 1);
    assert!(
        matches!(&hits[0].found, SearchFound::Document { document, .. } if document.id == shared.base.id)
    );
    sqlx::query("UPDATE permission_entries SET removed_at=$2 WHERE id=$1")
        .bind(grant)
        .bind(desk.world.now())
        .execute(desk.world.app.app.db.pool())
        .await
        .unwrap();
    assert!(find(&desk, &bea, &ask).await.items.is_empty());
}

#[tokio::test]
async fn shared_filter_follows_a_group_grant_to_a_documents_folder_without_revealing_its_parents() {
    use orch_api_contract::access::{
        AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate,
    };
    use orch_api_contract::directory::{GroupHolderAdd, OrgGroupCreate, OrgGroupCreateBody};
    use orch_api_contract::document_tree::{
        DocumentContainerCreate, DocumentContainerCreateBody, DocumentContainerDocumentCreate,
        DocumentTreeItem,
    };

    let desk = Desk::new().await;
    let reader = desk.stranger("group-reader").await;
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([42; 32]),
    );
    let group = desk
        .world
        .app
        .call(OrgGroupCreate, &[&desk.org])
        .token(&desk.ada.token)
        .body(&OrgGroupCreateBody {
            name: "Readers".into(),
            description: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        desk.world
            .app
            .call(GroupHolderAdd, &[&group.id])
            .token(&desk.ada.token)
            .body(&HolderInput::User { id: reader.id() })
            .send()
            .await
            .status,
        204
    );
    let created = desk
        .world
        .app
        .call(DocumentContainerCreate, &[&desk.topic])
        .token(&desk.ada.token)
        .body(&DocumentContainerCreateBody {
            parent_container_id: None,
            name: "Shared research".into(),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let Some(DocumentTreeItem::Container(folder)) = created.entries.first().map(|entry| entry.item)
    else {
        panic!("a folder was created")
    };
    let document = desk
        .world
        .app
        .call(DocumentContainerDocumentCreate, &[&folder])
        .token(&desk.ada.token)
        .body(&desk::body("starling handbook"))
        .send()
        .await
        .ok();
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"document-containers", &folder])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::Group { id: group.id }],
                choice: AccessChoice::FullView,
                export: Some(false),
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let ask = SearchListQuery {
        shared_with_me: Some(true),
        kinds: Some("document".into()),
        ..query("starling")
    };
    let found = find(&desk, &reader, &ask).await;
    assert_eq!(found.items.len(), 1);
    assert!(
        matches!(&found.items[0].found, SearchFound::Document { document: hit, .. } if hit.id == document.base.id)
    );
    let text = serde_json::to_string(&found).unwrap();
    for concealed in ["Acme", "Web", "Plans"] {
        assert!(!text.contains(concealed));
    }
}

#[tokio::test]
async fn file_type_selects_the_media_type_of_a_chat_attachment() {
    use orch_api_contract::search::{SearchStreamEvent, SearchStreamList};

    let room = Room::new().await;
    let message = room.say(&room.ada, room.chat, "marigold review").await;
    let hash = orch_types::Hash::of(b"file");
    sqlx::query("INSERT INTO blobs (hash,size,pin_count) VALUES ($1,4,1)")
        .bind(hash.as_bytes().as_slice())
        .execute(room.world.app.app.db.pool())
        .await
        .unwrap();
    let parts = serde_json::json!([
        {"type":"text","text":"marigold review"},
        {"type":"attachment","blob":hash,"name":"review.pdf","size":4,"media_type":"application/pdf"}
    ]);
    sqlx::query(
        "UPDATE chat_messages SET content=$2,search_text='marigold review review.pdf' WHERE id=$1",
    )
    .bind(message.id)
    .bind(parts)
    .execute(room.world.app.app.db.pool())
    .await
    .unwrap();
    let ask = SearchListQuery {
        kinds: Some("chat_message".into()),
        file_type: Some("application/pdf".into()),
        ..query("marigold")
    };
    for (selection, wanted) in [
        (ask.clone(), 1),
        (
            SearchListQuery {
                file_type: Some("image/*".into()),
                ..ask
            },
            0,
        ),
    ] {
        let page = room
            .world
            .app
            .call(SearchList, &[])
            .token(&room.ada.token)
            .query(&selection)
            .send()
            .await
            .ok();
        assert_eq!(page.items.len(), wanted);
        let response = room
            .world
            .app
            .call(SearchStreamList, &[])
            .token(&room.ada.token)
            .query(&SearchListQuery {
                immediate: Some(true),
                ..selection
            })
            .send_bytes()
            .await;
        assert_eq!(response.status, 200);
        let hits = response
            .raw
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .filter(|line| {
                matches!(
                    serde_json::from_slice::<SearchStreamEvent>(line).unwrap(),
                    SearchStreamEvent::Hit { .. }
                )
            })
            .count();
        assert_eq!(hits, wanted);
    }
    let unrelated = room.say(&room.ada, room.chat, "metadataonly").await;
    let content =
        serde_json::json!([{"type":"text","text":"metadataonly","media_type":"image/png"}]);
    sqlx::query("UPDATE chat_messages SET content=$2 WHERE id=$1")
        .bind(unrelated.id)
        .bind(content)
        .execute(room.world.app.app.db.pool())
        .await
        .unwrap();
    let image = SearchListQuery {
        kinds: Some("chat_message".into()),
        file_type: Some("image/*".into()),
        ..query("metadataonly")
    };
    let result = room
        .world
        .app
        .call(SearchList, &[])
        .token(&room.ada.token)
        .query(&image)
        .send()
        .await
        .ok();
    assert!(result.items.is_empty() && result.next_cursor.is_none());
}

#[tokio::test]
async fn any_file_type_includes_a_conversation_file_without_recorded_media_type() {
    use orch_api_contract::search::{SearchStreamEvent, SearchStreamList};

    let desk = Desk::new().await;
    let (thread, segment) = thread(&desk, &desk.ada, desk.topic, "Files").await;
    let item = say(
        &desk,
        &desk.ada,
        thread,
        segment,
        "papertrail",
        desk.world.now(),
    )
    .await;
    let parts = serde_json::json!([
        {"type":"text","text":"papertrail"},
        {"type":"file","path":"paper.pdf","name":"paper.pdf","size":4,"blob":null}
    ]);
    sqlx::query("UPDATE items SET payload=jsonb_set(payload,'{parts}',$2::jsonb) WHERE id=$1")
        .bind(item)
        .bind(parts)
        .execute(desk.world.app.app.db.pool())
        .await
        .unwrap();
    let any = SearchListQuery {
        kinds: Some("user_message".into()),
        file_type: Some("*/*".into()),
        ..query("papertrail")
    };
    let found = find(&desk, &desk.ada, &any).await;
    assert_eq!(found.items.len(), 1);
    assert!(matches!(&found.items[0].found, SearchFound::Item { item: hit, .. } if hit.id == item));
    let typed = SearchListQuery {
        file_type: Some("application/pdf".into()),
        ..any.clone()
    };
    assert!(find(&desk, &desk.ada, &typed).await.items.is_empty());
    let response = desk
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&desk.ada.token)
        .query(&SearchListQuery {
            immediate: Some(true),
            ..any
        })
        .send_bytes()
        .await;
    assert_eq!(response.status, 200);
    let hits = response
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter(|line| {
            matches!(
                serde_json::from_slice::<SearchStreamEvent>(line).unwrap(),
                SearchStreamEvent::Hit { .. }
            )
        })
        .count();
    assert_eq!(hits, 1);
}

#[tokio::test]
async fn a_tool_image_is_found_by_the_with_file_and_image_type_filters() {
    use orch_api_contract::search::{SearchStreamEvent, SearchStreamList};
    use orch_engine::{
        ingest::{ItemSpec, add_item},
        unit::Unit,
    };

    let desk = Desk::new().await;
    let (thread, segment) = thread(&desk, &desk.ada, desk.topic, "Images").await;
    let hash = orch_types::Hash::of(b"image");
    let payload = serde_json::json!({"title":"chartneedle", "started_at":Time::new(desk.world.now()), "duration_ms":null,
        "tool":{"kind":"image","blob":hash,"media_type":"image/png","name":"chart.png","caption":null},
        "raw":{"name":"image","input":{"inline":"","total_len":0,"truncated":false,"blob":null},"output":null,"is_error":false}});
    let mut tx = desk.world.app.app.db.begin().await.unwrap();
    sqlx::query("INSERT INTO blobs(hash,size,pin_count) VALUES($1,5,1)")
        .bind(hash.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .unwrap();
    let mut unit = Unit::new(&mut tx, desk.world.now());
    let item = add_item(
        &mut unit,
        &ItemSpec {
            thread,
            segment,
            turn: None,
            stream: "main",
            parent: None,
            kind: "tool_call",
            status: "completed",
            author: None,
            payload: &payload,
            native_ref: None,
        },
    )
    .await
    .unwrap()
    .unwrap();
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    let ask = SearchListQuery {
        kinds: Some("tool_call".into()),
        with_file: Some(true),
        file_type: Some("image/*".into()),
        ..query("chartneedle")
    };
    let found = find(&desk, &desk.ada, &ask).await;
    assert_eq!(found.items.len(), 1);
    assert!(matches!(&found.items[0].found, SearchFound::Item {item:hit,..} if hit.id == item.id));
    let response = desk
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&desk.ada.token)
        .query(&SearchListQuery {
            immediate: Some(true),
            ..ask
        })
        .send_bytes()
        .await;
    assert_eq!(response.status, 200);
    let hits = response
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter(|line| {
            matches!(
                serde_json::from_slice::<SearchStreamEvent>(line).unwrap(),
                SearchStreamEvent::Hit { .. }
            )
        })
        .count();
    assert_eq!(hits, 1);
}

#[tokio::test]
async fn a_file_type_filter_does_not_let_a_newer_other_type_fill_the_page() {
    use orch_api_contract::search::{SearchStreamEvent, SearchStreamList};

    let desk = Desk::new().await;
    let image = desk.make(&desk.ada, "pollen image").await;
    let other = desk.make(&desk.ada, "pollen text").await;
    let hash = vec![9u8; 32];
    let mut conn = desk.world.conn().await;
    sqlx::query("INSERT INTO blobs(hash,size,pin_count) VALUES($1,1,2)")
        .bind(&hash)
        .execute(&mut *conn)
        .await
        .unwrap();
    for (document, name, media, is_text) in [
        (image.base.id, "photo.png", "image/png", false),
        (other.base.id, "notes.txt", "text/plain", true),
    ] {
        sqlx::query("INSERT INTO document_files(id,document_id,blob,name,media_type,size,is_text,added_by,added_at) VALUES($1,$2,$3,$4,$5,1,$6,$7,$8)")
            .bind(new_id()).bind(document).bind(&hash).bind(name).bind(media).bind(is_text).bind(desk.ada.id()).bind(desk.world.now()).execute(&mut *conn).await.unwrap();
    }
    sqlx::query("UPDATE documents SET files_bytes=1 WHERE id=ANY($1)")
        .bind(vec![image.base.id, other.base.id])
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE documents SET updated_at=$2,files_bytes=1 WHERE id=$1")
        .bind(other.base.id)
        .bind(desk.world.now() + chrono::Duration::days(1))
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let ask = SearchListQuery {
        kinds: Some("document".into()),
        file_type: Some("image/*".into()),
        limit: Some(1),
        ..query("pollen")
    };
    let result = find(&desk, &desk.ada, &ask).await;
    assert_eq!(result.items.len(), 1);
    assert!(
        matches!(&result.items[0].found, SearchFound::Document { document, file:Some(file), .. } if document.id == image.base.id && file.media_type == "image/png")
    );
    assert!(result.next_cursor.is_none());
    let response = desk
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&desk.ada.token)
        .query(&SearchListQuery {
            immediate: Some(true),
            ..ask
        })
        .send_bytes()
        .await;
    assert_eq!(response.status, 200);
    let hits: Vec<_> = response
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(
            |line| match serde_json::from_slice::<SearchStreamEvent>(line).unwrap() {
                SearchStreamEvent::Hit { hit, .. } => Some(hit),
                _ => None,
            },
        )
        .collect();
    assert_eq!(hits.len(), 1);
    assert!(
        matches!(&hits[0].found, SearchFound::Document {document,..} if document.id == image.base.id)
    );
}

#[tokio::test]
async fn a_thread_hidden_from_another_person_never_matches_or_contributes_a_next_cursor() {
    let desk = Desk::new().await;
    let bea = desk.member("bea", "viewer").await;
    let (thread, segment) = thread(&desk, &desk.ada, desk.topic, "Secret release").await;
    for _ in 0..3 {
        say(
            &desk,
            &desk.ada,
            thread,
            segment,
            "release secret",
            desk.world.now(),
        )
        .await;
    }
    let ask = SearchListQuery {
        kinds: Some("user_message".to_owned()),
        limit: Some(1),
        ..query("release")
    };
    assert_eq!(find(&desk, &bea, &ask).await.items.len(), 1);
    cut_view(&desk, thread).await;
    let hidden = find(&desk, &bea, &ask).await;
    assert!(hidden.items.is_empty() && hidden.next_cursor.is_none());
    assert_eq!(find(&desk, &desk.ada, &ask).await.items.len(), 1);
    let names = desk
        .world
        .app
        .call(NameSearchList, &[])
        .token(&bea.token)
        .query(&NameSearchListQuery {
            q: Some("release".to_owned()),
            kinds: Some("thread".to_owned()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(names.is_empty());
}

#[tokio::test]
async fn direct_and_group_chats_are_found_by_participants_and_never_by_an_inspecting_admin() {
    let room = Room::new().await;
    let direct = room.direct(&room.ada, &room.bea).await.base.id;
    let group = room
        .group(&room.ada, &[&room.bea], Some("Release group"))
        .await
        .base
        .id;
    room.say(&room.ada, direct, "release direct").await;
    room.say(&room.bea, group, "release group").await;
    assert!(!room.chat(&room.dee, direct).await.can.send);
    let ask = SearchListQuery {
        kinds: Some("chat_message".to_owned()),
        scope: Some(room.root),
        ..query("release")
    };
    for who in [&room.ada, &room.bea] {
        let hits = room
            .world
            .app
            .call(SearchList, &[])
            .token(&who.token)
            .query(&ask)
            .send()
            .await
            .ok();
        assert_eq!(hits.items.len(), 2);
        assert!(
            hits.items.iter().all(
                |hit| matches!(&hit.found,SearchFound::ChatMessage {path,..} if path.is_empty())
            )
        );
    }
    let before = room.world.audit().await;
    for who in [&room.dee, &room.eve] {
        let hits = room
            .world
            .app
            .call(SearchList, &[])
            .token(&who.token)
            .query(&ask)
            .send()
            .await
            .ok();
        assert!(hits.items.is_empty() && hits.next_cursor.is_none());
        let names = room
            .world
            .app
            .call(NameSearchList, &[])
            .token(&who.token)
            .query(&NameSearchListQuery {
                q: Some("release".to_owned()),
                kinds: Some("chat".to_owned()),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert!(names.is_empty());
    }
    assert_eq!(
        room.world.audit().await,
        before,
        "a search never inspects a chat"
    );
    let confined = room
        .world
        .app
        .call(SearchList, &[])
        .token(&room.ada.token)
        .query(&SearchListQuery {
            scope: Some(room.topic),
            ..ask
        })
        .send()
        .await
        .ok();
    assert!(
        confined.items.is_empty(),
        "direct chats belong to the root, outside a topic scope"
    );
}

#[tokio::test]
async fn edits_replace_chat_matches_and_deleted_messages_never_match() {
    let room = Room::new().await;
    let message = room
        .say(&room.ada, room.chat, "old searchable phrase")
        .await;
    let ask = |words: &str| {
        room.world
            .app
            .call(SearchList, &[])
            .token(&room.ada.token)
            .query(&SearchListQuery {
                kinds: Some("chat_message".to_owned()),
                ..query(words)
            })
            .send()
    };
    assert_eq!(ask("old searchable").await.ok().items.len(), 1);
    room.world
        .app
        .call(ChatMessageUpdate, &[&room.chat, &message.id])
        .token(&room.ada.token)
        .body(&ChatMessageUpdateBody {
            parts: vec![text("new searchable phrase")],
        })
        .send()
        .await
        .ok();
    assert!(ask("old searchable").await.ok().items.is_empty());
    assert_eq!(ask("new searchable").await.ok().items.len(), 1);
    room.world
        .app
        .call(ChatMessageDelete, &[&room.chat, &message.id])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let gone = ask("new searchable").await.ok();
    assert!(gone.items.is_empty() && gone.next_cursor.is_none());
}

#[tokio::test]
async fn edited_chat_dates_and_editor_follow_the_actual_message_edit() {
    use orch_api_contract::search::{SearchDateField, SearchStreamEvent, SearchStreamList};

    let room = Room::new().await;
    let message = room.say(&room.ada, room.chat, "concordance before").await;
    room.world.advance(chrono::Duration::days(1));
    let edited = Time::new(room.world.now());
    room.world
        .app
        .call(ChatMessageUpdate, &[&room.chat, &message.id])
        .token(&room.ada.token)
        .body(&ChatMessageUpdateBody {
            parts: vec![text("concordance after")],
        })
        .send()
        .await
        .ok();
    let ask = SearchListQuery {
        kinds: Some("chat_message".into()),
        edited_by_id: Some(room.ada.id()),
        date_field: Some(SearchDateField::Edited),
        from: Some(edited),
        to: Some(edited),
        ..query("concordance")
    };
    let page = room
        .world
        .app
        .call(SearchList, &[])
        .token(&room.ada.token)
        .query(&ask)
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 1);
    assert!(
        matches!(&page.items[0].found, SearchFound::ChatMessage { message: hit, .. } if hit.id == message.id)
    );
    let created = SearchListQuery {
        date_field: Some(SearchDateField::Created),
        ..ask.clone()
    };
    assert!(
        room.world
            .app
            .call(SearchList, &[])
            .token(&room.ada.token)
            .query(&created)
            .send()
            .await
            .ok()
            .items
            .is_empty()
    );
    let wrong_editor = SearchListQuery {
        edited_by_id: Some(room.bea.id()),
        ..ask.clone()
    };
    assert!(
        room.world
            .app
            .call(SearchList, &[])
            .token(&room.ada.token)
            .query(&wrong_editor)
            .send()
            .await
            .ok()
            .items
            .is_empty()
    );
    let response = room
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&room.ada.token)
        .query(&SearchListQuery {
            immediate: Some(true),
            ..ask
        })
        .send_bytes()
        .await;
    assert_eq!(response.status, 200);
    let hits: Vec<_> = response
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(
            |line| match serde_json::from_slice::<SearchStreamEvent>(line).unwrap() {
                SearchStreamEvent::Hit { hit, .. } => Some(hit),
                _ => None,
            },
        )
        .collect();
    assert_eq!(hits.len(), 1);
    assert!(
        matches!(&hits[0].found, SearchFound::ChatMessage { message: hit, .. } if hit.id == message.id)
    );
}

#[tokio::test]
async fn document_search_uses_its_own_page_and_only_current_titles_and_live_blocks() {
    let desk = Desk::new().await;
    let bea = desk.member("bea", "viewer").await;
    let document = desk
        .make_body(
            &desk.ada,
            NodeDocumentCreateBody {
                title: "Release handbook".to_owned(),
                blocks: Some(vec![
                    paragraph("release first"),
                    paragraph("release second"),
                ]),
                markdown: None,
                access: Some(NewDocumentAccess::Private),
            },
        )
        .await
        .ok();
    let ask = SearchListQuery {
        kinds: Some("document".to_owned()),
        scope: Some(desk.project),
        ..query("release")
    };
    let mine = find(&desk, &desk.ada, &ask).await;
    assert_eq!(mine.items.len(), 3);
    assert_eq!(
        mine.items
            .iter()
            .filter(|hit| hit.match_field
                == Some(orch_api_contract::search::SearchMatchField::DocumentTitle))
            .count(),
        1
    );
    assert_eq!(mine.items.iter().filter(|hit| hit.match_field == Some(orch_api_contract::search::SearchMatchField::Body)).count(), 2);
    assert!(mine.items.iter().all(|hit|matches!(&hit.found,SearchFound::Document {document:doc,path,..} if doc.id==document.base.id && path.last().unwrap().id==desk.topic)));
    let hidden = find(&desk, &bea, &ask).await;
    assert!(hidden.items.is_empty() && hidden.next_cursor.is_none());
    let name_ask = NameSearchListQuery {
        q: Some("release".to_owned()),
        kinds: Some("document".to_owned()),
        ..Default::default()
    };
    assert!(
        desk.world
            .app
            .call(NameSearchList, &[])
            .token(&bea.token)
            .query(&name_ask)
            .send()
            .await
            .ok()
            .is_empty()
    );
    let blocks = desk.sync(&desk.ada, document.base.id).await.blocks;
    desk.write(
        &desk.ada,
        document.base.id,
        vec![DocumentOp::Remove { id: blocks[0].id }],
    )
    .await
    .ok();
    assert_eq!(find(&desk, &desk.ada, &ask).await.items.len(), 2);
    desk.world
        .app
        .call(DocumentUpdate, &[&document.base.id])
        .token(&desk.ada.token)
        .body(&DocumentUpdateBody {
            title: Some("Other handbook".to_owned()),
            description: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(find(&desk, &desk.ada, &ask).await.items.len(), 1);
    desk.world
        .app
        .call(DocumentDelete, &[&document.base.id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(find(&desk, &desk.ada, &ask).await.items.is_empty());
}

#[tokio::test]
async fn a_header_and_a_block_with_the_same_identifier_still_page_without_loss() {
    let desk = Desk::new().await;
    let document = desk.make(&desk.ada, "Release").await;
    let mut block = paragraph("release");
    block.id = document.base.id;
    desk.write(&desk.ada, document.base.id, vec![put(block)])
        .await
        .ok();
    let mut ask = SearchListQuery {
        kinds: Some("document".to_owned()),
        limit: Some(1),
        ..query("release")
    };
    let mut all = HashSet::new();
    for _ in 0..3 {
        let page = find(&desk, &desk.ada, &ask).await;
        for hit in &page.items {
            assert!(all.insert(key(hit)), "a page never repeats a match");
        }
        match page.next_cursor {
            Some(cursor) => ask.cursor = Some(cursor),
            None => break,
        }
    }
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn one_hundred_thousand_items_are_measured_through_the_authorized_request() {
    let desk = Desk::new().await;
    let (thread, segment) = thread(&desk, &desk.ada, desk.topic, "Scale").await;
    let ids: Vec<Id> = (0..100_000).map(|_| new_id()).collect();
    let mut conn = desk.world.conn().await;
    sqlx::query("INSERT INTO items (id,thread_id,segment_id,stream,kind,status,author_id,payload,search_text,seq_created,seq_updated,created_at,updated_at) SELECT id,$2,$3,'main','user_message','completed',$4,jsonb_build_object('parts',jsonb_build_array(jsonb_build_object('type','text','text',CASE WHEN ord%1000=0 THEN 'needle release' ELSE 'ordinary conversation' END)),'input_id',id,'delivery','queue','source','user'),CASE WHEN ord%1000=0 THEN 'needle release' ELSE 'ordinary conversation' END,ord,ord,$5,$5 FROM unnest($1::uuid[]) WITH ORDINALITY x(id,ord)")
        .bind(&ids).bind(thread).bind(segment).bind(desk.ada.id()).bind(desk.world.now()).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE threads SET next_seq=100001,activity_seq=100000 WHERE node_id=$1")
        .bind(thread)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let mut times = Vec::new();
    for _ in 0..5 {
        let start = std::time::Instant::now();
        let page = find(
            &desk,
            &desk.ada,
            &SearchListQuery {
                kinds: Some("user_message".to_owned()),
                ..query("needle")
            },
        )
        .await;
        times.push(start.elapsed().as_millis());
        assert_eq!(page.items.len(), 50);
        assert!(page.next_cursor.is_some());
    }
    times.sort_unstable();
    println!("SEARCH_100000 samples_ms={times:?} median_ms={}", times[2]);
}

#[tokio::test]
async fn a_long_conversation_pages_its_common_rare_and_old_words_newest_first_without_reading_it_whole()
 {
    // 1,200 items, one a second: every one says "common"; every 300th "rarest", which the newest rows
    // sample misses and the text index finds; the oldest 400 "spread", too many for the text index past
    // the sample, so they are read in recency order. Every page must equal the newest-first truth.
    let desk = Desk::new().await;
    let (thread, segment) = thread(&desk, &desk.ada, desk.topic, "Long").await;
    let ids: Vec<Id> = (0..1_200).map(|_| new_id()).collect();
    let text = "CASE WHEN ord%300=0 THEN 'common rarest' WHEN ord<=400 THEN 'common spread' ELSE 'common' END";
    let mut conn = desk.world.conn().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("INSERT INTO items (id,thread_id,segment_id,stream,kind,status,author_id,payload,search_text,seq_created,seq_updated,created_at,updated_at) SELECT id,$2,$3,'main','user_message','completed',$4,jsonb_build_object('parts',jsonb_build_array(jsonb_build_object('type','text','text',{text})),'input_id',id,'delivery','queue','source','user'),{text},ord,ord,$5-(1200-ord)*interval '1 second',$5 FROM unnest($1::uuid[]) WITH ORDINALITY x(id,ord)")))
        .bind(&ids).bind(thread).bind(segment).bind(desk.ada.id()).bind(desk.world.now()).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE threads SET next_seq=1201,activity_seq=1200 WHERE node_id=$1")
        .bind(thread)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    for (word, says) in [
        ("common", (|_| true) as fn(usize) -> bool),
        ("rarest", |ord| ord % 300 == 0),
        ("spread", |ord| ord <= 400 && ord % 300 != 0),
    ] {
        let truth: Vec<Id> = (1..=1_200)
            .rev()
            .filter(|ord| says(*ord))
            .map(|ord| ids[ord - 1])
            .collect();
        let mut found = Vec::new();
        let mut cursor = None;
        loop {
            let page = find(
                &desk,
                &desk.ada,
                &SearchListQuery {
                    scope: Some(thread),
                    kinds: Some("user_message".to_owned()),
                    cursor: cursor.clone(),
                    ..query(word)
                },
            )
            .await;
            assert!(page.items.len() <= 50);
            // Asked without a history, every hit follows the active one, as the reader that asked does.
            assert!(page.items.iter().all(|hit| matches!(&hit.found,
                SearchFound::Item { projection: Some(projection), .. } if projection.follows_active)));
            found.extend(page.items.iter().map(|hit| key(hit).1));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(found, truth, "{word}");
    }
}

#[tokio::test]
async fn node_chat_view_is_checked_independently_of_the_nodes_view() {
    let room = Room::new().await;
    room.say(&room.ada, room.chat, "release planning").await;
    let ask = SearchListQuery {
        kinds: Some("chat_message".to_owned()),
        scope: Some(room.topic),
        ..query("release")
    };
    for who in [&room.ada, &room.bea] {
        assert_eq!(
            room.world
                .app
                .call(SearchList, &[])
                .token(&who.token)
                .query(&ask)
                .send()
                .await
                .ok()
                .items
                .len(),
            1
        );
    }
    let mut conn = room.world.conn().await;
    sqlx::query("INSERT INTO permission_rows (object_kind,object_id,page,profile,permission,inherit) SELECT 'node',$1,'live','any',permission,false FROM unnest(ARRAY['chat.view','chat.interact','chat.manage']) x(permission)").bind(room.topic).execute(&mut *conn).await.unwrap();
    drop(conn);
    let hidden = room
        .world
        .app
        .call(SearchList, &[])
        .token(&room.bea.token)
        .query(&ask)
        .send()
        .await
        .ok();
    assert!(hidden.items.is_empty() && hidden.next_cursor.is_none());
    assert_eq!(
        room.world
            .app
            .call(SearchList, &[])
            .token(&room.ada.token)
            .query(&ask)
            .send()
            .await
            .ok()
            .items
            .len(),
        1
    );
    assert!(
        room.world
            .app
            .call(NameSearchList, &[])
            .token(&room.bea.token)
            .query(&NameSearchListQuery {
                q: Some("login".to_owned()),
                kinds: Some("chat".to_owned()),
                ..Default::default()
            })
            .send()
            .await
            .ok()
            .is_empty()
    );
}

#[tokio::test]
async fn one_cursor_pages_across_items_chat_messages_and_document_matches_without_repeating_them() {
    let desk = Desk::new().await;
    let (thread, segment) = thread(&desk, &desk.ada, desk.topic, "Mixed").await;
    say(
        &desk,
        &desk.ada,
        thread,
        segment,
        "release item",
        desk.world.now(),
    )
    .await;
    let chat = chat_world::chat_of(&desk.world, &desk.ada, desk.topic).await;
    desk.world
        .app
        .call(orch_api_contract::chats::ChatMessageCreate, &[&chat])
        .token(&desk.ada.token)
        .body(&orch_api_contract::chats::ChatMessageCreateBody {
            agent_review: None,
            parts: vec![text("release message")],
            reply_to: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    desk.make_body(
        &desk.ada,
        NodeDocumentCreateBody {
            title: "Release".to_owned(),
            blocks: Some(vec![paragraph("release block")]),
            markdown: None,
            access: None,
        },
    )
    .await
    .ok();
    let mut ask = SearchListQuery {
        limit: Some(1),
        ..query("release")
    };
    let mut all = HashSet::new();
    for _ in 0..5 {
        let page = find(&desk, &desk.ada, &ask).await;
        for hit in &page.items {
            assert!(all.insert(key(hit)));
        }
        match page.next_cursor {
            Some(cursor) => ask.cursor = Some(cursor),
            None => break,
        }
    }
    assert_eq!(all.len(), 4);
    assert_eq!(
        all.iter()
            .map(|(kind, _, _)| kind.as_str())
            .collect::<HashSet<_>>(),
        HashSet::from(["item", "chat_message", "document"])
    );
}

#[tokio::test]
async fn a_token_search_never_leaves_its_subtree_or_its_read_scopes() {
    let desk = Desk::new().await;
    let bea = desk.member("bea", "viewer").await;
    let inside = thread(&desk, &desk.ada, desk.topic, "Inside").await;
    say(
        &desk,
        &desk.ada,
        inside.0,
        inside.1,
        "release inside",
        desk.world.now(),
    )
    .await;
    let other = places::make_project(&desk.world, &desk.ada, desk.org, "Other")
        .await
        .base
        .id;
    let topic = places::make_topic(&desk.world, &desk.ada, other, "Elsewhere")
        .await
        .base
        .id;
    let outside = thread(&desk, &desk.ada, topic, "Outside").await;
    say(
        &desk,
        &desk.ada,
        outside.0,
        outside.1,
        "release outside",
        desk.world.now(),
    )
    .await;
    let token = orch_api_core::middleware::authentication::new_token(
        orch_api_core::middleware::authentication::API_TOKEN_PREFIX,
    );
    let mut conn = desk.world.conn().await;
    let id = orch_db::queries::identity::testing::insert_api_token(
        &mut conn,
        bea.id(),
        &orch_api_core::middleware::authentication::token_hash(&token),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE api_tokens SET node_id=$2,scopes=ARRAY['thread.view'] WHERE id=$1")
        .bind(id)
        .bind(desk.project)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let page = desk
        .world
        .app
        .call(SearchList, &[])
        .token(&token)
        .query(&query("release"))
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 1);
    assert!(matches!(&page.items[0].found,SearchFound::Item {thread,..} if thread.id==inside.0));
}

#[tokio::test]
async fn a_guest_can_find_the_group_they_joined_but_an_outsider_cannot() {
    let room = Room::new().await;
    let group = room
        .group(&room.ada, &[&room.bea], Some("Guest release"))
        .await
        .base
        .id;
    room.world
        .app
        .call(
            orch_api_contract::chats::ChatMemberAdd,
            &[&group, &room.eve.id()],
        )
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    room.say(&room.ada, group, "guest release notes").await;
    for who in [&room.ada, &room.eve] {
        let page = room
            .world
            .app
            .call(SearchList, &[])
            .token(&who.token)
            .query(&SearchListQuery {
                kinds: Some("chat_message".to_owned()),
                ..query("guest release")
            })
            .send()
            .await
            .ok();
        assert_eq!(page.items.len(), 1);
        let names = room
            .world
            .app
            .call(NameSearchList, &[])
            .token(&who.token)
            .query(&NameSearchListQuery {
                q: Some("guest release".to_owned()),
                kinds: Some("chat".to_owned()),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        assert_eq!(names.len(), 1);
    }
    assert!(
        room.world
            .app
            .call(SearchList, &[])
            .token(&room.fay.token)
            .query(&query("guest release"))
            .send()
            .await
            .ok()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn descriptions_headings_and_body_keep_their_source_field_in_newest_and_streamed_results() {
    use orch_api_contract::search::{
        SearchMatchField as Field, SearchSort, SearchStreamEvent, SearchStreamList,
    };
    let desk = Desk::new().await;
    let document = desk
        .make_body(
            &desk.ada,
            NodeDocumentCreateBody {
                title: "Reference".into(),
                blocks: None,
                markdown: Some("# quilldawn heading\n\nquilldawn body".into()),
                access: Some(NewDocumentAccess::Private),
            },
        )
        .await
        .ok();
    desk.world
        .app
        .call(DocumentUpdate, &[&document.base.id])
        .token(&desk.ada.token)
        .body(&DocumentUpdateBody {
            title: None,
            description: Some("quilldawn overview".into()),
        })
        .send()
        .await
        .ok();
    let ask = SearchListQuery {
        kinds: Some("document".into()),
        ..query("quilldawn")
    };
    let page = find(&desk, &desk.ada, &ask).await;
    assert_eq!(page.items.len(), 3);
    for field in [Field::Description, Field::DocumentHeading, Field::Body] {
        assert_eq!(
            page.items
                .iter()
                .filter(|hit| hit.match_field == Some(field))
                .count(),
            1
        );
    }
    let response = desk
        .world
        .app
        .call(SearchStreamList, &[])
        .token(&desk.ada.token)
        .query(&SearchListQuery {
            sort: Some(SearchSort::Relevance),
            immediate: Some(true),
            ..ask
        })
        .send_bytes()
        .await;
    assert_eq!(response.status, 200);
    let events: Vec<SearchStreamEvent> = response
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    let payloads: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let SearchStreamEvent::Hit { id, hit, .. } = event {
                Some((id, hit.match_field))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(payloads.len(), 3);
    let order = events
        .iter()
        .rev()
        .find_map(|event| {
            if let SearchStreamEvent::Order { items, .. } = event {
                Some(items)
            } else {
                None
            }
        })
        .unwrap();
    for (id, field) in payloads {
        assert_eq!(
            order.iter().find(|row| &row.id == id).unwrap().match_field,
            field
        );
    }
}

#[tokio::test]
async fn a_documents_file_name_matches_as_a_header_and_obeys_its_documents_page() {
    let desk = Desk::new().await;
    let bea = desk.member("bea", "viewer").await;
    let document = desk
        .make_body(
            &desk.ada,
            NodeDocumentCreateBody {
                title: "Files".to_owned(),
                blocks: None,
                markdown: None,
                access: Some(NewDocumentAccess::Private),
            },
        )
        .await
        .ok();
    let mut conn = desk.world.conn().await;
    let hash = vec![7u8; 32];
    sqlx::query("INSERT INTO blobs (hash,size,pin_count) VALUES ($1,1,2)")
        .bind(&hash)
        .execute(&mut *conn)
        .await
        .unwrap();
    let file_id = new_id();
    sqlx::query("INSERT INTO document_files (id,document_id,blob,name,media_type,size,is_text,added_by,added_at) VALUES ($1,$2,$3,'release_notes.txt','text/plain',1,true,$4,$5)").bind(file_id).bind(document.base.id).bind(&hash).bind(desk.ada.id()).bind(desk.world.now()).execute(&mut *conn).await.unwrap();
    let image_id = new_id();
    sqlx::query("INSERT INTO document_files (id,document_id,blob,name,media_type,size,is_text,added_by,added_at) VALUES ($1,$2,$3,'diagram.png','image/png',1,false,$4,$5)")
        .bind(image_id).bind(document.base.id).bind(&hash).bind(desk.ada.id()).bind(desk.world.now()).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE documents SET files_bytes=2 WHERE id=$1")
        .bind(document.base.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let page = find(&desk, &desk.ada, &query("release_notes")).await;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].snippet, "release_notes.txt");
    assert_eq!(
        page.items[0].match_field,
        Some(orch_api_contract::search::SearchMatchField::FileName)
    );
    assert!(matches!(
        &page.items[0].found,
        SearchFound::Document { block_id: None, .. }
    ));
    let text_file = SearchListQuery {
        file_type: Some("text/*".into()),
        ..query("release_notes")
    };
    assert_eq!(find(&desk, &desk.ada, &text_file).await.items.len(), 1);
    let image_file = SearchListQuery {
        file_type: Some("image/*".into()),
        ..text_file.clone()
    };
    assert!(find(&desk, &desk.ada, &image_file).await.items.is_empty());
    let title_only = SearchListQuery {
        title_only: Some(true),
        ..query("release_notes")
    };
    assert!(find(&desk, &desk.ada, &title_only).await.items.is_empty());
    let title = SearchListQuery {
        kinds: Some("document".into()),
        title_only: Some(true),
        ..query("Files")
    };
    assert_eq!(find(&desk, &desk.ada, &title).await.items.len(), 1);
    let with_file = SearchListQuery {
        with_file: Some(true),
        kinds: Some("document".into()),
        ..query("Files")
    };
    let title_with_file = find(&desk, &desk.ada, &with_file).await;
    assert!(
        matches!(&title_with_file.items[0].found, SearchFound::Document { file: Some(file), .. } if file.id == image_id)
    );
    let image_title = SearchListQuery {
        file_type: Some("image/*".into()),
        ..query("Files")
    };
    let image_result = find(&desk, &desk.ada, &image_title).await;
    assert!(
        matches!(&image_result.items[0].found, SearchFound::Document { file: Some(file), .. } if file.id == image_id)
    );
    for (ask, wanted) in [(text_file, 1), (image_file, 0), (title_only, 0), (title, 1)] {
        let response = desk
            .world
            .app
            .call(orch_api_contract::search::SearchStreamList, &[])
            .token(&desk.ada.token)
            .query(&SearchListQuery {
                immediate: Some(true),
                ..ask
            })
            .send_bytes()
            .await;
        assert_eq!(response.status, 200);
        let hits = response
            .raw
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .filter(|line| {
                matches!(
                    serde_json::from_slice::<orch_api_contract::search::SearchStreamEvent>(line)
                        .unwrap(),
                    orch_api_contract::search::SearchStreamEvent::Hit { .. }
                )
            })
            .count();
        assert_eq!(hits, wanted);
    }
    for ask in [
        query("release_notes"),
        query("notes release"),
        query("\"release_notes\""),
    ] {
        let found = find(&desk, &desk.ada, &ask).await;
        assert_eq!(found.items.len(), 1);
        let SearchFound::Document {
            file: Some(file), ..
        } = &found.items[0].found
        else {
            panic!("a filename hit carries its authorized file description");
        };
        assert_eq!(file.id, file_id);
        assert_eq!(file.name, "release_notes.txt");
        assert_eq!(file.media_type, "text/plain");
        assert_eq!(file.size, 1);
        assert!(file.added_by.is_some());
    }
    let response = desk
        .world
        .app
        .call(orch_api_contract::search::SearchStreamList, &[])
        .token(&desk.ada.token)
        .query(&SearchListQuery {
            regex: Some(true),
            immediate: Some(true),
            sort: Some(orch_api_contract::search::SearchSort::Relevance),
            ..query(r"release_.*\.txt")
        })
        .send_bytes()
        .await;
    assert_eq!(response.status, 200);
    let streamed: Vec<SearchHit> = response
        .raw
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            match serde_json::from_slice::<orch_api_contract::search::SearchStreamEvent>(line)
                .unwrap()
            {
                orch_api_contract::search::SearchStreamEvent::Hit { hit, .. } => Some(*hit),
                _ => None,
            }
        })
        .collect();
    assert_eq!(streamed.len(), 1);
    assert_eq!(
        streamed[0].match_field,
        Some(orch_api_contract::search::SearchMatchField::FileName)
    );
    assert!(!streamed[0].matches.is_empty());
    let SearchFound::Document {
        file: Some(file),
        block_id: None,
        ..
    } = &streamed[0].found
    else {
        panic!("the regex filename hit carries its authorized file description");
    };
    assert_eq!(file.id, file_id);
    assert_eq!(file.name, "release_notes.txt");
    assert_eq!(file.media_type, "text/plain");
    assert_eq!(file.size, 1);
    assert!(file.added_by.is_some());
    assert!(
        find(&desk, &bea, &query("release_notes"))
            .await
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn unicode_highlights_use_the_same_case_mapping_as_the_database_match() {
    let desk = Desk::new().await;
    let (thread, segment) = thread(&desk, &desk.ada, desk.topic, "Unicode").await;
    say(
        &desk,
        &desk.ada,
        thread,
        segment,
        "İSTANBUL",
        desk.world.now(),
    )
    .await;
    let page = find(&desk, &desk.ada, &query("istanbul")).await;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].matches, [(0, 8)]);
}

#[tokio::test]
async fn legacy_pages_scan_past_a_full_raw_page_of_private_matches_without_losing_the_visible_cursor()
 {
    let desk = Desk::new().await;
    let bea = desk.member("bea", "developer").await;
    let visible = desk.make(&bea, "pagewindowneedle visible").await;
    let private = desk
        .make_body(
            &desk.ada,
            NodeDocumentCreateBody {
                title: "pagewindowneedle private".into(),
                blocks: None,
                markdown: None,
                access: Some(NewDocumentAccess::Private),
            },
        )
        .await
        .ok();
    let mut tx = desk.world.app.app.db.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO documents(id,node_id,org_id,owner_id,created_by,title,basename,updated_at)
       SELECT md5('private-window/'||i)::uuid,d.node_id,d.org_id,d.owner_id,d.created_by,
         'pagewindowneedle private '||i,'private-window-'||i||'.md',d.updated_at+interval '1 minute'
       FROM documents d CROSS JOIN generate_series(1,225)i WHERE d.id=$1",
    )
    .bind(private.base.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO permission_rows(object_kind,object_id,page,profile,permission,inherit,locked)
       SELECT p.object_kind,md5('private-window/'||i)::uuid,p.page,p.profile,p.permission,p.inherit,p.locked
       FROM permission_rows p CROSS JOIN generate_series(1,225)i WHERE p.object_kind='document' AND p.object_id=$1")
        .bind(private.base.id).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO permission_entries(id,object_kind,object_id,page,permission,holder_kind,holder_id,source)
       SELECT md5('private-window/owner/'||i)::uuid,'document',md5('private-window/'||i)::uuid,
        'live','access.own','user',$1,'creation' FROM generate_series(1,225)i")
        .bind(desk.ada.id()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let ask = SearchListQuery {
        kinds: Some("document".into()),
        limit: Some(1),
        ..query("pagewindowneedle")
    };
    let page = find(&desk, &bea, &ask).await;
    assert_eq!(page.items.len(), 1);
    assert!(
        matches!(&page.items[0].found,SearchFound::Document {document,..} if document.id==visible.base.id)
    );
    assert!(
        page.next_cursor.is_none(),
        "private raw matches do not create a continuation"
    );
}
