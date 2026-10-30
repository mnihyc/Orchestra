//! What an agent lists of the shared tree: its own topic until it is told where to look, the places on the way to what it reaches, each entry with whether it may be read and written and why it is there, and nothing of what it may not read.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_link_contract::commands::OrchTool;
use serde_json::json;
use shared::*;

#[tokio::test]
async fn an_agent_with_nothing_shared_lists_its_own_topic_and_the_threads_beside_it_and_nothing_more()
 {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Repro").await;
    let beside = world.thread("Docs").await;
    let result = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({"ids":true,"sort":"name"}),
    )
    .await);
    let path = result["path"].as_str().unwrap();
    assert!(path.ends_with("/Web/Login/"), "{path}");
    assert_eq!(result["about"]["kind"], "topic");
    assert_eq!(result["about"]["name"], "Login");
    assert_eq!(result["about"]["status"], "open");
    assert_eq!(result["truncated"], false);
    let entries = result["entries"].as_array().unwrap();
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Docs", "Repro", "docs"],
        "the threads of the topic, by name: {entries:?}"
    );
    let other = entry(&result, "Docs");
    assert_eq!(other["kind"], "thread");
    assert_eq!(other["id"], json!(beside));
    assert_eq!(
        other["readable"], false,
        "it may not read what nobody shared"
    );
    assert_eq!(
        other["writable"], true,
        "it may always hand something to a thread of its own topic"
    );
    assert_eq!(other["via"], "topic");
    let own = entry(&result, "Repro");
    assert_eq!(own["is_self"], true);
    assert_eq!(
        own["readable"], true,
        "its own conversation is its own to read"
    );
    assert_eq!(own["writable"], false);
    assert!(
        entries
            .iter()
            .all(|e| e["kind"] == "thread" || (e["kind"] == "folder" && e["name"] == "docs")),
        "no chat or foreign content; the topic policy supplies its document directory"
    );
    let docs = entry(&result, "docs");
    assert_eq!(docs["via"], "policy");
    assert_eq!(docs["children"], 0);
    assert_eq!(
        docs["writable"], true,
        "the shipped document policy permits creation without an explicit share"
    );
    assert!(
        docs.get("manageable").is_none(),
        "the policy grants no document management"
    );
}

#[tokio::test]
async fn the_top_of_the_tree_lists_the_places_on_the_way_to_what_the_agent_reaches() {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Repro").await;
    let result = ok(call(&world, mine, OrchTool::ListShared, json!({ "path": "/" })).await);
    assert_eq!(result["path"], "/");
    assert!(result["about"].is_null(), "the top is no place");
    let entries = result["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "only the org the agent works in: {entries:?}"
    );
    assert_eq!(entries[0]["kind"], "org");
    assert!(entries[0]["name"].as_str().unwrap().starts_with("acme-"));
    let deep = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": "/", "depth": 4 }),
    )
    .await);
    let found = paths(&deep);
    assert!(
        found.iter().any(|p| p.ends_with("/Web/Login/Repro/")),
        "{found:?}"
    );
    let thread = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": "Repro" }),
    )
    .await);
    assert!(
        paths(&thread)
            .iter()
            .any(|p| p.ends_with("/Repro/conversation.md")),
        "its own conversation is in the tree: {thread}"
    );
}

#[tokio::test]
async fn a_chat_shared_for_reading_is_listed_readable_and_not_writable_and_one_shared_for_writing_is_writable()
 {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Repro").await;
    let other_topic = world.topic("Docs").await;
    share(
        &world,
        "node",
        other_topic,
        "live",
        "chat.view",
        "any",
        mine,
    )
    .await;
    let result = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": "/", "depth": 4, "kinds": ["chat"] }),
    )
    .await);
    let chats = result["entries"].as_array().unwrap();
    assert_eq!(chats.len(), 1, "{chats:?}");
    assert!(
        chats[0]["path"]
            .as_str()
            .unwrap()
            .ends_with("/Web/Docs/chat.md")
    );
    assert_eq!(chats[0]["readable"], true);
    assert_eq!(
        chats[0]["writable"], false,
        "a share for reading does not let it write"
    );
    assert_eq!(chats[0]["via"], "share");
    assert_eq!(chats[0]["unit"], "message");
    share(
        &world,
        "node",
        other_topic,
        "live",
        "chat.interact",
        "any",
        mine,
    )
    .await;
    let result = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": "/", "depth": 4, "kinds": ["chat"] }),
    )
    .await);
    assert_eq!(result["entries"][0]["writable"], true);
}

#[tokio::test]
async fn a_place_shared_with_agents_appears_with_what_is_inside_it_and_the_places_on_the_way_are_directories_with_nothing_else()
 {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Repro").await;
    // Another project, shared for its conversations with the agents of this one's topic.
    let mobile = world.project_in_root("Mobile").await;
    let app = world.topic_in(mobile, "App").await;
    let build = world.thread_in(app, "Build").await;
    share(
        &world,
        "node",
        mobile,
        "defaults",
        "thread.view",
        "managed",
        world.topic,
    )
    .await;
    share(
        &world,
        "node",
        mobile,
        "defaults",
        "thread.view",
        "connected",
        world.topic,
    )
    .await;
    let result = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": "/", "depth": 4, "names": "B*", "ids": true }),
    )
    .await);
    let found = result["entries"].as_array().unwrap();
    let thread = found
        .iter()
        .find(|e| e["name"] == "Build")
        .expect("the shared thread is in the tree");
    assert_eq!(thread["readable"], true);
    assert_eq!(
        thread["writable"], false,
        "it may read there and not send there"
    );
    assert_eq!(thread["via"], "share");
    assert_eq!(thread["id"], json!(build));
    // The directories on the way to it are there.
    let all = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": "/", "depth": 4 }),
    )
    .await);
    let found = paths(&all);
    assert!(found.iter().any(|p| p.ends_with("/Mobile/")), "{found:?}");
    assert!(
        found.iter().any(|p| p.ends_with("/Mobile/App/Build/")),
        "{found:?}"
    );
    let build_dir = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": format!("id:{build}") }),
    )
    .await);
    assert!(
        paths(&build_dir)
            .iter()
            .any(|p| p.ends_with("/Mobile/App/Build/conversation.md")),
        "{build_dir}"
    );
}

#[tokio::test]
async fn what_the_agent_may_not_read_is_not_listed_and_asked_for_it_answers_as_what_is_not_there() {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Repro").await;
    let secret_project = world.project_in_root("Secret").await;
    let secret_topic = world.topic_in(secret_project, "Plans").await;
    let secret = world.thread_in(secret_topic, "Ledger").await;
    let top = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": "/", "depth": 4 }),
    )
    .await);
    let text = top.to_string();
    assert!(
        !text.contains("Secret") && !text.contains("Ledger"),
        "{text}"
    );
    let asked = call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": "id:".to_owned() + &secret.to_string() }),
    )
    .await;
    let (code, message) = refused(&asked);
    assert_eq!(code, "not_found");
    let missing = call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": "/nothing/here" }),
    )
    .await;
    let (missing_code, missing_message) = refused(&missing);
    assert_eq!(missing_code, "not_found");
    assert!(!message.contains("Secret") && !missing_message.contains("Secret"));
    let hidden = call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": format!("/{}/Secret/Plans", top["entries"][0]["name"].as_str().unwrap()) }),
    )
    .await;
    let (hidden_code, hidden_message) = refused(&hidden);
    assert_eq!(
        hidden_code, "not_found",
        "a place it cannot see answers as one that is not there"
    );
    assert!(!hidden_message.contains("Ledger"), "{hidden_message}");
    let nowhere = call(&world, mine, OrchTool::ListShared, json!({ "path": format!("/{}/Nothing/Plans", top["entries"][0]["name"].as_str().unwrap()) })).await;
    assert_eq!(
        hidden_message.replace("Secret", "Nothing"),
        refused(&nowhere).1,
        "a place that is there and is not the agent's to see answers exactly as one that is not there"
    );
}

#[tokio::test]
async fn a_refusal_for_a_path_that_names_nothing_gives_the_closest_names_of_its_directory() {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Repro").await;
    world.thread("Fix login").await;
    let asked = call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": "Fix logn" }),
    )
    .await;
    let (code, message) = refused(&asked);
    assert_eq!(code, "not_found");
    assert!(
        message.contains("\"Fix login\"") && message.contains("\"Repro\""),
        "{message}"
    );
}

#[tokio::test]
async fn two_entries_of_one_name_in_a_directory_have_names_that_differ() {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Repro").await;
    let first = world.thread("Docs").await;
    let second = world.thread("Docs").await;
    let result = ok(call(&world, mine, OrchTool::ListShared, json!({})).await);
    let names: Vec<&str> = result["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Docs"), "{names:?}");
    let twin = names
        .iter()
        .find(|name| name.starts_with("Docs~"))
        .expect("the younger has a suffix");
    let second_id = second.simple().to_string();
    assert!(twin[5..].chars().all(|c| second_id.contains(c)));
    // Each name finds its own thread, and `id:` finds either.
    let by_name = ok(call(&world, mine, OrchTool::ListShared, json!({ "path": *twin })).await);
    assert!(
        by_name["path"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/{twin}/"))
    );
    let by_id = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": format!("id:{first}") }),
    )
    .await);
    assert!(by_id["path"].as_str().unwrap().ends_with("/Docs/"));
}

#[tokio::test]
async fn kinds_and_names_narrow_a_listing_and_a_file_is_no_directory() {
    let world = a_world_whose_agent_takes_tools().await;
    let mine = world.thread("Repro").await;
    world.thread("Fix login").await;
    let only = ok(call(&world, mine, OrchTool::ListShared, json!({ "names": "F*" })).await);
    assert_eq!(only["entries"].as_array().unwrap().len(), 1);
    let none = ok(call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "kinds": ["document"] }),
    )
    .await);
    assert!(none["entries"].as_array().unwrap().is_empty());
    let file = call(
        &world,
        mine,
        OrchTool::ListShared,
        json!({ "path": "Repro/conversation.md" }),
    )
    .await;
    let (code, message) = refused(&file);
    assert_eq!(code, "wrong_kind");
    assert!(message.contains("read_shared"));
    let pattern = call(&world, mine, OrchTool::ListShared, json!({ "names": "[a" })).await;
    assert_eq!(refused(&pattern).0, "invalid");
}
