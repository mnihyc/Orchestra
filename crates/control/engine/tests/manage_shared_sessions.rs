//! Thread lifecycle and chat moderation reuse native domains with current giver-bound plans.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use common::{World, eventually};
use orch_link_contract::commands::{OrchTool, OrchToolResult};
use orch_types::{Id, Json};
use serde_json::json;
use shared::*;

async fn manage(world: &World, agent: Id, body: Json) -> OrchToolResult {
    call(world, agent, OrchTool::ManageShared, body).await
}

async fn setup() -> (World, Id, Id) {
    let world = a_world_whose_agent_takes_tools().await;
    let agent = world.thread("Agent that manages").await;
    let target = world.thread("Shared target").await;
    share(&world, "node", target, "live", "thread.view", "any", agent).await;
    (world, agent, target)
}

fn request(command: &str, target: Id) -> Json {
    json!({"command":command,"paths":[format!("id:{target}")]})
}

async fn confirm(world: &World, agent: Id, mut body: Json) -> Json {
    let plan = ok(manage(world, agent, body.clone()).await);
    assert_eq!(plan["done"], false);
    assert_eq!(plan["refused"], json!([]));
    assert_eq!(plan["confirm"].as_str().unwrap().len(), 32);
    body["confirm"] = plan["confirm"].clone();
    ok(manage(world, agent, body).await)
}

#[tokio::test]
async fn each_thread_act_refuses_the_missing_row_and_manage_confers_archive_and_unarchive() {
    let (world, agent, target) = setup().await;
    for (command, row) in [
        ("archive", "thread.archive"),
        ("unarchive", "thread.archive"),
        ("fork", "thread.fork"),
        ("delete", "thread.manage"),
    ] {
        let result = manage(&world, agent, request(command, target)).await;
        assert_eq!(refused(&result).0, "not_manageable");
        assert!(refused(&result).1.contains(row));
    }
    share(
        &world,
        "node",
        target,
        "live",
        "thread.manage",
        "any",
        agent,
    )
    .await;
    let body = request("archive", target);
    let plan = ok(manage(&world, agent, body.clone()).await);
    let archived: bool =
        sqlx::query_scalar("SELECT archived_at IS NOT NULL FROM nodes WHERE id=$1")
            .bind(target)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(!archived, "preview changes no lifecycle state");
    let mut confirmed = body;
    confirmed["confirm"] = plan["confirm"].clone();
    assert_eq!(
        ok(manage(&world, agent, confirmed.clone()).await)["done"],
        true
    );
    assert_eq!(
        refused(&manage(&world, agent, confirmed).await).0,
        "not_manageable"
    );
    assert_eq!(
        confirm(&world, agent, request("unarchive", target)).await["done"],
        true
    );
    let actor: (String,Id) = sqlx::query_as("SELECT actor_kind,actor_id FROM audit_log WHERE object_id=$1 AND action='thread.unarchived_by_agent'")
        .bind(target).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(actor, ("agent".to_owned(), agent));
}

#[tokio::test]
async fn independent_archive_and_delete_require_confirmation_and_keep_native_recovery() {
    let (world, agent, target) = setup().await;
    share(
        &world,
        "node",
        target,
        "live",
        "thread.archive",
        "any",
        agent,
    )
    .await;
    assert_eq!(
        confirm(&world, agent, request("archive", target)).await["done"],
        true
    );
    assert_eq!(
        confirm(&world, agent, request("unarchive", target)).await["done"],
        true
    );
    assert!(
        refused(&manage(&world, agent, request("delete", target)).await)
            .1
            .contains("thread.manage")
    );
    share(
        &world,
        "node",
        target,
        "live",
        "thread.manage",
        "any",
        agent,
    )
    .await;
    let doc = world
        .document_beside(target, "Kept document", &["Recoverable text"])
        .await;
    assert_eq!(
        confirm(&world, agent, request("delete", target)).await["done"],
        true
    );
    let kept: (bool,bool)=sqlx::query_as("SELECT n.deleted_at IS NOT NULL,d.deleted_at IS NOT NULL FROM nodes n JOIN documents d ON d.node_id=n.id WHERE n.id=$1 AND d.id=$2")
        .bind(target).bind(doc).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(kept, (true, true));
    assert_eq!(world.count("SELECT count(*) FROM audit_log WHERE action='thread.deleted_by_agent' AND object_id=$1",target).await,1);
}

#[tokio::test]
async fn stale_rebound_and_lapsed_giver_confirmations_have_no_effect() {
    let (world, agent, target) = setup().await;
    let entry = share(
        &world,
        "node",
        target,
        "live",
        "thread.archive",
        "any",
        agent,
    )
    .await;
    let mut body = request("archive", target);
    let plan = ok(manage(&world, agent, body.clone()).await);
    body["confirm"] = plan["confirm"].clone();
    let other = world.thread("Other agent").await;
    share(
        &world,
        "node",
        target,
        "live",
        "thread.archive",
        "any",
        other,
    )
    .await;
    assert_eq!(
        refused(&manage(&world, other, body.clone()).await).0,
        "not_confirmed"
    );
    sqlx::query("UPDATE nodes SET revision=revision+1 WHERE id=$1")
        .bind(target)
        .execute(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        refused(&manage(&world, agent, body).await).0,
        "not_confirmed"
    );
    let mut body = request("archive", target);
    let plan = ok(manage(&world, agent, body.clone()).await);
    body["confirm"] = plan["confirm"].clone();
    sqlx::query("UPDATE permission_entries SET expires_at=$2 WHERE id=$1")
        .bind(entry)
        .bind(world.clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    let result = manage(&world, agent, body).await;
    assert!(refused(&result).1.contains("thread.archive"));
    let archived: bool =
        sqlx::query_scalar("SELECT archived_at IS NOT NULL FROM nodes WHERE id=$1")
            .bind(target)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(!archived);
}

#[tokio::test]
async fn chat_moderation_names_its_row_confirms_and_never_borrows_the_givers_identity() {
    let (world, agent, target) = setup().await;
    let chat = chat_of(&world, target).await;
    share(&world, "node", target, "live", "chat.view", "any", agent).await;
    let author = world.person("another-author").await;
    say_in_chat(&world, chat, author, "Another person's message").await;
    let message: Id =
        sqlx::query_scalar("SELECT id FROM chat_messages WHERE chat_id=$1 AND author_id=$2")
            .bind(chat)
            .bind(author)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    let body = json!({"command":"delete","paths":[format!("id:{chat}")],"message_id":message});
    assert!(
        refused(&manage(&world, agent, body.clone()).await)
            .1
            .contains("chat.manage")
    );
    let grant = share(&world, "node", target, "live", "chat.manage", "any", agent).await;
    let plan = ok(manage(&world, agent, body.clone()).await);
    let deleted: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM chat_messages WHERE id=$1")
            .bind(message)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(!deleted);
    let mut lapsed = body.clone();
    lapsed["confirm"] = plan["confirm"].clone();
    sqlx::query("UPDATE permission_entries SET expires_at=$2 WHERE id=$1")
        .bind(grant)
        .bind(world.clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    assert!(
        refused(&manage(&world, agent, lapsed).await)
            .1
            .contains("chat.manage")
    );
    share(&world, "node", target, "live", "chat.manage", "any", agent).await;
    assert_eq!(confirm(&world, agent, body).await["done"], true);
    let tombstone: (Json, Option<Id>) =
        sqlx::query_as("SELECT content,deleted_by FROM chat_messages WHERE id=$1")
            .bind(message)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(tombstone, (json!([]), None));
    let actor:(String,Id)=sqlx::query_as("SELECT actor_kind,actor_id FROM audit_log WHERE action='chat.message_deleted_by_agent' AND object_id=$1")
        .bind(chat).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(actor, ("agent".to_owned(), agent));
}

#[tokio::test]
async fn a_confirmed_agent_fork_uses_the_native_job_and_makes_its_giver_first_owner() {
    for permission in ["thread.fork", "thread.manage"] {
        let (world, agent, target) = setup().await;
        share(&world, "node", target, "live", permission, "any", agent).await;
        let mut body = request("fork", target);
        body["name"] = json!("Agent fork");
        let done = confirm(&world, agent, body).await;
        let job: Id = serde_json::from_value(done["job_id"].clone()).unwrap();
        eventually("the native fork job finished", || async {
            let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
                .bind(job)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
            !matches!(state.as_str(), "queued" | "running")
        })
        .await;
        let (state, plan, error): (String, Json, Option<Json>) =
            sqlx::query_as("SELECT state,plan,error FROM jobs WHERE id=$1")
                .bind(job)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        assert_eq!(state, "succeeded", "{error:?}");
        assert_eq!(plan["by"], json!(world.ada));
        assert_eq!(plan["by_agent"], json!(agent));
        let made: Id = serde_json::from_value(plan["new_thread"].clone()).unwrap();
        let owner: Id = sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id=$1")
            .bind(made)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
        assert_eq!(owner, world.ada);
        assert_eq!(world.count("SELECT count(*) FROM permission_entries WHERE object_id=$1 AND permission='access.own' AND holder_kind='agents' AND removed_at IS NULL",made).await,0);
        assert_eq!(world.count("SELECT count(*) FROM audit_log WHERE action='thread.forked_by_agent' AND object_id=$1",target).await,1);
    }
}

#[tokio::test]
async fn a_givers_role_loss_between_preview_and_confirmation_stops_every_new_act() {
    for command in ["archive", "unarchive", "fork", "delete", "message_delete"] {
        let (world, agent, target) = setup().await;
        let giver = world.person("separate-giver").await;
        let role = world.give(giver, "admin").await;
        let permission = if command == "message_delete" {
            "chat.manage"
        } else {
            "thread.manage"
        };
        let grant = share(&world, "node", target, "live", permission, "any", agent).await;
        sqlx::query("UPDATE permission_entries SET created_by=$2 WHERE id=$1")
            .bind(grant)
            .bind(giver)
            .execute(world.pool.pool())
            .await
            .unwrap();
        let body = if command == "message_delete" {
            let chat = chat_of(&world, target).await;
            share(&world, "node", target, "live", "chat.view", "any", agent).await;
            say_in_chat(&world, chat, world.ada, "A message to moderate").await;
            let message: Id = sqlx::query_scalar(
                "SELECT id FROM chat_messages WHERE chat_id=$1 AND kind='message'",
            )
            .bind(chat)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
            json!({"command":"delete","paths":[format!("id:{chat}")],"message_id":message})
        } else {
            if command == "unarchive" {
                sqlx::query("UPDATE nodes SET archived_at=$2 WHERE id=$1")
                    .bind(target)
                    .bind(world.clock.now())
                    .execute(world.pool.pool())
                    .await
                    .unwrap();
            }
            request(command, target)
        };
        let plan = ok(manage(&world, agent, body.clone()).await);
        let mut confirmed = body;
        confirmed["confirm"] = plan["confirm"].clone();
        world.take(role).await;
        let result = manage(&world, agent, confirmed).await;
        let needed = match command {
            "archive" | "unarchive" => "thread.archive",
            "fork" => "thread.fork",
            "message_delete" => "chat.manage",
            _ => "thread.manage",
        };
        assert_eq!(refused(&result).0, "not_manageable", "{command}");
        assert!(refused(&result).1.contains(needed), "{command}");
        assert_eq!(world.count("SELECT count(*) FROM audit_log WHERE actor_kind='agent' AND object_id=$1 AND action IN ('thread.archived_by_agent','thread.unarchived_by_agent','thread.forked_by_agent','thread.deleted_by_agent')",target).await,0);
    }
}

#[tokio::test]
async fn archived_thread_management_keeps_the_ordinary_delete_and_unarchive_capabilities() {
    let (world, _, target) = setup().await;
    let live_topic = world.topic("Live caller topic").await;
    let agent = thread_in_topic(&world, live_topic, "Agent outside the archive").await;
    share(&world, "node", target, "live", "thread.view", "any", agent).await;
    share(
        &world,
        "node",
        target,
        "live",
        "thread.manage",
        "any",
        agent,
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::hierarchy::archive(&mut conn, target, world.ada, world.clock.now())
        .await
        .unwrap();
    orch_db::queries::hierarchy::archive(&mut conn, world.topic, world.ada, world.clock.now())
        .await
        .unwrap();
    drop(conn);
    let body = request("unarchive", target);
    let plan = ok(manage(&world, agent, body.clone()).await);
    assert_eq!(
        plan["affects"][0]["read_only_after"], true,
        "the parent remains archived"
    );
    let mut body = body;
    body["confirm"] = plan["confirm"].clone();
    assert_eq!(ok(manage(&world, agent, body).await)["done"], true);
    let parent_archived: bool =
        sqlx::query_scalar("SELECT archived_at IS NOT NULL FROM nodes WHERE id=$1")
            .bind(world.topic)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(
        parent_archived,
        "unarchiving a thread never unarchives its place"
    );
    assert_eq!(
        confirm(&world, agent, request("delete", target)).await["done"],
        true
    );
}

#[tokio::test]
async fn a_complete_thread_preview_never_issues_or_spends_a_word_for_a_partial_answer() {
    let (world, agent, target) = setup().await;
    share(
        &world,
        "node",
        target,
        "live",
        "thread.manage",
        "any",
        agent,
    )
    .await;
    for number in 0..10 {
        world
            .document_beside(target, &format!("Document {number}"), &["Retained text"])
            .await;
    }
    let mut body = request("delete", target);
    body["max_bytes"] = json!(1000);
    let refused_plan = manage(&world, agent, body.clone()).await;
    assert_eq!(refused(&refused_plan).0, "too_large");
    body["max_bytes"] = json!(40000);
    let full = ok(manage(&world, agent, body.clone()).await);
    assert_eq!(full["affects"].as_array().unwrap().len(), 11);
    body["confirm"] = full["confirm"].clone();
    body["max_bytes"] = json!(1000);
    assert_eq!(
        refused(&manage(&world, agent, body.clone()).await).0,
        "too_large"
    );
    body["max_bytes"] = json!(40000);
    assert_eq!(ok(manage(&world, agent, body).await)["done"], true);
}

#[tokio::test]
async fn one_givers_unavailable_binding_does_not_hide_another_valid_fork_contribution() {
    let (world, agent, target) = setup().await;
    let limited = world.person("limited-giver").await;
    world.give(limited, "developer").await;
    let first = share(&world, "node", target, "live", "thread.fork", "any", agent).await;
    sqlx::query("UPDATE permission_entries SET created_by=$2 WHERE id=$1")
        .bind(first)
        .bind(limited)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.access.deny(limited);
    share(
        &world,
        "node",
        target,
        "live",
        "thread.manage",
        "any",
        agent,
    )
    .await;
    let plan = ok(manage(&world, agent, request("fork", target)).await);
    assert_eq!(plan["affects"][0]["owner"], json!(world.ada));
    assert_eq!(plan["done"], false);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM jobs WHERE node_id=$1 AND kind='thread_fork'",
                target
            )
            .await,
        0
    );
}
