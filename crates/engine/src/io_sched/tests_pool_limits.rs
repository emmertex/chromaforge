//! Adaptive limits in the pool (§5.7, PR 5), on fake volumes and the fake clock: jobs wait until
//! the test lets them finish, so the test plays the device. "Linear": every running job finishes
//! each half second (more readers, more bytes); "flat": two finish each half second whatever the
//! limit (a device at its knee). What the controller learned is read from the pool's published
//! limits ([`super::shared::Shared::learned`]).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use lcc_catalog::folders::{Mounts, VolumeKey, volume_key};

use super::clock::SEC;
use super::job::ByteKind;
use super::run::IoJob;
use super::shared::Learned;
use super::tests::{Call, H, R, bytes, job, wait_until, wrote};

/// Jobs the test finishes: each waits for a permit.
#[derive(Default)]
struct Device {
    st: Mutex<(usize, usize)>,
    cv: Condvar,
    most: AtomicUsize,
}

impl Device {
    fn waiting(&self) -> usize {
        self.st.lock().unwrap().0
    }

    /// Let `n` waiting jobs finish.
    fn finish(&self, n: usize) {
        self.st.lock().unwrap().1 += n;
        self.cv.notify_all();
    }

    fn enter(&self) {
        let mut g = self.st.lock().unwrap();
        g.0 += 1;
        self.most.fetch_max(g.0, Ordering::SeqCst);
        while g.1 == 0 {
            g = self.cv.wait(g).unwrap();
        }
        g.1 -= 1;
        g.0 -= 1;
    }
}

/// `n` jobs on `vol` (copies into `dest` when given) that wait for the device, then count 1 MB
/// read (a copy also 1 MB written and 1 MB read back, as `copy_verified` counts them); `gate`:
/// they say they wait at the work gate meanwhile.
fn jobs(vol: &str, dest: Option<&str>, n: usize, dev: &Arc<Device>, gate: bool) -> Vec<IoJob<R>> {
    let copy = dest.is_some();
    (0..n)
        .map(|i| {
            let dev = dev.clone();
            let run = move || {
                let st = crate::memory::current_job();
                if gate && let Some(s) = &st {
                    s.set_at_gate(true);
                }
                dev.enter();
                if let Some(s) = &st {
                    s.set_at_gate(false);
                }
                bytes(1_000_000);
                if copy {
                    wrote(1_000_000);
                    read_back(1_000_000);
                }
                Ok(i as u32)
            };
            match dest {
                Some(d) => IoJob { source: format!("{vol}/f{i}"), dest: Some(format!("{d}/out")), skippable: false, run: Box::new(run) },
                None => job(&format!("{vol}/f{i}"), run),
            }
        })
        .collect()
}

/// `n` bytes read back from the thread's job's destination (a copy's verify).
fn read_back(n: usize) {
    if let Some(j) = crate::memory::current_job() {
        j.add(ByteKind::ReadBack, n);
    }
}

/// `n` copies from `vol` into `dest` that wait for the device, then read 1 MB from the source and
/// read 1 MB back from the destination, writing nothing.
fn copies_writing_nothing(vol: &str, dest: &str, n: usize, dev: &Arc<Device>) -> Vec<IoJob<R>> {
    (0..n)
        .map(|i| {
            let dev = dev.clone();
            let run = move || {
                dev.enter();
                bytes(1_000_000);
                read_back(1_000_000);
                Ok(i as u32)
            };
            IoJob { source: format!("{vol}/f{i}"), dest: Some(format!("{dest}/out")), skippable: false, run: Box::new(run) }
        })
        .collect()
}

/// `n` jobs on `vol` that wait for the device, then return without moving a byte (files that
/// fail at once, hard links).
fn empty_jobs(vol: &str, n: usize, dev: &Arc<Device>) -> Vec<IoJob<R>> {
    (0..n)
        .map(|i| {
            let dev = dev.clone();
            job(&format!("{vol}/f{i}"), move || {
                dev.enter();
                Ok(i as u32)
            })
        })
        .collect()
}

fn key(path: &str) -> VolumeKey {
    let points: Vec<String> = ["/", "/a", "/b", "/c", "/srv/nas"].iter().map(|s| s.to_string()).collect();
    volume_key(path, &Mounts::from_points(points, false))
}

fn learned(h: &H, vol: &str) -> Learned {
    h.ctx.shared.learned(&key(&format!("{vol}/x"))).unwrap_or_default()
}

/// Start `jobs` as one call and wait until the pool holds them all.
fn start(h: &H, jobs: Vec<IoJob<R>>, workers: usize) -> Call {
    let n = jobs.len();
    let call = h.start(jobs, workers);
    wait_until(|| h.ctx.pending.load(Ordering::SeqCst) == n, "the pool to hold the call's jobs");
    call
}

fn ticks(h: &H, n: u64) {
    let t = h.ctx.ticks.load(Ordering::SeqCst);
    // (a pool with no job left stops ticking)
    wait_until(|| h.ctx.ticks.load(Ordering::SeqCst) >= t + n || h.ctx.pending.load(Ordering::SeqCst) == 0, "pool ticks");
}

/// One half second of the device: `finish(waiting)` decides how many of the waiting jobs end.
fn step(h: &H, call: &Call, dev: &Device, finish: impl Fn(usize) -> usize) {
    step_on(h, call, dev, &["/a"], finish);
}

/// [`step`] for jobs on `vols` (a copy's two): the slots filled are the smaller of their limits.
fn step_on(h: &H, call: &Call, dev: &Device, vols: &[&str], finish: impl Fn(usize) -> usize) {
    ticks(h, 2);
    // (every slot the pool has filled is a job waiting on the device)
    wait_until(
        || {
            let limit =
                vols.iter().map(|v| h.ctx.shared.learned(&key(&format!("{v}/x"))).map_or(usize::MAX, |l| l.limit)).min().unwrap_or(usize::MAX);
            dev.waiting() >= limit.min(h.ctx.pending.load(Ordering::SeqCst))
        },
        "the volume's slots to fill",
    );
    let n = finish(dev.waiting());
    let done = call.done.load(Ordering::SeqCst);
    dev.finish(n);
    wait_until(|| call.done.load(Ordering::SeqCst) >= done + n, "the finished jobs' outcomes");
    ticks(h, 2);
    h.advance(SEC / 2);
}

/// A device whose throughput grows with its readers: the limit climbs from the call's `workers()`
/// (2) by kept up-probes; the pool never exceeds the volume's published limit.
#[test]
fn a_linear_volume_climbs() {
    let h = H::new();
    let dev = Arc::new(Device::default());
    let call = start(&h, jobs("/a", None, 2000, &dev, false), 2);
    let mut most_limit = 0;
    for _ in 0..140 {
        step(&h, &call, &dev, |w| w);
        most_limit = most_limit.max(learned(&h, "/a").limit);
        if learned(&h, "/a").raised >= 3 {
            break;
        }
    }
    let l = learned(&h, "/a");
    assert!(l.raised >= 3 && l.limit >= 5, "two kept steps up from 2: {l:?}");
    assert!(l.windows >= 6, "{l:?}");
    assert!(dev.most.load(Ordering::SeqCst) <= most_limit, "never above the limit");
    assert!(dev.most.load(Ordering::SeqCst) >= 3, "more readers than the start");
    dev.finish(usize::MAX / 2);
    assert_eq!(super::tests::ok_count(&call.join()), 2000);
}

/// A device at its knee (two jobs a half second whatever the limit): the up-probe fails, the
/// down-probe loses half the rate and fails too, and the volume holds at its start.
#[test]
fn a_flat_volume_holds_its_start() {
    let h = H::new();
    let dev = Arc::new(Device::default());
    let call = start(&h, jobs("/a", None, 2000, &dev, false), 2);
    for _ in 0..60 {
        step(&h, &call, &dev, |w| w.min(2));
    }
    let l = learned(&h, "/a");
    assert!(l.windows >= 4, "the probe was judged: {l:?}");
    assert_eq!((l.limit, l.raised), (2, 0), "{l:?}");
    assert!(!l.probing, "holding: {l:?}");
    dev.finish(usize::MAX / 2);
    assert_eq!(super::tests::ok_count(&call.join()), 2000);
}

/// A copy is measured on its destination only (§5.4): the SSD's windows run, the card's never do.
#[test]
fn copies_are_measured_on_their_destination() {
    let h = H::new();
    let dev = Arc::new(Device::default());
    let call = start(&h, jobs("/a", Some("/b"), 400, &dev, false), 2);
    for _ in 0..30 {
        step(&h, &call, &dev, |w| w);
    }
    let (a, b) = (learned(&h, "/a"), learned(&h, "/b"));
    assert!(b.windows >= 1, "the destination is measured: {b:?}");
    assert_eq!((a.windows, a.probing), (0, false), "the card is not: {a:?}");
    dev.finish(usize::MAX / 2);
    assert_eq!(super::tests::ok_count(&call.join()), 400);
}

/// A copy is measured on the bytes it wrote (§5.4), not on its source read or its verify
/// read-back: copies that read 1 MB and read 1 MB back but write nothing give their destination no
/// measurement (every window moved no byte); the same copies writing 1 MB each do.
#[test]
fn a_copy_is_measured_on_what_it_wrote() {
    for write in [false, true] {
        let h = H::new();
        let dev = Arc::new(Device::default());
        let jobs = if write { jobs("/a", Some("/b"), 400, &dev, false) } else { copies_writing_nothing("/a", "/b", 400, &dev) };
        let call = start(&h, jobs, 2);
        for _ in 0..30 {
            step(&h, &call, &dev, |w| w);
        }
        let b = learned(&h, "/b");
        if write {
            assert!(b.windows >= 1, "writes are measured: {b:?}");
        } else {
            assert_eq!(b.windows, 0, "reads and read-back aren't: {b:?}");
            assert!(b.discarded >= 1, "{b:?}");
        }
        dev.finish(usize::MAX / 2);
        assert_eq!(super::tests::ok_count(&call.join()), 400);
    }
}

/// "At its limit" is also exactly its limit running with nothing queued (a call's tail): the
/// first baseline's window opens and takes the probing token. One fewer than the limit running,
/// nothing queued, is demand-limited (no token); one more job, queued, is at its limit too.
#[test]
fn exactly_its_limit_running_is_at_its_limit() {
    for (n, at_limit) in [(3, true), (2, false), (4, true)] {
        let h = H::new();
        let dev = Arc::new(Device::default());
        let call = start(&h, jobs("/a", None, n, &dev, false), 3);
        wait_until(|| dev.waiting() == n.min(3), "the jobs to start");
        ticks(&h, 3);
        assert_eq!(learned(&h, "/a").probing, at_limit, "{n} jobs at limit 3: {:?}", learned(&h, "/a"));
        dev.finish(usize::MAX / 2);
        assert_eq!(super::tests::ok_count(&call.join()), n);
    }
}

/// While a lowered limit drains (more running than the new limit), no window runs: a
/// down-probe's window opens only once the running jobs are down to its limit, so the time spent
/// draining isn't measured as the lower limit's.
#[test]
fn a_lowered_limit_measures_nothing_until_it_drains() {
    let h = H::new();
    let dev = Arc::new(Device::default());
    let call = start(&h, jobs("/a", None, 2000, &dev, false), 2);
    let draining = || {
        let l = learned(&h, "/a");
        l.limit == 1 && l.probing && dev.waiting() == 2
    };
    let finish_one = || {
        let done = call.done.load(Ordering::SeqCst);
        dev.finish(1);
        wait_until(|| call.done.load(Ordering::SeqCst) > done, "the finished job's outcome");
    };
    // one job a half second whatever the limit: the up-probe fails, then the down-probe (2 -> 1)
    // begins with two running
    let mut found = false;
    for _ in 0..300 {
        ticks(&h, 2);
        if draining() {
            found = true;
            break;
        }
        let limit = learned(&h, "/a").limit.max(1);
        wait_until(|| dev.waiting() >= limit, "the volume's slots to fill");
        finish_one();
        ticks(&h, 2);
        if draining() {
            found = true;
            break;
        }
        h.advance(SEC / 2);
    }
    assert!(found, "a down-probe began with two running: {:?}", learned(&h, "/a"));
    let before = learned(&h, "/a").windows;
    // nothing finishes for 5 s (draining), then eight jobs finish at once at the probe's limit
    for _ in 0..10 {
        h.advance(SEC / 2);
        ticks(&h, 2);
    }
    for _ in 0..8 {
        finish_one();
        wait_until(|| dev.waiting() >= 1, "the probe's slot to fill");
        ticks(&h, 2);
    }
    assert_eq!(learned(&h, "/a").windows, before, "the drain was measured: {:?}", learned(&h, "/a"));
    dev.finish(usize::MAX / 2);
    assert_eq!(super::tests::ok_count(&call.join()), 2000);
}

/// A destination that its card can't fill (every copy into it waits for the card's slots, not
/// the destination's) is not at its limit: its up-probe ends as demand-limited instead of being
/// judged, and it never steps down to what the card feeds it (it went to 1, so the card's copies
/// then ran one at a time).
#[test]
fn a_destination_its_card_cant_fill_is_not_at_its_limit() {
    let h = H::new();
    let dev = Arc::new(Device::default());
    let call = start(&h, jobs("/a", Some("/b"), 2000, &dev, false), 2);
    for _ in 0..80 {
        step_on(&h, &call, &dev, &["/a", "/b"], |w| w.min(1));
    }
    let b = learned(&h, "/b");
    assert!(b.windows >= 2, "the destination was measured at its limit: {b:?}");
    assert!(b.limit >= 2, "it kept a step down to what its card feeds it: {b:?}");
    dev.finish(usize::MAX / 2);
    assert_eq!(super::tests::ok_count(&call.join()), 2000);
}

/// A destination that two cards copy into keeps at least 2 running (PR 1's cap): here one copy
/// finishes each half second whatever the limit, so a step down to 1 costs nothing measurable,
/// but a hung card's copy would then hold the destination's only slot.
#[test]
fn a_destination_of_two_cards_keeps_at_least_two() {
    let h = H::new();
    let dev = Arc::new(Device::default());
    let mut copies = jobs("/a", Some("/c"), 1000, &dev, false);
    copies.extend(jobs("/b", Some("/c"), 1000, &dev, false));
    let call = start(&h, copies, 2);
    for _ in 0..80 {
        step_on(&h, &call, &dev, &["/c"], |w| w.min(1));
    }
    let c = learned(&h, "/c");
    assert!(c.windows >= 4, "an up-probe was judged and a down step tried: {c:?}");
    assert!(c.limit >= 2, "{c:?}");
    dev.finish(usize::MAX / 2);
    assert_eq!(super::tests::ok_count(&call.join()), 2000);
}

/// A window whose jobs wait at the work gate (memory, not the volume, holds them up) is discarded:
/// the limit stays.
#[test]
fn gate_waits_discard_the_windows() {
    let h = H::new();
    let dev = Arc::new(Device::default());
    let call = start(&h, jobs("/a", None, 400, &dev, true), 2);
    for _ in 0..30 {
        step(&h, &call, &dev, |w| w);
    }
    let l = learned(&h, "/a");
    assert!(l.discarded >= 1 && l.windows == 0, "{l:?}");
    assert_eq!(l.limit, 2);
    dev.finish(usize::MAX / 2);
    assert_eq!(super::tests::ok_count(&call.join()), 400);
}

/// Jobs that finish without moving a byte measure nothing: the volume keeps its start however
/// many windows' worth of them finish at its limit (on `0 ≥ 0` it climbed to the cap).
#[test]
fn jobs_that_move_no_bytes_leave_the_limit() {
    let h = H::new();
    let dev = Arc::new(Device::default());
    let call = start(&h, empty_jobs("/a", 2000, &dev), 2);
    for _ in 0..60 {
        step(&h, &call, &dev, |w| w);
    }
    let l = learned(&h, "/a");
    assert_eq!((l.limit, l.raised, l.windows), (2, 0, 0), "{l:?}");
    assert!(l.discarded >= 1, "{l:?}");
    assert!(dev.most.load(Ordering::SeqCst) <= 2, "never above the start");
    dev.finish(usize::MAX / 2);
    assert_eq!(super::tests::ok_count(&call.join()), 2000);
}

/// A window open across a wake from sleep (detected by a late step, where the monotonic clock
/// counts the sleep, as on Windows) measured the sleep: it is dropped and starts again. The same
/// 10 s gap with no wake detected completes the window (six jobs in 3 s, the gap, two more).
#[test]
fn a_wake_drops_the_open_windows() {
    for wake in [true, false] {
        let mut h = H::new();
        if let Some(ctx) = Arc::get_mut(&mut h.ctx) {
            ctx.late_tick_is_wake = wake;
        }
        assert_eq!(h.ctx.late_tick_is_wake, wake, "the context is not shared yet");
        let dev = Arc::new(Device::default());
        let call = start(&h, jobs("/a", None, 2000, &dev, false), 2);
        let finish_one = || {
            wait_until(|| dev.waiting() >= 2, "the volume's slots to fill");
            let done = call.done.load(Ordering::SeqCst);
            dev.finish(1);
            wait_until(|| call.done.load(Ordering::SeqCst) > done, "the finished job's outcome");
        };
        // the first baseline's window opens; six of its eight jobs finish in 3 s
        wait_until(|| dev.waiting() >= 2, "the volume's slots to fill");
        ticks(&h, 2);
        for _ in 0..6 {
            finish_one();
            ticks(&h, 2);
            h.advance(SEC / 2);
        }
        // one step 10 s late (under the 30 s stall time), then the last two jobs
        h.advance(10 * SEC);
        ticks(&h, 3);
        for _ in 0..2 {
            finish_one();
        }
        ticks(&h, 3);
        let l = learned(&h, "/a");
        assert_eq!(l.windows, if wake { 0 } else { 1 }, "wake detected {wake}: {l:?}");
        dev.finish(usize::MAX / 2);
        assert_eq!(super::tests::ok_count(&call.join()), 2000);
    }
}

/// A call too short to complete a window leaves its volume as it was, and the pool's limit for
/// a later call is still the calls' `workers()`.
#[test]
fn a_short_call_leaves_the_limit() {
    let h = H::new();
    let dev = Arc::new(Device::default());
    let call = start(&h, jobs("/a", None, 6, &dev, false), 2);
    for _ in 0..3 {
        step(&h, &call, &dev, |w| w);
    }
    assert_eq!(super::tests::ok_count(&call.join()), 6);
    let l = learned(&h, "/a");
    assert_eq!((l.limit, l.raised, l.windows, l.probing), (2, 0, 0, false), "{l:?}");
}

/// The profile lines (`<PREFIX>_PROFILE`) name the volume and the limit, and say what was decided.
#[test]
fn profile_lines_say_what_was_decided() {
    use super::limits::{AbortReason, Event, Phase};
    use super::sched::describe;
    let v = key("/a/x");
    let lines = [
        describe(&Event::Window { vol: v.clone(), phase: Phase::Base1, limit: 8, rate: 120e6, total: 600e6, span: 4.2, jobs: 16 }),
        describe(&Event::Decided {
            vol: v.clone(),
            up: true,
            from: 8,
            to: 10,
            kept: true,
            base: 100e6,
            probe: 130e6,
            base_total: 500e6,
            probe_total: 530e6,
        }),
        describe(&Event::Decided {
            vol: v.clone(),
            up: false,
            from: 8,
            to: 4,
            kept: false,
            base: 100e6,
            probe: 60e6,
            base_total: 500e6,
            probe_total: 460e6,
        }),
        describe(&Event::Aborted { vol: v.clone(), reason: AbortReason::Demand, limit: 8 }),
        describe(&Event::Hold { vol: v.clone(), limit: 8, windows: 16 }),
    ];
    assert!(lines.iter().all(|l| l.starts_with("io-limit /a: ")), "{lines:?}");
    assert!(lines[0].contains("at limit 8") && lines[0].contains("120.0 MB/s") && lines[0].contains("process 600.0 MB/s"), "{}", lines[0]);
    assert!(lines[1].contains("up 8 -> 10 kept") && lines[1].ends_with("limit now 10"), "{}", lines[1]);
    assert!(lines[2].contains("down 8 -> 4 not kept") && lines[2].ends_with("limit now 8"), "{}", lines[2]);
    assert!(lines[3].contains("ran out of work") && lines[3].ends_with("limit 8"), "{}", lines[3]);
    assert!(lines[4].contains("holds at limit 8 for 16 windows"), "{}", lines[4]);
}
