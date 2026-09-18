//! Per-worker circuit breakers: after `failure_threshold` consecutive
//! reachability failures a worker's circuit opens and calls are refused
//! fast — the gateway answers degraded immediately instead of waiting
//! out the 10s upstream timeout on a dead worker. Once the cooldown
//! elapses the circuit half-opens and admits exactly one probe: its
//! success closes the circuit, its failure re-opens it with a fresh
//! cooldown counted from that moment.
//!
//! State per key is a consecutive-failure count plus an open-until
//! deadline; "half-open" is not stored — it is an open circuit whose
//! deadline has passed. Entries are keyed by repo id and never evicted:
//! the registry bounds the key space, so growth stays bounded in
//! practice.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Per-key circuit state. `open_until == None` is closed; `Some` in the
/// future is open; `Some` in the past is half-open — the cooldown has
/// elapsed and the next `admit` claims the single probe permit.
#[derive(Debug, Default)]
struct Circuit {
    /// Consecutive reachability failures; reset by `succeed`.
    failures: u32,
    /// Open-until deadline set by the last threshold-crossing failure.
    open_until: Option<Instant>,
    /// A half-open probe is in flight; released by its outcome report.
    probing: bool,
}

impl Circuit {
    /// Admit decision for an existing circuit. Closed admits; open
    /// inside the cooldown refuses; half-open claims the probe permit
    /// for exactly one call — a second `admit` sees `probing` already
    /// set and is refused.
    fn admit(&mut self) -> bool {
        match self.open_until {
            None => true,
            Some(until) if Instant::now() < until => false,
            Some(_) if self.probing => false,
            Some(_) => {
                self.probing = true;
                true
            }
        }
    }

    /// The worker answered — resets the consecutive-failure count and
    /// closes the circuit, releasing the probe permit if one was held.
    const fn succeed(&mut self) {
        self.failures = 0;
        self.open_until = None;
        self.probing = false;
    }

    /// Record a reachability failure. Closed counts toward `threshold`;
    /// half-open or already-open re-opens with a fresh cooldown counted
    /// from now, releasing a held probe permit either way.
    fn fail(&mut self, threshold: u32, cooldown: Duration) {
        self.failures = self.failures.saturating_add(1);
        self.probing = false;
        if self.open_until.is_some() || self.failures >= threshold {
            self.open_until = Some(Instant::now() + cooldown);
        }
    }
}

/// Registry of per-worker circuits behind one mutex: the half-open
/// probe flag is set atomically under the lock so two racing admits
/// cannot both become the probe.
#[derive(Debug)]
pub(crate) struct Breakers {
    circuits: parking_lot::Mutex<HashMap<String, Circuit>>,
    failure_threshold: u32,
    cooldown: Duration,
}

impl Breakers {
    pub(crate) fn new(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            circuits: parking_lot::Mutex::new(HashMap::new()),
            failure_threshold,
            cooldown,
        }
    }

    /// `true` when a call to this worker may proceed now. Admits all
    /// calls while closed; none while open; exactly one concurrent
    /// probe while half-open — a second `admit` during a probe returns
    /// `false`.
    ///
    /// Every `true` obligates the caller to report exactly one outcome
    /// (`on_success` or `on_failure`) for that call. A dropped outcome
    /// strands the half-open permit: the flag stays set and every later
    /// `admit` keeps refusing — reporting is the permit-release
    /// mechanism, there is no probe timeout.
    pub(crate) fn admit(&self, key: &str) -> bool {
        self.circuits.lock().get_mut(key).is_none_or(Circuit::admit)
    }

    /// The worker answered with any reached HTTP response below 500 —
    /// resets the consecutive-failure count and closes the circuit. A
    /// late success from a call admitted before the circuit opened
    /// still closes it: the worker demonstrably answered.
    pub(crate) fn on_success(&self, key: &str) {
        if let Some(circuit) = self.circuits.lock().get_mut(key) {
            circuit.succeed();
        }
    }

    /// Timeout, connect error, other transport failure, or a reached
    /// HTTP 5xx. While closed it counts toward opening — the circuit
    /// opens when consecutive failures reach `failure_threshold` (N=1
    /// opens on the first failure). While half-open or already open
    /// (a straggler admitted earlier) it re-opens with a fresh cooldown
    /// counted from now.
    pub(crate) fn on_failure(&self, key: &str) {
        self.circuits
            .lock()
            .entry(key.to_owned())
            .or_default()
            .fail(self.failure_threshold, self.cooldown);
    }

    /// Current state name for metrics/debugging: `"closed"`, `"open"`,
    /// or `"half_open"` (cooldown elapsed, probe permitted).
    pub(crate) fn state_name(&self, key: &str) -> &'static str {
        let open_until = self
            .circuits
            .lock()
            .get(key)
            .map(|circuit| circuit.open_until);
        match open_until {
            None | Some(None) => "closed",
            Some(Some(until)) if Instant::now() < until => "open",
            Some(Some(_)) => "half_open",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COOLDOWN: Duration = Duration::from_millis(1);

    /// Sleep long enough for a 1ms cooldown to have elapsed.
    fn wait_cooldown() {
        std::thread::sleep(Duration::from_millis(2));
    }

    #[test]
    fn opens_exactly_at_threshold() {
        let breakers = Breakers::new(3, Duration::from_mins(1));
        breakers.on_failure("w");
        breakers.on_failure("w");
        assert!(breakers.admit("w")); // 2 < 3: still closed
        assert_eq!(breakers.state_name("w"), "closed");
        breakers.on_failure("w");
        assert_eq!(breakers.state_name("w"), "open");
    }

    #[test]
    fn threshold_one_opens_on_first_failure() {
        let breakers = Breakers::new(1, Duration::from_mins(1));
        breakers.on_failure("w");
        assert_eq!(breakers.state_name("w"), "open");
    }

    #[test]
    fn open_rejects_admits() {
        let breakers = Breakers::new(1, Duration::from_mins(1));
        breakers.on_failure("w");
        assert!(!breakers.admit("w"));
        assert!(!breakers.admit("w"));
    }

    #[test]
    fn cooldown_admits_one_probe() {
        let breakers = Breakers::new(1, COOLDOWN);
        breakers.on_failure("w");
        wait_cooldown();
        assert!(breakers.admit("w"));
        assert_eq!(breakers.state_name("w"), "half_open");
    }

    #[test]
    fn second_concurrent_probe_refused() {
        let breakers = Breakers::new(1, COOLDOWN);
        breakers.on_failure("w");
        wait_cooldown();
        assert!(breakers.admit("w"));
        assert!(!breakers.admit("w")); // probe permit already taken
    }

    #[test]
    fn probe_success_closes() {
        let breakers = Breakers::new(1, COOLDOWN);
        breakers.on_failure("w");
        wait_cooldown();
        assert!(breakers.admit("w"));
        breakers.on_success("w");
        assert_eq!(breakers.state_name("w"), "closed");
        assert!(breakers.admit("w")); // probe permit released
    }

    #[test]
    fn probe_failure_reopens_with_fresh_cooldown() {
        let breakers = Breakers::new(1, COOLDOWN);
        breakers.on_failure("w");
        wait_cooldown();
        assert!(breakers.admit("w"));
        breakers.on_failure("w");
        assert_eq!(breakers.state_name("w"), "open");
        assert!(!breakers.admit("w")); // fresh cooldown, not the spent one
        wait_cooldown();
        assert!(breakers.admit("w")); // next probe after the new cooldown
    }

    #[test]
    fn success_resets_consecutive_failures() {
        let breakers = Breakers::new(2, Duration::from_mins(1));
        breakers.on_failure("w");
        breakers.on_success("w");
        breakers.on_failure("w"); // 1 consecutive, not 2 — stays closed
        assert_eq!(breakers.state_name("w"), "closed");
        assert!(breakers.admit("w"));
        breakers.on_failure("w"); // now 2 consecutive — opens
        assert_eq!(breakers.state_name("w"), "open");
    }

    #[test]
    fn unknown_keys_are_closed() {
        let breakers = Breakers::new(1, COOLDOWN);
        assert!(breakers.admit("w"));
        assert_eq!(breakers.state_name("w"), "closed");
        breakers.on_success("w"); // no entry — harmless no-op
        assert_eq!(breakers.state_name("w"), "closed");
    }
}
