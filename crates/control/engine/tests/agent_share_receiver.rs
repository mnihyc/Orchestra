//! Widening a live share requires the giver's current authority over its recipient.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use chrono::Duration;
use orch_api_contract::share::{
    ShareAgentUpdateBody, ShareDocumentsLevel, ShareParts, ShareTo, ShareToThread, ShareWhat,
    ShareWhatDocument, ShareWithAgents,
};
use orch_api_contract::threads::ShareMode;
use orch_authz::{Caller, TokenGrant};
use orch_db::audit::Origin;
use orch_engine::sharing::ShareError;
use orch_types::Time;
use shared::{Places, a_world_whose_agent_takes_tools};

#[tokio::test]
async fn extending_a_share_requires_current_drive_but_shortening_does_not() {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let topic = world.topic("Docs").await;
    let document = world
        .document_beside(topic, "Handbook", &["Be kind."])
        .await;
    let now = world.clock.now();
    let initial = Time::new(now + Duration::hours(1));
    let request = ShareWithAgents {
        read_first: None,
        what: ShareWhat::Document(ShareWhatDocument {
            document_id: document,
        }),
        inside: None,
        download: Some(false),
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..ShareParts::default()
        }),
        mode: ShareMode::Live,
        to: ShareTo::Threads(vec![ShareToThread { thread_id: reader }]),
        comment: None,
        expires_at: Some(initial),
        client_id: "receiver-scope".to_owned(),
    };
    let engine = world.engine();
    let made = engine
        .share_with_agents(&Caller::session(world.ada), &Origin::default(), &request)
        .await
        .unwrap();
    let share = &made[0];
    let token = Caller::token(
        world.ada,
        TokenGrant::from_stored(&["doc.view".to_owned()], None).unwrap(),
    );
    let extended = Time::new(now + Duration::hours(2));
    let change = ShareAgentUpdateBody {
        read_first: None,
        parts: None,
        expires_at: Some(extended),
    };
    let denied = engine
        .update_share(&token, &Origin::default(), share.id, &change)
        .await;
    assert!(matches!(denied, Err(ShareError::Denied(_))), "{denied:?}");
    let editor = Caller::token(
        world.ada,
        TokenGrant::from_stored(&["doc.view".to_owned(), "doc.edit".to_owned()], None).unwrap(),
    );
    let adds_edit = ShareAgentUpdateBody {
        read_first: None,
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Edit),
            ..ShareParts::default()
        }),
        expires_at: None,
    };
    let denied = engine
        .update_share(&editor, &Origin::default(), share.id, &adds_edit)
        .await;
    assert!(matches!(denied, Err(ShareError::Denied(_))), "{denied:?}");
    let mut conn = world.pool.acquire().await.unwrap();
    for grant in &share.grants {
        let expiry: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
            "SELECT expires_at FROM permission_entries WHERE id=$1 AND removed_at IS NULL",
        )
        .bind(grant.entry_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(expiry, Some(initial.get()));
    }
    let entries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM permission_entries WHERE object_kind='document' AND object_id=$1 AND holder_kind='agents' AND holder_id=$2 AND removed_at IS NULL",
    )
    .bind(document)
    .bind(reader)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(entries, 1);
    drop(conn);
    let shorter = ShareAgentUpdateBody {
        read_first: None,
        parts: None,
        expires_at: Some(Time::new(now + Duration::minutes(30))),
    };
    engine
        .update_share(&token, &Origin::default(), share.id, &shorter)
        .await
        .unwrap();
    engine
        .update_share(
            &Caller::session(world.ada),
            &Origin::default(),
            share.id,
            &change,
        )
        .await
        .unwrap();

    // A mixed request can still remove Edit when this token cannot drive the
    // recipient; the requested extension of Read must not take effect.
    engine
        .update_share(
            &Caller::session(world.ada),
            &Origin::default(),
            share.id,
            &adds_edit,
        )
        .await
        .unwrap();
    let mixed = ShareAgentUpdateBody {
        read_first: None,
        parts: Some(ShareParts {
            documents: Some(ShareDocumentsLevel::Read),
            ..ShareParts::default()
        }),
        expires_at: Some(Time::new(now + Duration::hours(3))),
    };
    let narrowed = engine
        .update_share(&editor, &Origin::default(), share.id, &mixed)
        .await
        .unwrap();
    assert_eq!(narrowed.grants.len(), 1);
    assert_eq!(narrowed.grants[0].permission, "doc.view");
    assert_eq!(narrowed.grants[0].expires_at, Some(extended.get()));
}
