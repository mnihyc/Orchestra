//! An Ask segment retains the exact API interface or private program selection that served it.
use super::*;
use orch_db::queries::{ask as rows, threads};

async fn api_segment(s: &Scene) -> (Id, Id) {
    let provider = configured_provider(s).await;
    let conversation = create_unseeded(s, &s.ada).await.conversation_id;
    let segment = new_id();
    let mut tx = s.rig.db.begin().await.unwrap();
    let mut unit = orch_engine::unit::Unit::new(&mut tx, s.rig.world.now());
    let seq = unit.stamp(conversation).await.unwrap();
    rows::segment(
        unit.conn,
        segment,
        conversation,
        &rows::SegmentBinding {
            provider_id: provider,
            transport: rows::DialogueTransport::Api {
                protocol: "openai-responses".into(),
                backend: AskModelIdentity::api(provider, MODEL.into()).backend,
            },
            model: MODEL.into(),
        },
        s.ada.id(),
        s.rig.world.now(),
        seq,
    )
    .await
    .unwrap();
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    (conversation, segment)
}

#[tokio::test]
async fn api_segments_keep_their_exact_backend_and_protocol() {
    let s = Scene::new().await;
    s.rig.engine.stop();
    let (conversation, segment) = api_segment(&s).await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let stored: (String, Option<Json>) =
        sqlx::query_as("SELECT program_protocol,program_backend FROM segments WHERE id=$1")
            .bind(segment)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(stored.0, "openai-responses");
    let binding = rows::segment_binding(&mut conn, conversation, segment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        binding.transport,
        rows::DialogueTransport::Api {
            protocol: "openai-responses".into(),
            backend: AskModelIdentity::api(binding.provider_id, MODEL.into()).backend
        }
    );
    assert_eq!(
        stored.1,
        Some(serde_json::to_value(binding.transport.backend()).unwrap())
    );
    assert_eq!(binding.model, MODEL);
    assert!(
        rows::program_binding(&mut conn, conversation)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_program_segment_keeps_its_exact_backend_without_borrowing_an_api_interface() {
    let s = Scene::new().await;
    s.rig.engine.stop();
    let (conversation, previous) = api_segment(&s).await;
    let provider = s.on.provider;
    let backend = serde_json::from_value(json!({
        "route":"ask.delegation","offering":{"provider":provider,"offering":MODEL},
        "config":{"environment_id":s.on.environment,"helper_topic_id":s.tree.topic.base.id},
        "x_choice":{"revision":"selected"}
    }))
    .unwrap();
    let binding = rows::SegmentBinding {
        provider_id: provider,
        model: MODEL.into(),
        transport: rows::DialogueTransport::Program(backend),
    };
    let segment = new_id();
    let mut tx = s.rig.db.begin().await.unwrap();
    let mut unit = orch_engine::unit::Unit::new(&mut tx, s.rig.world.now());
    let seq = unit.stamp(conversation).await.unwrap();
    let mut mismatched = binding.clone();
    mismatched.provider_id = new_id();
    assert!(
        rows::segment(
            unit.conn,
            segment,
            conversation,
            &mismatched,
            s.ada.id(),
            s.rig.world.now(),
            seq
        )
        .await
        .is_err()
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT ended_at IS NULL FROM segments WHERE id=$1")
            .bind(previous)
            .fetch_one(&mut *unit.conn)
            .await
            .unwrap()
    );
    rows::segment(
        unit.conn,
        segment,
        conversation,
        &binding,
        s.ada.id(),
        s.rig.world.now(),
        seq,
    )
    .await
    .unwrap();
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    let mut conn = s.rig.db.acquire().await.unwrap();
    assert_eq!(
        rows::segment_binding(&mut conn, conversation, segment)
            .await
            .unwrap(),
        Some(binding)
    );
    assert!(
        rows::program_binding(&mut conn, conversation)
            .await
            .unwrap()
            .is_some()
    );
    let physical: (Option<String>, Option<Id>, Option<Id>, String) = sqlx::query_as(
        "SELECT program_protocol,environment_id,agent_id,credential_path FROM segments WHERE id=$1",
    )
    .bind(segment)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(physical, (None, None, None, "native".into()));
    assert_eq!(s.rig.commands.issued_of("provider.generate"), 0);
}

#[tokio::test]
async fn reserved_program_backends_decode_as_unavailable_without_api_fallback() {
    let s = Scene::new().await;
    s.rig.engine.stop();
    let (conversation, segment) = api_segment(&s).await;
    let backend = json!({"route":"reserved-session","offering":null,"config":null});
    for protocol in [None, Some("openai-responses")] {
        sqlx::query("UPDATE segments SET program_backend=$2,program_protocol=$3 WHERE id=$1")
            .bind(segment)
            .bind(&backend)
            .bind(protocol)
            .execute(s.rig.db.pool())
            .await
            .unwrap();
        let mut conn = s.rig.db.acquire().await.unwrap();
        assert!(
            threads::segments_of(&mut conn, conversation)
                .await
                .unwrap()
                .iter()
                .any(|row| row.id == segment)
        );
        assert!(
            rows::segment_binding(&mut conn, conversation, segment)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            rows::program_binding(&mut conn, conversation)
                .await
                .unwrap()
                .is_none(),
            "a stored API preference cannot stand in for an unavailable backend"
        );
    }
    assert_eq!(s.rig.commands.issued_of("provider.generate"), 0);
    assert_eq!(s.rig.commands.issued_of("provider.complete"), 0);
}

#[tokio::test]
async fn program_backend_shape_and_native_segment_metadata_are_constrained() {
    let s = Scene::new().await;
    s.rig.engine.stop();
    let (_, segment) = api_segment(&s).await;
    for backend in [
        Json::Null,
        json!(false),
        json!(1),
        json!("route"),
        json!([]),
    ] {
        let error = sqlx::query("UPDATE segments SET program_backend=$2 WHERE id=$1")
            .bind(segment)
            .bind(backend)
            .execute(s.rig.db.pool())
            .await
            .unwrap_err();
        assert_eq!(
            error
                .as_database_error()
                .and_then(|error| error.constraint()),
            Some("segments_program_backend_shape_check")
        );
    }
    let thread = s.thread().await.base.id;
    let native = s.rig.segment_of(thread).await;
    let error = sqlx::query("UPDATE segments SET program_backend='{}'::jsonb WHERE id=$1")
        .bind(native)
        .execute(s.rig.db.pool())
        .await
        .unwrap_err();
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.constraint()),
        Some("segments_program_protocol_check")
    );
    let error = sqlx::query("UPDATE segments SET program_protocol='openai-responses' WHERE id=$1")
        .bind(native)
        .execute(s.rig.db.pool())
        .await
        .unwrap_err();
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.constraint()),
        Some("segments_program_protocol_check")
    );
}

#[tokio::test]
async fn a_live_ask_counts_remaining_items_and_its_continuation_cascade_releases_the_whole_count() {
    let s = Scene::new().await;
    let (conversation, segment) = api_segment(&s).await;
    let mut tx = s.rig.db.begin().await.unwrap();
    let owner: Id =
        sqlx::query_scalar("SELECT active_continuation_id FROM threads WHERE node_id=$1")
            .bind(conversation)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    sqlx::query("INSERT INTO items(id,thread_id,segment_id,continuation_id,kind,status,payload,seq_created,seq_updated)
        SELECT gen_random_uuid(),$1,$2,$3,'agent_message','completed','{}',n,n FROM generate_series(1,3) AS n")
        .bind(conversation).bind(segment).bind(owner).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "DELETE FROM items WHERE continuation_id=$1 AND kind='agent_message' AND seq_created=1",
    )
    .bind(owner)
    .execute(&mut *tx)
    .await
    .unwrap();
    let remaining:(i64,i64,i64)=sqlx::query_as("SELECT item_count,first_seq,last_seq FROM thread_history_turn_counts
        WHERE continuation_id=$1 AND turn_id IS NULL AND stream='main' AND kind='agent_message' AND NOT before_orch")
        .bind(owner).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(remaining, (2, 2, 3));
    // The governing-root cascade deletes the owned node directly.
    sqlx::query("DELETE FROM nodes WHERE id=$1")
        .bind(conversation)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let retained: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM thread_history_turn_counts WHERE continuation_id=$1)",
    )
    .bind(owner)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert!(!retained);
}
