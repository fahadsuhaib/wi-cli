//! Auto-refresh of visible file previews. Pure logic only: the caller stats the visible files on
//! a slow timer (no watchers, nothing resident on a box) and feeds what it saw to a `Watch`, which
//! decides when the file has settled enough to read again.

use std::time::{Duration, Instant};

/// What a stat of the file showed: modification time (nanoseconds since the epoch) and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub mtime: i128,
    pub size: u64,
}

/// One look at the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Obs {
    Seen(Stamp),
    /// The file does not exist (any more).
    Gone,
    /// The stat itself failed or timed out: learn nothing.
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do.
    Idle,
    /// It changed and is not stable yet.
    Wait,
    /// Read it again now.
    Reload,
    /// The file just disappeared (reported once).
    Removed,
}

/// How often visible files are looked at.
pub fn poll_interval(remote: bool) -> Duration {
    Duration::from_secs(if remote { 2 } else { 1 })
}

/// A file that keeps changing is still refreshed at least this often.
pub fn cap_for(debounce: Duration) -> Duration {
    (debounce * 3).max(Duration::from_secs(3))
}

struct Pending {
    first: Instant,
    last_change: Instant,
    seen: Stamp,
}

pub struct Watch {
    debounce: Duration,
    cap: Duration,
    loaded: Option<Stamp>,
    pend: Option<Pending>,
    removed: bool,
}

impl Watch {
    pub fn new(debounce: Duration) -> Self {
        Watch { debounce, cap: cap_for(debounce), loaded: None, pend: None, removed: false }
    }

    pub fn observe(&mut self, now: Instant, obs: Obs) -> Step {
        let s = match obs {
            Obs::Failed => return Step::Idle,
            Obs::Gone => {
                self.pend = None;
                return if std::mem::replace(&mut self.removed, true) { Step::Idle } else { Step::Removed };
            }
            Obs::Seen(s) => s,
        };
        // A file that came back counts as changed against the content on screen.
        self.removed = false;
        let Some(loaded) = self.loaded else {
            self.loaded = Some(s);
            return Step::Idle;
        };
        if s == loaded {
            self.pend = None;
            return Step::Idle;
        }
        let p = self.pend.get_or_insert(Pending { first: now, last_change: now, seen: s });
        if p.seen != s {
            p.seen = s;
            p.last_change = now;
        }
        if now.duration_since(p.last_change) >= self.debounce || now.duration_since(p.first) >= self.cap {
            self.loaded = Some(s);
            self.pend = None;
            Step::Reload
        } else {
            Step::Wait
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(m: i128, n: u64) -> Obs {
        Obs::Seen(Stamp { mtime: m, size: n })
    }

    #[test]
    fn first_look_is_the_baseline() {
        let t0 = Instant::now();
        let mut w = Watch::new(Duration::from_millis(400));
        assert_eq!(w.observe(t0, st(1, 10)), Step::Idle);
        assert_eq!(w.observe(t0 + Duration::from_secs(1), st(1, 10)), Step::Idle);
    }

    #[test]
    fn change_reloads_after_the_debounce() {
        let t0 = Instant::now();
        let mut w = Watch::new(Duration::from_millis(400));
        w.observe(t0, st(1, 10));
        let t1 = t0 + Duration::from_secs(1);
        assert_eq!(w.observe(t1, st(2, 20)), Step::Wait);
        assert_eq!(w.observe(t1 + Duration::from_millis(399), st(2, 20)), Step::Wait);
        assert_eq!(w.observe(t1 + Duration::from_millis(400), st(2, 20)), Step::Reload);
        assert_eq!(w.observe(t1 + Duration::from_secs(2), st(2, 20)), Step::Idle);
    }

    #[test]
    fn further_changes_restart_the_wait_but_the_cap_wins() {
        let t0 = Instant::now();
        let mut w = Watch::new(Duration::from_secs(5));
        w.observe(t0, st(1, 1));
        let mut got = None;
        for i in 1..=20u64 {
            let r = w.observe(t0 + Duration::from_secs(i), st(1 + i as i128, 1 + i));
            if r == Step::Reload {
                got = Some(i);
                break;
            }
            assert_eq!(r, Step::Wait);
        }
        assert_eq!(got, Some(16), "constantly written file refreshes 3 x debounce after the first change");
    }

    #[test]
    fn change_that_reverts_cancels() {
        let t0 = Instant::now();
        let mut w = Watch::new(Duration::from_millis(400));
        w.observe(t0, st(1, 1));
        assert_eq!(w.observe(t0 + Duration::from_secs(1), st(2, 2)), Step::Wait);
        assert_eq!(w.observe(t0 + Duration::from_secs(2), st(1, 1)), Step::Idle);
    }

    #[test]
    fn removal_is_reported_once_and_return_reloads() {
        let t0 = Instant::now();
        let mut w = Watch::new(Duration::from_millis(0));
        w.observe(t0, st(1, 1));
        assert_eq!(w.observe(t0 + Duration::from_secs(1), Obs::Gone), Step::Removed);
        assert_eq!(w.observe(t0 + Duration::from_secs(2), Obs::Gone), Step::Idle);
        assert_eq!(w.observe(t0 + Duration::from_secs(3), Obs::Failed), Step::Idle);
        assert_eq!(w.observe(t0 + Duration::from_secs(4), st(5, 1)), Step::Reload);
    }

    #[test]
    fn cap_scales_with_debounce() {
        assert_eq!(cap_for(Duration::from_secs(5)), Duration::from_secs(15));
        assert_eq!(cap_for(Duration::from_millis(400)), Duration::from_secs(3));
    }
}
