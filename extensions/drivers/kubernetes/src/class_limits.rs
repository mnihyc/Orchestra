//! The shared runtime-class limitations for a Kubernetes environment.

use std::collections::BTreeMap;

use orch_driver_contract::isolation::{RuntimeLimit, SUBJECT_MOUNTS, SUBJECT_VOLUME_CARRY};

const MOUNT: &str = "To change a mount, the environment restarts.";
const DEVICE: &str = "To change a device, the environment restarts.";
const MOVE: &str = "To take this environment to another host, copy its files first: Kubernetes volumes do not travel with a move.";
const HOST_PATH: &str = "Host paths are not offered on this cluster: use a volume.";
const GPU: &str = "A GPU is not offered on this cluster: use a runc environment on a Docker host with the GPU capability.";
const NETWORK: &str = "To restrict an environment's network, run it on a Docker host: Orch cannot yet prove that a cluster enforces a restricted network.";

fn limit(key: &str, touches: &[&str], sentence: &str) -> RuntimeLimit {
    RuntimeLimit {
        key: key.into(),
        touches: touches.iter().map(|subject| (*subject).into()).collect(),
        todo: None,
        why: sentence.into(),
    }
}

/// The network instruction stays present until a real per-class proof exists.
pub(crate) fn limits(network_verified: bool) -> Vec<RuntimeLimit> {
    let mut limits = vec![
        limit("kubernetes_mount_restart", &[SUBJECT_MOUNTS], MOUNT),
        limit("kubernetes_device_restart", &["kvm", "gpu"], DEVICE),
        limit("kubernetes_volume_move", &[SUBJECT_VOLUME_CARRY], MOVE),
        limit("kubernetes_host_path", &[SUBJECT_MOUNTS], HOST_PATH),
        limit("kubernetes_gpu", &["gpu"], GPU),
    ];
    if !network_verified {
        limits.push(limit("kubernetes_network_unverified", &[], NETWORK));
    }
    limits
}

/// Use the identical text in the capability's unavailable reason and keyed limit.
pub(crate) fn unavailable() -> BTreeMap<String, String> {
    BTreeMap::from([("gpu".into(), GPU.into())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_class_data_describes_the_affected_choices_and_network_check() {
        let entries = limits(false);
        let find = |key: &str| entries.iter().find(|entry| entry.key == key).unwrap();
        assert_eq!(find("kubernetes_mount_restart").sentence(), MOUNT);
        assert!(find("kubernetes_mount_restart").touches_any(&[SUBJECT_MOUNTS]));
        assert_eq!(find("kubernetes_volume_move").sentence(), MOVE);
        assert!(find("kubernetes_volume_move").touches_any(&[SUBJECT_VOLUME_CARRY]));
        assert_eq!(find("kubernetes_gpu").sentence(), unavailable()["gpu"]);
        assert!(find("kubernetes_gpu").touches_any(&["gpu"]));
        assert_eq!(find("kubernetes_network_unverified").sentence(), NETWORK);
        assert!(
            limits(true)
                .iter()
                .all(|limit| limit.key != "kubernetes_network_unverified")
        );
    }
}
