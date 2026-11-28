//! The controller's thresholds (§7 "Controller (PR 5)"), each at, just below and just above: the
//! 15 % gain, the half-of-own-gain total rule, the "not below either baseline" floor, the 5 % and
//! 75 % down rules, the 20 % gate-wait discard, the 4 s, `max(8, 2L)` and 60 s window rules, the
//! abort deadline, the hold lengths, the 10-minute forget; and the steps, the token, the start.
//! Pool-level behaviour (the I/O cap, measurement on the right volume) is in `tests_pool_limits`.

use super::limits::{
    ABORT_FACTOR, ABORT_MIN, AbortReason, Controller, Counters, DOWN_OWN, DOWN_TOTAL, Demand, Event, FORGET, GATE_SHARE, HOLD_FIRST, HOLD_MAX,
    IO_CAP, Phase, TOTAL_SHARE, UP_GAIN, WINDOW_JOBS, WINDOW_LONG, WINDOW_MIN, cut, demand, under_cap,
};

const V: &str = "v";
const W: &str = "w";

/// A controller and one volume's counters, driven window by window.
struct Rig {
    ctl: Controller<&'static str>,
    t: f64,
    c: Counters,
    total: f64,
}

impl Rig {
    fn new(start: usize) -> Rig {
        let mut ctl = Controller::new(32, start);
        ctl.offer_start(0.0, &V, start);
        Rig { ctl, t: 100.0, c: Counters::default(), total: 0.0 }
    }

    fn tick(&mut self, d: Demand) {
        self.ctl.tick(self.t, &V, d, self.c, self.total);
    }

    /// Move `secs` on: the volume moves `rate` per second and finishes `jobs`, the process `total`
    /// per second; one tick at the end.
    fn run(&mut self, secs: f64, rate: f64, total: f64, jobs: u64) {
        self.t += secs;
        self.c.bytes += rate * secs;
        self.c.jobs += jobs;
        self.total += total * secs;
        self.tick(Demand::AtLimit);
    }

    /// One whole window (opened, then 4 s with enough jobs).
    fn window(&mut self, rate: f64, total: f64) {
        self.tick(Demand::AtLimit);
        self.run(4.0, rate, total, 64);
    }

    fn limit(&self) -> usize {
        self.ctl.limit(&V)
    }

    fn phase(&self) -> Option<Phase> {
        self.ctl.phase(&V)
    }

    /// The last up/down decision: kept or not.
    fn kept(&mut self) -> Option<bool> {
        self.ctl.take_events().into_iter().rev().find_map(|e| match e {
            Event::Decided { kept, .. } => Some(kept),
            _ => None,
        })
    }

    /// A whole up-probe from `start`: baselines `b1` and `b2`, probe `p`; process totals likewise.
    fn up_probe(start: usize, (b1, p, b2): (f64, f64, f64), (t1, tp, t2): (f64, f64, f64)) -> (Rig, Option<bool>) {
        let mut r = Rig::new(start);
        r.window(b1, t1);
        assert_eq!(r.phase(), Some(Phase::Probe));
        r.window(p, tp);
        assert_eq!(r.phase(), Some(Phase::Base2));
        r.window(b2, t2);
        let kept = r.kept();
        (r, kept)
    }

    /// A failed up-probe from `start`, then a whole down-probe: baselines 100 (process 1000), the
    /// down window `p` (process `tp`).
    fn down_probe(start: usize, p: f64, tp: f64) -> (Rig, Option<bool>) {
        let (mut r, kept) = Rig::up_probe(start, (100.0, 100.0, 100.0), (1000.0, 1000.0, 1000.0));
        assert_eq!(kept, Some(false));
        r.window(100.0, 1000.0);
        assert_eq!(r.phase(), Some(Phase::Probe), "a failed up-probe tries down");
        r.window(p, tp);
        r.window(100.0, 1000.0);
        let kept = r.kept();
        (r, kept)
    }
}

#[test]
fn steps_up_by_a_quarter_rounded_up_and_down_by_half() {
    for (from, up) in [(1, 2), (2, 3), (3, 4), (4, 5), (5, 7), (8, 10), (9, 12), (26, 32), (30, 32)] {
        let mut r = Rig::new(from);
        r.window(100.0, 100.0);
        assert_eq!(r.limit(), up, "up from {from}");
    }
    for (from, down) in [(2, 1), (3, 2), (4, 2), (5, 3), (8, 4)] {
        let (r, _) = Rig::down_probe(from, 100.0, 1000.0);
        assert_eq!(r.limit(), down, "kept down from {from}");
    }
    // at the cap an up-probe can't happen: it counts as failed and goes down
    let mut r = Rig::new(32);
    r.window(100.0, 100.0);
    assert_eq!((r.phase(), r.limit()), (Some(Phase::Probe), 16));
    // at 1 a down-probe can't happen either: it holds
    let (r, kept) = Rig::up_probe(1, (100.0, 100.0, 100.0), (1000.0, 1000.0, 1000.0));
    assert_eq!(kept, Some(false));
    let mut r = r;
    r.window(100.0, 1000.0);
    assert_eq!((r.phase(), r.limit()), (Some(Phase::Hold), 1));
}

/// The probe must beat the mean of the baselines by 15 %: at, just below, just above. Baselines
/// of 80, where the threshold is exactly 92 in f64 (with baselines of 100 it is 114.999…, so a
/// probe of 115 lay above it and `>` in place of `>=` went unseen).
#[test]
fn up_gain_threshold() {
    assert_eq!(UP_GAIN, 0.15);
    assert_eq!(80.0 * (1.0 + UP_GAIN), 92.0, "the \"at\" row below is on the threshold");
    for (p, want) in [(92.0, true), (91.9, false), (92.1, true)] {
        let (r, kept) = Rig::up_probe(4, (80.0, p, 80.0), (1000.0, 1000.0 + p, 1000.0));
        assert_eq!(kept, Some(want), "probe {p} against baselines 80");
        assert_eq!(r.limit(), if want { 5 } else { 4 });
    }
}

/// The process total must rise by at least half the volume's own gain: at, just below, above.
#[test]
fn up_total_share_threshold() {
    assert_eq!(TOTAL_SHARE, 0.5);
    // (own gain 30 over the mean: the total must rise by 15)
    for (tp, want) in [(1015.0, true), (1014.9, false), (1015.1, true)] {
        let (_, kept) = Rig::up_probe(4, (100.0, 130.0, 100.0), (1000.0, tp, 1000.0));
        assert_eq!(kept, Some(want), "process total {tp}");
    }
}

/// The probe must not be below either baseline (periodic interference): at, just below, above.
#[test]
fn up_floor_threshold() {
    // (baselines 60 and 140: mean 100, so 15 % is met by anything from 115 on)
    for (p, want) in [(140.0, true), (139.9, false), (140.1, true)] {
        let (_, kept) = Rig::up_probe(4, (60.0, p, 140.0), (1000.0, 2000.0, 1000.0));
        assert_eq!(kept, Some(want), "probe {p} against baselines 60 and 140");
    }
    // either baseline: the first one too
    let (_, kept) = Rig::up_probe(4, (140.0, 139.9, 60.0), (1000.0, 2000.0, 1000.0));
    assert_eq!(kept, Some(false));
}

/// A lower limit is kept when the process total falls by no more than 5 %: at, below, above.
#[test]
fn down_total_threshold() {
    assert_eq!(DOWN_TOTAL, 0.05);
    for (tp, want) in [(950.0, true), (949.9, false), (950.1, true)] {
        let (r, kept) = Rig::down_probe(4, 100.0, tp);
        assert_eq!(kept, Some(want), "process total {tp} against 1000");
        assert_eq!(r.limit(), if want { 2 } else { 4 });
    }
}

/// ... and the volume keeps at least 75 % of its own rate: at, below, above.
#[test]
fn down_own_threshold() {
    assert_eq!(DOWN_OWN, 0.25);
    for (p, want) in [(75.0, true), (74.9, false), (75.1, true)] {
        let (_, kept) = Rig::down_probe(4, p, 1000.0);
        assert_eq!(kept, Some(want), "own rate {p} against 100");
    }
}

/// A kept step probes the same way again; a failed down holds.
#[test]
fn a_kept_step_goes_on_and_a_failed_down_holds() {
    let (mut r, kept) = Rig::up_probe(4, (100.0, 200.0, 100.0), (100.0, 200.0, 100.0));
    assert_eq!(kept, Some(true));
    r.window(200.0, 200.0);
    assert_eq!((r.phase(), r.limit()), (Some(Phase::Probe), 7), "up again from 5");
    let (r, kept) = Rig::down_probe(4, 10.0, 100.0);
    assert_eq!(kept, Some(false));
    assert_eq!((r.phase(), r.limit()), (Some(Phase::Hold), 4));
    assert!(r.ctl.token().is_none());
}

/// A window whose jobs spent more than 20 % of their time at the work gate is discarded: at,
/// just below, just above; a discarded window ends a probe and restores its limit.
#[test]
fn gate_wait_discard_threshold() {
    assert_eq!(GATE_SHARE, 0.2);
    for (gate, discarded) in [(2.0, false), (1.99, false), (2.01, true)] {
        let mut r = Rig::new(4);
        r.tick(Demand::AtLimit);
        r.c.busy += 10.0;
        r.c.gate += gate;
        r.run(4.0, 100.0, 100.0, 64);
        let events = r.ctl.take_events();
        let was = events.iter().any(|e| matches!(e, Event::Discarded { .. }));
        assert_eq!(was, discarded, "{gate} s of 10 at the gate");
        if discarded {
            assert!(events.iter().any(|e| matches!(e, Event::Aborted { reason: AbortReason::Discarded, .. })));
            assert_eq!((r.phase(), r.limit()), (Some(Phase::Hold), 4));
            assert!(r.ctl.token().is_none());
        } else {
            assert_eq!((r.phase(), r.limit()), (Some(Phase::Probe), 5));
        }
    }
    // in the probe window: the stored limit comes back
    let mut r = Rig::new(4);
    r.window(100.0, 100.0);
    assert_eq!(r.limit(), 5);
    r.tick(Demand::AtLimit);
    r.c.busy += 10.0;
    r.c.gate += 3.0;
    r.run(4.0, 100.0, 100.0, 64);
    assert_eq!((r.phase(), r.limit()), (Some(Phase::Hold), 4));
    // a window in a hold that is discarded doesn't count as one of the hold's windows
    let mut r = Rig::new(4);
    r.tick(Demand::AtLimit);
    r.tick(Demand::Limited);
    assert_eq!(r.phase(), Some(Phase::Hold));
    for _ in 0..HOLD_FIRST - 1 {
        r.window(100.0, 100.0);
    }
    r.tick(Demand::AtLimit);
    r.c.gate += 1.0;
    r.c.busy += 1.0;
    r.run(4.0, 100.0, 100.0, 64);
    assert_eq!(r.phase(), Some(Phase::Hold), "a discarded window isn't counted");
    r.window(100.0, 100.0);
    assert_eq!(r.phase(), Some(Phase::Base1));
}

/// A window lasts at least 4 s: at, just below, just above.
#[test]
fn window_minimum_time() {
    assert_eq!(WINDOW_MIN, 4.0);
    for (secs, ends) in [(4.0, true), (3.99, false), (4.01, true)] {
        let mut r = Rig::new(4);
        r.tick(Demand::AtLimit);
        r.run(secs, 100.0, 100.0, 64);
        assert_eq!(r.phase() == Some(Phase::Probe), ends, "{secs} s");
    }
}

/// ... and until `max(8, 2 × limit)` jobs finished: at, just below, just above, both sides of 8.
#[test]
fn window_job_count() {
    assert_eq!(WINDOW_JOBS, 8);
    for (limit, jobs, ends) in [(3, 8, true), (3, 7, false), (3, 9, true), (5, 10, true), (5, 9, false), (5, 11, true), (4, 8, true), (4, 7, false)] {
        let mut r = Rig::new(limit);
        r.tick(Demand::AtLimit);
        r.run(5.0, 100.0, 100.0, jobs);
        assert_eq!(r.phase() == Some(Phase::Probe), ends, "limit {limit}, {jobs} jobs");
    }
}

/// ... or 60 s with bytes moving (2 GB clips): at, just below, just above; not without bytes.
#[test]
fn window_long_with_bytes() {
    assert_eq!(WINDOW_LONG, 60.0);
    for (secs, rate, ends) in [(60.0, 1.0, true), (59.99, 1.0, false), (60.01, 1.0, true), (60.0, 0.0, false), (61.0, 0.0, false)] {
        let mut r = Rig::new(4);
        r.tick(Demand::AtLimit);
        r.run(secs, rate, 100.0, 1);
        assert_eq!(r.phase() == Some(Phase::Probe), ends, "{secs} s at {rate}");
    }
}

/// A probing volume's window that can't complete in `max(90 s, 3 × the last window)` ends the
/// probe: at, just below, just above, for both terms.
#[test]
fn abort_deadline() {
    assert_eq!((ABORT_MIN, ABORT_FACTOR), (90.0, 3.0));
    // (the first baseline took 4 s: 90 s rules; one that took 40 s: 120 s)
    for (first, wait, aborts) in
        [(4.0, 90.0, false), (4.0, 89.99, false), (4.0, 90.01, true), (40.0, 120.0, false), (40.0, 119.99, false), (40.0, 120.01, true)]
    {
        let mut r = Rig::new(4);
        r.tick(Demand::AtLimit);
        r.run(first, 100.0, 100.0, 64);
        assert_eq!(r.limit(), 5);
        // the probe window opens, then nothing finishes and nothing moves
        r.tick(Demand::AtLimit);
        r.t += wait;
        r.tick(Demand::AtLimit);
        let ended = r.ctl.take_events().iter().any(|e| matches!(e, Event::Aborted { reason: AbortReason::Deadline, .. }));
        assert_eq!(ended, aborts, "first window {first} s, then {wait} s");
        if aborts {
            assert_eq!((r.phase(), r.limit()), (Some(Phase::Hold), 4));
            assert!(r.ctl.token().is_none());
        }
    }
}

/// Time spent draining to a lower limit doesn't count toward the deadline.
#[test]
fn draining_does_not_count_toward_the_deadline() {
    let (mut r, _) = Rig::up_probe(4, (100.0, 100.0, 100.0), (1000.0, 1000.0, 1000.0));
    r.window(100.0, 1000.0);
    assert_eq!(r.limit(), 2);
    r.tick(Demand::Draining);
    r.t += 200.0;
    r.tick(Demand::Draining);
    r.tick(Demand::AtLimit);
    r.t += 80.0;
    r.tick(Demand::AtLimit);
    assert_eq!(r.phase(), Some(Phase::Probe), "200 s draining, then 80 s measuring");
}

/// A volume running out of work mid-probe ends the probe: its limit and the token come back.
#[test]
fn running_out_of_work_ends_a_probe() {
    let mut r = Rig::new(4);
    r.window(100.0, 100.0);
    assert_eq!((r.limit(), r.ctl.token()), (5, Some(&V)));
    r.tick(Demand::Limited);
    assert_eq!((r.phase(), r.limit(), r.ctl.token()), (Some(Phase::Hold), 4, None));
}

/// Hold: 8 windows after the first failed down-probe or abort, then 16, 16; a kept change
/// resets it to 8. At, just below and just above each length.
#[test]
fn hold_lengths() {
    assert_eq!((HOLD_FIRST, HOLD_MAX), (8, 16));
    let mut r = Rig::new(4);
    let hold = |r: &mut Rig, n: u32| {
        // (an abort in the first baseline)
        r.tick(Demand::AtLimit);
        r.tick(Demand::Limited);
        assert_eq!(r.phase(), Some(Phase::Hold));
        for k in 1..=n + 1 {
            r.window(100.0, 100.0);
            let held = r.phase() == Some(Phase::Hold);
            assert_eq!(held, k < n, "window {k} of a hold of {n}");
            if !held {
                break;
            }
        }
    };
    hold(&mut r, 8);
    hold(&mut r, 16);
    hold(&mut r, 16);
    // a kept change resets it
    r.window(100.0, 100.0);
    r.window(200.0, 200.0);
    r.window(100.0, 100.0);
    assert_eq!(r.kept(), Some(true));
    hold(&mut r, 8);
}

/// A volume with no jobs for 10 minutes is forgotten, at, just below and just above; a forgotten
/// volume starts afresh at its calls' limit.
#[test]
fn ten_minute_forget() {
    assert_eq!(FORGET, 600.0);
    for (idle, forgotten) in [(600.0, true), (599.99, false), (600.01, true)] {
        let mut ctl: Controller<&str> = Controller::new(32, 8);
        ctl.note_busy(0.0, &V, true);
        ctl.offer_start(0.0, &V, 4);
        let mut r = Rig { ctl, t: 0.0, c: Counters::default(), total: 0.0 };
        r.window(100.0, 100.0);
        assert_eq!(r.limit(), 5);
        r.ctl.note_busy(r.t, &V, true);
        // (its probe ended with its work)
        r.tick(Demand::Limited);
        r.ctl.note_busy(r.t, &V, false);
        let last = r.t;
        r.ctl.note_busy(last + idle, &V, false);
        assert_eq!(r.ctl.phase(&V).is_none(), forgotten, "after {idle} s");
        // a new call: a forgotten volume takes the new call's limit, a known one keeps its own
        r.ctl.note_busy(last + idle, &V, true);
        r.ctl.offer_start(last + idle, &V, 2);
        assert_eq!(r.ctl.limit(&V), if forgotten { 2 } else { 4 });
    }
    // a volume busy at the next tick after a long gap is forgotten too
    let mut ctl: Controller<&str> = Controller::new(32, 8);
    ctl.note_busy(0.0, &V, true);
    ctl.note_busy(700.0, &V, true);
    assert!(ctl.take_events().iter().any(|e| matches!(e, Event::Forgotten { .. })));
}

/// A volume follows its calls' `workers()` until it is measured, then keeps what it learned.
#[test]
fn a_volume_starts_at_its_calls_limit() {
    let mut ctl: Controller<&str> = Controller::new(32, 8);
    ctl.offer_start(0.0, &V, 3);
    assert_eq!(ctl.limit(&V), 3);
    ctl.offer_start(0.0, &V, 6);
    assert_eq!(ctl.limit(&V), 6, "not measured yet: follows");
    ctl.offer_start(0.0, &V, 99);
    assert_eq!(ctl.limit(&V), 32, "never above the cap");
    let mut r = Rig { ctl, t: 0.0, c: Counters::default(), total: 0.0 };
    r.ctl.offer_start(0.0, &V, 6);
    r.window(100.0, 100.0);
    assert_eq!(r.limit(), 8);
    r.ctl.offer_start(r.t, &V, 2);
    assert_eq!(r.limit(), 8, "measured: keeps its own");
    assert_eq!(r.ctl.raised(&V), 2);
}

/// One volume probes at a time: the other waits for the token, takes it once it is free.
#[test]
fn one_volume_probes_at_a_time() {
    let mut ctl: Controller<&str> = Controller::new(32, 4);
    let c = Counters::default();
    ctl.tick(0.0, &V, Demand::AtLimit, c, 0.0);
    ctl.tick(0.0, &W, Demand::AtLimit, c, 0.0);
    assert_eq!(ctl.token(), Some(&V));
    let done = Counters { bytes: 400.0, jobs: 64, busy: 0.0, gate: 0.0 };
    ctl.tick(4.0, &V, Demand::AtLimit, done, 400.0);
    ctl.tick(4.0, &W, Demand::AtLimit, done, 400.0);
    assert_eq!((ctl.limit(&V), ctl.limit(&W)), (5, 4), "w holds while v probes");
    assert_eq!(ctl.phase(&W), Some(Phase::Base1));
    // v's work ends: the token is free and w takes it at its next tick
    ctl.tick(5.0, &V, Demand::Limited, done, 400.0);
    assert_eq!(ctl.token(), None);
    ctl.tick(5.0, &W, Demand::AtLimit, done, 400.0);
    assert_eq!(ctl.token(), Some(&W));
}

/// A window in which the volume moved no byte is no measurement (`0 ≥ 0` passed every up and
/// down rule, so the limit climbed to the cap on nothing): it is discarded and a probe ends; one
/// byte is enough to count (at 0, just above).
#[test]
fn windows_that_move_no_bytes_are_discarded() {
    for (bytes, counts) in [(0.0, false), (1.0, true)] {
        let mut r = Rig::new(4);
        r.tick(Demand::AtLimit);
        r.c.bytes += bytes;
        r.run(4.0, 0.0, 0.0, 64);
        let events = r.ctl.take_events();
        assert_eq!(events.iter().any(|e| matches!(e, Event::NoBytes { .. })), !counts, "{bytes} bytes");
        if counts {
            assert_eq!((r.phase(), r.limit()), (Some(Phase::Probe), 5), "{bytes} bytes");
        } else {
            assert!(events.iter().any(|e| matches!(e, Event::Aborted { reason: AbortReason::NoBytes, .. })));
            assert_eq!((r.phase(), r.limit(), r.ctl.token()), (Some(Phase::Hold), 4, None));
        }
    }
    // a probe window that moved nothing ends the probe: the stored limit comes back
    let mut r = Rig::new(4);
    r.window(100.0, 100.0);
    assert_eq!(r.limit(), 5);
    r.window(0.0, 0.0);
    assert_eq!((r.phase(), r.limit(), r.ctl.token()), (Some(Phase::Hold), 4, None));
    // windows that never move a byte never move the limit, however many
    let mut r = Rig::new(4);
    for _ in 0..200 {
        r.window(0.0, 0.0);
    }
    let events = r.ctl.take_events();
    assert!(!events.iter().any(|e| matches!(e, Event::Probe { .. } | Event::Decided { .. })), "{events:?}");
    assert_eq!(r.limit(), 4);
}

/// Where a volume stands after admission: running against its limit (at, just below, just
/// above), and below its limit only a job waiting for the I/O cap keeps it at its limit.
#[test]
fn demand_after_admission() {
    use Demand::{AtLimit, Draining, Limited};
    for (running, waiting, want) in [
        (4, false, AtLimit),
        (3, false, Limited),
        (5, false, Draining),
        (4, true, AtLimit),
        (3, true, AtLimit),
        (5, true, Draining),
        (0, false, Limited),
    ] {
        assert_eq!(demand(running, 4, waiting), want, "{running} running at limit 4, waiting {waiting}");
    }
}

/// R4a: a volume with work is cut while some job doesn't need it (at, below, above the total);
/// its limit is then one fewer than the cap, at least 1; never above the cap.
#[test]
fn the_cut_and_the_limit_under_the_cap() {
    assert_eq!(IO_CAP, 32);
    for (active, total, want) in [(5, 5, false), (5, 6, true), (5, 4, false), (0, 3, false)] {
        assert_eq!(cut(active, total), want, "{active} of {total}");
    }
    for (own, cap, cut, want) in
        [(40, 32, false, 32), (40, 32, true, 31), (32, 32, true, 31), (31, 32, true, 31), (30, 32, true, 30), (5, 2, true, 1), (5, 1, true, 1)]
    {
        assert_eq!(under_cap(own, cap, cut), want, "{own} under {cap}, cut {cut}");
    }
}

/// A volume's floor (PR 1's "at least 2" for a destination that several cards copy into): a down
/// step stops at it (at, below and above the step's own result), a limit learned below a floor
/// that rises comes up to it, and a down-probe measuring under it ends.
#[test]
fn the_floor_holds_a_down_step() {
    for (start, floor, want) in [(2, 2, 2), (2, 1, 1), (4, 2, 2), (4, 3, 3), (4, 1, 2), (8, 2, 4)] {
        let mut r = Rig::new(start);
        r.ctl.set_floor(&V, floor);
        // a failed up-probe, then the down step's baseline
        for _ in 0..3 {
            r.window(100.0, 1000.0);
        }
        r.window(100.0, 1000.0);
        assert_eq!(r.limit(), want, "from {start} with floor {floor}");
        assert_eq!(r.phase(), Some(if want == start { Phase::Hold } else { Phase::Probe }), "from {start} with floor {floor}");
    }
    let (mut r, kept) = Rig::down_probe(2, 100.0, 1000.0);
    assert_eq!((kept, r.limit()), (Some(true), 1));
    r.ctl.set_floor(&V, 2);
    assert_eq!(r.limit(), 2);
    assert!(r.ctl.take_events().iter().any(|e| matches!(e, Event::Floor { limit: 2, .. })));
    // mid down-probe (2 -> 1): it ends, and the limit is the floor
    let mut r = Rig::new(2);
    for _ in 0..4 {
        r.window(100.0, 1000.0);
    }
    assert_eq!((r.phase(), r.limit()), (Some(Phase::Probe), 1));
    r.ctl.set_floor(&V, 2);
    assert_eq!((r.phase(), r.limit(), r.ctl.token()), (Some(Phase::Hold), 2, None));
    assert!(r.ctl.take_events().iter().any(|e| matches!(e, Event::Aborted { reason: AbortReason::Floor, .. })));
    // a volume not measured yet starts at least at its floor
    let mut ctl: Controller<&str> = Controller::new(32, 8);
    ctl.offer_start(0.0, &V, 1);
    ctl.set_floor(&V, 2);
    ctl.offer_start(0.0, &V, 1);
    assert_eq!(ctl.limit(&V), 2);
}
