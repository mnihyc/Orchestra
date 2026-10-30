//! Folded paste delivery uses the remaining context, preserves its stored chip and keeps drafts out of the workspace.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, queued};
use orch_api_contract::threads::{ContentPart, PastedText};
use orch_authz::{Caller, Principal};
use orch_blob_contract::store::{BlobStore, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_engine::paste::{available_name, inline_allowance};
use orch_types::{Hash, Time};
use serde_json::json;
use std::{collections::BTreeSet, sync::Arc};

#[test]
fn the_inline_budget_is_a_share_of_remaining_context_and_unknown_context_stays_unknown() {
    assert_eq!(inline_allowance(Some(1000), Some(900), 25), 25);
    assert_eq!(inline_allowance(Some(1000), Some(901), 25), 24);
    assert_eq!(inline_allowance(Some(1000), Some(1200), 25), 0);
    assert_eq!(inline_allowance(Some(1000), None, 25), 0);
    assert_eq!(inline_allowance(None, Some(0), 25), 0);
    assert_eq!(inline_allowance(Some(1000), Some(0), 0), 0);
}

#[test]
fn filenames_get_numbered_siblings_without_losing_their_extension() {
    let used = BTreeSet::from([
        "shot.png".to_owned(),
        "shot (2).png".to_owned(),
        ".env".to_owned(),
    ]);
    assert_eq!(available_name("shot.png", &used), "shot (3).png");
    assert_eq!(available_name(".env", &used), ".env (2)");
    for name in [
        format!("{}.png", "x".repeat(196)),
        format!("{}.png", "文".repeat(80)),
    ] {
        let sibling = available_name(&name, &BTreeSet::from([name.clone()]));
        assert!(sibling.ends_with(" (2).png"));
        assert!(sibling.chars().count() <= orch_engine::delivery::NAME_MAX_CHARS);
        assert!(sibling.len() <= name.len().max(orch_engine::delivery::NAME_MAX_CHARS));
        assert_eq!(orch_engine::delivery::safe_name(&sibling), sibling);
    }
}

async fn kept(world: &World, store: &LocalBlobStore, text: &str) -> PastedText {
    let hash = Hash::of(text.as_bytes());
    store
        .put(
            &hash,
            Some(text.len() as i64),
            stream_of(vec![bytes::Bytes::copy_from_slice(text.as_bytes())]),
        )
        .await
        .unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::storage::register_blob(
        &mut conn,
        &hash,
        text.len() as i64,
        world.clock.now(),
    )
    .await
    .unwrap();
    orch_db::queries::uploads::remember(&mut conn, world.ada, &hash, world.clock.now())
        .await
        .unwrap();
    PastedText {
        delivery: None,
        r#type: orch_api_contract::threads::PastedTextType::PastedText,
        blob: hash,
        name: "pasted-test.txt".to_owned(),
        path: ".orch/attachments/pasted-test.txt".to_owned(),
        size: text.len() as i64,
        characters: text.chars().count() as i64,
        ascii_characters: text.bytes().filter(u8::is_ascii).count() as i64,
        inline_text: Some("forged inline words".to_owned()),
        source: None,
    }
}

#[tokio::test]
async fn a_paste_at_the_share_boundary_is_read_from_its_blob_and_sent_in_the_message() {
    let world = World::new().await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LocalBlobStore::open(root.path().to_path_buf())
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let thread = world.thread("Folded paste").await;
    sqlx::query("UPDATE threads SET usage=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(json!({"context_window":1000,"context_used":900}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let text = "x".repeat(100);
    let paste = kept(&world, &store, &text).await;
    let mut request = queued("");
    request.content = vec![ContentPart::PastedText(paste)];
    let accepted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, request)
        .await
        .unwrap();
    let ContentPart::PastedText(public) = &accepted.input.content[0] else {
        panic!("the original remains folded");
    };
    assert!(
        public.inline_text.is_none(),
        "opening the timeline must not preload the folded text"
    );
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT content FROM thread_inputs WHERE id=$1")
            .bind(accepted.input.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    let stored: Vec<ContentPart> = serde_json::from_value(stored).unwrap();
    let ContentPart::PastedText(paste) = &stored[0] else {
        panic!("the private delivery keeps the folded source");
    };
    assert_eq!(paste.inline_text.as_deref(), Some(text.as_str()));
    assert!(orch_engine::delivery::plan(accepted.input.id, &stored).is_empty());
    let start = world.turn_start(thread, 0).await;
    assert!(start.text.contains(&text));
    assert!(!start.text.contains("forged inline words"));
}

#[tokio::test]
async fn the_sender_can_choose_inline_past_the_default_or_a_file_under_the_always_inline_default() {
    use orch_api_contract::threads::PasteDelivery;
    let world = World::new().await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LocalBlobStore::open(root.path().to_path_buf())
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    for (share, choice, inline) in [
        (0, Some(PasteDelivery::Inline), true),
        (100, Some(PasteDelivery::File), false),
        (100, None, true),
    ] {
        let thread = world.thread("A sender chooses paste delivery").await;
        world
            .set(thread, "composer.paste_inline_context_share", json!(share))
            .await;
        sqlx::query("UPDATE threads SET usage=$2 WHERE node_id=$1")
            .bind(thread)
            .bind(json!({"context_window":1000,"context_used":1000}))
            .execute(world.pool.pool())
            .await
            .unwrap();
        let text = "x".repeat(4000);
        let mut paste = kept(&world, &store, &text).await;
        paste.delivery = choice;
        let mut request = queued("");
        request.content = vec![ContentPart::PastedText(paste)];
        let accepted = world
            .engine()
            .submit_input(&Caller::session(world.ada), thread, request)
            .await
            .unwrap();
        let stored: serde_json::Value =
            sqlx::query_scalar("SELECT content FROM thread_inputs WHERE id=$1")
                .bind(accepted.input.id)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        let stored: Vec<ContentPart> = serde_json::from_value(stored).unwrap();
        let ContentPart::PastedText(paste) = &stored[0] else {
            panic!("the paste stays folded")
        };
        assert_eq!(
            paste.inline_text.as_deref(),
            inline.then_some(text.as_str())
        );
        assert_eq!(
            orch_engine::delivery::plan(accepted.input.id, &stored).is_empty(),
            inline
        );
    }
}

#[tokio::test]
async fn zero_disables_inline_delivery_even_after_a_paste_is_edited_empty() {
    let world = World::new().await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LocalBlobStore::open(root.path().to_path_buf())
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let thread = world.thread("File-only pastes").await;
    world.engine().stop();
    world
        .set(thread, "composer.paste_inline_context_share", json!(0))
        .await;
    sqlx::query("UPDATE threads SET usage=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(json!({"context_window":1000,"context_used":0}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let paste = kept(&world, &store, "").await;
    let mut request = queued("");
    request.content = vec![ContentPart::PastedText(paste)];
    let accepted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, request)
        .await
        .unwrap();
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT content FROM thread_inputs WHERE id=$1")
            .bind(accepted.input.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(stored[0].get("inline_text").is_none());
    let stored: Vec<ContentPart> = serde_json::from_value(stored).unwrap();
    let planned = orch_engine::delivery::plan(accepted.input.id, &stored);
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].path, ".orch/attachments/pasted-test.txt");
}

#[tokio::test]
async fn a_paste_one_token_over_the_remaining_share_is_a_file_with_its_path_and_size() {
    let world = World::new().await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LocalBlobStore::open(root.path().to_path_buf())
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let thread = world.thread("Paste as file").await;
    sqlx::query("UPDATE threads SET usage=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(json!({"context_window":1000,"context_used":901}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let paste = kept(&world, &store, &"x".repeat(100)).await;
    let mut request = queued("");
    request.content = vec![ContentPart::PastedText(paste)];
    let accepted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, request)
        .await
        .unwrap();
    let ContentPart::PastedText(paste) = &accepted.input.content[0] else {
        panic!("the original remains folded");
    };
    assert!(paste.inline_text.is_none());
    assert_eq!(paste.size, 100);
    let planned = orch_engine::delivery::plan(accepted.input.id, &accepted.input.content);
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].path, ".orch/attachments/pasted-test.txt");
}

#[tokio::test]
async fn four_byte_unicode_at_the_context_boundary_is_not_mistaken_for_an_oversized_paste() {
    let world = World::new().await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LocalBlobStore::open(root.path().to_path_buf())
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let thread = world.thread("Unicode paste").await;
    sqlx::query("UPDATE provider_models SET model_name='gpt-6' WHERE provider_id=(SELECT provider_id FROM threads WHERE node_id=$1)").bind(thread).execute(world.pool.pool()).await.unwrap();
    world.set(thread, "agent.model", json!("gpt-6")).await;
    sqlx::query("UPDATE threads SET usage=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(json!({"context_window":1000,"context_used":724}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let words = "🌍".repeat(90);
    let paste = kept(&world, &store, &words).await;
    let mut request = queued("");
    request.content = vec![ContentPart::PastedText(paste)];
    let accepted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, request)
        .await
        .unwrap();
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT content FROM thread_inputs WHERE id=$1")
            .bind(accepted.input.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(stored[0]["inline_text"], words);
    assert_eq!(stored[0]["characters"], 90);
    assert_eq!(stored[0]["ascii_characters"], 0);
}

#[tokio::test]
async fn the_selected_context_fill_decides_delivery_even_when_the_usage_total_looks_roomy() {
    let world = World::new().await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LocalBlobStore::open(root.path().to_path_buf())
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let thread = world.thread("Filled conversation").await;
    let (start, _) = world.running_turn(thread, "Work").await;
    world.engine().stop();
    sqlx::query("UPDATE threads SET usage=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(json!({"context_window":1000,"context_used":100}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE turns SET context=$2 WHERE id=$1").bind(start.turn_id)
        .bind(json!({"count":"orch","used":901,"window":1000,"compact_at":null,"scaled":false,"at":Time::new(world.clock.now()),"parts":[{"kind":"other","tokens":901,"count":"orch","rows":[]}]}))
        .execute(world.pool.pool()).await.unwrap();
    let paste = kept(&world, &store, &"x".repeat(100)).await;
    let mut request = queued("");
    request.content = vec![ContentPart::PastedText(paste)];
    let accepted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, request)
        .await
        .unwrap();
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT content FROM thread_inputs WHERE id=$1")
            .bind(accepted.input.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(
        stored[0]
            .get("inline_text")
            .is_none_or(serde_json::Value::is_null)
    );
    let stored: Vec<ContentPart> = serde_json::from_value(stored).unwrap();
    let planned = orch_engine::delivery::plan(accepted.input.id, &stored);
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].path, ".orch/attachments/pasted-test.txt");
}

#[tokio::test]
async fn an_unknown_fill_is_not_invented_as_zero_even_when_no_turn_was_recorded() {
    let world = World::new().await;
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LocalBlobStore::open(root.path().to_path_buf())
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let thread = world.thread("Unknown context").await;
    world.engine().stop();
    sqlx::query("UPDATE threads SET usage=NULL WHERE node_id=$1")
        .bind(thread)
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE provider_models SET parameters=jsonb_set(parameters,'{context_window}','1000'::jsonb) WHERE provider_id=$1")
        .bind(world.provider).execute(world.pool.pool()).await.unwrap();
    let paste = kept(&world, &store, "short").await;
    let mut request = queued("");
    request.content = vec![ContentPart::PastedText(paste)];
    let accepted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, request)
        .await
        .unwrap();
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT content FROM thread_inputs WHERE id=$1")
            .bind(accepted.input.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    let stored: Vec<ContentPart> = serde_json::from_value(stored).unwrap();
    assert_eq!(
        orch_engine::delivery::plan(accepted.input.id, &stored).len(),
        1
    );
}

#[tokio::test]
async fn a_steer_not_yet_dispatched_returns_to_its_original_queue_position() {
    use orch_api_contract::threads::{Delivery, InputState};
    let world = World::new().await;
    let thread = world.thread("Return a steer").await;
    world.running_turn(thread, "Work").await;
    world.engine().stop();
    let caller = Caller::session(world.ada);
    let first = world
        .engine()
        .submit_input(&caller, thread, queued("First queued"))
        .await
        .unwrap();
    let steer = world
        .engine()
        .submit_input(&caller, thread, common::steering("Steer or wait"))
        .await
        .unwrap();
    let last = world
        .engine()
        .submit_input(&caller, thread, queued("Last queued"))
        .await
        .unwrap();
    let position = steer.input.position.clone();
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(&mut tx, caller.clone(), Time::new(world.clock.now()))
        .await
        .unwrap();
    let returned = world
        .engine()
        .update_input_guarded(
            tx,
            principal,
            thread,
            steer.input.id,
            serde_json::from_value(json!({"delivery":"queue"})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(returned.input.state, InputState::Pending);
    assert_eq!(returned.input.delivery, Delivery::Queue);
    assert_eq!(returned.input.position, position);
    assert!(
        first.input.position.parse::<i128>().unwrap()
            < returned.input.position.parse::<i128>().unwrap()
    );
    assert!(
        returned.input.position.parse::<i128>().unwrap()
            < last.input.position.parse::<i128>().unwrap()
    );
    assert_eq!(world.commands.issued_of("turn.steer"), 0);
}

#[tokio::test]
async fn a_steers_reserved_place_survives_other_messages_being_reordered() {
    let world = World::new().await;
    let thread = world.thread("Reorder around a steer").await;
    world.running_turn(thread, "Work").await;
    world.engine().stop();
    let caller = Caller::session(world.ada);
    let first = world
        .engine()
        .submit_input(&caller, thread, queued("First"))
        .await
        .unwrap();
    let steer = world
        .engine()
        .submit_input(&caller, thread, common::steering("Return here"))
        .await
        .unwrap();
    let last = world
        .engine()
        .submit_input(&caller, thread, queued("Last"))
        .await
        .unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(&mut tx, caller.clone(), Time::new(world.clock.now()))
        .await
        .unwrap();
    let moved = world
        .engine()
        .update_input_guarded(
            tx,
            principal,
            thread,
            first.input.id,
            serde_json::from_value(json!({"after_input_id":last.input.id})).unwrap(),
        )
        .await
        .unwrap();
    let reserved = moved
        .inputs
        .iter()
        .find(|input| input.id == steer.input.id)
        .expect("the answer includes a renamed reserved position")
        .position
        .clone();
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(&mut tx, caller, Time::new(world.clock.now()))
        .await
        .unwrap();
    let returned = world
        .engine()
        .update_input_guarded(
            tx,
            principal,
            thread,
            steer.input.id,
            serde_json::from_value(json!({"delivery":"queue"})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(returned.input.position, reserved);
    let order: Vec<orch_types::Id> = sqlx::query_scalar("SELECT id FROM thread_inputs WHERE thread_id=$1 AND state='pending' ORDER BY position,created_at,id")
        .bind(thread).fetch_all(world.pool.pool()).await.unwrap();
    assert_eq!(order, [steer.input.id, last.input.id, first.input.id]);
    assert_eq!(world.commands.issued_of("turn.steer"), 0);
}

#[tokio::test]
async fn a_steer_taken_by_the_program_cannot_be_recalled() {
    use orch_api_contract::threads::Delivery;
    let world = World::new().await;
    let thread = world.thread("Taken steer").await;
    world.running_turn(thread, "Work").await;
    let caller = Caller::session(world.ada);
    let steer = world
        .engine()
        .submit_input(&caller, thread, common::steering("Take this now"))
        .await
        .unwrap();
    world.wait_input(steer.input.id, "delivered").await;
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(&mut tx, caller.clone(), Time::new(world.clock.now()))
        .await
        .unwrap();
    let error = world
        .engine()
        .update_input_guarded(
            tx,
            principal,
            thread,
            steer.input.id,
            serde_json::from_value(json!({"delivery":Delivery::Queue})).unwrap(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        orch_engine::inputs::InputError::NotPending { .. }
    ));
}
