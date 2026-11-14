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
use orch_types::{Hash, Id, Json, Time};
use serde::Serialize;
use std::collections::BTreeMap;

type Grants = BTreeMap<Id, (Holder, Json, Option<Time>)>;

#[derive(Serialize)]
enum Scope {
    Host(Id),
    Defaults(Option<Id>),
}

pub struct Before {
    objects: BTreeMap<Id, Grants>,
    intent: Hash,
}

async fn grants(conn: &mut Conn, principal: &Principal, object: ObjectRef) -> ApiResult<Grants> {
    Ok(orch_authz::engine::resource_permission_grants(
        conn,
        object,
        Permission::HostUse,
        principal.now(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|entry| (entry.id, (entry.holder, entry.options, entry.expires_at)))
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
        Ok(Some(
            Self::capture(
                conn,
                principal,
                vec![object.id],
                Scope::Host(object.id),
                body,
            )
            .await?,
        ))
    }

    /// Inspect only protected hosts actually reached by this defaults scope, without naming them.
    pub async fn defaults(
        conn: &mut Conn,
        principal: &Principal,
        node: Option<Id>,
        body: &PageChange,
    ) -> ApiResult<Self> {
        let ids: Vec<Id> = sqlx::query_scalar("SELECT h.id FROM hosts h JOIN machines m ON m.id=h.machine_id LEFT JOIN nodes n ON n.id=h.home_id WHERE h.bundled_with_server AND m.deleted_at IS NULL AND ($1::uuid IS NULL OR h.home_id=$1 OR $1=ANY(n.path)) ORDER BY h.id")
            .bind(node).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        Self::capture(conn, principal, ids, Scope::Defaults(node), body).await
    }

    async fn capture(
        conn: &mut Conn,
        principal: &Principal,
        ids: Vec<Id>,
        scope: Scope,
        body: &PageChange,
    ) -> ApiResult<Self> {
        let mut objects = BTreeMap::new();
        for id in ids {
            objects.insert(
                id,
                grants(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Host,
                        id,
                    },
                )
                .await?,
            );
        }
        let mut canonical = serde_json::to_value((&scope, principal.user(), body, &objects))
            .map_err(ApiError::internal)?;
        canonical.sort_all_objects();
        let intent = Hash::of(&serde_json::to_vec(&canonical).map_err(ApiError::internal)?);
        Ok(Self { objects, intent })
    }

    pub async fn disclosure(
        &self,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<Option<ServerHostUseDisclosure>> {
        let mut hosts = 0_i64;
        for (id, before) in &self.objects {
            let fresh = grants(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: *id,
                },
            )
            .await?;
            if fresh
                .iter()
                .any(|(id, grant)| before.get(id) != Some(grant))
            {
                hosts = hosts
                    .checked_add(1)
                    .ok_or_else(|| ApiError::internal("Too many affected server hosts."))?;
            }
        }
        if hosts == 0 {
            return Ok(None);
        }
        Ok(Some(ServerHostUseDisclosure {
            hosts,
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
            "Confirm Host Use on hosts installed with the server; their containers may share the server's kernel.")
            .with("server_host_use", disclosure))
    }
}
