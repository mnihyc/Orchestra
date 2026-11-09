//! A capability's reported outcome is separate from a failed infrastructure operation.
use super::*;

/// Safe capability facts retained for the ordinary tool reply and existing call rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityFailure {
    pub capability: String,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Time>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper: Option<Json>,
}
impl std::fmt::Display for CapabilityFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CapabilityFailure {}
impl From<CapabilityFailure> for EngineError {
    fn from(failure: CapabilityFailure) -> Self {
        Self::Capability(Box::new(failure))
    }
}
impl CapabilityFailure {
    pub(super) fn unavailable(capability: &str, reason: &str, message: &str) -> Self {
        Self {
            capability: capability.into(),
            code: "unavailable".into(),
            message: message.into(),
            reason: Some(reason.into()),
            retryable: false,
            window: None,
            resets_at: None,
            helper: None,
        }
    }
    pub(super) fn with_helper(mut self, helper: Json) -> Self {
        self.helper = Some(helper);
        self
    }
    pub(super) fn native(
        capability: &str,
        error: &Json,
        helper: Json,
    ) -> Result<Self, EngineError> {
        let failure = error.pointer("/details/failure").unwrap_or(error);
        Ok(Self {
            capability: capability.into(),
            code: failure
                .get("code")
                .and_then(Json::as_str)
                .unwrap_or("unavailable")
                .into(),
            message: failure
                .get("message")
                .and_then(Json::as_str)
                .or_else(|| error.get("message").and_then(Json::as_str))
                .unwrap_or("The native capability did not return a successful result.")
                .into(),
            reason: failure
                .get("reason")
                .and_then(Json::as_str)
                .map(str::to_owned),
            retryable: failure
                .get("retryable")
                .and_then(Json::as_bool)
                .unwrap_or(false),
            window: failure
                .get("window")
                .or_else(|| error.get("window"))
                .and_then(Json::as_str)
                .map(str::to_owned),
            resets_at: failure
                .get("resets_at")
                .or_else(|| error.get("resets_at"))
                .filter(|value| !value.is_null())
                .cloned()
                .map(serde_json::from_value)
                .transpose()?,
            helper: Some(helper),
        })
    }
    /// Only normalized failure facts enter recovery; helper identity is not a rule or acting credential.
    pub(crate) fn rule_failure(&self) -> Json {
        let mut value = serde_json::json!(self);
        if let Some(value) = value.as_object_mut() {
            value.remove("helper");
        }
        value
    }
    /// An explicit failed domain result preserves the helper link without fabricating an accepted image.
    pub(crate) fn reply(&self) -> Json {
        let mut value =
            serde_json::json!({"state":"failed","isError":true,"failure":self.rule_failure()});
        if let Some(helper) = &self.helper {
            value["helper"] = helper.clone();
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_failure_facts_remain_structured_and_never_become_a_successful_image()
    -> Result<(), EngineError> {
        let helper = serde_json::json!({"thread_id":"actual-helper"});
        let error = serde_json::json!({"code":"image_generation_limit","message":"Native image limit","window":"synthetic-images","resets_at":"2026-10-15T13:00:00Z",
            "details":{"failure":{"code":"rate_limited","message":"Native image limit","reason":"capability_limit","retryable":true}}});
        let failed = CapabilityFailure::native("image_generation", &error, helper.clone())?;
        let reply = failed.reply();
        assert!(orch_agent_contract::tool_answers::reported_failure(&reply));
        assert_eq!(reply["helper"], helper);
        assert_eq!(reply["failure"]["resets_at"], error["resets_at"]);
        assert_eq!(reply["failure"]["window"], error["window"]);
        assert!(failed.rule_failure().get("helper").is_none());
        assert!(reply.get("images").is_none());
        Ok(())
    }
}
