use super::*;

fn temp(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("lcc-diag-test-{}-{tag}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
    serde_json::from_slice(&std::fs::read(dir.join(FileKind::Inflight.name(d.run))).unwrap()).unwrap()
}

fn ring(dir: &Path, d: &Diag) -> RingFile {
    serde_json::from_slice(&std::fs::read(dir.join(FileKind::Ring.name(d.run))).unwrap()).unwrap()
}

/// A recorder writing into a fresh folder, as if its writer thread ran (changes below critical wait for a tick).
fn on_disk(tag: &str) -> (PathBuf, Diag) {
    let dir = temp(tag);
    let d = Diag::new(Some(dir.clone()), 8);
    d.writer.store(true, Ordering::Relaxed);
    (dir, d)
}

#[test]
fn a_text_is_cut_on_a_character_boundary() {
    let long = "é".repeat(TEXT_MAX_CHARS * 3);
    assert_eq!(Val::text(&long), Val::Text("é".repeat(TEXT_MAX_CHARS)));
    let exact = "a".repeat(TEXT_MAX_CHARS);
    assert_eq!(Val::text(&exact), Val::Text(exact.clone()), "exactly the cap is kept whole");
    assert_eq!(Val::text(&format!("{exact}b")), Val::Text(exact), "one over is cut");
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
        assert_eq!(Subsystem::parse(&s.as_str().to_ascii_uppercase()), Some(*s), "any case");
    }
    let mut names: Vec<_> = Subsystem::ALL.iter().map(|s| s.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), Subsystem::COUNT, "names are distinct");
    for l in Level::ALL {
        assert_eq!(Level::parse(l.as_str()), Some(l));
        assert_eq!(level_of(l as u8), Some(l));
    }
    assert_eq!(Level::parse("Normal"), Some(Level::Normal));
    assert_eq!(level_of(0), None);
    assert_eq!(Subsystem::parse(""), None);
    assert_eq!(Level::parse(""), None);
}

#[test]
fn a_call_is_recorded_at_the_threshold_and_below_but_not_above() {
    let d = Diag::new(None, 8);
    for t in [None, Some(Level::Critical), Some(Level::Normal), Some(Level::Detail), Some(Level::Trace)] {
        d.set_level(Some(Subsystem::App), t);
        for l in Level::ALL {
            let expected = t.is_some_and(|t| l <= t);
            assert_eq!(d.wants(Subsystem::App, l), expected, "threshold {t:?}, call {l:?}");
        }
    }
}

#[test]
fn config_entries_set_the_default_and_overrides_in_any_order() {
    let a = cfg("critical,import=trace");
    assert_eq!(a, cfg("import=trace,critical"));
    assert_eq!(a.level_of(Subsystem::Import), Some(Level::Trace));
    assert_eq!(a.level_of(Subsystem::App), Some(Level::Critical));
    let c = cfg("  normal , probe=off ,, probe=detail ");
    assert_eq!(c.level_of(Subsystem::Probe), Some(Level::Detail), "the last entry for a subsystem wins");
    assert_eq!(c.level_of(Subsystem::App), Some(Level::Normal));
    for on in ["1", "on", "true", "yes", "ON", "True"] {
        assert_eq!(cfg(on), Config::default(), "{on} means normal");
    }
    assert_eq!(cfg("TRACE").level_of(Subsystem::App), Some(Level::Trace), "levels in any case");
}

#[test]
fn every_off_word_in_any_case_turns_a_subsystem_off_and_a_list_of_them_records_nothing() {
    for off in ["0", "off", "false", "no", "OFF", "Off", "FALSE", "No"] {
        assert!(!cfg(off).records_anything(), "{off}");
        assert_eq!(cfg(&format!("normal,probe={off}")).level_of(Subsystem::Probe), None, "probe={off}");
    }
    assert!(!cfg("off,off").records_anything());
    assert!(!cfg("off,probe=0").records_anything());
    assert!(cfg("off,probe=trace").records_anything(), "one subsystem on is enough");
}

#[test]
fn config_entries_that_are_not_understood_are_reported_and_the_rest_still_applies() {
    let (c, issues) = Config::parse("loud,import=trace,imprt=trace,app=2,commit=");
    assert_eq!(c.level_of(Subsystem::Import), Some(Level::Trace));
    assert_eq!(c.level_of(Subsystem::App), Some(Level::Normal), "a bad value leaves the subsystem as it was");
    assert_eq!(
        issues,
        vec![
            ConfigIssue { entry: "loud".into(), problem: ConfigProblem::UnknownLevel },
            ConfigIssue { entry: "imprt=trace".into(), problem: ConfigProblem::UnknownSubsystem },
            ConfigIssue { entry: "app=2".into(), problem: ConfigProblem::UnknownLevel },
            ConfigIssue { entry: "commit=".into(), problem: ConfigProblem::UnknownLevel },
        ]
    );
}

#[test]
fn the_start_plan_is_off_without_a_value_or_a_folder() {
    let dir = Some(PathBuf::from("/settings/diag"));
    let off = |v: Option<&str>| matches!(plan_start(v, None, dir.clone()), Err(StartStatus::Off));
    assert!(off(None), "unset");
    assert!(off(Some("")) && off(Some("  ")), "empty");
    assert!(off(Some("false")) && off(Some("OFF")) && off(Some("off,off")), "records nothing");
    let Ok((_, issues, at)) = plan_start(Some("normal,bogus"), None, dir.clone()) else { panic!("starts") };
    assert_eq!(at, PathBuf::from("/settings/diag"));
    assert_eq!(issues.len(), 1);
    let Ok((_, _, at)) = plan_start(Some("1"), Some(std::ffi::OsStr::new("/elsewhere")), dir.clone()) else { panic!("override") };
    assert_eq!(at, PathBuf::from("/elsewhere"), "<PREFIX>_DIAG_DIR wins");
    assert_eq!(
        plan_start(Some("1"), Some(std::ffi::OsStr::new("")), dir).err(),
        Some(StartStatus::NoFolder),
        "an empty DIAG_DIR is no folder, not the current directory"
    );
    assert_eq!(plan_start(Some("1"), None, None).err(), Some(StartStatus::NoFolder), "a run that keeps no settings");
}

#[test]
fn only_the_recorders_own_file_names_are_recognised() {
    let run = RunId { pid: 42, started_ms: 1700 };
    for kind in FileKind::ALL {
        let name = kind.name(run);
        assert_eq!(parse_name(&name), Some((kind, run)), "{name}");
        assert_eq!(parse_name(&format!("{name}.tmp")), Some((kind, run)));
    }
    for foreign in [
        "stats-2024.csv",
        "ring-1.png",
        "inflight-12.json",
        "ring-1-2.json.bak",
        "stats-1-x.json",
        "inflight--1.json",
        "inflight-1-.json",
        "notes.txt",
        "rings-1-2.json",
    ] {
        assert_eq!(parse_name(foreign), None, "{foreign}");
    }
}

#[test]
fn a_below_threshold_call_leaves_no_trace_and_is_not_counted() {
    let d = Diag::with_config(None, 8, &cfg("off"));
    let mut s = d.span(Subsystem::Commit, "x", Class::Top, &[("k", Val::U64(1))]);
    s.set("a", Val::U64(1));
    s.note("n", Val::U64(1));
    s.finish(Outcome::Ok);
    d.event_at(Level::Normal, Subsystem::App, "e", &[]);
    d.span(Subsystem::Probe, "leaf", Class::Leaf, &[]).finish(Outcome::Ok);
    let snap = d.snapshot(10);
    assert!(snap.ring.is_empty() && snap.open.is_empty() && snap.stats.is_empty());
    assert_eq!(snap.overhead.calls, 0);
    assert_eq!(d.seq.load(Ordering::Relaxed), 1, "not even a sequence number was taken");
}

#[test]
fn import_at_trace_and_everything_else_at_critical_records_only_what_each_admits() {
    let d = Diag::with_config(None, 64, &cfg("critical,import=trace"));
    d.span(Subsystem::Import, "item", Class::Leaf, &[]).finish(Outcome::Ok);
    top(&d, "commit_stage").finish(Outcome::Ok);
    d.event_at(Level::Normal, Subsystem::App, "normal_event", &[]);
    d.event_at(Level::Critical, Subsystem::App, "freeze", &[]);
    d.event_at(Level::Trace, Subsystem::Import, "detail_event", &[]);
    let snap = d.snapshot(100);
    assert_eq!(snap.stats.iter().map(|r| r.op.as_str()).collect::<Vec<_>>(), ["item"]);
    let ops: Vec<_> = snap.ring.iter().map(|e| e.op.as_str()).collect();
    assert_eq!(ops, ["freeze", "detail_event"]);
}

#[test]
fn the_ring_never_holds_more_than_its_cap_and_keeps_a_failure() {
    let d = Diag::new(None, 8);
    top(&d, "bad").finish(Outcome::Failed);
    for _ in 0..50 {
        top(&d, "routine").finish(Outcome::Ok);
    }
    let snap = d.snapshot(1000);
    assert_eq!(snap.ring.len(), 8);
    assert!(snap.ring.iter().any(|e| e.op == "bad"), "the failure outlived 50 routine successes");
    assert_eq!(snap.evicted, 43, "every push past the cap is counted");
}

#[test]
fn at_the_cap_the_oldest_goes_when_nothing_is_routine() {
    let d = Diag::new(None, 8);
    for i in 0..9u64 {
        d.event_at(Level::Normal, Subsystem::App, "e", &[("i", Val::U64(i))]);
    }
    let ring = d.snapshot(100).ring;
    assert_eq!(ring.iter().map(|e| e.fields[0].1.clone()).collect::<Vec<_>>(), (1..9).map(Val::U64).collect::<Vec<_>>());
}

#[test]
fn a_newcomer_only_pushes_out_entries_at_least_as_verbose_as_itself() {
    let d = Diag::new(None, 4);
    d.event_at(Level::Critical, Subsystem::App, "critical", &[]);
    d.event_at(Level::Trace, Subsystem::App, "trace", &[]);
    d.event_at(Level::Normal, Subsystem::App, "normal1", &[]);
    d.event_at(Level::Normal, Subsystem::App, "normal2", &[]);
    d.event_at(Level::Normal, Subsystem::App, "normal3", &[]);
    let ops = |d: &Diag| d.snapshot(100).ring.iter().map(|e| e.op.clone()).collect::<Vec<_>>();
    assert_eq!(ops(&d), ["critical", "normal1", "normal2", "normal3"], "the trace entry went first, whatever its age");
    d.event_at(Level::Normal, Subsystem::App, "normal4", &[]);
    assert_eq!(ops(&d), ["critical", "normal2", "normal3", "normal4"], "then the oldest normal one, never the critical one");
    let full = Diag::new(None, 2);
    full.event_at(Level::Critical, Subsystem::App, "c1", &[]);
    full.event_at(Level::Critical, Subsystem::App, "c2", &[]);
    full.event_at(Level::Trace, Subsystem::App, "t", &[]);
    assert_eq!(ops(&full), ["c1", "c2"], "a ring of critical entries keeps them and drops the verbose newcomer");
    assert_eq!(full.snapshot(0).evicted, 1);
}

#[test]
fn leaf_spans_are_counted_and_timed_with_their_failures() {
    let d = Diag::new(None, 8);
    for outcome in [Outcome::Ok, Outcome::Failed, Outcome::Ok] {
        d.span(Subsystem::Commit, "apply_photo", Class::Leaf, &[]).finish(outcome);
    }
    drop(d.span(Subsystem::Commit, "apply_photo", Class::Leaf, &[]));
    let snap = d.snapshot(10);
    assert!(snap.ring.is_empty() && snap.open.is_empty());
    let row = &snap.stats[0];
    assert_eq!((row.op.as_str(), row.count, row.failed), ("apply_photo", 4, 2), "a failed and an abandoned one");
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
fn many_short_lived_threads_share_one_cell_per_name() {
    let d = Diag::new(None, 8);
    for _ in 0..20 {
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..50 {
                        d.span(Subsystem::Probe, "probe_file", Class::Leaf, &[]).finish(Outcome::Ok);
                    }
                });
            }
        });
    }
    assert_eq!(d.snapshot(0).stats[0].count, 8000, "none lost");
    assert_eq!(d.stats.lock().unwrap().len(), 1, "160 threads, one cell");
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
fn done_maps_a_result_to_an_outcome_and_set_fields_reach_the_entry() {
    let d = Diag::new(None, 8);
    let mut s = top(&d, "x");
    s.note("done", Val::U64(3));
    s.set("done", Val::U64(7));
    s.done(&Err::<(), ()>(()));
    let e = d.snapshot(1).ring.remove(0);
    assert_eq!(e.outcome, Some(Outcome::Failed));
    assert_eq!(e.fields, vec![("done".to_string(), Val::U64(7))], "the final value set at the end wins over the last note");
    assert_eq!(e.level, Level::Normal);
}

#[test]
fn traced_ends_with_the_closures_outcome_on_every_path() {
    let d = Diag::new(None, 8);
    let classify = |e: &&str| if *e == "cancel" { Outcome::Cancelled } else { Outcome::Failed };
    let run = |r: Result<u8, &'static str>| end_traced(d.span(Subsystem::Commit, "step", Class::Top, &[]), classify, |_| r);
    assert_eq!(run(Ok(1)), Ok(1));
    assert_eq!(run(Err("cancel")), Err("cancel"));
    assert_eq!(run(Err("broken")), Err("broken"));
    let early = || -> Result<u8, &'static str> {
        end_traced(d.span(Subsystem::Commit, "step", Class::Top, &[]), classify, |_| {
            let v: Result<u8, &str> = Err("broken");
            let v = v?; // an early return inside still ends the span with its outcome
            Ok(v)
        })
    };
    assert!(early().is_err());
    let outcomes: Vec<_> = d.snapshot(10).ring.iter().map(|e| e.outcome).collect();
    assert_eq!(outcomes, [Some(Outcome::Ok), Some(Outcome::Cancelled), Some(Outcome::Failed), Some(Outcome::Failed)]);
}

#[test]
fn an_item_timer_ends_ok_on_drop() {
    let d = Diag::new(None, 8);
    for i in 0..3 {
        let _t = ItemTimer(d.span(Subsystem::Commit, "apply_photo", Class::Leaf, &[]));
        if i == 1 {
            continue;
        }
    }
    assert_eq!((d.snapshot(0).stats[0].count, d.snapshot(0).stats[0].failed), (3, 0));
}

#[test]
fn a_critical_top_span_and_a_critical_event_are_on_disk_at_once_and_a_normal_one_after_a_tick() {
    let (dir, d) = on_disk("critical");
    let critical = d.span_at(Level::Critical, Subsystem::Commit, "commit", Class::Top, &[("photos", Val::U64(5))]);
    let file = inflight(&dir, &d);
    assert_eq!((file.open.len(), file.open[0].op.as_str(), file.open[0].fields[0].1.clone()), (1, "commit", Val::U64(5)));
    let normal = top(&d, "stage");
    assert_eq!(inflight(&dir, &d).open.len(), 1, "the normal span is not in the file yet");
    d.tick();
    assert_eq!(inflight(&dir, &d).open.len(), 2);
    normal.finish(Outcome::Ok);
    critical.finish(Outcome::Ok);
    assert!(inflight(&dir, &d).open.is_empty(), "a critical end writes everything, the normal span's end included");
    d.event_at(Level::Critical, Subsystem::App, "alarm", &[]);
    assert_eq!(ring(&dir, &d).entries.last().map(|e| e.op.clone()), Some("alarm".to_string()), "a critical event, without a tick");
    d.event_at(Level::Normal, Subsystem::App, "quiet", &[]);
    assert_ne!(ring(&dir, &d).entries.last().map(|e| e.op.clone()), Some("quiet".to_string()), "a normal event waits for the tick");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_older_render_never_lands_over_a_newer_one() {
    let (dir, d) = on_disk("race");
    let open = top(&d, "first");
    // what a writer-thread tick would have rendered, before a critical write it then loses the race to
    let stale = {
        let mut st = d.lock();
        d.render(&mut st)
    };
    open.finish(Outcome::Ok);
    d.event_at(Level::Critical, Subsystem::App, "newer", &[]);
    d.put(stale);
    let file = inflight(&dir, &d);
    assert!(file.open.is_empty(), "the stale render (with `first` still open) was dropped");
    assert_eq!(ring(&dir, &d).entries.last().map(|e| e.op.clone()), Some("newer".to_string()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_file_planted_at_the_temp_name_is_replaced_not_written_through() {
    let dir = temp("planted");
    let name = "ring-1-2.json";
    let target = dir.join("victim.txt");
    std::fs::write(&target, b"keep me").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, dir.join(format!("{name}.tmp"))).unwrap();
    #[cfg(not(unix))]
    std::fs::write(dir.join(format!("{name}.tmp")), b"left over").unwrap();
    write_atomic(&dir, name, b"{}").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"keep me");
    assert_eq!(std::fs::read(dir.join(name)).unwrap(), b"{}");
    assert!(!dir.join(format!("{name}.tmp")).exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn finish_marks_a_clean_end_that_a_later_tick_keeps() {
    let (dir, d) = on_disk("finish");
    top(&d, "work").finish(Outcome::Ok);
    d.finish();
    assert_eq!(inflight(&dir, &d).ended, Some(Ended::Clean));
    d.event_at(Level::Normal, Subsystem::App, "after", &[]);
    d.tick();
    assert_eq!(inflight(&dir, &d).ended, Some(Ended::Clean));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_tick_writes_only_when_something_changed() {
    let (dir, d) = on_disk("tick");
    let path = dir.join(FileKind::Ring.name(d.run));
    d.tick();
    assert!(!path.exists(), "nothing happened yet");
    d.event_at(Level::Normal, Subsystem::App, "e", &[]);
    d.tick();
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
    let mut d = Diag::new(Some(dir.clone()), 8);
    d.flush_every_ms = 1_000_000;
    d.event_at(Level::Normal, Subsystem::App, "first", &[]);
    let path = dir.join(FileKind::Ring.name(d.run));
    assert!(path.exists(), "the first change is due at once");
    let size = std::fs::metadata(&path).unwrap().len();
    d.event_at(Level::Normal, Subsystem::App, "second", &[]);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), size, "inside the interval the file is not rewritten");
    d.flush();
    assert!(std::fs::metadata(&path).unwrap().len() > size);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_note_reaches_the_in_flight_file_and_one_handle_passes_one_per_fifty_ms() {
    let (dir, d) = on_disk("note");
    let s = top(&d, "apply");
    s.note("done", Val::U64(1));
    s.note("done", Val::U64(2)); // inside the window: dropped
    d.tick();
    assert_eq!(inflight(&dir, &d).open[0].fields, vec![("done".to_string(), Val::U64(1))]);
    std::thread::sleep(Duration::from_millis(60));
    s.note("done", Val::U64(3)); // the window has passed
    d.tick();
    assert_eq!(inflight(&dir, &d).open[0].fields, vec![("done".to_string(), Val::U64(3))]);
    drop(s);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_failed_write_is_counted_and_never_fails_the_work() {
    let dir = temp("blocked");
    let d = Diag { dir: Some(dir.join("gone")), ..Diag::new(None, 8) };
    top(&d, "y").finish(Outcome::Ok);
    d.flush();
    assert_eq!(d.snapshot(1).ring.len(), 1);
    assert!(d.snapshot(1).write_failures > 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_panic_while_the_recorder_is_busy_is_counted_not_waited_for() {
    let (dir, d) = on_disk("panic");
    {
        let _busy = d.lock();
        d.panic_event("lib.rs:1:1");
    }
    assert_eq!(d.snapshot(0).panics_lost, 1);
    d.panic_event("lib.rs:2:5");
    let entries = ring(&dir, &d).entries;
    assert_eq!(entries.last().map(|e| (e.op.clone(), e.fields.clone())), Some(("panic".into(), vec![("location".into(), Val::text("lib.rs:2:5"))])));
    assert_eq!(inflight(&dir, &d).panics_lost, 1, "the lost one is in the file");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_panic_location_keeps_the_file_name_not_its_path() {
    assert_eq!(
        panic_location("/Users/someone/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/zune-jpeg-0.4.14/src/decoder.rs", 812, 17),
        "decoder.rs:812:17"
    );
    assert_eq!(panic_location(r"C:\Users\someone\src\lib.rs", 3, 1), "lib.rs:3:1");
    assert_eq!(panic_location("crates/diag/src/lib.rs", 9, 2), "lib.rs:9:2");
    assert_eq!(panic_location("lib.rs", 1, 1), "lib.rs:1:1");
}

#[test]
fn processes_that_ended_cleanly_are_pruned_oldest_first_and_unclean_ones_are_kept() {
    let dir = temp("prune");
    let write = |run: RunId, ended: Option<Ended>| {
        let f = InflightFile {
            schema: SCHEMA,
            pid: run.pid,
            started_ms: run.started_ms,
            written_ms: run.started_ms,
            write_failures: 0,
            panics_lost: 0,
            ended,
            levels: Vec::new(),
            open: Vec::new(),
        };
        std::fs::write(dir.join(FileKind::Inflight.name(run)), serde_json::to_vec(&f).unwrap()).unwrap();
        std::fs::write(dir.join(FileKind::Ring.name(run)), b"{}").unwrap();
    };
    let hung = RunId { pid: 7, started_ms: 1 };
    write(hung, None);
    for i in 0..(KEEP_CLEAN as u64 + 3) {
        write(RunId { pid: 100 + i as u32, started_ms: 1000 + i }, Some(Ended::Clean));
    }
    for foreign in ["stats-2023.csv", "ring-1.png", "notes.txt"] {
        std::fs::write(dir.join(foreign), b"x").unwrap();
    }
    let own = RunId { pid: 1, started_ms: 0 };
    std::fs::write(dir.join(FileKind::Stats.name(own)), b"{}").unwrap();
    prune(&dir, own);
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    let has = |n: &str| names.iter().any(|x| x == n);
    assert!(has(&FileKind::Inflight.name(hung)) && has(&FileKind::Ring.name(hung)), "the oldest, unclean process is kept: {names:?}");
    for i in 0..3u64 {
        assert!(!has(&FileKind::Inflight.name(RunId { pid: 100 + i as u32, started_ms: 1000 + i })), "the oldest clean ones go");
    }
    for i in 3..(KEEP_CLEAN as u64 + 3) {
        assert!(has(&FileKind::Inflight.name(RunId { pid: 100 + i as u32, started_ms: 1000 + i })), "the newest {KEEP_CLEAN} clean ones stay");
    }
    assert!(has("stats-2023.csv") && has("ring-1.png") && has("notes.txt"), "files that are not ours are never touched");
    assert!(has(&FileKind::Stats.name(own)), "our own are never touched");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_report_names_what_was_in_flight_the_levels_and_the_recorders_cost() {
    let dir = temp("report");
    let d = Diag::with_config(Some(dir.clone()), 8, &cfg("normal,import=trace,probe=off"));
    let _open = d.span(Subsystem::Commit, "lightroom_commit", Class::Top, &[("stage", Val::text("apply"))]);
    d.span(Subsystem::Import, "apply_photo", Class::Leaf, &[]).finish(Outcome::Ok);
    d.event_at(Level::Normal, Subsystem::App, "slow", &[("ms", Val::U64(105_936))]);
    d.flush();
    let gone = RunId { pid: 4_000_000, started_ms: 1 };
    let f = InflightFile {
        schema: SCHEMA,
        pid: gone.pid,
        started_ms: 1,
        written_ms: 2,
        write_failures: 0,
        panics_lost: 2,
        ended: None,
        levels: Vec::new(),
        open: vec![OpenView {
            seq: 1,
            ts_ms: 1,
            age_ms: 5000,
            subsystem: Subsystem::Probe,
            level: Level::Normal,
            op: "probe_all".into(),
            thread: 1,
            fields: vec![("done".into(), Val::U64(10))],
        }],
    };
    std::fs::write(dir.join(FileKind::Inflight.name(gone)), serde_json::to_vec(&f).unwrap()).unwrap();
    let text = report(&dir);
    assert!(text.contains("IN FLIGHT commit:lightroom_commit") && text.contains("stage=apply"), "{text}");
    assert!(text.contains("slow") && text.contains("ms=105936") && text.contains("apply_photo"), "{text}");
    assert!(text.contains("IN FLIGHT probe:probe_all") && text.contains("done=10") && text.contains("2 panics"), "{text}");
    assert!(text.contains("levels: all others normal, import trace, probe off"), "{text}");
    assert!(text.contains("the recorder's own cost:"), "{text}");
    // whether a process is running can only be asked where `ps` exists
    if cfg!(unix) {
        assert!(text.contains(&format!("process {}: running", d.run.pid)), "{text}");
    } else {
        lcc_testkit::skip!(System, "is_running needs `ps` (unix only)");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn files_it_cannot_read_are_named_in_the_report_not_skipped() {
    let dir = temp("unreadable");
    std::fs::write(dir.join("inflight-77-1.json"), b"{not json").unwrap();
    std::fs::write(dir.join("inflight-78-2.json"), br#"{"schema":9,"pid":78}"#).unwrap();
    let text = report(&dir);
    assert!(text.contains("process 77") && text.contains("damaged"), "{text}");
    assert!(text.contains("process 78") && text.contains("another version (format 9"), "{text}");
    assert!(report(&temp("none")).starts_with("no diagnostics files"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn hostile_files_in_the_directory_never_panic_the_report_or_the_pruning() {
    let dir = temp("hostile");
    let max = u64::MAX;
    let cases: Vec<(String, Vec<u8>)> = vec![
        ("inflight-1-1.json".into(), Vec::new()),
        ("inflight-2-1.json".into(), b"null".to_vec()),
        ("inflight-3-1.json".into(), br#"{"schema":1,"pid":3,"started_ms":1,"written_ms":2,"write_failures":0,"open":[{"seq":1}]}"#.to_vec()),
        (
            "inflight-4-1.json".into(),
            format!(r#"{{"schema":1,"pid":4,"started_ms":{max},"written_ms":{max},"write_failures":{max},"panics_lost":{max},"ended":null,"levels":[],"open":[{{"seq":1,"ts_ms":1,"age_ms":{max},"subsystem":"commit","level":"critical","op":"x\u001b[2J","thread":1,"fields":[["k\n",{{"t":"len","v":{max}}}]]}}]}}"#).into_bytes(),
        ),
        ("ring-4-1.json".into(), b"\xff\xfe not utf-8".to_vec()),
        (
            "stats-4-1.json".into(),
            format!(r#"{{"schema":1,"pid":4,"written_ms":0,"overhead":{{"calls":1,"recorder_ns":{max},"writer_ns":0,"uptime_ms":0}},"stats":[{{"subsystem":"probe","op":"x","count":0,"failed":0,"total_us":{max},"min_us":{max},"max_us":0}}]}}"#).into_bytes(),
        ),
        ("inflight-5-1.json".into(), br#"{"schema":1,"pid":5,"open":[{"subsystem":"no_such"}]}"#.to_vec()),
        ("inflight-99999999999999999999-1.json".into(), b"{}".to_vec()),
        ("inflight-x-1.json".into(), b"{}".to_vec()),
    ];
    for (name, bytes) in &cases {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    let text = report(&dir);
    assert!(text.contains("process 4:"), "the one well-formed file is still reported: {text}");
    assert!(!text.contains('\u{1b}') && !text.contains("k\n"), "control characters are not printed: {text}");
    prune(&dir, RunId { pid: 0, started_ms: 0 });
    let _ = std::fs::remove_dir_all(dir);
}
