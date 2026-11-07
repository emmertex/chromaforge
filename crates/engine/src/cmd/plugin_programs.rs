//! Programs plug-ins start (`program.start`, see `lcc_plugins::programs`): each run is watched on
//! a thread of its own with a row in the activity stack, and its plug-in's `program.end` hook is
//! called there when the program exits or the user stops waiting (✕).
//!
//! The hook runs off the session's thread, so its catalog requests are relayed to that thread
//! ([`answer_requests`], called by the app every frame). A session nobody answers for (headless
//! MCP or CLI) refuses them at once instead of hanging the hook.
//!
//! The run's folder is lent to the hook for reading and deleted once the hook has returned and
//! the program has exited: after ✕ the thread keeps waiting for the program, which may still use
//! the folder. If the app quits first, the folder stays and a later start sweeps it
//! (`lcc_plugins::programs::sweep_old_runs`).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use lcc_plugins::programs::{EndStatus, check_quick_failure, wait_for_end, wait_for_exit};
use lcc_plugins::{HostApi, Manager, StartRequest};
use serde_json::Value;

use super::plugins::{HostRequest, RelayHost, Relayed, manager_for};
use crate::Session;
use crate::activity::Cancel;

/// How long a start is watched for a program that fails at once (it can't be started).
const QUICK_FAILURE: Duration = Duration::from_millis(300);
/// How long a hook waits for a plug-in that is busy (an export still running its hooks).
const BUSY_WAIT: Duration = Duration::from_secs(600);
/// How long a relayed catalog request waits for the session's thread.
const ANSWER_WAIT: Duration = Duration::from_secs(60);

/// The catalog requests of running `program.end` hooks, for the session's thread.
#[allow(clippy::type_complexity)]
static RELAY: Mutex<Option<(Sender<HostRequest>, Receiver<HostRequest>)>> = Mutex::new(None);
/// Set once something answers relayed requests ([`answer_requests`]).
static ANSWERED: AtomicBool = AtomicBool::new(false);
/// `program.end` hooks running now.
static HOOKS: AtomicUsize = AtomicUsize::new(0);

/// Where plug-in calls on a worker send their catalog requests for [`answer_requests`]
/// (`program.end` hooks, jobs).
pub(super) fn relay_sender() -> Sender<HostRequest> {
    let mut g = RELAY.lock().unwrap_or_else(PoisonError::into_inner);
    g.get_or_insert_with(channel).0.clone()
}

/// Answers the catalog requests of `program.end` hooks and plug-in jobs from `s` (the app, once per frame).
pub fn answer_requests(s: &mut Session) {
    ANSWERED.store(true, Ordering::Relaxed);
    loop {
        let req = {
            let g = RELAY.lock().unwrap_or_else(PoisonError::into_inner);
            match g.as_ref() {
                Some((_, rx)) => rx.try_recv().ok(),
                None => None,
            }
        };
        match req {
            Some(r) => r.answer(s),
            None => break,
        }
    }
}

/// Whether something answers relayed requests in this process ([`answer_requests`] has run).
pub(super) fn answered() -> bool {
    ANSWERED.load(Ordering::Relaxed)
}

/// Whether a `program.end` hook is running (the app then looks for its requests often).
pub fn hooks_running() -> bool {
    HOOKS.load(Ordering::Relaxed) > 0
}

/// What a `program.end` hook sees: catalog requests relayed to the session's thread when the app
/// answers them (refused at once otherwise), and the session's desktop to look programs up.
/// Starting a program from there is refused: a run starts from a command, whose user is there.
struct HookHost(RelayHost);

impl HookHost {
    fn reachable(&self) -> Result<(), String> {
        if ANSWERED.load(Ordering::Relaxed) { Ok(()) } else { Err("the catalog can't be reached from a program's end in this session".into()) }
    }
}

impl HostApi for HookHost {
    fn catalog(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        self.reachable()?;
        self.0.ask_within(Relayed::Catalog(method.to_string()), params, ANSWER_WAIT)
    }

    fn set_metadata(&mut self, params: &Value) -> Result<Value, String> {
        self.reachable()?;
        self.0.ask_within(Relayed::SetMetadata, params, ANSWER_WAIT)
    }

    fn library_add(&mut self, params: &Value) -> Result<Value, String> {
        self.reachable()?;
        self.0.ask_within(Relayed::LibraryAdd, params, ANSWER_WAIT)
    }

    fn photo_pixels(&mut self, photo: &str, long_edge: u32) -> Result<Value, String> {
        self.reachable()?;
        self.0.ask_within(Relayed::Pixels, &super::plugins::pixels_params(photo, long_edge), ANSWER_WAIT)
    }

    fn set_crop(&mut self, photo: &str, rect: [f64; 4], angle: Option<f64>) -> Result<Value, String> {
        self.reachable()?;
        self.0.ask_within(Relayed::SetCrop, &super::plugins::crop_params(photo, rect, angle), ANSWER_WAIT)
    }

    fn find_program(&mut self, program: &lcc_plugins::ProgramDecl) -> Vec<lcc_plugins::Found> {
        self.0.find_program(program)
    }

    fn start_program(&mut self, _: StartRequest) -> Result<(), String> {
        Err("program.start is not available in program.end; start the program from a command".into())
    }
}

/// The host a `program.end` hook gets (tests).
#[cfg(test)]
pub(super) fn hook_host(relay: RelayHost) -> impl HostApi {
    HookHost(relay)
}

/// Starts and watches a run for the session `s` (see the module docs). An error means nothing is
/// running, and dropping `run` has deleted its folder.
pub(super) fn start(s: &mut Session, run: StartRequest) -> Result<(), String> {
    // the watcher thread first: once the program runs, something must own the run
    let (hand_over, watch) = channel::<Box<dyn FnOnce() + Send>>();
    std::thread::Builder::new()
        .name("plugin-program".into())
        .spawn(move || {
            if let Ok(work) = watch.recv() {
                work();
            }
        })
        .map_err(|e| format!("{} was not started: no thread to watch it ({e})", run.program.name))?;
    let not_started = |e: String| format!("{} could not be started: {e}", run.program.name);
    let mut child = s.desktop.start_program(&run.found, &run.args, run.dir.path()).map_err(not_started)?;
    check_quick_failure(child.as_mut(), QUICK_FAILURE).map_err(not_started)?;
    let cancel = Arc::new(AtomicBool::new(false));
    // the names aren't translated, the text around them is; ✕ would throw the run's results away, so the row
    // carries a warning and frontends ask first
    let guard = s.activity.start("pluginProgram", "Waiting for {program} to finish ({plugin})", Cancel::Flag(cancel.clone()));
    guard.name("program", &run.program.name);
    guard.name("plugin", &run.plugin_name);
    guard.detail("The results come back when {program} closes");
    guard.warn_on_stop("What {program} makes from now on won't come back to the library, and {program} keeps running.");
    let library = s.plugin_library.clone();
    let host = HookHost(RelayHost::new(relay_sender(), s.desktop.clone()));
    let work = move || {
        let StartRequest { plugin, program, dir, reservation, .. } = run;
        let (status, code) = wait_for_end(child.as_mut(), &cancel);
        HOOKS.fetch_add(1, Ordering::Relaxed);
        call_end(&library, &plugin, reservation.run(), &program.id, (status, code), dir.path(), host);
        HOOKS.fetch_sub(1, Ordering::Relaxed);
        // the run is over: its row goes, and the plug-in may start the program again
        drop(guard);
        drop(reservation);
        if status == EndStatus::Cancelled {
            wait_for_exit(child.as_mut());
        }
        dir.remove();
    };
    // the thread waits for the run and stops only after taking it, so this can't fail; if it
    // did, the run would end here unwatched (never on this thread: it would hold the UI)
    if hand_over.send(Box::new(work)).is_err() {
        log::warn!("the watcher of a plug-in's program is gone; the run ends unwatched");
    }
    Ok(())
}

/// Calls plug-in `plugin`'s `program.end`, waiting while the plug-in is busy. Failures go to its log.
fn call_end(
    library: &str,
    plugin: &str,
    run: &str,
    program: &str,
    (status, code): (EndStatus, Option<i32>),
    dir: &std::path::Path,
    mut host: HookHost,
) {
    let started = std::time::Instant::now();
    let mut pending = loop {
        let r = manager_for(library).get_or_insert_with(Manager::in_memory).program_end_call(plugin, run, program, status, code, dir);
        match r {
            Ok(p) => break p,
            Err(lcc_plugins::Error::Busy(..)) if started.elapsed() < BUSY_WAIT => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => {
                log::warn!("{plugin}: program.end for {run} could not run: {e}");
                return;
            }
        }
    };
    let outcome = pending.run(&mut host);
    // the manager logs a failed hook in the plug-in's log
    let _ = manager_for(library).get_or_insert_with(Manager::in_memory).end(pending, Some(outcome));
}
