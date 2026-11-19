//! Same actual session rows and HTTP ticket requests serve issue and redemption controls.
#![allow(
    dead_code,
    reason = "each finite ticket control uses a subset of the shared fixture"
)]

#[path = "../native_close_support/mod.rs"]
mod sessions;
pub(crate) use sessions::{Surface, scene};

use crate::{rig::Scene, support::World};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;

pub type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub struct Legacy {
    pub id: Id,
    pub secret: String,
}

pub async fn legacy(world: &World, user: Id) -> Proof<Legacy> {
    let id = new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Ticket continuation control",
            token_hash: &token_hash(&secret),
            scopes: &["*".to_owned()],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        None,
    )
    .await?;
    Ok(Legacy { id, secret })
}

pub fn selection(surface: Surface, control: bool) -> &'static str {
    match (surface, control) {
        (Surface::Browser, false) => "BrowserTicketCreate.view",
        (Surface::Browser, true) => "BrowserTicketCreate.control",
        (Surface::Display, false) => "DisplayTicketCreate.view",
        (Surface::Display, true) => "DisplayTicketCreate.control",
    }
}

pub fn rules(operation: &str, environment: Id, manage: bool) -> Vec<Json> {
    vec![
        json!({"target":{"kind":"environment","id":environment,"reach":"self"},
        "allow":[{"operation":operation,"actions":if manage {vec!["read","manage"]} else {vec!["read"]}}]}),
    ]
}

pub async fn request(
    scene: &Scene,
    surface: Surface,
    id: Id,
    token: &str,
    control: bool,
) -> RawReply {
    match surface {
        Surface::Browser => {
            scene
                .rig
                .world
                .app
                .call(orch_api_contract::browsers::BrowserTicketCreate, &[&id])
                .token(token)
                .body(&orch_api_contract::browsers::BrowserTicketCreateBody {
                    control: Some(control),
                })
                .send_bytes()
                .await
        }
        Surface::Display => {
            scene
                .rig
                .world
                .app
                .call(orch_api_contract::displays::DisplayTicketCreate, &[&id])
                .token(token)
                .body(&orch_api_contract::displays::DisplayTicketCreateBody {
                    control: Some(control),
                })
                .send_bytes()
                .await
        }
    }
}

pub fn ticket(reply: &RawReply, stage: &str) -> Proof<String> {
    assert_eq!(
        reply.status.as_u16(),
        200,
        "{stage}: {}",
        String::from_utf8_lossy(&reply.raw)
    );
    let value: Json = serde_json::from_slice(&reply.raw)?;
    assert!(
        value.get("ticket_expires_at").is_some(),
        "{stage}: actual ticket deadline absent"
    );
    Ok(value["ticket"]
        .as_str()
        .ok_or("actual ticket absent")?
        .to_owned())
}

/// A real legacy→modern policy change updates BOTH mutually exclusive stored modes.
/// Original captured legacy bounds remain on the issued ticket, not on the current DB row.
pub async fn current_scope(world: &World, id: Id, rules: Vec<Json>) -> Proof {
    let scope: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":rules}))?;
    assert!(scope.validate().is_empty());
    let changed =
        sqlx::query("UPDATE api_tokens SET scopes=ARRAY[]::text[],operation_scope=$2 WHERE id=$1")
            .bind(id)
            .bind(serde_json::to_value(scope)?)
            .execute(&world.db.pool)
            .await?;
    assert_eq!(changed.rows_affected(), 1);
    Ok(())
}
