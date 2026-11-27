//! The controller against the spec's simulator (§7 "Controller (PR 5)"): the Rust port of the
//! fluid model ([`super::sim`]) runs the product's controller ([`super::limits`]) on every
//! scenario of §5.7.1 from starts 8, 4 and 1, and the results are compared with the Python
//! simulator's run for run ([`super::sim_reference`]: `io_controller_sim.py` and
//! `io_controller_seeds.py`, `python3 -I`).
//!
//! Tolerances: every run's completion time equals Python's within 1e-9 (relative; the port keeps
//! the order of floating-point operations, so in practice they are identical), and every settled
//! limit is the same. The spec's own bounds (§7) are checked separately, so a later change that
//! breaks parity on purpose still has them: times no worse than the table's by more than 10 %,
//! settled limits within one step, and the named properties of c2, M4, D2, c and f; noise means
//! and the interference sweep's mean and worst case no worse than §5.7.1's tables by 10 %.

use super::limits::IO_CAP;
use super::limits::{Controller, Counters, Demand};
use super::sim::{Adaptive, Fixed, PyRandom, Scenario, noise_scenarios, scenarios, simulate, wave};
use super::sim_reference::{CLEAN, CLEAN_FIXED, NOISE, NOISE_FIXED, WAVE};

const T_MAX: f64 = 20000.0;
/// The simulator's default noise seed (unused by the noise-free scenarios).
const SEED: u64 = 7;
/// Relative tolerance for a run against Python's.
const SAME: f64 = 1e-9;

fn adaptive(sc: &Scenario, start: usize, seed: u64) -> (f64, Vec<usize>, Controller<usize>) {
    let mut a = Adaptive::new(start);
    let t = simulate(sc, &mut a, T_MAX, seed);
    let limits = (0..sc.volumes.len()).map(|v| a.0.limit(&v)).collect();
    (t, limits, a.0)
}

/// After a run: one more tick, every volume out of work (the simulator stops at the last job,
/// before the controller sees the volumes idle).
fn ends(ctl: &mut Controller<usize>, t: f64, volumes: usize) {
    for v in 0..volumes {
        ctl.tick(t + super::sim::DT, &v, Demand::Limited, Counters::default(), 0.0);
    }
}

fn same(rust: f64, py: f64) -> bool {
    ((rust - py) / py).abs() <= SAME
}

/// Within one step of `l` (down to `l − max(1, l/2)`, up to `l + ceil(l/4)`).
fn within_one_step(got: usize, l: usize) -> bool {
    let lo = l.saturating_sub((l / 2).max(1)).max(1);
    let hi = (l + l.div_ceil(4)).min(IO_CAP);
    (lo..=hi).contains(&got)
}

fn best_fixed(i: usize) -> f64 {
    CLEAN_FIXED.iter().filter(|r| r.0 == i).map(|r| r.2).fold(f64::INFINITY, f64::min)
}

#[test]
fn pythons_random_is_reproduced() {
    let firsts = [
        (0, 0.8444218515250481, 0.10318176117612099),
        (1, 0.13436424411240122, 0.13897349477489307),
        (2, 0.9560342718892494, 0.1791309948237398),
        (3, 0.23796462709189137, 0.017691690118380732),
        (4, 0.23604808973743452, -0.1587335863077137),
        (5, 0.6229016948897019, 0.09671479570429176),
    ];
    for (seed, a, b) in firsts {
        let mut r = PyRandom::from_int(seed);
        assert_eq!((r.random(), r.uniform(-0.2, 0.2)), (a, b), "seed {seed}");
    }
    for (seed, a, b) in [
        ("1:d", 0.831162386138837, 0.07608678747677078),
        ("5:box3", 0.9601063961027709, 0.7993016810272504),
        ("3:cfe1", 0.10263174631128891, 0.6268454669179911),
    ] {
        let mut r = PyRandom::from_str(seed);
        assert_eq!((r.random(), r.random()), (a, b), "seed {seed}");
    }
    let mut r = PyRandom::from_int(1);
    for _ in 0..1000 {
        r.random();
    }
    assert_eq!(r.random(), 0.4116430517162146, "past one twist");
}

#[test]
fn clean_scenarios_match_the_python_simulator() {
    let sc = scenarios();
    assert_eq!(sc.len(), 15);
    let mut bad = Vec::new();
    for &(i, start, py, py_limits) in CLEAN {
        let (t, limits, _) = adaptive(&sc[i], start, SEED);
        if !same(t, py) || limits != py_limits {
            bad.push(format!("{} from {start}: {t} s {limits:?}, Python {py} s {py_limits:?}", sc[i].name));
        }
    }
    for &(i, n, py) in CLEAN_FIXED {
        let t = simulate(&sc[i], &mut Fixed(n), T_MAX, SEED);
        if !same(t, py) {
            bad.push(format!("{} fixed {n}: {t} s, Python {py} s", sc[i].name));
        }
    }
    assert!(bad.is_empty(), "runs differ from Python's:\n{}", bad.join("\n"));
}

#[test]
fn noise_scenarios_match_the_python_simulator() {
    let sc = noise_scenarios();
    let mut bad = Vec::new();
    for &(i, start, seed, py, py_limits) in NOISE {
        let (t, limits, _) = adaptive(&sc[i], start, seed);
        if !same(t, py) || limits != py_limits {
            bad.push(format!("{} from {start}, seed {seed}: {t} s {limits:?}, Python {py} s {py_limits:?}", sc[i].name));
        }
    }
    for &(i, n, seed, py) in NOISE_FIXED {
        let t = simulate(&sc[i], &mut Fixed(n), T_MAX, seed);
        if !same(t, py) {
            bad.push(format!("{} fixed {n}, seed {seed}: {t} s, Python {py} s", sc[i].name));
        }
    }
    assert!(bad.is_empty(), "runs differ from Python's:\n{}", bad.join("\n"));
}

#[test]
fn interference_sweep_matches_the_python_simulator() {
    let mut bad = Vec::new();
    for &(start, period, phase, py, py_limit) in WAVE {
        let sc = wave(period, phase);
        let (t, limit) = if start == 0 {
            (simulate(&sc, &mut Fixed(1), T_MAX, SEED), 0)
        } else {
            let (t, l, _) = adaptive(&sc, start, SEED);
            (t, l[0])
        };
        if !same(t, py) || limit != py_limit {
            bad.push(format!("period {period} phase {phase} from {start}: {t} s at {limit}, Python {py} s at {py_limit}"));
        }
    }
    assert!(bad.is_empty(), "runs differ from Python's:\n{}", bad.join("\n"));
}

/// §7: every §5.7.1 scenario from 8, 4 and 1 settles within one step of the table's limits, no
/// slower than the table by more than 10 %; c2 beats fixed 8; M4 comes down from the cap; D2
/// leaves no token held once the work ends; no NAS volume falls below a third of a fair share; short calls cost
/// nothing.
#[test]
fn clean_scenarios_keep_the_specs_bounds() {
    let sc = scenarios();
    for &(i, start, table_t, table_limits) in CLEAN {
        let (t, limits, mut ctl) = adaptive(&sc[i], start, SEED);
        ends(&mut ctl, t, limits.len());
        let name = &sc[i].name;
        assert!(t <= table_t * 1.10, "{name} from {start}: {t} s, table {table_t} s");
        for (got, want) in limits.iter().zip(table_limits) {
            assert!(within_one_step(*got, *want), "{name} from {start}: settled {limits:?}, table {table_limits:?}");
        }
        assert!(ctl.token().is_none(), "{name} from {start}: a token is still held once the work ended");
        match name.split(':').next() {
            Some("c2") => assert!(t < simulate(&sc[i], &mut Fixed(8), T_MAX, SEED), "c2 from {start} must beat fixed 8"),
            Some("M4") => {
                assert!(limits[0] < IO_CAP, "M4 from {start} must come down from the cap: {limits:?}");
                assert!(t < T_MAX, "M4 from {start} must finish");
            }
            Some("c") => {
                let fair = limits.iter().sum::<usize>() as f64 / limits.len() as f64;
                assert!(limits.iter().all(|&l| l as f64 >= fair / 3.0), "c from {start}: {limits:?} starves a volume");
            }
            // (the SSD's call ends mid-probe: the token must come back for the HDD to probe down)
            Some("D2") if start > 1 => assert!(limits[1] < start, "D2 from {start}: the HDD never probed: {limits:?}"),
            Some("f") => assert!(t <= best_fixed(i) * 1.02, "f from {start}: short calls cost {t} s against {}", best_fixed(i)),
            _ => {}
        }
    }
}

/// A short call that can't complete a window leaves the limit and the token as they were.
#[test]
fn a_call_too_short_to_measure_leaves_the_limit() {
    let mut sc = scenarios().swap_remove(0);
    sc.volumes[0].jobs.truncate(7);
    for start in [8, 4, 1] {
        let (t, limits, mut ctl) = adaptive(&sc, start, SEED);
        ends(&mut ctl, t, 1);
        assert_eq!(limits, vec![start]);
        assert!(ctl.token().is_none());
    }
}

/// §7 "Noise": each noise scenario's mean multiple of the best fixed limit over seeds 1–5 is no
/// worse than §5.7.1's table by more than 10 %, and a linear device still climbs.
#[test]
fn noise_keeps_the_specs_bounds() {
    // (§5.7.1 "Noise", mean multiples from starts 8, 4, 1)
    let table = [[1.05, 1.12, 1.19], [1.05, 1.10, 1.18], [1.09, 1.02, 1.00], [1.24, 1.33, 1.50], [1.02, 1.02, 1.44], [1.01, 1.03, 1.07]];
    let sc = noise_scenarios();
    for (i, s) in sc.iter().enumerate() {
        for (k, start) in [8usize, 4, 1].into_iter().enumerate() {
            let mut multiples = Vec::new();
            for seed in 1..=5u64 {
                let best = s.fixed_best.iter().map(|&n| simulate(s, &mut Fixed(n), T_MAX, seed)).fold(f64::INFINITY, f64::min);
                let (t, limits, _) = adaptive(s, start, seed);
                multiples.push(t / best);
                // (a linear device keeps climbing: the SSD from below its knee, the pool from every start)
                if (i <= 1 && start < 8) || i == 3 {
                    assert!(limits[0] > start, "{} from {start}, seed {seed}: settled at {limits:?}", s.name);
                }
            }
            let mean = multiples.iter().sum::<f64>() / multiples.len() as f64;
            assert!(mean <= table[i][k] * 1.10, "{} from {start}: mean {mean:.3} against the table's {}", s.name, table[i][k]);
        }
    }
}

/// §7 "Periodic interference": the sweep's mean and worst multiple of fixed 1 are no worse than
/// §5.7.1's v3 row by more than 10 %.
#[test]
fn interference_keeps_the_specs_bounds() {
    for (start, mean_max, worst_max) in [(4usize, 1.08, 1.31), (1, 1.03, 1.22)] {
        let mut multiples = Vec::new();
        for period in [4.0, 6.0, 8.0, 10.0, 12.0, 16.0, 20.0] {
            for phase in [0.0, 1.3, 2.7] {
                let sc = wave(period, phase);
                let fixed = simulate(&sc, &mut Fixed(1), T_MAX, SEED);
                let (t, _, _) = adaptive(&sc, start, SEED);
                multiples.push(t / fixed);
            }
        }
        let mean = multiples.iter().sum::<f64>() / multiples.len() as f64;
        let worst = multiples.iter().copied().fold(0.0, f64::max);
        assert!(mean <= mean_max * 1.10, "from {start}: mean {mean:.3}");
        assert!(worst <= worst_max * 1.10, "from {start}: worst {worst:.3}");
    }
}
