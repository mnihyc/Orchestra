//! A workspace's own harness configuration: whether a harness may use the `.codex` or `.claude` its environment's
//! workspace holds.
//!
//! The first time a session of a harness would start in a workspace whose directories hold a configuration of its
//! own (anything in it but empty directories), the people who manage the environment are asked once, in the thread
//! and in their Inbox: Trust, and the harness uses it as in a folder it trusts; Keep it out, and it works without
//! it as in a folder it does not. The threads that meet it wait for the answer. An agent never answers. The answer
//! is remembered for the environment and the harness ([`ConfigTrust`], the environment's `config_trust`), shown
//! and changed on the environment's page, and asked again only when the files change; a directory the harness
//! made for itself and left empty is not asked about.

use orch_types::Time;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::routes::Access::Rule;
use crate::users::UserRef;

/// What the people who manage an environment answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigTrustAnswer {
    /// The harness uses the workspace's configuration, as it does in a folder it trusts.
    Trust,
    /// The harness works without it, as it does in a folder it does not trust.
    KeepOut,
}

/// The workspace's own configuration of one harness, and the answer given for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigTrust {
    /// The harness, by its kind.
    pub harness: String,
    /// The same harness, named.
    pub program: crate::resources::HarnessKindName,
    /// What was answered; `null` while the question waits.
    pub answer: Option<ConfigTrustAnswer>,
    /// Who answered.
    pub by: Option<UserRef>,
    /// When.
    pub at: Option<Time>,
    /// Where the configuration was found, relative to the workspace root.
    pub path: String,
}

/// The answer to give for a harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigTrustBody {
    /// Trust or Keep it out.
    pub answer: ConfigTrustAnswer,
}

crate::endpoints! {
    /// Answers, or changes the answer, for the workspace's own configuration of a harness: remembered until it is changed here or the files change; the threads that waited go on.
    EnvironmentConfigTrustSet: PUT "/environments/{id}/config-trust/{harness}", Rule("environment.manage; never an agent's token"), none, json ConfigTrustBody, json crate::environment::Environment, 200;
}
