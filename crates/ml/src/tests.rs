use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::*;

fn wait_for(mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < Duration::from_secs(20), "timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn gpu_probe(calls: Arc<AtomicUsize>) -> Probe {
    Box::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok("Test GPU".into())
    })
}

fn queue<T: Send + 'static>() -> Queue<T> {
    Queue::new(Config::default(), gpu_probe(Arc::new(AtomicUsize::new(0))), None)
}

/// Collect results until `n` have come.
fn collect<T: Send + 'static>(q: &Queue<T>, n: usize) -> Vec<Finished<T>> {
    let mut out = Vec::new();
    wait_for(|| {
        out.extend(q.drain());
        out.len() >= n
    });
    out
}

/// A job that holds the worker until `open` is set (or it is cancelled).
fn gate(open: Arc<AtomicBool>, started: Arc<AtomicBool>) -> Job<u32> {
    Job::new("gate", move |ctx: &Ctx| {
        started.store(true, Ordering::SeqCst);
        while !open.load(Ordering::SeqCst) {
            ctx.check()?;
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(0)
    })
}

#[test]
fn jobs_run_off_the_calling_thread() {
    let q: Queue<std::thread::ThreadId> = queue();
    q.submit(Job::new("a", |_: &Ctx| Ok(std::thread::current().id()))).unwrap();
    let r = collect(&q, 1);
    assert_eq!(r[0].key, "a");
    match &r[0].outcome {
        Outcome::Done(id) => assert_ne!(*id, std::thread::current().id()),
        o => panic!("{o:?}"),
    }
    assert_eq!(r[0].device, Some(Kind::Gpu));
    assert!(q.status().threaded);
}

#[test]
fn interactive_jobs_go_before_background_ones() {
    let q: Queue<u32> = queue();
    let (open, started) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    q.submit(gate(open.clone(), started.clone())).unwrap();
    wait_for(|| started.load(Ordering::SeqCst));
    q.submit(Job::new("bg1", |_: &Ctx| Ok(1))).unwrap();
    q.submit(Job::new("bg2", |_: &Ctx| Ok(2))).unwrap();
    q.submit(Job::new("now", |_: &Ctx| Ok(3)).priority(Priority::Interactive)).unwrap();
    open.store(true, Ordering::SeqCst);
    let keys: Vec<String> = collect(&q, 4).into_iter().map(|f| f.key).collect();
    assert_eq!(keys, ["gate", "now", "bg1", "bg2"]);
}

#[test]
fn a_waiting_job_with_the_same_key_is_replaced() {
    let q: Queue<u32> = queue();
    let (open, started) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    q.submit(gate(open.clone(), started.clone())).unwrap();
    wait_for(|| started.load(Ordering::SeqCst));
    q.submit(Job::new("x", |_: &Ctx| Ok(1))).unwrap();
    q.submit(Job::new("x", |_: &Ctx| Ok(2))).unwrap();
    assert_eq!(q.status().pending, 1);
    open.store(true, Ordering::SeqCst);
    let r = collect(&q, 2);
    assert_eq!(r[1].outcome, Outcome::Done(2));
}

#[test]
fn cancelling_a_waiting_job_never_runs_it() {
    let q: Queue<u32> = queue();
    let (open, started) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    q.submit(gate(open.clone(), started.clone())).unwrap();
    wait_for(|| started.load(Ordering::SeqCst));
    let ran = Arc::new(AtomicBool::new(false));
    let r2 = ran.clone();
    q.submit(Job::new("x", move |_: &Ctx| {
        r2.store(true, Ordering::SeqCst);
        Ok(1)
    }))
    .unwrap();
    assert!(q.cancel("x"));
    assert!(!q.cancel("nothing"));
    open.store(true, Ordering::SeqCst);
    let r = collect(&q, 2);
    assert!(r.iter().any(|f| f.key == "x" && f.outcome == Outcome::Cancelled && f.device.is_none()));
    assert!(!ran.load(Ordering::SeqCst));
}

#[test]
fn cancelling_the_running_job_stops_it() {
    let q: Queue<u32> = queue();
    let (open, started) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    q.submit(gate(open, started.clone())).unwrap();
    wait_for(|| started.load(Ordering::SeqCst));
    assert_eq!(q.status().running.as_deref(), Some("gate"));
    assert!(q.cancel("gate"));
    let r = collect(&q, 1);
    assert_eq!(r[0].outcome, Outcome::Cancelled);
    wait_for(|| !q.busy());
    assert_eq!(q.status().cancelled, 1);
}

#[test]
fn cancel_all_ends_what_runs_and_what_waits() {
    let q: Queue<u32> = queue();
    let (open, started) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    q.submit(gate(open, started.clone())).unwrap();
    wait_for(|| started.load(Ordering::SeqCst));
    for i in 0..5 {
        q.submit(Job::new(format!("j{i}"), move |_: &Ctx| Ok(i))).unwrap();
    }
    assert_eq!(q.cancel_all(), 6);
    let r = collect(&q, 6);
    assert!(r.iter().all(|f| f.outcome == Outcome::Cancelled));
    wait_for(|| !q.busy());
    // the queue keeps working afterwards
    q.submit(Job::new("after", |_: &Ctx| Ok(9))).unwrap();
    assert_eq!(collect(&q, 1)[0].outcome, Outcome::Done(9));
}

#[test]
fn a_gpu_failure_reruns_the_job_on_the_cpu_and_stays_there() {
    let calls = Arc::new(AtomicUsize::new(0));
    let q: Queue<Kind> = Queue::new(Config::default(), gpu_probe(calls.clone()), None);
    q.submit(Job::new("a", |ctx: &Ctx| if ctx.device().is_gpu() { Err(JobError::Device("out of memory".into())) } else { Ok(ctx.device().kind) }))
        .unwrap();
    let r = collect(&q, 1);
    assert_eq!(r[0].outcome, Outcome::Done(Kind::Cpu));
    assert_eq!(r[0].device, Some(Kind::Cpu));
    let st = q.status();
    let dev = st.device.unwrap();
    assert_eq!(dev.kind, Kind::Cpu);
    assert!(dev.why_cpu.unwrap().contains("out of memory"));
    // later jobs go straight to the CPU, without asking for the GPU again
    q.submit(Job::new("b", |ctx: &Ctx| Ok(ctx.device().kind))).unwrap();
    assert_eq!(collect(&q, 1)[0].outcome, Outcome::Done(Kind::Cpu));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // until the preference is set again
    q.set_preference(Preference::Auto);
    q.submit(Job::new("c", |ctx: &Ctx| Ok(ctx.device().kind))).unwrap();
    assert_eq!(collect(&q, 1)[0].outcome, Outcome::Done(Kind::Gpu));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn a_device_error_on_the_cpu_is_a_failure() {
    let q: Queue<u32> = Queue::new(Config { preference: Preference::Cpu, ..Config::default() }, gpu_probe(Arc::new(AtomicUsize::new(0))), None);
    q.submit(Job::new("a", |_: &Ctx| Err(JobError::Device("bad".into())))).unwrap();
    assert!(matches!(&collect(&q, 1)[0].outcome, Outcome::Failed(w) if w.contains("bad")));
}

#[test]
fn the_cpu_preference_never_probes_the_gpu() {
    let calls = Arc::new(AtomicUsize::new(0));
    let q: Queue<Kind> = Queue::new(Config { preference: Preference::Cpu, ..Config::default() }, gpu_probe(calls.clone()), None);
    q.submit(Job::new("a", |ctx: &Ctx| Ok(ctx.device().kind))).unwrap();
    assert_eq!(collect(&q, 1)[0].outcome, Outcome::Done(Kind::Cpu));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn the_probe_runs_on_the_worker_not_the_caller() {
    let caller = std::thread::current().id();
    let seen = Arc::new(Mutex::new(None));
    let s2 = seen.clone();
    let probe: Probe = Box::new(move || {
        *s2.lock().unwrap() = Some(std::thread::current().id());
        Err("no GPU here".into())
    });
    let q: Queue<u32> = Queue::new(Config::default(), probe, None);
    q.submit(Job::new("a", |_: &Ctx| Ok(1))).unwrap();
    collect(&q, 1);
    assert_ne!(seen.lock().unwrap().unwrap(), caller);
    assert_eq!(q.status().device.unwrap().why_cpu.as_deref(), Some("no GPU here"));
}

#[test]
fn a_job_that_panics_fails_and_the_worker_goes_on() {
    let q: Queue<u32> = queue();
    q.submit(Job::new("boom", |_: &Ctx| -> Result<u32, JobError> { panic!("model bug") })).unwrap();
    q.submit(Job::new("fine", |_: &Ctx| Ok(1))).unwrap();
    let r = collect(&q, 2);
    assert!(matches!(r[0].outcome, Outcome::Failed(_)));
    assert_eq!(r[1].outcome, Outcome::Done(1));
    assert_eq!(q.status().failed, 1);
}

#[test]
fn a_full_queue_refuses_more() {
    let q: Queue<u32> = Queue::new(Config { max_pending: 2, ..Config::default() }, gpu_probe(Arc::new(AtomicUsize::new(0))), None);
    let (open, started) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    q.submit(gate(open.clone(), started.clone())).unwrap();
    wait_for(|| started.load(Ordering::SeqCst));
    q.submit(Job::new("a", |_: &Ctx| Ok(1))).unwrap();
    q.submit(Job::new("b", |_: &Ctx| Ok(1))).unwrap();
    assert_eq!(q.submit(Job::new("c", |_: &Ctx| Ok(1))), Err(SubmitError::Full(2)));
    // replacing a waiting job is no new one
    assert_eq!(q.submit(Job::new("a", |_: &Ctx| Ok(2))), Ok(()));
    open.store(true, Ordering::SeqCst);
    collect(&q, 3);
}

#[test]
fn unclaimed_results_are_capped() {
    let q: Queue<u32> = Queue::new(Config { max_finished: 3, ..Config::default() }, gpu_probe(Arc::new(AtomicUsize::new(0))), None);
    for i in 0..10 {
        q.submit(Job::new(format!("j{i}"), move |_: &Ctx| Ok(i))).unwrap();
    }
    wait_for(|| q.status().done == 10);
    let r = q.drain();
    assert_eq!(r.len(), 3);
    assert_eq!(r[2].key, "j9");
}

#[test]
fn cancelling_many_waiting_jobs_keeps_the_newest_results() {
    // a ✕ on a large batch: every waiting job ends cancelled, only the newest `max_finished` stay to be taken
    let q: Queue<u32> = Queue::new(Config { max_finished: 5, ..Config::default() }, gpu_probe(Arc::new(AtomicUsize::new(0))), None);
    let (open, started) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let (o2, s2) = (open.clone(), started.clone());
    q.submit(Job::new("long", move |ctx: &Ctx| {
        s2.store(true, Ordering::SeqCst);
        while !o2.load(Ordering::SeqCst) && !ctx.cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(0)
    }))
    .unwrap();
    wait_for(|| started.load(Ordering::SeqCst));
    for i in 0..20_000u32 {
        q.submit(Job::new(format!("embed:{i}"), move |_: &Ctx| Ok(i))).unwrap();
    }
    assert_eq!(q.cancel_all(), 20_001);
    open.store(true, Ordering::SeqCst);
    wait_for(|| q.status().running.is_none() && q.status().pending == 0);
    let st = q.status();
    assert_eq!(st.cancelled, 20_000);
    let r = q.drain();
    assert!(r.len() <= 5, "{} results kept", r.len());
    assert!(r.iter().any(|f| f.key == "embed:19999" || f.key == "long"), "{:?}", r.iter().map(|f| &f.key).collect::<Vec<_>>());
}

#[test]
fn dropping_the_queue_stops_the_running_job() {
    let q: Queue<u32> = queue();
    let (open, started) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let stopped = Arc::new(AtomicBool::new(false));
    let (s2, st2) = (stopped.clone(), started.clone());
    q.submit(Job::new("long", move |ctx: &Ctx| {
        st2.store(true, Ordering::SeqCst);
        while !open.load(Ordering::SeqCst) {
            if ctx.cancelled() {
                s2.store(true, Ordering::SeqCst);
                return Err(JobError::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(0)
    }))
    .unwrap();
    wait_for(|| started.load(Ordering::SeqCst));
    drop(q);
    wait_for(|| stopped.load(Ordering::SeqCst));
}

// ---------------------------------------------------------------------------------------------------------------------
// the batch row

#[derive(Default)]
struct Log {
    events: Mutex<Vec<String>>,
    flags: Mutex<Vec<Arc<AtomicBool>>>,
}

impl Log {
    fn push(&self, e: String) {
        self.events.lock().unwrap().push(e);
    }
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

struct TestRows(Arc<Log>);
struct TestRow(Arc<Log>);

impl Rows for TestRows {
    fn begin(&self, cancel: Arc<AtomicBool>) -> Box<dyn Row> {
        self.0.push("begin".into());
        self.0.flags.lock().unwrap().push(cancel);
        Box::new(TestRow(self.0.clone()))
    }
}

impl Row for TestRow {
    fn progress(&self, done: u64, total: u64) {
        self.0.push(format!("progress {done}/{total}"));
    }
    fn detail(&self, name: &str) {
        self.0.push(format!("detail {name}"));
    }
    fn item_done(&self) {
        self.0.push("done".into());
    }
    fn item_failed(&self, why: &str) {
        self.0.push(format!("failed {why}"));
    }
}

impl Drop for TestRow {
    fn drop(&mut self) {
        self.0.push("end".into());
    }
}

fn queue_with_rows() -> (Queue<u32>, Arc<Log>) {
    let log = Arc::new(Log::default());
    let q = Queue::new(Config::default(), gpu_probe(Arc::new(AtomicUsize::new(0))), Some(Arc::new(TestRows(log.clone())) as Arc<dyn Rows>));
    (q, log)
}

#[test]
fn a_batch_has_one_row_while_busy_and_none_when_idle() {
    let (q, log) = queue_with_rows();
    let (open, started) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    q.submit(gate(open.clone(), started.clone())).unwrap();
    wait_for(|| started.load(Ordering::SeqCst));
    q.submit(Job::new("a", |_: &Ctx| Ok(1)).name("IMG_1.CR3")).unwrap();
    q.submit(Job::new("b", |_: &Ctx| Err(JobError::Failed("no model".into()))).name("IMG_2.CR3")).unwrap();
    open.store(true, Ordering::SeqCst);
    collect(&q, 3);
    wait_for(|| log.events().last().map(String::as_str) == Some("end"));
    let ev = log.events();
    assert_eq!(ev.iter().filter(|e| *e == "begin").count(), 1, "{ev:?}");
    assert!(ev.contains(&"detail IMG_1.CR3".to_string()), "{ev:?}");
    assert!(ev.contains(&"failed IMG_2.CR3: no model".to_string()), "{ev:?}");
    assert_eq!(ev.iter().filter(|e| *e == "done").count(), 2, "{ev:?}");
    assert!(ev.contains(&"progress 3/3".to_string()), "{ev:?}");
    // a later job starts a new batch
    q.submit(Job::new("c", |_: &Ctx| Ok(1))).unwrap();
    collect(&q, 1);
    wait_for(|| log.events().iter().filter(|e| *e == "end").count() == 2);
    assert_eq!(log.events().iter().filter(|e| *e == "begin").count(), 2);
}

#[test]
fn the_rows_cancel_flag_cancels_the_batch() {
    let (q, log) = queue_with_rows();
    let (open, started) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    q.submit(gate(open, started.clone())).unwrap();
    wait_for(|| started.load(Ordering::SeqCst));
    for i in 0..3 {
        q.submit(Job::new(format!("j{i}"), move |_: &Ctx| Ok(i))).unwrap();
    }
    // the row's ✕
    let flag = log.flags.lock().unwrap()[0].clone();
    flag.store(true, Ordering::SeqCst);
    let r = collect(&q, 4);
    assert!(r.iter().all(|f| f.outcome == Outcome::Cancelled), "{r:?}");
    wait_for(|| !q.busy() && log.events().last().map(String::as_str) == Some("end"));
    // the next batch has a fresh flag and runs
    q.submit(Job::new("next", |_: &Ctx| Ok(5))).unwrap();
    assert_eq!(collect(&q, 1)[0].outcome, Outcome::Done(5));
    let flags = log.flags.lock().unwrap();
    assert_eq!(flags.len(), 2);
    assert!(!flags[1].load(Ordering::SeqCst));
}
