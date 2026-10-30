//! The runtimes of the daemon as declared classes, with what holds a unit in under each and what does not work as it does elsewhere.
//!
//! The daemon lists the runtimes it has and cannot say what they are. The operator declares each one's boundary in
//! `[node.docker.runtimes.<name>]`; an undeclared runtime counts as a shared kernel. From the boundary this module
//! works out what the class can give (a guest with a kernel of its own is offered Docker inside and none of the
//! host's devices), which live changes the node can make to it (none that join the unit's namespaces from the
//! host), what its units cost the host and how long their connector needs to enrol, and the lines a person or an
//! agent is told. Names are only used to say how a runtime is called on screens; nothing here decides by one.

use std::collections::BTreeMap;

use orch_driver_contract::descriptor::LiveChanges;
use orch_driver_contract::isolation::{Boundary, RuntimeClass, RuntimeLimit, SUBJECT_MOUNTS};
use orch_driver_contract::network::EgressMode;

use crate::capabilities::{self, DOCKER_INSIDE};
use crate::client::DaemonFacts;
use crate::config::{DockerConfig, RuntimeSettings};

/// The name the daemon gives the shim of runc beside the short one: the same runtime, listed twice.
const RUNC_SHIM: &str = "io.containerd.runc.v2";

/// The short name of runc, which the shim's name is an alias of.
const RUNC: &str = "runc";

/// What a unit costs the host beyond its limits under a kernel in user space, in MiB: the sentry and its file
/// proxy, tens of MiB.
pub const USER_SPACE_OVERHEAD_MB: i64 = 64;

/// What a unit costs the host beyond its limits as a virtual machine, in MiB: the hypervisor, the shared-file daemon
/// and the guest's own kernel measured about 0.3 GiB at rest.
pub const VM_OVERHEAD_MB: i64 = 320;

/// How long the connector of a unit in a kernel of user space has to enrol, in seconds: the sandbox starts in
/// about a second, and the first start of an image fills its file proxy.
pub const USER_SPACE_ENROL_SECS: u64 = 180;

/// How long the connector of a unit in a virtual machine has to enrol, in seconds: the guest boots, mounts its shared
/// files and starts its agent before the connector's image can start, which took seconds on a host that is itself a
/// virtual machine.
pub const VM_ENROL_SECS: u64 = 300;

/// The name of a runtime as a screen calls it, where it is a well-known one; the name itself otherwise.
#[must_use]
pub fn title_of(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let known = if lower == RUNC || lower == RUNC_SHIM {
        Some("runc")
    } else if lower.starts_with("runsc") || lower.contains("gvisor") || lower.contains("runsc") {
        Some("gVisor")
    } else if lower.contains("kata") {
        Some("Kata")
    } else if lower.contains("sysbox") {
        Some("Sysbox")
    } else if lower == "crun" {
        Some("crun")
    } else {
        None
    };
    known.map_or_else(|| name.to_owned(), str::to_owned)
}

/// The class a unit is made under when its spec names none: the daemon's default runtime where it is a class, else
/// the first class.
#[must_use]
pub fn default_class(facts: &DaemonFacts, classes: &[RuntimeClass]) -> Option<String> {
    classes
        .iter()
        .find(|class| !facts.default_runtime.is_empty() && class.is_named(&facts.default_runtime))
        .or_else(|| classes.first())
        .map(|class| class.name.clone())
}

/// What the operator declared of a runtime, or nothing: the declaration of `name` among `declared`, which is also
/// found by its title's alias for runc.
fn declared_for<'a>(
    declared: &'a BTreeMap<String, RuntimeSettings>,
    name: &str,
) -> Option<&'a RuntimeSettings> {
    declared.get(name).or_else(|| {
        (name == RUNC || name == RUNC_SHIM)
            .then(|| declared.get(RUNC).or_else(|| declared.get(RUNC_SHIM)))
            .flatten()
    })
}

/// What holds a unit in under the runtime `name` of this configuration.
#[must_use]
pub fn boundary_of(config: &DockerConfig, name: &str) -> Boundary {
    declared_for(&config.runtimes, name)
        .map_or(Boundary::SharedKernel, |declared| declared.boundary)
}

/// The capability keys a class with this boundary can give, each with the schema of its patch.
pub(crate) fn capabilities_for(boundary: Boundary) -> BTreeMap<String, orch_types::Json> {
    let own_guest = boundary.has_kernel_of_its_own();
    capabilities::declared()
        .into_iter()
        .filter(|(key, _)| {
            if own_guest {
                // Whatever is given is given inside the guest: a debugger, the network and the container's filters
                // stay in it, and Docker of its own runs there. Nothing of the host's devices reaches it.
                matches!(
                    key.as_str(),
                    "ptrace" | "net_admin" | "unconfined" | DOCKER_INSIDE
                )
            } else {
                key != DOCKER_INSIDE
            }
        })
        .collect()
}

/// Why the capabilities the driver has but the class cannot give cannot be given, by key.
fn unavailable_for(boundary: Boundary) -> BTreeMap<String, String> {
    let mut reasons = BTreeMap::new();
    if boundary.has_kernel_of_its_own() {
        let kind = if boundary == Boundary::OwnKernel {
            "a virtual machine"
        } else {
            "a kernel in user space"
        };
        for (key, why) in [
            (
                "kvm",
                format!(
                    "{kind} has no hardware virtualisation of its own to give: virtual machines inside run by software emulation"
                ),
            ),
            (
                "gpu",
                format!("the host's graphics processors cannot be passed into {kind}"),
            ),
            (
                "privileged",
                format!("host devices cannot be passed into {kind}"),
            ),
            (
                "host_network",
                format!("{kind} has a network of its own; the host's cannot be given"),
            ),
            (
                "docker_socket",
                format!(
                    "the host's Docker socket cannot be reached from {kind}; ask for Docker inside instead"
                ),
            ),
        ] {
            reasons.insert(key.to_owned(), why);
        }
    } else {
        reasons.insert(
            DOCKER_INSIDE.to_owned(),
            "Docker inside needs a runtime with a kernel of its own, so that what it does stays inside; under a kernel shared with the host only the critical host socket or privileged mode give Docker".to_owned(),
        );
    }
    reasons
}

/// How risky a capability is under the boundary where that differs from its definition's own: a debugger and the
/// network's privileges act only inside a guest with a kernel of its own, so they are low, the container's filters
/// being off is moderate, and Docker inside is moderate.
fn risks_for(boundary: Boundary) -> BTreeMap<String, String> {
    if !boundary.has_kernel_of_its_own() {
        return BTreeMap::new();
    }
    [
        ("ptrace", "low"),
        ("net_admin", "low"),
        ("unconfined", "medium"),
        (DOCKER_INSIDE, "medium"),
    ]
    .into_iter()
    .map(|(key, risk)| (key.to_owned(), risk.to_owned()))
    .collect()
}

/// Whether software installed outside the volumes survives a stop under a runtime of this boundary unless the
/// operator declares otherwise: a container's own layer persists; a virtual machine's root is the shared layer of the
/// host; a kernel in user space keeps its changes in an overlay inside the sandbox that goes with it.
#[must_use]
pub const fn usually_keeps_software(boundary: Boundary) -> bool {
    !matches!(boundary, Boundary::UserSpaceKernel)
}

fn limit(key: &str, touches: &[&str], todo: Option<&str>, why: &str) -> RuntimeLimit {
    RuntimeLimit {
        key: key.to_owned(),
        touches: touches
            .iter()
            .map(|touched| (*touched).to_owned())
            .collect(),
        todo: todo.map(str::to_owned),
        why: why.to_owned(),
    }
}

/// What does not work as it does under a plain container, in the pairs a person or an agent is told.
fn limits_for(boundary: Boundary, keeps_software: bool) -> Vec<RuntimeLimit> {
    let mut limits = Vec::new();
    if boundary.has_kernel_of_its_own() {
        // Making the environment again keeps what it installed outside its volumes only where the runtime keeps a
        // unit's own files across a replacement.
        let (key, why) = if keeps_software {
            (
                "live_changes",
                "this runtime cannot change them while it runs".to_owned(),
            )
        } else {
            (
                "live_changes_lost",
                "this runtime cannot change them while it runs; software installed outside the volumes is removed".to_owned(),
            )
        };
        limits.push(limit(
            key,
            &[],
            Some("a mount or device change makes the environment again"),
            &why,
        ));
        limits.push(limit(
            "file_events",
            &[SUBJECT_MOUNTS],
            Some("read the file again instead of waiting for a change event"),
            "files changed outside this environment raise no file events inside it and show up about a second late",
        ));
        limits.push(limit(
            "kvm",
            &["kvm"],
            Some("run virtual machines by software emulation (QEMU without KVM)"),
            "this runtime gives no access to the host's hardware virtualisation",
        ));
        limits.push(limit(
            "host_network",
            &["host_network"],
            None,
            "the host's network cannot be given under this runtime",
        ));
    }
    if boundary == Boundary::UserSpaceKernel {
        limits.push(limit(
            "docker_inside_network",
            &[DOCKER_INSIDE],
            Some("run the containers you start inside with --network=host"),
            "this runtime has no firewall for Docker's own network: a container on it cannot reach the network, though -p still serves a port",
        ));
    }
    if !keeps_software {
        limits.push(limit(
            "software",
            &[],
            Some("keep what you install in the image or the volumes"),
            "software installed outside the volumes is lost whenever the environment stops",
        ));
    }
    limits
}

/// What the node does of a class's live changes: the driver's own where the unit shares the host's kernel and the
/// node's helper can join its namespaces, and nothing but what the gateway gives where the unit has a kernel of its
/// own, whose mounts and devices the helper cannot reach.
fn live_changes_for(boundary: Boundary, driver: &LiveChanges) -> LiveChanges {
    if boundary.has_kernel_of_its_own() {
        LiveChanges {
            gateway_given: driver.gateway_given.clone(),
            resources: false,
            mounts: false,
            capabilities: Vec::new(),
        }
    } else {
        driver.clone()
    }
}

/// What the probes found for each runtime, by name: whether a restricted unit under it reached nothing but its proxy.
pub type RestrictedProved = BTreeMap<String, bool>;

/// The classes of this daemon: one for each runtime it has, in the order of its names, the aliases of runc folded
/// into it. `driver` is what the driver declares for a unit that shares the host's kernel, which a class of that
/// boundary keeps and any other lies over; `daemon_restricted` is whether the daemon makes a restricted unit's network
/// as it must be, and `proved` what was found of each runtime on top of that.
#[must_use]
pub fn classes(
    facts: &DaemonFacts,
    config: &DockerConfig,
    driver: &LiveChanges,
    daemon_restricted: bool,
    proved: &RestrictedProved,
) -> Vec<RuntimeClass> {
    let mut seen_runc = false;
    facts
        .runtimes
        .iter()
        .filter_map(|name| {
            let is_runc = name == RUNC || name == RUNC_SHIM;
            if is_runc {
                if seen_runc {
                    return None;
                }
                seen_runc = true;
            }
            let canonical = if is_runc { RUNC } else { name.as_str() };
            let declared = declared_for(&config.runtimes, canonical);
            let boundary = declared.map_or(Boundary::SharedKernel, |declared| declared.boundary);
            let mut class = RuntimeClass::new(canonical, boundary);
            class.title = declared
                .and_then(|declared| declared.title.clone())
                .unwrap_or_else(|| title_of(canonical));
            if is_runc && facts.runtimes.iter().any(|other| other == RUNC_SHIM) && canonical == RUNC
            {
                class.aliases = vec![RUNC_SHIM.to_owned()];
            }
            class.capabilities = capabilities_for(boundary);
            class.unavailable = unavailable_for(boundary);
            class.risks = risks_for(boundary);
            class.live_changes = live_changes_for(boundary, driver);
            class.keeps_software_on_stop = declared
                .and_then(|declared| declared.keeps_software_on_stop)
                .unwrap_or_else(|| usually_keeps_software(boundary));
            class.recreate_preserves_files = class.keeps_software_on_stop;
            class.memory_overhead_mb = declared
                .and_then(|declared| declared.memory_overhead_mb)
                .unwrap_or(match boundary {
                    Boundary::OwnKernel => VM_OVERHEAD_MB,
                    Boundary::UserSpaceKernel => USER_SPACE_OVERHEAD_MB,
                    Boundary::SharedKernel | Boundary::Unbounded => 0,
                });
            class.enrol_timeout_secs = declared
                .and_then(|declared| declared.enrol_timeout_secs)
                .or(match boundary {
                    Boundary::OwnKernel => Some(VM_ENROL_SECS),
                    Boundary::UserSpaceKernel => Some(USER_SPACE_ENROL_SECS),
                    Boundary::SharedKernel | Boundary::Unbounded => None,
                });
            // A restricted unit is offered only where one was seen to reach nothing but its proxy: the daemon makes
            // the network as it must be, and, for a runtime that is not a plain container, a unit of that runtime on
            // such a network was tried.
            let restricted = daemon_restricted
                && (!boundary.has_kernel_of_its_own() || proved.get(canonical) == Some(&true));
            class.network_egress = if restricted {
                vec![EgressMode::Open, EgressMode::Restricted]
            } else {
                vec![EgressMode::Open]
            };
            class.limits = limits_for(boundary, class.keeps_software_on_stop);
            Some(class)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_driver_contract::descriptor::LiveChanges;

    fn facts(names: &[&str]) -> DaemonFacts {
        DaemonFacts {
            runtimes: names.iter().map(|name| (*name).to_owned()).collect(),
            ..DaemonFacts::default()
        }
    }

    fn config(declared: &[(&str, Boundary)]) -> DockerConfig {
        DockerConfig {
            runtimes: declared
                .iter()
                .map(|(name, boundary)| {
                    (
                        (*name).to_owned(),
                        RuntimeSettings {
                            boundary: *boundary,
                            ..RuntimeSettings::default()
                        },
                    )
                })
                .collect(),
            ..DockerConfig::default()
        }
    }

    fn driver_live() -> LiveChanges {
        LiveChanges {
            gateway_given: vec!["port_place".into()],
            resources: true,
            mounts: true,
            capabilities: vec!["kvm".into(), "docker_socket".into()],
        }
    }

    fn find<'a>(classes: &'a [RuntimeClass], name: &str) -> &'a RuntimeClass {
        classes.iter().find(|class| class.name == name).unwrap()
    }

    #[test]
    fn a_runtime_the_operator_declared_nothing_of_shares_the_hosts_kernel_whatever_its_name() {
        let classes = classes(
            &facts(&["io.containerd.runc.v2", "runc", "runsc", "kata"]),
            &DockerConfig::default(),
            &driver_live(),
            true,
            &RestrictedProved::new(),
        );
        assert_eq!(
            classes
                .iter()
                .map(|class| class.name.as_str())
                .collect::<Vec<_>>(),
            ["runc", "runsc", "kata"]
        );
        assert!(
            classes
                .iter()
                .all(|class| class.boundary == Boundary::SharedKernel)
        );
        assert!(
            classes
                .iter()
                .all(|class| !class.capabilities.contains_key(DOCKER_INSIDE))
        );
        assert_eq!(find(&classes, "runc").aliases, ["io.containerd.runc.v2"]);
        assert_eq!(find(&classes, "runsc").title, "gVisor");
        assert_eq!(find(&classes, "kata").title, "Kata");
    }

    #[test]
    fn a_guest_with_a_kernel_of_its_own_is_offered_docker_inside_and_none_of_the_hosts_devices() {
        let classes = classes(
            &facts(&["runc", "kata"]),
            &config(&[("kata", Boundary::OwnKernel)]),
            &driver_live(),
            true,
            &RestrictedProved::from([("kata".to_owned(), true)]),
        );
        let vm = find(&classes, "kata");
        let keys: Vec<_> = vm.capabilities.keys().map(String::as_str).collect();
        assert_eq!(keys, ["docker_inside", "net_admin", "ptrace", "unconfined"]);
        for key in ["kvm", "gpu", "privileged", "host_network", "docker_socket"] {
            assert!(
                vm.unavailable.contains_key(key),
                "{key} says why it is not offered"
            );
        }
        assert_eq!(vm.risk_of("docker_inside", "high"), "medium");
        assert_eq!(vm.risk_of("ptrace", "medium"), "low");
        let plain = find(&classes, "runc");
        assert!(
            plain.capabilities.contains_key("privileged")
                && !plain.capabilities.contains_key(DOCKER_INSIDE)
        );
        assert!(plain.unavailable.contains_key(DOCKER_INSIDE));
    }

    #[test]
    fn what_the_nodes_helper_changes_live_it_changes_only_for_a_unit_whose_namespaces_it_can_join()
    {
        let classes = classes(
            &facts(&["runc", "runsc", "kata"]),
            &config(&[
                ("runsc", Boundary::UserSpaceKernel),
                ("kata", Boundary::OwnKernel),
            ]),
            &driver_live(),
            true,
            &RestrictedProved::new(),
        );
        assert!(find(&classes, "runc").live_changes.mounts);
        for name in ["runsc", "kata"] {
            let live = &find(&classes, name).live_changes;
            assert!(!live.mounts && live.capabilities.is_empty(), "{name}");
            assert_eq!(
                live.gateway_given,
                ["port_place"],
                "the gateway's grants do not need the helper"
            );
        }
    }

    #[test]
    fn restricted_egress_is_offered_under_a_guest_only_where_a_unit_of_it_was_seen_to_hold() {
        let classes = classes(
            &facts(&["runc", "runsc", "kata"]),
            &config(&[
                ("runsc", Boundary::UserSpaceKernel),
                ("kata", Boundary::OwnKernel),
            ]),
            &driver_live(),
            true,
            &RestrictedProved::from([("kata".to_owned(), true)]),
        );
        assert!(
            find(&classes, "runc")
                .network_egress
                .contains(&EgressMode::Restricted)
        );
        assert!(
            find(&classes, "kata")
                .network_egress
                .contains(&EgressMode::Restricted)
        );
        assert!(
            !find(&classes, "runsc")
                .network_egress
                .contains(&EgressMode::Restricted)
        );
        let none = super::classes(
            &facts(&["runc"]),
            &DockerConfig::default(),
            &driver_live(),
            false,
            &RestrictedProved::new(),
        );
        assert_eq!(
            none[0].network_egress,
            vec![EgressMode::Open],
            "the daemon cannot make the network"
        );
    }

    #[test]
    fn software_installed_outside_the_volumes_is_kept_except_in_the_overlay_of_a_kernel_in_user_space()
     {
        let declared = |keeps: Option<bool>| DockerConfig {
            runtimes: BTreeMap::from([(
                "runsc".to_owned(),
                RuntimeSettings {
                    boundary: Boundary::UserSpaceKernel,
                    keeps_software_on_stop: keeps,
                    ..RuntimeSettings::default()
                },
            )]),
            ..DockerConfig::default()
        };
        let lost = classes(
            &facts(&["runsc"]),
            &declared(None),
            &driver_live(),
            false,
            &RestrictedProved::new(),
        );
        assert!(!lost[0].keeps_software_on_stop && !lost[0].recreate_preserves_files);
        assert!(lost[0].limits.iter().any(|limit| limit.key == "software"));
        let kept = classes(
            &facts(&["runsc"]),
            &declared(Some(true)),
            &driver_live(),
            false,
            &RestrictedProved::new(),
        );
        assert!(kept[0].keeps_software_on_stop && kept[0].recreate_preserves_files);
        assert!(kept[0].limits.iter().all(|limit| limit.key != "software"));
    }

    #[test]
    fn the_port_line_of_a_nested_docker_is_told_where_the_kernel_is_gvisors() {
        let classes = classes(
            &facts(&["runsc", "kata"]),
            &config(&[
                ("runsc", Boundary::UserSpaceKernel),
                ("kata", Boundary::OwnKernel),
            ]),
            &driver_live(),
            false,
            &RestrictedProved::new(),
        );
        let ports = |name: &str, subjects: &[&str]| {
            find(&classes, name)
                .limits_touching(subjects)
                .iter()
                .any(|limit| limit.key == "docker_inside_network")
        };
        assert!(ports("runsc", &[DOCKER_INSIDE]));
        assert!(
            !ports("runsc", &["ptrace"]),
            "told only where Docker inside is held or offered"
        );
        assert!(!ports("kata", &[DOCKER_INSIDE]));
    }

    #[test]
    fn what_a_unit_costs_beyond_its_limits_and_how_long_it_has_to_enrol_follow_the_boundary_or_the_operator()
     {
        let mut declared = config(&[
            ("kata", Boundary::OwnKernel),
            ("runsc", Boundary::UserSpaceKernel),
        ]);
        declared
            .runtimes
            .get_mut("runsc")
            .unwrap()
            .memory_overhead_mb = Some(128);
        declared
            .runtimes
            .get_mut("runsc")
            .unwrap()
            .enrol_timeout_secs = Some(90);
        let classes = classes(
            &facts(&["runc", "runsc", "kata"]),
            &declared,
            &driver_live(),
            false,
            &RestrictedProved::new(),
        );
        assert_eq!(find(&classes, "kata").memory_overhead_mb, VM_OVERHEAD_MB);
        assert_eq!(
            find(&classes, "kata").enrol_timeout_secs,
            Some(VM_ENROL_SECS)
        );
        assert_eq!(find(&classes, "runsc").memory_overhead_mb, 128);
        assert_eq!(find(&classes, "runsc").enrol_timeout_secs, Some(90));
        assert_eq!(find(&classes, "runc").memory_overhead_mb, 0);
        assert_eq!(find(&classes, "runc").enrol_timeout_secs, None);
    }
}
