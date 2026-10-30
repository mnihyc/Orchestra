//! Transport and contract version ranges, required and optional methods.
//!
//! The transport is made to last: versions are negotiated, optional parts are declared, unknown things are
//! tolerated, and an implementation may carry members of its own. The first exchange is always:
//!
//! ```text
//! host   → { "id": 0, "method": "hello", "params": { "kind": "driver", "transport": [1, 1], "contract": [1, 1],
//!                                                     "host_version": "0.1.0" } }
//! plugin → { "id": 0, "result": { "kind": "driver", "transport": 1, "contract": 1,
//!                                 "name": "firecracker", "version": "0.3.0",
//!                                 "methods": ["describe", "create", "start", "stop", "remove", "inspect", "list",
//!                                             "ensure_volume", "remove_volume"],
//!                                 "descriptor": { … } } }
//! ```
//!
//! | Rule | Detail |
//! |------|--------|
//! | Versions are ranges | The host offers `[lowest, highest]` for the transport and for the kind's contract; the plugin answers with the one it will speak. The host keeps supporting the previous version of each beside the current one. No common version, or another `kind`, stops the plugin |
//! | Optional parts are declared | `methods` lists what the plugin implements. Each contract marks its methods required or optional. A missing required method fails the handshake; a missing optional one is simply never called, and what depends on it is shown as unavailable |
//! | Unknown is tolerated | Both sides ignore members they do not know. A method a side does not know is answered with the error `unsupported`, never by failing |
//! | Versions only add | A contract version adds members and optional methods. Removing one, or changing what one means, is a new version |
//! | Room for its own | Descriptor members whose names start with `x_` belong to the implementation ([`crate::descriptor::Extensions`]) |
//!
//! [`offer`] is what a host sends, [`answer`] is what a plugin picks, and [`accept`] is what a host checks of
//! the answer. [`methods`] is the catalogue of each kind's methods, with which are required.

use std::collections::BTreeSet;
use std::fmt;

use orch_types::Json;
use schemars::JsonSchema;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::descriptor::PluginIdentity;
use crate::registration::PluginKind;

/// The version of the transport that this host speaks: the lines and the exchanges of [`crate::messages`].
pub const TRANSPORT_VERSION: u32 = 1;
/// The version of the contract of every kind that this host speaks.
pub const CONTRACT_VERSION: u32 = 1;

/// A range of versions, `[lowest, highest]`, written as a pair of numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VersionRange {
    lowest: u32,
    highest: u32,
}

/// A range whose lowest version is above its highest, or that starts below 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "a version range is [lowest, highest] with 1 <= lowest <= highest, and [{lowest}, {highest}] is not"
)]
pub struct RangeError {
    lowest: u32,
    highest: u32,
}

impl VersionRange {
    /// The range `[lowest, highest]`.
    pub const fn new(lowest: u32, highest: u32) -> Result<Self, RangeError> {
        if lowest >= 1 && lowest <= highest {
            Ok(Self { lowest, highest })
        } else {
            Err(RangeError { lowest, highest })
        }
    }

    /// The range of one version.
    #[must_use]
    pub const fn only(version: u32) -> Self {
        let version = if version == 0 { 1 } else { version };
        Self {
            lowest: version,
            highest: version,
        }
    }

    /// What a host that speaks `current` offers: the previous version beside it, where there is one. The
    /// host keeps supporting the previous version of each beside the current one.
    #[must_use]
    pub const fn up_to(current: u32) -> Self {
        let current = if current == 0 { 1 } else { current };
        Self {
            lowest: if current > 1 { current - 1 } else { 1 },
            highest: current,
        }
    }

    /// The lowest version.
    #[must_use]
    pub const fn lowest(&self) -> u32 {
        self.lowest
    }

    /// The highest version.
    #[must_use]
    pub const fn highest(&self) -> u32 {
        self.highest
    }

    /// Whether `version` is in the range.
    #[must_use]
    pub const fn contains(&self, version: u32) -> bool {
        self.lowest <= version && version <= self.highest
    }

    /// The highest version that is in both ranges, if there is one.
    #[must_use]
    pub const fn latest_common(&self, other: &Self) -> Option<u32> {
        let highest = if self.highest < other.highest {
            self.highest
        } else {
            other.highest
        };
        let lowest = if self.lowest > other.lowest {
            self.lowest
        } else {
            other.lowest
        };
        if lowest <= highest {
            Some(highest)
        } else {
            None
        }
    }
}

impl fmt::Display for VersionRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}, {}]", self.lowest, self.highest)
    }
}

impl Serialize for VersionRange {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        [self.lowest, self.highest].serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for VersionRange {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let [lowest, highest] = <[u32; 2]>::deserialize(deserializer)?;
        Self::new(lowest, highest).map_err(D::Error::custom)
    }
}

impl JsonSchema for VersionRange {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "VersionRange".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "array", "items": { "type": "integer", "minimum": 1 }, "minItems": 2, "maxItems": 2,
            "description": "[lowest, highest]"
        })
    }
}

/// What the host says first: the kind it expects and the versions it speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hello {
    /// The kind of implementation the host expects.
    pub kind: PluginKind,
    /// The versions of the transport it speaks.
    pub transport: VersionRange,
    /// The versions of the kind's contract it speaks.
    pub contract: VersionRange,
    /// The version of the host.
    pub host_version: String,
}

/// What the plugin answers: the versions it will speak, who it is, what it implements, and its descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HelloReply {
    /// The kind it implements.
    pub kind: PluginKind,
    /// The version of the transport it will speak.
    pub transport: u32,
    /// The version of the kind's contract it will speak.
    pub contract: u32,
    /// Its name, as it is registered.
    pub name: String,
    /// Its own version.
    pub version: String,
    /// The methods it implements.
    pub methods: Vec<String>,
    /// The kind's descriptor, with members of its own among them.
    pub descriptor: Json,
}

/// The offer of a host that speaks the current versions and the previous ones.
#[must_use]
pub fn offer(kind: PluginKind, host_version: &str) -> Hello {
    Hello {
        kind,
        transport: VersionRange::up_to(TRANSPORT_VERSION),
        contract: VersionRange::up_to(CONTRACT_VERSION),
        host_version: host_version.to_owned(),
    }
}

/// What a plugin supports: the versions of the transport and of its kind's contract it can speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Support {
    /// The versions of the transport.
    pub transport: VersionRange,
    /// The versions of the contract.
    pub contract: VersionRange,
}

/// Why a handshake stops the plugin.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandshakeError {
    /// The plugin implements another kind than the host expected.
    #[error("the host expected a {expected} and the plugin is a {found}")]
    KindMismatch {
        /// What the host expected.
        expected: PluginKind,
        /// What the plugin said.
        found: PluginKind,
    },
    /// The two sides speak no version of the transport in common.
    #[error(
        "no version of the transport in common: the host offers {offered}, the plugin speaks {answered}"
    )]
    Transport {
        /// What the host offered.
        offered: VersionRange,
        /// What the plugin answered; for a plugin choosing, the range it supports.
        answered: String,
    },
    /// The two sides speak no version of the kind's contract in common.
    #[error(
        "no version of the contract in common: the host offers {offered}, the plugin speaks {answered}"
    )]
    Contract {
        /// What the host offered.
        offered: VersionRange,
        /// What the plugin answered; for a plugin choosing, the range it supports.
        answered: String,
    },
    /// The plugin does not say who it is.
    #[error("a plugin states its name and its version")]
    Anonymous,
    /// The plugin lacks methods its kind requires.
    #[error("the plugin lacks required methods: {}", .0.join(", "))]
    MissingRequired(Vec<String>),
}

/// What a plugin picks from an offer: the highest version of the transport and of the contract that it
/// supports and the host offers.
pub fn answer(hello: &Hello, support: &Support) -> Result<(u32, u32), HandshakeError> {
    let transport =
        hello
            .transport
            .latest_common(&support.transport)
            .ok_or(HandshakeError::Transport {
                offered: hello.transport,
                answered: support.transport.to_string(),
            })?;
    let contract =
        hello
            .contract
            .latest_common(&support.contract)
            .ok_or(HandshakeError::Contract {
                offered: hello.contract,
                answered: support.contract.to_string(),
            })?;
    Ok((transport, contract))
}

/// The methods a plugin implements, as it declared them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodSet {
    kind: PluginKind,
    declared: BTreeSet<String>,
}

impl MethodSet {
    /// Whether the plugin implements `method`. A method it does not implement is never called, and what
    /// depends on it is shown as unavailable.
    #[must_use]
    pub fn supports(&self, method: &str) -> bool {
        self.declared.contains(method)
    }

    /// The optional methods of the kind that the plugin does not implement: what is unavailable.
    #[must_use]
    pub fn unavailable(&self) -> Vec<&'static str> {
        methods(self.kind)
            .iter()
            .filter(|spec| !self.declared.contains(spec.name))
            .map(|spec| spec.name)
            .collect()
    }

    /// Every method the plugin declared, those this host does not know among them.
    pub fn declared(&self) -> impl Iterator<Item = &str> {
        self.declared.iter().map(String::as_str)
    }
}

/// What a host knows of a plugin once the handshake succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Negotiated {
    /// Who the plugin is.
    pub identity: PluginIdentity,
    /// The version of the transport that will be spoken.
    pub transport: u32,
    /// The version of the contract that will be spoken.
    pub contract: u32,
    /// What it implements.
    pub methods: MethodSet,
    /// Its descriptor, to be read as the kind's descriptor.
    pub descriptor: Json,
}

/// Checks a plugin's answer to an offer: it is the kind the host expected; the versions it will speak are
/// inside what the host offered; it says who it is; and it implements every method its kind requires. Methods
/// and members the host does not know are tolerated.
pub fn accept(sent: &Hello, reply: HelloReply) -> Result<Negotiated, HandshakeError> {
    if reply.kind != sent.kind {
        return Err(HandshakeError::KindMismatch {
            expected: sent.kind,
            found: reply.kind,
        });
    }
    if !sent.transport.contains(reply.transport) {
        return Err(HandshakeError::Transport {
            offered: sent.transport,
            answered: reply.transport.to_string(),
        });
    }
    if !sent.contract.contains(reply.contract) {
        return Err(HandshakeError::Contract {
            offered: sent.contract,
            answered: reply.contract.to_string(),
        });
    }
    if reply.name.is_empty() || reply.version.is_empty() {
        return Err(HandshakeError::Anonymous);
    }
    let declared: BTreeSet<String> = reply.methods.into_iter().collect();
    let missing: Vec<String> = methods(sent.kind)
        .iter()
        .filter(|spec| spec.required && !declared.contains(spec.name))
        .map(|spec| spec.name.to_owned())
        .collect();
    if !missing.is_empty() {
        return Err(HandshakeError::MissingRequired(missing));
    }
    Ok(Negotiated {
        identity: PluginIdentity {
            kind: sent.kind,
            name: reply.name,
            version: reply.version,
        },
        transport: reply.transport,
        contract: reply.contract,
        methods: MethodSet {
            kind: sent.kind,
            declared,
        },
        descriptor: reply.descriptor,
    })
}

/// One method of a kind's contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodSpec {
    /// The method's name on the transport.
    pub name: &'static str,
    /// Whether a plugin without it fails the handshake.
    pub required: bool,
}

const fn required(name: &'static str) -> MethodSpec {
    MethodSpec {
        name,
        required: true,
    }
}

const fn optional(name: &'static str) -> MethodSpec {
    MethodSpec {
        name,
        required: false,
    }
}

/// The methods of an environment driver: a driver's `stats`, `validate`, `ensure_image` and `import` are
/// optional.
const DRIVER: [MethodSpec; 38] = [
    required("describe"),
    optional("validate"),
    required("create"),
    required("start"),
    required("stop"),
    required("remove"),
    required("inspect"),
    required("list"),
    optional("stats"),
    required("ensure_volume"),
    required("remove_volume"),
    optional("ensure_image"),
    optional("rebuild_image"),
    optional("registry_digest"),
    optional("storages"),
    optional("resources"),
    optional("remove_resource"),
    optional("stop_resource"),
    optional("host_resources"),
    optional("configure"),
    optional("check_volume_carry"),
    optional("measure_volume"),
    optional("export_volume"),
    optional("import_volume"),
    optional("stop_volume_work"),
    optional("volume_exists"),
    optional("suspend"),
    optional("resume"),
    optional("apply_resources"),
    optional("apply_capabilities"),
    optional("estimate_keep"),
    optional("keep_files"),
    optional("kept_images"),
    optional("remove_kept_image"),
    optional("sweep_kept_metadata"),
    optional("export_kept_image"),
    optional("import_kept_image"),
    optional("import"),
];

/// The methods of an agent adapter: the agent-role commands of the link. What every adapter does is
/// required; the rest belongs to a capability the adapter may not declare.
const ADAPTER: [MethodSpec; 24] = [
    required("agent.start"),
    required("agent.stop"),
    required("session.open"),
    required("session.close"),
    required("turn.start"),
    optional("turn.steer"),
    optional("turn.interrupt"),
    optional("approval.resolve"),
    optional("orch.tool_result"),
    optional("session.export"),
    optional("session.resync"),
    optional("agent.discover_sessions"),
    optional("mcp.discover"),
    optional("mcp.import"),
    optional("provider.login.start"),
    optional("provider.login.input"),
    optional("provider.login.cancel"),
    optional("provider.logout"),
    optional("provider.refresh"),
    optional("provider.discover_models"),
    optional("provider.credential.issue"),
    optional("provider.credential.accept"),
    optional("agent.credential.return"),
    optional("agent.credential.set"),
];

/// The methods of an executor.
const EXECUTOR: [MethodSpec; 4] = [
    required("describe"),
    required("available"),
    required("start"),
    required("stop"),
];

/// The methods of a protocol translator. The response stream is converted by a converter that the plugin
/// keeps for one response: `response_translator` opens it, `translate_chunk` feeds it and `finish_response`
/// ends it.
const TRANSLATOR: [MethodSpec; 5] = [
    required("describe"),
    required("translate_request"),
    required("response_translator"),
    required("translate_chunk"),
    required("finish_response"),
];

/// The methods of a summarizer.
const SUMMARIZER: [MethodSpec; 2] = [required("describe"), required("summarize")];

/// The methods of a notifier: `link_url` and `poll_links` are for one that links people, and optional for
/// the rest.
const NOTIFIER: [MethodSpec; 5] = [
    required("describe"),
    required("check"),
    required("deliver"),
    optional("link_url"),
    optional("poll_links"),
];

/// The methods of a provider source: it describes itself, and implements any of the rest, as its descriptor
/// declares (facts with the resets they list, models, the presentation of a key).
const PROVIDER: [MethodSpec; 5] = [
    required("describe"),
    optional("facts.read"),
    optional("models.list"),
    optional("resets.spend"),
    optional("credential.present"),
];

/// The methods of a kind, with which are required. The names are the trait's methods in `snake_case`, and
/// for adapters the command names of the link.
#[must_use]
pub fn methods(kind: PluginKind) -> &'static [MethodSpec] {
    match kind {
        PluginKind::Driver => &DRIVER,
        PluginKind::Adapter => &ADAPTER,
        PluginKind::Executor => &EXECUTOR,
        PluginKind::Translator => &TRANSLATOR,
        PluginKind::Summarizer => &SUMMARIZER,
        PluginKind::Notifier => &NOTIFIER,
        PluginKind::Provider => &PROVIDER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(kind: PluginKind, methods: &[&str]) -> HelloReply {
        HelloReply {
            kind,
            transport: 1,
            contract: 1,
            name: "firecracker".into(),
            version: "0.3.0".into(),
            methods: methods.iter().map(ToString::to_string).collect(),
            descriptor: json!({ "name": "firecracker", "x_kernel": "/var/lib/fc/vmlinux" }),
        }
    }

    const DRIVER_METHODS: [&str; 9] = [
        "describe",
        "create",
        "start",
        "stop",
        "remove",
        "inspect",
        "list",
        "ensure_volume",
        "remove_volume",
    ];

    #[test]
    fn the_first_exchange_reads_and_writes_as_the_spec_shows_it() {
        let hello: Hello = serde_json::from_value(json!({
            "kind": "driver", "transport": [1, 1], "contract": [1, 1], "host_version": "0.1.0"
        }))
        .unwrap();
        assert_eq!(hello, offer(PluginKind::Driver, "0.1.0"));
        assert_eq!(
            serde_json::to_value(&hello).unwrap()["transport"],
            json!([1, 1])
        );
        let answered: HelloReply = serde_json::from_value(json!({
            "kind": "driver", "transport": 1, "contract": 1, "name": "firecracker", "version": "0.3.0",
            "methods": ["describe", "create", "start", "stop", "remove", "inspect", "list", "ensure_volume", "remove_volume"],
            "descriptor": {}
        }))
        .unwrap();
        let negotiated = accept(&hello, answered).unwrap();
        assert_eq!(negotiated.identity.name, "firecracker");
        assert_eq!((negotiated.transport, negotiated.contract), (1, 1));
    }

    #[test]
    fn a_range_is_a_pair_with_the_lowest_first_and_a_host_offers_the_previous_version_beside_the_current()
     {
        assert_eq!(VersionRange::up_to(1), VersionRange::new(1, 1).unwrap());
        assert_eq!(VersionRange::up_to(3), VersionRange::new(2, 3).unwrap());
        assert!(VersionRange::new(2, 1).is_err());
        assert!(VersionRange::new(0, 1).is_err());
        assert!(serde_json::from_value::<VersionRange>(json!([3, 2])).is_err());
        assert!(serde_json::from_value::<VersionRange>(json!([1, 2, 3])).is_err());
        let range = VersionRange::new(2, 4).unwrap();
        assert!(range.contains(2) && range.contains(4) && !range.contains(1) && !range.contains(5));
        assert_eq!(range.to_string(), "[2, 4]");
    }

    #[test]
    fn the_plugin_picks_the_highest_version_both_sides_speak_and_no_common_version_stops_it() {
        let wide = VersionRange::new(1, 3).unwrap();
        assert_eq!(
            wide.latest_common(&VersionRange::new(2, 5).unwrap()),
            Some(3)
        );
        assert_eq!(wide.latest_common(&VersionRange::only(1)), Some(1));
        assert_eq!(wide.latest_common(&VersionRange::new(4, 5).unwrap()), None);
        let hello = Hello {
            kind: PluginKind::Driver,
            transport: VersionRange::new(1, 2).unwrap(),
            contract: VersionRange::new(2, 3).unwrap(),
            host_version: "0.2.0".into(),
        };
        let both = Support {
            transport: VersionRange::new(1, 5).unwrap(),
            contract: VersionRange::new(1, 2).unwrap(),
        };
        assert_eq!(answer(&hello, &both), Ok((2, 2)));
        let old = Support {
            transport: VersionRange::only(1),
            contract: VersionRange::only(1),
        };
        assert!(matches!(
            answer(&hello, &old),
            Err(HandshakeError::Contract { .. })
        ));
        let new = Support {
            transport: VersionRange::new(3, 4).unwrap(),
            contract: VersionRange::only(2),
        };
        assert!(matches!(
            answer(&hello, &new),
            Err(HandshakeError::Transport { .. })
        ));
    }

    #[test]
    fn a_plugin_of_another_kind_or_of_a_version_the_host_did_not_offer_is_stopped() {
        let hello = offer(PluginKind::Driver, "0.1.0");
        assert_eq!(
            accept(&hello, reply(PluginKind::Notifier, &DRIVER_METHODS)),
            Err(HandshakeError::KindMismatch {
                expected: PluginKind::Driver,
                found: PluginKind::Notifier
            })
        );
        let mut later = reply(PluginKind::Driver, &DRIVER_METHODS);
        later.transport = 2;
        assert!(matches!(
            accept(&hello, later),
            Err(HandshakeError::Transport { .. })
        ));
        let mut newer_contract = reply(PluginKind::Driver, &DRIVER_METHODS);
        newer_contract.contract = 7;
        assert!(matches!(
            accept(&hello, newer_contract),
            Err(HandshakeError::Contract { .. })
        ));
        let mut anonymous = reply(PluginKind::Driver, &DRIVER_METHODS);
        anonymous.name.clear();
        assert_eq!(accept(&hello, anonymous), Err(HandshakeError::Anonymous));
    }

    #[test]
    fn a_missing_required_method_fails_the_handshake_and_a_missing_optional_one_is_unavailable() {
        let hello = offer(PluginKind::Driver, "0.1.0");
        let without_remove: Vec<&str> = DRIVER_METHODS
            .iter()
            .copied()
            .filter(|m| *m != "remove")
            .collect();
        assert_eq!(
            accept(&hello, reply(PluginKind::Driver, &without_remove)),
            Err(HandshakeError::MissingRequired(vec!["remove".into()]))
        );
        let negotiated = accept(&hello, reply(PluginKind::Driver, &DRIVER_METHODS)).unwrap();
        assert!(negotiated.methods.supports("create"));
        assert!(!negotiated.methods.supports("stats"));
        assert_eq!(
            negotiated.methods.unavailable(),
            [
                "validate",
                "stats",
                "ensure_image",
                "rebuild_image",
                "registry_digest",
                "storages",
                "resources",
                "remove_resource",
                "stop_resource",
                "host_resources",
                "configure",
                "check_volume_carry",
                "measure_volume",
                "export_volume",
                "import_volume",
                "stop_volume_work",
                "volume_exists",
                "suspend",
                "resume",
                "apply_resources",
                "apply_capabilities",
                "estimate_keep",
                "keep_files",
                "kept_images",
                "remove_kept_image",
                "sweep_kept_metadata",
                "export_kept_image",
                "import_kept_image",
                "import"
            ]
        );
        let mut with_more = DRIVER_METHODS.to_vec();
        with_more.extend(["stats", "x_snapshot"]);
        let negotiated = accept(&hello, reply(PluginKind::Driver, &with_more)).unwrap();
        assert!(
            negotiated.methods.supports("stats"),
            "an optional method that is declared is called"
        );
        assert!(
            negotiated.methods.declared().any(|m| m == "x_snapshot"),
            "a method this host does not know is tolerated"
        );
        assert_eq!(negotiated.descriptor["x_kernel"], "/var/lib/fc/vmlinux");
    }

    #[test]
    fn each_kind_marks_its_methods_required_or_optional() {
        let required = |kind| -> Vec<&'static str> {
            methods(kind)
                .iter()
                .filter(|spec| spec.required)
                .map(|spec| spec.name)
                .collect()
        };
        assert_eq!(
            required(PluginKind::Driver),
            [
                "describe",
                "create",
                "start",
                "stop",
                "remove",
                "inspect",
                "list",
                "ensure_volume",
                "remove_volume"
            ],
            "a driver's stats, validate, ensure_image and import are optional"
        );
        assert_eq!(required(PluginKind::Summarizer), ["describe", "summarize"]);
        assert_eq!(
            required(PluginKind::Notifier),
            ["describe", "check", "deliver"]
        );
        assert_eq!(
            required(PluginKind::Executor),
            ["describe", "available", "start", "stop"]
        );
        assert_eq!(
            required(PluginKind::Adapter),
            [
                "agent.start",
                "agent.stop",
                "session.open",
                "session.close",
                "turn.start"
            ]
        );
        assert_eq!(required(PluginKind::Translator).len(), 5);
        for kind in PluginKind::ALL {
            let all = methods(kind);
            let mut names: Vec<_> = all.iter().map(|spec| spec.name).collect();
            names.sort_unstable();
            names.dedup();
            assert_eq!(names.len(), all.len(), "{kind} names a method twice");
        }
    }
}
