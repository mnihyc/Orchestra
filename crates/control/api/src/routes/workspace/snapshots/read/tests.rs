//! A real held selected window cannot borrow a fresh empty window's proof.

use super::Source;
use crate::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use crate::rig::{PATIENCE, Scene};
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::PageQuery;
use orch_api_contract::workspace::ThreadSnapshotList;
use orch_db::queries::{
    identity::{self, NewApiToken},
    storage::{self, NewSnapshot},
};
use orch_types::{Id, new_id};
use serde_json::json;
use std::cell::RefCell;
use tokio::sync::oneshot;

struct Boundary {
    reached: oneshot::Sender<Vec<(Id, Id, Id)>>,
    resume: oneshot::Receiver<()>,
}

tokio::task_local! {
    static BOUNDARY: RefCell<Option<Boundary>>;
}

pub(super) async fn cut(sources: &[Source]) {
    let boundary = BOUNDARY
        .try_with(|held| held.borrow_mut().take())
        .ok()
        .flatten();
    if let Some(boundary) = boundary {
        let _ = boundary.reached.send(
            sources
                .iter()
                .map(|source| {
                    (
                        source.row.id,
                        source.governing_environment(),
                        source.row.environment_id,
                    )
                })
                .collect(),
        );
        let _ = boundary.resume.await;
    }
}

#[tokio::test]
async fn snapshot_list_held_rows_keep_their_actual_sources_when_the_current_window_becomes_empty()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    let world = &scene.rig.world;
    let snapshot = new_id();
    storage::begin_snapshot(
        &mut *world.conn().await,
        &NewSnapshot {
            id: snapshot,
            thread_id: Some(thread),
            environment_id: scene.on.environment,
            env_generation: 1,
            label: "Actual held Snapshot window words",
            selection: &json!({"include":["**"],"exclude":[]}),
            created_by: Some(scene.ada.id()),
            at: world.now(),
        },
    )
    .await?;
    let elsewhere = new_id();
    sqlx::query("INSERT INTO environments(id,name,owner_id,org_id,home_node_id,placement,workspace_root,generation,status)
        SELECT $1,'Other existing source',owner_id,org_id,home_node_id,'connected','/other',1,'running'
        FROM environments WHERE id=$2")
        .bind(elsewhere).bind(scene.on.environment).execute(&world.db.pool).await?;
    let rule = |environment| {
        json!({
            "target":{"kind":"environment","id":environment,"reach":"self"},
            "allow":[{"operation":"ThreadSnapshotList","actions":["read"]}]
        })
    };
    let full: KeyScope = serde_json::from_value(json!({"rules":[rule(scene.on.environment)]}))?;
    let token = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: token,
            user_id: scene.ada.id(),
            name: "Held selected Snapshot source",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(full)?),
    )
    .await?;
    let query = PageQuery::default();
    let control = world
        .app
        .call(ThreadSnapshotList, &[&thread])
        .token(&secret)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(control.status.as_u16(), 200);
    assert!(String::from_utf8_lossy(&control.raw).contains("Actual held Snapshot window words"));
    let (reached, waiting) = oneshot::channel();
    let (resume, blocked) = oneshot::channel();
    let read = world
        .app
        .call(ThreadSnapshotList, &[&thread])
        .token(&secret)
        .query(&query)
        .send_bytes();
    let read = BOUNDARY.scope(
        RefCell::new(Some(Boundary {
            reached,
            resume: blocked,
        })),
        read,
    );
    let write = async {
        let sources = waiting.await?;
        assert_eq!(
            sources,
            [(snapshot, scene.on.environment, scene.on.environment)],
            "the native held response must actually carry both selected source roles"
        );
        let narrow: KeyScope = serde_json::from_value(json!({"rules":[rule(elsewhere)]}))?;
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
            .bind(token)
            .bind(serde_json::to_value(narrow)?)
            .execute(&world.db.pool)
            .await?;
        resume
            .send(())
            .map_err(|()| "the native held read ended before release")?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (reply, written) =
        tokio::time::timeout(PATIENCE, async { tokio::join!(read, write) }).await?;
    written?;
    assert_eq!(reply.status.as_u16(), 403);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Actual held Snapshot window words"));
    let empty = world
        .app
        .call(ThreadSnapshotList, &[&thread])
        .token(&secret)
        .query(&query)
        .send_bytes()
        .await;
    assert_eq!(
        empty.status.as_u16(),
        200,
        "the narrowed operation still has genuine checked-empty admission"
    );
    let body: serde_json::Value = serde_json::from_slice(&empty.raw)?;
    assert_eq!(body["items"], json!([]));
    Ok(())
}
