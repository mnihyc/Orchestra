//! Reserved program metadata stays inert and cannot masquerade as an API binding.
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
            protocol: "openai-responses".into(),
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
async fn api_segments_keep_their_protocol_and_an_unset_program_backend() {
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
    assert_eq!(stored, ("openai-responses".into(), None));
    let binding = rows::segment_binding(&mut conn, conversation, segment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(binding.protocol, "openai-responses");
    assert_eq!(binding.model, MODEL);
    assert!(
        rows::program_binding(&mut conn, conversation)
            .await
            .unwrap()
            .is_some()
    );
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
