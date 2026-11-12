//! Profile options that hold back the end of a turn, as the program that has them declares them.
//!
//! An engine that has asked a turn to stop ends the turn itself when the harness has not answered in a time it is
//! given (`[engine] interrupt_force_secs`). A program may hold the completion of a turn for a time of its own, for
//! instance to settle what the turn's sub-agents used before the turn is said to be over; that time is an option of
//! the agent's profile, and the longer it is set, the more of the time given to an unanswered interrupt it takes. The
//! program declares the options that do so and what it adds besides, and the engine refuses a profile whose options
//! do not fit under the time it gives, naming both.

use std::time::Duration;

use orch_types::Json;
use serde::{Deserialize, Serialize};

/// The retained extension key; a declaration cannot be changed or removed by descriptor overrides.
pub const EXTENSION: &str = "x_turn_end_delays";

/// The profile options of a program that hold back the end of a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnEndDelays {
    /// The first version of this additive declaration.
    pub version: u32,
    /// The options, each with what the program waits besides.
    #[serde(default)]
    pub delays: Vec<Delay>,
}

/// One option of the profile's configuration, in milliseconds, that holds back the end of a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delay {
    /// JSON pointer within the profile's configuration, such as `/child_usage_wait_ms`.
    pub pointer: String,
    /// What the program waits besides the option, in milliseconds (for the exit of its process).
    #[serde(default)]
    pub also_ms: u64,
}

impl TurnEndDelays {
    /// Whether every option set in `config` fits under `bound`, the time the engine gives an unanswered interrupt.
    ///
    /// # Errors
    /// The words that name the option, what it holds back and the bound.
    pub fn check(&self, config: &Json, bound: Duration) -> Result<(), String> {
        if self.version != 1 {
            return Err(
                "the program's declaration of the options that hold back the end of a turn is unsupported"
                    .to_owned(),
            );
        }
        let bound_ms = u64::try_from(bound.as_millis()).unwrap_or(u64::MAX);
        for delay in &self.delays {
            let Some(set) = config.pointer(&delay.pointer).and_then(Json::as_u64) else {
                continue;
            };
            let held = set.saturating_add(delay.also_ms);
            if held > bound_ms {
                return Err(format!(
                    "the agent profile's `{}` ({set} ms, and {} ms more that the program waits for its process to end) holds back the end of a turn for longer than the {bound_ms} ms that `[engine] interrupt_force_secs` gives an unanswered interrupt: lower the option or raise the setting",
                    delay.pointer.trim_start_matches('/'),
                    delay.also_ms,
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn declared() -> TurnEndDelays {
        TurnEndDelays {
            version: 1,
            delays: vec![Delay {
                pointer: "/child_usage_wait_ms".into(),
                also_ms: 8_000,
            }],
        }
    }

    #[test]
    fn an_option_that_fits_under_the_bound_is_accepted_and_one_that_does_not_is_refused_naming_both() {
        let bound = Duration::from_secs(30);
        assert!(declared().check(&json!({}), bound).is_ok());
        assert!(
            declared()
                .check(&json!({"child_usage_wait_ms": 22_000}), bound)
                .is_ok()
        );
        let refused = declared()
            .check(&json!({"child_usage_wait_ms": 22_001}), bound)
            .unwrap_err();
        assert!(refused.contains("child_usage_wait_ms"), "{refused}");
        assert!(refused.contains("interrupt_force_secs"), "{refused}");
        assert!(refused.contains("30000"), "{refused}");
    }

    #[test]
    fn a_declaration_of_another_version_is_not_read() {
        let mut other = declared();
        other.version = 2;
        assert!(other.check(&json!({}), Duration::from_secs(30)).is_err());
    }
}
