//! Profile options that hold back the end of a turn, as the program that has them declares them.
//!
//! An engine that has asked a turn to stop ends the turn itself when the harness has not answered in a time it is
//! given (`[engine] interrupt_force_secs`). A program may hold the completion of a turn for a time of its own, for
//! instance to settle what the turn's sub-agents used before the turn is said to be over; that time is an option of
//! the agent's profile, and the longer it is set, the more of the time given to an unanswered interrupt it takes. The
//! program declares the options that do so, what each is when the profile does not set it, and what the program
//! waits besides, and the engine fits the profile under the time it gives:
//! - a value the profile sets is honoured, or the profile is refused, naming the option and the setting; it is never
//!   shortened;
//! - it is refused only when the value itself does not fit: when what the program waits besides is already all the
//!   time there is, no value is at fault, and nothing is refused;
//! - an option the profile does not set takes its default, or less where the time leaves less, so that leaving it out
//!   is never worse than writing out the number it comes to.

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
    /// What the option is when the profile does not set it, in milliseconds.
    #[serde(default)]
    pub default_ms: u64,
    /// System registry setting supplying this default in milliseconds, where the program declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_setting: Option<String>,
}

/// An option the profile did not set that took less than its default, because the time left no more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lowered {
    /// The option's pointer in the profile's configuration.
    pub pointer: String,
    /// What the option is when the profile does not set it, in milliseconds.
    pub default_ms: u64,
    /// What it took, in milliseconds.
    pub to_ms: u64,
    /// The time the engine gives an unanswered interrupt, in milliseconds.
    pub bound_ms: u64,
}

impl TurnEndDelays {
    /// The declaration as the program made it, or `None` where it cannot be read or is of another version: such a
    /// declaration declares nothing.
    #[must_use]
    pub fn read(declared: &Json) -> Option<Self> {
        serde_json::from_value::<Self>(declared.clone())
            .ok()
            .filter(|read| read.version == 1)
    }

    /// Fits the profile's `config` under `bound`, the time the engine gives an unanswered interrupt: an option it sets is
    /// kept as it is, and an option it does not set takes the least of its default and the time that is left.
    ///
    /// Returns the options that took less than their default.
    ///
    /// # Errors
    /// The words that name the option, what it holds back and the bound, for a value set that does not fit.
    pub fn fit(&self, config: &mut Json, bound: Duration) -> Result<Vec<Lowered>, String> {
        let bound_ms = u64::try_from(bound.as_millis()).unwrap_or(u64::MAX);
        let mut lowered = Vec::new();
        for delay in &self.delays {
            // Where what the program waits besides is all the bound gives, no value of the option is at fault.
            if bound_ms <= delay.also_ms {
                continue;
            }
            let room = bound_ms - delay.also_ms;
            match config.pointer(&delay.pointer).and_then(Json::as_u64) {
                Some(set) if set > room => {
                    return Err(format!(
                        "the agent profile's `{}` ({set} ms, and {} ms more that the program waits for its process to end) holds back the end of a turn for longer than the {bound_ms} ms that `[engine] interrupt_force_secs` gives an unanswered interrupt: lower the option or raise the setting",
                        delay.pointer.trim_start_matches('/'),
                        delay.also_ms,
                    ));
                }
                Some(_) => {}
                None if delay.default_ms > room => {
                    put(config, &delay.pointer, room);
                    lowered.push(Lowered {
                        pointer: delay.pointer.clone(),
                        default_ms: delay.default_ms,
                        to_ms: room,
                        bound_ms,
                    });
                }
                None if delay.default_setting.is_some() => {
                    put(config, &delay.pointer, delay.default_ms);
                }
                None => {}
            }
        }
        Ok(lowered)
    }

    /// Whether the profile's `config` fits under `bound`; see [`TurnEndDelays::fit`].
    ///
    /// # Errors
    /// As [`TurnEndDelays::fit`].
    pub fn check(&self, config: &Json, bound: Duration) -> Result<(), String> {
        self.fit(&mut config.clone(), bound).map(|_| ())
    }
}

/// Puts `value` at the JSON pointer `pointer` of `config`, making the objects on the way.
fn put(config: &mut Json, pointer: &str, value: u64) {
    let mut at = config;
    for segment in pointer.split('/').skip(1) {
        let name = segment.replace("~1", "/").replace("~0", "~");
        if !at.is_object() {
            *at = Json::Object(serde_json::Map::new());
        }
        let Some(object) = at.as_object_mut() else {
            return;
        };
        at = object.entry(name).or_insert(Json::Null);
    }
    *at = Json::from(value);
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
                default_ms: 1_000,
                default_setting: None,
            }],
        }
    }

    #[test]
    fn a_registry_default_is_carried_to_the_program_and_still_fits_the_interrupt_window() {
        let mut declared = declared();
        declared.delays[0].default_setting = Some("agents.native_result_wait_ms".into());
        declared.delays[0].default_ms = 3_000;
        let mut config = json!({});
        declared.fit(&mut config, Duration::from_secs(30)).unwrap();
        assert_eq!(config["child_usage_wait_ms"], 3_000);
        let mut config = json!({});
        declared
            .fit(&mut config, Duration::from_millis(8_500))
            .unwrap();
        assert_eq!(config["child_usage_wait_ms"], 500);
    }

    #[test]
    fn an_option_that_fits_under_the_bound_is_accepted_and_one_that_does_not_is_refused_naming_both()
     {
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
    fn a_value_set_is_kept_or_refused_and_never_shortened() {
        let bound = Duration::from_secs(30);
        let mut config = json!({"child_usage_wait_ms": 22_000, "other": 1});
        declared().fit(&mut config, bound).unwrap();
        assert_eq!(config, json!({"child_usage_wait_ms": 22_000, "other": 1}));
        let mut over = json!({"child_usage_wait_ms": 22_001});
        assert!(declared().fit(&mut over, bound).is_err());
        assert_eq!(
            over,
            json!({"child_usage_wait_ms": 22_001}),
            "left as it was"
        );
    }

    #[test]
    fn where_the_programs_own_wait_is_all_the_bound_gives_no_value_is_at_fault() {
        for bound in [Duration::from_secs(5), Duration::from_secs(8)] {
            for config in [
                json!({}),
                json!({"child_usage_wait_ms": 0}),
                json!({"child_usage_wait_ms": 1_000}),
                json!({"child_usage_wait_ms": 60_000}),
            ] {
                let mut fitted = config.clone();
                declared().fit(&mut fitted, bound).unwrap();
                assert_eq!(
                    fitted, config,
                    "bound {bound:?}: nothing is refused or changed"
                );
            }
        }
    }

    #[test]
    fn an_option_not_set_takes_its_default_or_less_where_the_time_leaves_less() {
        // 8.5 s leaves 500 ms after the program's own 8 s: the default of 1 000 ms is more than that.
        let mut config = json!({});
        let lowered = declared()
            .fit(&mut config, Duration::from_millis(8_500))
            .unwrap();
        assert_eq!(config, json!({"child_usage_wait_ms": 500}));
        assert_eq!(
            lowered,
            vec![Lowered {
                pointer: "/child_usage_wait_ms".to_owned(),
                default_ms: 1_000,
                to_ms: 500,
                bound_ms: 8_500,
            }],
            "what was lowered is told, so that the engine can say so"
        );
        // The number it comes to, written out, is accepted as it is.
        assert!(
            declared()
                .check(
                    &json!({"child_usage_wait_ms": 500}),
                    Duration::from_millis(8_500)
                )
                .is_ok()
        );
        // Where the default fits, nothing is put in.
        let mut config = json!({"other": true});
        declared()
            .fit(&mut config, Duration::from_secs(30))
            .unwrap();
        assert_eq!(config, json!({"other": true}));
        // A value that is not a number is read by the program as not set.
        let mut config = json!({"child_usage_wait_ms": "long"});
        declared()
            .fit(&mut config, Duration::from_millis(8_500))
            .unwrap();
        assert_eq!(config, json!({"child_usage_wait_ms": 500}));
        // A profile with no configuration at all is given an object.
        let mut config = Json::Null;
        declared()
            .fit(&mut config, Duration::from_millis(8_500))
            .unwrap();
        assert_eq!(config, json!({"child_usage_wait_ms": 500}));
    }

    #[test]
    fn a_declaration_of_another_version_or_one_that_cannot_be_read_declares_nothing() {
        let mut other = serde_json::to_value(declared()).unwrap();
        assert!(TurnEndDelays::read(&other).is_some());
        other["version"] = json!(2);
        assert!(TurnEndDelays::read(&other).is_none());
        assert!(TurnEndDelays::read(&json!("nonsense")).is_none());
        assert!(TurnEndDelays::read(&json!({"version": 1, "delays": 7})).is_none());
    }
}
