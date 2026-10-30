//! Measured and assumed transfer estimates.
//!
//! How long a fork or a switch keeps a thread still is worked out from what has to travel and how fast the
//! machines at either end have been seen to send and receive: the files that go up from the source and down
//! to the target, the history if it travels as an archive, a fixed second or two for every thousand files,
//! the time to make an environment or to start a runtime, and ten seconds for the checks. A machine that has
//! not been measured yet is taken to move four mebibytes a second, and the estimate says it was assumed.
//! The range shown to a person is from seven tenths of the figure to one and a half times it.

use orch_api_contract::forks::{Estimate, EstimateBasis};

/// What a machine is taken to move each second until it has been measured: four mebibytes, a rate any
/// network between a person's machine and a host keeps up on a bad day, so that the first estimate errs
/// long rather than short.
pub const ASSUMED_RATE_BPS: i64 = 4 * 1024 * 1024;

/// The seconds that the bookkeeping of every thousand files takes, on top of their bytes.
pub const SECONDS_PER_THOUSAND_FILES: i64 = 2;

/// The seconds that making an environment takes when it has to be provisioned.
pub const PROVISION_SECONDS: i64 = 60;

/// The seconds that starting the runtime of a provider takes when only that has to be done.
pub const START_RUNTIME_SECONDS: i64 = 15;

/// The seconds the health checks of the last phase are allowed for in an estimate.
pub const CHECK_SECONDS: i64 = 10;

/// The part of the figure the short end of the range is, in tenths.
pub const LOW_TENTHS: i64 = 7;

/// The part of the figure the long end of the range is, in tenths.
pub const HIGH_TENTHS: i64 = 15;

/// How fast a machine moves bytes, and whether anybody measured it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    /// Bytes a second.
    pub bps: i64,
    /// Whether this is an average of what the machine did, and not the assumption.
    pub measured: bool,
}

impl Rate {
    /// The rate of a machine whose measurement is `measured`, or the assumed one when it has none or a
    /// useless one.
    #[must_use]
    pub const fn of(measured: Option<i64>) -> Self {
        match measured {
            Some(bps) if bps > 0 => Self {
                bps,
                measured: true,
            },
            _ => Self {
                bps: ASSUMED_RATE_BPS,
                measured: false,
            },
        }
    }
}

/// The two ends of a transfer: what the source sends and what the target receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rates {
    /// How fast the machine the files come from sends.
    pub up: Rate,
    /// How fast the machine they go to receives.
    pub down: Rate,
}

impl Rates {
    /// Rates for two machines that were not measured.
    #[must_use]
    pub const fn assumed() -> Self {
        Self {
            up: Rate::of(None),
            down: Rate::of(None),
        }
    }
}

/// What has to be made ready before anything travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Making {
    /// Nothing: the target is up.
    #[default]
    Nothing,
    /// The runtime of the target provider has to be started.
    StartRuntime,
    /// A target environment has to be provisioned.
    Environment,
}

/// What a change has to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Work {
    /// Bytes that go up from the source to the store.
    pub bytes_up: i64,
    /// Bytes that go down from the store to the target.
    pub bytes_down: i64,
    /// How many files there are.
    pub files: i64,
    /// The size of the archive a history travels as, if it does.
    pub archive_bytes: Option<i64>,
    /// What has to be made ready.
    pub making: Making,
}

fn seconds_for(bytes: i64, rate: Rate) -> i64 {
    let bytes = bytes.max(0);
    let rate = rate.bps.max(1);
    bytes / rate + i64::from(bytes % rate != 0)
}

/// How long the change takes, as a range, and whether the figures were measured.
#[must_use]
pub fn estimate(work: &Work, rates: &Rates) -> Estimate {
    let copy = seconds_for(work.bytes_up, rates.up)
        + seconds_for(work.bytes_down, rates.down)
        + work.files.max(0) / 1_000 * SECONDS_PER_THOUSAND_FILES;
    let archive = work.archive_bytes.unwrap_or(0);
    let history = seconds_for(archive, rates.up) + seconds_for(archive, rates.down);
    let provision = match work.making {
        Making::Nothing => 0,
        Making::StartRuntime => START_RUNTIME_SECONDS,
        Making::Environment => PROVISION_SECONDS,
    };
    let seconds = provision + copy + history + CHECK_SECONDS;
    let moves_bytes = work.bytes_up > 0 || work.bytes_down > 0 || archive > 0;
    let measured = !moves_bytes || (rates.up.measured && rates.down.measured);
    Estimate {
        seconds_low: seconds * LOW_TENTHS / 10,
        seconds_high: seconds * HIGH_TENTHS / 10,
        basis: if measured {
            EstimateBasis::Measured
        } else {
            EstimateBasis::Assumed
        },
    }
}

/// How long several changes take that run `parallel` at a time: the sum of their estimates divided by the
/// limit, and never less than the longest of them.
#[must_use]
pub fn together(each: &[Estimate], parallel: i64) -> Estimate {
    let parallel = parallel.max(1);
    let share = |sum: i64| sum / parallel + i64::from(sum % parallel != 0);
    let low = each.iter().map(|one| one.seconds_low).sum::<i64>();
    let high = each.iter().map(|one| one.seconds_high).sum::<i64>();
    Estimate {
        seconds_low: share(low).max(each.iter().map(|one| one.seconds_low).max().unwrap_or(0)),
        seconds_high: share(high).max(each.iter().map(|one| one.seconds_high).max().unwrap_or(0)),
        basis: if each.iter().all(|one| one.basis == EstimateBasis::Measured) {
            EstimateBasis::Measured
        } else {
            EstimateBasis::Assumed
        },
    }
}

/// One new environment precedes a batch's independent transfers; its provisioning is counted once.
#[must_use]
pub fn together_in_new_environment(each: &[Estimate], parallel: i64) -> Estimate {
    if each.is_empty() {
        return together(each, parallel);
    }
    let low = PROVISION_SECONDS * LOW_TENTHS / 10;
    let high = PROVISION_SECONDS * HIGH_TENTHS / 10;
    let work: Vec<Estimate> = each
        .iter()
        .map(|one| Estimate {
            seconds_low: one.seconds_low.saturating_sub(low),
            seconds_high: one.seconds_high.saturating_sub(high),
            basis: one.basis,
        })
        .collect();
    let mut estimate = together(&work, parallel);
    estimate.seconds_low = estimate.seconds_low.saturating_add(low);
    estimate.seconds_high = estimate.seconds_high.saturating_add(high);
    estimate
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEBIBYTE: i64 = 1024 * 1024;

    #[test]
    fn a_machine_that_was_never_measured_is_taken_to_move_four_mebibytes_a_second() {
        let work = Work {
            bytes_up: 40 * MEBIBYTE,
            bytes_down: 40 * MEBIBYTE,
            files: 2_500,
            ..Work::default()
        };
        let range = estimate(&work, &Rates::assumed());
        // 10 s up, 10 s down, 2 s for each full thousand of files, 10 s of checks.
        let seconds = 10 + 10 + 4 + 10;
        assert_eq!(range.seconds_low, seconds * 7 / 10);
        assert_eq!(range.seconds_high, seconds * 15 / 10);
        assert_eq!(range.basis, EstimateBasis::Assumed);
    }

    #[test]
    fn measured_rates_make_a_measured_estimate_and_a_change_that_moves_nothing_needs_none() {
        let fast = Rate::of(Some(10 * MEBIBYTE));
        let rates = Rates {
            up: fast,
            down: fast,
        };
        let work = Work {
            bytes_up: 100 * MEBIBYTE,
            bytes_down: 100 * MEBIBYTE,
            ..Work::default()
        };
        let range = estimate(&work, &rates);
        assert_eq!(range.basis, EstimateBasis::Measured);
        assert_eq!(range.seconds_low, (10 + 10 + 10) * 7 / 10);
        let nothing = estimate(&Work::default(), &Rates::assumed());
        assert_eq!(
            nothing.basis,
            EstimateBasis::Measured,
            "nothing that has to be moved rests on no guess"
        );
        assert_eq!(nothing.seconds_low, 10 * 7 / 10);
    }

    #[test]
    fn making_an_environment_costs_a_minute_and_starting_a_runtime_a_quarter_of_that() {
        let environment = estimate(
            &Work {
                making: Making::Environment,
                ..Work::default()
            },
            &Rates::assumed(),
        );
        let runtime = estimate(
            &Work {
                making: Making::StartRuntime,
                ..Work::default()
            },
            &Rates::assumed(),
        );
        assert_eq!(environment.seconds_high, (60 + 10) * 15 / 10);
        assert_eq!(runtime.seconds_high, (15 + 10) * 15 / 10);
    }

    #[test]
    fn three_changes_share_one_provisioning_before_their_parallel_or_serial_work() {
        let one = estimate(
            &Work {
                making: Making::Environment,
                ..Work::default()
            },
            &Rates::assumed(),
        );
        let serial = together_in_new_environment(&[one.clone(), one.clone(), one.clone()], 1);
        let parallel = together_in_new_environment(&[one.clone(), one.clone(), one], 4);
        assert_eq!(serial.seconds_high, (60 + 3 * 10) * HIGH_TENTHS / 10);
        assert_eq!(parallel.seconds_high, (60 + 10) * HIGH_TENTHS / 10);
    }

    #[test]
    fn several_changes_in_parallel_take_their_sum_over_the_limit_but_never_less_than_the_longest() {
        let one = |seconds: i64| Estimate {
            seconds_low: seconds,
            seconds_high: seconds * 2,
            basis: EstimateBasis::Measured,
        };
        let batch = together(&[one(10), one(10), one(10), one(10)], 4);
        assert_eq!((batch.seconds_low, batch.seconds_high), (10, 20));
        let uneven = together(&[one(100), one(5), one(5)], 4);
        assert_eq!(
            uneven.seconds_low, 100,
            "the sum over the limit is shorter than the longest one"
        );
        let two_at_a_time = together(&[one(10), one(10), one(10)], 2);
        assert_eq!(two_at_a_time.seconds_low, 15);
        let guessed = together(
            &[
                one(10),
                Estimate {
                    basis: EstimateBasis::Assumed,
                    ..one(10)
                },
            ],
            4,
        );
        assert_eq!(guessed.basis, EstimateBasis::Assumed);
    }
}
