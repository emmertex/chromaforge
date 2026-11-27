//! Adaptive per-volume limits (§5.7 of the I/O spec, PR 5): each volume's limit on running jobs
//! is learned from its own throughput and the process's, one volume probing at a time, each probe
//! judged against fresh baselines (A-B-A).
//!
//! The rule, translated from the spec's simulator (`io_controller_sim.py`, class `Controller`, the
//! `floor` up rule with no repeats; read as a reference, structure kept so the two can be compared
//! line by line):
//! - a volume starts at today's limit (the calls' `workers()`), and follows it until its first
//!   measurement ([`Controller::offer_start`]);
//! - a **window** runs only while the volume is at its limit ([`Demand::AtLimit`]); it ends after
//!   at least [`WINDOW_MIN`] and either `max(8, 2 × limit)` finished jobs or [`WINDOW_LONG`] with
//!   bytes moving; its rates are the volume's bytes and the process's total bytes per second; a
//!   window whose jobs spent more than [`GATE_SHARE`] of their time at the work gate is discarded;
//! - **one token**: one volume probes at a time; it is taken before the first baseline;
//! - a **probe** is a baseline at the current limit, a window at the probe limit, a second
//!   baseline at the stored limit; up (`L + ceil(L/4)`, at most the cap) is kept when the probe
//!   beats the baselines' mean by [`UP_GAIN`], is not below either baseline, and the process total
//!   rose by at least [`TOTAL_SHARE`] of the volume's own gain; down (`L − max(1, L/2)`), tried
//!   after a failed up, is kept when the total falls by no more than [`DOWN_TOTAL`] and the volume
//!   keeps `1 − `[`DOWN_OWN`] of its rate; a kept step probes the same way again;
//! - a probe **aborts** (stored limit back, token released) when the volume runs out of work, a
//!   window is discarded, or a window can't complete within `max(90 s, 3 × the last window)`;
//! - **hold** after a failed down or an abort: [`HOLD_FIRST`] windows, doubling to [`HOLD_MAX`];
//!   a kept change resets it;
//! - a volume with no jobs for [`FORGET`] is forgotten (a different card at the same mount path
//!   relearns).
//!
//! Pure: the scheduler (or the simulator in the tests) calls [`Controller::tick`] for each volume
//! with the time, whether it is at its limit, and its cumulative counters. Times are seconds.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::collections::HashMap;
use std::hash::Hash;

/// Running jobs in the process (§5.7 "I/O cap"; R4a holds a busy volume to one fewer while
/// another volume is busy).
pub(crate) const IO_CAP: usize = 32;
/// A window lasts at least this long (s).
pub(crate) const WINDOW_MIN: f64 = 4.0;
/// ... and until this many jobs finished (or twice the limit, if more) ...
pub(crate) const WINDOW_JOBS: u64 = 8;
/// ... or this long with bytes moving (s).
pub(crate) const WINDOW_LONG: f64 = 60.0;
/// An up-probe must beat the mean of its baselines by this share.
pub(crate) const UP_GAIN: f64 = 0.15;
/// ... and the process total must rise by at least this share of the volume's own gain.
pub(crate) const TOTAL_SHARE: f64 = 0.5;
/// A down-probe is kept when the process total falls by no more than this share ...
pub(crate) const DOWN_TOTAL: f64 = 0.05;
/// ... and the volume loses no more than this share of its own rate.
pub(crate) const DOWN_OWN: f64 = 0.25;
/// Windows held after a failed down-probe or an abort, the first time ...
pub(crate) const HOLD_FIRST: u32 = 8;
/// ... doubling each time, to at most this.
pub(crate) const HOLD_MAX: u32 = 16;
/// A probing volume's window that hasn't ended this long after it started (or
/// [`ABORT_FACTOR`] × the last window, if longer) ends the probe (s).
pub(crate) const ABORT_MIN: f64 = 90.0;
pub(crate) const ABORT_FACTOR: f64 = 3.0;
/// A window whose jobs spent more than this share of their time at the work gate is discarded.
pub(crate) const GATE_SHARE: f64 = 0.2;
/// A volume with no jobs for this long is forgotten (s).
pub(crate) const FORGET: f64 = 600.0;
/// The "last window" before any (the simulator's; only the abort deadline reads it, where
/// [`ABORT_MIN`] is larger anyway).
const FIRST_LAST_WINDOW: f64 = 10.0;

/// Where a volume stands against its limit at a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Demand {
    /// At its limit: jobs queued, or exactly its limit running. Windows run.
    AtLimit,
    /// More running than its limit (it was lowered): windows wait until it drains.
    Draining,
    /// Demand-limited: nothing queued and fewer running than its limit (or nothing it can be
    /// measured on). A probe ends.
    Limited,
}

/// A volume's cumulative counters (only ever growing).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Counters {
    /// Bytes its measured jobs moved (MB in the simulator).
    pub(crate) bytes: f64,
    /// Its measured jobs finished.
    pub(crate) jobs: u64,
    /// Seconds its measured jobs ran (summed over jobs).
    pub(crate) busy: f64,
    /// Seconds of that spent waiting at the work gate.
    pub(crate) gate: f64,
}

/// Where a volume is in its cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Measuring a baseline at its limit (first of a probe; it takes the token first).
    Base1,
    /// Measuring at the probe limit.
    Probe,
    /// Measuring the second baseline, back at the stored limit.
    Base2,
    /// Counting windows before probing up again.
    Hold,
}

/// Why a probe ended without a decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AbortReason {
    /// The volume ran out of work (its call ended, its queue ran dry).
    Demand,
    /// A window couldn't complete in time.
    Deadline,
    /// A window was discarded (gate waits).
    Discarded,
}

/// What the controller did, for the profile output and the tests.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event<K> {
    /// A window completed: the phase it measured, at `limit`, its rates (bytes/s) over `span` s.
    Window { vol: K, phase: Phase, limit: usize, rate: f64, total: f64, span: f64, jobs: u64 },
    /// A window was discarded: `gate` of its jobs' time was spent at the work gate.
    Discarded { vol: K, limit: usize, gate: f64 },
    /// A probe of `to` began from `from`.
    Probe { vol: K, from: usize, to: usize },
    /// A probe was judged: `kept` moves the limit `from` → `to`.
    Decided { vol: K, up: bool, from: usize, to: usize, kept: bool, base: f64, probe: f64, base_total: f64, probe_total: f64 },
    /// A probe ended without a decision; `limit` is restored.
    Aborted { vol: K, reason: AbortReason, limit: usize },
    /// It holds for `windows` windows at `limit`.
    Hold { vol: K, limit: usize, windows: u32 },
    /// Forgotten after [`FORGET`] with no jobs.
    Forgotten { vol: K, limit: usize },
}

#[derive(Clone, Copy, Debug)]
struct Window {
    t0: f64,
    at: Counters,
    total0: f64,
}

#[derive(Clone, Debug)]
struct VolCtl {
    limit: usize,
    /// Its start limit (the calls' `workers()` when it was first measured).
    start: usize,
    /// Measured at least once: it no longer follows its calls' `workers()`.
    learned: bool,
    phase: Phase,
    up: bool,
    target: usize,
    /// The limit to restore after the probe window (stored, never recomputed).
    old: usize,
    last_window: f64,
    base1: f64,
    base1_total: f64,
    probe: f64,
    probe_total: f64,
    hold_left: u32,
    hold_len: u32,
    win: Option<Window>,
    /// When it last had jobs (s).
    last_busy: f64,
}

impl VolCtl {
    fn new(limit: usize, now: f64) -> VolCtl {
        VolCtl {
            limit,
            start: limit,
            learned: false,
            phase: Phase::Base1,
            up: true,
            target: 0,
            old: 0,
            last_window: FIRST_LAST_WINDOW,
            base1: 0.0,
            base1_total: 0.0,
            probe: 0.0,
            probe_total: 0.0,
            hold_left: 0,
            hold_len: HOLD_FIRST,
            win: None,
            last_busy: now,
        }
    }
}

/// Every volume's limit and where it is in its probe cycle; the one probing token.
pub(crate) struct Controller<K> {
    vols: HashMap<K, VolCtl>,
    token: Option<K>,
    cap: usize,
    /// The limit of a volume not seen yet.
    start: usize,
    events: Vec<Event<K>>,
}

impl<K: Clone + Eq + Hash> Controller<K> {
    /// `cap`: the most any volume may reach; `start`: a new volume's limit unless the scheduler
    /// offers another ([`Controller::offer_start`]).
    pub(crate) fn new(cap: usize, start: usize) -> Controller<K> {
        let cap = cap.max(1);
        Controller { vols: HashMap::new(), token: None, cap, start: start.clamp(1, cap), events: Vec::new() }
    }

    /// Volume `k`'s limit now.
    pub(crate) fn limit(&self, k: &K) -> usize {
        self.vols.get(k).map_or(self.start, |s| s.limit)
    }

    /// How far volume `k`'s limit is above its start (the import's read-ahead grows with it).
    pub(crate) fn raised(&self, k: &K) -> usize {
        self.vols.get(k).map_or(0, |s| s.limit.saturating_sub(s.start))
    }

    /// Where volume `k` is in its cycle (`None`: not seen, or forgotten).
    pub(crate) fn phase(&self, k: &K) -> Option<Phase> {
        self.vols.get(k).map(|s| s.phase)
    }

    /// The volume probing (holding the token).
    pub(crate) fn token(&self) -> Option<&K> {
        self.token.as_ref()
    }

    /// The volumes it knows, with their limits.
    pub(crate) fn limits(&self) -> impl Iterator<Item = (&K, usize)> {
        self.vols.iter().map(|(k, s)| (k, s.limit))
    }

    /// What it did since the last call.
    pub(crate) fn take_events(&mut self) -> Vec<Event<K>> {
        std::mem::take(&mut self.events)
    }

    /// Volume `k`'s calls run `hint` jobs at once today (PR 1's `workers()`): a volume not yet
    /// measured takes that limit (so a short call, or one too short to measure anything, runs as
    /// before); a measured one keeps what it learned.
    pub(crate) fn offer_start(&mut self, now: f64, k: &K, hint: usize) {
        let hint = hint.clamp(1, self.cap);
        let s = self.vols.entry(k.clone()).or_insert_with(|| VolCtl::new(hint, now));
        if !s.learned && s.limit != hint {
            s.limit = hint;
            s.start = hint;
            // (its window measured another limit)
            s.win = None;
        }
    }

    /// Volume `k` has jobs (`busy`) or not at `now`: a volume with none for [`FORGET`] is
    /// forgotten first, so a later call on it starts afresh. Call before [`Controller::tick`].
    pub(crate) fn note_busy(&mut self, now: f64, k: &K, busy: bool) {
        if let Some(s) = self.vols.get(k)
            && now - s.last_busy >= FORGET
        {
            let limit = s.limit;
            self.vols.remove(k);
            if self.token.as_ref() == Some(k) {
                self.token = None;
            }
            self.events.push(Event::Forgotten { vol: k.clone(), limit });
        }
        if busy {
            let start = self.start;
            self.vols.entry(k.clone()).or_insert_with(|| VolCtl::new(start, now)).last_busy = now;
        }
    }

    fn up(&self, l: usize) -> usize {
        // (ceil(L/4): never under +25 %)
        l.saturating_add(l.div_ceil(4)).min(self.cap)
    }

    fn down(l: usize) -> usize {
        l.saturating_sub((l / 2).max(1)).max(1)
    }

    fn probing(&self, k: &K, s: &VolCtl) -> bool {
        self.token.as_ref() == Some(k) && matches!(s.phase, Phase::Base1 | Phase::Probe | Phase::Base2)
    }

    /// One tick for volume `k` at `now`: where it stands, its counters, the process's total bytes.
    pub(crate) fn tick(&mut self, now: f64, k: &K, demand: Demand, c: Counters, total: f64) {
        let start = self.start;
        let mut s = match self.vols.remove(k) {
            Some(s) => s,
            None => VolCtl::new(start, now),
        };
        self.step(now, k, &mut s, demand, c, total);
        self.vols.insert(k.clone(), s);
    }

    fn step(&mut self, now: f64, k: &K, s: &mut VolCtl, demand: Demand, c: Counters, total: f64) {
        let probing = self.probing(k, s);
        if probing
            && let Some(w) = s.win
            && now - w.t0 > ABORT_MIN.max(ABORT_FACTOR * s.last_window)
        {
            // (a window that can't complete: no bytes moving)
            return self.abort(k, s, AbortReason::Deadline);
        }
        match demand {
            Demand::Limited => {
                s.win = None;
                if probing {
                    // (the volume ran out of work mid-probe)
                    self.abort(k, s, AbortReason::Demand);
                }
                return;
            }
            Demand::Draining => {
                s.win = None;
                return;
            }
            Demand::AtLimit => {}
        }
        if s.phase == Phase::Base1 && self.token.as_ref().is_some_and(|t| t != k) {
            // (another volume is probing: wait for the token)
            s.win = None;
            return;
        }
        let Some(w) = s.win else {
            if s.phase == Phase::Base1 && self.token.is_none() {
                // (taken before the first baseline, so no baseline overlaps another's probe)
                self.token = Some(k.clone());
            }
            s.win = Some(Window { t0: now, at: c, total0: total });
            return;
        };
        let span = now - w.t0;
        let jobs = c.jobs.saturating_sub(w.at.jobs);
        let enough_jobs = jobs >= WINDOW_JOBS.max((s.limit as u64).saturating_mul(2));
        let long_with_bytes = span >= WINDOW_LONG && c.bytes > w.at.bytes;
        if span < WINDOW_MIN || !(enough_jobs || long_with_bytes) {
            return;
        }
        let rate = (c.bytes - w.at.bytes) / span;
        let tot = (total - w.total0) / span;
        s.win = None;
        s.last_window = span;
        let (busy, gate) = (c.busy - w.at.busy, c.gate - w.at.gate);
        if gate > GATE_SHARE * busy {
            self.events.push(Event::Discarded { vol: k.clone(), limit: s.limit, gate: if busy > 0.0 { gate / busy } else { 1.0 } });
            if self.probing(k, s) {
                self.abort(k, s, AbortReason::Discarded);
            }
            return;
        }
        self.events.push(Event::Window { vol: k.clone(), phase: s.phase, limit: s.limit, rate, total: tot, span, jobs });
        self.window_done(k, s, rate, tot);
    }

    fn window_done(&mut self, k: &K, s: &mut VolCtl, rate: f64, total: f64) {
        match s.phase {
            Phase::Hold => {
                s.hold_left = s.hold_left.saturating_sub(1);
                if s.hold_left == 0 {
                    s.phase = Phase::Base1;
                    s.up = true;
                }
            }
            Phase::Base1 => {
                s.learned = true;
                s.base1 = rate;
                s.base1_total = total;
                s.target = if s.up { self.up(s.limit) } else { Self::down(s.limit) };
                if s.target == s.limit && s.up {
                    // (at the cap a step up can't happen: the up-probe counts as failed)
                    s.up = false;
                    s.target = Self::down(s.limit);
                }
                if s.target == s.limit {
                    return self.finish(k, s);
                }
                self.events.push(Event::Probe { vol: k.clone(), from: s.limit, to: s.target });
                s.old = s.limit;
                s.limit = s.target;
                s.phase = Phase::Probe;
            }
            Phase::Probe => {
                s.probe = rate;
                s.probe_total = total;
                // (back to the stored limit)
                s.limit = s.old;
                s.phase = Phase::Base2;
            }
            Phase::Base2 => {
                let mean = (s.base1 + rate) / 2.0;
                let base_total = (s.base1_total + total) / 2.0;
                let ok = if s.up {
                    s.probe >= mean * (1.0 + UP_GAIN)
                        && (s.probe_total - base_total) >= TOTAL_SHARE * (s.probe - mean)
                        && s.probe >= s.base1.max(rate)
                } else {
                    // (fewer readers are kept when the process loses next to nothing and this
                    // volume keeps most of its rate: no starving one volume on a shared link)
                    s.probe_total >= base_total * (1.0 - DOWN_TOTAL) && s.probe >= mean * (1.0 - DOWN_OWN)
                };
                self.events.push(Event::Decided {
                    vol: k.clone(),
                    up: s.up,
                    from: s.limit,
                    to: s.target,
                    kept: ok,
                    base: mean,
                    probe: s.probe,
                    base_total,
                    probe_total: s.probe_total,
                });
                self.token = None;
                if ok {
                    // (keep going the same way)
                    s.limit = s.target;
                    s.phase = Phase::Base1;
                    s.hold_len = HOLD_FIRST;
                    return;
                }
                if s.up {
                    // (no gain up: try down)
                    s.up = false;
                    s.phase = Phase::Base1;
                    return;
                }
                self.finish(k, s);
            }
        }
    }

    fn abort(&mut self, k: &K, s: &mut VolCtl, reason: AbortReason) {
        if s.phase == Phase::Probe {
            s.limit = s.old;
        }
        s.win = None;
        self.events.push(Event::Aborted { vol: k.clone(), reason, limit: s.limit });
        self.finish(k, s);
    }

    fn finish(&mut self, k: &K, s: &mut VolCtl) {
        s.phase = Phase::Hold;
        s.hold_left = s.hold_len;
        s.hold_len = s.hold_len.saturating_mul(2).min(HOLD_MAX);
        self.events.push(Event::Hold { vol: k.clone(), limit: s.limit, windows: s.hold_left });
        if self.token.as_ref() == Some(k) {
            self.token = None;
        }
    }
}
