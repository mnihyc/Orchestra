//! One tag catalogue, namespace and inherited limit for places and flat resources.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::routes::nodes::invalid;
use orch_db::{Conn, queries::hierarchy};
use orch_types::Id;

/// Existing assignments can stay after a move; only additions must belong to this home path.
pub(crate) async fn check(
    conn: &mut Conn,
    home: Option<Id>,
    tags: &[Id],
    carried: &[Id],
) -> ApiResult<()> {
    let mut distinct = tags.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    let place = home.map_or(orch_settings::Place::System, orch_settings::Place::Node);
    let most = orch_settings::get::<i64>(
        conn,
        place,
        orch_settings_contract::registry::keys::TAGS_MAX_PER_PLACE,
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or_default();
    if i64::try_from(distinct.len()).unwrap_or(i64::MAX) > most {
        return Err(invalid(
            "tag_ids",
            &format!("a place carries at most {most} tags here"),
        ));
    }
    let found = hierarchy::tags_by_id(conn, &distinct).await?;
    let above = match home {
        Some(home) => hierarchy::with_ancestors(conn, &[home])
            .await?
            .into_iter()
            .find(|row| row.id == home)
            .map(|row| row.path)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    for (index, tag) in distinct.iter().enumerate() {
        if !found.iter().any(|definition| {
            definition.id == *tag && (above.contains(&definition.node_id) || carried.contains(tag))
        }) {
            return Err(invalid(
                &format!("tag_ids[{index}]"),
                "is no tag defined here or above this place",
            ));
        }
    }
    Ok(())
}
