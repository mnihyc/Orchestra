//! Read the actual attempt's saved root policy and leaf under the current reader's access.
use crate::EngineError;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::threads::{Turn, TurnPoolChoice};
use orch_authz::Principal;
use orch_db::{Conn, DbError};
use orch_provider_contract::pools::Policy;
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet};

#[derive(sqlx::FromRow)]
struct Receipt {
    turn: Id,
    choice: Id,
    root: Id,
    path: Vec<Id>,
    provider: Id,
    model: Option<String>,
    policy: Option<sqlx::types::Json<Policy>>,
}

/// Add the pool receipt to turns already authorized by their thread reader. Names of
/// nested members require View on the whole path. The history read checks again on expansion.
pub async fn project(
    conn: &mut Conn,
    principal: &Principal,
    turns: &mut [Turn],
) -> Result<(), EngineError> {
    if turns.is_empty() || !super::visible::may_read(principal) {
        return Ok(());
    }
    TurnReceipts::load(conn, turns)
        .await?
        .project(conn, principal, turns)
        .await
}

pub struct TurnReceipts(Vec<Receipt>);

impl TurnReceipts {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub async fn load(conn: &mut Conn, turns: &[Turn]) -> Result<Self, EngineError> {
        if turns.is_empty() {
            return Ok(Self(Vec::new()));
        }
        let ids: Vec<_> = turns.iter().map(|turn| turn.id).collect();
        let rows: Vec<Receipt> = sqlx::query_as(
        "WITH attempts AS (
             SELECT DISTINCT attempt_id, choice_id FROM pool_attempt_scopes WHERE attempt_id=ANY($1)
         )
         SELECT a.attempt_id AS turn, c.read_id AS choice, (c.target->>'id')::uuid AS root,
                ARRAY(SELECT (h->>'pool_id')::uuid FROM jsonb_array_elements(c.path) h) AS path,
                COALESCE(c.leaf #>> '{backend,offering,provider}',c.leaf->>'id')::uuid AS provider,
                c.leaf #>> '{backend,offering,offering}' AS model,
                (SELECT p->'policy' FROM jsonb_array_elements(c.policies) p
                  WHERE p->>'pool_id'=c.target->>'id' LIMIT 1) AS policy
         FROM attempts a JOIN pool_choices c ON c.id=a.choice_id
         WHERE c.state='selected' AND c.target->>'type'='pool' AND c.leaf->>'kind' IN ('provider','model')
           AND COALESCE(c.leaf #>> '{backend,offering,provider}',c.leaf->>'id') IS NOT NULL
           AND NOT EXISTS(SELECT 1 FROM attempts other WHERE other.attempt_id=a.attempt_id AND other.choice_id<>a.choice_id)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
        Ok(Self(rows))
    }

    pub async fn project(
        self,
        conn: &mut Conn,
        principal: &Principal,
        turns: &mut [Turn],
    ) -> Result<(), EngineError> {
        let rows = self.0;
        if rows.is_empty() || !super::visible::may_read(principal) {
            return Ok(());
        }
        let ids: BTreeSet<_> = rows
            .iter()
            .flat_map(|row| row.path.iter().copied().chain([row.root, row.provider]))
            .collect();
        let names = super::visible::names(conn, principal, ids).await?;
        let mut receipts = BTreeMap::new();
        for row in rows {
            if !names.contains_key(&(ObjectKind::Pool, row.root)) {
                continue;
            }
            let member = if names.contains_key(&(ObjectKind::Provider, row.provider))
                && row.path.first() == Some(&row.root)
                && row
                    .path
                    .iter()
                    .all(|pool| names.contains_key(&(ObjectKind::Pool, *pool)))
            {
                row.model
                    .or_else(|| names.get(&(ObjectKind::Provider, row.provider)).cloned())
            } else {
                None
            };
            if let Some(name) = names.get(&(ObjectKind::Pool, row.root)) {
                receipts.insert(
                    row.turn,
                    TurnPoolChoice {
                        pool_id: row.root,
                        pool_name: name.clone(),
                        choice_id: row.choice,
                        member,
                        policy: row.policy.map(|policy| policy.0),
                    },
                );
            }
        }
        for turn in turns {
            turn.pool_choice = receipts.remove(&turn.id);
        }
        Ok(())
    }
}
