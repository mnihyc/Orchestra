//! Real tool calls resolve paths faithfully, refuse ambiguous writes and keep same-named hidden content private.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use orch_link_contract::commands::OrchTool;
use orch_types::Id;
use serde_json::json;
use shared::*;

/// Equal visible titles remain valid; their stored directory basenames must be distinct.
async fn title_twin(world: &common::World, node: Id, title: &str, text: &str) -> Id {
    use orch_api_contract::documents::{DocumentOp, NewBlock};
    use orch_db::queries::documents::{self as rows, batch};
    let mut conn = world.pool.acquire().await.unwrap();
    let id = orch_types::new_id();
    let org: Id = sqlx::query_scalar("SELECT org_id FROM nodes WHERE id=$1")
        .bind(node)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let now = chrono::Utc::now();
    let basename = format!("{title}~{}.md", &id.simple().to_string()[28..]);
    rows::containers::insert_document(
        &mut conn,
        &rows::NewDocument {
            id,
            node_id: node,
            org_id: org,
            owner_id: world.ada,
            created_by_thread: None,
            created_by_agent: None,
            title,
            now,
        },
        None,
        &basename,
    )
    .await
    .unwrap();
    orch_authz::documents::start(&mut conn, id, world.ada, orch_types::Time::new(now))
        .await
        .unwrap();
    let block: NewBlock = serde_json::from_value(
        json!({"id":orch_types::new_id(),"type":"paragraph","props":{},
        "content":[{"type":"text","text":text}]}),
    )
    .unwrap();
    let outcome = batch::apply(
        &mut conn,
        id,
        &orch_documents::versions::Writer::User(world.ada),
        None,
        &[DocumentOp::Insert {
            block,
            parent_id: None,
            after_id: None,
        }],
        batch::Limits {
            max_bytes: 2_097_152,
            max_blocks: 5_000,
        },
        batch::Rules {
            window_secs: 600,
            kept: 200,
            history_max_bytes: 16_777_216,
        },
        now,
    )
    .await
    .unwrap();
    assert!(outcome.is_kept(), "{outcome:?}");
    id
}

#[tokio::test]
async fn a_foreign_path_never_delivers_to_the_same_named_thread_in_the_senders_topic() {
    let world = a_world_whose_agent_takes_tools().await;
    let sender = world.thread("Sender").await;
    let local = world.thread("Deploy").await;
    let topic = world.topic("Payments").await;
    let remote = thread_in_topic(&world, topic, "Deploy").await;
    share_threads_inside(&world, topic, sender, true).await;
    let listing = ok(call(
        &world,
        sender,
        OrchTool::ListShared,
        json!({"path":format!("id:{topic}")}),
    )
    .await);
    let path = entry(&listing, "Deploy")["path"].as_str().unwrap();
    ok(call(
        &world,
        sender,
        OrchTool::SendMessage,
        json!({"target": { "kind": "thread", "path": path },"text":"Only the payments thread."}),
    )
    .await);
    let targets: Vec<Id> =
        sqlx::query_scalar("SELECT to_thread_id FROM announcements WHERE from_thread_id=$1")
            .bind(sender)
            .fetch_all(&mut *world.pool.acquire().await.unwrap())
            .await
            .unwrap();
    assert_eq!(targets, vec![remote]);
    assert!(!targets.contains(&local));
    let absent = path.replace("/Payments/", "/No such topic/");
    let denied = call(
        &world,
        sender,
        OrchTool::SendMessage,
        json!({"target": { "kind": "thread", "path": absent },"text":"Do not fall back."}),
    )
    .await;
    assert_eq!(refused(&denied).0, "not_found");
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM announcements WHERE from_thread_id=$1",
                sender
            )
            .await,
        1
    );
    ok(call(&world, sender, OrchTool::SendMessage, json!({"target": { "kind": "thread", "path": "dEpLoY" },"text":"A unique local name also accepts case differences."})).await);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM announcements WHERE to_thread_id=$1",
                local
            )
            .await,
        1
    );
}

#[tokio::test]
async fn a_duplicate_thread_name_is_a_read_with_one_notice_and_a_write_with_candidates_until_an_id_is_given()
 {
    let world = a_world_whose_agent_takes_tools().await;
    let sender = world.thread("Sender").await;
    let first = world.thread("Review").await;
    let second = world.thread("Review").await;
    share_threads_inside(&world, world.topic, sender, true).await;
    talk(&world, first, "First candidate opening", "Ready.").await;
    talk(&world, second, "Second candidate opening", "Ready.").await;
    the_agent_takes_tools_again(&world).await;
    sqlx::query("UPDATE nodes SET archived_at=$2 WHERE id=$1")
        .bind(first)
        .bind(world.clock.now())
        .execute(&mut *world.pool.acquire().await.unwrap())
        .await
        .unwrap();
    let denied = call(&world, sender, OrchTool::SendMessage, json!({"target": { "kind": "thread", "path": "Review" },"text":"Choose no thread silently."})).await;
    let (code, message) = refused(&denied);
    assert_eq!(code, "ambiguous");
    for text in [
        first.to_string(),
        second.to_string(),
        "First candidate opening".into(),
        "Second candidate opening".into(),
        "idle".into(),
        "archived".into(),
    ] {
        assert!(message.contains(&text), "{message}");
    }
    let candidates: Vec<_> = message
        .lines()
        .filter(|line| line.contains(" · id:"))
        .collect();
    assert_eq!(candidates.len(), 2, "{message}");
    for candidate in candidates {
        let facts: Vec<_> = candidate.split(" · ").collect();
        assert!(
            facts[0].starts_with("\"/") && facts[0].contains("/Review"),
            "{candidate}"
        );
        assert!(
            facts[2].parse::<chrono::DateTime<chrono::Utc>>().is_ok(),
            "a candidate gives its actual change time: {candidate}"
        );
        assert!(facts[3].contains("candidate opening"), "{candidate}");
    }
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM announcements WHERE from_thread_id=$1",
                sender
            )
            .await,
        0
    );
    let read = ok(call(
        &world,
        sender,
        OrchTool::ReadShared,
        json!({"path":"Review"}),
    )
    .await);
    assert!(read["text"].as_str().unwrap().contains("candidate opening"));
    let notice = read["also_named_so"].as_str().unwrap();
    assert!(notice.starts_with("Also named so:"));
    assert_eq!(notice.lines().count(), 1);
    ok(call(&world, sender, OrchTool::SendMessage, json!({"target": { "kind": "thread", "path": format!("id:{second}") },"text":"The explicit choice."})).await);
    let target: Id =
        sqlx::query_scalar("SELECT to_thread_id FROM announcements WHERE from_thread_id=$1")
            .bind(sender)
            .fetch_one(&mut *world.pool.acquire().await.unwrap())
            .await
            .unwrap();
    assert_eq!(target, second);
}

#[tokio::test]
async fn listing_and_sending_use_the_same_reserved_trimmed_name_and_random_id_tail() {
    let world = a_world_whose_agent_takes_tools().await;
    let sender = world.thread("Sender").await;
    let target = world.thread("  docs  ").await;
    world.thread("Docs").await;
    world
        .document_beside(world.topic, "Guide", &["The real document directory."])
        .await;
    share(
        &world,
        "node",
        world.topic,
        "live",
        "doc.view",
        "any",
        sender,
    )
    .await;
    let listing = ok(call(&world, sender, OrchTool::ListShared, json!({"ids":true})).await);
    let expected = format!("docs~{}", &target.simple().to_string()[28..]);
    let named = entry(&listing, &expected);
    assert_eq!(named["id"], json!(target));
    let sent = ok(call(
        &world,
        sender,
        OrchTool::SendMessage,
        json!({"target": { "kind": "thread", "path": expected },"text":"Use the listed name."}),
    )
    .await);
    assert_eq!(sent["target"]["path"], named["path"]);
    let docs = ok(call(&world, sender, OrchTool::ListShared, json!({"path":"docs"})).await);
    assert_eq!(entry(&docs, "Guide.md")["kind"], "document");
}

#[tokio::test]
async fn document_reads_report_twins_but_edit_and_manage_refuse_before_changing_either_document() {
    let world = a_world_whose_agent_takes_tools().await;
    let sender = world.thread("Reader").await;
    let topic = world.topic("Design").await;
    let first = world
        .document_beside(topic, "Plan", &["First document opening."])
        .await;
    let second = title_twin(&world, topic, "Plan", "Second document opening.").await;
    for permission in ["doc.view", "doc.edit", "doc.manage"] {
        share(&world, "node", topic, "live", permission, "any", sender).await;
    }
    let listing = ok(call(
        &world,
        sender,
        OrchTool::ListShared,
        json!({"path":format!("id:{topic}")}),
    )
    .await);
    let path = format!(
        "{}Plan.md",
        entry(&listing, "docs")["path"].as_str().unwrap()
    );
    let read = ok(call(&world, sender, OrchTool::ReadShared, json!({"path":path})).await);
    assert!(
        read["also_named_so"]
            .as_str()
            .unwrap()
            .contains("document opening")
    );
    let edited = call(&world, sender, OrchTool::EditDocument, json!({"command":"replace","path":path,"old":"opening","new":"changed","seq":read["entry"]["seq"]})).await;
    assert_eq!(refused(&edited).0, "ambiguous");
    let managed = call(
        &world,
        sender,
        OrchTool::ManageShared,
        json!({"command":"delete","paths":[path]}),
    )
    .await;
    assert_eq!(refused(&managed).0, "ambiguous");
    for id in [first, second] {
        assert_eq!(world.count("SELECT count(*) FROM document_blocks WHERE document_id=$1 AND text LIKE '%opening%' AND removed_at IS NULL", id).await, 1);
        assert_eq!(
            world
                .count(
                    "SELECT count(*) FROM documents WHERE id=$1 AND deleted_at IS NULL",
                    id
                )
                .await,
            1
        );
    }
    world.add_paragraph(first, "A later change.").await;
    world.add_paragraph(second, "Another later change.").await;
    let history = format!(
        "{}Plan.history/1",
        entry(&listing, "docs")["path"].as_str().unwrap()
    );
    let old = ok(call(
        &world,
        sender,
        OrchTool::ReadShared,
        json!({"path":history}),
    )
    .await);
    assert!(old["also_named_so"].as_str().unwrap().contains("Plan~"));
    let refused_restore = call(
        &world,
        sender,
        OrchTool::ManageShared,
        json!({"command":"restore","paths":[history]}),
    )
    .await;
    assert_eq!(refused(&refused_restore).0, "ambiguous");
}

#[tokio::test]
async fn a_hidden_namesake_supplies_neither_an_ambiguity_candidate_nor_its_first_line() {
    let world = a_world_whose_agent_takes_tools().await;
    let sender = world.thread("Reader").await;
    let topic = world.topic("Secrets").await;
    let hidden = world
        .document_beside(topic, "Plan", &["Never expose this hidden opening."])
        .await;
    let visible = title_twin(&world, topic, "Plan", "The shared opening.").await;
    share(
        &world, "document", visible, "live", "doc.view", "any", sender,
    )
    .await;
    let listed = ok(call(
        &world,
        sender,
        OrchTool::ListShared,
        json!({"path":format!("id:{topic}")}),
    )
    .await);
    let path = format!(
        "{}Plan.md",
        entry(&listed, "docs")["path"].as_str().unwrap()
    );
    let read = ok(call(
        &world,
        sender,
        OrchTool::ReadShared,
        json!({"path":path,"ids":true}),
    )
    .await);
    assert_eq!(read["entry"]["id"], json!(visible));
    assert!(read.get("also_named_so").is_none());
    assert!(!read.to_string().contains("Never expose"));
    assert!(!read.to_string().contains(&hidden.to_string()));
    // Manage includes read, but a deleted namesake is listed only for restoration, never read as live content.
    share(
        &world,
        "document",
        hidden,
        "live",
        "doc.manage",
        "any",
        sender,
    )
    .await;
    sqlx::query("UPDATE documents SET deleted_at=$2,deleted_by=$3 WHERE id=$1")
        .bind(hidden)
        .bind(world.clock.now())
        .bind(world.ada)
        .execute(&mut *world.pool.acquire().await.unwrap())
        .await
        .unwrap();
    let live_read = ok(call(
        &world,
        sender,
        OrchTool::ReadShared,
        json!({"path":path,"ids":true}),
    )
    .await);
    assert_eq!(live_read["entry"]["id"], json!(visible));
    assert!(live_read.get("also_named_so").is_none());
    assert!(!live_read.to_string().contains("Never expose"));
}
