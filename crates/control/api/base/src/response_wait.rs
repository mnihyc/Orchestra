//! A caller's response budget, separate from work that continues after it leaves.
use std::future::Future;
use std::time::Duration;

/// Wait for an answer within the caller's budget. A late answer remains owned
/// by its producer; this helper drops only the waiting future.
pub async fn wait<F: Future>(budget: Duration, work: F) -> Option<F::Output> {
    #[cfg(any(test, feature = "test-support"))]
    if let Ok(clock) = RESPONSE_CLOCK.try_with(Clone::clone) {
        return tokio::select! {
            biased;
            value = work => Some(value),
            () = clock.sleep(budget) => None,
        };
    }
    tokio::time::timeout(budget, work).await.ok()
}

#[cfg(any(test, feature = "test-support"))]
tokio::task_local! {static RESPONSE_CLOCK: testing::ManualResponseClock;}

/// A response-only clock for cases that keep real database and machine IO live.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::*;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use tokio::sync::Notify;

    #[derive(Debug, Default)]
    struct State {
        elapsed: Duration,
        budgets: Vec<Duration>,
    }

    #[derive(Debug, Default)]
    struct Inner {
        state: Mutex<State>,
        changed: Notify,
    }

    /// A monotonic response clock that moves only when the case advances it.
    /// Its scope follows one HTTP future and is not inherited by spawned work.
    #[derive(Debug, Clone, Default)]
    pub struct ManualResponseClock(Arc<Inner>);

    impl ManualResponseClock {
        fn state(&self) -> MutexGuard<'_, State> {
            self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
        }

        /// Time advanced by the case, independently of runner or IO latency.
        #[must_use]
        pub fn elapsed(&self) -> Duration {
            self.state().elapsed
        }

        /// The original budget of the last response wait that actually began.
        #[must_use]
        pub fn scheduled(&self) -> Option<Duration> {
            self.state().budgets.last().copied()
        }

        /// Advances this response clock and wakes waits whose deadline passed.
        pub fn advance(&self, by: Duration) {
            let mut state = self.state();
            state.elapsed = state.elapsed.saturating_add(by);
            drop(state);
            self.0.changed.notify_waiters();
        }

        pub(super) async fn sleep(&self, budget: Duration) {
            let deadline = {
                let mut state = self.state();
                state.budgets.push(budget);
                state.elapsed.saturating_add(budget)
            };
            loop {
                let changed = self.0.changed.notified();
                tokio::pin!(changed);
                let _ = changed.as_mut().enable();
                if self.elapsed() >= deadline {
                    return;
                }
                changed.await;
            }
        }
    }

    /// Holds only response deadlines while this caller is polled; background
    /// work retains its ordinary runtime clock and original native deadlines.
    pub async fn with_response_clock<F: Future>(clock: &ManualResponseClock, work: F) -> F::Output {
        RESPONSE_CLOCK.scope(clock.clone(), work).await
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{ManualResponseClock, with_response_clock};
    use super::*;
    use std::task::Poll;

    #[tokio::test]
    async fn a_response_clock_keeps_its_budget_while_background_timers_keep_running() {
        let clock = ManualResponseClock::default();
        let budget = Duration::from_secs(2);
        let response = with_response_clock(&clock, wait(budget, std::future::pending::<()>()));
        tokio::pin!(response);
        std::future::poll_fn(|cx| {
            assert!(response.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(clock.scheduled(), Some(budget));
        // The response scope must not freeze the producer's runtime clock.
        with_response_clock(&clock, async {
            let native = tokio::spawn(wait(Duration::from_millis(1), std::future::pending::<()>()));
            let result = tokio::time::timeout(budget * 2, native)
                .await
                .expect("the independent native clock progresses")
                .expect("the native timer completes");
            assert_eq!(result, None);
        })
        .await;
        assert_eq!(clock.scheduled(), Some(budget));
        clock.advance(Duration::from_secs(1));
        std::future::poll_fn(|cx| {
            assert!(response.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        clock.advance(Duration::from_secs(1));
        assert_eq!(response.await, None);
        assert_eq!(clock.elapsed(), budget);
    }
}
