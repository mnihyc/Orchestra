//! Runtimes as declared classes: what holds a unit in, and what each class can and cannot do.
//!
//! A container runtime is not one thing. Under `runc` a unit shares the host's kernel; under gVisor its kernel is
//! written in user space; under Kata it is a virtual machine with a kernel of its own. A driver such as `docker`
//! can only name the runtimes its daemon has, so each runtime is a **class** the driver declares, with what holds a
//! unit in (its [`Boundary`], which the operator declares because the daemon cannot tell), and every member of the
//! driver's declaration that does not hold the same way under it: the capabilities it can give, the live changes it
//! can make, whether restricted egress was proved under it, whether files outside the volumes survive, what a unit
//! costs the host on top of its limits, and how long its connector needs to enrol.
//!
//! The core never tests a runtime's name. It reads the declaration through [`DriverDescriptor::effective_for`], which
//! lays a class over the driver's own declaration, and the limits the class states ([`RuntimeLimit`]) are data: the
//! screens and the agents are told the same lines, filtered by what the environment has or can be given.

use std::collections::BTreeMap;

use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::descriptor::{DriverDescriptor, LiveChanges};
use crate::network::EgressMode;

/// What holds a unit in, declared by the operator. A runtime with no declared boundary counts as a shared kernel.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
pub enum Boundary {
    /// Nothing: the unit is the machine itself.
    #[serde(rename = "none")]
    Unbounded,
    /// A container on the host's kernel: runc, crun, Sysbox.
    #[default]
    #[serde(rename = "shared_kernel")]
    SharedKernel,
    /// A kernel written in user space, between the unit and the host's: gVisor.
    #[serde(rename = "user_space_kernel")]
    UserSpaceKernel,
    /// A virtual machine with a kernel of its own: Kata.
    #[serde(rename = "own_kernel")]
    OwnKernel,
}

impl Boundary {
    /// The boundary as it is written in a configuration and on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unbounded => "none",
            Self::SharedKernel => "shared_kernel",
            Self::UserSpaceKernel => "user_space_kernel",
            Self::OwnKernel => "own_kernel",
        }
    }

    /// The boundary a configuration names; none for a word that is not one.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        [
            Self::Unbounded,
            Self::SharedKernel,
            Self::UserSpaceKernel,
            Self::OwnKernel,
        ]
        .into_iter()
        .find(|boundary| boundary.as_str() == word)
    }

    /// Whether the unit's own kernel stands between its processes and the host's: a virtual machine or a kernel in
    /// user space. Whatever the unit is given is then given inside that kernel, and a helper that joins the unit's
    /// namespaces from the host cannot reach what it would change.
    #[must_use]
    pub const fn has_kernel_of_its_own(self) -> bool {
        matches!(self, Self::UserSpaceKernel | Self::OwnKernel)
    }
}

/// One thing that does not work, or works differently, under a class: the pair of what to do instead and why, or
/// the reason alone where nothing can be done instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeLimit {
    /// The row of the compatibility matrix it belongs to. The screens look their words up by it; `todo` and `why`
    /// are the words for a reader that has none.
    pub key: String,
    /// What it touches: capability keys, or the feature words `mounts` and `software`. Empty means everything
    /// there is, so that the limit is always told.
    #[serde(default)]
    pub touches: Vec<String>,
    /// What to do instead, as an instruction without its full stop. None where nothing can be done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<String>,
    /// Why, as a fragment: after `todo` in parentheses, or alone where there is no `todo`.
    pub why: String,
}

impl RuntimeLimit {
    /// Whether it touches what an environment has or can be given: it names no subject, or one of them is held,
    /// offered or allowed by the template.
    #[must_use]
    pub fn touches_any(&self, subjects: &[&str]) -> bool {
        self.touches.is_empty()
            || self
                .touches
                .iter()
                .any(|touched| subjects.contains(&touched.as_str()))
    }

    /// The limit as one sentence, without its full stop.
    #[must_use]
    pub fn sentence(&self) -> String {
        match &self.todo {
            Some(todo) => format!("{todo} ({})", self.why),
            None => self.why.clone(),
        }
    }
}

/// The subject word of an environment that has a mount attached beside its workspace.
pub const SUBJECT_MOUNTS: &str = "mounts";

/// The subject of moving an environment's owned volume data to another host.
pub const SUBJECT_VOLUME_CARRY: &str = "volume_carry";

/// What one runtime of a driver is, and how it differs from what the driver declares as a whole. Everything given
/// here replaces the driver's own member for a unit made under the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeClass {
    /// The name the daemon knows it by, which a template's `runtime` names.
    pub name: String,
    /// What it is called on screens: `runc`, `gVisor`, `Kata`; the name itself where the driver knows no better.
    pub title: String,
    /// The other names the daemon knows it by (the shim's own name beside the short one): a template naming one of
    /// them gets this class.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// What holds a unit in.
    #[serde(default)]
    pub boundary: Boundary,
    /// The capability keys this class can give, each with the schema of the driver's patch for it. A key that is
    /// not here is not offered under this class, and `unavailable` may say why.
    #[serde(default)]
    pub capabilities: BTreeMap<String, Json>,
    /// Why a capability the driver has cannot be given under this class, by key: the words a screen shows where
    /// it would be offered.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
    /// How risky a capability is under this class where that differs from its definition's own: `low`, `medium`,
    /// `high` or `critical`, by key. A guest that holds its own kernel makes `docker_inside` and a debugger moderate.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub risks: BTreeMap<String, String>,
    /// The changes made to a running unit under this class.
    #[serde(default)]
    pub live_changes: LiveChanges,
    /// The egress modes proved under this class: `restricted` only where a unit of this class was seen to reach
    /// nothing but its proxy.
    #[serde(default)]
    pub network_egress: Vec<EgressMode>,
    /// Whether a same-image replacement keeps the unit's own files outside the volumes.
    #[serde(default)]
    pub recreate_preserves_files: bool,
    /// Whether software installed outside the volumes survives a stop, an idle stop, a recycle and a restart.
    #[serde(default)]
    pub keeps_software_on_stop: bool,
    /// What each unit costs the host beyond its own limits, in MiB, counted when a host is judged full.
    #[serde(default)]
    pub memory_overhead_mb: i64,
    /// How long a new unit's connector has to enrol, in seconds, where that differs from the node's own figure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enrol_timeout_secs: Option<u64>,
    /// What does not work as it does elsewhere.
    #[serde(default)]
    pub limits: Vec<RuntimeLimit>,
}

impl RuntimeClass {
    /// A class that declares nothing beyond its name and boundary: no capability, no live change.
    #[must_use]
    pub fn new(name: impl Into<String>, boundary: Boundary) -> Self {
        let name = name.into();
        Self {
            title: name.clone(),
            name,
            aliases: Vec::new(),
            boundary,
            capabilities: BTreeMap::new(),
            unavailable: BTreeMap::new(),
            risks: BTreeMap::new(),
            live_changes: LiveChanges::default(),
            network_egress: vec![EgressMode::Open],
            recreate_preserves_files: false,
            keeps_software_on_stop: false,
            memory_overhead_mb: 0,
            enrol_timeout_secs: None,
            limits: Vec::new(),
        }
    }

    /// Whether `runtime` is this class: its name or one of its aliases.
    #[must_use]
    pub fn is_named(&self, runtime: &str) -> bool {
        self.name == runtime || self.aliases.iter().any(|alias| alias == runtime)
    }

    /// The limits that touch what an environment has or can be given: `subjects` are the capability keys it holds,
    /// is offered or is allowed, and the feature words of what it has attached.
    #[must_use]
    pub fn limits_touching(&self, subjects: &[&str]) -> Vec<&RuntimeLimit> {
        self.limits
            .iter()
            .filter(|limit| limit.touches_any(subjects))
            .collect()
    }

    /// The risk of capability `key` under this class where it differs from the definition's `fallback`.
    #[must_use]
    pub fn risk_of<'a>(&'a self, key: &str, fallback: &'a str) -> &'a str {
        self.risks.get(key).map_or(fallback, String::as_str)
    }
}

/// A runtime's name and what it is, as a screen shows the pair "Runs on · Runtime".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeIsolation {
    /// The driver the unit runs on.
    pub driver: String,
    /// What the driver is called.
    pub driver_title: String,
    /// The runtime the unit is made under; none where the driver has no classes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// What it is called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_title: Option<String>,
    /// What holds the unit in.
    pub boundary: Boundary,
    /// Whether software installed outside the volumes survives a stop; none where the class is not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keeps_software_on_stop: Option<bool>,
    /// What does not work as it does elsewhere, filtered to what the environment has or can be given.
    #[serde(default)]
    pub limits: Vec<RuntimeLimit>,
    /// Capabilities its template allows that this class cannot give, with the reason, by key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unavailable: BTreeMap<String, String>,
}

impl DriverDescriptor {
    /// The class named `runtime`, or the driver's default one where none is named.
    #[must_use]
    pub fn runtime_class(&self, runtime: Option<&str>) -> Option<&RuntimeClass> {
        let wanted = runtime.or(self.default_runtime.as_deref())?;
        self.runtimes.iter().find(|class| class.is_named(wanted))
    }

    /// The driver's declaration as it holds for a unit made under `runtime`: the class's capabilities, live
    /// changes, proved egress and kept files lie over the driver's. A driver with no such class is unchanged, and
    /// so is a driver that declares no classes at all.
    #[must_use]
    pub fn effective_for(&self, runtime: Option<&str>) -> Self {
        let mut effective = self.clone();
        if let Some(class) = self.runtime_class(runtime) {
            effective.capabilities = class.capabilities.clone();
            effective.live_changes = class.live_changes.clone();
            effective.network_egress = class.network_egress.clone();
            effective.recreate_preserves_files = class.recreate_preserves_files;
        }
        effective
    }
}

/// The member [`effective_json`] adds to a descriptor: the name of the class that was laid over it.
pub const EFFECTIVE_RUNTIME: &str = "effective_runtime";

/// The class a stored descriptor holds for `runtime`, or for the one it was laid over by, or its default.
#[must_use]
pub fn class_in(descriptor: &Json, runtime: Option<&str>) -> Option<RuntimeClass> {
    let object = descriptor.as_object()?;
    let wanted = runtime
        .or_else(|| object.get(EFFECTIVE_RUNTIME).and_then(Json::as_str))
        .or_else(|| object.get("default_runtime").and_then(Json::as_str))?;
    object
        .get("runtimes")?
        .as_array()?
        .iter()
        .filter_map(|class| serde_json::from_value::<RuntimeClass>(class.clone()).ok())
        .find(|class| class.is_named(wanted))
}

/// What a declared descriptor of JSON, as the system stores a host's drivers, comes to for a unit made under
/// `runtime`: the same lay-over as [`DriverDescriptor::effective_for`], on the document as it was reported, so that
/// readers that look at members by pointer need no other change. A document that is not a descriptor of classes is
/// returned as it is.
#[must_use]
pub fn effective_json(descriptor: &Json, runtime: Option<&str>) -> Json {
    let Some(object) = descriptor.as_object() else {
        return descriptor.clone();
    };
    let wanted = runtime.or_else(|| object.get("default_runtime").and_then(Json::as_str));
    let class = wanted.and_then(|wanted| {
        object
            .get("runtimes")
            .and_then(Json::as_array)
            .and_then(|classes| {
                classes.iter().find(|class| {
                    class.get("name").and_then(Json::as_str) == Some(wanted)
                        || class
                            .get("aliases")
                            .and_then(Json::as_array)
                            .is_some_and(|aliases| {
                                aliases.iter().any(|alias| alias.as_str() == Some(wanted))
                            })
                })
            })
    });
    let Some(class) = class else {
        return descriptor.clone();
    };
    let mut effective = object.clone();
    // Which class was laid over, so that a reader of the stored document knows what it is reading.
    if let Some(name) = class.get("name") {
        effective.insert(EFFECTIVE_RUNTIME.to_owned(), name.clone());
    }
    for (member, class_member) in [
        ("capabilities", "capabilities"),
        ("live_changes", "live_changes"),
        ("network_egress", "network_egress"),
        ("recreate_preserves_files", "recreate_preserves_files"),
    ] {
        if let Some(value) = class.get(class_member) {
            effective.insert(member.to_owned(), value.clone());
        }
    }
    Json::Object(effective)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn class(name: &str, boundary: Boundary) -> RuntimeClass {
        let mut class = RuntimeClass::new(name, boundary);
        class
            .capabilities
            .insert("ptrace".into(), json!({ "type": "object" }));
        class.network_egress = vec![EgressMode::Open, EgressMode::Restricted];
        class
    }

    fn docker() -> DriverDescriptor {
        let mut descriptor: DriverDescriptor = serde_json::from_value(json!({
            "name": "docker", "version": "1", "title": "Docker",
            "capabilities": { "ptrace": {}, "kvm": {} },
            "live_changes": { "mounts": true, "capabilities": ["kvm"] },
            "network_egress": ["open", "restricted"],
            "recreate_preserves_files": true
        }))
        .unwrap();
        let mut vm = class("kata", Boundary::OwnKernel);
        vm.network_egress = vec![EgressMode::Open];
        descriptor.runtimes = vec![class("runc", Boundary::SharedKernel), vm];
        descriptor.default_runtime = Some("runc".into());
        descriptor
    }

    #[test]
    fn a_runtime_with_no_declared_boundary_shares_the_hosts_kernel() {
        let read: RuntimeClass =
            serde_json::from_value(json!({ "name": "crun", "title": "crun" })).unwrap();
        assert_eq!(read.boundary, Boundary::SharedKernel);
        assert_eq!(Boundary::parse("own_kernel"), Some(Boundary::OwnKernel));
        assert_eq!(Boundary::parse("vm"), None);
        assert_eq!(
            serde_json::to_value(Boundary::Unbounded).unwrap(),
            json!("none")
        );
    }

    #[test]
    fn a_class_lies_over_the_drivers_declaration_for_the_units_made_under_it() {
        let descriptor = docker();
        let vm = descriptor.effective_for(Some("kata"));
        assert!(vm.capabilities.contains_key("ptrace") && !vm.capabilities.contains_key("kvm"));
        assert!(!vm.live_changes.mounts, "the class declares no live mount");
        assert_eq!(vm.network_egress, vec![EgressMode::Open]);
        assert!(!vm.recreate_preserves_files);
        assert_eq!(
            descriptor.effective_for(Some("nothing")),
            descriptor,
            "no such class, no change"
        );
        assert_eq!(
            descriptor.effective_for(None).network_egress,
            vec![EgressMode::Open, EgressMode::Restricted],
            "the default class applies where none is named"
        );
    }

    #[test]
    fn the_stored_document_is_laid_over_the_same_way_as_the_typed_one() {
        let descriptor = docker();
        let stored = serde_json::to_value(&descriptor).unwrap();
        for runtime in [None, Some("kata"), Some("runc"), Some("other")] {
            let typed = serde_json::to_value(descriptor.effective_for(runtime)).unwrap();
            let json = effective_json(&stored, runtime);
            for member in [
                "capabilities",
                "live_changes",
                "network_egress",
                "recreate_preserves_files",
            ] {
                assert_eq!(json[member], typed[member], "{member} under {runtime:?}");
            }
        }
        assert_eq!(
            effective_json(&json!("not a descriptor"), Some("kata")),
            json!("not a descriptor")
        );
    }

    #[test]
    fn only_the_limits_that_touch_what_an_environment_has_or_can_be_given_are_told() {
        let mut gvisor = class("runsc", Boundary::UserSpaceKernel);
        gvisor.limits = vec![
            RuntimeLimit {
                key: "live_changes".into(),
                touches: vec![],
                todo: Some("to change a mount, restart the environment".into()),
                why: "its units cannot be reached from the host".into(),
            },
            RuntimeLimit {
                key: "docker_inside_ports".into(),
                touches: vec!["docker_inside".into()],
                todo: Some(
                    "run containers you start inside with --network=host to serve a port".into(),
                ),
                why: "-p and --expose do not work here".into(),
            },
            RuntimeLimit {
                key: "host_network".into(),
                touches: vec!["host_network".into()],
                todo: None,
                why: "the host's network cannot be given here".into(),
            },
        ];
        let told = |subjects: &[&str]| -> Vec<String> {
            gvisor
                .limits_touching(subjects)
                .iter()
                .map(|limit| limit.key.clone())
                .collect()
        };
        assert_eq!(told(&[]), ["live_changes"]);
        assert_eq!(
            told(&["docker_inside"]),
            ["live_changes", "docker_inside_ports"]
        );
        assert_eq!(
            told(&["ptrace", "host_network"]),
            ["live_changes", "host_network"]
        );
        assert_eq!(
            gvisor.limits[1].sentence(),
            "run containers you start inside with --network=host to serve a port (-p and --expose do not work here)"
        );
        assert_eq!(
            gvisor.limits[2].sentence(),
            "the host's network cannot be given here"
        );
    }

    #[test]
    fn a_guest_with_a_kernel_of_its_own_is_told_from_one_that_shares_the_hosts() {
        assert!(Boundary::OwnKernel.has_kernel_of_its_own());
        assert!(Boundary::UserSpaceKernel.has_kernel_of_its_own());
        assert!(!Boundary::SharedKernel.has_kernel_of_its_own());
        assert!(!Boundary::Unbounded.has_kernel_of_its_own());
        let mut vm = class("kata", Boundary::OwnKernel);
        vm.risks.insert("docker_inside".into(), "medium".into());
        assert_eq!(vm.risk_of("docker_inside", "high"), "medium");
        assert_eq!(vm.risk_of("kvm", "high"), "high");
    }
}
