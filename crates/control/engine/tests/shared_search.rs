//! What an agent finds by searching what was shared with it: the names and the text of conversations, chats and documents, a word or a bounded pattern, narrowed by kind, place or name; and nothing of what it may not read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_link_contract::commands::OrchTool;
use orch_types::Id;
use serde_json::{Value, json};
use shared::*;

struct Place {
    world: common::World,
    reader: Id,
    writer: Id,
    topic: Id,
}

/// A reader, and a thread in another topic whose conversation, chat and documents are shared with it.
async fn a_place_to_search() -> Place {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let topic = world.topic("Docs").await;
    let writer = thread_in_topic(&world, topic, "Writer").await;
    talk(
        &world,
        writer,
        "Why does the login fail?",
        "The session cookie is dropped on redirect.",
    )
    .await;
    let chat = chat_of(&world, topic).await;
    say_in_chat(&world, chat, world.ada, "Staging is down again").await;
    say_in_chat(
        &world,
        chat,
        world.ada,
        "Did anybody restart the cookie service?",
    )
    .await;
    let doc = world
        .document_beside(
            topic,
            "Rollout plan",
            &[
                "First we build.",
                "Then the cookie policy ships.\nIt is the last step.",
            ],
        )
        .await;
    let _ = doc;
    for (permission, kind, thing) in [
        ("thread.view", "node", writer),
        ("chat.view", "node", topic),
        ("doc.view", "node", topic),
    ] {
        share(&world, kind, thing, "live", permission, "any", reader).await;
    }
    Place {
        world,
        reader,
        writer,
        topic,
    }
}

async fn search(place: &Place, arguments: Value) -> orch_link_contract::commands::OrchToolResult {
    call(
        &place.world,
        place.reader,
        OrchTool::SearchShared,
        arguments,
    )
    .await
}

fn hits(result: &Value) -> Vec<&Value> {
    result["hits"].as_array().unwrap().iter().collect()
}

fn kinds_of(result: &Value) -> Vec<String> {
    hits(result)
        .iter()
        .map(|hit| hit["kind"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn an_agents_literal_search_requires_every_word_and_keeps_quoted_phrases_together() {
    let p = a_place_to_search().await;
    let unordered = ok(search(&p, json!({"query":"ships policy", "kinds":["documents"]})).await);
    assert!(!hits(&unordered).is_empty());
    let phrase = ok(search(
        &p,
        json!({"query":"\"policy ships\" cookie", "kinds":["documents"]}),
    )
    .await);
    assert!(!hits(&phrase).is_empty());
    let backwards = ok(search(
        &p,
        json!({"query":"\"ships policy\"", "kinds":["documents"]}),
    )
    .await);
    assert!(hits(&backwards).is_empty());
    let missing = ok(search(
        &p,
        json!({"query":"ships policy missing", "kinds":["documents"]}),
    )
    .await);
    assert!(hits(&missing).is_empty());
}

#[tokio::test]
async fn a_word_is_found_in_conversations_chats_and_documents_with_where_to_read_on() {
    let p = a_place_to_search().await;
    let found = ok(search(&p, json!({ "query": "cookie", "snippets": true })).await);
    let kinds = kinds_of(&found);
    for kind in ["conversation", "chat", "document"] {
        assert!(kinds.contains(&kind.to_owned()), "{kind} in {found}");
    }
    let in_chat = hits(&found)
        .into_iter()
        .find(|hit| hit["kind"] == "chat")
        .unwrap();
    assert!(in_chat["path"].as_str().unwrap().ends_with("/Docs/chat.md"));
    assert_eq!(
        in_chat["position"], 2,
        "the number of the message, which read_shared takes as from"
    );
    assert!(
        in_chat["snippet"]
            .as_str()
            .unwrap()
            .contains("cookie service")
    );
    let matches = in_chat["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    let snippet: Vec<char> = in_chat["snippet"].as_str().unwrap().chars().collect();
    let (from, to) = (
        matches[0][0].as_u64().unwrap() as usize,
        matches[0][1].as_u64().unwrap() as usize,
    );
    assert_eq!(
        snippet[from..to].iter().collect::<String>().to_lowercase(),
        "cookie"
    );
    let in_doc = hits(&found)
        .into_iter()
        .find(|hit| hit["kind"] == "document")
        .unwrap();
    assert_eq!(in_doc["position"], 2, "the top-level block it lies in");
    let in_talk = hits(&found)
        .into_iter()
        .find(|hit| hit["kind"] == "conversation")
        .unwrap();
    assert_eq!(in_talk["position"], 1, "the turn");
    assert!(
        in_talk["path"]
            .as_str()
            .unwrap()
            .ends_with("/Writer/conversation.md")
    );
    assert_eq!(found["truncated"], false);
    assert!(found["searched"]["entries"].as_i64().unwrap() >= 3);
    // And the position is what read_shared takes as from.
    let read = ok(call(
        &p.world,
        p.reader,
        OrchTool::ReadShared,
        json!({ "path": in_chat["path"], "from": in_chat["position"], "to": in_chat["position"] }),
    )
    .await);
    assert!(read["text"].as_str().unwrap().contains("cookie service"));
}

#[tokio::test]
async fn it_looks_only_at_what_the_agent_may_read_each_time_it_is_asked() {
    let p = a_place_to_search().await;
    let secret_topic = p.world.topic("Secrets").await;
    let secret = thread_in_topic(&p.world, secret_topic, "Vault").await;
    talk(&p.world, secret, "The cookie recipe is secret.", "Kept.").await;
    let found = ok(search(&p, json!({ "query": "recipe" })).await);
    assert!(hits(&found).is_empty(), "{found}");
    // Shared now; taken back at once.
    let entry = share(
        &p.world,
        "node",
        secret,
        "live",
        "thread.view",
        "any",
        p.reader,
    )
    .await;
    assert_eq!(
        hits(&ok(search(&p, json!({ "query": "recipe" })).await)).len(),
        1
    );
    let mut conn = p.world.pool.acquire().await.unwrap();
    orch_db::queries::access::remove_entry(&mut conn, entry, p.world.ada, chrono::Utc::now())
        .await
        .unwrap();
    drop(conn);
    assert!(
        hits(&ok(search(&p, json!({ "query": "recipe" })).await)).is_empty(),
        "a share taken back is gone at the next call"
    );
}

#[tokio::test]
async fn the_names_of_places_and_threads_are_matched_whatever_the_kinds_and_a_title_is_a_hit_of_its_document()
 {
    let p = a_place_to_search().await;
    let found = ok(search(&p, json!({ "query": "writer", "kinds": ["chats"] })).await);
    assert_eq!(kinds_of(&found), ["thread"], "{found}");
    assert!(
        hits(&found)[0]["path"]
            .as_str()
            .unwrap()
            .ends_with("/Docs/Writer/")
    );
    assert!(
        hits(&found)[0]["position"].is_null(),
        "a match in a name has no position"
    );
    let titled = ok(search(&p, json!({ "query": "rollout" })).await);
    let doc = hits(&titled)
        .into_iter()
        .find(|hit| hit["kind"] == "document")
        .unwrap();
    assert!(doc["position"].is_null());
    assert!(
        doc["path"]
            .as_str()
            .unwrap()
            .ends_with("/docs/Rollout plan.md")
    );
}

#[tokio::test]
async fn kinds_a_place_and_a_pattern_of_names_narrow_a_search() {
    let p = a_place_to_search().await;
    let only_chats = ok(search(&p, json!({ "query": "cookie", "kinds": ["chats"] })).await);
    assert_eq!(kinds_of(&only_chats), ["chat"]);
    let only_md = ok(search(&p, json!({ "query": "cookie", "names": "chat.md" })).await);
    assert_eq!(kinds_of(&only_md), ["chat"]);
    let under = ok(search(
        &p,
        json!({ "query": "cookie", "path": format!("id:{}", p.writer) }),
    )
    .await);
    assert!(
        kinds_of(&under)
            .iter()
            .all(|kind| kind == "conversation" || kind == "thread"),
        "{under}"
    );
    let nowhere = search(&p, json!({ "query": "cookie", "path": "/nothing/here" })).await;
    assert_eq!(refused(&nowhere).0, "not_found");
}

#[tokio::test]
async fn a_regular_expression_is_matched_by_the_engine_and_a_bad_one_is_refused_with_the_reason() {
    let p = a_place_to_search().await;
    let found = ok(search(&p, json!({ "query": "session\\s+cookie", "regex": true })).await);
    assert!(
        kinds_of(&found).contains(&"conversation".to_owned()),
        "{found}"
    );
    let case = ok(search(&p, json!({ "query": "(?-i)COOKIE", "regex": true })).await);
    assert!(
        hits(&case).is_empty(),
        "it ignores case unless it says (?-i)"
    );
    let behind = search(&p, json!({ "query": "(?<=a)b", "regex": true })).await;
    let (code, message) = refused(&behind);
    assert_eq!(code, "invalid");
    assert!(
        message.contains("look-around") || message.contains("look"),
        "{message}"
    );
    let back = search(&p, json!({ "query": "(a)\\1", "regex": true })).await;
    assert_eq!(refused(&back).0, "invalid");
    let long = search(&p, json!({ "query": "x".repeat(1_001) })).await;
    assert_eq!(refused(&long).0, "invalid");
}

#[tokio::test]
async fn place_and_document_descriptions_share_ranking_and_an_exhausted_older_description_cursor_stays_finished()
 {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let topic = world.topic("Elsewhere").await;
    let document = world.document_beside(topic, "Reference", &["A note"]).await;
    share(&world, "node", topic, "live", "doc.view", "any", reader).await;
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE nodes SET description='quilldawn instructions' WHERE id=$1")
        .bind(topic)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE documents SET description='quilldawn reference',description_source='user' WHERE id=$1").bind(document).execute(&mut *conn).await.unwrap();
    drop(conn);
    let mut cursor = None;
    let mut ids = Vec::new();
    for _ in 0..3 {
        let mut args = json!({"query":"quilldawn","max_results":1});
        if let Some(cursor) = &cursor {
            args["cursor"] = json!(cursor);
        }
        let found = ok(call(&world, reader, OrchTool::SearchShared, args).await);
        ids.extend(
            hits(&found)
                .iter()
                .map(|hit| hit["id"].as_str().unwrap().to_owned()),
        );
        cursor = found["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    ids.sort();
    let mut expected = vec![topic.to_string(), document.to_string()];
    expected.sort();
    assert_eq!(
        ids, expected,
        "both description sources appear once across pages"
    );

    let legacy = serde_json::from_value(json!({"fields":{},"exhausted":["name","title","description","file","heading","block","chat","item","revision"]})).unwrap();
    let grants = json!({});
    let kinds = vec!["user_message".to_owned()];
    let ask = orch_db::queries::search::Content {
        selected: None,
        items: None,
        continuation: None,
        user: world.ada,
        groups: &[],
        member_roots: &[],
        grants: &grants,
        at: chrono::DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        scope: Some(world.root),
        chat: None,
        document: None,
        participating: false,
        relevance: true,
        names: true,
        with_file: false,
        mentions_me: None,
        text: "quilldawn",
        item_kinds: &kinds,
        chat_messages: true,
        document_content: true,
        author: None,
        from: None,
        to: None,
        after: None,
        limit: 50,
    };
    let mut tx = world.pool.begin().await.unwrap();
    let resumed =
        orch_db::queries::search::candidates::window(&mut tx, &ask, legacy, 50, 2000, 200)
            .await
            .unwrap();
    assert!(
        resumed.parts.is_empty(),
        "finishing the combined description stream also finished both of its replacements"
    );
    assert!(resumed.next.is_none());
}

#[tokio::test]
async fn scoped_word_windows_keep_the_legacy_parts_order_and_continuations_in_both_scripts() {
    use orch_db::queries::search::{self as rows, candidates};
    let p = a_place_to_search().await;
    p.world
        .document_beside(
            p.topic,
            "Cookie guide",
            &["Cookie policy cookie", "回滚流程", "再次回滚"],
        )
        .await;
    let hidden = p.world.topic("Not shared").await;
    p.world
        .document_beside(hidden, "Cookie secret", &["Cookie policy 回滚"])
        .await;
    let grants =
        json!({"search_readable":{"scopes":[p.topic],"documents":[],"chats":[],"groups":[]}});
    let kinds = vec!["user_message".to_owned(), "agent_message".to_owned()];
    let tuning = candidates::Tuning {
        scope_candidates: 30000,
        request_windows: None,
        tiers: Default::default(),
    };
    for query in ["cookie", "回滚", "回", "\"cookie policy\""] {
        let ask = rows::Content {
            selected: None,
            items: None,
            continuation: None,
            user: p.world.ada,
            groups: &[],
            member_roots: &[],
            grants: &grants,
            at: chrono::DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            scope: Some(p.world.root),
            chat: None,
            document: None,
            participating: false,
            relevance: true,
            names: true,
            with_file: false,
            mentions_me: None,
            text: query,
            item_kinds: &kinds,
            chat_messages: true,
            document_content: true,
            author: None,
            from: None,
            to: None,
            after: None,
            limit: 50,
        };
        let mut old_position = candidates::Position::default();
        let mut new_position = old_position.clone();
        let mut ended = false;
        for _ in 0..20 {
            let mut tx = p.world.pool.begin().await.unwrap();
            rows::snapshot(&mut tx).await.unwrap();
            let old = candidates::window(&mut tx, &ask, old_position, 2, 100, 8)
                .await
                .unwrap();
            let fast =
                candidates::window_with_tuning(&mut tx, &ask, new_position, 2, 100, 8, &tuning)
                    .await
                    .unwrap();
            let identities = |parts: &[candidates::Part]| {
                parts
                    .iter()
                    .map(|part| {
                        (
                            part.kind.clone(),
                            part.source_id,
                            part.part_id,
                            part.node_id,
                            part.block_id,
                            part.tier,
                            part.at,
                        )
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                identities(&fast.parts),
                identities(&old.parts),
                "same authorized scope and order for {query}"
            );
            assert_eq!(
                serde_json::to_value(&fast.next).unwrap(),
                serde_json::to_value(&old.next).unwrap(),
                "same next boundary for {query}"
            );
            tx.commit().await.unwrap();
            let (Some(old_next), Some(new_next)) = (old.next, fast.next) else {
                ended = true;
                break;
            };
            old_position = old_next;
            new_position = new_next;
        }
        assert!(ended, "both windows finish the finite fixture for {query}");
    }
}

#[tokio::test]
async fn an_indexed_root_counts_only_its_bounded_parts_while_a_named_chat_still_reads_its_matches()
{
    let p = a_place_to_search().await;
    p.world
        .set(p.world.root, "search.word_candidates", json!(2))
        .await;
    let chat = chat_of(&p.world, p.topic).await;
    for number in 0..12 {
        say_in_chat(
            &p.world,
            chat,
            p.world.ada,
            &format!("needle 回滚 {number}"),
        )
        .await;
    }
    for query in ["needle", "回滚", "回"] {
        let root = ok(search(&p, json!({"query":query,"kinds":["chats"]})).await);
        assert_eq!(hits(&root).len(), 1);
        assert_eq!(hits(&root)[0]["match_count"], 2);
        assert_eq!(hits(&root)[0]["match_count_partial"], true);
        assert_eq!(root["counts_complete"], false);
        assert!(
            root["searched"]["bytes"].as_i64().unwrap() < 100,
            "an indexed candidate must not reopen all matching messages in its chat: {root}"
        );
        let path = hits(&root)[0]["path"].as_str().unwrap();
        let named = ok(search(&p, json!({"query":query,"path":path,"kinds":["chats"]})).await);
        assert_eq!(hits(&named)[0]["match_count"], 12);
        assert_eq!(named["counts_complete"], true);
    }
}

#[tokio::test]
async fn five_matches_in_one_chat_are_one_entry_with_a_count_and_native_positions_without_default_snippets()
 {
    let p = a_place_to_search().await;
    let chat = chat_of(&p.world, p.topic).await;
    for n in 0..5 {
        say_in_chat(&p.world, chat, p.world.ada, &format!("needle number {n}")).await;
    }
    let all = ok(search(&p, json!({ "query": "needle", "max_results": 50 })).await);
    assert_eq!(hits(&all).len(), 1);
    assert_eq!(hits(&all)[0]["match_count"], 5);
    assert_eq!(hits(&all)[0]["positions"], json!([3, 4, 5, 6, 7]));
    assert!(hits(&all)[0].get("snippet").is_none());
    assert!(hits(&all)[0].get("snippets").is_none());
    assert_eq!(all["counts_complete"], true);
    assert_eq!(all["truncated"], false);
    let some = ok(search(&p, json!({ "query": "needle", "max_results": 2 })).await);
    assert_eq!(hits(&some).len(), 1);
    assert_eq!(
        some["truncated"], false,
        "the limit counts entries rather than message hits"
    );
}

#[tokio::test]
async fn requested_snippets_highlight_the_visible_term_when_another_required_word_is_beyond_the_snippet()
 {
    let p = a_place_to_search().await;
    let body = format!("needle {} farword", "x ".repeat(400));
    p.world
        .document_beside(p.topic, "Long match", &[&body])
        .await;
    let found = ok(search(
        &p,
        json!({"query":"farword needle","kinds":["documents"],"snippets":true}),
    )
    .await);
    let row = hits(&found)
        .into_iter()
        .find(|hit| hit["kind"] == "document")
        .unwrap();
    let shown: Vec<_> = row["snippet"].as_str().unwrap().chars().collect();
    assert!(!row["snippet"].as_str().unwrap().contains("farword"));
    let marks = row["matches"].as_array().unwrap();
    assert!(!marks.is_empty());
    let from = marks[0][0].as_u64().unwrap() as usize;
    let to = marks[0][1].as_u64().unwrap() as usize;
    assert_eq!(shown[from..to].iter().collect::<String>(), "needle");
}

#[tokio::test]
async fn newest_entries_are_kept_before_the_limit_and_a_cursor_returns_every_older_entry_without_repeating()
 {
    let p = a_place_to_search().await;
    let mut made = Vec::new();
    for (name, at) in [
        ("Old", "2026-10-10T00:00:00Z"),
        ("Middle", "2026-10-11T00:00:00Z"),
        ("Newest", "2026-10-12T00:00:00Z"),
    ] {
        let document = p
            .world
            .document_beside(p.topic, name, &["A needle belongs here."])
            .await;
        let at: chrono::DateTime<chrono::Utc> = at.parse().unwrap();
        sqlx::query("UPDATE document_blocks SET updated_at=$2 WHERE document_id=$1")
            .bind(document)
            .bind(at)
            .execute(&mut *p.world.pool.acquire().await.unwrap())
            .await
            .unwrap();
        made.push(document);
    }
    let first = ok(search(
        &p,
        json!({"query":"needle","kinds":["documents"],"max_results":1}),
    )
    .await);
    assert_eq!(hits(&first)[0]["id"], json!(made[2]));
    assert_eq!(first["partial"], true);
    let cursor = first["next_cursor"].as_str().unwrap();
    let second = ok(search(
        &p,
        json!({"query":"needle","kinds":["documents"],"max_results":1,"cursor":cursor}),
    )
    .await);
    assert_eq!(hits(&second)[0]["id"], json!(made[1]));
    let third = ok(search(&p, json!({"query":"needle","kinds":["documents"],"max_results":1,"cursor":second["next_cursor"]})).await);
    assert_eq!(hits(&third)[0]["id"], json!(made[0]));
    assert!(third["next_cursor"].is_null());
    assert_eq!(third["partial"], false);
    let changed = search(
        &p,
        json!({"query":"another","kinds":["documents"],"max_results":1,"cursor":cursor}),
    )
    .await;
    assert_eq!(refused(&changed).0, "invalid");
    // A byte-cut reader can resume after its last displayed row even when the engine's page has more rows.
    let full = ok(search(&p, json!({"query":"needle","kinds":["documents"]})).await);
    let rest = ok(search(
        &p,
        json!({"query":"needle","kinds":["documents"],"cursor":hits(&full)[0]["cursor"]}),
    )
    .await);
    assert_eq!(hits(&rest).len(), 2);
    assert!(hits(&rest).iter().all(|hit| hit["id"] != json!(made[2])));
}

#[tokio::test]
async fn an_empty_bounded_regex_window_still_has_a_cursor_that_advances_to_the_older_match() {
    const SCAN_BYTES: i64 = 1_024;
    const NON_MATCH_BYTES: usize = 2_048;
    let p = a_place_to_search().await;
    p.world
        .set(
            p.world.project,
            "sharing.snapshot_max_bytes",
            json!(SCAN_BYTES),
        )
        .await;
    let matched = p
        .world
        .document_beside(p.topic, "Old result", &["The needle is here."])
        .await;
    sqlx::query(
        "UPDATE document_blocks SET updated_at='2026-10-10T00:00:00Z' WHERE document_id=$1",
    )
    .bind(matched)
    .execute(&mut *p.world.pool.acquire().await.unwrap())
    .await
    .unwrap();
    for n in 1..=3 {
        let junk = p
            .world
            .document_beside(
                p.topic,
                &format!("New nonmatch {n}"),
                &[&"x".repeat(NON_MATCH_BYTES)],
            )
            .await;
        let at: chrono::DateTime<chrono::Utc> = format!("2026-10-10T00:00:0{n}Z").parse().unwrap();
        sqlx::query("UPDATE document_blocks SET updated_at=$2 WHERE document_id=$1")
            .bind(junk)
            .bind(at)
            .execute(&mut *p.world.pool.acquire().await.unwrap())
            .await
            .unwrap();
    }
    let mut cursors = std::collections::HashSet::new();
    let mut cursor: Option<String> = None;
    let mut empty = false;
    let mut found = false;
    let mut ended = false;
    // Three nonmatching large rows plus the older match must finish in this finite number of windows.
    for _ in 0..8 {
        let mut args = json!({"query":"(needle|never)","regex":true,"kinds":["documents"]});
        if let Some(cursor) = &cursor {
            args["cursor"] = json!(cursor);
        }
        let page = ok(search(&p, args).await);
        empty |= hits(&page).is_empty() && !page["next_cursor"].is_null();
        found |= hits(&page).iter().any(|hit| hit["id"] == json!(matched));
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        match &cursor {
            Some(next) => assert!(cursors.insert(next.clone()), "a continuation never repeats"),
            None => {
                ended = true;
                break;
            }
        }
    }
    assert!(
        empty && found && ended,
        "empty={empty}, found={found}, ended={ended}"
    );
}

#[tokio::test]
async fn a_search_cursor_never_keeps_the_authority_of_a_share_that_was_taken_back() {
    const SCAN_BYTES: i64 = 1_024;
    const PADDING_BYTES: usize = 600;
    let world = a_world_whose_agent_takes_tools().await;
    world
        .set(
            world.project,
            "sharing.snapshot_max_bytes",
            json!(SCAN_BYTES),
        )
        .await;
    let reader = world.thread("Reader").await;
    let topic = world.topic("Records").await;
    let body = format!("needle {}", "x".repeat(PADDING_BYTES));
    let revoked = format!("revoked {body}");
    let first = world.document_beside(topic, "One", &[&body]).await;
    let second = world.document_beside(topic, "Two", &[&body]).await;
    let third = world.document_beside(topic, "Three", &[&revoked]).await;
    let fourth = world.document_beside(topic, "Four", &[&body]).await;
    for (offset, document) in [first, second, third, fourth].into_iter().enumerate() {
        let at = world.clock.now() + chrono::Duration::seconds(i64::try_from(offset).unwrap());
        sqlx::query("UPDATE document_blocks SET updated_at=$2 WHERE document_id=$1")
            .bind(document)
            .bind(at)
            .execute(&mut *world.pool.acquire().await.unwrap())
            .await
            .unwrap();
    }
    share(&world, "document", first, "live", "doc.view", "any", reader).await;
    share(
        &world, "document", second, "live", "doc.view", "any", reader,
    )
    .await;
    let grant = share(&world, "document", third, "live", "doc.view", "any", reader).await;
    share(
        &world, "document", fourth, "live", "doc.view", "any", reader,
    )
    .await;
    let page = ok(call(
        &world,
        reader,
        OrchTool::SearchShared,
        json!({"query":"needle","kinds":["documents"],"max_results":1}),
    )
    .await);
    assert_eq!(hits(&page).len(), 1);
    // The first window crosses its byte bound after the two newest entries; its endpoint is Three.
    assert_eq!(hits(&page)[0]["id"], json!(fourth));
    assert!(page["next_cursor"].is_string());
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::access::remove_entry(&mut conn, grant, world.ada, world.clock.now())
        .await
        .unwrap();
    drop(conn);
    let after = ok(call(&world, reader, OrchTool::SearchShared, json!({"query":"needle","kinds":["documents"],"max_results":1,"cursor":page["next_cursor"]})).await);
    assert!(
        hits(&after).is_empty(),
        "the removed window endpoint is not replaced by an older entry from the next window"
    );
    assert!(after["next_cursor"].is_string());
    let shown = after.to_string();
    assert!(!shown.contains(&third.to_string()));
    assert!(!shown.contains("revoked needle"));
    let older = ok(call(&world, reader, OrchTool::SearchShared, json!({"query":"needle","kinds":["documents"],"max_results":1,"cursor":after["next_cursor"]})).await);
    assert_eq!(hits(&older)[0]["id"], json!(second));
    let oldest = ok(call(&world, reader, OrchTool::SearchShared, json!({"query":"needle","kinds":["documents"],"max_results":1,"cursor":older["next_cursor"]})).await);
    assert_eq!(hits(&oldest)[0]["id"], json!(first));
    assert!(oldest["next_cursor"].is_null());
}

#[tokio::test]
async fn newest_search_entries_have_a_real_continuation_instead_of_losing_older_matches() {
    let p = a_place_to_search().await;
    let mut expected = Vec::new();
    for number in 1..=3 {
        let doc = p
            .world
            .document_beside(p.topic, &format!("Record {number}"), &["qf03needle"])
            .await;
        let at =
            p.world.clock.now() - chrono::Duration::hours(1) + chrono::Duration::seconds(number);
        let mut conn = p.world.pool.acquire().await.unwrap();
        sqlx::query("UPDATE document_blocks SET updated_at=$2 WHERE document_id=$1")
            .bind(doc)
            .bind(at)
            .execute(&mut *conn)
            .await
            .unwrap();
        expected.push(doc);
    }
    expected.reverse();
    let mut cursor: Option<String> = None;
    let mut seen = Vec::new();
    loop {
        let mut arguments = json!({"query":"qf03needle","kinds":["documents"],"max_results":1});
        if let Some(cursor) = &cursor {
            arguments["cursor"] = json!(cursor);
        }
        let page = ok(search(&p, arguments).await);
        let rows = hits(&page);
        assert_eq!(rows.len(), 1);
        let doc: Id = serde_json::from_value(rows[0]["id"].clone()).unwrap();
        assert!(!seen.contains(&doc));
        seen.push(doc);
        let next = page["next_cursor"].as_str().map(str::to_owned);
        assert_eq!(page["partial"], json!(next.is_some()));
        assert_eq!(page["truncated"], page["partial"]);
        if let Some(next) = next {
            let changed = search(&p, json!({"query":"another query","kinds":["documents"],"max_results":1,"cursor":next})).await;
            assert_eq!(
                refused(&changed).0,
                "invalid",
                "a cursor carries no search authority"
            );
            cursor = Some(next);
        } else {
            break;
        }
    }
    assert_eq!(
        seen, expected,
        "a small result cap keeps the newest, and every older entry remains reachable"
    );
}

/// Gives a document, its blocks or a block that holds a text a last change at a moment of the test's choosing.
async fn changed_at(world: &common::World, document: Id, at: &str) {
    let at: chrono::DateTime<chrono::Utc> = at.parse().unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE documents SET updated_at=$2 WHERE id=$1")
        .bind(document)
        .bind(at)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE document_blocks SET updated_at=$2 WHERE document_id=$1")
        .bind(document)
        .bind(at)
        .execute(&mut *conn)
        .await
        .unwrap();
}

#[tokio::test]
async fn entries_rank_by_title_description_heading_and_body_then_how_many_rows_matched_then_newest_in_any_script()
 {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let topic = world.topic("Ranked").await;
    share(&world, "node", topic, "live", "doc.view", "any", reader).await;
    for (words, filler) in [("发布流程", "说明"), ("rollout", "notes")] {
        let title = world
            .document_beside(topic, &format!("{words} 一"), &[filler])
            .await;
        let described = world
            .document_beside(topic, &format!("{filler} 二"), &[filler])
            .await;
        let heading = world
            .document_beside(topic, &format!("{filler} 三"), &[words, filler])
            .await;
        let often = world
            .document_beside(topic, &format!("{filler} 四"), &[words, words, words])
            .await;
        let once = world
            .document_beside(topic, &format!("{filler} 五"), &[words])
            .await;
        let mut conn = world.pool.acquire().await.unwrap();
        sqlx::query("UPDATE documents SET description=$2, description_source='user' WHERE id=$1")
            .bind(described)
            .bind(format!("关于 {words} 的说明"))
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE document_blocks SET type='heading', props='{\"level\":1}' WHERE document_id=$1 AND text=$2")
            .bind(heading)
            .bind(words)
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        // The more telling a field, the older its document: recency alone would give the reverse order.
        for (document, at) in [
            (title, "2026-10-01T00:00:00Z"),
            (described, "2026-10-02T00:00:00Z"),
            (heading, "2026-10-03T00:00:00Z"),
            (often, "2026-10-04T00:00:00Z"),
            (once, "2026-10-05T00:00:00Z"),
        ] {
            changed_at(&world, document, at).await;
        }
        let found = ok(call(
            &world,
            reader,
            OrchTool::SearchShared,
            json!({"query":words,"kinds":["documents"]}),
        )
        .await);
        let order: Vec<Value> = hits(&found).iter().map(|hit| hit["id"].clone()).collect();
        assert_eq!(
            order,
            vec![
                json!(title),
                json!(described),
                json!(heading),
                json!(often),
                json!(once)
            ],
            "{words}: title, description, heading, then the body with more matching rows, then the newest"
        );
        assert_eq!(hits(&found)[3]["match_count"], 3);
        // One entry at a time follows the same order and repeats none.
        let mut cursor: Option<String> = None;
        let mut paged = Vec::new();
        for _ in 0..6 {
            let mut arguments = json!({"query":words,"kinds":["documents"],"max_results":1});
            if let Some(cursor) = &cursor {
                arguments["cursor"] = json!(cursor);
            }
            let page = ok(call(&world, reader, OrchTool::SearchShared, arguments).await);
            paged.extend(hits(&page).iter().map(|hit| hit["id"].clone()));
            cursor = page["next_cursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(paged, order, "{words}: pages follow the ranking");
    }
}
