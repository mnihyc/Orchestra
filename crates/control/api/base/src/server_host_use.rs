//! Informed grants on the protected server host use the existing page transaction and native chains.

use crate::{
    middleware::requests::{ApiError, ApiResult},
    registry::Ctx,
};
use orch_access_contract::{
    decisions::ObjectRef,
    pages::Holder,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    access::{PageChange, ServerHostUseDisclosure},
    common::ErrorCode,
};
use orch_authz::Principal;
use orch_db::Conn;
use orch_types::{Hash, Id};
use std::collections::BTreeMap;

pub struct Before {
    object: ObjectRef,
    grants: BTreeMap<Id, Holder>,
    intent: Hash,
}

async fn grants(
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
) -> ApiResult<BTreeMap<Id, Holder>> {
    Ok(orch_authz::engine::resource_permission_grants(
        conn,
        object,
        Permission::HostUse,
        principal.now(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|entry| (entry.id, entry.holder))
    .collect())
}

impl Before {
    pub async fn take(
        conn: &mut Conn,
        principal: &Principal,
        object: ObjectRef,
        body: &PageChange,
    ) -> ApiResult<Option<Self>> {
        if object.kind != ObjectKind::Host {
            return Ok(None);
        }
        let bundled: bool = sqlx::query_scalar(
            "SELECT COALESCE((SELECT bundled_with_server FROM hosts WHERE id=$1),false)",
        )
        .bind(object.id)
        .fetch_one(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
        if !bundled {
            return Ok(None);
        }
        let grants = grants(conn, principal, object).await?;
        let intent = Hash::of(
            &serde_json::to_vec(&(object, principal.user(), body, &grants))
                .map_err(ApiError::internal)?,
        );
        Ok(Some(Self {
            object,
            grants,
            intent,
        }))
    }

    pub async fn disclosure(
        &self,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<Option<ServerHostUseDisclosure>> {
        let fresh = grants(conn, principal, self.object).await?;
        if !fresh
            .iter()
            .any(|(id, holder)| self.grants.get(id) != Some(holder))
        {
            return Ok(None);
        }
        Ok(Some(ServerHostUseDisclosure {
            host_id: self.object.id,
            confirmation: format!("server-host:{}", self.intent),
        }))
    }

    pub async fn require(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<()> {
        let Some(disclosure) = self.disclosure(conn, principal).await? else {
            return Ok(());
        };
        if ctx
            .headers
            .get("orch-server-host-use-confirmed")
            .and_then(|value| value.to_str().ok())
            == Some(disclosure.confirmation.as_str())
        {
            return Ok(());
        }
        Err(ApiError::new(ErrorCode::ConflictConfirmationRequired,
            "Confirm Host Use on the host installed with the server; its containers may share the server's kernel.")
            .with("server_host_use", disclosure))
    }
}
