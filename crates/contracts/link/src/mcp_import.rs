//! Local tool-server requests carried on the authenticated machine's existing request channel.

use orch_api_contract::environment::McpScope;
use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One import may run a thirty-second machine command; leave time for admission, copying and its reply.
pub const ANSWER_WITHIN_SECS: u64 = 75;
/// A local console cannot accumulate an unbounded collection of unanswered tool-server requests.
pub const MAX_PENDING: usize = 16;

/// Only safe origin metadata crosses the link; the definition and its secrets stay on the machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpImportAsk {
    /// Correlation and replay key, also the identifier of the copy the system files.
    pub request_id: Id,
    /// A registered environment on this authenticated machine; supplies the discovered configuration's context.
    pub environment_id: Id,
    /// The harness that found it.
    pub source: String,
    /// User configuration or a directory's configuration.
    pub scope: McpScope,
    /// For a directory configuration, its relative workspace directory.
    pub directory: Option<String>,
    /// The name in that configuration.
    pub name: String,
    /// An optional resource name; the original configured name remains the import origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_name: Option<String>,
}

/// A local request in the existing import channel; the original import bytes stay unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum McpLocalAsk {
    /// File one actual discovered definition.
    Import(McpImportAsk),
    /// Forget one recorded imported resource and its native copy.
    Forget(McpForgetAsk),
}
impl McpLocalAsk {
    /// The same correlation identity for either local act.
    #[must_use]
    pub fn request_id(&self) -> Id {
        match self {
            Self::Import(request) => request.request_id,
            Self::Forget(request) => request.request_id,
        }
    }
}
impl From<McpImportAsk> for McpLocalAsk {
    fn from(request: McpImportAsk) -> Self {
        Self::Import(request)
    }
}
impl From<McpForgetAsk> for McpLocalAsk {
    fn from(request: McpForgetAsk) -> Self {
        Self::Forget(request)
    }
}

/// A distinct strict payload, with no fictitious environment or importing identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpForgetAsk {
    /// Correlation of this local request.
    pub request_id: Id,
    /// The original configured name, the resource's alias, or its exact identifier.
    pub forget: McpForgetSelection,
}

/// Safe selector metadata; the controller resolves the actual resource and fresh authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpForgetSelection {
    /// The name or identifier typed on the machine.
    pub name: String,
    /// The harness kind, where disambiguation is needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// The system's definitive result, or a refusal; absence on a broken link is not proof of failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpImportAnswer {
    /// The request answered.
    pub request_id: Id,
    /// The server filed or forgotten on success.
    pub server_id: Option<Id>,
    /// A stable refusal code, absent on success.
    pub code: Option<String>,
    /// A safe sentence for the local command or console.
    pub detail: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    use orch_types::new_id;
    use serde_json::json;

    #[test]
    fn native_tool_requests_keep_import_bytes_and_distinguish_forget_without_an_actor() {
        let old = McpImportAsk {
            request_id: new_id(),
            environment_id: new_id(),
            source: "fake".into(),
            scope: McpScope::User,
            directory: None,
            name: "native:name".into(),
            as_name: Some("native-tools".into()),
        };
        let bytes = serde_json::to_value(&old).unwrap();
        let import = McpLocalAsk::from(old.clone());
        assert_eq!(serde_json::to_value(&import).unwrap(), bytes);
        assert_eq!(
            serde_json::from_value::<McpLocalAsk>(bytes.clone()).unwrap(),
            import
        );
        assert_eq!(import.request_id(), old.request_id);

        let id = new_id();
        let forget = McpLocalAsk::from(McpForgetAsk {
            request_id: id,
            forget: McpForgetSelection {
                name: "native-tools".into(),
                from: Some("fake".into()),
            },
        });
        let payload = json!({"request_id":id,"forget":{"name":"native-tools","from":"fake"}});
        assert_eq!(serde_json::to_value(&forget).unwrap(), payload);
        assert_eq!(
            serde_json::from_value::<McpLocalAsk>(payload.clone()).unwrap(),
            forget
        );
        assert_eq!(forget.request_id(), id);
        for field in [
            "actor_id",
            "user_id",
            "machine_id",
            "environment_id",
            "admin",
        ] {
            let mut forged = payload.clone();
            forged[field] = json!(new_id());
            assert!(
                serde_json::from_value::<McpLocalAsk>(forged).is_err(),
                "{field}"
            );
        }
        let mut mixed = bytes;
        mixed["forget"] = payload["forget"].clone();
        assert!(serde_json::from_value::<McpLocalAsk>(mixed).is_err());
        let mut forged = payload;
        forged["forget"]["actor_id"] = json!(new_id());
        assert!(serde_json::from_value::<McpLocalAsk>(forged).is_err());
    }
}
