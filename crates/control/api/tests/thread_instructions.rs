//! What a thread's agent reads, over HTTP: what Orch gave its model, recorded as it was sent, for whoever reads the
//! thread and nobody else; and the instruction sources its last session was opened with, as each reader may see
//! them, with a change of the standing instructions shown as waiting for the next turn.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::instructions::{
    InstructionEvidence, InstructionSourceKind, ThreadInstructions, ThreadInstructionsGet,
};
use orch_api_contract::model_inputs::{
    ModelInputListQuery, ModelInputPurpose, ModelInputState, NodeModelInputList,
};
use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
use orch_types::{Id, Json};
use places::Who;
use rig::{Scene, eventually};
use serde_json::json;

async fn given(
    s: &Scene,
    by: &Who,
    node: Id,
    query: &ModelInputListQuery,
) -> orch_api_core::testing::Reply<
    orch_api_contract::common::Page<orch_api_contract::model_inputs::ModelInput>,
> {
    s.rig
        .world
        .app
        .call(NodeModelInputList, &[&node])
        .token(&by.token)
        .query(query)
        .send()
        .await
}

async fn instructions(
    s: &Scene,
    by: &Who,
    thread: Id,
) -> orch_api_core::testing::Reply<ThreadInstructions> {
    s.rig
        .world
        .app
        .call(ThreadInstructionsGet, &[&thread])
        .token(&by.token)
        .send()
        .await
}

/// Somebody who may read the thread and nothing more.
async fn reader(s: &Scene, thread: Id) -> Who {
    let reader = s.rig.person("gus").await;
    s.rig
        .lets_use("node", thread, "thread.view", &reader, json!({}))
        .await;
    reader
}

#[tokio::test]
async fn whoever_reads_a_thread_reads_what_its_model_was_given_and_nobody_else_does() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.exchange(thread, 0, "Fix the login", "Done").await;
    eventually(
        "the opening, catalogue and input records settled",
        || async {
            given(&s, &s.ada, thread, &ModelInputListQuery::default())
                .await
                .body
                .map(|page| {
                    page.items.len() == 3
                        && page
                            .items
                            .iter()
                            .all(|record| record.state == ModelInputState::Sent)
                })
                .unwrap_or(false)
        },
    )
    .await;
    let reader = reader(&s, thread).await;
    let page = given(&s, &reader, thread, &ModelInputListQuery::default())
        .await
        .ok();
    // The newest first: the turn's input, then what the session was opened with.
    let input = page
        .items
        .iter()
        .find(|record| record.purpose == ModelInputPurpose::Input)
        .unwrap();
    let session = page
        .items
        .iter()
        .find(|record| record.purpose == ModelInputPurpose::Session)
        .unwrap();
    let tools = page
        .items
        .iter()
        .find(|record| record.purpose == ModelInputPurpose::Tool)
        .unwrap();
    assert_eq!(session.purpose, ModelInputPurpose::Session);
    assert_eq!(session.blocks[0].key, "preamble");
    assert_eq!(input.purpose, ModelInputPurpose::Input);
    assert_eq!(input.blocks[0].key, "where_you_are");
    let words = input.blocks.last().unwrap();
    assert_eq!(words.key, "input");
    assert_eq!(words.text.inline, "Fix the login");
    assert!(!words.text.truncated);
    assert_eq!(page.next_cursor, None);
    // A page of one, and the page after it.
    let first = given(
        &s,
        &reader,
        thread,
        &ModelInputListQuery {
            limit: Some(1),
            ..ModelInputListQuery::default()
        },
    )
    .await
    .ok();
    assert_eq!(first.items[0].id, input.id);
    let before = first.next_cursor.unwrap().parse::<i64>().unwrap();
    let second = given(
        &s,
        &reader,
        thread,
        &ModelInputListQuery {
            before: Some(before),
            ..ModelInputListQuery::default()
        },
    )
    .await
    .ok();
    assert_eq!(
        second
            .items
            .iter()
            .map(|record| record.id)
            .collect::<Vec<_>>(),
        [tools.id, session.id]
    );
    // Somebody who may not read the thread reads none of it.
    let stranger = s.rig.person("fay").await;
    let refused = given(&s, &stranger, thread, &ModelInputListQuery::default()).await;
    assert!(
        refused.body.is_err(),
        "a stranger read it: {:?}",
        refused.body
    );
}

#[tokio::test]
async fn the_sources_a_session_was_opened_with_are_shown_as_each_reader_may_see_them() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.exchange(thread, 0, "hello", "hi").await;
    let segment = s.rig.segment_of(thread).await;
    let opened = instructions(&s, &s.ada, thread).await.ok();
    assert_eq!(opened.segment_id, Some(segment));
    assert_eq!(opened.sources[0].kind, InstructionSourceKind::Preamble);
    assert_eq!(opened.sources[0].evidence, InstructionEvidence::Orch);
    assert!(
        opened.sources[0].readable,
        "what Orch gave is read in what the agent was given"
    );
    assert!(!opened.standing.pending_next_turn);
    assert_eq!(opened.standing.limit, 40_000);
    // Two files a harness reported, kept as the engine keeps them: one of the workspace, one of the harness's home.
    let mut kept: Json =
        sqlx::query_scalar("SELECT instruction_sources FROM segments WHERE id = $1")
            .bind(segment)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
    kept.as_array_mut().unwrap().extend([
        json!({
            "kind": "file", "label": "AGENTS.md", "path": "/workspace/web/AGENTS.md", "home": false,
            "parent_path": "/workspace/CLAUDE.md", "load_reason": "include",
            "evidence": "reported", "bytes": 120, "loaded_bytes": 120, "cut_bytes": 0, "lines": 4,
            "loaded_lines": 4, "scope": null, "environment": s.on.environment
        }),
        json!({
            "kind": "file", "label": "AGENTS.md of the harness's home", "home": true, "evidence": "reported",
            "reason": "a file of the harness's home", "scope": null, "environment": null
        }),
    ]);
    sqlx::query("UPDATE segments SET instruction_sources = $2 WHERE id = $1")
        .bind(segment)
        .bind(&kept)
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
    let owner = instructions(&s, &s.ada, thread).await.ok();
    let file = owner
        .sources
        .iter()
        .find(|source| source.label == "AGENTS.md")
        .unwrap();
    assert_eq!(file.path.as_deref(), Some("/workspace/web/AGENTS.md"));
    assert_eq!(file.workspace_path.as_deref(), Some("web/AGENTS.md"));
    assert_eq!(file.parent_path.as_deref(), Some("/workspace/CLAUDE.md"));
    assert_eq!(file.load_reason.as_deref(), Some("include"));
    assert!(file.readable && file.editable);
    assert_eq!(
        file.source_environment
            .as_ref()
            .map(|environment| environment.id),
        Some(s.on.environment)
    );
    let home = owner
        .sources
        .iter()
        .find(|source| source.label.ends_with("home"))
        .unwrap();
    assert!(home.path.is_none() && !home.readable && !home.editable);
    // A reader of the thread who may not view the environment's files has the file by its label alone.
    let reader = reader(&s, thread).await;
    let read = instructions(&s, &reader, thread).await.ok();
    let file = read
        .sources
        .iter()
        .find(|source| source.label == "AGENTS.md")
        .unwrap();
    assert!(file.path.is_none() && !file.readable && !file.editable);
    assert!(file.workspace_path.is_none());
    assert!(file.parent_path.is_none());
    assert!(file.source_environment.is_none());
    // A change of the thread's standing instructions waits for the next turn, and says so.
    s.rig
        .world
        .app
        .call(NodeSettingSet, &[&thread, &"agent.instructions"])
        .token(&s.ada.token)
        .body(&NodeSettingSetBody {
            value: json!("Answer in one line."),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let changed = instructions(&s, &s.ada, thread).await.ok();
    assert!(changed.standing.pending_next_turn);
    assert!(changed.standing.characters > 0);
}

#[tokio::test]
async fn a_reader_of_the_thread_reads_long_supplied_text_whole_without_files_permission() {
    use orch_api_contract::model_inputs::NodeModelInputBlobGet;
    use orch_link_contract::commands::{Command, CommandKind};
    let s = Scene::new().await;
    s.rig.engine.attach_blobs(s.rig.world.app.app.blobs.store());
    let thread = s.thread().await.base.id;
    let long = format!("BEGIN-MATERIAL\n{}\nEND-MATERIAL", "中🙂".repeat(6_000));
    s.rig
        .world
        .set_at(
            orch_settings::Target::Node(thread),
            "agent.instructions",
            json!(long),
        )
        .await;
    s.exchange(thread, 0, "Go", "Done").await;
    let viewer = reader(&s, thread).await;
    let page = given(&s, &viewer, thread, &ModelInputListQuery::default())
        .await
        .ok();
    let session = page
        .items
        .iter()
        .find(|record| record.purpose == ModelInputPurpose::Session)
        .unwrap();
    let mut full = Vec::new();
    let mut kept_hash = None;
    for block in &session.blocks {
        if let Some(hash) = block.text.blob {
            assert!(block.text.truncated);
            assert!(
                block.text.inline.len()
                    <= usize::try_from(orch_api_contract::threads::OUTPUT_INLINE_MAX_BYTES)
                        .unwrap()
            );
            assert!(
                !block.text.inline.contains("BEGIN-MATERIAL"),
                "only a preview is inline"
            );
            let read = s
                .rig
                .world
                .app
                .call(NodeModelInputBlobGet, &[&thread, &session.id, &hash])
                .token(&viewer.token)
                .send_bytes()
                .await;
            assert!(read.status.is_success(), "{:?}", read.problem());
            full.push(String::from_utf8(read.raw.to_vec()).unwrap());
            kept_hash = Some(hash);
        } else {
            full.push(block.text.inline.clone());
        }
    }
    let Command::SessionOpen(opening) = s.rig.seen.of_kind(CommandKind::SessionOpen).remove(0)
    else {
        panic!("the actual native opening");
    };
    assert_eq!(
        full.join("\n\n"),
        opening.instructions,
        "no UTF-8 bytes, headings or original supplied text are lost"
    );
    let hash = kept_hash.unwrap();
    let stranger = s.rig.person("stranger").await;
    assert!(
        !s.rig
            .world
            .app
            .call(NodeModelInputBlobGet, &[&thread, &session.id, &hash])
            .token(&stranger.token)
            .send_bytes()
            .await
            .status
            .is_success()
    );
    let other = s.thread().await.base.id;
    assert_eq!(
        s.rig
            .world
            .app
            .call(NodeModelInputBlobGet, &[&other, &session.id, &hash])
            .token(&s.ada.token)
            .send_bytes()
            .await
            .status
            .as_u16(),
        404
    );
    let unknown = orch_types::Hash::of(b"unrelated text");
    assert_eq!(
        s.rig
            .world
            .app
            .call(NodeModelInputBlobGet, &[&thread, &session.id, &unknown])
            .token(&viewer.token)
            .send_bytes()
            .await
            .status
            .as_u16(),
        404
    );
    let mut conn = s.rig.db.acquire().await.unwrap();
    let past_grace = s.rig.world.now() + chrono::Duration::days(2);
    assert!(
        !orch_db::queries::storage::unreferenced(&mut conn, past_grace, 100)
            .await
            .unwrap()
            .iter()
            .any(|(found, _)| *found == hash)
    );
    assert!(
        !orch_db::queries::storage::forget_blob(&mut conn, &hash, past_grace)
            .await
            .unwrap(),
        "the readable record keeps the complete text after the blob grace"
    );
}
