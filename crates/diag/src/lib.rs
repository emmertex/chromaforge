//! What was this process doing when it hung, was killed or panicked?
//!
//! A small diagnostics recorder (issue #1363): typed spans and events, a write-ahead in-flight file, a bounded ring
//! that evicts routine successes first, aggregate timings (count / total / min / max per name) for high-volume leaf
//! spans, and a text report. Off by default; nothing is sent anywhere. See `docs/diagnostics.md`.
//!
//! - **Top spans** are whole steps (the probe, the commit, a stage of it). Each is listed in `inflight-<pid>.json` while it
//!   runs. A process that hangs, is killed or runs out of memory leaves the file behind: it says what was running and how
//!   far it had got ([`note`]). A completed top span goes into the ring.
//! - **Leaf spans** are routine per-item work (one photo applied, one file probed). They are not listed and not put
//!   in the ring: their duration is added to `stats-<pid>.json`'s per-name count / total / min / max, with atomics and no lock.
//! - **Events** are things that happened (a panic, a configuration entry that was not understood), with no duration.
//! - **The ring** (`ring-<pid>.json`, [`RING_CAP`] entries) keeps the newest; when full, the most verbose [`Level`] goes
//!   first, then a routine success, then the oldest, so a failure or a critical record outlives a burst of detail.
//!
//! # Levels: how much to record, and where, so the recorder does not change what it watches
//!
//! Every call has a [`Level`] (`critical` < `normal` < `detail` < `trace`) and every [`Subsystem`] has a threshold; a call is
//! recorded only when its level is at or below its subsystem's threshold (or the threshold is `off`). A call below the
//! threshold costs one atomic load: no lock, no allocation, no clock read. The level also sets how durable a record is: a
//! `critical` span or event is written to disk at once (the outermost steps and panics, which are what you need after a
//! kill); everything else is written by a background thread at most every 250 ms (so the in-flight file of a non-critical
//! span can be that stale), and the thread that does the work never waits for the disk.
//!
//! `<PREFIX>_DIAG` takes a comma-separated list: a bare level sets every subsystem, `name=level` overrides one:
//! `normal` (also `1`, `on`, `true`), `critical,import=trace`, `detail,probe=off`. Order does not matter, the last entry for
//! a subsystem wins, and an entry that is not understood is ignored and reported as a `config_rejected` event.
//! What each call site uses: `critical` for the outermost steps, `normal` for stages and slow-thing events, `detail` for work
//! that runs on every command, `trace` for per-item work (one photo, one file).
//!
//! # Its own cost is measured
//!
//! The time spent inside the recorder's own calls (lock waits included) and in its writer thread is counted ([`Overhead`]) and
//! shown in the report, so a trace says how much it perturbed the thing it traced.
//!
//! Records are typed: [`Subsystem`], [`Outcome`], [`Level`] and [`Class`] are closed enums and field keys are `&'static str`.
//! There is no message string anything branches on. A field value is a number, a flag, a length or a short text of at most
//! [`TEXT_MAX_CHARS`] characters: callers give ids, counts and format names, **never file names, paths or tag values**.
//!
//! Off unless `<PREFIX>_DIAG` is set ([`start_from_env`]): when off, every call is one atomic load. Writing diagnostics never
//! fails the work being diagnosed: a failed write is counted ([`Snapshot::write_failures`]) and nothing else.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![deny(clippy::indexing_slicing)]

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::{BuildHasherDefault, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Entries the ring keeps.
pub const RING_CAP: usize = 512;
/// Longest [`Val::Text`], in characters.
pub const TEXT_MAX_CHARS: usize = 64;
/// How many processes' files are kept when a new process starts (never our own).
const KEEP_PROCESSES: usize = 12;
/// The writer thread rewrites the files at most this often.
const FLUSH_EVERY_MS: u64 = 250;
/// A span handle passes a [`Span::note`] at most this often (the file is written every [`FLUSH_EVERY_MS`] anyway).
const NOTE_MIN_NS: u64 = 50_000_000;
const SCHEMA: u32 = 1;

/// Where a record comes from. A closed set: adding one is a code change. `Control` and `Ui` have no call sites yet
/// (the control server and the frame loop come later); they are here so a configuration naming them is understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subsystem {
    /// The recorder itself: start, panics, configuration it did not understand.
    App,
    /// Adding files to the library: the import commit.
    Import,
    /// Reading each file before an import (hash, format, metadata).
    Probe,
    /// Committing a Lightroom catalog import and its stages.
    Commit,
    /// Writing the library to disk after a command (persist, flush).
    Catalog,
    Control,
    Ui,
}

impl Subsystem {
    /// Every subsystem, in [`Subsystem::index`] order.
    pub const ALL: [Subsystem; 7] =
        [Subsystem::App, Subsystem::Import, Subsystem::Probe, Subsystem::Commit, Subsystem::Catalog, Subsystem::Control, Subsystem::Ui];
    pub const COUNT: usize = Self::ALL.len();

    /// Position in [`Subsystem::ALL`]; the match is exhaustive, so a new variant cannot be forgotten here.
    pub const fn index(self) -> usize {
        match self {
            Subsystem::App => 0,
            Subsystem::Import => 1,
            Subsystem::Probe => 2,
            Subsystem::Commit => 3,
            Subsystem::Catalog => 4,
            Subsystem::Control => 5,
            Subsystem::Ui => 6,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Subsystem::App => "app",
            Subsystem::Import => "import",
            Subsystem::Probe => "probe",
            Subsystem::Commit => "commit",
            Subsystem::Catalog => "catalog",
            Subsystem::Control => "control",
            Subsystem::Ui => "ui",
        }
    }

    /// The subsystem called `name` (the [`Subsystem::as_str`] spelling).
    pub fn parse(name: &str) -> Option<Subsystem> {
        Self::ALL.iter().copied().find(|s| s.as_str() == name)
    }
}

/// How much a record matters, and so how verbose a recorder must be set to keep it: `Critical` is the least verbose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// The outermost steps and panics: written to disk at once.
    Critical = 1,
    /// Stages of a step and slow-thing events.
    #[default]
    Normal = 2,
    /// Work that runs on every command.
    Detail = 3,
    /// Per-item work (one photo, one file).
    Trace = 4,
}

impl Level {
    pub const ALL: [Level; 4] = [Level::Critical, Level::Normal, Level::Detail, Level::Trace];

    pub const fn as_str(self) -> &'static str {
        match self {
            Level::Critical => "critical",
            Level::Normal => "normal",
            Level::Detail => "detail",
            Level::Trace => "trace",
        }
    }

    pub fn parse(name: &str) -> Option<Level> {
        Self::ALL.iter().copied().find(|l| l.as_str() == name)
    }
}

/// A subsystem's threshold as stored: `0` = off, else a [`Level`]'s number.
fn threshold_of(level: Option<Level>) -> u8 {
    level.map_or(0, |l| l as u8)
}

fn level_of(threshold: u8) -> Option<Level> {
    Level::ALL.iter().copied().find(|l| *l as u8 == threshold)
}

/// A threshold word: `off`/`0`, a level name, or `on`/`true`/`1` (= `normal`, what a plain "on" meant before levels).
fn parse_threshold(word: &str) -> Option<u8> {
    match word {
        "off" | "0" => Some(0),
        "on" | "true" | "1" => Some(Level::Normal as u8),
        other => Level::parse(other).map(|l| l as u8),
    }
}

/// What was wrong with one entry of a [`Config`] string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigProblem {
    UnknownLevel,
    UnknownSubsystem,
}

impl ConfigProblem {
    pub const fn as_str(self) -> &'static str {
        match self {
            ConfigProblem::UnknownLevel => "unknown_level",
            ConfigProblem::UnknownSubsystem => "unknown_subsystem",
        }
    }
}

/// An entry that was ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    pub entry: String,
    pub problem: ConfigProblem,
}

/// Which levels are recorded for which subsystems.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    default: u8,
    per: [Option<u8>; Subsystem::COUNT],
}

impl Default for Config {
    /// Every subsystem at `normal`.
    fn default() -> Config {
        Config::all(Some(Level::Normal))
    }
}

impl Config {
    /// Every subsystem at `level` (`None`: off).
    pub fn all(level: Option<Level>) -> Config {
        Config { default: threshold_of(level), per: [None; Subsystem::COUNT] }
    }

    /// `normal,import=trace,probe=off`: see the module docs. Returns what could be understood and what was ignored.
    pub fn parse(text: &str) -> (Config, Vec<ConfigIssue>) {
        let mut config = Config::default();
        let mut issues = Vec::new();
        for entry in text.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let problem = match entry.split_once('=') {
                None => match parse_threshold(entry) {
                    Some(t) => {
                        config.default = t;
                        None
                    }
                    None => Some(ConfigProblem::UnknownLevel),
                },
                Some((name, word)) => match (Subsystem::parse(name.trim()), parse_threshold(word.trim())) {
                    (Some(s), Some(t)) => {
                        if let Some(slot) = config.per.get_mut(s.index()) {
                            *slot = Some(t);
                        }
                        None
                    }
                    (None, _) => Some(ConfigProblem::UnknownSubsystem),
                    (Some(_), None) => Some(ConfigProblem::UnknownLevel),
                },
            };
            if let Some(problem) = problem {
                issues.push(ConfigIssue { entry: entry.to_string(), problem });
            }
        }
        (config, issues)
    }

    /// The most verbose level recorded for `subsystem` (`None`: nothing).
    pub fn level_of(&self, subsystem: Subsystem) -> Option<Level> {
        level_of(self.threshold(subsystem))
    }

    fn threshold(&self, subsystem: Subsystem) -> u8 {
        self.per.get(subsystem.index()).copied().flatten().unwrap_or(self.default)
    }
}

/// How a span ended. `Abandoned` is a guard dropped without a result (an early return); `Panicked` one dropped while unwinding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    Failed,
    Cancelled,
    Panicked,
    Abandoned,
}

impl Outcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Failed => "failed",
            Outcome::Cancelled => "cancelled",
            Outcome::Panicked => "panicked",
            Outcome::Abandoned => "abandoned",
        }
    }
}

/// `Top`: a step, listed while it runs and kept in the ring. `Leaf`: routine per-item work, only counted and timed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Top,
    Leaf,
}

impl Class {
    /// The level [`span`] uses: a step is `normal`, per-item work is `trace`.
    pub const fn default_level(self) -> Level {
        match self {
            Class::Top => Level::Normal,
            Class::Leaf => Level::Trace,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Span,
    Event,
}

/// A field value. Nothing arbitrary: a number, a flag, a byte length or a short text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum Val {
    U64(u64),
    Bool(bool),
    Len(u64),
    Text(String),
}

impl Val {
    /// A short identifier, cut at [`TEXT_MAX_CHARS`] characters (on a character boundary).
    pub fn text(s: &str) -> Val {
        Val::Text(s.chars().take(TEXT_MAX_CHARS).collect())
    }

    fn show(&self) -> String {
        match self {
            Val::U64(n) => n.to_string(),
            Val::Bool(b) => b.to_string(),
            Val::Len(n) => format!("{n} B"),
            Val::Text(t) => t.clone(),
        }
    }
}

/// One completed span or event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    pub ts_ms: u64,
    pub kind: Kind,
    pub subsystem: Subsystem,
    #[serde(default)]
    pub level: Level,
    pub op: String,
    pub outcome: Option<Outcome>,
    pub dur_us: Option<u64>,
    pub thread: u64,
    pub fields: Vec<(String, Val)>,
}

/// A top span that has begun and not ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenView {
    pub seq: u64,
    pub ts_ms: u64,
    pub age_ms: u64,
    pub subsystem: Subsystem,
    pub op: String,
    pub thread: u64,
    pub fields: Vec<(String, Val)>,
}

/// Aggregate timing of one leaf-span name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatRow {
    pub subsystem: Subsystem,
    pub op: String,
    pub count: u64,
    pub total_us: u64,
    pub min_us: u64,
    pub max_us: u64,
}

/// What the recorder itself cost.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overhead {
    /// Recorded calls (spans begun, spans ended, events, notes). Calls below the threshold are not counted: they cost one load.
    pub calls: u64,
    /// Time spent inside those calls on the calling threads, lock waits included, in nanoseconds.
    pub recorder_ns: u64,
    /// Time the writer thread spent serialising and writing, in nanoseconds (not on any working thread).
    pub writer_ns: u64,
    /// Time since the recorder started, in milliseconds.
    pub uptime_ms: u64,
}

/// One subsystem's threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelRow {
    pub subsystem: Subsystem,
    /// `None`: off.
    pub level: Option<Level>,
}

/// Everything in memory, for a control command or a test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub pid: u32,
    pub open: Vec<OpenView>,
    pub ring: Vec<Entry>,
    pub stats: Vec<StatRow>,
    pub write_failures: u64,
    pub overhead: Overhead,
    pub levels: Vec<LevelRow>,
}

#[derive(Serialize, Deserialize)]
struct InflightFile {
    schema: u32,
    pid: u32,
    started_ms: u64,
    written_ms: u64,
    write_failures: u64,
    #[serde(default)]
    levels: Vec<LevelRow>,
    open: Vec<OpenView>,
}

#[derive(Serialize, Deserialize)]
struct RingFile {
    schema: u32,
    pid: u32,
    entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
struct StatsFile {
    schema: u32,
    pid: u32,
    #[serde(default)]
    overhead: Overhead,
    stats: Vec<StatRow>,
}

struct Open {
    seq: u64,
    ts_ms: u64,
    subsystem: Subsystem,
    op: &'static str,
    thread: u64,
    started: Instant,
    fields: Vec<(String, Val)>,
}

#[derive(Default)]
struct State {
    open: Vec<Open>,
    /// Ring entries by level (index = level - 1), each with its push number, so the ring reads back in push order and
    /// the most verbose level is found without a scan.
    ring: [VecDeque<(u64, Entry)>; 4],
    pushed: u64,
    last_flush: Option<Instant>,
}

impl State {
    fn ring_len(&self) -> usize {
        self.ring.iter().map(VecDeque::len).sum()
    }

    /// The ring, oldest push first.
    fn ring_entries(&self) -> Vec<Entry> {
        let mut all: Vec<&(u64, Entry)> = self.ring.iter().flatten().collect();
        all.sort_by_key(|(n, _)| *n);
        all.into_iter().map(|(_, e)| e.clone()).collect()
    }
}

/// One leaf name's totals as one thread keeps them. Each thread has its own cell per name and is its only writer (plain
/// load and store, no locked instruction, no line shared with another thread); the reader sums the cells. A `shared` cell
/// (the fallback when thread-local storage is gone) is written with read-modify-write instead.
struct AtomicStat {
    shared: bool,
    count: AtomicU64,
    total_us: AtomicU64,
    min_us: AtomicU64,
    max_us: AtomicU64,
    /// Every [`LEAF_SAMPLE_EVERY`]th end is timed from the start of its bookkeeping: how many, and their total ns.
    sample_n: AtomicU64,
    sample_ns: AtomicU64,
}

impl AtomicStat {
    fn new(shared: bool) -> AtomicStat {
        AtomicStat {
            shared,
            count: AtomicU64::new(0),
            total_us: AtomicU64::new(0),
            min_us: AtomicU64::new(u64::MAX),
            max_us: AtomicU64::new(0),
            sample_n: AtomicU64::new(0),
            sample_ns: AtomicU64::new(0),
        }
    }

    fn record(&self, us: u64) {
        if self.shared {
            self.count.fetch_add(1, Ordering::Relaxed);
            self.total_us.fetch_add(us, Ordering::Relaxed);
            self.min_us.fetch_min(us, Ordering::Relaxed);
            self.max_us.fetch_max(us, Ordering::Relaxed);
        } else {
            self.count.store(self.count.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
            self.total_us.store(self.total_us.load(Ordering::Relaxed).saturating_add(us), Ordering::Relaxed);
            if us < self.min_us.load(Ordering::Relaxed) {
                self.min_us.store(us, Ordering::Relaxed);
            }
            if us > self.max_us.load(Ordering::Relaxed) {
                self.max_us.store(us, Ordering::Relaxed);
            }
        }
    }

    fn sample(&self, ns: u64) {
        self.sample_n.fetch_add(1, Ordering::Relaxed);
        self.sample_ns.fetch_add(ns, Ordering::Relaxed);
    }
}

/// FNV-1a: a leaf name is hashed on every call, so it should cost a few ns, not SipHash's tens.
struct FnvHasher(u64);

impl Default for FnvHasher {
    fn default() -> FnvHasher {
        FnvHasher(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for FnvHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

type StatKey = (Subsystem, &'static str);
/// A thread's stat cells: the recorder's id (tests run several recorders), then the name.
type LeafCells = HashMap<(u64, Subsystem, &'static str), Arc<AtomicStat>, BuildHasherDefault<FnvHasher>>;

/// Every this-many leaf-span ends on a thread is timed, to estimate what leaf recording costs without reading the clock twice
/// more on every call.
const LEAF_SAMPLE_EVERY: u64 = 32;

struct Local {
    ends: u64,
    cells: LeafCells,
}

thread_local! {
    /// Per thread, its own stat cells: after the first call of a name on a thread, recording a leaf span takes no lock and
    /// touches no line another thread writes.
    static LOCAL: RefCell<Local> = RefCell::new(Local { ends: 0, cells: HashMap::default() });
}

pub fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn thread_num() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    thread_local! { static ID: u64 = NEXT.fetch_add(1, Ordering::Relaxed); }
    ID.with(|id| *id)
}

fn own_fields(fields: &[(&'static str, Val)]) -> Vec<(String, Val)> {
    fields.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect()
}

fn set_field(fields: &mut Vec<(String, Val)>, key: &str, v: Val) {
    match fields.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = v,
        None => fields.push((key.to_string(), v)),
    }
}

/// The recorder. One process-wide instance ([`start`]); tests build their own.
pub struct Diag {
    id: u64,
    dir: Option<PathBuf>,
    pid: u32,
    started_ms: u64,
    started: Instant,
    seq: AtomicU64,
    cap: usize,
    flush_every_ms: AtomicU64,
    thresholds: [AtomicU8; Subsystem::COUNT],
    /// Every thread's cells per leaf name (see [`AtomicStat`]).
    stats: Mutex<HashMap<StatKey, Vec<Arc<AtomicStat>>>>,
    /// Something changed since the files were last written.
    dirty: AtomicBool,
    /// A writer thread is running (see [`Diag::tick`]); without one, a change writes the files itself, rate-limited.
    writer: AtomicBool,
    write_failures: AtomicU64,
    calls: AtomicU64,
    recorder_ns: AtomicU64,
    writer_ns: AtomicU64,
    state: Mutex<State>,
}

impl Diag {
    /// A recorder that keeps every level, writing into `dir` (created if needed; `None` keeps everything in memory) and
    /// keeping `cap` ring entries.
    pub fn new(dir: Option<PathBuf>, cap: usize) -> Diag {
        Diag::with_config(dir, cap, &Config::all(Some(Level::Trace)))
    }

    pub fn with_config(dir: Option<PathBuf>, cap: usize, config: &Config) -> Diag {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let pid = std::process::id();
        let dir = dir.filter(|d| std::fs::create_dir_all(d).is_ok());
        if let Some(d) = &dir {
            prune(d, pid);
        }
        Diag {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            dir,
            pid,
            started_ms: unix_ms(),
            started: Instant::now(),
            seq: AtomicU64::new(1),
            cap: cap.max(8),
            flush_every_ms: AtomicU64::new(FLUSH_EVERY_MS),
            thresholds: std::array::from_fn(|i| AtomicU8::new(Subsystem::ALL.get(i).map_or(0, |s| config.threshold(*s)))),
            stats: Mutex::new(HashMap::new()),
            dirty: AtomicBool::new(false),
            writer: AtomicBool::new(false),
            write_failures: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            recorder_ns: AtomicU64::new(0),
            writer_ns: AtomicU64::new(0),
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a call at `level` in `subsystem` is recorded: one atomic load.
    pub fn wants(&self, subsystem: Subsystem, level: Level) -> bool {
        level as u8 <= self.threshold(subsystem)
    }

    /// `subsystem`'s threshold now (`0`: off).
    fn threshold(&self, subsystem: Subsystem) -> u8 {
        self.thresholds.get(subsystem.index()).map_or(0, |t| t.load(Ordering::Relaxed))
    }

    /// Change a threshold while running (`subsystem` `None`: all of them). `level` `None`: off.
    pub fn set_level(&self, subsystem: Option<Subsystem>, level: Option<Level>) {
        let t = threshold_of(level);
        match subsystem {
            Some(s) => {
                if let Some(a) = self.thresholds.get(s.index()) {
                    a.store(t, Ordering::Relaxed);
                }
            }
            None => self.thresholds.iter().for_each(|a| a.store(t, Ordering::Relaxed)),
        }
    }

    pub fn levels(&self) -> Vec<LevelRow> {
        Subsystem::ALL.iter().map(|s| LevelRow { subsystem: *s, level: level_of(self.threshold(*s)) }).collect()
    }

    /// Count a recorded call and the time it took.
    fn charge(&self, since: Instant) {
        self.recorder_ns.fetch_add(since.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    /// Something changed. With a writer thread that is all (flag it, without touching the shared line when it is already set);
    /// without one, write the files now if the interval is up.
    fn changed(&self, st: &mut State) {
        if !self.dirty.load(Ordering::Relaxed) {
            self.dirty.store(true, Ordering::Relaxed);
        }
        if !self.writer.load(Ordering::Relaxed) {
            self.flush_if_due(st);
        }
    }

    /// Begin a span at the class's default level ([`Class::default_level`]). A [`Class::Leaf`] span only times itself (its
    /// fields are ignored).
    pub fn span(&self, subsystem: Subsystem, op: &'static str, class: Class, fields: &[(&'static str, Val)]) -> Span<'_> {
        self.span_at(class.default_level(), subsystem, op, class, fields)
    }

    /// Begin a span at `level`: nothing is recorded (and nothing costs more than one load) when `subsystem`'s threshold is
    /// below it. A `Critical` top span is on disk before this returns.
    pub fn span_at(&self, level: Level, subsystem: Subsystem, op: &'static str, class: Class, fields: &[(&'static str, Val)]) -> Span<'_> {
        if !self.wants(subsystem, level) {
            return Span::none();
        }
        if class == Class::Leaf {
            // routine per-item work: no sequence number, no lock, no counter shared with other threads; one clock read
            return Span {
                diag: Some(self),
                seq: 0,
                subsystem,
                op,
                level,
                class,
                started: Some(Instant::now()),
                extra: Vec::new(),
                note_next_ns: AtomicU64::new(0),
            };
        }
        let t0 = Instant::now();
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        if class == Class::Top {
            let mut st = self.lock();
            st.open.push(Open { seq, ts_ms: unix_ms(), subsystem, op, thread: thread_num(), started: t0, fields: own_fields(fields) });
            if level == Level::Critical {
                self.write_all(&mut st);
            } else {
                self.changed(&mut st);
            }
        }
        self.charge(t0);
        // The span's own clock starts after the bookkeeping above, so its duration does not include the recorder's cost.
        Span { diag: Some(self), seq, subsystem, op, level, class, started: Some(Instant::now()), extra: Vec::new(), note_next_ns: AtomicU64::new(0) }
    }

    /// Record something that happened, at [`Level::Normal`].
    pub fn event(&self, subsystem: Subsystem, op: &str, fields: &[(&'static str, Val)]) {
        self.event_at(Level::Normal, subsystem, op, fields);
    }

    /// Record something that happened, if `subsystem`'s threshold admits `level`. A `Critical` event is on disk before this
    /// returns.
    pub fn event_at(&self, level: Level, subsystem: Subsystem, op: &str, fields: &[(&'static str, Val)]) {
        if !self.wants(subsystem, level) {
            return;
        }
        let t0 = Instant::now();
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut st = self.lock();
        self.push_ring(
            &mut st,
            Entry {
                seq,
                ts_ms: unix_ms(),
                kind: Kind::Event,
                subsystem,
                level,
                op: op.to_string(),
                outcome: None,
                dur_us: None,
                thread: thread_num(),
                fields: own_fields(fields),
            },
        );
        if level == Level::Critical {
            self.write_all(&mut st);
        } else {
            self.changed(&mut st);
        }
        drop(st);
        self.charge(t0);
    }

    /// Set a field on the calling thread's innermost open top span (how far it has got).
    pub fn note(&self, key: &'static str, v: Val) {
        let t0 = Instant::now();
        let me = thread_num();
        let mut st = self.lock();
        if let Some(o) = st.open.iter_mut().rev().find(|o| o.thread == me) {
            set_field(&mut o.fields, key, v);
            self.changed(&mut st);
            drop(st);
            self.charge(t0);
        }
    }

    fn note_seq(&self, seq: u64, key: &'static str, v: Val) {
        let t0 = Instant::now();
        let mut st = self.lock();
        if let Some(o) = st.open.iter_mut().find(|o| o.seq == seq) {
            set_field(&mut o.fields, key, v);
            self.changed(&mut st);
            drop(st);
            self.charge(t0);
        }
    }

    fn end(&self, span: &Span<'_>, outcome: Outcome, dur_us: u64) {
        if span.class == Class::Leaf {
            // counted and (by sampling) timed in its own cell: nothing shared with other threads
            self.record_leaf(span.subsystem, span.op, dur_us);
            return;
        }
        let t0 = Instant::now();
        match span.class {
            Class::Leaf => {}
            Class::Top => {
                let mut st = self.lock();
                let Some(i) = st.open.iter().position(|o| o.seq == span.seq) else { return };
                let o = st.open.remove(i);
                let mut fields = o.fields;
                for (k, v) in &span.extra {
                    set_field(&mut fields, k, v.clone());
                }
                self.push_ring(
                    &mut st,
                    Entry {
                        seq: o.seq,
                        ts_ms: o.ts_ms,
                        kind: Kind::Span,
                        subsystem: o.subsystem,
                        level: span.level,
                        op: o.op.to_string(),
                        outcome: Some(outcome),
                        dur_us: Some(dur_us),
                        thread: o.thread,
                        fields,
                    },
                );
                if span.level == Level::Critical {
                    self.write_all(&mut st);
                } else {
                    self.changed(&mut st);
                }
            }
        }
        self.charge(t0);
    }

    /// Add one leaf duration to this thread's cell for the name: no lock after the first call of that name on the thread.
    fn record_leaf(&self, subsystem: Subsystem, op: &'static str, dur_us: u64) {
        if !self.dirty.load(Ordering::Relaxed) {
            self.dirty.store(true, Ordering::Relaxed);
        }
        let key = (self.id, subsystem, op);
        let done = LOCAL.try_with(|local| {
            let mut local = local.try_borrow_mut().ok()?;
            local.ends = local.ends.wrapping_add(1);
            let t0 = (local.ends % LEAF_SAMPLE_EVERY == 0).then(Instant::now);
            let cell = local.cells.entry(key).or_insert_with(|| self.new_cell(subsystem, op, false));
            cell.record(dur_us);
            if let Some(t0) = t0 {
                cell.sample(t0.elapsed().as_nanos() as u64);
            }
            Some(())
        });
        if !matches!(done, Ok(Some(()))) {
            self.shared_cell(subsystem, op).record(dur_us);
        }
    }

    fn new_cell(&self, subsystem: Subsystem, op: &'static str, shared: bool) -> Arc<AtomicStat> {
        let cell = Arc::new(AtomicStat::new(shared));
        let mut map = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
        map.entry((subsystem, op)).or_default().push(Arc::clone(&cell));
        cell
    }

    /// The cell for a thread that can't have its own (thread-local storage already torn down).
    fn shared_cell(&self, subsystem: Subsystem, op: &'static str) -> Arc<AtomicStat> {
        let found = {
            let map = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
            map.get(&(subsystem, op)).and_then(|cells| cells.iter().find(|c| c.shared).cloned())
        };
        found.unwrap_or_else(|| self.new_cell(subsystem, op, true))
    }

    /// Make room for `e`: the most verbose level present loses one entry, a routine success if it has one, else its oldest.
    fn push_ring(&self, st: &mut State, e: Entry) {
        if st.ring_len() >= self.cap
            && let Some(q) = st.ring.iter_mut().rev().find(|q| !q.is_empty())
        {
            let i = q.iter().position(|(_, x)| x.kind == Kind::Span && x.outcome == Some(Outcome::Ok)).unwrap_or(0);
            q.remove(i);
        }
        st.pushed += 1;
        let n = st.pushed;
        let slot = (e.level as usize).saturating_sub(1);
        if let Some(q) = st.ring.get_mut(slot) {
            q.push_back((n, e));
        }
    }

    pub fn overhead(&self) -> Overhead {
        let (leaf_calls, leaf_ns) = self.leaf_totals();
        Overhead {
            calls: self.calls.load(Ordering::Relaxed) + leaf_calls,
            recorder_ns: self.recorder_ns.load(Ordering::Relaxed) + leaf_ns,
            writer_ns: self.writer_ns.load(Ordering::Relaxed),
            uptime_ms: self.started.elapsed().as_millis() as u64,
        }
    }

    /// Leaf spans recorded, and an estimate of the time their recording took (from the sampled ends).
    fn leaf_totals(&self) -> (u64, u64) {
        let map = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut calls, mut ns) = (0u64, 0u64);
        for cell in map.values().flatten() {
            let count = cell.count.load(Ordering::Relaxed);
            calls = calls.saturating_add(count);
            let n = cell.sample_n.load(Ordering::Relaxed);
            if n > 0 {
                ns = ns.saturating_add((u128::from(cell.sample_ns.load(Ordering::Relaxed)) * u128::from(count) / u128::from(n)) as u64);
            }
        }
        (calls, ns)
    }

    /// What is in memory now (`tail` newest ring entries).
    pub fn snapshot(&self, tail: usize) -> Snapshot {
        let st = self.lock();
        let all = st.ring_entries();
        let skip = all.len().saturating_sub(tail);
        Snapshot {
            pid: self.pid,
            open: self.open_views(&st),
            ring: all.into_iter().skip(skip).collect(),
            stats: self.stat_rows(),
            write_failures: self.write_failures.load(Ordering::Relaxed),
            overhead: self.overhead(),
            levels: self.levels(),
        }
    }

    fn open_views(&self, st: &State) -> Vec<OpenView> {
        st.open
            .iter()
            .map(|o| OpenView {
                seq: o.seq,
                ts_ms: o.ts_ms,
                age_ms: o.started.elapsed().as_millis() as u64,
                subsystem: o.subsystem,
                op: o.op.to_string(),
                thread: o.thread,
                fields: o.fields.clone(),
            })
            .collect()
    }

    fn stat_rows(&self) -> Vec<StatRow> {
        let map = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
        let mut rows: Vec<StatRow> = map
            .iter()
            .map(|((subsystem, op), cells)| {
                let mut row = StatRow { subsystem: *subsystem, op: (*op).to_string(), count: 0, total_us: 0, min_us: u64::MAX, max_us: 0 };
                for c in cells {
                    let count = c.count.load(Ordering::Relaxed);
                    if count == 0 {
                        continue;
                    }
                    row.count = row.count.saturating_add(count);
                    row.total_us = row.total_us.saturating_add(c.total_us.load(Ordering::Relaxed));
                    row.min_us = row.min_us.min(c.min_us.load(Ordering::Relaxed));
                    row.max_us = row.max_us.max(c.max_us.load(Ordering::Relaxed));
                }
                if row.count == 0 {
                    row.min_us = 0;
                }
                row
            })
            .collect();
        rows.sort_by(|a, b| b.total_us.cmp(&a.total_us).then_with(|| a.op.cmp(&b.op)));
        rows
    }

    /// Write every file now, on this thread.
    pub fn flush(&self) {
        let mut st = self.lock();
        self.write_all(&mut st);
    }

    /// What the writer thread does every [`FLUSH_EVERY_MS`]: if anything changed, serialise under the lock and write the
    /// files outside it.
    pub fn tick(&self) {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let t0 = Instant::now();
        let files = {
            let mut st = self.lock();
            st.last_flush = Some(Instant::now());
            self.render(&st)
        };
        self.put(files);
        self.writer_ns.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    fn flush_if_due(&self, st: &mut State) {
        let every = self.flush_every_ms.load(Ordering::Relaxed);
        let due = st.last_flush.is_none_or(|t| t.elapsed().as_millis() as u64 >= every);
        if due {
            self.write_all(st);
        }
    }

    /// The three files' bytes. A file that can't be serialised is counted as a failed write.
    fn render(&self, st: &State) -> Vec<(String, Vec<u8>)> {
        let inflight = InflightFile {
            schema: SCHEMA,
            pid: self.pid,
            started_ms: self.started_ms,
            written_ms: unix_ms(),
            write_failures: self.write_failures.load(Ordering::Relaxed),
            levels: self.levels(),
            open: self.open_views(st),
        };
        let ring = RingFile { schema: SCHEMA, pid: self.pid, entries: st.ring_entries() };
        let stats = StatsFile { schema: SCHEMA, pid: self.pid, overhead: self.overhead(), stats: self.stat_rows() };
        let mut files = Vec::with_capacity(3);
        for (name, bytes) in [
            (format!("inflight-{}.json", self.pid), serde_json::to_vec(&inflight)),
            (format!("ring-{}.json", self.pid), serde_json::to_vec(&ring)),
            (format!("stats-{}.json", self.pid), serde_json::to_vec(&stats)),
        ] {
            match bytes {
                Ok(b) => files.push((name, b)),
                Err(_) => {
                    self.write_failures.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        files
    }

    /// Write rendered files (temp file + rename). A failed write is counted.
    fn put(&self, files: Vec<(String, Vec<u8>)>) {
        let Some(dir) = &self.dir else { return };
        for (name, bytes) in files {
            let tmp = dir.join(format!("{name}.tmp"));
            if std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, dir.join(&name))).is_err() {
                self.write_failures.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn write_all(&self, st: &mut State) {
        st.last_flush = Some(Instant::now());
        let files = self.render(st);
        self.put(files);
    }

    /// For the panic hook: never waits for the lock (a panic on a thread that holds it would wait for itself).
    fn panic_event(&self, location: &str) {
        let Ok(mut st) = self.state.try_lock() else { return };
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        self.push_ring(
            &mut st,
            Entry {
                seq,
                ts_ms: unix_ms(),
                kind: Kind::Event,
                subsystem: Subsystem::App,
                level: Level::Critical,
                op: "panic".into(),
                outcome: None,
                dur_us: None,
                thread: thread_num(),
                fields: vec![("location".into(), Val::text(location))],
            },
        );
        self.write_all(&mut st);
    }
}

/// Keep the newest [`KEEP_PROCESSES`] processes' files (by modification time), never `own`'s.
fn prune(dir: &Path, own: u32) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut by_pid: HashMap<u32, (std::time::SystemTime, Vec<PathBuf>)> = HashMap::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let pid = ["inflight-", "ring-", "stats-"]
            .iter()
            .find_map(|p| name.strip_prefix(p))
            .and_then(|rest| rest.split('.').next())
            .and_then(|n| n.parse::<u32>().ok());
        let Some(pid) = pid else { continue };
        let mtime = entry.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);
        let slot = by_pid.entry(pid).or_insert((mtime, Vec::new()));
        slot.0 = slot.0.max(mtime);
        slot.1.push(entry.path());
    }
    let mut all: Vec<_> = by_pid.into_iter().filter(|(pid, _)| *pid != own).collect();
    all.sort_by_key(|(_, (t, _))| std::cmp::Reverse(*t));
    for (_, (_, files)) in all.into_iter().skip(KEEP_PROCESSES) {
        for f in files {
            // best effort: a file that can't be removed now is tried again by the next process
            let _ = std::fs::remove_file(f);
        }
    }
}

/// A span in progress. Finish it with [`Span::finish`] / [`Span::done`]; dropped without either it ends `Abandoned`
/// (`Panicked` while unwinding).
#[must_use = "a span records its outcome when finished or dropped; binding it to _ ends it at once"]
pub struct Span<'a> {
    diag: Option<&'a Diag>,
    seq: u64,
    subsystem: Subsystem,
    op: &'static str,
    level: Level,
    class: Class,
    /// `None` for a span that records nothing.
    started: Option<Instant>,
    extra: Vec<(String, Val)>,
    /// Earliest time (since the recorder started) a [`Span::note`] from this handle is passed on.
    note_next_ns: AtomicU64,
}

impl<'a> Span<'a> {
    /// A span that records nothing: what the code gets when diagnostics are off or the level is below the threshold. It
    /// reads no clock.
    pub fn none() -> Span<'a> {
        Span {
            diag: None,
            seq: 0,
            subsystem: Subsystem::App,
            op: "",
            level: Level::Trace,
            class: Class::Leaf,
            started: None,
            extra: Vec::new(),
            note_next_ns: AtomicU64::new(0),
        }
    }

    /// Whether this span is being recorded.
    pub fn recording(&self) -> bool {
        self.diag.is_some()
    }
}

impl Span<'_> {
    /// Set a field of this span, kept in the entry when it ends (and visible in the in-flight file for a top span, via [`note`]).
    pub fn set(&mut self, key: &'static str, v: Val) {
        if self.diag.is_some() {
            set_field(&mut self.extra, key, v);
        }
    }

    /// Set a field of this open top span from any thread (how far it has got). Passed on at most every 50 ms per span
    /// handle (the in-flight file is rewritten every 250 ms anyway), so calling it per item is cheap.
    pub fn note(&self, key: &'static str, v: Val) {
        if let (Some(d), Class::Top) = (self.diag, self.class) {
            let now = d.started.elapsed().as_nanos() as u64;
            if now < self.note_next_ns.load(Ordering::Relaxed) {
                return;
            }
            self.note_next_ns.store(now.saturating_add(NOTE_MIN_NS), Ordering::Relaxed);
            d.note_seq(self.seq, key, v);
        }
    }

    pub fn finish(mut self, outcome: Outcome) {
        self.end(outcome);
    }

    /// `Ok` or `Failed` from a `Result`, without touching it.
    pub fn done<T, E>(self, result: &Result<T, E>) {
        self.finish(if result.is_ok() { Outcome::Ok } else { Outcome::Failed });
    }

    fn end(&mut self, outcome: Outcome) {
        if let Some(d) = self.diag.take() {
            let dur_us = self.started.map_or(0, |s| s.elapsed().as_micros() as u64);
            d.end(self, outcome, dur_us);
        }
    }
}

impl Drop for Span<'_> {
    fn drop(&mut self) {
        let outcome = if std::thread::panicking() { Outcome::Panicked } else { Outcome::Abandoned };
        self.end(outcome);
    }
}

static GLOBAL: OnceLock<Diag> = OnceLock::new();

/// Start the process-wide recorder with `config`, install the panic hook (chained after the existing one), start the writer
/// thread and record a `start` event. `false` when one is already running, and always on the web build (no clock,
/// threads or files there).
pub fn start(dir: Option<PathBuf>, config: &Config) -> bool {
    if cfg!(target_arch = "wasm32") {
        return false;
    }
    let first = GLOBAL.set(Diag::with_config(dir, RING_CAP, config)).is_ok();
    if first {
        install_panic_hook();
        if let Some(d) = GLOBAL.get()
            && d.dir.is_some()
            && std::thread::Builder::new().name("lcc-diag-writer".into()).spawn(writer_loop).is_ok()
        {
            d.writer.store(true, Ordering::Relaxed);
        }
        event_at(Level::Critical, Subsystem::App, "start", &[("pid", Val::U64(u64::from(std::process::id())))]);
    }
    first
}

fn writer_loop() {
    loop {
        let Some(d) = GLOBAL.get() else { return };
        d.tick();
        std::thread::sleep(Duration::from_millis(d.flush_every_ms.load(Ordering::Relaxed).max(10)));
    }
}

/// [`start`] when `<PREFIX>_DIAG` is set (and not `0`/`off`), in `<PREFIX>_DIAG_DIR` or `<settings>/diag`. The value is a
/// [`Config`] string; entries that are not understood are ignored and recorded as `config_rejected` events.
pub fn start_from_env() -> bool {
    let value = match lcc_brand::env("DIAG") {
        Some(v) if !matches!(v.trim(), "" | "0" | "off") => v,
        _ => return false,
    };
    let (config, issues) = Config::parse(&value);
    let dir = lcc_brand::env_os("DIAG_DIR").map(PathBuf::from).or_else(|| lcc_brand::settings_dir().map(|d| d.join("diag")));
    let first = start(dir, &config);
    if first {
        for issue in issues.iter().take(8) {
            event_at(
                Level::Critical,
                Subsystem::App,
                "config_rejected",
                &[("entry", Val::text(&issue.entry)), ("problem", Val::text(issue.problem.as_str()))],
            );
        }
    }
    first
}

pub fn enabled() -> bool {
    GLOBAL.get().is_some()
}

/// Whether a call at `level` in `subsystem` would be recorded now: false when diagnostics are off.
pub fn wants(subsystem: Subsystem, level: Level) -> bool {
    GLOBAL.get().is_some_and(|d| d.wants(subsystem, level))
}

/// [`Diag::set_level`] on the process-wide recorder.
pub fn set_level(subsystem: Option<Subsystem>, level: Option<Level>) {
    if let Some(d) = GLOBAL.get() {
        d.set_level(subsystem, level);
    }
}

pub fn span(subsystem: Subsystem, op: &'static str, class: Class, fields: &[(&'static str, Val)]) -> Span<'static> {
    span_at(class.default_level(), subsystem, op, class, fields)
}

pub fn span_at(level: Level, subsystem: Subsystem, op: &'static str, class: Class, fields: &[(&'static str, Val)]) -> Span<'static> {
    match GLOBAL.get() {
        Some(d) => d.span_at(level, subsystem, op, class, fields),
        None => Span::none(),
    }
}

pub fn event(subsystem: Subsystem, op: &str, fields: &[(&'static str, Val)]) {
    event_at(Level::Normal, subsystem, op, fields);
}

pub fn event_at(level: Level, subsystem: Subsystem, op: &str, fields: &[(&'static str, Val)]) {
    if let Some(d) = GLOBAL.get() {
        d.event_at(level, subsystem, op, fields);
    }
}

/// [`Diag::note`] on the process-wide recorder.
pub fn note(key: &'static str, v: Val) {
    if let Some(d) = GLOBAL.get() {
        d.note(key, v);
    }
}

pub fn snapshot(tail: usize) -> Option<Snapshot> {
    GLOBAL.get().map(|d| d.snapshot(tail))
}

pub fn flush() {
    if let Some(d) = GLOBAL.get() {
        d.flush();
    }
}

fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(d) = GLOBAL.get() {
                let at = info.location().map_or_else(String::new, |l| format!("{}:{}", l.file(), l.line()));
                d.panic_event(&at);
            }
            previous(info);
        }));
    });
}

/// Whether process `pid` is running (`ps -p`; `None` where that can't be asked).
pub fn is_running(pid: u32) -> Option<bool> {
    if !cfg!(unix) {
        return None;
    }
    let out = std::process::Command::new("ps").args(["-p", &pid.to_string(), "-o", "pid="]).output().ok()?;
    Some(!String::from_utf8_lossy(&out.stdout).trim().is_empty())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn show_fields(fields: &[(String, Val)]) -> String {
    fields.iter().map(|(k, v)| format!("{k}={}", v.show())).collect::<Vec<_>>().join(" ")
}

fn show_us(us: u64) -> String {
    if us >= 1_000_000 {
        format!("{:.2} s", us as f64 / 1e6)
    } else if us >= 1000 {
        format!("{:.1} ms", us as f64 / 1e3)
    } else {
        format!("{us} µs")
    }
}

/// `app critical, import trace, probe off`: the thresholds in effect, with equal neighbours folded to "all others".
fn show_levels(levels: &[LevelRow]) -> String {
    let word = |l: Option<Level>| l.map_or("off", Level::as_str);
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for r in levels {
        match counts.iter_mut().find(|(w, _)| *w == word(r.level)) {
            Some(slot) => slot.1 += 1,
            None => counts.push((word(r.level), 1)),
        }
    }
    let common = counts.iter().max_by_key(|&&(w, n)| (n, std::cmp::Reverse(w))).map(|&(w, _)| w);
    let mut parts: Vec<String> = common.map(|w| format!("all others {w}")).into_iter().collect();
    parts.extend(levels.iter().filter(|r| Some(word(r.level)) != common).map(|r| format!("{} {}", r.subsystem.as_str(), word(r.level))));
    parts.join(", ")
}

/// A text report of every process that left files in `dir`, newest first: running or gone, what was open, what ran last,
/// where the time went, and what the recorder itself cost.
pub fn report(dir: &Path) -> String {
    let mut procs: Vec<InflightFile> = Vec::new();
    if let Ok(read) = std::fs::read_dir(dir) {
        for e in read.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("inflight-")
                && name.ends_with(".json")
                && let Some(f) = read_json::<InflightFile>(&e.path())
            {
                procs.push(f);
            }
        }
    }
    procs.sort_by_key(|p| std::cmp::Reverse(p.started_ms));
    if procs.is_empty() {
        return format!("no diagnostics files in {}\n", dir.display());
    }
    let mut out = String::new();
    for p in &procs {
        let state = match is_running(p.pid) {
            Some(true) => "running (or hung)",
            Some(false) => "gone (finished, killed or crashed)",
            None => "state unknown",
        };
        out.push_str(&format!(
            "process {} ({state}); started {} ms, last written {} ms ago (clock difference to now)\n",
            p.pid,
            p.started_ms,
            unix_ms().saturating_sub(p.written_ms)
        ));
        if !p.levels.is_empty() {
            out.push_str(&format!("  levels: {}\n", show_levels(&p.levels)));
        }
        if p.write_failures > 0 {
            out.push_str(&format!("  {} diagnostics writes failed\n", p.write_failures));
        }
        if p.open.is_empty() {
            out.push_str("  nothing was in flight\n");
        }
        for o in &p.open {
            out.push_str(&format!(
                "  IN FLIGHT {}:{} thread {} open {} at last write  {}\n",
                o.subsystem.as_str(),
                o.op,
                o.thread,
                show_us(o.age_ms.saturating_mul(1000)),
                show_fields(&o.fields)
            ));
        }
        if let Some(r) = read_json::<RingFile>(&dir.join(format!("ring-{}.json", p.pid))) {
            out.push_str("  last entries (oldest first):\n");
            for e in r.entries.iter().rev().take(15).rev() {
                let what = match e.kind {
                    Kind::Span => format!("{} in {}", e.outcome.map_or("?", Outcome::as_str), show_us(e.dur_us.unwrap_or(0))),
                    Kind::Event => "event".to_string(),
                };
                out.push_str(&format!("    {}:{} {what}  {}\n", e.subsystem.as_str(), e.op, show_fields(&e.fields)));
            }
        }
        if let Some(s) = read_json::<StatsFile>(&dir.join(format!("stats-{}.json", p.pid))) {
            if !s.stats.is_empty() {
                out.push_str("  where the time went (leaf spans):\n");
            }
            for r in s.stats.iter().take(10) {
                out.push_str(&format!(
                    "    {}:{}  {} calls, total {}, mean {}, min {}, max {}\n",
                    r.subsystem.as_str(),
                    r.op,
                    r.count,
                    show_us(r.total_us),
                    show_us(r.total_us / r.count.max(1)),
                    show_us(r.min_us),
                    show_us(r.max_us)
                ));
            }
            let o = &s.overhead;
            if o.calls > 0 {
                let share = o.recorder_ns as f64 / (o.uptime_ms.max(1) as f64 * 1e6) * 100.0;
                out.push_str(&format!(
                    "  the recorder's own cost: {} recorded calls took {} on the working threads (about {share:.3}% of one core over {}); writer thread {}\n",
                    o.calls,
                    show_us(o.recorder_ns / 1000),
                    show_us(o.uptime_ms.saturating_mul(1000)),
                    show_us(o.writer_ns / 1000)
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!("lcc-diag-test-{}-{tag}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)))
    }

    fn top<'a>(d: &'a Diag, op: &'static str) -> Span<'a> {
        d.span(Subsystem::Commit, op, Class::Top, &[])
    }

    fn cfg(text: &str) -> Config {
        let (c, issues) = Config::parse(text);
        assert!(issues.is_empty(), "{issues:?}");
        c
    }

    fn inflight(dir: &Path, d: &Diag) -> InflightFile {
        read_json(&dir.join(format!("inflight-{}.json", d.pid))).unwrap()
    }

    #[test]
    fn a_text_is_cut_on_a_character_boundary() {
        let long = "é".repeat(TEXT_MAX_CHARS * 3);
        let Val::Text(t) = Val::text(&long) else { return };
        assert_eq!(t.chars().count(), TEXT_MAX_CHARS);
        assert_eq!(Val::text("abc"), Val::Text("abc".into()));
        assert_eq!(Val::text(""), Val::Text(String::new()));
    }

    #[test]
    fn a_span_that_records_nothing_is_harmless() {
        let mut s = Span::none();
        assert!(!s.recording());
        s.set("a", Val::U64(1));
        s.note("a", Val::U64(1));
        s.finish(Outcome::Ok);
    }

    #[test]
    fn subsystem_and_level_tables_agree_with_their_names() {
        for (i, s) in Subsystem::ALL.iter().enumerate() {
            assert_eq!(s.index(), i);
            assert_eq!(Subsystem::parse(s.as_str()), Some(*s));
        }
        let mut names: Vec<_> = Subsystem::ALL.iter().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Subsystem::COUNT, "names are distinct");
        for l in Level::ALL {
            assert_eq!(Level::parse(l.as_str()), Some(l));
            assert_eq!(level_of(l as u8), Some(l));
        }
        assert_eq!(level_of(0), None);
        assert_eq!(Subsystem::parse("Import"), None, "names are lower case");
        assert_eq!(Subsystem::parse(""), None);
        assert_eq!(Level::parse(""), None);
    }

    #[test]
    fn a_call_is_recorded_at_the_threshold_and_below_but_not_above() {
        let d = Diag::new(None, 8);
        for t in [None, Some(Level::Critical), Some(Level::Normal), Some(Level::Detail), Some(Level::Trace)] {
            d.set_level(Some(Subsystem::Ui), t);
            for l in Level::ALL {
                let expected = t.is_some_and(|t| l <= t);
                assert_eq!(d.wants(Subsystem::Ui, l), expected, "threshold {t:?}, call {l:?}");
            }
        }
    }

    #[test]
    fn config_entries_set_the_default_and_overrides_in_any_order() {
        let a = cfg("critical,import=trace");
        let b = cfg("import=trace,critical");
        assert_eq!(a, b);
        assert_eq!(a.level_of(Subsystem::Import), Some(Level::Trace));
        assert_eq!(a.level_of(Subsystem::Ui), Some(Level::Critical));
        let c = cfg("  normal , probe=off ,, probe=detail ");
        assert_eq!(c.level_of(Subsystem::Probe), Some(Level::Detail), "the last entry for a subsystem wins");
        assert_eq!(c.level_of(Subsystem::App), Some(Level::Normal));
        assert_eq!(cfg("off").level_of(Subsystem::App), None);
        assert_eq!(cfg("detail,commit=off").level_of(Subsystem::Commit), None);
        for on in ["1", "on", "true"] {
            assert_eq!(cfg(on), Config::default(), "{on} means normal");
        }
        assert_eq!(cfg(""), Config::default());
    }

    #[test]
    fn config_entries_that_are_not_understood_are_reported_and_the_rest_still_applies() {
        let (c, issues) = Config::parse("loud,import=trace,imprt=trace,ui=2,commit=");
        assert_eq!(c.level_of(Subsystem::Import), Some(Level::Trace));
        assert_eq!(c.level_of(Subsystem::Ui), Some(Level::Normal), "a bad value leaves the subsystem as it was");
        assert_eq!(
            issues,
            vec![
                ConfigIssue { entry: "loud".into(), problem: ConfigProblem::UnknownLevel },
                ConfigIssue { entry: "imprt=trace".into(), problem: ConfigProblem::UnknownSubsystem },
                ConfigIssue { entry: "ui=2".into(), problem: ConfigProblem::UnknownLevel },
                ConfigIssue { entry: "commit=".into(), problem: ConfigProblem::UnknownLevel },
            ]
        );
    }

    #[test]
    fn import_at_trace_and_everything_else_at_critical_records_only_what_each_admits() {
        let d = Diag::with_config(None, 64, &cfg("critical,import=trace"));
        d.span(Subsystem::Import, "item", Class::Leaf, &[]).finish(Outcome::Ok);
        top(&d, "commit_stage").finish(Outcome::Ok);
        d.event(Subsystem::Ui, "slow_frame", &[]);
        d.event_at(Level::Critical, Subsystem::Ui, "freeze", &[]);
        d.event_at(Level::Trace, Subsystem::Import, "detail_event", &[]);
        let snap = d.snapshot(100);
        assert_eq!(snap.stats.iter().map(|r| r.op.as_str()).collect::<Vec<_>>(), ["item"]);
        let ops: Vec<_> = snap.ring.iter().map(|e| e.op.as_str()).collect();
        assert_eq!(ops, ["freeze", "detail_event"], "the commit stage (normal) and the slow frame (normal) were not recorded");
    }

    #[test]
    fn a_call_below_the_threshold_leaves_no_trace_and_is_not_counted() {
        let d = Diag::with_config(None, 8, &cfg("off"));
        let mut s = d.span(Subsystem::Commit, "x", Class::Top, &[("k", Val::U64(1))]);
        s.set("a", Val::U64(1));
        s.note("n", Val::U64(1));
        s.finish(Outcome::Ok);
        d.event(Subsystem::Ui, "e", &[]);
        d.span(Subsystem::Probe, "leaf", Class::Leaf, &[]).finish(Outcome::Ok);
        let snap = d.snapshot(10);
        assert!(snap.ring.is_empty() && snap.open.is_empty() && snap.stats.is_empty());
        assert_eq!(snap.overhead.calls, 0);
        assert_eq!(d.seq.load(Ordering::Relaxed), 1, "not even a sequence number was taken");
    }

    #[test]
    fn a_threshold_can_be_changed_while_running() {
        let d = Diag::with_config(None, 8, &cfg("critical"));
        d.event(Subsystem::Ui, "before", &[]);
        d.set_level(Some(Subsystem::Ui), Some(Level::Normal));
        d.event(Subsystem::Ui, "after", &[]);
        d.set_level(None, None);
        d.event(Subsystem::Ui, "off", &[]);
        let ring = d.snapshot(10).ring;
        assert_eq!(ring.iter().map(|e| e.op.as_str()).collect::<Vec<_>>(), ["after"]);
    }

    #[test]
    fn recorded_calls_are_counted_in_the_overhead() {
        let d = Diag::new(None, 8);
        top(&d, "a").finish(Outcome::Ok); // begin + end: two calls
        d.event(Subsystem::Ui, "e", &[]);
        d.span(Subsystem::Probe, "leaf", Class::Leaf, &[]).finish(Outcome::Ok); // a leaf span is one call
        let o = d.snapshot(0).overhead;
        assert_eq!(o.calls, 4);
        assert_eq!(o.writer_ns, 0);
        for _ in 0..LEAF_SAMPLE_EVERY * 4 {
            d.span(Subsystem::Probe, "leaf", Class::Leaf, &[]).finish(Outcome::Ok);
        }
        let o = d.snapshot(0).overhead;
        assert_eq!(o.calls, 4 + LEAF_SAMPLE_EVERY * 4);
        assert!(o.recorder_ns > 0, "sampled ends give a cost estimate");
    }

    #[test]
    fn the_ring_never_holds_more_than_its_cap_and_keeps_a_failure() {
        let d = Diag::new(None, 8);
        top(&d, "bad").finish(Outcome::Failed);
        for _ in 0..50 {
            top(&d, "routine").finish(Outcome::Ok);
        }
        let snap = d.snapshot(1000);
        assert!(snap.ring.len() <= 8, "{}", snap.ring.len());
        assert!(snap.ring.iter().any(|e| e.op == "bad"), "the failure outlived 50 routine successes");
        assert_eq!(snap.ring.iter().filter(|e| e.op == "routine").count(), snap.ring.len() - 1);
    }

    #[test]
    fn at_the_cap_the_oldest_goes_when_nothing_is_routine() {
        let d = Diag::new(None, 8);
        for i in 0..8u64 {
            d.event(Subsystem::Ui, "e", &[("i", Val::U64(i))]);
        }
        d.event(Subsystem::Ui, "e", &[("i", Val::U64(8))]);
        let ring = d.snapshot(100).ring;
        assert_eq!(ring.len(), 8);
        assert_eq!(ring[0].fields[0].1, Val::U64(1));
        assert_eq!(ring[7].fields[0].1, Val::U64(8));
    }

    #[test]
    fn a_more_verbose_record_is_evicted_before_a_less_verbose_one_whatever_their_age() {
        let d = Diag::new(None, 8);
        d.event_at(Level::Critical, Subsystem::App, "oldest_critical", &[]);
        d.event_at(Level::Trace, Subsystem::Ui, "trace_failure_like", &[]);
        for _ in 0..6 {
            d.event_at(Level::Normal, Subsystem::Ui, "normal", &[]);
        }
        // Full (8). Each further normal event must push out the trace one first, then a normal one, never the critical one.
        for _ in 0..20 {
            d.event_at(Level::Normal, Subsystem::Ui, "normal", &[]);
        }
        let ring = d.snapshot(100).ring;
        assert_eq!(ring.len(), 8);
        assert!(ring.iter().any(|e| e.op == "oldest_critical"), "a critical record outlives 20 normal ones");
        assert!(!ring.iter().any(|e| e.op == "trace_failure_like"));
    }

    #[test]
    fn the_ring_reads_back_in_push_order_across_levels() {
        let d = Diag::new(None, 16);
        let order = [Level::Trace, Level::Critical, Level::Detail, Level::Normal, Level::Critical, Level::Trace];
        for (i, l) in order.iter().enumerate() {
            d.event_at(*l, Subsystem::Ui, "e", &[("i", Val::U64(i as u64))]);
        }
        let got: Vec<_> = d.snapshot(100).ring.iter().map(|e| e.fields[0].1.clone()).collect();
        assert_eq!(got, (0..6).map(Val::U64).collect::<Vec<_>>());
        let tail: Vec<_> = d.snapshot(2).ring.iter().map(|e| e.fields[0].1.clone()).collect();
        assert_eq!(tail, vec![Val::U64(4), Val::U64(5)]);
    }

    #[test]
    fn leaf_spans_are_counted_and_timed_not_listed_or_kept() {
        let d = Diag::new(None, 8);
        for _ in 0..3 {
            d.span(Subsystem::Commit, "apply_photo", Class::Leaf, &[]).finish(Outcome::Ok);
        }
        let snap = d.snapshot(10);
        assert!(snap.ring.is_empty() && snap.open.is_empty());
        let row = &snap.stats[0];
        assert_eq!((row.op.as_str(), row.count), ("apply_photo", 3));
        assert!(row.min_us <= row.max_us && row.total_us >= row.max_us);
    }

    #[test]
    fn a_leaf_name_seen_by_two_recorders_on_one_thread_keeps_separate_totals() {
        let a = Diag::new(None, 8);
        let b = Diag::new(None, 8);
        a.span(Subsystem::Commit, "x", Class::Leaf, &[]).finish(Outcome::Ok);
        b.span(Subsystem::Commit, "x", Class::Leaf, &[]).finish(Outcome::Ok);
        b.span(Subsystem::Commit, "x", Class::Leaf, &[]).finish(Outcome::Ok);
        assert_eq!((a.snapshot(0).stats[0].count, b.snapshot(0).stats[0].count), (1, 2));
    }

    #[test]
    fn a_span_dropped_without_a_result_is_abandoned_and_one_unwinding_is_panicked() {
        let d = Diag::new(None, 8);
        drop(top(&d, "early_return"));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _s = top(&d, "boom");
            std::panic::resume_unwind(Box::new("x"));
        }));
        assert!(r.is_err());
        let ring = d.snapshot(10).ring;
        assert_eq!(ring[0].outcome, Some(Outcome::Abandoned));
        assert_eq!(ring[1].outcome, Some(Outcome::Panicked));
        assert!(d.snapshot(10).open.is_empty());
    }

    #[test]
    fn done_maps_a_result_to_an_outcome_and_extra_fields_reach_the_entry() {
        let d = Diag::new(None, 8);
        let mut s = top(&d, "x");
        s.set("photos", Val::U64(7));
        s.done(&Err::<(), ()>(()));
        let e = d.snapshot(1).ring.remove(0);
        assert_eq!(e.outcome, Some(Outcome::Failed));
        assert_eq!(e.fields, vec![("photos".to_string(), Val::U64(7))]);
        assert_eq!(e.level, Level::Normal);
    }

    #[test]
    fn a_critical_top_span_is_on_disk_at_once_and_a_normal_one_only_after_a_tick() {
        let dir = temp("inflight");
        let d = Diag::new(Some(dir.clone()), 8);
        d.writer.store(true, Ordering::Relaxed); // as if the writer thread ran: changes no longer write inline
        let critical = d.span_at(Level::Critical, Subsystem::Commit, "commit", Class::Top, &[("photos", Val::U64(5))]);
        let file = inflight(&dir, &d);
        assert_eq!((file.open.len(), file.open[0].op.as_str(), file.open[0].fields[0].1.clone()), (1, "commit", Val::U64(5)));
        let normal = top(&d, "stage");
        assert_eq!(inflight(&dir, &d).open.len(), 1, "the normal span is not in the file yet");
        d.tick();
        assert_eq!(inflight(&dir, &d).open.len(), 2);
        normal.finish(Outcome::Ok);
        assert_eq!(inflight(&dir, &d).open.len(), 2, "its end is not written yet either");
        critical.finish(Outcome::Ok);
        let file = inflight(&dir, &d);
        assert!(file.open.is_empty(), "a critical end writes everything, the normal span's end included");
        let ring: RingFile = read_json(&dir.join(format!("ring-{}.json", d.pid))).unwrap();
        assert_eq!(ring.entries.iter().map(|e| e.op.as_str()).collect::<Vec<_>>(), ["stage", "commit"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_tick_writes_only_when_something_changed() {
        let dir = temp("tick");
        let d = Diag::new(Some(dir.clone()), 8);
        d.writer.store(true, Ordering::Relaxed);
        d.tick();
        assert!(!dir.join(format!("ring-{}.json", d.pid)).exists(), "nothing happened yet");
        d.event(Subsystem::Ui, "e", &[]);
        d.tick();
        let path = dir.join(format!("ring-{}.json", d.pid));
        assert!(path.exists());
        std::fs::remove_file(&path).unwrap();
        d.tick();
        assert!(!path.exists(), "no change, no write");
        d.span(Subsystem::Probe, "leaf", Class::Leaf, &[]).finish(Outcome::Ok);
        d.tick();
        assert!(path.exists(), "a leaf span counts as a change (its stats moved)");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn without_a_writer_thread_a_change_writes_inline_but_rate_limited() {
        let dir = temp("inline");
        let d = Diag::new(Some(dir.clone()), 8);
        d.flush_every_ms.store(1_000_000, Ordering::Relaxed);
        d.event(Subsystem::Ui, "first", &[]);
        let path = dir.join(format!("ring-{}.json", d.pid));
        assert!(path.exists(), "the first change is due at once");
        let size = std::fs::metadata(&path).unwrap().len();
        d.event(Subsystem::Ui, "second", &[]);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), size, "inside the interval the file is not rewritten");
        d.flush();
        assert!(std::fs::metadata(&path).unwrap().len() > size);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_note_reaches_the_in_flight_file_and_one_handle_passes_at_most_one_in_fifty_ms() {
        let dir = temp("note");
        let d = Diag::new(Some(dir.clone()), 8);
        d.writer.store(true, Ordering::Relaxed);
        let s = top(&d, "apply");
        s.note("done", Val::U64(1));
        s.note("done", Val::U64(2)); // inside the window: dropped
        d.tick();
        assert_eq!(inflight(&dir, &d).open[0].fields, vec![("done".to_string(), Val::U64(1))]);
        // The thread-level note is not rate limited by a handle.
        d.note("done", Val::U64(3));
        d.tick();
        assert_eq!(inflight(&dir, &d).open[0].fields, vec![("done".to_string(), Val::U64(3))]);
        drop(s);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_note_goes_to_the_calling_threads_innermost_span_only() {
        let d = Diag::new(None, 8);
        let _outer = top(&d, "outer");
        let _inner = top(&d, "inner");
        d.note("k", Val::U64(1));
        let open = d.snapshot(0).open;
        assert!(open[0].fields.is_empty());
        assert_eq!(open[1].fields.len(), 1);
        std::thread::scope(|s| {
            s.spawn(|| d.note("other", Val::U64(2)));
        });
        assert!(d.snapshot(0).open.iter().all(|o| o.fields.iter().all(|(k, _)| k != "other")), "another thread has no open span of its own");
    }

    #[test]
    fn a_failed_write_is_counted_and_never_fails_the_work() {
        let dir = temp("blocked");
        std::fs::write(&dir, b"a file where the directory should be").unwrap();
        let d = Diag::new(Some(dir.clone()), 8);
        top(&d, "x").finish(Outcome::Ok);
        assert_eq!(d.snapshot(1).ring.len(), 1);
        let d = Diag { dir: Some(dir.join("gone")), ..Diag::new(None, 8) };
        top(&d, "y").finish(Outcome::Ok);
        d.flush();
        assert!(d.snapshot(1).write_failures > 0);
        let _ = std::fs::remove_file(dir);
    }

    #[test]
    fn many_threads_recording_leaf_spans_lose_none() {
        let d = Diag::new(None, 8);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..1000 {
                        d.span(Subsystem::Probe, "probe_file", Class::Leaf, &[]).finish(Outcome::Ok);
                    }
                });
            }
        });
        let row = &d.snapshot(0).stats[0];
        assert_eq!(row.count, 8000);
        assert!(row.min_us <= row.max_us && row.total_us >= row.max_us);
    }

    #[test]
    fn old_processes_files_are_pruned_but_not_our_own() {
        let dir = temp("prune");
        std::fs::create_dir_all(&dir).unwrap();
        for pid in 1000..1000 + (KEEP_PROCESSES as u32 + 5) {
            std::fs::write(dir.join(format!("inflight-{pid}.json")), b"{}").unwrap();
        }
        let own = std::process::id();
        std::fs::write(dir.join(format!("ring-{own}.json")), b"{}").unwrap();
        std::fs::write(dir.join("unrelated.txt"), b"x").unwrap();
        prune(&dir, own);
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.contains(&format!("ring-{own}.json")) && names.contains(&"unrelated.txt".to_string()));
        assert_eq!(names.iter().filter(|n| n.starts_with("inflight-")).count(), KEEP_PROCESSES);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_report_names_what_was_in_flight_the_levels_and_the_recorders_cost() {
        let dir = temp("report");
        let d = Diag::with_config(Some(dir.clone()), 8, &cfg("normal,import=trace,probe=off"));
        let _open = d.span(Subsystem::Commit, "lightroom_commit", Class::Top, &[("stage", Val::text("apply"))]);
        d.span(Subsystem::Import, "apply_photo", Class::Leaf, &[]).finish(Outcome::Ok);
        d.event(Subsystem::Ui, "slow_frame", &[("ms", Val::U64(105_936))]);
        d.flush();
        let gone = InflightFile {
            schema: SCHEMA,
            pid: 4_000_000,
            started_ms: 1,
            written_ms: 2,
            write_failures: 0,
            levels: Vec::new(),
            open: vec![OpenView {
                seq: 1,
                ts_ms: 1,
                age_ms: 5000,
                subsystem: Subsystem::Probe,
                op: "probe_all".into(),
                thread: 1,
                fields: vec![("done".into(), Val::U64(10))],
            }],
        };
        std::fs::write(dir.join("inflight-4000000.json"), serde_json::to_vec(&gone).unwrap()).unwrap();
        let text = report(&dir);
        assert!(text.contains("IN FLIGHT commit:lightroom_commit") && text.contains("stage=apply"), "{text}");
        assert!(text.contains("slow_frame") && text.contains("ms=105936") && text.contains("apply_photo"), "{text}");
        assert!(text.contains("process 4000000 (gone") && text.contains("IN FLIGHT probe:probe_all") && text.contains("done=10"), "{text}");
        assert!(text.contains(&format!("process {} (running", d.pid)), "{text}");
        assert!(text.contains("levels: all others normal, import trace, probe off"), "{text}");
        assert!(text.contains("the recorder's own cost:") && text.contains("recorded calls"), "{text}");
        assert!(report(&temp("none")).starts_with("no diagnostics files"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_file_is_skipped_by_the_report_not_a_panic() {
        let dir = temp("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("inflight-77.json"), b"{not json").unwrap();
        assert!(report(&dir).starts_with("no diagnostics files"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hostile_files_in_the_directory_never_panic_the_report_or_the_pruning() {
        let dir = temp("hostile");
        std::fs::create_dir_all(&dir).unwrap();
        let cases: &[(&str, &[u8])] = &[
            ("inflight-1.json", b""),
            ("inflight-2.json", b"null"),
            ("inflight-3.json", br#"{"schema":1,"pid":3,"started_ms":1,"written_ms":2,"write_failures":0,"open":[{"seq":1}]}"#),
            ("inflight-4.json", br#"{"schema":1,"pid":4,"started_ms":18446744073709551615,"written_ms":18446744073709551615,"write_failures":0,"open":[{"seq":1,"ts_ms":1,"age_ms":18446744073709551615,"subsystem":"commit","op":"x","thread":1,"fields":[["k",{"t":"len","v":18446744073709551615}]]}]}"#),
            ("ring-4.json", b"\xff\xfe not utf-8"),
            ("stats-4.json", br#"{"schema":1,"pid":4,"overhead":{"calls":1,"recorder_ns":18446744073709551615,"writer_ns":0,"uptime_ms":0},"stats":[{"subsystem":"probe","op":"x","count":0,"total_us":18446744073709551615,"min_us":0,"max_us":0}]}"#),
            ("inflight-5.json", br#"{"schema":1,"pid":5,"started_ms":1,"written_ms":2,"write_failures":0,"open":[{"seq":1,"ts_ms":1,"age_ms":1,"subsystem":"no_such","op":"x","thread":1,"fields":[]}]}"#),
            ("inflight-x.json", b"{}"),
            ("inflight-99999999999999999999.json", b"{}"),
        ];
        for (name, bytes) in cases {
            std::fs::write(dir.join(name), bytes).unwrap();
        }
        let text = report(&dir);
        assert!(text.contains("process 4 "), "the one well-formed file is still reported: {text}");
        prune(&dir, 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
