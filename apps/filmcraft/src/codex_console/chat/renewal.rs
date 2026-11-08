//! A bounded, monotonic voice-only renewal policy. No connection or edit is replayed here.
use std::time::Duration;

pub(super) const WARN_AFTER: Duration = Duration::from_secs(54 * 60);
pub(super) const RENEW_AFTER: Duration = Duration::from_secs(55 * 60);
const TRANSITION_LIMIT: Duration = Duration::from_secs(65);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Warn,
    Stop,
    Restart,
    Renewed,
}
#[derive(Clone, Copy, Debug, Default)]
enum Phase {
    #[default]
    Watching,
    Waiting(Duration),
    Restarting(Duration),
}
#[derive(Default)]
pub(super) struct Renewal {
    phase: Phase,
    warned: bool,
}
impl Renewal {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn pending(&self) -> bool {
        !matches!(self.phase, Phase::Watching)
    }
    pub fn step(
        &mut self,
        now: Duration,
        age: Option<Duration>,
        active: bool,
        stopped: bool,
        failed: bool,
        busy: bool,
    ) -> Result<Option<Action>, &'static str> {
        match self.phase {
            Phase::Watching => {
                if !active {
                    return Ok(None);
                }
                let Some(age) = age else { return Ok(None) };
                if !self.warned && age >= WARN_AFTER {
                    self.warned = true;
                    return Ok(Some(Action::Warn));
                }
                if age >= RENEW_AFTER {
                    self.phase = Phase::Waiting(now);
                    return Ok(Some(Action::Stop));
                }
            }
            Phase::Waiting(since) => {
                if failed || now.saturating_sub(since) >= TRANSITION_LIMIT {
                    if stopped && busy && !failed {
                        return Err("Voice renewal is waiting for the current text task. Let it finish, then start voice again. No task or edit was replayed.");
                    }
                    return Err(
                        "Voice renewal could not confirm shutdown. Reconnect Codex and inspect the project before repeating edits; the saved conversation is preserved.",
                    );
                }
                if stopped && !busy {
                    self.phase = Phase::Restarting(now);
                    return Ok(Some(Action::Restart));
                }
            }
            Phase::Restarting(since) => {
                if failed || now.saturating_sub(since) >= TRANSITION_LIMIT {
                    return Err("Voice renewal did not reconnect. Turn voice off and retry; the saved conversation and project are preserved.");
                }
                if active {
                    self.reset();
                    return Ok(Some(Action::Renewed));
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seconds(n: u64) -> Duration {
        Duration::from_secs(n)
    }
    #[test]
    fn warns_then_stops_once_and_waits_for_real_closure_and_task_completion() {
        let mut r = Renewal::default();
        assert_eq!(r.step(seconds(1), Some(seconds(3239)), true, false, false, false).unwrap(), None);
        assert_eq!(r.step(seconds(2), Some(WARN_AFTER), true, false, false, false).unwrap(), Some(Action::Warn));
        assert_eq!(r.step(seconds(3), Some(WARN_AFTER), true, false, false, false).unwrap(), None);
        assert_eq!(r.step(seconds(60), Some(RENEW_AFTER), true, false, false, true).unwrap(), Some(Action::Stop));
        assert!(r.pending());
        assert_eq!(r.step(seconds(61), None, false, false, false, false).unwrap(), None, "an acknowledgement without closure is insufficient");
        assert_eq!(r.step(seconds(62), None, false, true, false, true).unwrap(), None, "a pending task must not be declared finished");
        assert_eq!(r.step(seconds(63), None, false, true, false, false).unwrap(), Some(Action::Restart));
        assert_eq!(r.step(seconds(64), Some(seconds(1)), false, false, false, false).unwrap(), None);
        assert_eq!(r.step(seconds(65), Some(seconds(2)), true, false, false, false).unwrap(), Some(Action::Renewed));
        assert!(!r.pending());
    }
    #[test]
    fn stop_or_disconnect_cancels_the_intent_and_missing_closure_is_bounded() {
        let mut r = Renewal::default();
        r.step(seconds(0), Some(RENEW_AFTER), true, false, false, false).unwrap();
        assert_eq!(r.step(seconds(1), Some(RENEW_AFTER), true, false, false, false).unwrap(), Some(Action::Stop));
        assert!(r.step(seconds(66), None, false, false, false, false).is_err());
        r.reset();
        assert!(!r.pending());
        assert_eq!(r.step(seconds(67), None, false, true, false, false).unwrap(), None);
    }
    #[test]
    fn failure_never_restarts_and_a_backwards_clock_does_not_expire_the_wait() {
        let mut r = Renewal::default();
        r.step(seconds(100), Some(RENEW_AFTER), true, false, false, false).unwrap();
        r.step(seconds(101), Some(RENEW_AFTER), true, false, false, false).unwrap();
        assert_eq!(r.step(seconds(0), None, false, false, false, false).unwrap(), None);
        assert!(r.step(seconds(102), None, false, true, true, false).is_err());
    }
    #[test]
    fn long_text_task_is_not_misreported_as_unconfirmed_shutdown() {
        let mut r = Renewal::default();
        r.step(seconds(0), Some(RENEW_AFTER), true, false, false, true).unwrap();
        r.step(seconds(1), Some(RENEW_AFTER), true, false, false, true).unwrap();
        let error = r.step(seconds(66), None, false, true, false, true).unwrap_err();
        assert!(error.contains("current text task") && !error.contains("Reconnect Codex"));
        assert!(r.pending(), "The caller must explicitly cancel the renewal intent after an error");
    }
}
