//! A people-chat member gives agents only what the current caller holds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use chrono::Duration;
use orch_api_contract::share::{
    ShareAgentUpdateBody, ShareLevel, ShareParts, ShareTo, ShareToThread, ShareWhat, ShareWhatChat,
    ShareWithAgents,
};
use orch_api_contract::threads::ShareMode;
use orch_authz::{Caller, TokenGrant};
use orch_db::audit::Origin;
use orch_engine::sharing::ShareError;
use orch_types::{Time, new_id};
use shared::{Places, a_world_whose_agent_takes_tools};

#[tokio::test]
async fn chat_shares_and_copies_are_bounded_by_the_givers_current_chat_authority() {
    let world = a_world_whose_agent_takes_tools().await;
    let reader = world.thread("Reader").await;
    let chat = new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query(
        "INSERT INTO chats (id,kind,org_id,name,created_by) VALUES ($1,'group',$2,'Group',$3)",
    )
    .bind(chat)
    .bind(world.root)
    .bind(world.ada)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chat_members (chat_id,user_id,added_by,seq_updated) VALUES ($1,$2,$2,1)",
    )
    .bind(chat)
    .bind(world.ada)
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    let reader_token = Caller::token(
        world.ada,
        TokenGrant::from_stored(
            &[
                "chat.view".to_owned(),
                "thread.view".to_owned(),
                "thread.interact".to_owned(),
            ],
            None,
        )
        .unwrap(),
    );
    let writer_token = Caller::token(
        world.ada,
        TokenGrant::from_stored(
            &[
                "chat.view".to_owned(),
                "chat.interact".to_owned(),
                "thread.view".to_owned(),
                "thread.interact".to_owned(),
            ],
            None,
        )
        .unwrap(),
    );
    let session = Caller::session(world.ada);
    let engine = world.engine();
    let now = world.clock.now();
    let initial = Time::new(now + Duration::hours(1));
    let mut request = ShareWithAgents {
        read_first: None,
        what: ShareWhat::Chat(ShareWhatChat { chat_id: chat }),
        inside: None,
        download: Some(true),
        parts: Some(ShareParts {
            chat: Some(ShareLevel::Write),
            ..ShareParts::default()
        }),
        mode: ShareMode::Live,
        to: ShareTo::Threads(vec![ShareToThread { thread_id: reader }]),
        comment: None,
        expires_at: Some(initial),
        client_id: "bounded-chat".to_owned(),
    };
    // Asking for Write with Read-only authority still makes a useful Read share.
    let read = engine
        .share_with_agents(&reader_token, &Origin::default(), &request)
        .await
        .unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].grants.len(), 1);
    assert_eq!(read[0].grants[0].permission, "chat.view");
    assert_eq!(read[0].parts.chat, Some(ShareLevel::Read));
    let mut conn = world.pool.acquire().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='chat' AND object_id=$1 AND holder_kind='agents' AND removed_at IS NULL")
        .bind(chat).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(count, 1);
    drop(conn);

    request.mode = ShareMode::Snapshot;
    request.parts = Some(ShareParts {
        chat: Some(ShareLevel::Read),
        ..ShareParts::default()
    });
    let denied = engine
        .share_with_agents(&reader_token, &Origin::default(), &request)
        .await;
    assert!(matches!(denied, Err(ShareError::Denied(_))), "{denied:?}");

    // Download is optional on a live share: Read survives without an Export row.
    request.mode = ShareMode::Live;
    let read = engine
        .share_with_agents(&reader_token, &Origin::default(), &request)
        .await
        .unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].grants.len(), 1);
    assert_eq!(read[0].grants[0].permission, "chat.view");
    let add_write = ShareAgentUpdateBody {
        read_first: None,
        parts: Some(ShareParts {
            chat: Some(ShareLevel::Write),
            ..ShareParts::default()
        }),
        expires_at: None,
    };
    let unchanged = engine
        .update_share(&reader_token, &Origin::default(), read[0].id, &add_write)
        .await
        .unwrap();
    assert_eq!(unchanged.grants.len(), 1);
    assert_eq!(unchanged.grants[0].permission, "chat.view");

    // The session can give Write and Export. A token without Export extends only
    // its Read and Write rows. Removing Write and shortening remain allowed.
    request.parts = add_write.parts;
    let full = engine
        .share_with_agents(&session, &Origin::default(), &request)
        .await
        .unwrap();
    assert_eq!(full[0].grants.len(), 3);
    let extension = ShareAgentUpdateBody {
        read_first: None,
        parts: None,
        expires_at: Some(Time::new(now + Duration::hours(2))),
    };
    engine
        .update_share(&writer_token, &Origin::default(), full[0].id, &extension)
        .await
        .unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    for grant in &full[0].grants {
        let expiry: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
            "SELECT expires_at FROM permission_entries WHERE id=$1 AND removed_at IS NULL",
        )
        .bind(grant.entry_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        let expected = if grant.permission == "chat.export" {
            initial.get()
        } else {
            extension.expires_at.unwrap().get()
        };
        assert_eq!(expiry, Some(expected));
    }
    drop(conn);
    let narrower = ShareAgentUpdateBody {
        read_first: None,
        parts: Some(ShareParts {
            chat: Some(ShareLevel::Read),
            ..ShareParts::default()
        }),
        expires_at: Some(Time::new(now + Duration::minutes(30))),
    };
    let narrowed = engine
        .update_share(&reader_token, &Origin::default(), full[0].id, &narrower)
        .await
        .unwrap();
    assert!(
        !narrowed
            .grants
            .iter()
            .any(|grant| grant.permission == "chat.interact")
    );
    assert!(
        narrowed
            .grants
            .iter()
            .any(|grant| grant.permission == "chat.export")
    );
    engine
        .update_share(&session, &Origin::default(), full[0].id, &extension)
        .await
        .unwrap();
}
