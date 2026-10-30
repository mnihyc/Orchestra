//! The template spec as the driver reads it: the members of the Docker spec, their defaults, and what makes one wrong.
//!
//! A template's `spec` is in the driver's own schema; Orch itself reads only a few members of it and leaves the
//! rest to the driver. This module reads the members the driver acts on ([`DockerSpec`]) and checks them. The same
//! reading serves the check a template is saved with and the making of a unit, so a spec that was accepted can be
//! made. The places inside a unit that the driver fixes are named once, in [`layout`].

use std::net::IpAddr;

use orch_driver_contract::driver::DriverError;
use orch_driver_contract::mounts::check_target;
use orch_driver_contract::runtime::{LAUNCH_VARIABLES, Resources, RuntimeKind};
use orch_driver_contract::sources::{Source, SourceError};
use orch_types::Json;
use serde::Deserialize;

use crate::capabilities::CapabilityPatch;

/// The places inside a unit that belong to Orch and not to the image: nothing else may be mounted over or under
/// them, and the variables of the launch point at them.
pub mod layout {
    /// The directory that holds `orch-connector`, bound read-only from the tool cache.
    pub const CONNECTOR_DIR: &str = "/opt/orch/bin";
    /// The connector, which is the unit's main process.
    pub const CONNECTOR_PROGRAM: &str = "/opt/orch/bin/orch-connector";
    /// The directory with a directory for each harness build, each bound read-only from the tool cache.
    pub const TOOLS_DIR: &str = "/opt/orch/tools";
    /// The directory of the files that are for one run of the unit: the enrollment token, the authority, the list
    /// of mounts. It is a volume of the unit's own, only the unit's user can enter, and the token in it is
    /// emptied when the unit stops.
    pub const RUN_DIR: &str = "/run/orch";
    /// Where the connector's state volume is mounted, and the connector's data directory.
    pub const STATE_DIR: &str = "/var/lib/orch";
    /// The file that holds the certificate authority.
    pub const CA_FILE: &str = "/run/orch/ca.pem";
    /// The file that holds the enrollment token.
    pub const ENROLL_FILE: &str = "/run/orch/enroll";
    /// The file that lists the mounts that were attached.
    pub const MOUNTS_FILE: &str = "/run/orch/mounts.json";
    /// The places no mount of an environment may be on, or contain, or lie inside, besides its workspace and
    /// home.
    pub const RESERVED: [&str; 3] = ["/opt/orch", RUN_DIR, STATE_DIR];
}

/// Where the workspace volume is mounted unless the spec says another place.
pub const DEFAULT_WORKSPACE_MOUNT: &str = "/workspace";

/// Where the home volume is mounted unless the spec says another place.
pub const DEFAULT_HOME_MOUNT: &str = "/home/agent";

/// Who the unit's processes run as: a name the image knows, or numbers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct UserSpec {
    /// The user's name; looked up in the image when the numbers are not given.
    #[serde(default)]
    pub name: Option<String>,
    /// The user's number.
    #[serde(default)]
    pub uid: Option<u32>,
    /// The group's number; the user's own when only the user's is given.
    #[serde(default)]
    pub gid: Option<u32>,
}

impl UserSpec {
    /// Whether the spec names a user at all.
    #[must_use]
    pub fn is_given(&self) -> bool {
        self.name.is_some() || self.uid.is_some()
    }

    /// The numbers, when the spec gives them.
    #[must_use]
    pub fn numbers(&self) -> Option<(u32, u32)> {
        self.uid.map(|uid| (uid, self.gid.unwrap_or(uid)))
    }

    /// The `User` of the container: the numbers when they are given, otherwise the name, otherwise the image's.
    #[must_use]
    pub fn container_user(&self) -> Option<String> {
        match (self.numbers(), &self.name) {
            (Some((uid, gid)), _) => Some(format!("{uid}:{gid}")),
            (None, Some(name)) => Some(name.clone()),
            (None, None) => None,
        }
    }
}

/// How a unit is connected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    /// The unit reaches the gateway and, beyond it, the internet.
    #[default]
    Bridge,
    /// The unit reaches the gateway and nothing else.
    None,
}

/// The `network` of the spec.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct NetworkSpec {
    /// How far the unit reaches.
    pub mode: NetworkMode,
    /// What the unit may reach out to: `open`, or `restricted` (Orch's transport and the other host names its place
    /// allows, through its host's proxy).
    pub egress: String,
    /// The name servers the unit uses; none means the network's own.
    pub dns: Vec<String>,
}

impl Default for NetworkSpec {
    fn default() -> Self {
        Self {
            mode: NetworkMode::Bridge,
            egress: "open".to_owned(),
            dns: Vec::new(),
        }
    }
}

impl NetworkSpec {
    /// How a unit of this network reaches out: through its proxy to Orch alone for `none`; through its proxy to
    /// Orch and its place's other names for `restricted`, or wherever its runtime's policy `required` restriction;
    /// and openly otherwise.
    #[must_use]
    pub fn reach(&self, required: bool) -> crate::restricted::Reach {
        use crate::restricted::Reach;
        match (self.mode, self.egress.as_str()) {
            (NetworkMode::None, _) => Reach::OnlyOrch,
            (_, "restricted") => Reach::Restricted,
            _ if required => Reach::Restricted,
            _ => Reach::Open,
        }
    }
}

/// The `security` of the spec.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SecuritySpec {
    /// Whether the unit's file system is read-only outside its volumes and mounts.
    pub read_only_rootfs: bool,
    /// Whether a process of the unit can never gain privileges it was not started with.
    pub no_new_privileges: bool,
    /// Kernel capabilities added to the unit.
    pub cap_add: Vec<String>,
    /// Kernel capabilities taken from the unit.
    pub cap_drop: Vec<String>,
}

/// What a mount of the template is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateMountKind {
    /// Memory.
    Tmpfs,
    /// A named volume (or an unnamed one).
    Volume,
    /// A directory of the host.
    Bind,
}

/// One entry of the spec's own `mounts`: part of the recipe. The data that a project or an environment attaches
/// comes in the runtime request instead.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TemplateMount {
    /// What kind of place it is.
    #[serde(rename = "type")]
    pub kind: TemplateMountKind,
    /// The volume's name or the host's directory; none for memory, and for a volume that is made for the unit.
    #[serde(default)]
    pub source: Option<String>,
    /// Where it is inside the unit.
    pub target: String,
    /// Whether the unit may write to it.
    #[serde(default)]
    pub read_only: bool,
}

/// The spec of a template (and of a runtime made from it) as the Docker driver reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct DockerSpec {
    /// Where the unit's file system comes from.
    pub source: Source,
    /// The container runtime to use; the daemon's default when absent.
    pub runtime: Option<String>,
    /// Who the unit runs as.
    pub user: UserSpec,
    /// Where the workspace volume is mounted (environments).
    pub workspace_mount: String,
    /// Where the home volume is mounted (environments).
    pub home_mount: String,
    /// How the unit is connected.
    pub network: NetworkSpec,
    /// The confinement the spec asks for.
    pub security: SecuritySpec,
    /// The template's own mounts.
    pub mounts: Vec<TemplateMount>,
    /// The members the capabilities merged in.
    pub patch: CapabilityPatch,
    /// The variables the spec sets, which `RuntimeSpec::env` repeats.
    pub env: Vec<(String, String)>,
}

/// The members of the spec that are read by their names. Members not listed are for Orch or are not known.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Raw {
    runtime: Option<String>,
    user: Option<UserSpec>,
    workspace_mount: Option<String>,
    home_mount: Option<String>,
    network: NetworkSpec,
    security: SecuritySpec,
    mounts: Vec<TemplateMount>,
    env: std::collections::BTreeMap<String, String>,
    resources: Option<Resources>,
    resources_max: Option<Resources>,
}

/// Whether `path` is, contains, or lies inside `other`.
fn overlaps(path: &str, other: &str) -> bool {
    let inside = |outer: &str, inner: &str| {
        inner == outer
            || inner
                .strip_prefix(outer)
                .is_some_and(|rest| rest.starts_with('/'))
    };
    inside(path, other) || inside(other, path)
}

/// Whether a name can be a runtime, a capability or a user: letters, digits and a few joining marks, at most
/// 64 characters (a name is a word, and a longer one is a mistake).
fn is_word(text: &str, extra: &[char]) -> bool {
    !text.is_empty()
        && text.len() <= MAX_WORD
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
}

/// The longest name of a runtime, a capability or a user that is accepted.
const MAX_WORD: usize = 64;

fn source_problem(error: &SourceError) -> String {
    error.to_string()
}

impl DockerSpec {
    /// Reads a spec of a template of the pool `kind`, and says everything that is wrong with it at once.
    pub fn parse(kind: RuntimeKind, spec: &Json) -> Result<Self, DriverError> {
        let mut problems: Vec<String> = Vec::new();
        let raw: Raw = match serde_json::from_value(spec.clone()) {
            Ok(raw) => raw,
            Err(error) => return Err(DriverError::InvalidSpec(error.to_string())),
        };
        let source = match Source::from_spec(spec) {
            Ok(source) => Some(source),
            Err(error) => {
                problems.push(source_problem(&error));
                None
            }
        };
        let patch = match CapabilityPatch::from_spec(spec) {
            Ok(patch) => patch,
            Err(error) => {
                problems.push(error);
                CapabilityPatch::default()
            }
        };
        let workspace_mount = raw
            .workspace_mount
            .clone()
            .unwrap_or_else(|| DEFAULT_WORKSPACE_MOUNT.to_owned());
        let home_mount = raw
            .home_mount
            .clone()
            .unwrap_or_else(|| DEFAULT_HOME_MOUNT.to_owned());
        let user = raw.user.clone().unwrap_or_default();

        match kind {
            RuntimeKind::Environment => {
                for (member, path) in [
                    ("workspace_mount", &workspace_mount),
                    ("home_mount", &home_mount),
                ] {
                    if let Err(error) = check_target(path) {
                        problems.push(format!("{member}: {error}"));
                    } else if layout::RESERVED.iter().any(|place| overlaps(place, path)) {
                        problems.push(format!("{member}: {path} is a place of Orch's own"));
                    }
                }
                if overlaps(&workspace_mount, &home_mount) {
                    problems.push("the workspace and the home are different places".to_owned());
                }
            }
            RuntimeKind::Provider => {
                if raw.workspace_mount.is_some() || raw.home_mount.is_some() {
                    problems.push(
                        "a provider's runtime has the volume state alone: no workspace_mount or home_mount"
                            .to_owned(),
                    );
                }
            }
        }

        if let Some(name) = &user.name
            && !is_word(name, &['_', '-', '.'])
        {
            problems.push(format!("user.name: {name:?} is not a user name"));
        }
        if raw.user.is_some() && !user.is_given() {
            problems.push("user: a name or a number is given".to_owned());
        }
        if let Some(runtime) = &raw.runtime
            && !is_word(runtime, &['_', '-', '.', '+'])
        {
            problems.push(format!("runtime: {runtime:?} is not the name of a runtime"));
        }
        match raw.network.egress.as_str() {
            "open" => {}
            "restricted" if raw.network.mode == NetworkMode::None => problems.push(
                "network.egress: restricted is for a unit on a network; network.mode none reaches Orch alone already"
                    .to_owned(),
            ),
            "restricted" if !raw.network.dns.is_empty() => problems.push(
                "network.egress: a restricted unit asks its proxy's resolver and takes no network.dns of its own"
                    .to_owned(),
            ),
            "restricted" => {}
            other => problems.push(format!(
                "network.egress: {other:?} is neither open nor restricted"
            )),
        }
        if raw.network.mode == NetworkMode::None && !raw.network.dns.is_empty() {
            problems.push(
                "network.dns: a unit without a way out asks its proxy's resolver alone".to_owned(),
            );
        }
        for server in &raw.network.dns {
            if server.parse::<IpAddr>().is_err() {
                problems.push(format!("network.dns: {server:?} is not an address"));
            }
        }
        for capability in raw.security.cap_add.iter().chain(&raw.security.cap_drop) {
            if !is_word(capability, &['_']) {
                problems.push(format!("security: {capability:?} is not a capability"));
            }
        }

        let mut taken: Vec<&str> = Vec::new();
        if kind == RuntimeKind::Environment {
            taken.push(&workspace_mount);
            taken.push(&home_mount);
        }
        for (index, mount) in raw.mounts.iter().enumerate() {
            let at = format!("mounts[{index}]");
            if let Err(error) = check_target(&mount.target) {
                problems.push(format!("{at}.target: {error}"));
                continue;
            }
            if layout::RESERVED
                .iter()
                .any(|place| overlaps(place, &mount.target))
            {
                problems.push(format!(
                    "{at}.target: {} is a place of Orch's own",
                    mount.target
                ));
            } else if taken.iter().any(|place| overlaps(place, &mount.target)) {
                problems.push(format!(
                    "{at}.target: {} is, contains or lies inside another mount, the workspace or the home",
                    mount.target
                ));
            }
            taken.push(&mount.target);
            match (mount.kind, &mount.source) {
                (TemplateMountKind::Tmpfs, Some(_)) => {
                    problems.push(format!("{at}: a tmpfs has no source"));
                }
                (TemplateMountKind::Bind, None) => {
                    problems.push(format!(
                        "{at}: a bind mount names the directory of the host"
                    ));
                }
                (TemplateMountKind::Bind, Some(source)) if !source.starts_with('/') => {
                    problems.push(format!("{at}.source: {source:?} is not an absolute path"));
                }
                _ => {}
            }
        }
        for name in raw.env.keys() {
            let valid = !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid {
                problems.push(format!("env: {name:?} is not a variable name"));
            } else if LAUNCH_VARIABLES
                .iter()
                .any(|variable| variable.name == name)
            {
                problems.push(format!("env: {name} is set by the launch"));
            }
        }
        if let (Some(wanted), Some(most)) = (&raw.resources, &raw.resources_max)
            && !wanted.within(most)
        {
            problems.push("resources are within resources_max".to_owned());
        }
        for limits in [raw.resources.as_ref(), raw.resources_max.as_ref()]
            .into_iter()
            .flatten()
        {
            if limits.memory_swap_mb.is_some_and(|swap| {
                swap <= 0 || limits.memory_mb.is_none_or(|memory| swap < memory)
            }) {
                problems.push("memory_swap_mb needs memory_mb and may not be smaller".into());
            }
            if limits
                .blkio_weight
                .is_some_and(|weight| !(10..=1000).contains(&weight))
            {
                problems.push("blkio_weight is between 10 and 1000".into());
            }
        }

        let Some(source) = source else {
            return Err(DriverError::InvalidSpec(problems.join("; ")));
        };
        if problems.is_empty() {
            Ok(Self {
                source,
                runtime: raw.runtime,
                user,
                workspace_mount,
                home_mount,
                network: raw.network,
                security: raw.security,
                mounts: raw.mounts,
                patch,
                env: raw.env.into_iter().collect(),
            })
        } else {
            Err(DriverError::InvalidSpec(problems.join("; ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn environment() -> Json {
        json!({
          "image": "ghcr.io/OWNER/orch-env-base:0.1.0", "image_digest": null, "build": null, "archive": null,
          "runtime": "runc", "user": { "name": "agent", "uid": 1000, "gid": 1000 },
          "workspace_mount": "/workspace", "home_mount": "/home/agent",
          "resources": { "cpu_millis": 4000, "memory_mb": 8192, "pids": 4096, "disk_gb": 50 },
          "resources_max": { "cpu_millis": 16000, "memory_mb": 65536, "pids": 32768, "disk_gb": 500 },
          "network": { "mode": "bridge", "egress": "open", "dns": [] },
          "security": { "read_only_rootfs": false, "no_new_privileges": false, "cap_add": [], "cap_drop": [] },
          "env": {}, "mounts": [], "sandbox": { "default": "open", "max": "open" },
          "exec": { "offers": ["plain"], "orch_tools": true }, "host_labels": []
        })
    }

    fn provider() -> Json {
        json!({ "image": "gcr.io/distroless/static-debian12:nonroot", "image_digest": null, "runtime": "runc",
            "user": { "name": "nonroot", "uid": 65532, "gid": 65532 },
            "resources": { "cpu_millis": 2000, "memory_mb": 2048, "pids": 512 },
            "network": { "mode": "bridge", "egress": "open", "dns": [] },
            "security": { "read_only_rootfs": true, "no_new_privileges": true, "cap_add": [], "cap_drop": ["ALL"] },
            "env": {}, "mounts": [ { "type": "tmpfs", "target": "/tmp" } ], "host_labels": [] })
    }

    #[test]
    fn the_shipped_templates_of_both_pools_are_specs_the_driver_reads() {
        let spec = DockerSpec::parse(RuntimeKind::Environment, &environment()).unwrap();
        assert_eq!(spec.user.container_user().as_deref(), Some("1000:1000"));
        assert_eq!(spec.workspace_mount, "/workspace");
        assert!(matches!(spec.source, Source::Image { .. }));
        let spec = DockerSpec::parse(RuntimeKind::Provider, &provider()).unwrap();
        assert!(spec.security.read_only_rootfs && spec.security.no_new_privileges);
        assert_eq!(spec.security.cap_drop, ["ALL"]);
        assert_eq!(spec.mounts[0].kind, TemplateMountKind::Tmpfs);
    }

    #[test]
    fn absent_members_take_their_defaults() {
        let spec =
            DockerSpec::parse(RuntimeKind::Environment, &json!({ "image": "busybox" })).unwrap();
        assert_eq!(spec.workspace_mount, "/workspace");
        assert_eq!(spec.home_mount, "/home/agent");
        assert_eq!(spec.network, NetworkSpec::default());
        assert_eq!(spec.user.container_user(), None);
    }

    #[test]
    fn a_user_is_the_numbers_when_there_are_numbers_and_otherwise_the_name() {
        let named = UserSpec {
            name: Some("agent".into()),
            ..UserSpec::default()
        };
        assert_eq!(named.container_user().as_deref(), Some("agent"));
        assert_eq!(named.numbers(), None);
        let uid_only = UserSpec {
            uid: Some(1000),
            ..UserSpec::default()
        };
        assert_eq!(uid_only.container_user().as_deref(), Some("1000:1000"));
    }

    #[test]
    fn everything_wrong_is_said_at_once() {
        let mut spec = environment();
        spec["image"] = json!("busybox");
        spec["build"] = json!({ "dockerfile": "FROM x" });
        spec["network"]["egress"] = json!("restricted");
        spec["network"]["dns"] = json!(["not-an-address"]);
        spec["workspace_mount"] = json!("workspace");
        spec["home_mount"] = json!("/run/orch/home");
        spec["mounts"] = json!([
            { "type": "tmpfs", "target": "/tmp" },
            { "type": "tmpfs", "target": "/tmp/inner" },
            { "type": "bind", "source": "relative", "target": "/data" },
            { "type": "tmpfs", "source": "x", "target": "/mem" },
            { "type": "volume", "target": "/opt/orch/bin" }
        ]);
        spec["env"] = json!({ "ORCH_ROLES": "x", "1bad": "y" });
        spec["resources"]["memory_mb"] = json!(1_000_000);
        spec["runtime"] = json!("run c");
        let DriverError::InvalidSpec(message) =
            DockerSpec::parse(RuntimeKind::Environment, &spec).unwrap_err()
        else {
            panic!("an invalid spec");
        };
        for expected in [
            "exactly one of image, build and archive",
            "network.egress",
            "network.dns",
            "workspace_mount",
            "home_mount: /run/orch/home is a place of Orch's own",
            "mounts[1].target: /tmp/inner is, contains or lies inside",
            "mounts[2].source",
            "mounts[3]: a tmpfs has no source",
            "mounts[4].target: /opt/orch/bin is a place of Orch's own",
            "env: ORCH_ROLES is set by the launch",
            "env: \"1bad\" is not a variable name",
            "resources are within resources_max",
            "runtime",
        ] {
            assert!(message.contains(expected), "{expected:?} in {message:?}");
        }
    }

    #[test]
    fn a_providers_runtime_has_no_workspace_or_home() {
        let mut spec = provider();
        spec["workspace_mount"] = json!("/workspace");
        assert!(DockerSpec::parse(RuntimeKind::Provider, &spec).is_err());
    }

    #[test]
    fn a_member_of_the_wrong_shape_is_refused_with_the_readers_own_words() {
        let error = DockerSpec::parse(
            RuntimeKind::Environment,
            &json!({ "image": "x", "user": 5 }),
        )
        .unwrap_err();
        assert!(matches!(error, DriverError::InvalidSpec(_)));
    }

    #[test]
    fn places_overlap_when_one_contains_the_other_and_not_when_a_name_starts_like_it() {
        assert!(overlaps("/run/orch", "/run/orch/x"));
        assert!(overlaps("/run/orch/x", "/run/orch"));
        assert!(overlaps("/a", "/a"));
        assert!(!overlaps("/workspace", "/workspace2"));
    }
}
