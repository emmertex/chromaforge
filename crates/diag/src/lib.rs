//! What was this process doing when it hung, was killed or panicked?
//!
//! A small diagnostics recorder (issue #1363): typed spans and events, a write-ahead in-flight file, a bounded ring
//! that evicts verbose records first, aggregate timings (count / failed / total / min / max per name) for high-volume
//! leaf spans, and a text report. Off by default; nothing is sent anywhere. See `docs/diagnostics.md`.
//!
//! - **Top spans** are whole steps (the probe of a batch, a commit, a stage of it). Each is listed in the in-flight
//!   file while it runs. A process that hangs, is killed or runs out of memory leaves the file behind: it says what was
//!   running and how far it had got ([`Span::note`]). A completed top span goes into the ring.
//! - **Leaf spans** are routine per-item work (one photo applied, one file probed). They are not listed and not put in
//!   the ring: their duration is added to the stats file's per-name count / failed / total / min / max.
//! - **Events** are things that happened (the start, a panic, a configuration that was not understood), with no
//!   duration.
//! - **The ring** keeps the newest [`RING_CAP`] entries. When it is full, a newcomer pushes out an entry at least as
//!   verbose as itself: the most verbose level first, within it a routine success, else the oldest. When everything in
//!   the ring is less verbose than the newcomer, the newcomer itself is dropped. Every drop is counted.
//!
//! # Levels
//!
//! Every call has a [`Level`] (`critical` < `normal` < `detail` < `trace`) and every [`Subsystem`] has a threshold. A
//! call is recorded when its level is at or below its subsystem's threshold; a threshold of `off` records nothing.
//! A call that is not recorded costs one atomic load: no lock, no allocation, no clock read. A `critical` span or
//! event is on disk before the call returns (the outermost steps and panics: what you need after a kill); everything
//! else is written by a background thread at most every 250 ms, so the working thread never waits for the disk.
//! Panics are recorded whenever the recorder is on, whatever the thresholds.
//!
//! `<PREFIX>_DIAG` takes a comma-separated list: a bare level sets every subsystem, `name=level` overrides one:
//! `normal` (also `1`, `on`, `true`, `yes`), `critical,import=trace`, `detail,probe=off`. Words are not case-sensitive;
//! `0`, `off`, `false` and `no` mean off, and a list that records nothing leaves the recorder off. An entry that is not
//! understood is ignored and counted in a `config_rejected` event. What each call site uses: `critical` for a step
//! that runs once per user action and matters after a kill, `normal` for stages and per-batch steps, `detail` for work
//! that runs after every command, `trace` for per-item work.
//!
//! Records are typed: [`Subsystem`], [`Outcome`], [`Level`] and [`Class`] are closed enums and field keys are
//! `&'static str`. Nothing parses a message string. A field value is a number, a flag, a length or a short text of at
//! most [`TEXT_MAX_CHARS`] characters: callers give ids, counts and format names, **never file names, paths or tag
//! values**. A panic is recorded with its source file's name (not its path), line and column.
//!
//! Its own cost on the working threads is measured ([`Overhead`]); writing diagnostics never fails the work being
//! diagnosed: a failed write is counted ([`Snapshot::write_failures`]) and nothing else.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![deny(clippy::indexing_slicing)]

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once, OnceLock, PoisonError, TryLockError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Entries the ring keeps.
pub const RING_CAP: usize = 512;
/// Longest [`Val::Text`], in characters.
pub const TEXT_MAX_CHARS: usize = 64;
/// Processes that ended cleanly whose files are kept when a new process starts.
const KEEP_CLEAN: usize = 12;
/// Processes that did not end cleanly (still running, hung, killed, crashed) whose files are kept: the evidence the
/// recorder exists for, so more of them, and a run of clean starts never pushes them out.
const KEEP_UNCLEAN: usize = 24;
/// The writer thread rewrites the files at most this often.
const FLUSH_EVERY_MS: u64 = 250;
/// A span handle passes a [`Span::note`] at most this often (the file is written every [`FLUSH_EVERY_MS`] anyway).
const NOTE_MIN_NS: u64 = 50_000_000;
/// The report reads no file larger than this.
const MAX_FILE_BYTES: u64 = 16 << 20;
/// The format of the three files; the report says when a file has another.
const SCHEMA: u32 = 1;

/// Where a record comes from. A closed set: adding one is a code change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subsystem {
    /// The recorder itself: start, panics, configuration it did not understand.
    App,
    /// Adding files to the library: the import commit of each batch.
    Import,
    /// Reading files before an import, a reload or a recheck (hash, format, metadata).
    Probe,
    /// Committing a Lightroom catalog import and its stages.
    Commit,
    /// Writing the library to disk after a command (persist, flush).
    Catalog,
}

impl Subsystem {
    /// Every subsystem, in [`Subsystem::index`] order.
    pub const ALL: [Subsystem; 5] = [Subsystem::App, Subsystem::Import, Subsystem::Probe, Subsystem::Commit, Subsystem::Catalog];
    pub const COUNT: usize = Self::ALL.len();

    /// Position in [`Subsystem::ALL`]; the match is exhaustive, so a new variant cannot be forgotten here.
    pub const fn index(self) -> usize {
        match self {
            Subsystem::App => 0,
            Subsystem::Import => 1,
            Subsystem::Probe => 2,
            Subsystem::Commit => 3,
            Subsystem::Catalog => 4,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Subsystem::App => "app",
            Subsystem::Import => "import",
            Subsystem::Probe => "probe",
            Subsystem::Commit => "commit",
            Subsystem::Catalog => "catalog",
        }
    }

    /// The subsystem called `name` (the [`Subsystem::as_str`] spelling, any case).
    pub fn parse(name: &str) -> Option<Subsystem> {
        Self::ALL.iter().copied().find(|s| s.as_str().eq_ignore_ascii_case(name))
    }
}

/// How much a record matters, and so how verbose a recorder must be set to keep it: `Critical` is the least verbose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Steps that run once per user action and matter after a kill, and panics: written to disk at once.
    Critical = 1,
    /// Stages of a step, per-batch steps.
    Normal = 2,
    /// Work that runs after every command.
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

    /// The level called `name`, any case.
    pub fn parse(name: &str) -> Option<Level> {
        Self::ALL.iter().copied().find(|l| l.as_str().eq_ignore_ascii_case(name))
    }
}

/// A subsystem's threshold as stored: `0` = off, else a [`Level`]'s number.
fn threshold_of(level: Option<Level>) -> u8 {
    level.map_or(0, |l| l as u8)
}

fn level_of(threshold: u8) -> Option<Level> {
    Level::ALL.iter().copied().find(|l| *l as u8 == threshold)
}

/// A threshold word, any case: `off`/`0`/`false`/`no`, a level name, or `on`/`1`/`true`/`yes` (= `normal`). The one
/// list of on and off words: [`start_from_env`] decides "off" through it too.
fn parse_threshold(word: &str) -> Option<u8> {
    match word.to_ascii_lowercase().as_str() {
        "off" | "0" | "false" | "no" => Some(0),
        "on" | "1" | "true" | "yes" => Some(Level::Normal as u8),
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

    /// Whether any subsystem records anything: a configuration that records nothing leaves the recorder off.
    pub fn records_anything(&self) -> bool {
        Subsystem::ALL.iter().any(|s| self.threshold(*s) > 0)
    }

    fn threshold(&self, subsystem: Subsystem) -> u8 {
        self.per.get(subsystem.index()).copied().flatten().unwrap_or(self.default)
    }
}

/// How a span ended. `Abandoned`: the guard was dropped without a result, i.e. its code left by an early return (an
/// error passed up with `?`, usually); the enclosing step's outcome says what the error was. `Panicked`: dropped while
/// unwinding.
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

/// How a process ended, as far as it could say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ended {
    /// [`finish`] ran: the process was ending on purpose.
    Clean,
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
            Val::Text(t) => printable(t),
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
    pub level: Level,
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
    /// Of `count`, the ones that ended other than `ok`.
    pub failed: u64,
    pub total_us: u64,
    pub min_us: u64,
    pub max_us: u64,
}

/// What the recorder itself cost.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overhead {
    /// Recorded top spans, events and notes. Leaf spans are counted in the stats, not here, and their cost is not
    /// measured on each call (`examples/bench.rs` measures it).
    pub calls: u64,
    /// Time spent inside those calls on the calling threads, lock waits included, in nanoseconds.
    pub recorder_ns: u64,
    /// Time spent serialising and writing the files, in nanoseconds.
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

/// Everything in memory, for tests and the benchmark.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub pid: u32,
    pub open: Vec<OpenView>,
    pub ring: Vec<Entry>,
    pub stats: Vec<StatRow>,
    pub write_failures: u64,
    pub panics_lost: u64,
    pub evicted: u64,
    pub ended: Option<Ended>,
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
    panics_lost: u64,
    ended: Option<Ended>,
    levels: Vec<LevelRow>,
    open: Vec<OpenView>,
}

#[derive(Serialize, Deserialize)]
struct RingFile {
    schema: u32,
    pid: u32,
    written_ms: u64,
    evicted: u64,
    entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
struct StatsFile {
    schema: u32,
    pid: u32,
    written_ms: u64,
    overhead: Overhead,
    stats: Vec<StatRow>,
}

/// The three files a process writes. The one place their names are made and recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Inflight,
    Ring,
    Stats,
}

impl FileKind {
    const ALL: [FileKind; 3] = [FileKind::Inflight, FileKind::Ring, FileKind::Stats];

    const fn stem(self) -> &'static str {
        match self {
            FileKind::Inflight => "inflight",
            FileKind::Ring => "ring",
            FileKind::Stats => "stats",
        }
    }

    /// `<stem>-<pid>-<started ms>.json`: the start time keeps a later process that reuses the pid from overwriting an
    /// earlier one's files.
    fn name(self, run: RunId) -> String {
        format!("{}-{}-{}.json", self.stem(), run.pid, run.started_ms)
    }
}

/// One process run: its pid and when its recorder started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RunId {
    pid: u32,
    started_ms: u64,
}

/// A file name this recorder writes, and nothing else: exactly `<stem>-<digits>-<digits>.json`, or the same with
/// `.tmp` (a write that never finished). Anything else in the folder is not ours.
fn parse_name(name: &str) -> Option<(FileKind, RunId)> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    FileKind::ALL.iter().copied().find_map(|kind| {
        let rest = name.strip_prefix(kind.stem())?.strip_prefix('-')?;
        let rest = rest.strip_suffix(".json.tmp").or_else(|| rest.strip_suffix(".json"))?;
        let (pid, ms) = rest.split_once('-')?;
        (digits(pid) && digits(ms)).then_some(())?;
        Some((kind, RunId { pid: pid.parse().ok()?, started_ms: ms.parse().ok()? }))
    })
}

struct Open {
    seq: u64,
    ts_ms: u64,
    subsystem: Subsystem,
    level: Level,
    op: &'static str,
    thread: u64,
    started: Instant,
    fields: Vec<(String, Val)>,
}

#[derive(Default)]
struct State {
    open: Vec<Open>,
    /// Completed spans and events, in push order.
    ring: VecDeque<Entry>,
    /// Entries pushed out of the ring, or not let in, because it was full.
    evicted: u64,
    /// How many ring entries each level has (index: level number - 1), so eviction scans only levels that are there.
    per_level: [usize; 4],
    ended: Option<Ended>,
    /// Bumped by every render: the files on disk never go back to an older render ([`Diag::put`]).
    generation: u64,
    last_flush: Option<Instant>,
}

/// Stripes per leaf name: a thread writes the stripe its number picks, so threads rarely share a cache line, and the
/// memory per name is fixed however many threads come and go.
const STRIPES: usize = 16;

/// One stripe of a leaf name's totals, on its own cache line.
#[repr(align(128))]
struct AtomicStat {
    count: AtomicU64,
    failed: AtomicU64,
    total_us: AtomicU64,
    min_us: AtomicU64,
    max_us: AtomicU64,
}

impl AtomicStat {
    fn new() -> AtomicStat {
        AtomicStat {
            count: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            total_us: AtomicU64::new(0),
            min_us: AtomicU64::new(u64::MAX),
            max_us: AtomicU64::new(0),
        }
    }

    fn record(&self, us: u64, failed: bool) {
        self.total_us.fetch_add(us, Ordering::Relaxed);
        self.min_us.fetch_min(us, Ordering::Relaxed);
        self.max_us.fetch_max(us, Ordering::Relaxed);
        if failed {
            self.failed.fetch_add(1, Ordering::Relaxed);
        }
        // last, with Release: a reader that sees the count sees the rest
        self.count.fetch_add(1, Ordering::Release);
    }
}

/// A leaf name's totals: [`STRIPES`] stripes, summed when read.
struct LeafStats {
    stripes: [AtomicStat; STRIPES],
}

impl LeafStats {
    fn new() -> LeafStats {
        LeafStats { stripes: std::array::from_fn(|_| AtomicStat::new()) }
    }

    fn record(&self, us: u64, failed: bool) {
        let i = (thread_num() as usize) % STRIPES;
        if let Some(stripe) = self.stripes.get(i) {
            stripe.record(us, failed);
        }
    }

    /// (count, failed, total, min, max) over every stripe; min is `u64::MAX` when nothing was recorded.
    fn totals(&self) -> (u64, u64, u64, u64, u64) {
        self.stripes.iter().fold((0, 0, 0, u64::MAX, 0), |(n, f, t, lo, hi), s| {
            let count = s.count.load(Ordering::Acquire);
            if count == 0 {
                return (n, f, t, lo, hi);
            }
            (
                n.saturating_add(count),
                f.saturating_add(s.failed.load(Ordering::Relaxed)),
                t.saturating_add(s.total_us.load(Ordering::Relaxed)),
                lo.min(s.min_us.load(Ordering::Relaxed)),
                hi.max(s.max_us.load(Ordering::Relaxed)),
            )
        })
    }
}

type StatKey = (Subsystem, &'static str);

thread_local! {
    /// Per thread, the shared cell of each leaf name it has used (keyed by recorder id too: tests run several), so
    /// after the first call of a name a leaf span takes no lock.
    static LOCAL: RefCell<HashMap<(u64, Subsystem, &'static str), Arc<LeafStats>>> = RefCell::new(HashMap::new());
    static THREAD: u64 = NEXT_THREAD.fetch_add(1, Ordering::Relaxed);
}

static NEXT_THREAD: AtomicU64 = AtomicU64::new(1);

pub fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The recorder's number for the calling thread; `0` once its thread-local storage is gone (never a panic).
fn thread_num() -> u64 {
    THREAD.try_with(|id| *id).unwrap_or(0)
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

/// Bytes of the three files, made under the state lock, written outside it.
struct Rendered {
    generation: u64,
    files: Vec<(FileKind, Vec<u8>)>,
}

/// The recorder. One process-wide instance ([`start`]); tests build their own.
pub struct Diag {
    id: u64,
    dir: Option<PathBuf>,
    run: RunId,
    started: Instant,
    seq: AtomicU64,
    cap: usize,
    flush_every_ms: u64,
    thresholds: [AtomicU8; Subsystem::COUNT],
    stats: Mutex<HashMap<StatKey, Arc<LeafStats>>>,
    /// Something changed since the files were last rendered.
    dirty: AtomicBool,
    /// A writer thread is running (see [`Diag::tick`]); without one, a change writes the files itself, rate-limited.
    writer: AtomicBool,
    write_failures: AtomicU64,
    /// Panics the hook could not record because another thread held the state lock.
    panics_lost: AtomicU64,
    calls: AtomicU64,
    recorder_ns: AtomicU64,
    writer_ns: AtomicU64,
    state: Mutex<State>,
    /// Serialises file writes; holds the generation of the files on disk.
    io: Mutex<u64>,
}

impl Diag {
    /// A recorder that keeps every level, writing into `dir` (`None` keeps everything in memory, for tests) and
    /// keeping `cap` ring entries.
    pub fn new(dir: Option<PathBuf>, cap: usize) -> Diag {
        Diag::with_config(dir, cap, &Config::all(Some(Level::Trace)))
    }

    /// `dir` must exist (see [`start`]); old processes' files in it are pruned.
    pub fn with_config(dir: Option<PathBuf>, cap: usize, config: &Config) -> Diag {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let run = RunId { pid: std::process::id(), started_ms: unix_ms() };
        if let Some(d) = &dir {
            prune(d, run);
        }
        Diag {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            dir,
            run,
            started: Instant::now(),
            seq: AtomicU64::new(1),
            cap: cap.max(1),
            flush_every_ms: FLUSH_EVERY_MS,
            thresholds: std::array::from_fn(|i| AtomicU8::new(Subsystem::ALL.get(i).map_or(0, |s| config.threshold(*s)))),
            stats: Mutex::new(HashMap::new()),
            dirty: AtomicBool::new(false),
            writer: AtomicBool::new(false),
            write_failures: AtomicU64::new(0),
            panics_lost: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            recorder_ns: AtomicU64::new(0),
            writer_ns: AtomicU64::new(0),
            state: Mutex::new(State::default()),
            io: Mutex::new(0),
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

    fn mark_dirty(&self) {
        if !self.dirty.load(Ordering::Relaxed) {
            self.dirty.store(true, Ordering::Release);
        }
    }

    /// After a change under `st`: a `Critical` record is rendered now and written before returning (the state lock is
    /// released first, so other threads never wait for this disk write); anything else is left to the writer thread,
    /// or, without one, written here at most every [`FLUSH_EVERY_MS`].
    fn after_change(&self, mut st: MutexGuard<'_, State>, level: Level) {
        if self.dir.is_none() {
            return;
        }
        let due = level == Level::Critical
            || (!self.writer.load(Ordering::Relaxed) && st.last_flush.is_none_or(|t| t.elapsed().as_millis() as u64 >= self.flush_every_ms));
        if !due {
            self.mark_dirty();
            return;
        }
        let rendered = self.render(&mut st);
        drop(st);
        self.put(rendered);
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
        let t0 = Instant::now();
        if class == Class::Leaf {
            // routine per-item work: no sequence number, no lock, one clock read
            return Span::active(self, 0, subsystem, op, level, class, t0);
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut st = self.lock();
        st.open.push(Open { seq, ts_ms: unix_ms(), subsystem, level, op, thread: thread_num(), started: t0, fields: own_fields(fields) });
        self.after_change(st, level);
        self.charge(t0);
        // The span's own clock starts after the bookkeeping above, so its duration does not include the recorder's cost.
        Span::active(self, seq, subsystem, op, level, class, Instant::now())
    }

    /// Record something that happened, if `subsystem`'s threshold admits `level`. A `Critical` event is on disk before this
    /// returns.
    pub fn event_at(&self, level: Level, subsystem: Subsystem, op: &'static str, fields: &[(&'static str, Val)]) {
        if !self.wants(subsystem, level) {
            return;
        }
        let t0 = Instant::now();
        let mut st = self.lock();
        let e = self.event_entry(level, subsystem, op, own_fields(fields));
        self.push_ring(&mut st, e);
        self.after_change(st, level);
        self.charge(t0);
    }

    fn event_entry(&self, level: Level, subsystem: Subsystem, op: &str, fields: Vec<(String, Val)>) -> Entry {
        Entry {
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            ts_ms: unix_ms(),
            kind: Kind::Event,
            subsystem,
            level,
            op: op.to_string(),
            outcome: None,
            dur_us: None,
            thread: thread_num(),
            fields,
        }
    }

    fn note_seq(&self, seq: u64, key: &'static str, v: Val) {
        let t0 = Instant::now();
        let mut st = self.lock();
        let Some(o) = st.open.iter_mut().find(|o| o.seq == seq) else { return };
        set_field(&mut o.fields, key, v);
        self.after_change(st, Level::Normal);
        self.charge(t0);
    }

    fn end(&self, span: &mut Span<'_>, outcome: Outcome, dur_us: u64) {
        if span.class == Class::Leaf {
            self.record_leaf(span.subsystem, span.op, dur_us, outcome != Outcome::Ok);
            return;
        }
        let t0 = Instant::now();
        let mut st = self.lock();
        let Some(i) = st.open.iter().position(|o| o.seq == span.seq) else { return };
        let o = st.open.remove(i);
        let mut fields = o.fields;
        for (k, v) in std::mem::take(&mut span.extra) {
            set_field(&mut fields, k, v);
        }
        let e = Entry {
            seq: o.seq,
            ts_ms: o.ts_ms,
            kind: Kind::Span,
            subsystem: o.subsystem,
            level: o.level,
            op: o.op.to_string(),
            outcome: Some(outcome),
            dur_us: Some(dur_us),
            thread: o.thread,
            fields,
        };
        self.push_ring(&mut st, e);
        self.after_change(st, span.level);
        self.charge(t0);
    }

    /// Add one leaf duration to the name's shared cell: no lock after this thread's first call of that name.
    fn record_leaf(&self, subsystem: Subsystem, op: &'static str, dur_us: u64, failed: bool) {
        let key = (self.id, subsystem, op);
        // recorded through a reference: cloning the `Arc` per call would make its count one line every thread writes
        let recorded = LOCAL
            .try_with(|local| {
                let mut local = local.try_borrow_mut().ok()?;
                local.entry(key).or_insert_with(|| self.cell(subsystem, op)).record(dur_us, failed);
                Some(())
            })
            .ok()
            .flatten();
        if recorded.is_none() {
            self.cell(subsystem, op).record(dur_us, failed);
        }
        // after the update: a tick that sees the flag sees the new totals
        self.mark_dirty();
    }

    fn cell(&self, subsystem: Subsystem, op: &'static str) -> Arc<LeafStats> {
        let mut map = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(map.entry((subsystem, op)).or_insert_with(|| Arc::new(LeafStats::new())))
    }

    /// Make room for `e` (see the module docs): an entry at least as verbose as `e` goes, the most verbose level first,
    /// within it a routine success, else the oldest; if every entry is less verbose than `e`, `e` itself is dropped.
    fn push_ring(&self, st: &mut State, e: Entry) {
        if st.ring.len() >= self.cap {
            let routine = |x: &Entry| x.kind == Kind::Span && x.outcome == Some(Outcome::Ok);
            let present = |st: &State, l: Level| st.per_level.get(l as usize - 1).is_some_and(|n| *n > 0);
            let victim = Level::ALL.iter().rev().filter(|l| **l >= e.level && present(st, **l)).find_map(|l| {
                let at = |x: &&Entry| x.level == *l;
                st.ring.iter().position(|x| at(&x) && routine(x)).or_else(|| st.ring.iter().position(|x| at(&x)))
            });
            st.evicted = st.evicted.saturating_add(1);
            match victim.and_then(|i| st.ring.remove(i)) {
                Some(gone) => {
                    if let Some(n) = st.per_level.get_mut(gone.level as usize - 1) {
                        *n = n.saturating_sub(1);
                    }
                }
                None => return,
            }
        }
        if let Some(n) = st.per_level.get_mut(e.level as usize - 1) {
            *n += 1;
        }
        st.ring.push_back(e);
    }

    pub fn overhead(&self) -> Overhead {
        Overhead {
            calls: self.calls.load(Ordering::Relaxed),
            recorder_ns: self.recorder_ns.load(Ordering::Relaxed),
            writer_ns: self.writer_ns.load(Ordering::Relaxed),
            uptime_ms: self.started.elapsed().as_millis() as u64,
        }
    }

    /// What is in memory now (`tail` newest ring entries).
    pub fn snapshot(&self, tail: usize) -> Snapshot {
        let st = self.lock();
        let skip = st.ring.len().saturating_sub(tail);
        Snapshot {
            pid: self.run.pid,
            open: self.open_views(&st),
            ring: st.ring.iter().skip(skip).cloned().collect(),
            stats: self.stat_rows(),
            write_failures: self.write_failures.load(Ordering::Relaxed),
            panics_lost: self.panics_lost.load(Ordering::Relaxed),
            evicted: st.evicted,
            ended: st.ended,
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
                level: o.level,
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
            .filter_map(|((subsystem, op), c)| {
                let (count, failed, total_us, min_us, max_us) = c.totals();
                (count > 0).then(|| StatRow { subsystem: *subsystem, op: (*op).to_string(), count, failed, total_us, min_us, max_us })
            })
            .collect();
        rows.sort_by(|a, b| b.total_us.cmp(&a.total_us).then_with(|| a.op.cmp(&b.op)));
        rows
    }

    /// Write every file now, on this thread.
    pub fn flush(&self) {
        let mut st = self.lock();
        let rendered = self.render(&mut st);
        drop(st);
        self.put(rendered);
    }

    /// Mark the process as ending on purpose and write every file: the report then says "ended cleanly".
    pub fn finish(&self) {
        let mut st = self.lock();
        st.ended = Some(Ended::Clean);
        let rendered = self.render(&mut st);
        drop(st);
        self.put(rendered);
    }

    /// What the writer thread does every [`FLUSH_EVERY_MS`]: if anything changed, render under the lock and write
    /// outside it.
    pub fn tick(&self) {
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return;
        }
        let mut st = self.lock();
        let rendered = self.render(&mut st);
        drop(st);
        self.put(rendered);
    }

    /// The three files' bytes, with a new generation number. A file that can't be serialised is counted as a failed
    /// write.
    fn render(&self, st: &mut State) -> Rendered {
        let t0 = Instant::now();
        st.generation = st.generation.saturating_add(1);
        st.last_flush = Some(Instant::now());
        let written_ms = unix_ms();
        let pid = self.run.pid;
        let mut files = Vec::with_capacity(3);
        let mut add = |kind: FileKind, bytes: serde_json::Result<Vec<u8>>| match bytes {
            Ok(b) => files.push((kind, b)),
            Err(_) => {
                self.write_failures.fetch_add(1, Ordering::Relaxed);
            }
        };
        let inflight = InflightFile {
            schema: SCHEMA,
            pid,
            started_ms: self.run.started_ms,
            written_ms,
            write_failures: self.write_failures.load(Ordering::Relaxed),
            panics_lost: self.panics_lost.load(Ordering::Relaxed),
            ended: st.ended,
            levels: self.levels(),
            open: self.open_views(st),
        };
        add(FileKind::Inflight, serde_json::to_vec(&inflight));
        let ring = RingFile { schema: SCHEMA, pid, written_ms, evicted: st.evicted, entries: st.ring.iter().cloned().collect() };
        add(FileKind::Ring, serde_json::to_vec(&ring));
        let stats = StatsFile { schema: SCHEMA, pid, written_ms, overhead: self.overhead(), stats: self.stat_rows() };
        add(FileKind::Stats, serde_json::to_vec(&stats));
        self.writer_ns.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
        Rendered { generation: st.generation, files }
    }

    /// Write rendered files, one writer at a time, never older than what is on disk: a render that lost the race to a
    /// newer one is dropped, so a critical write or the final [`Diag::finish`] is never overwritten by an older tick.
    fn put(&self, rendered: Rendered) {
        let Some(dir) = &self.dir else { return };
        let t0 = Instant::now();
        let mut on_disk = self.io.lock().unwrap_or_else(PoisonError::into_inner);
        if rendered.generation <= *on_disk {
            return;
        }
        for (kind, bytes) in rendered.files {
            if write_atomic(dir, &kind.name(self.run), &bytes).is_err() {
                self.write_failures.fetch_add(1, Ordering::Relaxed);
            }
        }
        *on_disk = rendered.generation;
        drop(on_disk);
        self.writer_ns.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    /// For the panic hook: records the panic if the state lock is free (a poisoned lock is used as it is), and otherwise
    /// counts it as lost instead of waiting (the panicking thread may hold the lock itself).
    fn panic_event(&self, location: &str) {
        let mut st = match self.state.try_lock() {
            Ok(st) => st,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.panics_lost.fetch_add(1, Ordering::Relaxed);
                self.mark_dirty();
                return;
            }
        };
        let e = self.event_entry(Level::Critical, Subsystem::App, "panic", vec![("location".into(), Val::text(location))]);
        self.push_ring(&mut st, e);
        self.after_change(st, Level::Critical);
    }
}

/// `name` in `dir`, written through a temp file that is created fresh (never through a link or file someone left
/// at that name) and renamed into place. On failure the temp file is removed.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = dir.join(format!("{name}.tmp"));
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut f| f.write_all(bytes))
        .and_then(|()| std::fs::rename(&tmp, dir.join(name)));
    if result.is_err() {
        // best effort: the next write removes it, and prune removes it with its process
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The panic's source file name (not its path), line and column: `lib.rs:12:5`.
fn panic_location(file: &str, line: u32, column: u32) -> String {
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    format!("{name}:{line}:{column}")
}

/// Read a file the recorder wrote, refusing one larger than [`MAX_FILE_BYTES`].
fn read_capped(path: &Path) -> Result<Vec<u8>, &'static str> {
    let len = std::fs::metadata(path).map_err(|_| "unreadable")?.len();
    if len > MAX_FILE_BYTES {
        return Err("too large");
    }
    std::fs::read(path).map_err(|_| "unreadable")
}

/// Keep the newest [`KEEP_CLEAN`] processes that ended cleanly and the newest [`KEEP_UNCLEAN`] that did not (a hung
/// process never ended, so it stays however many clean runs follow), never `own`'s; only files [`parse_name`]
/// recognises are ever removed.
fn prune(dir: &Path, own: RunId) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut runs: HashMap<RunId, Vec<PathBuf>> = HashMap::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some((_, run)) = parse_name(&name)
            && run != own
        {
            runs.entry(run).or_default().push(entry.path());
        }
    }
    let ended_clean = |run: RunId| {
        read_capped(&dir.join(FileKind::Inflight.name(run)))
            .ok()
            .and_then(|b| serde_json::from_slice::<InflightFile>(&b).ok())
            .is_some_and(|f| f.ended == Some(Ended::Clean))
    };
    let (mut clean, mut unclean): (Vec<RunId>, Vec<RunId>) = runs.keys().copied().partition(|r| ended_clean(*r));
    for (list, keep) in [(&mut clean, KEEP_CLEAN), (&mut unclean, KEEP_UNCLEAN)] {
        list.sort_by_key(|r| std::cmp::Reverse((r.started_ms, r.pid)));
        for run in list.iter().skip(keep) {
            for f in runs.get(run).into_iter().flatten() {
                // best effort: a file that can't be removed now is tried again by the next process
                let _ = std::fs::remove_file(f);
            }
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
    /// `None` for a span that records nothing (no clock read: `Instant::now` panics on the web build).
    started: Option<Instant>,
    extra: Vec<(&'static str, Val)>,
    /// Earliest time (since the recorder started) a [`Span::note`] from this handle is passed on.
    note_next_ns: AtomicU64,
}

impl<'a> Span<'a> {
    /// A span that records nothing: what the code gets when diagnostics are off or the level is below the threshold.
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

    fn active(diag: &'a Diag, seq: u64, subsystem: Subsystem, op: &'static str, level: Level, class: Class, started: Instant) -> Span<'a> {
        Span { diag: Some(diag), seq, subsystem, op, level, class, started: Some(started), extra: Vec::new(), note_next_ns: AtomicU64::new(0) }
    }

    /// Whether this span is being recorded.
    pub fn recording(&self) -> bool {
        self.diag.is_some()
    }
}

impl Span<'_> {
    /// Set a field of this span, kept in the ring entry when it ends (a final count, say: [`Span::note`] passes values
    /// on at most every 50 ms, so the last one noted may not be the last one).
    pub fn set(&mut self, key: &'static str, v: Val) {
        if self.diag.is_some() {
            match self.extra.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = v,
                None => self.extra.push((key, v)),
            }
        }
    }

    /// Set a field of this open top span from any thread (how far it has got), shown in the in-flight file. Passed on
    /// at most every 50 ms per span handle (the file is rewritten every 250 ms anyway), so calling it per item is cheap.
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
        if self.diag.is_none() {
            return;
        }
        let outcome = if std::thread::panicking() { Outcome::Panicked } else { Outcome::Abandoned };
        self.end(outcome);
    }
}

/// Times one item of routine work as a leaf span that ends `ok` when dropped (`panicked` while unwinding): for a loop
/// body with `continue`s, where an item has no failure of its own to report.
#[must_use = "the timer measures until it is dropped; binding it to _ ends it at once"]
pub struct ItemTimer<'a>(Span<'a>);

impl Drop for ItemTimer<'_> {
    fn drop(&mut self) {
        let outcome = if std::thread::panicking() { Outcome::Panicked } else { Outcome::Ok };
        self.0.end(outcome);
    }
}

/// An [`ItemTimer`] at `trace` (nothing but one load when that level is not recorded).
pub fn item_timer(subsystem: Subsystem, op: &'static str) -> ItemTimer<'static> {
    ItemTimer(span(subsystem, op, Class::Leaf, &[]))
}

/// Run `f` inside a top span that ends with `f`'s outcome: `Ok`, or what `on_error` says an error is (`Failed`, or
/// `Cancelled` for a cancel). Every return path of `f`, `?` included, ends the span with its real outcome.
pub fn traced<T, E>(
    level: Level,
    subsystem: Subsystem,
    op: &'static str,
    fields: &[(&'static str, Val)],
    on_error: impl FnOnce(&E) -> Outcome,
    f: impl FnOnce(&Span<'static>) -> Result<T, E>,
) -> Result<T, E> {
    end_traced(span_at(level, subsystem, op, Class::Top, fields), on_error, f)
}

fn end_traced<'a, T, E>(span: Span<'a>, on_error: impl FnOnce(&E) -> Outcome, f: impl FnOnce(&Span<'a>) -> Result<T, E>) -> Result<T, E> {
    let r = f(&span);
    let outcome = match &r {
        Ok(_) => Outcome::Ok,
        Err(e) => on_error(e),
    };
    span.finish(outcome);
    r
}

static GLOBAL: OnceLock<Diag> = OnceLock::new();

/// What [`start`] / [`start_from_env`] did, for the host to log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartStatus {
    /// `<PREFIX>_DIAG` unset, empty, or a configuration that records nothing.
    Off,
    /// Recording into this folder.
    Started(PathBuf),
    /// Asked for, but there is no folder to write to (`<PREFIX>_DIAG_DIR` empty, or no settings folder in this run).
    NoFolder,
    /// Asked for, but the folder can't be created.
    FolderUnavailable(PathBuf, std::io::ErrorKind),
    /// A recorder is already running in this process.
    AlreadyRunning,
    /// Not on this platform (the web build: no clock, threads or files).
    Unsupported,
}

/// Start the process-wide recorder in `dir` (created if needed) with `config`: install the panic hook (after the
/// existing one), start the writer thread and record a `start` event.
pub fn start(dir: PathBuf, config: &Config) -> StartStatus {
    if cfg!(target_arch = "wasm32") {
        return StartStatus::Unsupported;
    }
    if GLOBAL.get().is_some() {
        return StartStatus::AlreadyRunning;
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return StartStatus::FolderUnavailable(dir, e.kind());
    }
    if GLOBAL.set(Diag::with_config(Some(dir.clone()), RING_CAP, config)).is_err() {
        return StartStatus::AlreadyRunning;
    }
    install_panic_hook();
    let writer = GLOBAL.get().is_some_and(|d| {
        let ok = std::thread::Builder::new().name("lcc-diag-writer".into()).spawn(writer_loop).is_ok();
        d.writer.store(ok, Ordering::Relaxed);
        ok
    });
    event_at(Level::Critical, Subsystem::App, "start", &[("pid", Val::U64(u64::from(std::process::id()))), ("writer_thread", Val::Bool(writer))]);
    StartStatus::Started(dir)
}

fn writer_loop() {
    loop {
        let Some(d) = GLOBAL.get() else { return };
        d.tick();
        std::thread::sleep(Duration::from_millis(d.flush_every_ms.max(10)));
    }
}

/// What `<PREFIX>_DIAG` and `<PREFIX>_DIAG_DIR` ask for: the configuration, what it did not understand and the folder;
/// or why nothing starts. `diag` is the `<PREFIX>_DIAG` value, `dir_override` the `<PREFIX>_DIAG_DIR` value (empty: no
/// folder), `default_dir` the host's folder for this run (`None` when the run keeps no settings).
fn plan_start(
    diag: Option<&str>,
    dir_override: Option<&std::ffi::OsStr>,
    default_dir: Option<PathBuf>,
) -> Result<(Config, Vec<ConfigIssue>, PathBuf), StartStatus> {
    let value = diag.map(str::trim).filter(|v| !v.is_empty()).ok_or(StartStatus::Off)?;
    let (config, issues) = Config::parse(value);
    if !config.records_anything() {
        return Err(StartStatus::Off);
    }
    let dir = match dir_override {
        Some(d) if d.is_empty() => None,
        Some(d) => Some(PathBuf::from(d)),
        None => default_dir,
    };
    let dir = dir.ok_or(StartStatus::NoFolder)?;
    Ok((config, issues, dir))
}

/// [`start`] as `<PREFIX>_DIAG` asks (a [`Config`] string), in `<PREFIX>_DIAG_DIR` or else `default_dir`: the host's
/// settings folder's `diag/`, or `None` for a run that keeps no settings (`--memory`, `<PREFIX>_NO_PREFS`). Entries
/// that are not understood are counted in a `config_rejected` event. The host logs the returned status.
pub fn start_from_env(default_dir: Option<PathBuf>) -> StartStatus {
    let diag = lcc_brand::env("DIAG");
    let dir_override = lcc_brand::env_os("DIAG_DIR");
    let (config, issues, dir) = match plan_start(diag.as_deref(), dir_override.as_deref(), default_dir) {
        Ok(plan) => plan,
        Err(status) => return status,
    };
    let status = start(dir, &config);
    if matches!(status, StartStatus::Started(_))
        && let Some(first) = issues.first()
    {
        event_at(
            Level::Critical,
            Subsystem::App,
            "config_rejected",
            &[("entries", Val::U64(issues.len() as u64)), ("first_problem", Val::text(first.problem.as_str()))],
        );
    }
    status
}

/// Whether a call at `level` in `subsystem` would be recorded now: false when diagnostics are off.
pub fn wants(subsystem: Subsystem, level: Level) -> bool {
    GLOBAL.get().is_some_and(|d| d.wants(subsystem, level))
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

pub fn event_at(level: Level, subsystem: Subsystem, op: &'static str, fields: &[(&'static str, Val)]) {
    if let Some(d) = GLOBAL.get() {
        d.event_at(level, subsystem, op, fields);
    }
}

pub fn snapshot(tail: usize) -> Option<Snapshot> {
    GLOBAL.get().map(|d| d.snapshot(tail))
}

/// Write every file now (no-op when diagnostics are off).
pub fn flush() {
    if let Some(d) = GLOBAL.get() {
        d.flush();
    }
}

/// The process is ending on purpose: mark it and write every file (no-op when diagnostics are off). Call it on every
/// orderly way out, including one that ends with `std::process::exit`.
pub fn finish() {
    if let Some(d) = GLOBAL.get() {
        d.finish();
    }
}

fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // the existing hooks first (stderr, panic log, log file), as they chain theirs
            previous(info);
            if let Some(d) = GLOBAL.get() {
                let at = info.location().map_or_else(|| "unknown".to_string(), |l| panic_location(l.file(), l.line(), l.column()));
                d.panic_event(&at);
            }
        }));
    });
}

/// Whether process `pid` is running (`ps -p`; `None` where that can't be asked or `ps` fails).
pub fn is_running(pid: u32) -> Option<bool> {
    if !cfg!(unix) {
        return None;
    }
    let out = std::process::Command::new("ps").args(["-p", &pid.to_string(), "-o", "pid="]).output().ok()?;
    let listed = String::from_utf8_lossy(&out.stdout).trim() == pid.to_string();
    // `ps -p` exits 1 when the pid is not listed; anything else with nothing listed is `ps` failing
    match (listed, out.status.code()) {
        (true, _) => Some(true),
        (false, Some(0 | 1)) => Some(false),
        _ => None,
    }
}

/// `text` with control characters shown as `?`, so a damaged or foreign file can't split report lines or drive the
/// terminal.
fn printable(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { '?' } else { c }).collect()
}

fn show_fields(fields: &[(String, Val)]) -> String {
    fields.iter().map(|(k, v)| format!("{}={}", printable(k), v.show())).collect::<Vec<_>>().join(" ")
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

/// `all others critical, import trace, probe off`: the thresholds in effect, with the commonest folded to "all others".
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

/// Parse one of the recorder's files, saying why not when it can't.
fn read_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let bytes = read_capped(path)?;
    match serde_json::from_slice::<T>(&bytes) {
        Ok(v) => Ok(v),
        Err(_) => {
            let schema = serde_json::from_slice::<serde_json::Value>(&bytes).ok().and_then(|v| v.get("schema").and_then(serde_json::Value::as_u64));
            Err(match schema {
                Some(s) if s != u64::from(SCHEMA) => format!("written by another version (format {s}; this one reads {SCHEMA})"),
                _ => "damaged".to_string(),
            })
        }
    }
}

/// Ring entries the report prints per process, and stat rows.
const REPORT_ENTRIES: usize = 15;
const REPORT_STATS: usize = 10;

/// A text report of every process that left files in `dir`, newest first: running, ended cleanly or not, what was
/// open, what ran last, where the time went, and what the recorder itself cost. Files it can't read are named, not
/// skipped.
pub fn report(dir: &Path) -> String {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => return format!("can't read {}: {e}\n", dir.display()),
    };
    let mut runs: Vec<RunId> = Vec::new();
    for e in read.flatten() {
        if let Some((FileKind::Inflight, run)) = parse_name(&e.file_name().to_string_lossy())
            && e.file_name().to_string_lossy().ends_with(".json")
        {
            runs.push(run);
        }
    }
    runs.sort_by_key(|r| std::cmp::Reverse((r.started_ms, r.pid)));
    if runs.is_empty() {
        return format!("no diagnostics files in {}\n", dir.display());
    }
    let mut out = String::new();
    for run in runs {
        let name = FileKind::Inflight.name(run);
        let p = match read_file::<InflightFile>(&dir.join(&name)) {
            Ok(p) => p,
            Err(why) => {
                out.push_str(&format!("process {} (started {} ms): {name} is {why}\n", run.pid, run.started_ms));
                continue;
            }
        };
        let state = match (p.ended, is_running(p.pid)) {
            (Some(Ended::Clean), _) => "ended cleanly",
            (None, Some(true)) => "running (or hung)",
            (None, Some(false)) => "ended without a clean exit (killed, crashed, or hung and stopped)",
            (None, None) => "did not end cleanly; whether it is still running is unknown here",
        };
        out.push_str(&format!(
            "process {}: {state}; started {} ms, last written {} ms ago (clock difference to now)\n",
            p.pid,
            p.started_ms,
            unix_ms().saturating_sub(p.written_ms)
        ));
        if p.schema != SCHEMA {
            out.push_str(&format!("  written in format {}; this report reads {SCHEMA}\n", p.schema));
        }
        out.push_str(&format!("  levels: {}\n", show_levels(&p.levels)));
        if p.write_failures > 0 {
            out.push_str(&format!("  {} diagnostics writes failed\n", p.write_failures));
        }
        if p.panics_lost > 0 {
            out.push_str(&format!("  {} panics could not be recorded (the recorder was busy)\n", p.panics_lost));
        }
        if p.open.is_empty() {
            out.push_str("  nothing was in flight\n");
        }
        for o in &p.open {
            out.push_str(&format!(
                "  IN FLIGHT {}:{} ({}) thread {} open {} at last write  {}\n",
                o.subsystem.as_str(),
                printable(&o.op),
                o.level.as_str(),
                o.thread,
                show_us(o.age_ms.saturating_mul(1000)),
                show_fields(&o.fields)
            ));
        }
        match read_file::<RingFile>(&dir.join(FileKind::Ring.name(run))) {
            Ok(r) => {
                let shown = r.entries.len().min(REPORT_ENTRIES);
                let hidden = r.entries.len().saturating_sub(shown);
                out.push_str("  last entries (oldest first):\n");
                if hidden > 0 || r.evicted > 0 {
                    out.push_str(&format!("    ({hidden} earlier entries not shown; {} pushed out of the full ring)\n", r.evicted));
                }
                for e in r.entries.iter().skip(hidden) {
                    let what = match e.kind {
                        Kind::Span => format!("{} in {}", e.outcome.map_or("?", Outcome::as_str), show_us(e.dur_us.unwrap_or(0))),
                        Kind::Event => "event".to_string(),
                    };
                    out.push_str(&format!("    {}:{} {what}  {}\n", e.subsystem.as_str(), printable(&e.op), show_fields(&e.fields)));
                }
            }
            Err(why) => out.push_str(&format!("  ring file {why}\n")),
        }
        match read_file::<StatsFile>(&dir.join(FileKind::Stats.name(run))) {
            Ok(s) => {
                if !s.stats.is_empty() {
                    out.push_str("  where the time went (leaf spans):\n");
                }
                for r in s.stats.iter().take(REPORT_STATS) {
                    out.push_str(&format!(
                        "    {}:{}  {} calls ({} not ok), total {}, mean {}, min {}, max {}\n",
                        r.subsystem.as_str(),
                        printable(&r.op),
                        r.count,
                        r.failed,
                        show_us(r.total_us),
                        show_us(r.total_us / r.count.max(1)),
                        show_us(if r.min_us == u64::MAX { 0 } else { r.min_us }),
                        show_us(r.max_us)
                    ));
                }
                let more = s.stats.len().saturating_sub(REPORT_STATS);
                if more > 0 {
                    out.push_str(&format!("    ({more} more names not shown)\n"));
                }
                let o = &s.overhead;
                if o.calls > 0 {
                    let share = o.recorder_ns as f64 / (o.uptime_ms.max(1) as f64 * 1e6) * 100.0;
                    out.push_str(&format!(
                        "  the recorder's own cost: {} recorded steps, events and notes took {} on the working threads (about {share:.3}% of one core over {}); rendering and writing {}\n",
                        o.calls,
                        show_us(o.recorder_ns / 1000),
                        show_us(o.uptime_ms.saturating_mul(1000)),
                        show_us(o.writer_ns / 1000)
                    ));
                }
            }
            Err(why) => out.push_str(&format!("  stats file {why}\n")),
        }
    }
    out
}

#[cfg(test)]
mod tests;
