//! Live native session commands and factual termination observations.

pub use orch_api_contract::native::{
    BackgroundEndState, BackgroundPlace, BackgroundState, COMMAND_PREVIEW_BYTES, CommandCleanup,
    NativeBackground, NativeBackgroundCommand, NativeBackgroundEnd, PAGE_BYTES, SNAPSHOT_COMMANDS,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_native_control_never_claims_a_zero_roster() {
        let roster = NativeBackground::unsupported("exact native End is unavailable");
        assert!(!roster.supported);
        assert_eq!(roster.state, BackgroundState::Unavailable);
        assert_eq!(roster.total, None);
        assert_eq!(roster.as_of, None);
        let value = serde_json::to_value(&roster).unwrap();
        assert_eq!(value["total"], serde_json::Value::Null);
        assert_eq!(
            serde_json::from_value::<NativeBackground>(value).unwrap(),
            roster
        );
    }
}
