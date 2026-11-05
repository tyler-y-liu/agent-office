//! When an agent's one-line task is re-summarized: the prompt or the running
//! todo changed, or enough tool calls passed that the work may have drifted.
//! Debounced so a burst of edits costs one request, and single-flight per
//! agent so a slow reply can't be overtaken by a stale one.

use std::time::{Duration, Instant};

pub const DEBOUNCE: Duration = Duration::from_secs(3);
pub const TOOLS_PER_REFRESH: u32 = 10;
/// A failed request is tried again after this, so a gateway blip does not
/// leave an idle agent unsummarized until its next prompt.
pub const RETRY_AFTER: Duration = Duration::from_secs(30);
/// The longest wait between retries: each failure in a row doubles it up to
/// this, so a gateway that stays down is not asked twice a minute per agent.
pub const RETRY_MAX: Duration = Duration::from_secs(8 * 60);

/// How long to wait after `failures` failed requests in a row.
pub fn retry_after(failures: u32) -> Duration {
    RETRY_AFTER
        .saturating_mul(1 << failures.saturating_sub(1).min(5))
        .min(RETRY_MAX)
}

#[derive(Debug, Default)]
pub struct Trigger {
    basis: Option<String>,
    pending_since: Option<Instant>,
    in_flight: bool,
    tools_at_last: u32,
    /// Requests failed in a row.
    failures: u32,
}

impl Trigger {
    /// Note what the task rests on now (`basis`) and the tool count so far.
    pub fn observe(&mut self, basis: &str, tool_calls: u32, now: Instant) {
        if self.basis.as_deref() != Some(basis) {
            self.basis = Some(basis.to_owned());
            // A newer prompt restarts the wait: summarize the settled one.
            self.pending_since = Some(now);
        } else if self.pending_since.is_none()
            && tool_calls.saturating_sub(self.tools_at_last) >= TOOLS_PER_REFRESH
        {
            self.pending_since = Some(now);
        }
    }

    /// The user asked for a fresh summary.
    pub fn force(&mut self, now: Instant) {
        self.pending_since = Some(now - DEBOUNCE);
    }

    /// Whether to send a request now; if so the trigger counts it in flight.
    pub fn take_due(&mut self, tool_calls: u32, now: Instant) -> bool {
        let due = !self.in_flight
            && self
                .pending_since
                .is_some_and(|t| now.duration_since(t) >= DEBOUNCE);
        if due {
            self.in_flight = true;
            self.pending_since = None;
            self.tools_at_last = tool_calls;
        }
        due
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight
    }

    pub fn finished(&mut self) {
        self.in_flight = false;
        self.failures = 0;
    }

    /// The request failed: finished, and due again after [`retry_after`]
    /// unless something newer arms it first.
    pub fn failed(&mut self, now: Instant) {
        self.in_flight = false;
        self.failures += 1;
        if self.pending_since.is_none() {
            self.pending_since = Some(now + retry_after(self.failures) - DEBOUNCE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_of_prompt_changes_costs_one_request_after_it_settles() {
        let t0 = Instant::now();
        let mut tr = Trigger::default();
        tr.observe("a", 0, t0);
        tr.observe("ab", 0, t0 + Duration::from_secs(1));
        assert!(
            !tr.take_due(0, t0 + Duration::from_secs(3)),
            "still settling"
        );
        assert!(tr.take_due(0, t0 + Duration::from_secs(4)));
        assert!(!tr.take_due(0, t0 + Duration::from_secs(10)), "nothing new");
    }

    #[test]
    fn twenty_tool_calls_on_one_prompt_refresh_twice() {
        let t0 = Instant::now();
        let mut tr = Trigger::default();
        tr.observe("p", 0, t0);
        assert!(tr.take_due(0, t0 + DEBOUNCE));
        tr.finished();
        let mut fired = 0;
        for i in 1..=20u32 {
            let now = t0 + DEBOUNCE + Duration::from_secs(u64::from(i) * 4);
            tr.observe("p", i, now);
            if tr.take_due(i, now + DEBOUNCE) {
                fired += 1;
                tr.finished();
            }
        }
        assert_eq!(fired, 2);
    }

    #[test]
    fn a_failed_request_is_retried_later_not_forgotten() {
        let t0 = Instant::now();
        let mut tr = Trigger::default();
        tr.observe("p", 0, t0);
        assert!(tr.take_due(0, t0 + DEBOUNCE));
        tr.failed(t0 + DEBOUNCE);
        tr.observe("p", 0, t0 + DEBOUNCE);
        assert!(
            !tr.take_due(0, t0 + DEBOUNCE * 2),
            "no hammering a failing gateway"
        );
        assert!(tr.take_due(0, t0 + DEBOUNCE + RETRY_AFTER));
    }

    /// A gateway that stays down is asked less and less often, but still
    /// asked: it may come back.
    #[test]
    fn failures_in_a_row_back_off_up_to_a_ceiling() {
        assert_eq!(retry_after(1), RETRY_AFTER);
        assert_eq!(retry_after(2), RETRY_AFTER * 2);
        assert_eq!(retry_after(3), RETRY_AFTER * 4);
        assert_eq!(retry_after(50), RETRY_MAX);
        let t0 = Instant::now();
        let mut tr = Trigger::default();
        tr.observe("p", 0, t0);
        assert!(tr.take_due(0, t0 + DEBOUNCE));
        tr.failed(t0 + DEBOUNCE);
        let t1 = t0 + DEBOUNCE + RETRY_AFTER;
        assert!(tr.take_due(0, t1));
        tr.failed(t1);
        assert!(
            !tr.take_due(0, t1 + RETRY_AFTER),
            "the second wait is longer"
        );
        assert!(tr.take_due(0, t1 + RETRY_AFTER * 2));
        tr.finished();
        tr.observe("q", 0, t1 + RETRY_AFTER * 2);
        let t2 = t1 + RETRY_AFTER * 2 + DEBOUNCE;
        assert!(tr.take_due(0, t2));
        tr.failed(t2);
        assert!(
            tr.take_due(0, t2 + RETRY_AFTER),
            "a success starts the count over"
        );
    }

    #[test]
    fn single_flight_holds_a_new_prompt_until_the_reply_lands() {
        let t0 = Instant::now();
        let mut tr = Trigger::default();
        tr.observe("one", 0, t0);
        assert!(tr.take_due(0, t0 + DEBOUNCE));
        tr.observe("two", 0, t0 + DEBOUNCE);
        let later = t0 + DEBOUNCE * 3;
        assert!(!tr.take_due(0, later), "one request per agent in flight");
        tr.finished();
        assert!(
            tr.take_due(0, later),
            "the newer prompt still gets its turn"
        );
    }
}
