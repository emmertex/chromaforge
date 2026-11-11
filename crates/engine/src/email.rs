//! File ▸ Email Photo…: handing exported files to the system's mail program.
//!
//! The files are made first (a small JPEG export into a temporary folder); this module only
//! decides how each platform's mail client is asked to start a new message with them attached,
//! and does it through [`Desktop`] (so headless sessions and tests record the request instead of
//! opening a mail window):
//!
//! - Linux and other Unix: `xdg-email --subject … --attach file …` (one `--attach` per file);
//! - macOS: `/usr/bin/open -a Mail file …` (Mail opens a new message holding the files);
//! - Windows: Outlook's command line (`OUTLOOK.EXE /c ipm.note /a file`) when Outlook is installed
//!   and there is one file (`/a` takes a single attachment); otherwise there is no mail client this
//!   can drive, and the folder is shown instead.
//!
//! No mail client found is not a crash and not lost work: [`Outcome::Folder`] says what to do, the
//! files stay in their folder, and the folder is shown in the file manager.
//!
//! Sources: `xdg-email(1)` man page (xdg-utils), `open(1)` man page, Microsoft's documented
//! Outlook command-line switches. Own design.

use std::path::{Path, PathBuf};

use crate::desktop::Desktop;

/// The platform a hand-off is planned for (a parameter so every platform's plan is testable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Unix,
    MacOs,
    Windows,
}

impl Os {
    /// The platform this build runs on.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Unix
        }
    }
}

/// What to do with the files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Start this command line (program, then arguments).
    Launch(Vec<String>),
    /// No mail program can be started with attachments: show the folder; `why` says what is missing.
    Folder { why: String },
}

/// What came of the hand-off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A mail program was started with the files attached.
    Opened { program: String },
    /// The files are in `dir`, shown in the file manager (when it could be); `why` is the reason
    /// no mail was started.
    Folder { dir: PathBuf, why: String },
}

/// Most files attached to one message (the command line stays well under any platform's limit).
pub const MAX_FILES: usize = 100;

/// Where Outlook may be, per Windows installation kind (Microsoft 365 / Office click-to-run,
/// then the older MSI layouts).
const OUTLOOK_PATHS: &[&str] = &[
    "C:/Program Files/Microsoft Office/root/Office16/OUTLOOK.EXE",
    "C:/Program Files (x86)/Microsoft Office/root/Office16/OUTLOOK.EXE",
    "C:/Program Files/Microsoft Office/Office16/OUTLOOK.EXE",
    "C:/Program Files (x86)/Microsoft Office/Office16/OUTLOOK.EXE",
    "C:/Program Files/Microsoft Office/Office15/OUTLOOK.EXE",
    "C:/Program Files (x86)/Microsoft Office/Office15/OUTLOOK.EXE",
    "C:/Program Files/Microsoft Office/Office14/OUTLOOK.EXE",
    "C:/Program Files (x86)/Microsoft Office/Office14/OUTLOOK.EXE",
];

/// How to start a new message with `files` attached on `os`. `find(name)` looks a program up
/// (`xdg-email` on the search path; for Windows an Outlook path from [`OUTLOOK_PATHS`]).
pub fn plan(os: Os, files: &[PathBuf], subject: &str, find: &dyn Fn(&str) -> Option<PathBuf>) -> Plan {
    if files.is_empty() {
        return Plan::Folder { why: "there is nothing to attach".into() };
    }
    if files.len() > MAX_FILES {
        return Plan::Folder { why: format!("a message takes at most {MAX_FILES} attachments") };
    }
    let s = |p: &Path| p.to_string_lossy().into_owned();
    match os {
        Os::Unix => match find("xdg-email") {
            Some(prog) => {
                let mut line = vec![s(&prog)];
                if !subject.is_empty() {
                    line.push("--subject".into());
                    line.push(subject.to_string());
                }
                for f in files {
                    line.push("--attach".into());
                    line.push(s(f));
                }
                Plan::Launch(line)
            }
            None => Plan::Folder { why: "no mail program was found (install xdg-utils, which provides xdg-email)".into() },
        },
        Os::MacOs => {
            let mut line = vec!["/usr/bin/open".to_string(), "-a".into(), "Mail".into()];
            line.extend(files.iter().map(|f| s(f)));
            Plan::Launch(line)
        }
        Os::Windows => {
            let outlook = OUTLOOK_PATHS.iter().find_map(|p| find(p));
            match (outlook, files) {
                (Some(prog), [only]) => Plan::Launch(vec![s(&prog), "/c".into(), "ipm.note".into(), "/a".into(), s(only)]),
                (Some(_), _) => {
                    Plan::Folder { why: "Outlook attaches one file at a time from the command line: attach the files from the folder".into() }
                }
                (None, _) => Plan::Folder { why: "Outlook was not found: attach the files from the folder to a new message".into() },
            }
        }
    }
}

/// `name` on the search path (`PATH`), or the file itself when `name` is a path that exists.
pub fn find_program(name: &str) -> Option<PathBuf> {
    let direct = Path::new(name);
    if direct.components().count() > 1 {
        return direct.is_file().then(|| direct.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Hand `files` (all in one folder) to the mail program of `os`; when none can be started, show
/// their folder. `Err` only when the files are not in a folder to show.
pub fn hand_off(desktop: &dyn Desktop, os: Os, files: &[PathBuf], subject: &str, find: &dyn Fn(&str) -> Option<PathBuf>) -> Result<Outcome, String> {
    let dir = files.first().and_then(|f| f.parent()).map(Path::to_path_buf).ok_or("there are no files to send")?;
    let show = |why: String| {
        // showing the folder is a courtesy: it failing leaves the reason and the folder in the message
        if let Some(first) = files.first() {
            let _ = desktop.reveal(first);
        }
        Outcome::Folder { dir: dir.clone(), why }
    };
    Ok(match plan(os, files, subject, find) {
        Plan::Launch(line) => match desktop.launch(&line) {
            Ok(()) => Outcome::Opened { program: line.first().cloned().unwrap_or_default() },
            Err(e) => show(format!("the mail program could not be started ({e})")),
        },
        Plan::Folder { why } => show(why),
    })
}

/// A fresh folder under the system temp folder for the files of one message (`<prefix>-email-…`).
pub fn temp_dir(prefix: &str) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("{prefix}-email-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not make a temporary folder for the files: {e}"))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::{Recorder, Request};

    fn files() -> Vec<PathBuf> {
        vec![PathBuf::from("/tmp/m/a.jpg"), PathBuf::from("/tmp/m/b b.jpg")]
    }

    fn only(name: &'static str) -> impl Fn(&str) -> Option<PathBuf> {
        move |n| (n == name).then(|| PathBuf::from(format!("/usr/bin/{n}")))
    }

    #[test]
    fn unix_attaches_each_file_through_xdg_email() {
        let Plan::Launch(line) = plan(Os::Unix, &files(), "Photos", &only("xdg-email")) else { panic!("no launch") };
        assert_eq!(line[0], "/usr/bin/xdg-email");
        let attached: Vec<&str> = line.windows(2).filter(|w| w[0] == "--attach").map(|w| w[1].as_str()).collect();
        assert_eq!(attached, ["/tmp/m/a.jpg", "/tmp/m/b b.jpg"], "{line:?}");
    }

    #[test]
    fn a_missing_mail_program_shows_the_folder_instead() {
        let rec = Recorder::default();
        let out = hand_off(&rec, Os::Unix, &files(), "Photos", &|_| None).unwrap();
        let Outcome::Folder { dir, why } = out else { panic!("expected the folder") };
        assert_eq!(dir, PathBuf::from("/tmp/m"));
        assert!(why.contains("xdg-email"), "{why}");
        assert_eq!(rec.requests(), [Request::Reveal(PathBuf::from("/tmp/m/a.jpg"))]);
    }

    #[test]
    fn a_launched_mail_program_is_recorded() {
        let rec = Recorder::default();
        let out = hand_off(&rec, Os::MacOs, &files(), "", &|_| None).unwrap();
        assert_eq!(out, Outcome::Opened { program: "/usr/bin/open".into() });
        assert_eq!(
            rec.requests(),
            [Request::Launch(vec!["/usr/bin/open".into(), "-a".into(), "Mail".into(), "/tmp/m/a.jpg".into(), "/tmp/m/b b.jpg".into()])]
        );
    }

    #[test]
    fn windows_uses_outlook_for_one_file_and_the_folder_for_several() {
        let outlook = |n: &str| (n == OUTLOOK_PATHS[0]).then(|| PathBuf::from(n));
        let one = &files()[..1];
        let Plan::Launch(line) = plan(Os::Windows, one, "", &outlook) else { panic!("no launch") };
        assert_eq!(&line[1..], ["/c", "ipm.note", "/a", "/tmp/m/a.jpg"]);
        assert!(matches!(plan(Os::Windows, &files(), "", &outlook), Plan::Folder { .. }));
        assert!(matches!(plan(Os::Windows, one, "", &|_| None), Plan::Folder { .. }));
    }

    #[test]
    fn nothing_or_too_much_to_attach_is_a_folder_plan_not_a_crash() {
        assert!(matches!(plan(Os::Unix, &[], "", &only("xdg-email")), Plan::Folder { .. }));
        let many: Vec<PathBuf> = (0..=MAX_FILES).map(|i| PathBuf::from(format!("/tmp/m/{i}.jpg"))).collect();
        assert!(matches!(plan(Os::Unix, &many, "", &only("xdg-email")), Plan::Folder { .. }));
        assert!(hand_off(&Recorder::default(), Os::Unix, &[], "", &|_| None).is_err());
    }

    #[test]
    fn find_program_ignores_missing_names_and_directories() {
        assert!(find_program("definitely-not-a-program-xyz").is_none());
        assert!(find_program("/definitely/not/here").is_none());
        assert!(find_program(&std::env::temp_dir().to_string_lossy()).is_none());
    }
}
