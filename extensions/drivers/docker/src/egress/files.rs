//! The files a node and a unit's proxy share: the orders the node writes, the state the proxy answers, and the node's heartbeat.
//!
//! They lie in one directory for each unit, which the node keeps and binds into the proxy's container. Each file
//! is written whole under another name and then renamed, so that a reader never sees half of one. The proxy
//! reads its orders and the heartbeat again and again; the node reads the proxy's state when it reports the
//! unit's network. A proxy whose heartbeat stops (the node is gone, or has lost its control link and says so by
//! not beating) closes every outward flow until it beats again.

use std::io;
use std::path::Path;

use orch_driver_contract::network::NetworkDenial;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::admission::Admission;

/// The orders: what the proxy admits, and whether anything may flow.
pub const ORDERS: &str = "orders.json";

/// The proxy's answer of what is in force.
pub const STATE: &str = "state.json";

/// The second of the node's last heartbeat, as text.
pub const HEARTBEAT: &str = "heartbeat";

/// What the node tells a unit's proxy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Orders {
    /// What it admits.
    pub admission: Admission,
    /// Whether outward flows may run at all: false until the policy is known to be the current one.
    pub open: bool,
}

/// What a unit's proxy answers of itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// The revision of the other names in force; none before the first orders.
    pub revision: Option<String>,
    /// The revision of the trusted endpoints in force.
    pub always_revision: Option<String>,
    /// Whether outward flows run.
    pub flowing: bool,
    /// Why they do not, in a sentence fit to show.
    pub reason: Option<String>,
    /// The last request refused: its host and why, and nothing of the request.
    pub last_denied: Option<NetworkDenial>,
}

/// Writes `bytes` as the file `name` of `dir`, whole: under another name first, then renamed.
pub async fn write(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let partial = dir.join(format!(".{name}.partial"));
    tokio::fs::write(&partial, bytes).await?;
    tokio::fs::rename(&partial, dir.join(name)).await
}

/// Writes `value` as the JSON file `name` of `dir`, whole.
pub async fn write_json<T: Serialize>(dir: &Path, name: &str, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    write(dir, name, &bytes).await
}

/// Reads the JSON file `name` of `dir`; none when it is not there or cannot be read as one.
pub async fn read_json<T: DeserializeOwned>(dir: &Path, name: &str) -> Option<T> {
    let bytes = tokio::fs::read(dir.join(name)).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Writes the node's heartbeat: the second it beats at.
pub async fn beat(dir: &Path, at_secs: i64) -> io::Result<()> {
    write(dir, HEARTBEAT, at_secs.to_string().as_bytes()).await
}

/// The second of the node's last heartbeat; none when it never beat.
pub async fn last_beat(dir: &Path) -> Option<i64> {
    tokio::fs::read_to_string(dir.join(HEARTBEAT))
        .await
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn what_is_written_is_read_whole_and_a_missing_file_is_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_json::<Orders>(dir.path(), ORDERS).await, None);
        assert_eq!(last_beat(dir.path()).await, None);
        let orders = Orders {
            admission: Admission {
                revision: "r1".into(),
                allowed_hosts: vec!["pypi.org".into()],
                ..Admission::default()
            },
            open: true,
        };
        write_json(dir.path(), ORDERS, &orders).await.unwrap();
        assert_eq!(read_json::<Orders>(dir.path(), ORDERS).await, Some(orders));
        beat(dir.path(), 1_760_000_000).await.unwrap();
        assert_eq!(last_beat(dir.path()).await, Some(1_760_000_000));
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|name| !name.ends_with(".partial")),
            "{names:?}"
        );
    }
}
