//! Editable built-in defaults: the system's defaults page, capabilities, profiles, the two seeded templates and the rate catalogue a release comes with; no rate card and no caps.
//!
//! What a fresh system starts with, as data that its admins edit afterwards.
//! [`seed`] runs at every start of the API, in one transaction: each thing it
//! makes has a fixed identity (a key, a well-known identifier, one profile for
//! each kind of adapter). Untouched older image-based environment templates
//! become build templates; an admin's edits are never undone. A thing
//! that is gone altogether is made again; a template or a profile that was
//! removed is only marked removed, so it stays gone. A template or a profile
//! is made together with its Use row, which names everybody, and only then:
//! an admin who narrowed that row keeps it narrowed.
//!
//! The system's defaults page and the roles belong to the access engine.
//! Nothing here is a rate card or a usage cap: those are for the people who
//! set a place's limits. The settings whose default is "what the system was
//! seeded with" (the template to make environments from, the ready-made
//! provider endpoints, the policy of each capability) are not rows of
//! `settings`: they are supplied to whoever resolves the key, so that clearing
//! a value brings the seeded one back and a release can improve it.
//! [`DEFAULT_TEMPLATE_ID`], [`PROVIDER_RUNTIME_TEMPLATE_ID`] and
//! [`provider_presets`] are what is supplied.
//!
//! One template is made only on demand: **Local embeddings**
//! ([`local_embeddings_template_spec`]), when an admin adds the provider that runs
//! the embeddings model of search by meaning; a start never makes it.
//!
//! The rate catalogue a release comes with is loaded by
//! [`load_shipped_catalogue`], apart from [`seed`], because a catalogue that
//! cannot be read must not stop the rest from being seeded.

use crate::audit::{self, Actor, Origin};
use crate::queries::access::{self, NewEntry, PageKey};
use crate::{Conn, DbError};
use chrono::{DateTime, NaiveDate, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_settings_contract::registry::{self, keys};
use orch_types::{Id, Json};
use serde_json::json;

/// The key of the advisory lock under which a start seeds, so that services
/// that start together seed once. Any constant will do.
const SEED_LOCK: i64 = 0x6f72_6368_5f73_6564;

/// The identifier of the seeded template for environments, **Default**. It is
/// what `env.template_id` resolves to until an org, a project, a topic or the
/// environment itself says otherwise.
pub const DEFAULT_TEMPLATE_ID: Id = Id::from_u128(0x0199_cd9b_9f19_4e0a_8856_dc26_29c1_0257);

/// The identifier of the seeded template **Developer tools**: the Default template's recipe on the image that also
/// holds Docker Engine, QEMU and the usual build tools. It names no runtime of its own beyond the Default's: the
/// runtime is chosen when an environment is made.
pub const DEVELOPER_TEMPLATE_ID: Id = Id::from_u128(0x019a_3d51_72be_4b3a_8c0e_6f4d_5c2b_81e7);

/// The identifier of the seeded template for providers' runtimes, **Provider
/// runtime**. It is what `runtime.template_id` resolves to until an org or a
/// provider says otherwise.
pub const PROVIDER_RUNTIME_TEMPLATE_ID: Id =
    Id::from_u128(0xd3f5_635f_d929_47a1_8251_4e43_99db_0443);

/// The only version of the rate catalogue's file that is read.
const CATALOGUE_FORMAT: i64 = 1;

/// The unit the figures of the catalogue are in: US dollars at the vendors'
/// list prices, because that is what vendors publish.
const CATALOGUE_UNIT: &str = "USD";

/// The most characters of the name of an agent profile.
const AGENT_NAME_MAX_CHARS: usize = 80;

/// The values of the seeded template for environments. They are the limits
/// of one unit and what an environment declares it needs; the template holds
/// them and an admin changes them there.
mod default_template {
    /// The recipe is also used to package the release image.
    pub const DOCKERFILE: &str = include_str!("../../../../images/orch-env-base/Dockerfile");
    /// Four CPUs: a language server, a build and a test run at once.
    pub const CPU_MILLIS: i64 = 4_000;
    /// 8 GiB of memory, in MiB.
    pub const MEMORY_MB: i64 = 8_192;
    /// Processes at once: a build tool forks many.
    pub const PIDS: i64 = 4_096;
    /// 50 GB of disk for the workspace and the home volume together.
    pub const DISK_GB: i64 = 50;
    /// The most the CPUs of one environment may be raised to: sixteen.
    pub const CPU_MILLIS_MAX: i64 = 16_000;
    /// The most the memory of one environment may be raised to: 64 GiB.
    pub const MEMORY_MB_MAX: i64 = 65_536;
    /// The most processes of one environment.
    pub const PIDS_MAX: i64 = 32_768;
    /// The most disk of one environment, in GB.
    pub const DISK_GB_MAX: i64 = 500;
    /// What an environment declares it needs at least: 2 GiB of memory, a
    /// quarter of what its unit may use, so that a host counts as full when
    /// its environments would be cramped and not when they could use most.
    pub const NEEDS_MEMORY_MB: i64 = 2_048;
    /// One CPU.
    pub const NEEDS_CPU_MILLIS: i64 = 1_000;
    /// 10 GB of disk.
    pub const NEEDS_DISK_GB: i64 = 10;
    /// The user and group the agent runs as inside the unit.
    pub const AGENT_ID: i64 = 1_000;
}

/// What Developer tools gives beyond the Default template: images with Docker, QEMU and a compiler are larger,
/// and a build or a container of its own needs more of everything.
mod developer_tools_template {
    /// The base and this stage form one Dockerfile, even on a host with no local base image.
    pub const DOCKERFILE: &str = concat!(
        include_str!("../../../../images/orch-env-base/Dockerfile"),
        "\n",
        include_str!("../../../../images/orch-env-dev/Dockerfile")
    );
    /// Four CPUs, as the Default template.
    pub const CPU_MILLIS: i64 = 4_000;
    /// 8 GiB, in MiB, as the Default template: a build and a container of its own run beside the agent.
    pub const MEMORY_MB: i64 = 8_192;
    /// Processes at once: Docker inside forks a process for each container and each build step.
    pub const PIDS: i64 = 8_192;
    /// The most processes of one environment.
    pub const PIDS_MAX: i64 = 65_536;
    /// 100 GB of disk: the image holds the toolchains, and images pulled inside are kept beside the files.
    pub const DISK_GB: i64 = 100;
    /// The most disk of one environment, in GB.
    pub const DISK_GB_MAX: i64 = 1_000;
    /// What it declares it needs at least: 4 GiB of memory, because Docker's daemon and a compiler run beside the
    /// agent.
    pub const NEEDS_MEMORY_MB: i64 = 4_096;
    /// What it declares it needs at least: 20 GB of disk.
    pub const NEEDS_DISK_GB: i64 = 20;
}

/// The values of the seeded template for providers' runtimes: a harness
/// process and some tens of sessions.
mod provider_runtime_template {
    /// A pinned base with the C/C++ libraries the native harness needs, no
    /// shell or package manager, and no context or release image to publish.
    pub const DOCKERFILE: &str = include_str!("../../../../images/orch-provider-base/Dockerfile");
    /// Two CPUs.
    pub const CPU_MILLIS: i64 = 2_000;
    /// 2 GiB of memory, in MiB: one harness process takes about 270 MiB and a
    /// few MiB more for each session.
    pub const MEMORY_MB: i64 = 2_048;
    /// Processes at once.
    pub const PIDS: i64 = 512;
    /// The unprivileged user of the image, which has no shell.
    pub const NONROOT_ID: i64 = 65_532;
}

/// The values of the template **Local embeddings**: llama.cpp's server with
/// EmbeddingGemma 2 (Q8_0) beside the connector, answering the Embeddings
/// format on the runtime's own loopback. They are a template's ordinary
/// defaults: an admin changes them there, for every provider made from it.
pub mod local_embeddings_template {
    /// A pinned base, llama.cpp's pinned build and the model, both checked by
    /// their SHA-256 when the image is built on its host.
    pub const DOCKERFILE: &str = include_str!("../../../../images/orch-embeddings/Dockerfile");
    /// Four CPUs: what the model was measured with, about two hundred tokens a
    /// second, and the threads the server is given below.
    pub const CPU_MILLIS: i64 = 4_000;
    /// 2 GiB of memory, in MiB: the server with the model takes about 0.7 GiB
    /// and the connector some tens of MiB.
    pub const MEMORY_MB: i64 = 2_048;
    /// Processes at once.
    pub const PIDS: i64 = 512;
    /// The unprivileged user it runs as.
    pub const NONROOT_ID: i64 = 65_532;
    /// The loopback address the server answers on, inside the runtime; the
    /// provider's endpoint names it.
    pub const ENDPOINT: &str = "http://127.0.0.1:8090/v1";
    /// The model, by the name the server gives it.
    pub const MODEL: &str = "embeddinggemma-2";
    /// What the model's card asks to be put before a query, and before a
    /// document with its title.
    pub const QUERY_PREFIX: &str = "task: search result | query: ";
    /// See [`QUERY_PREFIX`].
    pub const DOCUMENT_PREFIX: &str = "title: {title} | text: ";
    /// The length its vectors are kept at: a third of its own 768 numbers for
    /// about one point of quality.
    pub const DIMENSIONS: u32 = 256;
}

/// What a release brings that the seeds need.
#[derive(Debug, Clone, Copy)]
pub struct Release<'a> {
    /// The version written into images built from this release's recipes.
    pub version: &'a str,
    /// The adapters this build has built in; each gets one profile.
    pub adapters: &'a [BuiltInAdapter<'a>],
}

/// An adapter that is built in, which the system has a profile for.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInAdapter<'a> {
    /// Its kind, such as `codex`.
    pub kind: &'a str,
    /// What its profile is called.
    pub title: &'a str,
    /// Whether it may run inside environments by default: the template the system is made with lets it.
    pub colocated: bool,
}

/// What a start made. All zero when nothing was missing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Seeded {
    /// Capability definitions made.
    pub capabilities: u64,
    /// Templates made.
    pub templates: u64,
    /// Agent profiles made.
    pub agents: u64,
    /// Use rows made, one for each template and profile made.
    pub use_rows: u64,
}

impl Seeded {
    /// Whether the start made nothing.
    #[must_use]
    pub fn nothing(&self) -> bool {
        *self == Self::default()
    }
}

/// Why seeding failed.
#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    /// The database refused.
    #[error(transparent)]
    Db(#[from] DbError),
    /// An adapter's title cannot be the name of a profile.
    #[error("the title of the adapter {kind:?} must be 1 to {AGENT_NAME_MAX_CHARS} characters")]
    AgentName {
        /// The adapter.
        kind: String,
    },
    /// The rate catalogue of the release cannot be read.
    #[error("the rate catalogue cannot be read: {path}: {reason}")]
    Catalogue {
        /// The member of the catalogue that is wrong, as a path into its document; empty for
        /// the whole of it. A refusal places its fault there.
        path: String,
        /// What is wrong with it, naming the entry where there is one.
        reason: String,
    },
}

impl From<sqlx::Error> for SeedError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

/// One capability definition as shipped.
struct CapabilitySeed {
    key: &'static str,
    label: &'static str,
    description: &'static str,
    risk: &'static str,
    /// Each implementation applies the patch under its own key.
    driver_patch: std::collections::BTreeMap<String, Json>,
    decision: &'static str,
    approver: &'static str,
}

/// What ships of one capability's definition, as [`seed`] makes it where it is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShippedCapability {
    /// What it is called.
    pub label: &'static str,
    /// `low`, `medium`, `high` or `critical`.
    pub risk: &'static str,
    /// What a request for it comes to unless a policy says otherwise.
    pub decision: &'static str,
    /// Whose approvers decide it unless a policy says otherwise.
    pub approver_scope: &'static str,
}

/// The shipped definition of the capability `key`, where one ships: what a
/// reader of definitions falls back to before the seed has run (a service that
/// starts before another has seeded, a world made for a test).
#[must_use]
pub fn shipped_capability(key: &str) -> Option<ShippedCapability> {
    capabilities()
        .into_iter()
        .find(|capability| capability.key == key)
        .map(|capability| ShippedCapability {
            label: capability.label,
            risk: capability.risk,
            decision: capability.decision,
            approver_scope: capability.approver,
        })
}

/// The capabilities a template may allow, with what the docker driver merges
/// into a container for each, and what a request for it comes to unless a
/// policy says otherwise. The driver adds the device's group for `kvm` itself.
fn capabilities() -> Vec<CapabilitySeed> {
    vec![
        CapabilitySeed {
            key: "ptrace",
            label: "Debugger access",
            description: "Lets processes in the environment inspect and trace each other, as a debugger does.",
            risk: "medium",
            driver_patch: std::collections::BTreeMap::from([(
                "docker".into(),
                json!({ "cap_add": ["SYS_PTRACE"], "security_opt": ["seccomp=orch-ptrace"] }),
            )]),
            decision: "allow",
            approver: "host",
        },
        CapabilitySeed {
            key: "kvm",
            label: "KVM virtualisation",
            description: "Gives the environment the host's hardware virtualisation device, so that virtual machines can run inside it.",
            risk: "high",
            driver_patch: std::collections::BTreeMap::from([(
                "docker".into(),
                json!({ "devices": [ { "path": "/dev/kvm", "permissions": "rwm" } ] }),
            )]),
            decision: "ask",
            approver: "host",
        },
        CapabilitySeed {
            key: "gpu",
            label: "GPU devices",
            description: "Gives the environment the graphics processors of the host.",
            risk: "medium",
            driver_patch: std::collections::BTreeMap::from([(
                "docker".into(),
                json!({ "device_requests": [ { "count": -1, "capabilities": [["gpu"]] } ] }),
            )]),
            decision: "ask",
            approver: "host",
        },
        CapabilitySeed {
            key: "net_admin",
            label: "Network administration",
            description: "Lets processes in the environment configure the network: interfaces, routes and raw packets.",
            risk: "high",
            driver_patch: std::collections::BTreeMap::from([(
                "docker".into(),
                json!({ "cap_add": ["NET_ADMIN", "NET_RAW"] }),
            )]),
            decision: "ask",
            approver: "host",
        },
        CapabilitySeed {
            key: "unconfined",
            label: "No container sandbox",
            description: "Takes away the container's default filters on system calls and access, so that only the container itself holds what runs there back.",
            risk: "critical",
            driver_patch: std::collections::BTreeMap::from([(
                "docker".into(),
                json!({ "security_opt": ["seccomp=unconfined", "apparmor=unconfined"] }),
            )]),
            decision: "ask",
            approver: "org",
        },
        CapabilitySeed {
            key: "privileged",
            label: "Privileged container",
            description: "Runs the container with every privilege of the host and almost no isolation.",
            risk: "critical",
            driver_patch: std::collections::BTreeMap::from([(
                "docker".into(),
                json!({ "privileged": true }),
            )]),
            decision: "deny",
            approver: "system",
        },
        CapabilitySeed {
            key: "host_network",
            label: "Host network",
            description: "Puts the environment on the network of the host instead of one of its own.",
            risk: "critical",
            driver_patch: std::collections::BTreeMap::from([(
                "docker".into(),
                json!({ "network_mode": "host" }),
            )]),
            decision: "deny",
            approver: "system",
        },
        CapabilitySeed {
            key: "docker_socket",
            label: "Host Docker socket",
            description: "Gives the environment the socket of the host's container engine, with which it can start anything on the host.",
            risk: "critical",
            driver_patch: std::collections::BTreeMap::from([(
                "docker".into(),
                json!({ "binds": ["/var/run/docker.sock:/var/run/docker.sock"] }),
            )]),
            decision: "deny",
            approver: "system",
        },
        CapabilitySeed {
            key: "docker_inside",
            label: "Docker inside",
            description: "Lets the environment run Docker of its own, with its own containers and images, kept in a volume. Offered only where the environment's runtime has a kernel of its own, so that what it does stays inside.",
            risk: "medium",
            driver_patch: std::collections::BTreeMap::from([(
                "docker".into(),
                json!({ "cap_add": ["ALL"], "security_opt": ["seccomp=unconfined", "apparmor=unconfined", "systempaths=unconfined"], "device_cgroup_rules": ["b 7:* rwm", "c 10:237 rwm"] }),
            )]),
            decision: "ask",
            approver: "host",
        },
        CapabilitySeed {
            key: "port_place",
            label: "Ports for a place",
            risk: "medium",
            driver_patch: std::collections::BTreeMap::new(),
            description: "Lets the gateway give viewers of a named place access to an environment port.",
            decision: "ask",
            approver: "environment",
        },
        CapabilitySeed {
            key: "port_host",
            label: "Ports on the host",
            risk: "high",
            driver_patch: std::collections::BTreeMap::new(),
            description: "Lets the gateway publish an environment port to anyone who reaches its host address.",
            decision: "deny",
            approver: "environment",
        },
        CapabilitySeed {
            key: "port_link",
            label: "Ports through a link",
            risk: "high",
            driver_patch: std::collections::BTreeMap::new(),
            description: "Lets the gateway issue a secret link whose holder can reach an environment port.",
            decision: "deny",
            approver: "environment",
        },
    ]
}

/// The spec of the Default template: the unit an environment is made from,
/// and what a harness inside it is told by default. Its confinement is the
/// container, so the agent needs no sandbox of its own; it asks when necessary.
fn default_template_spec(release: &Release<'_>) -> Json {
    use default_template as v;
    // The programs that may run beside the tools there: those the adapters of this build say may, by kind.
    let mut colocated: Vec<&str> = release
        .adapters
        .iter()
        .filter(|adapter| adapter.colocated)
        .map(|adapter| adapter.kind)
        .collect();
    colocated.sort_unstable();
    json!({
        "image_digest": null,
        "build": { "dockerfile": v::DOCKERFILE, "context": null, "args": { "VERSION": release.version }, "target": null },
        "archive": null,
        "runtime": "runc",
        "user": { "name": "agent", "uid": v::AGENT_ID, "gid": v::AGENT_ID },
        "workspace_mount": "/workspace",
        "home_mount": "/home/agent",
        "resources": { "cpu_millis": v::CPU_MILLIS, "memory_mb": v::MEMORY_MB, "pids": v::PIDS, "disk_gb": v::DISK_GB },
        "resources_max": { "cpu_millis": v::CPU_MILLIS_MAX, "memory_mb": v::MEMORY_MB_MAX, "pids": v::PIDS_MAX, "disk_gb": v::DISK_GB_MAX },
        "needs": { "memory_mb": v::NEEDS_MEMORY_MB, "cpu_millis": v::NEEDS_CPU_MILLIS, "disk_gb": v::NEEDS_DISK_GB },
        "network": { "mode": "bridge", "egress": "open", "dns": [] },
        "security": { "read_only_rootfs": false, "no_new_privileges": false, "cap_add": [], "cap_drop": [] },
        "base_capabilities": [],
        "allowed_capabilities": ["ptrace", "kvm", "net_admin", "gpu", "docker_inside", "port_place", "port_host", "port_link"],
        "sandbox": { "default": "open", "max": "open" },
        "agent_defaults": { "approval.mode": "on_request" },
        "env": {},
        "mounts": [],
        "exec": { "offers": orch_agent_programs::Programs::builtin().default_offers_for(colocated.iter().copied()), "orch_tools": true },
        "colocated_harnesses": colocated,
        "mcp_server_ids": [],
        "after_up": [],
        "host_labels": []
    })
}

/// The spec of **Developer tools**: the Default template's recipe on the developer image, with room for builds and
/// for Docker of its own, which an environment is given where its runtime has a kernel of its own.
fn developer_tools_template_spec(release: &Release<'_>) -> Json {
    use developer_tools_template as v;
    let mut spec = default_template_spec(release);
    spec["build"]["dockerfile"] = json!(v::DOCKERFILE);
    spec["resources"] = json!({ "cpu_millis": v::CPU_MILLIS, "memory_mb": v::MEMORY_MB, "pids": v::PIDS, "disk_gb": v::DISK_GB });
    spec["resources_max"] = json!({ "cpu_millis": default_template::CPU_MILLIS_MAX, "memory_mb": default_template::MEMORY_MB_MAX, "pids": v::PIDS_MAX, "disk_gb": v::DISK_GB_MAX });
    spec["needs"] = json!({ "memory_mb": v::NEEDS_MEMORY_MB, "cpu_millis": default_template::NEEDS_CPU_MILLIS, "disk_gb": v::NEEDS_DISK_GB });
    spec
}

/// Reconstructs the image-based seed, so only an exact old definition can be converted.
fn old_environment_template_spec(release: &Release<'_>, image: &str, developer: bool) -> Json {
    let mut spec = if developer {
        developer_tools_template_spec(release)
    } else {
        default_template_spec(release)
    };
    spec["image"] = json!(image);
    spec["build"] = Json::Null;
    spec["image_facts"] = json!({ "image": image, "docker": developer, "qemu": developer });
    spec
}

/// The spec of the Provider runtime template: an image with no shell and
/// nothing to run beside the harness, so that what holds a credential has
/// nothing else in it.
fn provider_runtime_template_spec() -> Json {
    use provider_runtime_template as v;
    json!({
        "build": { "dockerfile": v::DOCKERFILE, "context": null, "args": {}, "target": null },
        "runtime": "runc",
        "user": { "name": "nonroot", "uid": v::NONROOT_ID, "gid": v::NONROOT_ID },
        "resources": { "cpu_millis": v::CPU_MILLIS, "memory_mb": v::MEMORY_MB, "pids": v::PIDS },
        "network": { "mode": "bridge", "egress": "open", "dns": [] },
        "security": { "read_only_rootfs": true, "no_new_privileges": true, "cap_add": [], "cap_drop": ["ALL"] },
        "env": {},
        "mounts": [ { "type": "tmpfs", "target": "/tmp" } ],
        "after_up": [],
        "plugins": [],
        "host_labels": []
    })
}

/// The spec of the template **Local embeddings**: the server's variables are
/// llama.cpp's own (`LLAMA_ARG_*`), so a change of thread count, context or
/// port is a change of the template's values; the command after start starts
/// the server and waits until it answers.
pub fn local_embeddings_template_spec() -> Json {
    use local_embeddings_template as v;
    json!({
        "build": { "dockerfile": v::DOCKERFILE, "context": null, "args": {}, "target": null },
        "runtime": "runc",
        "user": { "name": "nonroot", "uid": v::NONROOT_ID, "gid": v::NONROOT_ID },
        "resources": { "cpu_millis": v::CPU_MILLIS, "memory_mb": v::MEMORY_MB, "pids": v::PIDS },
        "network": { "mode": "bridge", "egress": "open", "dns": [] },
        "security": { "read_only_rootfs": true, "no_new_privileges": true, "cap_add": [], "cap_drop": ["ALL"] },
        "env": {
            "LLAMA_ARG_MODEL": "/opt/models/embeddinggemma-2-Q8_0.gguf",
            "LLAMA_ARG_ALIAS": v::MODEL,
            "LLAMA_ARG_EMBEDDINGS": "true",
            "LLAMA_ARG_POOLING": "mean",
            "LLAMA_ARG_HOST": "127.0.0.1",
            "LLAMA_ARG_PORT": "8090",
            "LLAMA_ARG_CTX_SIZE": "2048",
            "LLAMA_ARG_BATCH": "2048",
            "LLAMA_ARG_UBATCH": "2048",
            "LLAMA_ARG_THREADS": "4",
            "EMBEDDINGS_START_SECS": "120"
        },
        "mounts": [ { "type": "tmpfs", "target": "/tmp" } ],
        "after_up": [ { "name": "embeddings", "command": ["/bin/bash", "/opt/llama/serve"], "timeout_secs": 300 } ],
        "plugins": [],
        "host_labels": []
    })
}

/// The ready-made API endpoints a release offers when a provider is added,
/// by vendor name, in the shape of the setting `providers.presets`: the
/// addresses each vendor documents for the formats Orch relays. A place
/// adds to them, replaces one or hides one with `null`.
#[must_use]
pub fn provider_presets() -> Json {
    // Wire format names are the model contract's protocol identifiers.
    const RESPONSES: &str = "openai-responses";
    const MESSAGES: &str = "anthropic-messages";
    const CHAT: &str = "openai-chat";
    use local_embeddings_template as local;
    json!({
        "Local embeddings": {
            "upstreams": [{"protocol": "openai-embeddings", "base_url": local::ENDPOINT, "options": {"auth": "none"}}],
            "template": {"name": "Local embeddings", "home_id": null, "kind": "provider", "driver": "docker", "spec": local_embeddings_template_spec()},
            "model": {"model_name": local::MODEL, "display_name": "EmbeddingGemma 2"},
            "dimensions": local::DIMENSIONS
        },
        "OpenAI": {
            "upstreams": [ { "protocol": RESPONSES, "base_url": "https://api.openai.com/v1" } ]
        },
        "Anthropic": {
            "upstreams": [ { "protocol": MESSAGES, "base_url": "https://api.anthropic.com" } ]
        },
        "OpenRouter": {
            "upstreams": [ { "protocol": CHAT, "base_url": "https://openrouter.ai/api/v1" } ]
        },
        "DeepSeek": {
            "note": "Serves both formats on the same key.",
            "upstreams": [
                { "protocol": RESPONSES, "base_url": "https://api.deepseek.com" },
                { "protocol": MESSAGES, "base_url": "https://api.deepseek.com/anthropic" }
            ]
        }
    })
}

/// Makes what is missing of the built-in capability definitions, the two
/// templates and one profile for each adapter of the release, each template
/// and profile with its Use row. Run it in a transaction: it takes a lock so
/// that two services that start together seed once.
pub async fn seed(
    conn: &mut Conn,
    release: &Release<'_>,
    at: DateTime<Utc>,
) -> Result<Seeded, SeedError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SEED_LOCK)
        .execute(&mut *conn)
        .await?;
    let mut made = Seeded::default();
    for capability in &capabilities() {
        made.capabilities += seed_capability(conn, capability, at).await?;
    }
    convert_old_environment_templates(conn, release, at).await?;
    let templates = [
        (
            DEFAULT_TEMPLATE_ID,
            "Default",
            "Where environments are made unless a place says otherwise. The container is the confinement; the agent asks when it needs to.",
            "environment",
            default_template_spec(release),
        ),
        (
            PROVIDER_RUNTIME_TEMPLATE_ID,
            "Provider runtime",
            "Where a managed provider's credential and harness are kept. It has no shell and nothing to run beside the harness.",
            "provider",
            provider_runtime_template_spec(),
        ),
        (
            DEVELOPER_TEMPLATE_ID,
            "Developer tools",
            "The Default environment with Docker Engine, QEMU and the usual build tools in its image. Docker inside is given under a runtime that has a kernel of its own.",
            "environment",
            developer_tools_template_spec(release),
        ),
    ];
    for (id, name, description, kind, spec) in &templates {
        if seed_template(conn, *id, name, description, kind, spec, at).await? {
            made.templates += 1;
            use_row(conn, "template", *id, Permission::TemplateUse, at).await?;
            made.use_rows += 1;
        }
    }
    for adapter in release.adapters {
        if let Some(id) = seed_agent(conn, adapter, at).await? {
            made.agents += 1;
            use_row(conn, "agent", id, Permission::AgentUse, at).await?;
            made.use_rows += 1;
        }
    }
    Ok(made)
}

/// Changes only a still-original image seed. The conditional update also protects a concurrent edit.
async fn convert_old_environment_templates(
    conn: &mut Conn,
    release: &Release<'_>,
    at: DateTime<Utc>,
) -> Result<(), SeedError> {
    for (id, name, description, repository, developer) in [
        (
            DEFAULT_TEMPLATE_ID,
            "Default",
            "Where environments are made unless a place says otherwise. The container is the confinement; the agent asks when it needs to.",
            "orch-env-base:",
            false,
        ),
        (
            DEVELOPER_TEMPLATE_ID,
            "Developer tools",
            "The Default environment with Docker Engine, QEMU and the usual build tools in its image. Docker inside is given under a runtime that has a kernel of its own.",
            "orch-env-dev:",
            true,
        ),
    ] {
        let old: Option<Json> = sqlx::query_scalar("SELECT spec FROM env_templates WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
        let Some(old) = old else { continue };
        let Some(image) = old.get("image").and_then(Json::as_str) else {
            continue;
        };
        if !image
            .strip_prefix(repository)
            .is_some_and(|tag| !tag.is_empty())
            || old != old_environment_template_spec(release, image, developer)
        {
            continue;
        }
        let replacement = if developer {
            developer_tools_template_spec(release)
        } else {
            default_template_spec(release)
        };
        sqlx::query(
            "UPDATE env_templates SET spec = $2, revision = revision + 1, updated_at = $3
             WHERE id = $1 AND spec = $4 AND name = $5 AND description = $6
               AND kind = 'environment' AND driver = 'docker'
               AND home_id IS NULL AND org_id IS NULL AND owner_id IS NULL
               AND enabled AND deleted_at IS NULL AND revision = 1 AND updated_at = created_at",
        )
        .bind(id)
        .bind(replacement)
        .bind(at)
        .bind(old)
        .bind(name)
        .bind(description)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn seed_capability(
    conn: &mut Conn,
    capability: &CapabilitySeed,
    at: DateTime<Utc>,
) -> Result<u64, SeedError> {
    let done = sqlx::query(
        "INSERT INTO capability_defs (key, label, description, risk, driver_patch, default_decision,
                                      default_approver_scope, builtin, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, true, $8)
         ON CONFLICT (key) DO NOTHING",
    )
    .bind(capability.key)
    .bind(capability.label)
    .bind(capability.description)
    .bind(capability.risk)
    .bind(json!(capability.driver_patch))
    .bind(capability.decision)
    .bind(capability.approver)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

async fn seed_template(
    conn: &mut Conn,
    id: Id,
    name: &str,
    description: &str,
    kind: &str,
    spec: &Json,
    at: DateTime<Utc>,
) -> Result<bool, SeedError> {
    let done = sqlx::query(
        "INSERT INTO env_templates (id, name, description, kind, driver, spec, created_at, updated_at)
         VALUES ($1, $2, $3, $4, 'docker', $5, $6, $6)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .bind(name)
    .bind(description)
    .bind(kind)
    .bind(spec)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Makes the system-level profile of an adapter unless one exists, even a
/// removed one, and returns its identifier when it made one.
async fn seed_agent(
    conn: &mut Conn,
    adapter: &BuiltInAdapter<'_>,
    at: DateTime<Utc>,
) -> Result<Option<Id>, SeedError> {
    let length = adapter.title.chars().count();
    if length == 0 || length > AGENT_NAME_MAX_CHARS {
        return Err(SeedError::AgentName {
            kind: adapter.kind.to_owned(),
        });
    }
    let id = orch_types::new_id();
    let done = sqlx::query(
        "INSERT INTO agents (id, name, harness_kind, profile, created_at, updated_at)
         SELECT $1, $2, $3, $4, $5, $5
          WHERE NOT EXISTS (SELECT 1 FROM agents WHERE harness_kind = $3 AND org_id IS NULL AND machine_id IS NULL)",
    )
    .bind(id)
    .bind(adapter.title)
    .bind(adapter.kind)
    .bind(json!({ "harness_version": null, "config": {}, "env": {} }))
    .bind(at)
    .execute(conn)
    .await?;
    Ok((done.rows_affected() == 1).then_some(id))
}

/// The Use row of something the system ships: everybody may use it.
async fn use_row(
    conn: &mut Conn,
    object_kind: &str,
    object_id: Id,
    permission: Permission,
    at: DateTime<Utc>,
) -> Result<(), SeedError> {
    let page = PageKey::new(object_kind, object_id, "live");
    let options = json!({});
    access::insert_entry(
        conn,
        &NewEntry {
            id: orch_types::new_id(),
            page: &page,
            profile: "any",
            permission: permission.as_str(),
            holder_kind: "everyone",
            holder_id: None,
            holder_role: None,
            options: &options,
            expires_at: None,
            note: "",
            source: "seed",
            created_by: None,
            at,
        },
    )
    .await?;
    Ok(())
}

/// What loading the catalogue of a release came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogueOutcome {
    /// There was none in force; the release's is now.
    Loaded,
    /// The one in force was itself shipped and older; the release's replaced
    /// it.
    Replaced {
        /// The date of the figures that were replaced.
        older: NaiveDate,
    },
    /// The one in force was shipped and is as new as the release's, or newer.
    Current,
    /// An admin gave the one in force (uploaded it, or had it fetched), and it
    /// stays.
    Left {
        /// Whether the release's figures are newer than the ones in force, so
        /// that the Rates screen says that the release came with a newer one.
        release_is_newer: bool,
    },
}

/// Writes the system's defaults page as it ships: one entry for each row
/// the catalogue ships with a role. It is written on a system that nobody
/// has set up yet (no user exists) and whose defaults page has no entry;
/// from the first user on, the page is its admins' and is never written
/// here again, so a row an admin emptied stays empty, also when they
/// emptied them all. Run it in a transaction, as [`seed`]. Returns how
/// many entries it wrote.
pub async fn system_defaults(conn: &mut Conn, at: DateTime<Utc>) -> Result<u32, SeedError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SEED_LOCK)
        .execute(&mut *conn)
        .await?;
    let page = PageKey::new("system", access::SYSTEM_ID, "defaults");
    let settled: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users)
             OR EXISTS (SELECT 1 FROM permission_entries WHERE object_kind = $1 AND object_id = $2 AND page = $3)",
    )
    .bind("system")
    .bind(access::SYSTEM_ID)
    .bind("defaults")
    .fetch_one(&mut *conn)
    .await?;
    if settled {
        return Ok(0);
    }
    let options = json!({});
    let mut made = 0;
    for shipped in orch_access_contract::pages::shipped() {
        access::insert_entry(
            conn,
            &NewEntry {
                id: orch_types::new_id(),
                page: &page,
                profile: shipped.profile.as_str(),
                permission: shipped.permission.as_str(),
                holder_kind: "role",
                holder_id: None,
                holder_role: Some(shipped.role.as_str()),
                options: &options,
                expires_at: None,
                note: "",
                source: "seed",
                created_by: None,
                at,
            },
        )
        .await?;
        made += 1;
    }
    Ok(made)
}

/// Loads the rate catalogue a release comes with, `text` being the file
/// `model-rates.json`. Nothing is in force: it is loaded as shipped. A shipped
/// catalogue is in force and older by its date: the release's replaces it.
/// One an admin gave is left. The file is checked whole before anything is
/// written; a file that fails the check is an error and changes nothing. Run
/// it in a transaction, as [`seed`], so that services that start together load
/// it once.
pub async fn load_shipped_catalogue(
    conn: &mut Conn,
    text: &str,
    at: DateTime<Utc>,
) -> Result<CatalogueOutcome, SeedError> {
    let (doc, as_of, families) = read_catalogue(text)?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SEED_LOCK)
        .execute(&mut *conn)
        .await?;
    let in_force: Option<(String, NaiveDate)> =
        sqlx::query_as("SELECT origin, as_of FROM rate_catalogue WHERE id")
            .fetch_optional(&mut *conn)
            .await?;
    let outcome = match in_force {
        None => {
            insert_shipped(conn, &doc, as_of, at).await?;
            CatalogueOutcome::Loaded
        }
        Some((origin, older)) if origin == "shipped" => {
            if older < as_of {
                replace_shipped(conn, &doc, as_of, at).await?;
                CatalogueOutcome::Replaced { older }
            } else {
                CatalogueOutcome::Current
            }
        }
        Some((_, in_force)) => {
            return Ok(CatalogueOutcome::Left {
                release_is_newer: as_of > in_force,
            });
        }
    };
    let replaced = match outcome {
        CatalogueOutcome::Replaced { older } => Some(older.to_string()),
        _ => None,
    };
    if matches!(
        outcome,
        CatalogueOutcome::Loaded | CatalogueOutcome::Replaced { .. }
    ) {
        let action = audit::action!(AuditAction::RateCatalogueLoaded);
        let summary =
            format!("The rate catalogue of the release was loaded, with figures of {as_of}");
        let mut entry = audit::Entry::new(
            Actor::System,
            action.as_str(),
            action.object_kind().unwrap_or("rate_catalogue"),
            None,
            &summary,
        );
        entry.details = json!({
            "origin": "shipped",
            "as_of": as_of.to_string(),
            "families": families,
            "replaced_as_of": replaced,
        });
        audit::append(conn, &entry, &Origin::default(), at).await?;
    }
    Ok(outcome)
}

/// How a catalogue that no release brought came into force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken<'a> {
    /// An admin gave it as a file, in the browser or on the host.
    Uploaded,
    /// The system fetched it from the address an admin set.
    Fetched {
        /// The address.
        from: &'a str,
    },
}

/// What loading a catalogue came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    /// The date of its figures.
    pub as_of: NaiveDate,
    /// How many families it holds.
    pub families: usize,
    /// The date of the figures of the catalogue it replaced, where one was in
    /// force.
    pub replaced: Option<NaiveDate>,
}

/// A catalogue that has been read and is to be put in force.
struct Replacement<'a> {
    doc: Json,
    as_of: NaiveDate,
    families: usize,
    /// `shipped`, `uploaded` or `fetched`.
    origin: &'static str,
    source: Option<&'a str>,
    /// What the line of the audit trail says it came from.
    from: &'static str,
}

/// Puts a catalogue in force that was given and not shipped: a file an admin
/// uploaded or gave on the host, or one the system fetched from the address an
/// admin set. `text` is checked whole before anything is written, as an upload
/// is: its format, its unit, its date and every family, with its pattern and
/// its rate. A file that fails the check is an error that names the entry, and
/// changes nothing. Otherwise it replaces the catalogue in force at once,
/// whatever that was and whatever its date, and the audit trail names `by`.
/// Run it in the transaction of the change.
pub async fn load_catalogue(
    conn: &mut Conn,
    text: &str,
    taken: Taken<'_>,
    by: Actor,
    origin: &Origin,
    at: DateTime<Utc>,
) -> Result<Loaded, SeedError> {
    let (doc, as_of, families) = read_catalogue(text)?;
    let (kind, source, from) = match taken {
        Taken::Uploaded => ("uploaded", None, "a file that was given"),
        Taken::Fetched { from } => ("fetched", Some(from), "the address it is fetched from"),
    };
    replace_catalogue(
        conn,
        &Replacement {
            doc,
            as_of,
            families,
            origin: kind,
            source,
            from,
        },
        by,
        origin,
        at,
    )
    .await
}

/// Puts back the catalogue the running release came with: `text` is its file,
/// `model-rates.json`. It replaces whatever is in force, one an admin gave and
/// one that is newer included, and counts as shipped again, so that a later
/// release may replace it. It is checked whole first, as the one a start loads.
pub async fn put_back_shipped_catalogue(
    conn: &mut Conn,
    text: &str,
    by: Actor,
    origin: &Origin,
    at: DateTime<Utc>,
) -> Result<Loaded, SeedError> {
    let (doc, as_of, families) = read_catalogue(text)?;
    replace_catalogue(
        conn,
        &Replacement {
            doc,
            as_of,
            families,
            origin: "shipped",
            source: None,
            from: "the release",
        },
        by,
        origin,
        at,
    )
    .await
}

/// Writes the catalogue in force and the line of the audit trail, one after
/// the other, under the lock that serves a start.
async fn replace_catalogue(
    conn: &mut Conn,
    new: &Replacement<'_>,
    by: Actor,
    origin: &Origin,
    at: DateTime<Utc>,
) -> Result<Loaded, SeedError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SEED_LOCK)
        .execute(&mut *conn)
        .await?;
    let replaced: Option<NaiveDate> =
        sqlx::query_scalar("SELECT as_of FROM rate_catalogue WHERE id")
            .fetch_optional(&mut *conn)
            .await?;
    let loaded_by = match by {
        Actor::User(user) => Some(user),
        Actor::System | Actor::Agent(_) => None,
    };
    sqlx::query(
        "INSERT INTO rate_catalogue (id, doc, as_of, origin, source, loaded_at, loaded_by)
         VALUES (true, $1, $2, $3, $4, $5, $6)
         ON CONFLICT (id) DO UPDATE
            SET doc = EXCLUDED.doc, as_of = EXCLUDED.as_of, origin = EXCLUDED.origin,
                source = EXCLUDED.source, loaded_at = EXCLUDED.loaded_at, loaded_by = EXCLUDED.loaded_by",
    )
    .bind(&new.doc)
    .bind(new.as_of)
    .bind(new.origin)
    .bind(new.source)
    .bind(at)
    .bind(loaded_by)
    .execute(&mut *conn)
    .await?;
    let action = AuditAction::RateCatalogueLoaded;
    let summary = format!(
        "The rate catalogue was loaded from {}, with figures of {}",
        new.from, new.as_of
    );
    let mut entry = audit::Entry::new(
        by,
        action.as_str(),
        action.object_kind().unwrap_or("rate_catalogue"),
        None,
        &summary,
    );
    entry.history = action.is_history_line();
    entry.details = json!({
        "origin": new.origin,
        "as_of": new.as_of.to_string(),
        "families": new.families,
        "replaced_as_of": replaced.map(|day| day.to_string()),
        "source": new.source,
    });
    audit::append(conn, &entry, origin, at).await?;
    Ok(Loaded {
        as_of: new.as_of,
        families: new.families,
        replaced,
    })
}

async fn insert_shipped(
    conn: &mut Conn,
    doc: &Json,
    as_of: NaiveDate,
    at: DateTime<Utc>,
) -> Result<(), SeedError> {
    sqlx::query(
        "INSERT INTO rate_catalogue (id, doc, as_of, origin, source, loaded_at, loaded_by)
         VALUES (true, $1, $2, 'shipped', NULL, $3, NULL)",
    )
    .bind(doc)
    .bind(as_of)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

async fn replace_shipped(
    conn: &mut Conn,
    doc: &Json,
    as_of: NaiveDate,
    at: DateTime<Utc>,
) -> Result<(), SeedError> {
    sqlx::query(
        "UPDATE rate_catalogue SET doc = $1, as_of = $2, origin = 'shipped', source = NULL, loaded_at = $3, loaded_by = NULL
          WHERE id",
    )
    .bind(doc)
    .bind(as_of)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Reads and checks the file of a catalogue: its format and unit, its date,
/// and every family, which names the page its figures were read from and the
/// day, and whose rate fits the shape of a rate of the rate card. Returns the
/// document, its date and how many families it has.
fn read_catalogue(text: &str) -> Result<(Json, NaiveDate, usize), SeedError> {
    let bad = |path: &str, reason: String| SeedError::Catalogue {
        path: path.to_owned(),
        reason,
    };
    let doc: Json = serde_json::from_str(text).map_err(|error| bad("", error.to_string()))?;
    if doc.get("format").and_then(Json::as_i64) != Some(CATALOGUE_FORMAT) {
        return Err(bad(
            "format",
            format!("its format is not {CATALOGUE_FORMAT}"),
        ));
    }
    if doc.get("unit").and_then(Json::as_str) != Some(CATALOGUE_UNIT) {
        return Err(bad("unit", format!("its unit is not {CATALOGUE_UNIT}")));
    }
    let day = |value: Option<&Json>| {
        value
            .and_then(Json::as_str)
            .and_then(|text| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok())
    };
    let as_of =
        day(doc.get("as_of")).ok_or_else(|| bad("as_of", "as_of is not a day".to_owned()))?;
    let families = doc
        .get("families")
        .and_then(Json::as_array)
        .ok_or_else(|| bad("families", "families is not a list".to_owned()))?;
    let Some(rates) = registry::get(keys::USAGE_RATES) else {
        return Err(bad("", "the rate card is not in the registry".to_owned()));
    };
    for (index, family) in families.iter().enumerate() {
        let entry = format!("families[{index}]");
        let pattern = family
            .get("match")
            .and_then(Json::as_str)
            .filter(|pattern| !pattern.is_empty())
            .ok_or_else(|| bad(&format!("{entry}.match"), format!("{entry} names no model")))?;
        let names_its_page = family
            .get("source")
            .and_then(Json::as_str)
            .is_some_and(|source| !source.is_empty());
        if !names_its_page || day(family.get("as_of")).is_none() {
            return Err(bad(
                &entry,
                format!("{pattern} names no page its figures were read from, or no day"),
            ));
        }
        let rate = family.get("rate").cloned().unwrap_or(Json::Null);
        if let Some(fault) = rates.faults(&json!({ pattern: rate })).first() {
            // The rate card's path starts with the pattern it was checked under; the entry is
            // placed by where it stands in the file, and named by its pattern.
            let member = fault.path.strip_prefix(pattern).unwrap_or_default();
            return Err(bad(
                &format!("{entry}.rate{member}"),
                format!("{pattern}: {}", fault.message),
            ));
        }
    }
    let count = families.len();
    Ok((doc, as_of, count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;

    const ADAPTERS: &[BuiltInAdapter<'static>] = &[
        BuiltInAdapter {
            kind: "codex",
            title: "Codex",
            colocated: true,
        },
        BuiltInAdapter {
            kind: "claude-code",
            title: "Claude Code",
            colocated: true,
        },
        BuiltInAdapter {
            kind: "fake",
            title: "Fake",
            colocated: false,
        },
    ];

    fn release() -> Release<'static> {
        Release {
            version: "0.1.0",
            adapters: ADAPTERS,
        }
    }

    /// The image-based template stored by the previous release, independent of today's seed code.
    fn legacy_environment_template(developer: bool) -> Json {
        let mut spec: Json = serde_json::from_str(include_str!(
            "../tests/fixtures/legacy-environment-template.json"
        ))
        .unwrap();
        if developer {
            spec["image"] = json!("orch-env-dev:0.1.0");
            spec["image_facts"] =
                json!({"image": "orch-env-dev:0.1.0", "docker": true, "qemu": true});
            spec["resources"] =
                json!({"cpu_millis": 4000, "memory_mb": 8192, "pids": 8192, "disk_gb": 100});
            spec["resources_max"] =
                json!({"cpu_millis": 16000, "memory_mb": 65536, "pids": 65536, "disk_gb": 1000});
            spec["needs"] = json!({"memory_mb": 4096, "cpu_millis": 1000, "disk_gb": 20});
        }
        spec
    }

    async fn count(conn: &mut Conn, table: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(conn)
            .await
            .unwrap()
    }

    fn catalogue(as_of: &str, input: &str) -> String {
        json!({
            "format": 1, "as_of": as_of, "unit": "USD",
            "families": [ {
                "match": "gpt-x*", "vendor": "Example", "source": "https://example.test/pricing", "as_of": as_of,
                "rate": { "input_per_mtok": input, "cached_input_per_mtok": "0.1", "output_per_mtok": "10" }
            } ]
        })
        .to_string()
    }

    #[tokio::test]
    async fn a_fresh_system_gets_its_capabilities_its_templates_and_a_profile_for_each_adapter() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let made = seed(&mut conn, &release(), clock::start()).await.unwrap();
        assert_eq!(
            made,
            Seeded {
                capabilities: 12,
                templates: 3,
                agents: 3,
                use_rows: 6
            }
        );
        let kinds: Vec<String> =
            sqlx::query_scalar("SELECT harness_kind FROM agents ORDER BY harness_kind")
                .fetch_all(&mut *conn)
                .await
                .unwrap();
        assert_eq!(kinds, ["claude-code", "codex", "fake"]);
        let asks: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT key, default_decision, default_approver_scope FROM capability_defs WHERE default_decision <> 'allow' ORDER BY key",
        )
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert!(asks.contains(&("privileged".into(), "deny".into(), "system".into())));
        assert!(asks.contains(&("unconfined".into(), "ask".into(), "org".into())));
    }

    #[tokio::test]
    async fn both_environment_templates_build_their_release_recipes_on_the_host() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        seed(&mut conn, &release(), clock::start()).await.unwrap();
        let spec: Json = sqlx::query_scalar("SELECT spec FROM env_templates WHERE id = $1")
            .bind(DEVELOPER_TEMPLATE_ID)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert!(spec["image"].is_null());
        assert_eq!(
            spec["build"]["dockerfile"],
            developer_tools_template::DOCKERFILE
        );
        assert!(spec["build"]["context"].is_null());
        assert_eq!(spec["build"]["args"]["VERSION"], "0.1.0");
        assert_eq!(
            spec["runtime"], "runc",
            "the runtime is chosen when an environment is made"
        );
        let allowed = spec["allowed_capabilities"].as_array().unwrap();
        assert!(allowed.contains(&json!("docker_inside")) && allowed.contains(&json!("kvm")));
        let default_spec: Json = sqlx::query_scalar("SELECT spec FROM env_templates WHERE id = $1")
            .bind(DEFAULT_TEMPLATE_ID)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert!(default_spec["image"].is_null());
        assert_eq!(
            default_spec["build"]["dockerfile"],
            default_template::DOCKERFILE
        );
        assert!(default_spec["build"]["context"].is_null());
        assert_eq!(default_spec["build"]["args"]["VERSION"], "0.1.0");
        for template in [&spec, &default_spec] {
            let source = orch_driver_contract::sources::Source::from_spec(template).unwrap();
            assert_eq!(
                source.kind(),
                orch_driver_contract::sources::SourceKind::Build
            );
        }
    }

    #[tokio::test]
    async fn docker_inside_is_a_moderate_capability_that_asks_the_hosts_managers() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        seed(&mut conn, &release(), clock::start()).await.unwrap();
        let (risk, decision, approver, patch): (String, String, String, Json) = sqlx::query_as(
            "SELECT risk, default_decision, default_approver_scope, driver_patch -> 'docker' FROM capability_defs WHERE key = 'docker_inside'",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(
            (risk.as_str(), decision.as_str(), approver.as_str()),
            ("medium", "ask", "host")
        );
        assert_eq!(patch["cap_add"], json!(["ALL"]));
        for option in [
            "seccomp=unconfined",
            "apparmor=unconfined",
            "systempaths=unconfined",
        ] {
            assert!(
                patch["security_opt"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(option)),
                "{option}"
            );
        }
    }

    #[tokio::test]
    async fn seeding_twice_changes_nothing_the_second_time() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        seed(&mut conn, &release(), clock::start()).await.unwrap();
        let before = [
            count(&mut conn, "capability_defs").await,
            count(&mut conn, "env_templates").await,
            count(&mut conn, "agents").await,
            count(&mut conn, "permission_entries").await,
        ];
        let again = seed(
            &mut conn,
            &release(),
            clock::start() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
        assert!(again.nothing(), "{again:?}");
        let after = [
            count(&mut conn, "capability_defs").await,
            count(&mut conn, "env_templates").await,
            count(&mut conn, "agents").await,
            count(&mut conn, "permission_entries").await,
        ];
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn untouched_image_templates_are_converted_once_and_keep_their_use_rows() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = clock::start();
        seed(&mut conn, &release(), at).await.unwrap();
        for (id, developer) in [(DEFAULT_TEMPLATE_ID, false), (DEVELOPER_TEMPLATE_ID, true)] {
            let old = legacy_environment_template(developer);
            sqlx::query("UPDATE env_templates SET spec = $2 WHERE id = $1")
                .bind(id)
                .bind(old)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        let later = at + chrono::Duration::days(1);
        let next = Release {
            version: "0.1.1",
            ..release()
        };
        seed(&mut conn, &next, later).await.unwrap();
        for (id, expected) in [
            (DEFAULT_TEMPLATE_ID, default_template_spec(&next)),
            (DEVELOPER_TEMPLATE_ID, developer_tools_template_spec(&next)),
        ] {
            let (spec, revision, updated): (Json, i64, DateTime<Utc>) = sqlx::query_as(
                "SELECT spec, revision, updated_at FROM env_templates WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            assert_eq!(spec, expected);
            assert_eq!(revision, 2);
            assert_eq!(updated, later);
        }
        assert_eq!(count(&mut conn, "permission_entries").await, 6);
        seed(&mut conn, &next, later + chrono::Duration::days(1))
            .await
            .unwrap();
        let revisions: Vec<i64> = sqlx::query_scalar(
            "SELECT revision FROM env_templates WHERE id = ANY($1) ORDER BY revision",
        )
        .bind(&[DEFAULT_TEMPLATE_ID, DEVELOPER_TEMPLATE_ID][..])
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(revisions, [2, 2]);
    }

    #[tokio::test]
    async fn edited_disabled_and_removed_image_templates_are_not_converted() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = clock::start();
        seed(&mut conn, &release(), at).await.unwrap();
        let original = legacy_environment_template(false);
        let mut changed = original.clone();
        changed["resources"]["memory_mb"] = json!(4096);
        for (spec, name, revision, enabled, deleted) in [
            (changed, "Default", 1_i64, true, None),
            (original.clone(), "Renamed", 1, true, None),
            (original.clone(), "Default", 2, true, None),
            (original.clone(), "Default", 1, false, None),
            (original.clone(), "Default", 1, true, Some(at)),
        ] {
            sqlx::query("UPDATE env_templates SET spec = $2, name = $3, revision = $4, enabled = $5, deleted_at = $6 WHERE id = $1")
                .bind(DEFAULT_TEMPLATE_ID).bind(&spec).bind(name).bind(revision).bind(enabled).bind(deleted)
                .execute(&mut *conn).await.unwrap();
            seed(&mut conn, &release(), at + chrono::Duration::days(1))
                .await
                .unwrap();
            let saved: Json = sqlx::query_scalar("SELECT spec FROM env_templates WHERE id = $1")
                .bind(DEFAULT_TEMPLATE_ID)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            assert_eq!(
                saved, spec,
                "edited, disabled and removed templates are left alone"
            );
        }
    }

    #[tokio::test]
    async fn gateway_port_capabilities_ship_without_driver_patches_and_environment_templates_allow_them()
     {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        seed(&mut conn, &release(), clock::start()).await.unwrap();
        let definitions:Vec<(String,String,Json,String,String)>=sqlx::query_as("SELECT key,risk,driver_patch,default_decision,default_approver_scope FROM capability_defs WHERE key IN ('port_place','port_host','port_link') ORDER BY key").fetch_all(&mut *conn).await.unwrap();
        assert_eq!(definitions.len(), 3);
        for (key, risk, patch, decision, scope) in definitions {
            assert_eq!(patch, json!({}));
            assert_eq!(scope, "environment");
            assert_eq!(
                risk,
                if key == "port_place" {
                    "medium"
                } else {
                    "high"
                }
            );
            assert_eq!(decision, if key == "port_place" { "ask" } else { "deny" });
        }
        let templates: Vec<Json> =
            sqlx::query_scalar("SELECT spec FROM env_templates WHERE id = ANY($1)")
                .bind(&[DEFAULT_TEMPLATE_ID, DEVELOPER_TEMPLATE_ID][..])
                .fetch_all(&mut *conn)
                .await
                .unwrap();
        assert_eq!(templates.len(), 2, "both environment templates are present");
        for template in templates {
            for key in ["port_place", "port_host", "port_link"] {
                assert!(
                    template["allowed_capabilities"]
                        .as_array()
                        .unwrap()
                        .contains(&json!(key))
                );
            }
        }
    }

    #[tokio::test]
    async fn what_an_admin_edited_is_not_overwritten_and_what_they_narrowed_stays_narrow() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        seed(&mut conn, &release(), clock::start()).await.unwrap();
        sqlx::query("UPDATE capability_defs SET default_decision = 'deny' WHERE key = 'gpu'")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE env_templates SET spec = jsonb_set(spec, '{resources,memory_mb}', '4096') WHERE id = $1")
            .bind(DEFAULT_TEMPLATE_ID)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE permission_entries SET removed_at = now() WHERE object_kind = 'template'",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("UPDATE agents SET deleted_at = now() WHERE harness_kind = 'fake'")
            .execute(&mut *conn)
            .await
            .unwrap();
        let again = seed(&mut conn, &release(), clock::start()).await.unwrap();
        assert!(again.nothing(), "{again:?}");
        let decision: String =
            sqlx::query_scalar("SELECT default_decision FROM capability_defs WHERE key = 'gpu'")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(decision, "deny");
        let memory: i64 = sqlx::query_scalar(
            "SELECT (spec->'resources'->>'memory_mb')::bigint FROM env_templates WHERE id = $1",
        )
        .bind(DEFAULT_TEMPLATE_ID)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(memory, 4096);
        let live_use_rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM permission_entries WHERE object_kind = 'template' AND removed_at IS NULL",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(live_use_rows, 0, "a row an admin removed is not made again");
        assert_eq!(
            count(&mut conn, "agents").await,
            3,
            "a removed profile is not made again"
        );
    }

    #[tokio::test]
    async fn a_template_and_a_profile_are_made_with_a_use_row_that_names_everybody() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        seed(&mut conn, &release(), clock::start()).await.unwrap();
        let rows: Vec<(String, String, String, String, Option<Id>)> = sqlx::query_as(
            "SELECT object_kind, permission, holder_kind, source, created_by FROM permission_entries
              ORDER BY object_kind, object_id",
        )
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(rows.len(), 6);
        assert!(
            rows.iter()
                .all(|(_, _, holder, source, by)| holder == "everyone"
                    && source == "seed"
                    && by.is_none())
        );
        assert_eq!(
            rows.iter()
                .filter(|(kind, permission, ..)| kind == "template" && permission == "template.use")
                .count(),
            3
        );
        assert_eq!(
            rows.iter()
                .filter(|(kind, permission, ..)| kind == "agent" && permission == "agent.use")
                .count(),
            3
        );
    }

    #[test]
    fn the_seeded_templates_keep_their_own_rules() {
        let spec = default_template_spec(&release());
        for member in ["cpu_millis", "memory_mb", "pids", "disk_gb"] {
            let (now, most) = (
                spec["resources"][member].as_i64(),
                spec["resources_max"][member].as_i64(),
            );
            assert!(
                now <= most && now.is_some(),
                "{member}: a unit may not start above its maximum"
            );
        }
        assert!(spec["image"].is_null());
        assert_eq!(spec["build"]["args"]["VERSION"], "0.1.0");
        assert_eq!(spec["agent_defaults"]["approval.mode"], "on_request");
        let provider = provider_runtime_template_spec();
        assert_eq!(
            provider["security"]["read_only_rootfs"], true,
            "what holds a credential is read-only"
        );
        assert_eq!(provider["user"]["name"], "nonroot");
        for template in [provider, local_embeddings_template_spec()] {
            assert!(template.get("allowed_capabilities").is_none());
            assert_eq!(template["security"]["cap_drop"], json!(["ALL"]));
            assert_eq!(template["security"]["no_new_privileges"], true);
        }
    }

    #[test]
    fn a_program_that_is_only_a_manifest_is_let_run_beside_the_tools_of_a_fresh_template() {
        let programs = orch_agent_programs::Programs::builtin()
            .with_program(orch_testkit::programs::third_manifest())
            .unwrap();
        let adapters: Vec<BuiltInAdapter<'_>> = programs
            .programs()
            .map(|manifest| BuiltInAdapter {
                kind: manifest.kind.as_str(),
                title: manifest.name(),
                colocated: manifest.placement.colocated,
            })
            .collect();
        let spec = default_template_spec(&Release {
            version: "0.1.0",
            adapters: &adapters,
        });
        let mut expected: Vec<&str> = programs.colocated_by_default();
        expected.sort_unstable();
        assert!(expected.contains(&orch_testkit::programs::THIRD_KIND));
        assert!(!expected.contains(&"fake"));
        assert_eq!(
            spec["colocated_harnesses"],
            json!(expected),
            "the programs whose manifests say so are let run there, the third one among them and the scripted one not"
        );
        let offers = spec["exec"]["offers"].as_array().unwrap();
        assert!(offers.contains(&json!("codex-exec-server")));
        assert!(offers.contains(&json!("orch-exec:plain")));
        assert!(!offers.contains(&json!("orch-exec:custom")));
    }

    #[test]
    fn the_shipped_presets_fit_the_setting_they_are_the_default_of() {
        let presets = provider_presets();
        assert!(
            registry::validate_at(keys::PROVIDERS_PRESETS, registry::Scope::System, &presets)
                .is_ok()
        );
        let mut names: Vec<_> = presets
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "Anthropic",
                "DeepSeek",
                "Local embeddings",
                "OpenAI",
                "OpenRouter"
            ]
        );
        assert_eq!(
            presets["OpenRouter"]["upstreams"][0]["protocol"],
            "openai-chat"
        );
        assert_eq!(
            presets["OpenRouter"]["upstreams"][0]["base_url"],
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(
            presets["DeepSeek"]["upstreams"].as_array().unwrap().len(),
            2,
            "a vendor that serves both formats lists both"
        );
    }

    #[test]
    fn the_catalogue_that_ships_with_the_repository_passes_the_check_of_an_upload() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../deploy/model-rates.json"
        ))
        .unwrap();
        let (_, as_of, families) = read_catalogue(&text).unwrap();
        assert!(
            families > 20,
            "a catalogue covers the families of the world's vendors"
        );
        assert!(as_of.to_string().starts_with("20"));
    }

    #[test]
    fn a_catalogue_that_names_no_source_or_has_a_broken_rate_is_refused_with_the_entry() {
        let mut broken: Json = serde_json::from_str(&catalogue("2026-10-10", "1")).unwrap();
        broken["families"][0]["rate"]["output_per_mtok"] = json!(15);
        let error = read_catalogue(&broken.to_string()).unwrap_err().to_string();
        assert!(
            error.contains("gpt-x*") && error.contains("output_per_mtok"),
            "{error}"
        );
        let mut unsourced: Json = serde_json::from_str(&catalogue("2026-10-10", "1")).unwrap();
        unsourced["families"][0]["source"] = json!("");
        assert!(read_catalogue(&unsourced.to_string()).is_err());
        assert!(
            read_catalogue(&catalogue("2026-10-10", "1").replace("\"format\":1", "\"format\":2"))
                .is_err()
        );
        assert!(read_catalogue("{").is_err());
    }

    #[tokio::test]
    async fn a_later_releases_catalogue_replaces_a_shipped_one_and_leaves_one_an_admin_gave() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = clock::start();
        let first = load_shipped_catalogue(&mut conn, &catalogue("2026-10-10", "1"), at)
            .await
            .unwrap();
        assert_eq!(first, CatalogueOutcome::Loaded);
        let again = load_shipped_catalogue(&mut conn, &catalogue("2026-10-10", "1"), at)
            .await
            .unwrap();
        assert_eq!(
            again,
            CatalogueOutcome::Current,
            "the same figures change nothing"
        );
        let older = load_shipped_catalogue(&mut conn, &catalogue("2026-09-01", "9"), at)
            .await
            .unwrap();
        assert_eq!(
            older,
            CatalogueOutcome::Current,
            "an older release does not put back older figures"
        );
        let newer = load_shipped_catalogue(&mut conn, &catalogue("2026-11-01", "2"), at)
            .await
            .unwrap();
        assert_eq!(
            newer,
            CatalogueOutcome::Replaced {
                older: NaiveDate::from_ymd_opt(2026, 10, 10).unwrap()
            }
        );
        let (origin, as_of, input): (String, NaiveDate, String) = sqlx::query_as(
            "SELECT origin, as_of, doc->'families'->0->'rate'->>'input_per_mtok' FROM rate_catalogue",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(
            (origin.as_str(), as_of.to_string().as_str(), input.as_str()),
            ("shipped", "2026-11-01", "2")
        );

        let admin = new_id();
        sqlx::query("INSERT INTO users (id, username, display_name, avatar_color) VALUES ($1, 'admin', 'Admin', 1)")
            .bind(admin)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE rate_catalogue SET origin = 'uploaded', loaded_by = $1, doc = jsonb_set(doc, '{families,0,rate,input_per_mtok}', '\"7\"')")
            .bind(admin)
            .execute(&mut *conn)
            .await
            .unwrap();
        let left = load_shipped_catalogue(&mut conn, &catalogue("2026-12-01", "3"), at)
            .await
            .unwrap();
        assert_eq!(
            left,
            CatalogueOutcome::Left {
                release_is_newer: true
            }
        );
        let kept: String = sqlx::query_scalar(
            "SELECT doc->'families'->0->'rate'->>'input_per_mtok' FROM rate_catalogue",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(kept, "7", "what an admin gave stays");
        let left = load_shipped_catalogue(&mut conn, &catalogue("2026-01-01", "3"), at)
            .await
            .unwrap();
        assert_eq!(
            left,
            CatalogueOutcome::Left {
                release_is_newer: false
            }
        );
    }

    #[tokio::test]
    async fn loading_a_catalogue_is_audited_as_done_by_the_system_and_a_refused_one_writes_nothing()
    {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        load_shipped_catalogue(&mut conn, &catalogue("2026-10-10", "1"), clock::start())
            .await
            .unwrap();
        let rows: Vec<(String, String, bool)> =
            sqlx::query_as("SELECT action, actor_kind, history FROM audit_log")
                .fetch_all(&mut *conn)
                .await
                .unwrap();
        assert_eq!(
            rows,
            [(
                "rate_catalogue.loaded".to_owned(),
                "system".to_owned(),
                false
            )]
        );
        assert!(
            load_shipped_catalogue(&mut conn, "{\"format\":1}", clock::start())
                .await
                .is_err()
        );
        assert_eq!(count(&mut conn, "audit_log").await, 1);
        load_shipped_catalogue(&mut conn, &catalogue("2026-10-10", "1"), clock::start())
            .await
            .unwrap();
        assert_eq!(
            count(&mut conn, "audit_log").await,
            1,
            "a load that changed nothing leaves no row"
        );
    }
    /// Two families, the second with a rate that is broken.
    fn catalogue_with_a_broken_second_entry(as_of: &str) -> String {
        let mut doc: Json = serde_json::from_str(&catalogue(as_of, "1")).unwrap();
        let second = json!({
            "match": "other-model*", "vendor": "Example", "source": "https://example.test/other", "as_of": as_of,
            "rate": { "input_per_mtok": "two", "cached_input_per_mtok": "0.1", "output_per_mtok": "10" }
        });
        doc["families"].as_array_mut().unwrap().push(second);
        doc.to_string()
    }

    async fn in_force(conn: &mut Conn) -> (String, String, Option<Id>, Option<String>) {
        sqlx::query_as(
            "SELECT origin, as_of::text, loaded_by, doc->'families'->0->'rate'->>'input_per_mtok' FROM rate_catalogue",
        )
        .fetch_one(conn)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_catalogue_that_was_given_replaces_the_one_in_force_at_once_whatever_its_date() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = clock::start();
        load_shipped_catalogue(&mut conn, &catalogue("2026-10-10", "1"), at)
            .await
            .unwrap();
        let admin = new_id();
        sqlx::query("INSERT INTO users (id, username, display_name, avatar_color) VALUES ($1, 'boss', 'Boss', 1)")
            .bind(admin)
            .execute(&mut *conn)
            .await
            .unwrap();

        let loaded = load_catalogue(
            &mut conn,
            &catalogue("2026-01-01", "2"),
            Taken::Uploaded,
            Actor::User(admin),
            &Origin::default(),
            at,
        )
        .await
        .unwrap();

        assert_eq!(loaded.families, 1);
        assert_eq!(loaded.replaced, NaiveDate::from_ymd_opt(2026, 10, 10));
        assert_eq!(
            in_force(&mut conn).await,
            (
                "uploaded".to_owned(),
                "2026-01-01".to_owned(),
                Some(admin),
                Some("2".to_owned())
            ),
            "an older file counts: an admin's choice is not undone by a date"
        );
        // From the host nobody signed in loaded it.
        load_catalogue(
            &mut conn,
            &catalogue("2026-02-02", "3"),
            Taken::Fetched {
                from: "https://rates.example/model-rates.json",
            },
            Actor::System,
            &Origin::default(),
            at,
        )
        .await
        .unwrap();
        let (origin, _, by, input) = in_force(&mut conn).await;
        assert_eq!(
            (origin.as_str(), by, input.as_deref()),
            ("fetched", None, Some("3"))
        );
        let source: Option<String> = sqlx::query_scalar("SELECT source FROM rate_catalogue")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            source.as_deref(),
            Some("https://rates.example/model-rates.json")
        );
    }

    #[tokio::test]
    async fn a_catalogue_with_one_bad_entry_changes_nothing_and_names_the_entry() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = clock::start();
        load_shipped_catalogue(&mut conn, &catalogue("2026-10-10", "1"), at)
            .await
            .unwrap();
        let before = in_force(&mut conn).await;
        let audited = count(&mut conn, "audit_log").await;

        let refused = load_catalogue(
            &mut conn,
            &catalogue_with_a_broken_second_entry("2026-11-11"),
            Taken::Uploaded,
            Actor::System,
            &Origin::default(),
            at,
        )
        .await
        .unwrap_err();

        assert!(refused.to_string().contains("other-model*"), "{refused}");
        assert_eq!(in_force(&mut conn).await, before);
        assert_eq!(count(&mut conn, "audit_log").await, audited);
        for broken in ["{", "{\"format\":2}", "[]"] {
            assert!(
                load_catalogue(
                    &mut conn,
                    broken,
                    Taken::Uploaded,
                    Actor::System,
                    &Origin::default(),
                    at
                )
                .await
                .is_err(),
                "{broken}"
            );
        }
        assert_eq!(in_force(&mut conn).await, before);
    }

    #[tokio::test]
    async fn the_catalogue_of_the_release_is_put_back_over_one_an_admin_gave_and_over_a_newer_one()
    {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = clock::start();
        let shipped = catalogue("2026-10-10", "1");
        load_shipped_catalogue(&mut conn, &shipped, at)
            .await
            .unwrap();
        load_catalogue(
            &mut conn,
            &catalogue("2026-12-31", "9"),
            Taken::Uploaded,
            Actor::System,
            &Origin::default(),
            at,
        )
        .await
        .unwrap();
        // A start leaves what an admin gave.
        assert_eq!(
            load_shipped_catalogue(&mut conn, &shipped, at)
                .await
                .unwrap(),
            CatalogueOutcome::Left {
                release_is_newer: false
            }
        );

        let loaded =
            put_back_shipped_catalogue(&mut conn, &shipped, Actor::System, &Origin::default(), at)
                .await
                .unwrap();

        assert_eq!(loaded.replaced, NaiveDate::from_ymd_opt(2026, 12, 31));
        assert_eq!(
            in_force(&mut conn).await,
            (
                "shipped".to_owned(),
                "2026-10-10".to_owned(),
                None,
                Some("1".to_owned())
            )
        );
        // And it is a shipped one again, which a later release replaces.
        assert_eq!(
            load_shipped_catalogue(&mut conn, &catalogue("2026-11-01", "4"), at)
                .await
                .unwrap(),
            CatalogueOutcome::Replaced {
                older: NaiveDate::from_ymd_opt(2026, 10, 10).unwrap()
            }
        );
    }

    #[tokio::test]
    async fn every_load_is_audited_with_its_actor_where_it_came_from_and_what_it_replaced() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = clock::start();
        load_shipped_catalogue(&mut conn, &catalogue("2026-10-10", "1"), at)
            .await
            .unwrap();
        let origin = Origin {
            as_admin: None,
            ip: None,
            user_agent: Some("orchctl".to_owned()),
            request_id: None,
        };

        load_catalogue(
            &mut conn,
            &catalogue("2026-11-01", "2"),
            Taken::Uploaded,
            Actor::System,
            &origin,
            at,
        )
        .await
        .unwrap();

        let (actor, agent, details): (String, Option<String>, Json) = sqlx::query_as(
            "SELECT actor_kind, user_agent, details FROM audit_log
              WHERE action = 'rate_catalogue.loaded' ORDER BY at DESC, id DESC LIMIT 1",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(
            (actor.as_str(), agent.as_deref()),
            ("system", Some("orchctl"))
        );
        assert_eq!(details["origin"], "uploaded");
        assert_eq!(details["as_of"], "2026-11-01");
        assert_eq!(details["replaced_as_of"], "2026-10-10");
        assert_eq!(details["families"], 1);
    }

    #[tokio::test]
    async fn the_systems_defaults_are_written_before_anybody_is_there_and_never_after() {
        let test = orch_testkit::database::TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = orch_testkit::clock::start();
        let made = system_defaults(&mut conn, at).await.unwrap();
        assert_eq!(made as usize, orch_access_contract::pages::shipped().len());
        assert!(made > 0);
        // A second start before anybody set the system up writes nothing more.
        assert_eq!(system_defaults(&mut conn, at).await.unwrap(), 0);
        // Somebody set it up and emptied every row: no later start brings one back.
        crate::queries::identity::testing::insert_user(&mut conn, orch_types::new_id(), "ada")
            .await
            .unwrap();
        sqlx::query("DELETE FROM permission_entries WHERE page = 'defaults'")
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(system_defaults(&mut conn, at).await.unwrap(), 0);
        let left: i64 =
            sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE page = 'defaults'")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(left, 0);
    }
}
