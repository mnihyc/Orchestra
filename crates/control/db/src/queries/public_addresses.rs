//! One protected system-store snapshot, read at the moment a public address is used.

use super::settings::{self, SettingRow, Stored};
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_api_contract::public_addresses::{PublicAddressSnapshot, PublicAddresses};
use orch_types::Id;
use serde::{Deserialize, Serialize};

/// Reserved system-store value. It is deliberately absent from ordinary editable settings.
pub const KEY: &str = "system.public_addresses";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Value {
    seed: PublicAddresses,
    selected: Option<PublicAddresses>,
}

fn invalid() -> DbError {
    DbError::Rule {
        constraint: "public_addresses_value".to_owned(),
    }
}

fn decode(row: &SettingRow) -> Result<Value, DbError> {
    let value: Value = serde_json::from_value(row.value.clone()).map_err(|_| invalid())?;
    value.seed.validated().map_err(|_| invalid())?;
    if let Some(selected) = &value.selected {
        selected.validated().map_err(|_| invalid())?;
    }
    Ok(value)
}

fn snapshot(row: SettingRow) -> Result<PublicAddressSnapshot, DbError> {
    let value = decode(&row)?;
    Ok(PublicAddressSnapshot {
        from_file_seed: value.selected.is_none(),
        addresses: value.selected.unwrap_or(value.seed),
        revision: row.revision,
    })
}

/// Read the committed value. Only the first reader may seed an absent row; all later readers
/// use the winning stored seed, even if their process was started with a different file.
pub async fn read(
    conn: &mut Conn,
    seed: &PublicAddresses,
    at: DateTime<Utc>,
) -> Result<PublicAddressSnapshot, DbError> {
    if let Some(row) = settings::row(conn, None, KEY).await? {
        return snapshot(row);
    }
    let value = serde_json::to_value(Value {
        seed: seed.validated().map_err(|_| invalid())?,
        selected: None,
    })
    .map_err(|_| invalid())?;
    match settings::put(
        conn,
        None,
        KEY,
        &Stored {
            value: &value,
            locked: true,
            by: None,
            at,
        },
        Some(0),
    )
    .await?
    {
        settings::Put::Written(row) => snapshot(row),
        settings::Put::Stale => snapshot(
            settings::row(conn, None, KEY)
                .await?
                .ok_or(DbError::Retry)?,
        ),
    }
}

/// A publication either replaces the whole current revision or leaves it entirely unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// Both effective address fields at their new committed revision.
    Written(PublicAddressSnapshot),
    /// Another publication or reset won the expected revision.
    Stale,
}

/// Publish a prepared candidate at its admitted revision. Preparation belongs to the caller;
/// there is no ordinary setting route that can bypass it.
pub async fn publish(
    conn: &mut Conn,
    candidate: &PublicAddresses,
    expected: i64,
    by: Option<Id>,
    at: DateTime<Utc>,
) -> Result<Write, DbError> {
    let selected = candidate.validated().map_err(|_| invalid())?;
    let Some(row) = settings::row(conn, None, KEY).await? else {
        return Ok(Write::Stale);
    };
    if row.revision != expected {
        return Ok(Write::Stale);
    }
    let mut value = decode(&row)?;
    value.selected = Some(selected);
    let value = serde_json::to_value(value).map_err(|_| invalid())?;
    Ok(
        match settings::put(
            conn,
            None,
            KEY,
            &Stored {
                value: &value,
                locked: true,
                by,
                at,
            },
            Some(expected),
        )
        .await?
        {
            settings::Put::Written(row) => Write::Written(snapshot(row)?),
            settings::Put::Stale => Write::Stale,
        },
    )
}
