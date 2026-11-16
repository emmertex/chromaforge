//! The Android app's calls into its activity, over JNI, and the activity's answers.
//!
//! The app runs as a native library inside a small Java activity (`packaging/android`, a
//! `NativeActivity` subclass). What only Java can do goes through it: the system pickers (the
//! Storage Access Framework, which hands out `content://` documents rather than paths) and opening
//! links, and the on-screen keyboard (`nativeIme`, queued for the next frame: [`take_ime`]). The activity copies picked documents into the app's own folders, so the app only ever
//! sees file paths, and watches a saved file to copy it to the document the user chose. A folder
//! or file on the device's own storage that the app may read (with "All files access") is answered
//! with its real path instead, so nothing is copied. Copies that remain report through
//! [`CopyProgress`], so the app can show them in its activity stack and stop them.
//!
//! A picker answers asynchronously: [`Host::pick`] returns a channel at once, and the activity
//! calls back into native code (`nativePicked`, registered at start-up, so no symbol depends on
//! the package name) when the user is done. The registered waker then asks the UI for a frame.
//!
//! This is one of the crates allowed `unsafe`: the `jvm` module adopts the JVM and activity
//! handles android-activity gives the app. Its API is safe. Every JNI failure, a Java exception
//! included, comes back as an [`Error`]; nothing here panics, and the native callback never lets a
//! panic cross into Java.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fmt;

/// What a picker chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    /// Any number of files.
    Files,
    /// One file.
    File,
    /// One folder: its files are copied in (the app gets the copy's folder).
    Folder,
    /// Where to save one file: the app gets a path to write; the activity copies the file to the
    /// chosen document once it is written.
    Save,
}

impl PickKind {
    /// The number the activity's `startPick` takes.
    pub fn code(self) -> i32 {
        match self {
            PickKind::Files => 0,
            PickKind::File => 1,
            PickKind::Folder => 2,
            PickKind::Save => 3,
        }
    }

    /// The kind for a code from the activity.
    pub fn from_code(code: i64) -> Option<PickKind> {
        Some(match code {
            0 => PickKind::Files,
            1 => PickKind::File,
            2 => PickKind::Folder,
            3 => PickKind::Save,
            _ => return None,
        })
    }
}

/// A picker answer no request was waiting for: the system killed the app while the picker was up
/// (it reclaims memory from apps in the background) and started it again for the answer, so the
/// request that showed the picker is gone. The app decides what to do with it ([`Host::take_unclaimed`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unclaimed {
    pub kind: PickKind,
    /// The paths of the copies, or why there are none.
    pub paths: Result<Vec<String>, String>,
}

/// Where the activity's answers go: the channel of the request that is waiting for one, or the
/// [`Unclaimed`] queue.
#[derive(Default)]
pub struct Answers {
    waiting: std::collections::HashMap<i64, std::sync::mpsc::Sender<Vec<String>>>,
    unclaimed: Vec<Unclaimed>,
}

/// Why the activity's answer has no paths when it reports a failure.
pub const PICK_FAILED: &str = "the chosen files could not be copied into the app (see the log)";

impl Answers {
    /// A new request with `id`: its answer comes through the returned channel.
    pub fn wait(&mut self, id: i64) -> std::sync::mpsc::Receiver<Vec<String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.waiting.insert(id, tx);
        rx
    }

    /// Forgets a request whose picker never opened.
    pub fn forget(&mut self, id: i64) {
        self.waiting.remove(&id);
    }

    /// The activity's answer for `id`: `None` = the pick failed. A failed request's channel is
    /// dropped (the app reports a dialog that ended without an answer); an id ≤ 0 (or one nothing
    /// waits for) is an [`Unclaimed`] answer of kind `-id - 1`.
    pub fn answer(&mut self, id: i64, paths: Option<Vec<String>>) {
        if let Some(tx) = self.waiting.remove(&id) {
            match paths {
                Some(p) => {
                    log::info!("picker {id}: {} path(s)", p.len());
                    if tx.send(p).is_err() {
                        log::warn!("picker {id}: answered, but nothing is waiting any more");
                    }
                }
                None => log::error!("picker {id}: {PICK_FAILED}"),
            }
            return;
        }
        let kind = id.checked_neg().and_then(|n| n.checked_sub(1)).and_then(PickKind::from_code);
        let Some(kind) = kind else {
            log::warn!("picker {id}: an answer for a request that is gone; dropped");
            return;
        };
        log::warn!("picker: an answer ({kind:?}) for a request made before the app was restarted");
        self.unclaimed.push(Unclaimed { kind, paths: paths.ok_or_else(|| PICK_FAILED.to_string()) });
    }

    /// The unclaimed answers so far, oldest first.
    pub fn take_unclaimed(&mut self) -> Vec<Unclaimed> {
        std::mem::take(&mut self.unclaimed)
    }
}

/// The camera roll's answer: the photos' paths, or why there are none.
pub type CameraRoll = Result<Vec<String>, String>;

/// Answers waiting for the activity by request id: camera-roll listings (`nativeCameraRoll`) and
/// saves to Pictures (`nativeGallerySaved`).
#[derive(Default)]
pub struct RollAnswers {
    waiting: std::collections::HashMap<i64, std::sync::mpsc::Sender<CameraRoll>>,
}

impl RollAnswers {
    /// A new listing with `id`: its answer comes through the returned channel.
    pub fn wait(&mut self, id: i64) -> std::sync::mpsc::Receiver<CameraRoll> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.waiting.insert(id, tx);
        rx
    }

    /// Forgets a listing that never started.
    pub fn forget(&mut self, id: i64) {
        self.waiting.remove(&id);
    }

    /// The activity's answer for `id`: `paths` (null = failed) and, on failure, `error` (the
    /// activity's reason, e.g. the permission was refused).
    pub fn answer(&mut self, id: i64, paths: Option<Vec<String>>, error: Option<String>) {
        let Some(tx) = self.waiting.remove(&id) else {
            log::warn!("request {id}: an answer nothing waits for; dropped");
            return;
        };
        let answer = match (paths, error) {
            (_, Some(e)) if !e.is_empty() => Err(e),
            (Some(p), _) => Ok(p),
            (None, _) => Err("the activity could not do it (see the log)".into()),
        };
        if tx.send(answer).is_err() {
            log::warn!("request {id}: answered, but nothing is waiting any more");
        }
    }
}

/// A picker to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickRequest {
    pub kind: PickKind,
    /// MIME types the picker offers (`image/*`, …); empty = any file.
    pub mime_types: Vec<String>,
    /// [`PickKind::Save`]: the suggested file name.
    pub file_name: Option<String>,
}

impl PickRequest {
    /// MIME types for a list of file extensions (without dots). Extensions with no well-known type
    /// widen the picker to any file, so a raw or preset file is never greyed out.
    pub fn mime_types_for(extensions: &[&str]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for ext in extensions {
            let Some(mime) = mime_for(ext) else { return Vec::new() };
            if !out.iter().any(|m| m == mime) {
                out.push(mime.to_string());
            }
        }
        out
    }
}

/// The MIME type Android's document providers report for an extension, where one is well known.
fn mime_for(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "tif" | "tiff" => "image/tiff",
        "webp" => "image/webp",
        "heic" | "heif" => "image/heif",
        "avif" => "image/avif",
        "gif" => "image/gif",
        "json" => "application/json",
        "txt" => "text/plain",
        "zip" => "application/zip",
        "pdf" => "application/pdf",
        _ => return None,
    })
}

/// What the on-screen keyboard sent (the activity's input connection, `nativeIme`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImeEvent {
    /// Final text at the cursor.
    Commit(String),
    /// The composition so far (empty clears it).
    Preedit(String),
    /// Keep the composition as final text.
    FinishComposing,
    /// Delete characters before and after the cursor.
    DeleteSurrounding {
        before: usize,
        after: usize,
    },
    Backspace,
    /// Forward delete.
    Delete,
    Enter,
}

impl ImeEvent {
    /// Decodes `nativeIme(kind, text, a, b)` (the codes in `MainActivity.java.in`); `None` for an
    /// unknown kind. Negative counts are zero.
    pub fn from_wire(kind: i32, text: Option<String>, a: i32, b: i32) -> Option<ImeEvent> {
        let count = |n: i32| usize::try_from(n).unwrap_or(0);
        Some(match kind {
            0 => ImeEvent::Commit(text.unwrap_or_default()),
            1 => ImeEvent::Preedit(text.unwrap_or_default()),
            2 => ImeEvent::FinishComposing,
            3 => ImeEvent::DeleteSurrounding { before: count(a), after: count(b) },
            4 => match a {
                0 => ImeEvent::Backspace,
                1 => ImeEvent::Enter,
                2 => ImeEvent::Delete,
                _ => return None,
            },
            _ => return None,
        })
    }
}

/// Keyboard input waiting for the next frame, oldest first (the activity's UI thread adds, the
/// app's frame takes). Bounded: a flood is dropped rather than held.
static IME_QUEUE: std::sync::Mutex<Vec<ImeEvent>> = std::sync::Mutex::new(Vec::new());
const IME_QUEUE_MAX: usize = 4096;

/// Queues keyboard input; `false` when the queue is full and it was dropped.
pub fn push_ime(event: ImeEvent) -> bool {
    let mut q = IME_QUEUE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if q.len() >= IME_QUEUE_MAX {
        return false;
    }
    q.push(event);
    true
}

/// Takes the keyboard input that arrived since the last call.
pub fn take_ime() -> Vec<ImeEvent> {
    std::mem::take(&mut *IME_QUEUE.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// Back presses (the key, the gesture, predictive back on Android 13+) the activity reported
/// (`nativeBack`) that the app's next frame has not taken yet.
static BACKS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Most presses kept waiting: a flood can't build up a queue of Backs.
const BACKS_MAX: u32 = 16;

/// Counts a Back press for the next frame.
pub fn push_back() {
    // (a press past the bound is dropped: the count never wraps)
    if BACKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= BACKS_MAX {
        BACKS.store(BACKS_MAX, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The Back presses since the last call.
pub fn take_back() -> u32 {
    BACKS.swap(0, std::sync::atomic::Ordering::Relaxed)
}

/// The window's edges hidden by the display cutout and the system bars, in pixels (left, top,
/// right, bottom), as the activity last reported them (`nativeInsets`); `None` before the first.
static INSETS: std::sync::Mutex<Option<[i32; 4]>> = std::sync::Mutex::new(None);

/// Records the window's insets (negative values are zero).
pub fn set_insets(px: [i32; 4]) {
    *INSETS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(px.map(|v| v.max(0)));
}

/// The window's insets in pixels (left, top, right, bottom), once the activity has reported them.
pub fn insets() -> Option<[i32; 4]> {
    *INSETS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Where the activity's copies of picked documents report to (the app's activity stack).
///
/// A copy that takes a while (a folder on an SD card, files from a cloud provider) calls
/// [`CopyProgress::progress`] every so often with its own task number, starting with the first
/// file it really copies, and [`CopyProgress::ended`] once when it is done. The copy loop stops
/// at its next check once `progress` answers `true`.
pub trait CopyProgress: Send + Sync {
    /// `done` of `total` files (`total` 0 = not known yet), `detail` the file being copied.
    /// Returns whether the copy should stop.
    fn progress(&self, task: i64, done: u64, total: u64, detail: &str) -> bool;
    /// The copy `task` is over (finished, failed or stopped).
    fn ended(&self, task: i64);
}

/// A failed call into the activity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// The running activity, as android-activity (and so winit / eframe) know it.
#[cfg(target_os = "android")]
pub use android_activity::AndroidApp;
#[cfg(target_os = "android")]
pub use imp::Host;

/// Whether the app offers "All files access" (Settings ▸ Folders ▸ Device storage, `app.allFilesAccess`), given
/// whether the manifest declares it. A build without it (the Play build) never asks, and an unreadable answer counts
/// as not declared: asking for a permission the manifest lacks opens a settings page that can't grant it.
pub fn offers_all_files_access(declared: Result<bool, Error>) -> bool {
    matches!(declared, Ok(true))
}

#[cfg(target_os = "android")]
mod imp {
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::mpsc::Receiver;
    use std::sync::{Mutex, OnceLock, PoisonError};

    use android_activity::AndroidApp;
    use jni::objects::{GlobalRef, JObject, JObjectArray, JString, JValue};
    use jni::sys::jlong;
    use jni::{JNIEnv, JavaVM, NativeMethod};

    use super::{Answers, CopyProgress, Error, PickRequest, RollAnswers, Unclaimed};

    /// Pickers that are up, by the id the activity answers with, and answers nothing waits for.
    static PENDING: Mutex<Option<Answers>> = Mutex::new(None);
    static NEXT_ID: AtomicI64 = AtomicI64::new(1);
    /// Camera-roll listings that are running.
    static ROLLS: Mutex<Option<RollAnswers>> = Mutex::new(None);
    /// Saves to Pictures (and shares) that are running; answered like camera-roll listings.
    static GALLERY: Mutex<Option<RollAnswers>> = Mutex::new(None);
    /// Asks the UI for a frame (set by the app: an answer must not wait for the next touch).
    static WAKER: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
    /// Where copies report (set by the app once its session exists).
    static COPIES: Mutex<Option<std::sync::Arc<dyn CopyProgress>>> = Mutex::new(None);

    fn jni_error(what: &str, e: jni::errors::Error) -> Error {
        Error(format!("{what}: {e}"))
    }

    /// The app's handle on its activity.
    pub struct Host {
        vm: JavaVM,
        activity: GlobalRef,
    }

    impl Host {
        /// Adopts the activity android-activity started the app in and registers the native
        /// callbacks on its class.
        pub fn new(app: &AndroidApp) -> Result<Host, Error> {
            let vm = super::jvm::vm(app)?;
            let activity = {
                let mut env = vm.attach_current_thread_permanently().map_err(|e| jni_error("attach to the JVM", e))?;
                let local = super::jvm::activity(app)?;
                let global = env.new_global_ref(&local).map_err(|e| jni_error("reference the activity", e))?;
                let class = env.get_object_class(&global).map_err(|e| jni_error("the activity's class", e))?;
                let methods = [
                    NativeMethod {
                        name: "nativePicked".into(),
                        sig: "(J[Ljava/lang/String;)V".into(),
                        fn_ptr: native_picked as *mut std::ffi::c_void,
                    },
                    NativeMethod {
                        name: "nativeCopyProgress".into(),
                        sig: "(JJJLjava/lang/String;)Z".into(),
                        fn_ptr: native_copy_progress as *mut std::ffi::c_void,
                    },
                    NativeMethod { name: "nativeCopyEnded".into(), sig: "(J)V".into(), fn_ptr: native_copy_ended as *mut std::ffi::c_void },
                    NativeMethod {
                        name: "nativeCameraRoll".into(),
                        sig: "(J[Ljava/lang/String;Ljava/lang/String;)V".into(),
                        fn_ptr: native_camera_roll as *mut std::ffi::c_void,
                    },
                    NativeMethod {
                        name: "nativeGallerySaved".into(),
                        sig: "(J[Ljava/lang/String;Ljava/lang/String;)V".into(),
                        fn_ptr: native_gallery_saved as *mut std::ffi::c_void,
                    },
                    crate::opened::native_method(),
                    NativeMethod { name: "nativeBack".into(), sig: "()V".into(), fn_ptr: native_back as *mut std::ffi::c_void },
                    NativeMethod { name: "nativeInsets".into(), sig: "(IIII)V".into(), fn_ptr: native_insets as *mut std::ffi::c_void },
                ];
                env.register_native_methods(&class, &methods).map_err(|e| jni_error("register the native callbacks", e))?;
                super::ime::register(&mut env, &class);
                global
            };
            let host = Host { vm, activity };
            // picker answers that came before this (the app was restarted for a picker's result) and
            // photos shared before the app was up waited in the activity: ask for them now
            if let Err(e) = host.call("hostReady", |env, activity| env.call_method(activity, "hostReady", "()V", &[]).map(|_| ())) {
                log::warn!("{e}");
            }
            Ok(host)
        }

        /// Picker answers no request was waiting for (see [`Unclaimed`]).
        pub fn take_unclaimed(&self) -> Vec<Unclaimed> {
            pending(Answers::take_unclaimed)
        }

        /// Photos opened, shared or dropped from other apps since the last call (paths of the
        /// activity's copies, oldest first). The app drains this every frame.
        pub fn take_opened(&self) -> Vec<String> {
            crate::opened::take()
        }

        /// Sets what asks the UI for a frame when an answer arrives. Only the first call counts.
        pub fn set_waker(&self, wake: Box<dyn Fn() + Send + Sync>) {
            let _ = WAKER.set(wake);
        }

        /// Shows a picker; the channel answers with the chosen paths (none = cancelled).
        pub fn pick(&self, req: &PickRequest) -> Result<Receiver<Vec<String>>, Error> {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let rx = pending(|p| p.wait(id));
            let kind = req.kind.code();
            let result = self.call("startPick", |env, activity| {
                let mimes = string_array(env, &req.mime_types)?;
                let name: JObject = match &req.file_name {
                    Some(n) => env.new_string(n)?.into(),
                    None => JObject::null(),
                };
                env.call_method(
                    activity,
                    "startPick",
                    "(JI[Ljava/lang/String;Ljava/lang/String;)V",
                    &[JValue::Long(id), JValue::Int(kind), JValue::Object(&mimes), JValue::Object(&name)],
                )?;
                Ok(())
            });
            if let Err(e) = result {
                pending(|p| p.forget(id));
                return Err(e);
            }
            Ok(rx)
        }

        /// Lists the photos in the camera roll (DCIM/Camera, through MediaStore), asking for the
        /// photo permission first when the app lacks it. Photos the app can read where they are
        /// come back as their paths; others are copied (in the activity stack) first.
        pub fn camera_roll(&self) -> Result<Receiver<super::CameraRoll>, Error> {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let rx = rolls(|r| r.wait(id));
            let result = self.call("startCameraRoll", |env, activity| {
                env.call_method(activity, "startCameraRoll", "(J)V", &[JValue::Long(id)])?;
                Ok(())
            });
            if let Err(e) = result {
                rolls(|r| r.forget(id));
                return Err(e);
            }
            Ok(rx)
        }

        /// Copies exported files into the device's Pictures/`album` folder through MediaStore (so
        /// they show in the gallery), then, with `share`, offers them to other apps (the system
        /// share sheet, `ACTION_SEND`). The channel answers with the files' `content://` URIs, or
        /// why they could not be saved.
        pub fn save_to_gallery(&self, paths: &[String], album: &str, share: bool) -> Result<Receiver<super::CameraRoll>, Error> {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let rx = gallery(|r| r.wait(id));
            let result = self.call("saveToGallery", |env, activity| {
                let files = string_array(env, paths)?;
                let album = env.new_string(album)?;
                env.call_method(
                    activity,
                    "saveToGallery",
                    "(J[Ljava/lang/String;Ljava/lang/String;Z)V",
                    &[JValue::Long(id), JValue::Object(&files), JValue::Object(&album), JValue::Bool(u8::from(share))],
                )?;
                Ok(())
            });
            if let Err(e) = result {
                gallery(|r| r.forget(id));
                return Err(e);
            }
            Ok(rx)
        }

        /// Sets where the activity's copies of picked documents report their progress.
        pub fn set_copy_progress(&self, sink: std::sync::Arc<dyn CopyProgress>) {
            *COPIES.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
        }

        /// Whether the app may read every file on shared storage ("All files access" on Android
        /// 11+, the storage permission before), so folders can be read in place.
        pub fn has_all_files_access(&self) -> Result<bool, Error> {
            let mut granted = false;
            self.call("hasAllFilesAccess", |env, activity| {
                granted = env.call_method(activity, "hasAllFilesAccess", "()Z", &[])?.z()?;
                Ok(())
            })?;
            Ok(granted)
        }

        /// Whether this build's manifest declares "All files access" (`MANAGE_EXTERNAL_STORAGE`): the
        /// GitHub build does, the Play build doesn't. Read through the package manager.
        pub fn all_files_access_declared(&self) -> Result<bool, Error> {
            let mut declared = false;
            self.call("allFilesAccessDeclared", |env, activity| {
                declared = env.call_method(activity, "allFilesAccessDeclared", "()Z", &[])?.z()?;
                Ok(())
            })?;
            Ok(declared)
        }

        /// Asks for "All files access" (the system's settings page on Android 11+, the storage
        /// permission before). The answer is read later with [`Host::has_all_files_access`].
        pub fn ask_all_files_access(&self) -> Result<(), Error> {
            self.call("askAllFilesAccess", |env, activity| {
                env.call_method(activity, "askAllFilesAccess", "()V", &[])?;
                Ok(())
            })
        }

        /// Tells the activity the first frame is on screen: it asks its first-start questions
        /// ("All files access") only then, so they never cover the activity while its window and
        /// GPU surface are still being made.
        pub fn first_frame_shown(&self) -> Result<(), Error> {
            self.call("firstFrameShown", |env, activity| {
                env.call_method(activity, "firstFrameShown", "()V", &[])?;
                Ok(())
            })
        }

        /// Opens a web link in the browser.
        pub fn open_url(&self, url: &str) -> Result<(), Error> {
            self.call("openUrl", |env, activity| {
                let url = env.new_string(url)?;
                env.call_method(activity, "openUrl", "(Ljava/lang/String;)V", &[JValue::Object(&url)])?;
                Ok(())
            })
        }

        /// The system's Back, for when the app had nothing left to close: the app goes to the
        /// background (as Back does at an app's root), keeping its state.
        pub fn go_background(&self) -> Result<(), Error> {
            self.call("goBackground", |env, activity| {
                env.call_method(activity, "goBackground", "()V", &[])?;
                Ok(())
            })
        }

        /// Shows or hides the on-screen keyboard (the activity's input view takes focus to show it).
        pub fn set_keyboard(&self, show: bool) -> Result<(), Error> {
            self.call("setKeyboard", |env, activity| {
                env.call_method(activity, "setKeyboard", "(Z)V", &[JValue::Bool(u8::from(show))])?;
                Ok(())
            })
        }

        /// Runs `f` on this thread's JNI environment; a Java exception becomes an [`Error`].
        fn call(&self, what: &str, f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<()>) -> Result<(), Error> {
            let mut env = self.vm.attach_current_thread_permanently().map_err(|e| jni_error("attach to the JVM", e))?;
            // this thread stays attached for the life of the app, so local references would never
            // be freed on their own: give each call its own frame, dropped when it returns
            let activity = self.activity.as_obj();
            let result = env.with_local_frame(16, |env| f(env, activity));
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
                return Err(Error(format!("{what}: the activity threw an exception (see logcat)")));
            }
            result.map_err(|e| jni_error(what, e))
        }
    }

    /// Asks the UI for a frame, when the app has set a waker.
    pub(crate) fn wake() {
        if let Some(wake) = WAKER.get() {
            wake();
        }
    }

    fn pending<R>(f: impl FnOnce(&mut Answers) -> R) -> R {
        let mut guard = PENDING.lock().unwrap_or_else(PoisonError::into_inner);
        f(guard.get_or_insert_with(Answers::default))
    }

    fn rolls<R>(f: impl FnOnce(&mut RollAnswers) -> R) -> R {
        let mut guard = ROLLS.lock().unwrap_or_else(PoisonError::into_inner);
        f(guard.get_or_insert_with(RollAnswers::default))
    }

    fn gallery<R>(f: impl FnOnce(&mut RollAnswers) -> R) -> R {
        let mut guard = GALLERY.lock().unwrap_or_else(PoisonError::into_inner);
        f(guard.get_or_insert_with(RollAnswers::default))
    }

    /// `void nativeGallerySaved(long id, String[] uris, String error)`: the answer to
    /// [`Host::save_to_gallery`].
    extern "system" fn native_gallery_saved<'l>(mut env: JNIEnv<'l>, _this: JObject<'l>, id: jlong, uris: JObjectArray<'l>, error: JString<'l>) {
        // a panic must never unwind into the JVM
        let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let list = read_strings(&mut env, &uris).unwrap_or_else(|e| {
                log::error!("save to Pictures: answer unreadable: {e}");
                None
            });
            let error: Option<String> = if error.is_null() { None } else { env.get_string(&error).map(Into::into).ok() };
            gallery(|r| r.answer(id, list, error));
            wake();
        }));
        if answered.is_err() {
            log::error!("save to Pictures answer: a panic was caught");
        }
    }

    /// `void nativeCameraRoll(long id, String[] paths, String error)`: the answer to
    /// [`Host::camera_roll`].
    extern "system" fn native_camera_roll<'l>(mut env: JNIEnv<'l>, _this: JObject<'l>, id: jlong, paths: JObjectArray<'l>, error: JString<'l>) {
        // a panic must never unwind into the JVM
        let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let list = read_strings(&mut env, &paths).unwrap_or_else(|e| {
                log::error!("camera roll answer unreadable: {e}");
                None
            });
            let error: Option<String> = if error.is_null() { None } else { env.get_string(&error).map(Into::into).ok() };
            rolls(|r| r.answer(id, list, error));
            wake();
        }));
        if answered.is_err() {
            log::error!("camera roll answer: a panic was caught");
        }
    }

    fn string_array<'l>(env: &mut JNIEnv<'l>, items: &[String]) -> jni::errors::Result<JObjectArray<'l>> {
        let len = i32::try_from(items.len()).unwrap_or(i32::MAX);
        let array = env.new_object_array(len, "java/lang/String", JObject::null())?;
        for (i, s) in items.iter().take(len as usize).enumerate() {
            let js = env.new_string(s)?;
            env.set_object_array_element(&array, i as i32, &js)?;
            env.delete_local_ref(js)?;
        }
        Ok(array)
    }

    /// `None` for a null array (the activity's "the pick failed").
    pub(crate) fn read_strings(env: &mut JNIEnv, array: &JObjectArray) -> jni::errors::Result<Option<Vec<String>>> {
        if array.is_null() {
            return Ok(None);
        }
        let len = env.get_array_length(array)?;
        let mut out = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
        for i in 0..len {
            let item = env.get_object_array_element(array, i)?;
            let item = env.auto_local(item);
            if item.is_null() {
                continue;
            }
            let s: String = env.get_string(<&JString>::from(&*item))?.into();
            out.push(s);
        }
        Ok(Some(out))
    }

    /// `void nativePicked(long id, String[] paths)`: the activity's answer to [`Host::pick`].
    extern "system" fn native_picked<'l>(mut env: JNIEnv<'l>, _this: JObject<'l>, id: jlong, paths: JObjectArray<'l>) {
        // a panic must never unwind into the JVM
        let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let list = read_strings(&mut env, &paths).unwrap_or_else(|e| {
                log::error!("picker answer unreadable: {e}");
                None
            });
            pending(|p| p.answer(id, list));
            if let Some(wake) = WAKER.get() {
                wake();
            }
        }));
        if answered.is_err() {
            log::error!("picker answer: a panic was caught");
        }
    }

    /// `void nativeBack()`: Back was pressed (key, gesture or the predictive-back callback).
    extern "system" fn native_back<'l>(_env: JNIEnv<'l>, _this: JObject<'l>) {
        let done = std::panic::catch_unwind(|| {
            super::push_back();
            wake();
        });
        if done.is_err() {
            log::error!("back: a panic was caught");
        }
    }

    /// `void nativeInsets(int left, int top, int right, int bottom)`: the window's edges hidden by
    /// the display cutout and the system bars, in pixels, whenever they change.
    extern "system" fn native_insets<'l>(
        _env: JNIEnv<'l>,
        _this: JObject<'l>,
        left: jni::sys::jint,
        top: jni::sys::jint,
        right: jni::sys::jint,
        bottom: jni::sys::jint,
    ) {
        let done = std::panic::catch_unwind(|| {
            super::set_insets([left, top, right, bottom]);
            wake();
        });
        if done.is_err() {
            log::error!("insets: a panic was caught");
        }
    }

    fn copies() -> Option<std::sync::Arc<dyn CopyProgress>> {
        COPIES.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// `boolean nativeCopyProgress(long task, long done, long total, String detail)`: a copy's
    /// progress; answers whether it should stop.
    extern "system" fn native_copy_progress<'l>(
        mut env: JNIEnv<'l>,
        _this: JObject<'l>,
        task: jlong,
        done: jlong,
        total: jlong,
        detail: JString<'l>,
    ) -> jni::sys::jboolean {
        let stop = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let detail: String = if detail.is_null() { String::new() } else { env.get_string(&detail).map(Into::into).unwrap_or_default() };
            let done = u64::try_from(done).unwrap_or(0);
            let total = u64::try_from(total).unwrap_or(0);
            copies().is_some_and(|c| c.progress(task, done, total, &detail))
        }));
        match stop {
            Ok(true) => jni::sys::JNI_TRUE,
            Ok(false) => jni::sys::JNI_FALSE,
            Err(_) => {
                log::error!("copy progress: a panic was caught");
                jni::sys::JNI_FALSE
            }
        }
    }

    /// `void nativeCopyEnded(long task)`.
    extern "system" fn native_copy_ended<'l>(_env: JNIEnv<'l>, _this: JObject<'l>, task: jlong) {
        let ended = std::panic::catch_unwind(|| {
            if let Some(c) = copies() {
                c.ended(task);
            }
        });
        if ended.is_err() {
            log::error!("copy ended: a panic was caught");
        }
    }
}

/// Photos opened, shared or dropped from other apps (AND.6): the activity copies them and calls
/// `nativeOpened(String[] paths)`; they wait here until the app's next frame takes them.
#[cfg(target_os = "android")]
mod opened {
    use std::sync::{Mutex, PoisonError};

    use jni::objects::{JObject, JObjectArray};
    use jni::{JNIEnv, NativeMethod};

    /// Most paths that can wait: a runaway sender can't grow this without bound.
    const MAX_WAITING: usize = 10_000;

    static QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());

    pub fn native_method() -> NativeMethod {
        NativeMethod { name: "nativeOpened".into(), sig: "([Ljava/lang/String;)V".into(), fn_ptr: native_opened as *mut std::ffi::c_void }
    }

    pub fn take() -> Vec<String> {
        std::mem::take(&mut *QUEUE.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// `void nativeOpened(String[] paths)`.
    extern "system" fn native_opened<'l>(mut env: JNIEnv<'l>, _this: JObject<'l>, paths: JObjectArray<'l>) {
        // a panic must never unwind into the JVM
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let list = match super::imp::read_strings(&mut env, &paths) {
                Ok(l) => l.unwrap_or_default(),
                Err(e) => {
                    log::warn!("shared photos unreadable: {e}");
                    return;
                }
            };
            {
                let mut q = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
                let room = MAX_WAITING.saturating_sub(q.len());
                if list.len() > room {
                    log::warn!("{} shared photos dropped: too many waiting", list.len() - room);
                }
                q.extend(list.into_iter().take(room));
            }
            super::imp::wake();
        }));
        if done.is_err() {
            log::error!("shared photos: a panic was caught");
        }
    }
}

/// The on-screen keyboard's callback: the activity's input connection calls `nativeIme`.
#[cfg(target_os = "android")]
mod ime {
    use jni::JNIEnv;
    use jni::NativeMethod;
    use jni::objects::{JClass, JObject, JString};
    use jni::sys::jint;

    use super::ImeEvent;

    /// Registers `nativeIme` on the activity's class. A failure is logged: the app runs without
    /// typing from the on-screen keyboard rather than not at all.
    pub fn register(env: &mut JNIEnv, class: &JClass) {
        let methods =
            [NativeMethod { name: "nativeIme".into(), sig: "(ILjava/lang/String;II)V".into(), fn_ptr: native_ime as *mut std::ffi::c_void }];
        if let Err(e) = env.register_native_methods(class, &methods) {
            let _ = env.exception_clear();
            log::error!("no on-screen keyboard input (register nativeIme): {e}");
        }
    }

    /// `void nativeIme(int kind, String text, int a, int b)`: queued for the next frame, which the
    /// waker asks for.
    extern "system" fn native_ime<'l>(mut env: JNIEnv<'l>, _this: JObject<'l>, kind: jint, text: JString<'l>, a: jint, b: jint) {
        // a panic must never unwind into the JVM
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let text = if text.is_null() {
                None
            } else {
                match env.get_string(&text) {
                    Ok(s) => Some(String::from(s)),
                    Err(e) => {
                        log::warn!("keyboard text unreadable: {e}");
                        return;
                    }
                }
            };
            let Some(event) = ImeEvent::from_wire(kind, text, a, b) else {
                log::warn!("unknown keyboard event {kind}");
                return;
            };
            if !super::push_ime(event) {
                log::warn!("keyboard input dropped: the app is not reading it");
            }
            super::imp::wake();
        }));
        if done.is_err() {
            log::error!("keyboard input: a panic was caught");
        }
    }
}

/// The JVM handles android-activity gives the app, adopted for the `jni` crate.
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
mod jvm {
    use android_activity::AndroidApp;
    use jni::JavaVM;
    use jni::objects::JObject;

    use super::Error;

    /// The process's Java VM.
    pub fn vm(app: &AndroidApp) -> Result<JavaVM, Error> {
        let ptr = app.vm_as_ptr();
        if ptr.is_null() {
            return Err(Error("the activity has no Java VM".into()));
        }
        // SAFETY: android-activity returns the process's `JavaVM*` (from the NativeActivity it was
        // started by), non-null as checked, valid for the life of the process.
        unsafe { JavaVM::from_raw(ptr.cast()) }.map_err(|e| Error(format!("adopt the Java VM: {e}")))
    }

    /// The activity object, as a reference that is only used to make a global reference at once.
    pub fn activity(app: &AndroidApp) -> Result<JObject<'static>, Error> {
        let ptr = app.activity_as_ptr();
        if ptr.is_null() {
            return Err(Error("no activity".into()));
        }
        // SAFETY: android-activity returns its own global reference to the `Activity` (a `jobject`),
        // non-null as checked and valid while the app runs; the caller turns it into a global
        // reference of its own straight away and never deletes this one.
        Ok(unsafe { JObject::from_raw(ptr.cast()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_extensions_narrow_the_picker() {
        assert_eq!(PickRequest::mime_types_for(&["jpg", "JPEG", "png"]), vec!["image/jpeg", "image/png"]);
    }

    #[test]
    fn all_files_access_is_offered_only_when_the_manifest_declares_it() {
        // the GitHub build
        assert!(offers_all_files_access(Ok(true)));
        // the Play build, and an activity that couldn't answer: never ask
        assert!(!offers_all_files_access(Ok(false)));
        assert!(!offers_all_files_access(Err(Error("no activity".into()))));
    }

    #[test]
    fn an_unknown_extension_offers_every_file() {
        // a raw or preset file must never be greyed out
        assert!(PickRequest::mime_types_for(&["jpg", "cr3"]).is_empty());
        assert!(PickRequest::mime_types_for(&["lcpreset"]).is_empty());
    }

    #[test]
    fn keyboard_events_decode_and_hostile_ones_are_refused() {
        assert_eq!(ImeEvent::from_wire(0, Some("é".into()), 0, 0), Some(ImeEvent::Commit("é".into())));
        assert_eq!(ImeEvent::from_wire(1, None, 0, 0), Some(ImeEvent::Preedit(String::new())));
        assert_eq!(ImeEvent::from_wire(3, None, -5, i32::MAX), Some(ImeEvent::DeleteSurrounding { before: 0, after: i32::MAX as usize }));
        assert_eq!(ImeEvent::from_wire(4, None, 0, 0), Some(ImeEvent::Backspace));
        assert_eq!(ImeEvent::from_wire(4, None, 99, 0), None);
        assert_eq!(ImeEvent::from_wire(-1, None, 0, 0), None);
        assert_eq!(ImeEvent::from_wire(i32::MAX, None, 0, 0), None);
    }

    #[test]
    fn the_keyboard_queue_is_bounded_and_drains() {
        let _ = take_ime();
        let mut accepted = 0;
        for _ in 0..IME_QUEUE_MAX + 10 {
            if push_ime(ImeEvent::Enter) {
                accepted += 1;
            }
        }
        assert_eq!(accepted, IME_QUEUE_MAX);
        assert_eq!(take_ime().len(), IME_QUEUE_MAX);
        assert!(take_ime().is_empty());
    }

    #[test]
    fn answers_reach_their_request() {
        let mut a = Answers::default();
        let rx = a.wait(7);
        a.answer(7, Some(vec!["/x/a.jpg".into()]));
        assert_eq!(rx.try_recv().unwrap(), vec!["/x/a.jpg".to_string()]);
        assert!(a.take_unclaimed().is_empty());
    }

    #[test]
    fn a_failed_pick_ends_the_request_without_an_answer() {
        let mut a = Answers::default();
        let rx = a.wait(3);
        a.answer(3, None);
        assert!(matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
    }

    #[test]
    fn an_answer_after_a_restart_is_kept_for_the_app() {
        // the activity answers a request it didn't make with -(kind + 1)
        let mut a = Answers::default();
        a.answer(-(PickKind::Files.code() as i64) - 1, Some(vec!["/x/a.jpg".into()]));
        a.answer(-(PickKind::Folder.code() as i64) - 1, None);
        a.answer(i64::MIN, Some(vec![]));
        a.answer(-99, Some(vec![]));
        a.answer(42, Some(vec![]));
        let u = a.take_unclaimed();
        assert_eq!(u.len(), 2);
        assert_eq!(u[0], Unclaimed { kind: PickKind::Files, paths: Ok(vec!["/x/a.jpg".into()]) });
        assert_eq!(u[1].kind, PickKind::Folder);
        assert!(u[1].paths.is_err());
        assert!(a.take_unclaimed().is_empty());
    }

    #[test]
    fn camera_roll_answers_carry_the_photos_or_the_reason() {
        let mut r = RollAnswers::default();
        let ok = r.wait(1);
        let denied = r.wait(2);
        let broken = r.wait(3);
        r.answer(1, Some(vec!["/storage/emulated/0/DCIM/Camera/IMG_1.jpg".into()]), None);
        r.answer(2, None, Some("access to photos was refused".into()));
        r.answer(3, None, None);
        // an answer for a listing that is gone is dropped, not a panic
        r.answer(99, Some(vec![]), None);
        assert_eq!(ok.try_recv().unwrap(), Ok(vec!["/storage/emulated/0/DCIM/Camera/IMG_1.jpg".to_string()]));
        assert_eq!(denied.try_recv().unwrap(), Err("access to photos was refused".to_string()));
        assert!(broken.try_recv().unwrap().is_err());
        let forgotten = r.wait(4);
        r.forget(4);
        assert!(matches!(forgotten.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
    }

    #[test]
    fn back_presses_are_counted_bounded_and_taken_once() {
        take_back();
        for _ in 0..100 {
            push_back();
        }
        assert_eq!(take_back(), BACKS_MAX);
        assert_eq!(take_back(), 0);
    }

    #[test]
    fn insets_refuse_negative_edges() {
        set_insets([-5, 120, i32::MIN, 48]);
        assert_eq!(insets(), Some([0, 120, 0, 48]));
    }

    #[test]
    fn kinds_have_distinct_codes() {
        let codes = [PickKind::Files, PickKind::File, PickKind::Folder, PickKind::Save].map(PickKind::code);
        for (i, c) in codes.iter().enumerate() {
            assert!(!codes[i + 1..].contains(c));
        }
    }
}
