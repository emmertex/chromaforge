//! Test harness: the spec's fluid model of devices, links and admission (§5.7.1), translated from
//! the spec's Python simulator (not in this repository: functions `simulate`, `jobs`, `flat_after`,
//! `hdd`, `scenarios`, `noise_scenarios` and `wave`), read as a reference. The controller under test is the product's ([`super::limits::Controller`]).
//!
//! The scenarios' job sizes and noise come from Python's `random.Random` (MT19937 with Python's
//! seeding: an int seed's 32-bit words; a str seed's bytes followed by their SHA-512, as one
//! big-endian integer), ported here so both programs see the same inputs; the stepping keeps the
//! simulator's order of floating-point operations, so the two can be compared run for run.

use std::collections::VecDeque;

use sha2::{Digest, Sha512};

use super::limits::{Controller, Counters, Demand, IO_CAP, cut, demand, under_cap};

pub(crate) const DT: f64 = 0.05;
const CORES: f64 = 16.0;
const HASH_MBPS: f64 = 600.0;

// ------------------------------------------------------------------ Python's random.Random

/// Python's `random.Random`: MT19937, Python's seeding and `random()`.
pub(crate) struct PyRandom {
    mt: [u32; 624],
    idx: usize,
}

impl PyRandom {
    fn genrand(s: u32) -> PyRandom {
        let mut mt = [0u32; 624];
        mt[0] = s;
        for i in 1..624 {
            mt[i] = 1_812_433_253u32.wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_add(i as u32);
        }
        PyRandom { mt, idx: 624 }
    }

    fn by_array(key: &[u32]) -> PyRandom {
        let mut r = PyRandom::genrand(19_650_218);
        let (n, mt) = (624usize, &mut r.mt);
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..n.max(key.len()) {
            mt[i] = (mt[i] ^ (mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1_664_525)).wrapping_add(key[j]).wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= n {
                mt[0] = mt[n - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..n - 1 {
            mt[i] = (mt[i] ^ (mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1_566_083_941)).wrapping_sub(i as u32);
            i += 1;
            if i >= n {
                mt[0] = mt[n - 1];
                i = 1;
            }
        }
        mt[0] = 0x8000_0000;
        r
    }

    /// A big integer given as little-endian bytes: its 32-bit words, least significant first.
    fn key_le(bytes: &[u8]) -> Vec<u32> {
        let mut words: Vec<u32> = bytes
            .chunks(4)
            .map(|c| {
                let mut w = [0u8; 4];
                w[..c.len()].copy_from_slice(c);
                u32::from_le_bytes(w)
            })
            .collect();
        while words.len() > 1 && words.last() == Some(&0) {
            words.pop();
        }
        if words.is_empty() {
            words.push(0);
        }
        words
    }

    /// `random.Random(n)`.
    pub(crate) fn from_int(n: u64) -> PyRandom {
        PyRandom::by_array(&PyRandom::key_le(&n.to_le_bytes()))
    }

    /// `random.Random(s)` (version 2 seeding).
    pub(crate) fn from_str(s: &str) -> PyRandom {
        let mut be = s.as_bytes().to_vec();
        be.extend_from_slice(&Sha512::digest(s.as_bytes()));
        be.reverse();
        PyRandom::by_array(&PyRandom::key_le(&be))
    }

    fn next_u32(&mut self) -> u32 {
        if self.idx >= 624 {
            for k in 0..624 {
                let y = (self.mt[k] & 0x8000_0000) | (self.mt[(k + 1) % 624] & 0x7fff_ffff);
                self.mt[k] = self.mt[(k + 397) % 624] ^ (y >> 1) ^ if y & 1 == 1 { 0x9908_b0df } else { 0 };
            }
            self.idx = 0;
        }
        let mut y = self.mt[self.idx];
        self.idx += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    pub(crate) fn random(&mut self) -> f64 {
        let a = self.next_u32() >> 5;
        let b = self.next_u32() >> 6;
        (f64::from(a) * 67_108_864.0 + f64::from(b)) * (1.0 / 9_007_199_254_740_992.0)
    }

    pub(crate) fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.random()
    }
}

// ------------------------------------------------------------------ the world

/// A device's throughput (MB/s) over concurrent streams.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Curve {
    /// Linear to `peak` streams at `per` MB/s each, flat after (an SSD, a card, a pool).
    Flat { peak: usize, per: f64 },
    /// Best at `best` streams, `top` MB/s; each stream past it costs 15 % (seeks).
    Hdd { best: usize, top: f64 },
}

impl Curve {
    fn at(self, n: usize) -> f64 {
        match self {
            Curve::Flat { peak, per } => per * n.min(peak) as f64,
            Curve::Hdd { best, top } => top * (n.min(best) as f64 / best as f64) * 0.85f64.powf(n.saturating_sub(best) as f64),
        }
    }
}

pub(crate) fn flat_after(peak: usize, per: f64) -> Curve {
    Curve::Flat { peak, per }
}

pub(crate) fn hdd(best: usize, top: f64) -> Curve {
    Curve::Hdd { best, top }
}

#[derive(Clone, Debug)]
pub(crate) struct Device {
    pub(crate) name: String,
    pub(crate) curve: Curve,
    pub(crate) link: Option<String>,
    pub(crate) slow_at: Option<f64>,
    pub(crate) slow_factor: f64,
    pub(crate) change_at: Option<f64>,
    pub(crate) curve2: Option<Curve>,
    pub(crate) noise: f64,
    pub(crate) noise_period: f64,
    pub(crate) wave_period: f64,
    pub(crate) wave_lo: f64,
    pub(crate) wave_phase: f64,
}

pub(crate) fn device(name: &str, curve: Curve) -> Device {
    Device {
        name: name.to_string(),
        curve,
        link: None,
        slow_at: None,
        slow_factor: 1.0,
        change_at: None,
        curve2: None,
        noise: 0.0,
        noise_period: 4.0,
        wave_period: 0.0,
        wave_lo: 1.0,
        wave_phase: 0.0,
    }
}

/// A job: MB to read, latency (s).
pub(crate) type Job = (f64, f64);

#[derive(Clone, Debug)]
pub(crate) struct Volume {
    /// (as the simulator names it; for reading)
    #[allow(dead_code)]
    pub(crate) name: String,
    pub(crate) device: usize,
    pub(crate) jobs: Vec<Job>,
}

#[derive(Clone, Debug)]
pub(crate) struct Scenario {
    pub(crate) name: String,
    pub(crate) devices: Vec<Device>,
    pub(crate) links: Vec<(String, f64)>,
    pub(crate) volumes: Vec<Volume>,
    /// Calls arriving over time: (start, [(volume, jobs)]).
    pub(crate) calls: Vec<(f64, Vec<(usize, Vec<Job>)>)>,
    pub(crate) fixed_best: Vec<usize>,
}

/// `jobs(n, size, lat, jitter=0.2, seed)`.
pub(crate) fn jobs(n: usize, size: f64, lat: f64, seed: u64) -> Vec<Job> {
    let mut rnd = PyRandom::from_int(seed);
    (0..n).map(|_| (size * (1.0 + rnd.uniform(-0.2, 0.2)), lat)).collect()
}

/// What decides each volume's limit.
pub(crate) trait Policy {
    fn limit(&self, v: usize) -> usize;
    fn tick(&mut self, now: f64, v: usize, demand: Demand, bytes: f64, total: f64, jobs: u64);
}

/// The product's controller.
pub(crate) struct Adaptive(pub(crate) Controller<usize>);

impl Adaptive {
    pub(crate) fn new(start: usize) -> Adaptive {
        Adaptive(Controller::new(IO_CAP, start))
    }
}

impl Policy for Adaptive {
    fn limit(&self, v: usize) -> usize {
        self.0.limit(&v)
    }

    fn tick(&mut self, now: f64, v: usize, demand: Demand, bytes: f64, total: f64, jobs: u64) {
        self.0.tick(now, &v, demand, Counters { bytes, jobs, busy: 0.0, gate: 0.0 }, total);
    }
}

/// A fixed limit.
pub(crate) struct Fixed(pub(crate) usize);

impl Policy for Fixed {
    fn limit(&self, _: usize) -> usize {
        self.0
    }

    fn tick(&mut self, _: f64, _: usize, _: Demand, _: f64, _: f64, _: u64) {}
}

/// Run `sc` under `policy` until every job is done or `t_max`; the completion time.
pub(crate) fn simulate(sc: &Scenario, policy: &mut dyn Policy, t_max: f64, noise_seed: u64) -> f64 {
    let nv = sc.volumes.len();
    let mut noise_rng: Vec<PyRandom> = sc.devices.iter().map(|d| PyRandom::from_str(&format!("{noise_seed}:{}", d.name))).collect();
    let mut noise_f = vec![1.0f64; sc.devices.len()];
    let mut noise_next = vec![0.0f64; sc.devices.len()];
    let mut queue: Vec<VecDeque<Job>> = sc.volumes.iter().map(|v| v.jobs.iter().copied().collect()).collect();
    // [remaining MB, latency left]
    let mut running: Vec<Vec<[f64; 2]>> = vec![Vec::new(); nv];
    let mut done_bytes = vec![0.0f64; nv];
    let mut done_jobs = vec![0u64; nv];
    let mut total = 0.0f64;
    let mut t = 0.0f64;
    let mut rr: Vec<usize> = (0..nv).collect();
    let mut pending: VecDeque<(f64, Vec<(usize, Vec<Job>)>)> = {
        let mut c = sc.calls.clone();
        c.sort_by(|a, b| a.0.total_cmp(&b.0));
        c.into()
    };
    // devices in first-appearance order over the volumes (the simulator's `streams` dict)
    let mut dev_order: Vec<usize> = Vec::new();
    for v in &sc.volumes {
        if !dev_order.contains(&v.device) {
            dev_order.push(v.device);
        }
    }
    let mut rate: Vec<Vec<f64>> = vec![Vec::new(); nv];
    while t < t_max {
        while pending.front().is_some_and(|c| c.0 <= t) {
            if let Some((_, add)) = pending.pop_front() {
                for (v, js) in add {
                    queue[v].extend(js);
                }
            }
        }
        if pending.is_empty() && (0..nv).all(|v| queue[v].is_empty() && running[v].is_empty()) {
            break;
        }
        // admission, round-robin, under the product's limit (the I/O cap, R4a's cut: the
        // simulator's "another volume is busy", as its jobs each need one volume)
        let mut n_run: usize = running.iter().map(Vec::len).sum();
        let active: Vec<usize> = (0..nv).map(|v| queue[v].len() + running[v].len()).collect();
        let total_jobs: usize = active.iter().sum();
        let lim_of = |policy: &dyn Policy, v: usize| under_cap(policy.limit(v), IO_CAP, cut(active[v], total_jobs));
        let mut progressed = true;
        while progressed && n_run < IO_CAP {
            progressed = false;
            for v in rr.clone() {
                let lim = lim_of(&*policy, v);
                if !queue[v].is_empty() && running[v].len() < lim && n_run < IO_CAP {
                    if let Some((size, lat)) = queue[v].pop_front() {
                        running[v].push([size, lat]);
                    }
                    n_run += 1;
                    progressed = true;
                    rr.retain(|&x| x != v);
                    rr.push(v);
                }
            }
        }
        // a window counts only while the volume runs at its limit (the product's rule; a queued
        // job here waits for the I/O cap alone)
        let demand: Vec<Demand> = (0..nv).map(|v| demand(running[v].len(), lim_of(&*policy, v), !queue[v].is_empty())).collect();
        for (d, dev) in sc.devices.iter().enumerate() {
            if dev.noise != 0.0 && t >= noise_next[d] - 1e-9 {
                noise_f[d] = 1.0 + noise_rng[d].uniform(-dev.noise, dev.noise);
                noise_next[d] += dev.noise_period;
            }
        }
        // rates: device share, then link cap, then CPU
        for v in 0..nv {
            rate[v].clear();
            rate[v].resize(running[v].len(), 0.0);
        }
        // (streams per device, in volume order: (volume, job))
        let streams: Vec<(usize, Vec<(usize, usize)>)> = dev_order
            .iter()
            .map(|&d| {
                let ss = (0..nv)
                    .filter(|&v| sc.volumes[v].device == d)
                    .flat_map(|v| running[v].iter().enumerate().filter(|(_, r)| r[1] <= 0.0).map(move |(j, _)| (v, j)))
                    .collect();
                (d, ss)
            })
            .collect();
        let mut active = 0usize;
        for (d, ss) in &streams {
            let n = ss.len();
            if n == 0 {
                continue;
            }
            let dev = &sc.devices[*d];
            let curve = match (dev.change_at, dev.curve2) {
                (Some(at), Some(c2)) if t >= at => c2,
                _ => dev.curve,
            };
            let mut cap = curve.at(n) * if dev.slow_at.is_some_and(|at| t >= at) { dev.slow_factor } else { 1.0 };
            cap *= noise_f[*d];
            if dev.wave_period != 0.0 && (((t + dev.wave_phase) / dev.wave_period) as i64) % 2 == 1 {
                cap *= dev.wave_lo;
            }
            for &(v, j) in ss {
                rate[v][j] = cap / n as f64;
            }
            active += n;
        }
        for (lk, lcap) in &sc.links {
            let on: Vec<(usize, usize)> =
                streams.iter().filter(|(d, _)| sc.devices[*d].link.as_deref() == Some(lk.as_str())).flat_map(|(_, ss)| ss.iter().copied()).collect();
            let mut s = 0.0f64;
            for &(v, j) in &on {
                s += rate[v][j];
            }
            if s > *lcap {
                for &(v, j) in &on {
                    rate[v][j] *= lcap / s;
                }
            }
        }
        let cpu_share = (CORES / active.max(1) as f64).min(1.0);
        for (d, ss) in &streams {
            let _ = d;
            for &(v, j) in ss {
                rate[v][j] = rate[v][j].min(HASH_MBPS * cpu_share);
            }
        }
        // advance
        for v in 0..nv {
            let rs = std::mem::take(&mut running[v]);
            let mut keep = Vec::with_capacity(rs.len());
            for (j, mut r) in rs.into_iter().enumerate() {
                if r[1] > 0.0 {
                    r[1] -= DT;
                    keep.push(r);
                    continue;
                }
                let mb = r[0].min(rate[v].get(j).copied().unwrap_or(0.0) * DT);
                r[0] -= mb;
                done_bytes[v] += mb;
                total += mb;
                if r[0] <= 1e-9 {
                    done_jobs[v] += 1;
                } else {
                    keep.push(r);
                }
            }
            running[v] = keep;
        }
        t += DT;
        for v in 0..nv {
            policy.tick(t, v, demand[v], done_bytes[v], total, done_jobs[v]);
        }
    }
    t
}

// ------------------------------------------------------------------ scenarios

fn one(name: &str, dev: Device, vol: &str, jobs: Vec<Job>, fixed_best: &[usize]) -> Scenario {
    Scenario {
        name: name.to_string(),
        devices: vec![dev],
        links: Vec::new(),
        volumes: vec![Volume { name: vol.to_string(), device: 0, jobs }],
        calls: Vec::new(),
        fixed_best: fixed_best.to_vec(),
    }
}

fn nas(noise: f64) -> (Vec<Device>, Vec<Volume>) {
    let devs = (0..6).map(|i| Device { link: Some("nic".into()), noise, ..device(&format!("box{i}"), flat_after(4, 75.0)) }).collect();
    let vols = (0..6).map(|i| Volume { name: format!("nas{i}"), device: i, jobs: jobs(3000, 25.0, 0.02, i as u64) }).collect();
    (devs, vols)
}

fn cards(noise: f64) -> (Vec<Device>, Vec<Volume>) {
    let devs = vec![
        Device { noise, ..device("cfe1", flat_after(4, 400.0)) },
        Device { noise, ..device("cfe2", flat_after(4, 400.0)) },
        Device { noise, ..device("xqd", flat_after(2, 200.0)) },
        Device { noise, ..device("sd", flat_after(1, 90.0)) },
    ];
    let vols = vec![
        Volume { name: "cfe1".into(), device: 0, jobs: jobs(1500, 55.0, 0.0, 1) },
        Volume { name: "cfe2".into(), device: 1, jobs: jobs(1500, 55.0, 0.0, 2) },
        Volume { name: "xqd".into(), device: 2, jobs: jobs(1200, 45.0, 0.0, 3) },
        Volume { name: "sd".into(), device: 3, jobs: jobs(800, 25.0, 0.0, 4) },
    ];
    (devs, vols)
}

/// The simulator's `scenarios()`, in its order.
// (one push per scenario, in the simulator's order)
#[allow(clippy::vec_init_then_push)]
pub(crate) fn scenarios() -> Vec<Scenario> {
    let mut out = Vec::new();
    out.push(one("a: SSD linear to 8, flat after", device("d", flat_after(8, 120.0)), "ssd", jobs(6000, 50.0, 0.0, 1), &[4, 8, 16, 32]));
    out.push(one("b: HDD best at 1", device("d", hdd(1, 200.0)), "hdd", jobs(2500, 30.0, 0.0, 1), &[1, 2, 8]));
    out.push(one("b2: HDD best at 2", device("d", hdd(2, 200.0)), "hdd", jobs(2500, 30.0, 0.0, 1), &[1, 2, 8]));
    out.push(one("z: pool linear to 30", device("d", flat_after(30, 150.0)), "pool", jobs(30000, 40.0, 0.0, 1), &[8, 16, 32]));
    let (devices, volumes) = nas(0.0);
    out.push(Scenario {
        name: "c: six NAS volumes, one 1250 MB/s link".into(),
        devices,
        links: vec![("nic".into(), 1250.0)],
        volumes,
        calls: Vec::new(),
        fixed_best: vec![2, 4, 8],
    });
    out.push(Scenario {
        name: "c2: two volumes on one HDD best at 2".into(),
        devices: vec![device("d", hdd(2, 200.0))],
        links: Vec::new(),
        volumes: vec![
            Volume { name: "p1".into(), device: 0, jobs: jobs(1500, 30.0, 0.0, 1) },
            Volume { name: "p2".into(), device: 0, jobs: jobs(1500, 30.0, 0.0, 2) },
        ],
        calls: Vec::new(),
        fixed_best: vec![1, 2, 8],
    });
    out.push(one(
        "d: SSD halves its speed at 150 s",
        Device { slow_at: Some(150.0), slow_factor: 0.5, ..device("d", flat_after(8, 120.0)) },
        "ssd",
        jobs(6000, 50.0, 0.0, 1),
        &[8],
    ));
    let mut mix = jobs(4000, 3.0, 0.005, 1);
    mix.extend(jobs(1500, 50.0, 0.005, 3));
    out.push(one("e: card, 3 MB then 50 MB files", device("d", flat_after(4, 200.0)), "card", mix.clone(), &[4, 8]));
    mix.reverse();
    out.push(one("e2: card, 50 MB then 3 MB files", device("d", flat_after(4, 200.0)), "card", mix, &[4, 8]));
    let mut f = one("f: short calls (16 jobs / 5 s)", device("d", flat_after(8, 120.0)), "ssd", Vec::new(), &[1, 8]);
    f.calls = (0..40u64).map(|i| (i as f64 * 5.0, vec![(0, jobs(16, 50.0, 0.0, i))])).collect();
    out.push(f);
    let (devices, volumes) = cards(0.0);
    out.push(Scenario { name: "g: four cards".into(), devices, links: Vec::new(), volumes, calls: Vec::new(), fixed_best: vec![1, 4, 8] });
    out.push(one(
        "M4: pool at the cap degrades to best-at-4 at 100 s",
        Device { change_at: Some(100.0), curve2: Some(hdd(4, 400.0)), ..device("d", flat_after(30, 150.0)) },
        "pool",
        jobs(30000, 40.0, 0.0, 1),
        &[4, 32],
    ));
    out.push(Scenario {
        name: "D2: short SSD call ends mid-probe, then HDD".into(),
        devices: vec![device("s", flat_after(8, 120.0)), device("h", hdd(1, 200.0))],
        links: Vec::new(),
        volumes: vec![Volume { name: "ssd".into(), device: 0, jobs: Vec::new() }, Volume { name: "hdd".into(), device: 1, jobs: Vec::new() }],
        calls: vec![(0.0, vec![(0, jobs(40, 50.0, 0.0, 1))]), (20.0, vec![(1, jobs(2500, 30.0, 0.0, 1))])],
        fixed_best: vec![1, 8],
    });
    out.push(one("S2: HDD best at 1, 2 GB clips", device("d", hdd(1, 200.0)), "hdd", jobs(250, 2000.0, 0.0, 1), &[1, 8]));
    out.push(one(
        "W: HDD best at 1, bursts of another reader (8 s)",
        Device { wave_period: 8.0, wave_lo: 0.3, ..device("d", hdd(1, 200.0)) },
        "hdd",
        jobs(2500, 30.0, 0.0, 1),
        &[1, 2, 8],
    ));
    out
}

/// The simulator's `noise_scenarios()` (±20 %), in its order.
// (one push per scenario, in the simulator's order)
#[allow(clippy::vec_init_then_push)]
pub(crate) fn noise_scenarios() -> Vec<Scenario> {
    let n = 0.2;
    let mut out = Vec::new();
    out.push(one("Na: SSD a, noise 20%/4 s", Device { noise: n, ..device("d", flat_after(8, 120.0)) }, "ssd", jobs(6000, 50.0, 0.0, 1), &[4, 8, 16]));
    out.push(one(
        "Na1: SSD a, noise 20%/1 s",
        Device { noise: n, noise_period: 1.0, ..device("d", flat_after(8, 120.0)) },
        "ssd",
        jobs(6000, 50.0, 0.0, 1),
        &[4, 8, 16],
    ));
    out.push(one("Nb: HDD best at 1, noise 20%/4 s", Device { noise: n, ..device("d", hdd(1, 200.0)) }, "hdd", jobs(2500, 30.0, 0.0, 1), &[1, 2, 8]));
    out.push(one(
        "Nz: pool linear to 30, noise 20%/4 s",
        Device { noise: n, ..device("d", flat_after(30, 150.0)) },
        "pool",
        jobs(30000, 40.0, 0.0, 1),
        &[16, 32],
    ));
    let (devices, volumes) = nas(n);
    out.push(Scenario {
        name: "Nc: six NAS volumes, noise 20%/4 s".into(),
        devices,
        links: vec![("nic".into(), 1250.0)],
        volumes,
        calls: Vec::new(),
        fixed_best: vec![2, 4, 8],
    });
    let (devices, volumes) = cards(n);
    out.push(Scenario {
        name: "Ng: four cards, noise 20%/4 s".into(),
        devices,
        links: Vec::new(),
        volumes,
        calls: Vec::new(),
        fixed_best: vec![1, 4, 8],
    });
    out
}

/// The simulator's `wave`: a hard drive best at 1 with another reader in bursts.
pub(crate) fn wave(period: f64, phase: f64) -> Scenario {
    one("wave", Device { wave_period: period, wave_lo: 0.3, wave_phase: phase, ..device("d", hdd(1, 200.0)) }, "hdd", jobs(2500, 30.0, 0.0, 1), &[1])
}
