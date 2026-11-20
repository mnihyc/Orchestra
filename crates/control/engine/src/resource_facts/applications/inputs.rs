//! Fingerprints of the resource inputs actually accepted by a native unit operation.

use crate::EngineError;
use crate::commands::Outgoing;
use chrono::{DateTime, Utc};
use orch_db::{Conn, DbError};
use orch_link_contract::commands::CommandKind;
use orch_types::{Id, Json};
use serde_json::json;

/// Compares the logical recipe, excluding the image facts supplied by an inspection.
pub(crate) fn template_fingerprint(driver: &str, definition: &Json) -> String {
    let mut definition = definition.clone();
    if let Some(fields) = definition.as_object_mut() {
        fields.remove("image_facts");
    }
    crate::updates::definition_hash(driver, &definition, &Json::Null, &[]).to_string()
}

/// An absent association is unreported; an observed empty set is known to contain no resources.
pub(crate) fn mount_fingerprints(mounts: &Json) -> Option<Json> {
    let mounts = mounts.as_array()?;
    let mut result = serde_json::Map::new();
    for mount in mounts {
        let id = mount.get("resource_id")?.as_str()?.parse::<Id>().ok()?;
        let mut value = mount.clone();
        value.sort_all_objects();
        if result
            .insert(
                id.to_string(),
                json!(orch_types::Hash::of(value.to_string().as_bytes()).to_string()),
            )
            .is_some()
        {
            return None;
        }
    }
    Some(Json::Object(result))
}

/// Records the durable accepted body, rather than a later configuration re-resolved during replay.
pub(crate) async fn record_command(
    conn: &mut Conn,
    command: &Outgoing,
    result: &Json,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    if !matches!(
        command.kind,
        CommandKind::NodeRuntimeCreate | CommandKind::NodeMountsApply
    ) || command.kind == CommandKind::NodeMountsApply
        && result.get("applied").and_then(Json::as_bool) != Some(true)
    {
        return Ok(());
    }
    let recorded: Option<(Json, DateTime<Utc>)> =
        sqlx::query_as("SELECT body,created_at FROM commands WHERE id=$1 AND state='succeeded'")
            .bind(command.id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let (body, ordered_at) = recorded.unwrap_or_else(|| (command.body.clone(), now));
    let Some(runtime) = body
        .get("runtime_id")
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok())
    else {
        return Err(EngineError::unreadable(
            "the accepted runtime command has no runtime identity",
        ));
    };
    let mounts = body.get("mounts").and_then(mount_fingerprints);
    if command.kind == CommandKind::NodeRuntimeCreate {
        let template = body
            .get("driver")
            .and_then(Json::as_str)
            .zip(body.get("spec"))
            .map(|(driver, definition)| template_fingerprint(driver, definition));
        sqlx::query("INSERT INTO runtime_resource_inputs(runtime_id,template_fingerprint,mount_fingerprints,mount_command_at,mount_command_id,recorded_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(runtime_id) DO UPDATE SET template_fingerprint=COALESCE(runtime_resource_inputs.template_fingerprint,EXCLUDED.template_fingerprint),mount_fingerprints=CASE WHEN (runtime_resource_inputs.mount_command_at,runtime_resource_inputs.mount_command_id)<=(EXCLUDED.mount_command_at,EXCLUDED.mount_command_id) THEN EXCLUDED.mount_fingerprints ELSE runtime_resource_inputs.mount_fingerprints END,mount_command_at=GREATEST(runtime_resource_inputs.mount_command_at,EXCLUDED.mount_command_at),mount_command_id=CASE WHEN (runtime_resource_inputs.mount_command_at,runtime_resource_inputs.mount_command_id)<=(EXCLUDED.mount_command_at,EXCLUDED.mount_command_id) THEN EXCLUDED.mount_command_id ELSE runtime_resource_inputs.mount_command_id END,recorded_at=EXCLUDED.recorded_at")
            .bind(runtime).bind(template).bind(mounts).bind(ordered_at).bind(command.id).bind(now)
            .execute(conn).await.map_err(DbError::from)?;
    } else {
        sqlx::query("INSERT INTO runtime_resource_inputs(runtime_id,template_fingerprint,mount_fingerprints,mount_command_at,mount_command_id,recorded_at) VALUES($1,NULL,$2,$3,$4,$5) ON CONFLICT(runtime_id) DO UPDATE SET mount_fingerprints=EXCLUDED.mount_fingerprints,mount_command_at=EXCLUDED.mount_command_at,mount_command_id=EXCLUDED.mount_command_id,recorded_at=EXCLUDED.recorded_at WHERE (runtime_resource_inputs.mount_command_at,runtime_resource_inputs.mount_command_id)<=(EXCLUDED.mount_command_at,EXCLUDED.mount_command_id)")
            .bind(runtime).bind(mounts).bind(ordered_at).bind(command.id).bind(now)
            .execute(conn).await.map_err(DbError::from)?;
    }
    Ok(())
}
