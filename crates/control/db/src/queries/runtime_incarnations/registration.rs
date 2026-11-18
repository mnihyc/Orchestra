//! A Stop withdraws its original logical registration, never an independent new home or profile.

use crate::{Conn, DbError};
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Resource {
    Provider {
        id: Id,
        owner: Id,
        home: Id,
        org: Id,
        machine: Option<Id>,
        profile: Option<Id>,
        harness: Option<String>,
        provider_kind: String,
    },
    Environment {
        id: Id,
        owner: Id,
        home: Id,
        org: Id,
        machine: Option<Id>,
        placement: String,
        environment_generation: i32,
    },
}
/// Factual original placement, without settings, credentials or a permission grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    runtime_generation: i32,
    resource: Resource,
}

impl Registration {
    /// Capture only the actual original placement in the accepting resource transaction.
    pub async fn capture(conn: &mut Conn, runtime: Id) -> Result<Option<Self>, DbError> {
        let value: Option<Json> = sqlx::query_scalar("SELECT orch_runtime_registration($1)")
            .bind(runtime)
            .fetch_one(conn)
            .await?;
        value
            .map(|value| {
                serde_json::from_value(value).map_err(|_| DbError::Rule {
                    constraint: "runtime_stop_registration".into(),
                })
            })
            .transpose()
    }

    /// Match through the same factual predicate used by serving and dispatch readers.
    pub async fn matches(
        &self,
        conn: &mut Conn,
        control: &super::super::runtime_controls::Intent,
    ) -> Result<bool, DbError> {
        let snapshot = serde_json::to_value(self).map_err(|_| DbError::Rule {
            constraint: "runtime_stop_registration".into(),
        })?;
        Ok(
            sqlx::query_scalar("SELECT orch_runtime_registration_matches($1,$2,$3)")
                .bind(snapshot)
                .bind(control.runtime())
                .bind(control.id())
                .fetch_one(conn)
                .await?,
        )
    }
}
