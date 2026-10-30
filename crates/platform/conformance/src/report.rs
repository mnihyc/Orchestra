//! One result for each case of a suite, in the words an operator reads.
//!
//! A suite checks one implementation against the rules of its contract, one **case** for each rule. A case ends
//! in one of three ways: it **passed**; it **failed**, with why in a sentence; or it was **skipped**, with why,
//! because the rule asks about something the implementation does not declare (a summarizer that writes no tags
//! is not asked for tags). A skip is no failure, and the report says it as a skip, never as a pass.
//!
//! A [`Report`] holds the cases of one implementation in the order they ran. It prints as the lines an operator
//! reads after `orchctl plugin check`, a rule a line, and [`Report::passed`] is what the command's exit status
//! follows: one failed case is enough to fail it. A rule's name (`transport.ping`) stays the same from run to
//! run and from version to version, so that a failure can be looked up and compared.

use std::fmt;
use std::future::Future;
use std::time::Duration;

/// How one case came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The implementation did what the rule asks.
    Passed,
    /// It did not: why, in a sentence.
    Failed(String),
    /// The rule does not apply to it: why, in a sentence. No failure.
    Skipped(String),
}

impl Outcome {
    /// Whether the case failed.
    #[must_use]
    pub const fn is_failure(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

/// Why a case did not pass, as a check returns it: a broken rule, or a rule that does not apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Miss {
    /// The implementation broke the rule: why.
    Failed(String),
    /// The rule does not apply to it: why.
    Skipped(String),
}

impl From<String> for Miss {
    fn from(why: String) -> Self {
        Self::Failed(why)
    }
}

impl From<&str> for Miss {
    fn from(why: &str) -> Self {
        Self::Failed(why.to_owned())
    }
}

/// What a check returns: `Ok` when the rule held.
pub type Checked = Result<(), Miss>;

/// A rule that does not apply to the implementation, for a check to return.
#[must_use]
pub fn skip(why: impl Into<String>) -> Miss {
    Miss::Skipped(why.into())
}

/// One case and how it came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    /// The rule's name, the same in every run and every version of the suite: `transport.ping`.
    pub rule: String,
    /// What the rule asks, in a sentence.
    pub title: String,
    /// How it came out.
    pub outcome: Outcome,
}

/// What a suite found about one implementation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// What was checked, as the operator knows it: `notifier "file" 0.1.0`.
    pub subject: String,
    /// Every case, in the order it ran.
    pub cases: Vec<Case>,
}

impl Report {
    /// An empty report about `subject`.
    #[must_use]
    pub fn new(subject: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            cases: Vec::new(),
        }
    }

    /// Writes down how the case `rule` came out.
    pub fn record(&mut self, rule: &str, title: &str, outcome: Outcome) {
        self.cases.push(Case {
            rule: rule.to_owned(),
            title: title.to_owned(),
            outcome,
        });
    }

    /// Writes down what a check returned for the case `rule`.
    pub fn checked(&mut self, rule: &str, title: &str, checked: Checked) {
        let outcome = match checked {
            Ok(()) => Outcome::Passed,
            Err(Miss::Failed(why)) => Outcome::Failed(why),
            Err(Miss::Skipped(why)) => Outcome::Skipped(why),
        };
        self.record(rule, title, outcome);
    }

    /// Runs `check` as the case `rule`, and fails it when it has not come to an end within `within`: an
    /// implementation that does not answer breaks the rule it was asked about.
    pub async fn run<F>(&mut self, rule: &str, title: &str, within: Duration, check: F)
    where
        F: Future<Output = Checked>,
    {
        let checked = tokio::time::timeout(within, check)
            .await
            .unwrap_or_else(|_| Err(Miss::Failed(format!("no answer within {within:?}"))));
        self.checked(rule, title, checked);
    }

    /// Adds the cases of another report about the same implementation, after its own.
    pub fn extend(&mut self, other: Self) {
        self.cases.extend(other.cases);
    }

    /// Whether no case failed. Skips do not count against it.
    #[must_use]
    pub fn passed(&self) -> bool {
        !self.cases.iter().any(|case| case.outcome.is_failure())
    }

    /// The cases that failed, in the order they ran.
    pub fn failures(&self) -> impl Iterator<Item = &Case> {
        self.cases.iter().filter(|case| case.outcome.is_failure())
    }

    /// The outcome of the case `rule`, where it ran.
    #[must_use]
    pub fn outcome(&self, rule: &str) -> Option<&Outcome> {
        self.cases
            .iter()
            .find(|case| case.rule == rule)
            .map(|case| &case.outcome)
    }

    /// How many cases passed, failed and were skipped.
    #[must_use]
    pub fn counts(&self) -> (usize, usize, usize) {
        self.cases
            .iter()
            .fold((0, 0, 0), |(passed, failed, skipped), case| {
                match case.outcome {
                    Outcome::Passed => (passed + 1, failed, skipped),
                    Outcome::Failed(_) => (passed, failed + 1, skipped),
                    Outcome::Skipped(_) => (passed, failed, skipped + 1),
                }
            })
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.subject)?;
        let width = self
            .cases
            .iter()
            .map(|case| case.rule.len())
            .max()
            .unwrap_or(0);
        for case in &self.cases {
            let (word, why) = match &case.outcome {
                Outcome::Passed => ("passed ", None),
                Outcome::Failed(why) => ("FAILED ", Some(why)),
                Outcome::Skipped(why) => ("skipped", Some(why)),
            };
            write!(f, "  {word}  {:<width$}  {}", case.rule, case.title)?;
            match why {
                Some(why) => writeln!(f, ": {why}")?,
                None => writeln!(f)?,
            }
        }
        let (passed, failed, skipped) = self.counts();
        write!(f, "{passed} passed, {failed} failed, {skipped} skipped")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_failed_case_fails_the_report_and_a_skip_does_not() {
        let mut report = Report::new("summarizer \"rules\" 0.1.0");
        report
            .run("a.passes", "It passes", Duration::from_secs(1), async {
                Ok(())
            })
            .await;
        report
            .run("b.skips", "It is skipped", Duration::from_secs(1), async {
                Err(skip("it declares nothing of the kind"))
            })
            .await;
        assert!(report.passed(), "a skip is no failure");
        report
            .run("c.fails", "It fails", Duration::from_secs(1), async {
                Err("the answer was empty".into())
            })
            .await;
        assert!(!report.passed());
        assert_eq!(report.counts(), (1, 1, 1));
        assert_eq!(
            report
                .failures()
                .map(|case| case.rule.as_str())
                .collect::<Vec<_>>(),
            ["c.fails"]
        );
        let printed = report.to_string();
        assert!(
            printed.contains("FAILED   c.fails   It fails: the answer was empty"),
            "{printed}"
        );
        assert!(
            printed.ends_with("1 passed, 1 failed, 1 skipped"),
            "{printed}"
        );
    }

    #[tokio::test]
    async fn a_case_that_does_not_answer_in_time_fails_and_says_so() {
        let mut report = Report::new("driver \"slow\"");
        report
            .run("x.hangs", "It answers", Duration::from_millis(20), async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok(())
            })
            .await;
        assert_eq!(
            report.outcome("x.hangs"),
            Some(&Outcome::Failed("no answer within 20ms".to_owned()))
        );
    }
}
