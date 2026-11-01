//! The desktop hand-offs: showing a file in the system file manager and starting another
//! program (an external editor, the file's default application, or a program a plug-in may
//! start: [`Desktop::find_program`], [`Desktop::start_program`]).
//!
//! They go through [`Desktop`] so that headless sessions and tests never start desktop programs:
//! a session in a unit test, and every headless UI harness, records the request with
//! [`Recorder`] instead; the real app uses [`System`].

use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::sync::{Mutex, PoisonError};

#[cfg(not(target_arch = "wasm32"))]
use lcc_plugins::{Exit, Found, ProgramDecl, Running};

/// What the session asked the desktop to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Show this file in the system file manager.
    Reveal(PathBuf),
    /// Start this command line and return at once.
    Launch(Vec<String>),
    /// Start this command line and wait for it to exit.
    RunAndWait(Vec<String>),
    /// Start a plug-in's program (where it was found) with these arguments and temp folder.
    StartProgram { path: String, args: Vec<String>, tmp: PathBuf },
}

/// The hand-offs a session makes to the desktop.
pub trait Desktop: Send + Sync {
    /// Show `file` in the system file manager.
    fn reveal(&self, file: &Path) -> Result<(), String>;
    /// Start `line` (program, then arguments) without waiting for it.
    fn launch(&self, line: &[String]) -> Result<(), String>;
    /// Start `line` and wait for it. → whether it exited successfully
    fn run_and_wait(&self, line: &[String]) -> Result<bool, String>;
    /// Where a plug-in's declared program is installed on this system, in the entry's order
    /// (macOS: one per installed bundle id). Read-only. Looking can take a moment (a second or two
    /// at most); the UI thread uses [`Desktop::find_program_soon`]. (Native builds: the web build
    /// has no plug-in programs.)
    #[cfg(not(target_arch = "wasm32"))]
    fn find_program(&self, program: &ProgramDecl) -> Vec<Found>;
    /// [`Desktop::find_program`] without waiting: the last answer (`None`: none yet), while a
    /// fresh one is looked up in the background when it's old.
    #[cfg(not(target_arch = "wasm32"))]
    fn find_program_soon(&self, program: &ProgramDecl) -> Option<Vec<Found>> {
        Some(self.find_program(program))
    }
    /// Start an installed copy of a plug-in's program with `args`, `tmp` as its temp folder;
    /// the returned handle tells when it has exited.
    #[cfg(not(target_arch = "wasm32"))]
    fn start_program(&self, found: &Found, args: &[String], tmp: &Path) -> Result<Box<dyn Running>, String>;
}

/// The real desktop: spawns the file manager or the program.
#[derive(Clone, Copy, Debug, Default)]
pub struct System;

impl Desktop for System {
    #[cfg(not(target_arch = "wasm32"))]
    fn reveal(&self, file: &Path) -> Result<(), String> {
        lcc_updater::reveal(file).map_err(|e| e.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    fn reveal(&self, _: &Path) -> Result<(), String> {
        Err("not available in the browser".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn launch(&self, line: &[String]) -> Result<(), String> {
        let (prog, rest) = line.split_first().ok_or("no program")?;
        std::process::Command::new(prog).args(rest).spawn().map(|_| ()).map_err(|e| format!("{prog}: {e}"))
    }
    #[cfg(target_arch = "wasm32")]
    fn launch(&self, _: &[String]) -> Result<(), String> {
        Err("not available in the browser".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn run_and_wait(&self, line: &[String]) -> Result<bool, String> {
        let (prog, rest) = line.split_first().ok_or("no program")?;
        let status = std::process::Command::new(prog).args(rest).stdin(std::process::Stdio::null()).status().map_err(|e| format!("{prog}: {e}"))?;
        Ok(status.success())
    }
    #[cfg(target_arch = "wasm32")]
    fn run_and_wait(&self, _: &[String]) -> Result<bool, String> {
        Err("not available in the browser".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn find_program(&self, program: &ProgramDecl) -> Vec<Found> {
        programs::find(program)
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn find_program_soon(&self, program: &ProgramDecl) -> Option<Vec<Found>> {
        programs::find_soon(program)
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn start_program(&self, found: &Found, args: &[String], tmp: &Path) -> Result<Box<dyn Running>, String> {
        programs::start(found, args, tmp)
    }
}

/// A desktop that only records what was asked (headless sessions and tests). Every request
/// succeeds; [`Desktop::run_and_wait`] reports a successful exit. A plug-in's program counts as
/// installed wherever its entry names one for this system (at a made-up path), and a started one
/// runs until [`Recorder::finish_programs`].
#[derive(Debug, Default)]
pub struct Recorder {
    requests: Mutex<Vec<Request>>,
    #[cfg(not(target_arch = "wasm32"))]
    programs_done: Arc<AtomicBool>,
}

impl Recorder {
    /// The requests so far, oldest first.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
    /// Every program started so far exits (successfully), now and from now on.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn finish_programs(&self) {
        self.programs_done.store(true, Ordering::Relaxed);
    }
    fn push(&self, r: Request) {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).push(r);
    }
}

impl Desktop for Recorder {
    fn reveal(&self, file: &Path) -> Result<(), String> {
        self.push(Request::Reveal(file.to_path_buf()));
        Ok(())
    }
    fn launch(&self, line: &[String]) -> Result<(), String> {
        self.push(Request::Launch(line.to_vec()));
        Ok(())
    }
    fn run_and_wait(&self, line: &[String]) -> Result<bool, String> {
        self.push(Request::RunAndWait(line.to_vec()));
        Ok(true)
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn find_program(&self, program: &ProgramDecl) -> Vec<Found> {
        if cfg!(target_os = "macos") {
            let found = |b: &String| Found { bundle_id: Some(b.clone()), path: format!("/Applications/{b}.app"), version: None };
            program.mac_bundle_ids.iter().map(found).collect()
        } else {
            let exe = if cfg!(windows) { &program.windows_exe } else { &program.linux_exe };
            exe.iter().map(|e| Found { bundle_id: None, path: format!("/recorded/{e}"), version: None }).collect()
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn start_program(&self, found: &Found, args: &[String], tmp: &Path) -> Result<Box<dyn Running>, String> {
        self.push(Request::StartProgram { path: found.path.clone(), args: args.to_vec(), tmp: tmp.to_path_buf() });
        Ok(Box::new(Recorded(self.programs_done.clone())))
    }
}

/// A program [`Recorder`] "started": it runs until [`Recorder::finish_programs`].
#[cfg(not(target_arch = "wasm32"))]
struct Recorded(Arc<AtomicBool>);

#[cfg(not(target_arch = "wasm32"))]
impl Running for Recorded {
    fn try_wait(&mut self) -> Result<Option<Exit>, String> {
        Ok(self.0.load(Ordering::Relaxed).then_some(Exit { success: true, code: Some(0) }))
    }
}

/// Finding and starting plug-ins' programs on the real desktop.
#[cfg(not(target_arch = "wasm32"))]
mod programs {
    use super::*;

    /// How long a lookup is reused: the Plug-in Manager asks every frame it's open.
    const CACHE_FOR: std::time::Duration = std::time::Duration::from_secs(10);
    /// Longest a lookup command may take before it's stopped (and finds nothing).
    #[cfg(any(target_os = "macos", windows, test))]
    const LOOKUP_LIMIT: std::time::Duration = std::time::Duration::from_secs(3);
    /// Most programs whose lookups are kept.
    const CACHE_SIZE: usize = 64;

    /// A program's last lookup.
    struct Lookup {
        program: ProgramDecl,
        /// The answer and when it was made (`None`: none yet).
        found: Option<(std::time::Instant, Vec<Found>)>,
        /// A background lookup is running.
        pending: bool,
    }

    static CACHE: Mutex<Vec<Lookup>> = Mutex::new(Vec::new());

    fn cache() -> std::sync::MutexGuard<'static, Vec<Lookup>> {
        CACHE.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps `found` as `program`'s answer (and ends its background lookup).
    fn remember(program: &ProgramDecl, found: Option<Vec<Found>>) {
        let mut c = cache();
        let at = std::time::Instant::now();
        if let Some(l) = c.iter_mut().find(|l| l.program == *program) {
            l.pending = false;
            if let Some(f) = found {
                l.found = Some((at, f));
            }
            return;
        }
        if c.len() >= CACHE_SIZE {
            // the oldest answer that isn't being looked up goes
            let oldest = c.iter().enumerate().filter(|(_, l)| !l.pending).min_by_key(|(_, l)| l.found.as_ref().map(|(t, _)| *t)).map(|(i, _)| i);
            match oldest {
                Some(i) => {
                    c.remove(i);
                }
                None => return,
            }
        }
        c.push(Lookup { program: program.clone(), found: found.map(|f| (at, f)), pending: false });
    }

    pub(super) fn find(program: &ProgramDecl) -> Vec<Found> {
        let fresh = cache()
            .iter()
            .find(|l| l.program == *program)
            .and_then(|l| l.found.as_ref().filter(|(t, _)| t.elapsed() < CACHE_FOR).map(|(_, f)| f.clone()));
        if let Some(f) = fresh {
            return f;
        }
        let found = find_now(program);
        remember(program, Some(found.clone()));
        found
    }

    pub(super) fn find_soon(program: &ProgramDecl) -> Option<Vec<Found>> {
        if !cache().iter().any(|l| l.program == *program) {
            remember(program, None);
        }
        let mut c = cache();
        // not kept (every slot is being looked up): ask again next frame
        let l = c.iter_mut().find(|l| l.program == *program)?;
        let answer = l.found.as_ref().map(|(_, f)| f.clone());
        let stale = l.found.as_ref().is_none_or(|(t, _)| t.elapsed() >= CACHE_FOR);
        if stale && !l.pending {
            l.pending = true;
            let decl = program.clone();
            let spawned = std::thread::Builder::new().name("program-lookup".into()).spawn(move || {
                let found = find_now(&decl);
                remember(&decl, Some(found));
            });
            if let Err(e) = spawned {
                log::warn!("could not look for {}: {e}", program.name);
                l.pending = false;
                // say "not installed" rather than "checking" for ever
                return Some(answer.unwrap_or_default());
            }
        }
        answer
    }

    /// macOS: each bundle id through Spotlight's index of apps (what LaunchServices knows), its
    /// version from the app's `Info.plist` through `plutil`.
    #[cfg(target_os = "macos")]
    fn find_now(program: &ProgramDecl) -> Vec<Found> {
        program
            .mac_bundle_ids
            .iter()
            .filter_map(|b| {
                // bundle ids are checked to be [A-Za-z0-9._-] (lcc-plugins), so the query can't be escaped
                let out = command_output(Path::new("/usr/bin/mdfind"), &[&format!("kMDItemCFBundleIdentifier == '{b}'")])?;
                let home = std::env::var_os("HOME").map(PathBuf::from);
                let app = best_app(&out, home.as_deref(), |l| Path::new(l).is_dir())?.to_string();
                let plist = Path::new(&app).join("Contents/Info.plist");
                let version = command_output(
                    Path::new("/usr/bin/plutil"),
                    &["-extract", "CFBundleShortVersionString", "raw", "-o", "-", &plist.to_string_lossy()],
                )
                .map(|v| v.trim().chars().take(64).collect::<String>())
                .filter(|v| !v.is_empty());
                Some(Found { bundle_id: Some(b.clone()), path: app, version })
            })
            .collect()
    }

    /// The copy of an app to start, of the paths Spotlight printed: one in `/Applications` or
    /// `~/Applications` first, then elsewhere on the startup disk, then on another volume (a
    /// mounted disk image, an external drive) or inside another app; never one in a Trash.
    #[cfg(any(target_os = "macos", all(test, unix)))]
    pub(super) fn best_app<'a>(out: &'a str, home: Option<&Path>, exists: impl Fn(&str) -> bool) -> Option<&'a str> {
        let user_apps = home.map(|h| h.join("Applications"));
        let rank = |l: &str| {
            let p = Path::new(l);
            if p.starts_with("/Volumes") || l.trim_end_matches(".app").contains(".app/") {
                2
            } else if p.starts_with("/Applications") || user_apps.as_ref().is_some_and(|u| p.starts_with(u)) {
                0
            } else {
                1
            }
        };
        out.lines()
            .map(str::trim)
            .filter(|l| l.ends_with(".app") && Path::new(l).is_absolute() && !l.contains("/.Trash/") && !l.contains("/.Trashes/"))
            .filter(|l| exists(l))
            .min_by_key(|l| rank(l))
    }

    /// Windows: the executable registered under App Paths (the user's, the machine's, then the
    /// machine's 32-bit view), else the first on `PATH`. The registry is read with the system's
    /// own `reg.exe`, never one found on `PATH`.
    #[cfg(windows)]
    fn find_now(program: &ProgramDecl) -> Vec<Found> {
        let Some(exe) = &program.windows_exe else { return Vec::new() };
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).filter(|r| r.is_absolute()).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let reg = root.join("System32").join("reg.exe");
        let keys = [
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\App Paths",
            r"HKLM\Software\Microsoft\Windows\CurrentVersion\App Paths",
            r"HKLM\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths",
        ];
        let registered = keys.iter().find_map(|key| {
            let out = command_output(&reg, &["query", &format!(r"{key}\{exe}"), "/ve"])?;
            let path = registry_default(&out)?;
            (Path::new(&path).is_absolute() && Path::new(&path).is_file()).then_some(path)
        });
        registered.or_else(|| on_path(exe, ';')).map(|path| Found { bundle_id: None, path, version: None }).into_iter().collect()
    }

    /// The value in `reg query … /ve` output: `    (Default)    REG_SZ    C:\Tool\Tool.exe`, or
    /// `REG_EXPAND_SZ` with `%VARIABLES%` (expanded).
    #[cfg(any(windows, test))]
    pub(super) fn registry_default(out: &str) -> Option<String> {
        out.lines().find_map(|l| {
            let (expand, value) = match l.split_once("REG_EXPAND_SZ") {
                Some((_, v)) => (true, v),
                None => (false, l.split_once("REG_SZ")?.1),
            };
            let value = value.trim().trim_matches('"');
            let value = if expand { expand_vars(value, |k| std::env::var(k).ok())? } else { value.to_string() };
            (!value.is_empty()).then_some(value)
        })
    }

    /// `%NAME%` replaced by `var(NAME)`; `None` when one isn't set (the path would be wrong).
    #[cfg(any(windows, test))]
    pub(super) fn expand_vars(s: &str, var: impl Fn(&str) -> Option<String>) -> Option<String> {
        let mut out = String::new();
        let mut rest = s;
        while let Some(start) = rest.find('%') {
            out.push_str(rest.get(..start)?);
            let after = rest.get(start + 1..)?;
            let end = after.find('%')?;
            out.push_str(&var(after.get(..end)?)?);
            rest = after.get(end + 1..)?;
        }
        out.push_str(rest);
        Some(out)
    }

    /// Linux and the rest: the first executable of that name on `PATH`.
    #[cfg(all(not(target_os = "macos"), not(windows)))]
    fn find_now(program: &ProgramDecl) -> Vec<Found> {
        let Some(exe) = &program.linux_exe else { return Vec::new() };
        on_path(exe, ':').map(|path| Found { bundle_id: None, path, version: None }).into_iter().collect()
    }

    /// The first executable `exe` in the absolute folders of `PATH` (a relative entry, such as
    /// `.`, would depend on the app's current folder).
    #[cfg(not(target_os = "macos"))]
    fn on_path(exe: &str, sep: char) -> Option<String> {
        search(&std::env::var("PATH").ok()?, exe, sep)
    }

    /// [`on_path`] in the folder list `path`.
    #[cfg(any(not(target_os = "macos"), test))]
    pub(super) fn search(path: &str, exe: &str, sep: char) -> Option<String> {
        path.split(sep)
            .map(|d| Path::new(d.trim_matches('"')))
            .filter(|d| d.is_absolute())
            .map(|d| d.join(exe))
            .find(|p| executable(p))
            .map(|p| p.display().to_string())
    }

    #[cfg(all(unix, any(not(target_os = "macos"), test)))]
    fn executable(p: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }

    #[cfg(windows)]
    fn executable(p: &Path) -> bool {
        p.is_file()
    }

    /// Windows: no console window for a program started from the app.
    fn no_console(c: &mut std::process::Command) {
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(c, 0x0800_0000); // CREATE_NO_WINDOW
        let _ = c;
    }

    /// A short command's standard output (the first MiB), `None` when it fails or takes longer
    /// than [`LOOKUP_LIMIT`] (it is stopped then).
    #[cfg(any(target_os = "macos", windows, test))]
    pub(super) fn command_output(program: &Path, args: &[&str]) -> Option<String> {
        use std::io::Read;
        use std::process::{Command, Stdio};
        let mut c = Command::new(program);
        c.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        no_console(&mut c);
        let mut child = c.spawn().ok()?;
        let stop = |child: &mut std::process::Child| {
            let _ = child.kill();
            let _ = child.wait();
        };
        let Some(out) = child.stdout.take() else {
            stop(&mut child);
            return None;
        };
        // read while waiting, so a full pipe can't stall the command
        let reader = std::thread::Builder::new().name("lookup-output".into()).spawn(move || {
            let mut buf = Vec::new();
            let _ = out.take(1 << 20).read_to_end(&mut buf);
            buf
        });
        let Ok(reader) = reader else {
            stop(&mut child);
            return None;
        };
        let started = std::time::Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() < LOOKUP_LIMIT => std::thread::sleep(std::time::Duration::from_millis(10)),
                _ => {
                    log::warn!("{} took too long; stopped", program.display());
                    stop(&mut child);
                    return None;
                }
            }
        };
        let out = reader.join().ok()?;
        status.success().then(|| String::from_utf8_lossy(&out).chars().take(64 * 1024).collect())
    }

    /// A started program.
    struct Child(std::process::Child);

    impl Running for Child {
        fn try_wait(&mut self) -> Result<Option<Exit>, String> {
            // on macOS this is `open`'s status, not the app's: no code then
            let code = |s: std::process::ExitStatus| if cfg!(target_os = "macos") { None } else { s.code() };
            self.0.try_wait().map(|s| s.map(|s| Exit { success: s.success(), code: code(s) })).map_err(|e| e.to_string())
        }
    }

    /// macOS: `open -n -W -a <app> --env TMPDIR=<tmp>/ --args …`: a new instance of that app,
    /// `open` waiting until it quits (`--env` is in `open` since macOS 10.13 at the latest; the app
    /// needs 11). Elsewhere the executable itself, its temp folder variables set, with no console
    /// window on Windows. Nothing reads standard input or writes to the app's terminal.
    pub(super) fn start(found: &Found, args: &[String], tmp: &Path) -> Result<Box<dyn Running>, String> {
        use std::process::{Command, Stdio};
        let mut c = if cfg!(target_os = "macos") {
            let mut c = Command::new("/usr/bin/open");
            c.args(["-n", "-W", "-a"]).arg(&found.path).arg("--env").arg(format!("TMPDIR={}/", tmp.display()));
            if !args.is_empty() {
                c.arg("--args").args(args);
            }
            c
        } else {
            let mut c = Command::new(&found.path);
            c.args(args).env("TMPDIR", tmp).env("TEMP", tmp).env("TMP", tmp);
            c
        };
        no_console(&mut c);
        let child = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("{}: {e}", found.path))?;
        Ok(Box::new(Child(child)))
    }
}

/// The desktop a new session gets: the real one, except in this crate's unit tests.
pub(crate) fn default_desktop() -> std::sync::Arc<dyn Desktop> {
    if cfg!(test) { std::sync::Arc::new(Recorder::default()) } else { std::sync::Arc::new(System) }
}

#[cfg(test)]
mod tests {
    use super::programs::*;

    #[cfg(unix)]
    #[test]
    fn spotlight_results_prefer_installed_apps_over_trash_and_disk_images() {
        let home = std::path::Path::new("/Users/me");
        let out = "/Users/me/.Trash/Conv.app\n/Volumes/Conv 2/Conv.app\n/Applications/Suite.app/Contents/Helpers/Conv.app\n/Users/me/Downloads/Conv.app\n/Users/me/Applications/Conv.app\n/Applications/Conv.app\n";
        assert_eq!(
            best_app(out, Some(home), |_| true),
            Some("/Users/me/Applications/Conv.app"),
            "an Applications folder first, in Spotlight's order"
        );
        let out = "/Users/me/.Trash/Conv.app\n/Volumes/Conv 2/Conv.app\n/Users/me/Downloads/Conv.app\n";
        assert_eq!(best_app(out, Some(home), |_| true), Some("/Users/me/Downloads/Conv.app"), "the startup disk before another volume");
        let out = "/Users/me/.Trash/Conv.app\n/Volumes/Conv 2/Conv.app\n/Other.app/Contents/Helpers/Conv.app\n";
        assert_eq!(best_app(out, Some(home), |_| true), Some("/Volumes/Conv 2/Conv.app"), "a disk image rather than nothing");
        assert_eq!(best_app("/Users/me/.Trash/Conv.app\n/Volumes/X/.Trashes/501/Conv.app\n", Some(home), |_| true), None, "never one in a Trash");
        assert_eq!(best_app("/Applications/Conv.app\nrelative/Conv.app\n", None, |l| l != "/Applications/Conv.app"), None, "gone or relative: none");
    }

    #[test]
    fn registry_values_are_read_and_expanded() {
        let sz = "\r\nHKEY_LOCAL_MACHINE\\...\\Tool.exe\r\n    (Default)    REG_SZ    \"C:\\Program Files\\Tool\\Tool.exe\"\r\n";
        assert_eq!(registry_default(sz).as_deref(), Some("C:\\Program Files\\Tool\\Tool.exe"));
        let var = |k: &str| (k == "ProgramFiles").then(|| "C:\\Program Files".to_string());
        assert_eq!(expand_vars("%ProgramFiles%\\Tool\\Tool.exe", var).as_deref(), Some("C:\\Program Files\\Tool\\Tool.exe"));
        assert_eq!(expand_vars("%Unset%\\Tool.exe", var), None, "a variable that isn't set finds nothing");
        assert_eq!(expand_vars("%ProgramFiles", var), None, "an unclosed variable finds nothing");
        assert_eq!(registry_default("    (Default)    REG_EXPAND_SZ    %ZZ_NOT_SET_ANYWHERE%\\Tool.exe"), None);
        assert_eq!(registry_default("ERROR: The system was unable to find the specified registry key"), None);
    }

    #[cfg(unix)]
    #[test]
    fn only_absolute_path_folders_are_searched() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("lcc-desktop-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("lcc-tool");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(search(&format!("/nonexistent:{}", dir.display()), "lcc-tool", ':'), Some(exe.display().to_string()));
        // the same folder named relative to the current folder is skipped
        let up = "../".repeat(std::env::current_dir().unwrap().components().count());
        let relative = format!("{up}{}", dir.strip_prefix("/").unwrap().display());
        assert!(std::path::Path::new(&relative).join("lcc-tool").is_file(), "{relative} names the folder");
        assert_eq!(search(&relative, "lcc-tool", ':'), None, "a relative PATH entry is not searched");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_lookup_for_the_ui_never_waits() {
        let decl =
            lcc_plugins::ProgramDecl { id: "nowhere".into(), name: "Nowhere".into(), mac_bundle_ids: vec![], windows_exe: None, linux_exe: None };
        assert_eq!(find_soon(&decl), None, "the first answer comes from the background");
        let start = std::time::Instant::now();
        while find_soon(&decl).is_none() {
            assert!(start.elapsed() < std::time::Duration::from_secs(10), "the background lookup answers");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(find_soon(&decl), Some(vec![]), "then the answer is kept");
    }

    #[cfg(unix)]
    #[test]
    fn a_lookup_command_that_hangs_is_stopped() {
        let t = std::time::Instant::now();
        let sh = std::path::Path::new("/bin/sh");
        assert_eq!(command_output(sh, &["-c", "sleep 30"]), None);
        assert!(t.elapsed() < std::time::Duration::from_secs(10), "stopped after its limit: {:?}", t.elapsed());
        assert_eq!(command_output(sh, &["-c", "echo hi"]).as_deref(), Some("hi\n"));
    }
}
