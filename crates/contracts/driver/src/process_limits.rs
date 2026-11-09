//! Native process control facts and resource properties shared by scopes and directory groups.

use crate::descriptor::{ResourceLimitSupport, ResourceLimits};
use crate::runtime::Resources;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Each controller is proved independently, on the machine and account that will run the work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessControls {
    /// A processor quota is enforced.
    pub cpu: bool,
    /// A resident-memory ceiling is enforced.
    pub memory: bool,
    /// A task ceiling is enforced.
    pub pids: bool,
}

impl ProcessControls {
    /// Whether any native process control was proved.
    #[must_use]
    pub const fn any(self) -> bool {
        self.cpu || self.memory || self.pids
    }

    /// A process group never implies a filesystem quota.
    #[must_use]
    pub fn support(self) -> ResourceLimitSupport {
        let mut reasons = std::collections::BTreeMap::from([(
            "disk".into(),
            "No filesystem quota is configured for this directory data.".into(),
        )]);
        for (key, supported, name) in [
            ("cpu", self.cpu, "CPU quota"),
            ("memory", self.memory, "memory ceiling"),
            ("pids", self.pids, "process ceiling"),
        ] {
            if !supported {
                reasons.insert(
                    key.into(),
                    format!("The running account has no working {name} control."),
                );
            }
        }
        ResourceLimitSupport {
            reasons,
            cpu: if self.cpu {
                ResourceLimits::Enforced
            } else {
                ResourceLimits::Reported
            },
            memory: if self.memory {
                ResourceLimits::Enforced
            } else {
                ResourceLimits::Reported
            },
            pids: if self.pids {
                ResourceLimits::Enforced
            } else {
                ResourceLimits::Reported
            },
            disk: ResourceLimits::Reported,
        }
    }

    /// Positive declared ceilings for the controllers actually available. No disk quota is invented.
    #[must_use]
    pub fn properties(self, resources: &Resources, reset: bool) -> Vec<String> {
        let mut properties = Vec::new();
        if self.cpu {
            match resources.cpu_millis {
                Some(cpu) => properties.push(format!("CPUQuota={}.{}%", cpu / 10, cpu % 10)),
                None if reset => properties.push("CPUQuota=".into()),
                None => {}
            }
        }
        if self.memory {
            match resources.memory_mb {
                Some(memory) => {
                    properties.push(format!("MemoryMax={memory}M"));
                    // The contract names a combined total; the native controller takes swap alone.
                    let swap = resources
                        .memory_swap_mb
                        .unwrap_or(memory)
                        .saturating_sub(memory);
                    properties.push(format!("MemorySwapMax={swap}M"));
                }
                None if reset => {
                    properties.push("MemoryMax=infinity".into());
                    properties.push("MemorySwapMax=infinity".into());
                }
                None => {}
            }
        }
        if self.pids {
            match resources.pids {
                Some(pids) => properties.push(format!("TasksMax={pids}")),
                None if reset => properties.push("TasksMax=infinity".into()),
                None => {}
            }
        }
        properties
    }
}

/// Probe values fit a single short-lived command; controllers are not inferred from one another.
pub const PROBES: [&str; 3] = ["CPUQuota=100%", "MemoryMax=64M", "TasksMax=64"];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_memory_limits_disable_swap_by_default_and_translate_explicit_combined_totals() {
        let controls = ProcessControls {
            memory: true,
            ..Default::default()
        };
        for (total, swap_property) in [
            (None, "MemorySwapMax=0M"),
            (Some(64), "MemorySwapMax=0M"),
            (Some(192), "MemorySwapMax=128M"),
        ] {
            assert_eq!(
                controls.properties(
                    &Resources {
                        memory_mb: Some(64),
                        memory_swap_mb: total,
                        ..Default::default()
                    },
                    false,
                ),
                vec!["MemoryMax=64M", swap_property]
            );
        }
        assert!(controls.properties(&Resources::default(), false).is_empty());
        assert_eq!(
            controls.properties(&Resources::default(), true),
            vec!["MemoryMax=infinity", "MemorySwapMax=infinity"]
        );
    }

    #[test]
    fn partial_native_support_never_promises_other_controllers_or_a_disk_quota() {
        let controls = ProcessControls {
            cpu: true,
            memory: false,
            pids: true,
        };
        let support = controls.support();
        assert_eq!(support.cpu, ResourceLimits::Enforced);
        assert_eq!(support.memory, ResourceLimits::Reported);
        assert_eq!(support.pids, ResourceLimits::Enforced);
        assert_eq!(support.disk, ResourceLimits::Reported);
        let resources = Resources {
            cpu_millis: Some(1250),
            memory_mb: Some(128),
            memory_swap_mb: Some(256),
            pids: Some(8),
            disk_gb: Some(1),
            ..Default::default()
        };
        assert_eq!(
            controls.properties(&resources, false),
            vec!["CPUQuota=125.0%", "TasksMax=8"]
        );
        assert_eq!(
            controls.properties(&Resources::default(), true),
            vec!["CPUQuota=", "TasksMax=infinity"]
        );
    }
}
