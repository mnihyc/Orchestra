//! The provider's original deletion writer, on its caller's guarded transaction.
use crate::{Engine, EngineError, resource_pause::ResourceStop, unit::Finished};
use chrono::{DateTime, Utc};
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::resource_facts::FactTarget;
use orch_db::{
    Conn,
    queries::{jobs::JobRow, resources::routes},
};
use orch_types::Id;

/// The caller retains ordinary/current authority and appends its native audit before commit.
/// The same stop, interruption, cleanup journal and secret erasure serve Delete and Move.
pub async fn apply(
    engine: &Engine,
    conn: &mut Conn,
    row: &routes::ProviderRow,
    by: Id,
    now: DateTime<Utc>,
) -> Result<(JobRow, Finished), EngineError> {
    let stopped = engine
        .stop_resource_in(
            conn,
            ResourceStop {
                target: FactTarget {
                    object: ObjectRef {
                        kind: ObjectKind::Provider,
                        id: row.id,
                    },
                    model_id: None,
                },
                name: &row.name,
                by,
                note: None,
                deleted: true,
            },
            now,
        )
        .await?;
    routes::delete_provider(conn, row.id, now).await?;
    routes::delete_provider_secrets_except(conn, row.id, &[]).await?;
    Ok(stopped)
}

/// The reviewed native source and exact work set, not a permission or a reservation.
pub async fn fingerprint(
    conn: &mut Conn,
    row: &routes::ProviderRow,
) -> Result<orch_types::Hash, EngineError> {
    let target = FactTarget {
        object: ObjectRef {
            kind: ObjectKind::Provider,
            id: row.id,
        },
        model_id: None,
    };
    let mut work = crate::resource_facts::activity::read(conn, &target).await?;
    work.sort_by(|a, b| (&a.kind, a.id, a.thread).cmp(&(&b.kind, b.id, b.thread)));
    let work: Vec<_> = work.iter().map(|item| serde_json::json!({"kind":item.kind,"id":item.id,"count":item.count,"thread":item.thread,"segment":item.segment,"since":item.since})).collect();
    let mut requests = crate::resource_facts::activity::requests(conn, &target).await?;
    requests.sort_unstable();
    let models: serde_json::Value = sqlx::query_scalar("SELECT coalesce(jsonb_agg(to_jsonb(m) ORDER BY m.id),'[]'::jsonb) FROM provider_models m WHERE provider_id=$1").bind(row.id).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let bytes = serde_json::to_vec(
        &serde_json::json!({"revision":row.revision,"name":row.name,"home":row.home_id,"org":row.org_id,"kind":row.kind,"harness":row.harness_kind,"agent":row.agent_id,"machine":row.machine_id,"keeper":row.key_machine_id,"held_by":row.held_by_environment_id,"work":work,"requests":requests,"models":models}),
    )?;
    Ok(orch_types::Hash::of(&bytes))
}

/// An unfinished source handover keeps its original custody exclusive until success or return.
pub async fn pending_move(
    conn: &mut Conn,
    source: Id,
    except: Option<Id>,
) -> Result<Option<Id>, EngineError> {
    Ok(sqlx::query_scalar("SELECT id FROM jobs WHERE kind='provider_provision' AND state IN ('queued','running') AND plan#>>'{provider_creator,origin,operation}'='ProviderMoveOut' AND plan#>>'{provider_creator,duplicate,source}'=$1::uuid::text AND ($2::uuid IS NULL OR id<>$2) ORDER BY created_at,id LIMIT 1").bind(source).bind(except).fetch_optional(conn).await.map_err(orch_db::DbError::from)?)
}
