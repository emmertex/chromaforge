//! The catalog's steps of the desktop app's start and exit (fork-owned; `main.rs` calls each once).

use lcc_engine::Session;
use lcc_ui_egui::LccApp;

/// The catalog's backup schedule (Catalog Settings): checked on exit, like Lightroom.
pub fn backup_if_due(session: &mut Session) {
    match session.backup_catalog_if_due() {
        Ok(Some(dir)) => log::info!("catalog backed up to {}", dir.display()),
        Ok(None) => {}
        Err(e) => log::error!("catalog backup failed: {e}"),
    }
}

/// `recent-catalogs.json` in the settings folder of `config::config_dir` (beside `ui.json`, so
/// `<PREFIX>_CONFIG_DIR` and portable mode move it too): `None` when no list is kept
/// (`<PREFIX>_NO_PREFS`, or an empty `<PREFIX>_CONFIG_DIR`).
pub fn recent_file() -> Option<std::path::PathBuf> {
    if lcc_brand::env_is_set("NO_PREFS") {
        return None;
    }
    lcc_engine::config::config_dir().map(|d| lcc_engine::catalog::library::RecentCatalogs::file_in(&d))
}

/// The catalog chosen to open at startup (the chooser's default), if it still exists.
pub fn startup_catalog() -> Option<std::path::PathBuf> {
    let file = recent_file().filter(|_| !lcc_brand::env_is_set("LIBRARY"))?;
    let d = lcc_engine::catalog::library::RecentCatalogs::load(&file).default?;
    lcc_engine::catalog::library::resolve(&d).ok()
}

/// File ▸ Open Recent Catalog and the startup chooser (not for throwaway sessions).
pub fn remember_recent(app: &mut LccApp, in_memory: bool) {
    if !in_memory && let Some(file) = recent_file() {
        app.catalog_ui.recent_file = Some(file);
        if let Some(dir) = app.session.catalog_dir().map(std::path::Path::to_path_buf) {
            app.catalog_ui.touch(&dir);
        }
    }
}

/// What the app opens at startup.
#[derive(Debug, PartialEq)]
pub enum Startup {
    /// This library (it may still fail to open: the problem window then says why).
    Open(std::path::PathBuf),
    /// No catalog of this app to open yet (a first start, also right after the settings of a
    /// previous name were copied): the welcome dialog asks where to create one, or which to open.
    /// `foreign`: libraries of another app that would have been opened, named in the dialog.
    Welcome { foreign: Vec<std::path::PathBuf> },
}

/// A library folder holding another app's catalog (e.g. the upstream app's): never opened at
/// startup as this app's own.
pub fn is_foreign(dir: &std::path::Path) -> bool {
    lcc_engine::catalog::library::resolve(dir).is_ok_and(|d| lcc_engine::catalog::library::foreign_format(&d).is_some())
}

/// Decide what to open at startup. `explicit`: `--library` or `<PREFIX>_LIBRARY` (always opened,
/// or created). `remembered`: the catalogs the settings name (the startup catalog of the recent
/// list, then the library of the preferences). `default`: the default library folder, opened only
/// when it already holds a catalog of this app (never created silently).
pub fn decide(explicit: Option<std::path::PathBuf>, remembered: &[Option<std::path::PathBuf>], default: Option<std::path::PathBuf>) -> Startup {
    if let Some(d) = explicit {
        return Startup::Open(d);
    }
    let mut foreign: Vec<std::path::PathBuf> = Vec::new();
    for d in remembered.iter().flatten() {
        if !is_foreign(d) {
            return Startup::Open(d.clone());
        }
        if !foreign.contains(d) {
            foreign.push(d.clone());
        }
    }
    if let Some(d) = default {
        if is_foreign(&d) {
            if !foreign.contains(&d) {
                foreign.push(d);
            }
        } else if d.is_dir() && lcc_engine::catalog::library::resolve(&d).is_ok() {
            return Startup::Open(d);
        }
    }
    Startup::Welcome { foreign }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lcc-firstrun-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A library of the upstream app: its own v3 snapshot format.
    fn upstream_library(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("catalog.snap"), "{\"format\":\"upstream-catalog\",\"version\":6,\"seq\":0,\"catalog\":{}}\n").unwrap();
        std::fs::write(dir.join("catalog.log"), "").unwrap();
    }

    /// A first start: nothing remembered, no library at the default location → the welcome
    /// dialog, not a library created silently (or an error).
    #[test]
    fn first_start_shows_the_welcome() {
        let d = scratch("first");
        let default = d.join("Pictures").join("Lib");
        assert_eq!(decide(None, &[None, None], Some(default.clone())), Startup::Welcome { foreign: vec![] });
        assert!(!default.exists(), "nothing is created before the user chooses");
        // an empty folder at the default location is not a catalog either
        std::fs::create_dir_all(&default).unwrap();
        assert_eq!(decide(None, &[None, None], Some(default.clone())), Startup::Welcome { foreign: vec![] });
        // no home folder at all
        assert_eq!(decide(None, &[None, None], None), Startup::Welcome { foreign: vec![] });
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Later starts open this app's catalog: remembered first, else the default one.
    #[test]
    fn later_starts_open_the_catalog() {
        let d = scratch("later");
        let entry = lcc_engine::catalog::library::create(&d, "Default").unwrap();
        let default = entry.parent().unwrap().to_path_buf();
        assert_eq!(decide(None, &[None, None], Some(default.clone())), Startup::Open(default.clone()));
        let other = d.join("Other");
        assert_eq!(decide(None, &[Some(other.clone()), None], Some(default.clone())), Startup::Open(other.clone()));
        // --library / <PREFIX>_LIBRARY win (and may name a folder to create)
        let explicit = d.join("New");
        assert_eq!(decide(Some(explicit.clone()), &[Some(other)], Some(default)), Startup::Open(explicit));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The maintainer's report: on a first start the settings folder of a previous name (the
    /// upstream app's) was copied, its library setting (or the previous default library name)
    /// pointed at the upstream library, and the app opened it as its own and called it corrupt.
    /// Now it is named in the welcome dialog instead, and left untouched.
    #[test]
    fn migrated_legacy_settings_do_not_open_the_old_library() {
        let d = scratch("legacy");
        let upstream_lib = d.join("Pictures").join("Upstream Library");
        upstream_library(&upstream_lib);
        let legacy = d.join("config").join("upstream");
        std::fs::create_dir_all(&legacy).unwrap();
        let ui = serde_json::json!({"settings": {"library_path": upstream_lib}});
        std::fs::write(legacy.join("ui.json"), ui.to_string()).unwrap();
        let current = d.join("config").join("current");
        assert_eq!(lcc_brand::migrate_settings_from(&current, std::slice::from_ref(&legacy)).unwrap(), Some(legacy));
        let copied: serde_json::Value = serde_json::from_slice(&std::fs::read(current.join("ui.json")).unwrap()).unwrap();
        let remembered = copied["settings"]["library_path"].as_str().map(PathBuf::from);
        assert_eq!(remembered.as_deref(), Some(upstream_lib.as_path()));
        let got = decide(None, &[None, remembered], Some(upstream_lib.clone()));
        assert_eq!(got, Startup::Welcome { foreign: vec![upstream_lib.clone()] });
        assert_eq!(std::fs::read_dir(&upstream_lib).unwrap().count(), 2, "the upstream library is untouched");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Issue #163: the recent list is a settings file like `ui.json`. It used to live in the
    /// platform settings folder whatever `<PREFIX>_CONFIG_DIR` said, so a scratch run listed its
    /// library in the user's real list, and opened the real list's startup catalog. Each case runs
    /// in a child (this test binary again) with a scratch home, so the platform folder is scratch too.
    #[test]
    fn recent_catalogs_follow_the_config_dir() {
        if let Some(case) = lcc_brand::env("TEST_RECENT_CASE") {
            recent_catalogs_child(&case);
            return;
        }
        let d = scratch("recent");
        let a = lcc_engine::catalog::library::create(&d, "A").unwrap();
        let b = lcc_engine::catalog::library::create(&d, "B").unwrap();
        // the user's real list (in the platform folder) names A as the startup catalog
        let home = d.join("home");
        let mut platform = home.join(".config").join(lcc_brand::settings_dir_name(lcc_brand::SETTINGS_DIR));
        if cfg!(target_os = "macos") {
            platform = home.join("Library/Application Support").join(lcc_brand::SETTINGS_DIR);
        } else if cfg!(windows) {
            platform = home.join(lcc_brand::SETTINGS_DIR);
        }
        let real = lcc_engine::catalog::library::RecentCatalogs { paths: vec![a.clone()], default: Some(a.clone()), prompt_at_startup: false };
        real.save(&platform.join("recent-catalogs.json")).unwrap();
        for case in ["set", "empty", "unset"] {
            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            cmd.args(["--exact", "catalog_hooks::tests::recent_catalogs_follow_the_config_dir", "--nocapture"]);
            for prefix in std::iter::once(lcc_brand::ENV_PREFIX).chain(lcc_brand::LEGACY_ENV_PREFIXES.iter().copied()) {
                for name in ["CONFIG_DIR", "NO_PREFS", "LIBRARY"] {
                    cmd.env_remove(format!("{prefix}_{name}"));
                }
            }
            match case {
                "set" => cmd.env(lcc_brand::env_var("CONFIG_DIR"), d.join("cfg")),
                "empty" => cmd.env(lcc_brand::env_var("CONFIG_DIR"), ""),
                _ => &mut cmd,
            };
            let out = cmd
                .env("HOME", &home)
                .env("APPDATA", &home)
                .env_remove("XDG_CONFIG_HOME")
                .env(lcc_brand::env_var("TEST_RECENT_CASE"), case)
                .env(lcc_brand::env_var("TEST_RECENT_ROOT"), &d)
                .env(lcc_brand::env_var("TEST_RECENT_B"), &b)
                .output()
                .unwrap();
            let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert!(out.status.success(), "case {case}: {all}");
            assert!(all.contains(&format!("recent case {case} ok")), "case {case}: the child didn't run the test: {all}");
        }
        assert_eq!(
            lcc_engine::catalog::library::RecentCatalogs::load(&platform.join("recent-catalogs.json")),
            real,
            "no run with its own (or no) settings folder changes the platform folder's list"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// One case of [`recent_catalogs_follow_the_config_dir`], in the child.
    fn recent_catalogs_child(case: &str) {
        let root = PathBuf::from(lcc_brand::env_os("TEST_RECENT_ROOT").unwrap());
        let b = PathBuf::from(lcc_brand::env_os("TEST_RECENT_B").unwrap());
        let platform = lcc_brand::settings_dir().unwrap();
        assert!(platform.starts_with(&root), "the platform folder must be scratch here, not {}", platform.display());
        let a = lcc_engine::catalog::library::RecentCatalogs::load(&platform.join("recent-catalogs.json")).default.unwrap();
        let a = lcc_engine::catalog::library::resolve(&a).unwrap();
        let cfg = root.join("cfg");
        let mut app = {
            let mut s = Session::new().with_fs();
            s.open_library(root.join(format!("C-{case}")), false).unwrap();
            LccApp::new(s, lcc_ui_egui::Services::default())
        };
        match case {
            // AC-1, AC-2: the list lives in the config dir; the platform folder's isn't read
            "set" => {
                assert_eq!(recent_file(), Some(cfg.join("recent-catalogs.json")), "the same folder as ui.json");
                assert_eq!(startup_catalog(), None, "the platform folder's startup catalog is not opened");
                let own = lcc_engine::catalog::library::RecentCatalogs { paths: vec![b.clone()], default: Some(b.clone()), prompt_at_startup: false };
                own.save(&cfg.join("recent-catalogs.json")).unwrap();
                assert_eq!(startup_catalog(), Some(lcc_engine::catalog::library::resolve(&b).unwrap()));
                remember_recent(&mut app, false);
                let saved = lcc_engine::catalog::library::RecentCatalogs::load(&cfg.join("recent-catalogs.json"));
                assert!(saved.paths.first().is_some_and(|p| p.starts_with(root.join("C-set"))), "{saved:?}");
            }
            // AC-3: no settings folder, no list
            "empty" => {
                assert_eq!(recent_file(), None);
                assert_eq!(startup_catalog(), None);
                remember_recent(&mut app, false);
                assert_eq!(app.catalog_ui.recent_file, None);
                for list in [cfg.join("recent-catalogs.json"), platform.join("recent-catalogs.json")] {
                    let r = lcc_engine::catalog::library::RecentCatalogs::load(&list);
                    assert!(!r.paths.iter().any(|p| p.starts_with(root.join("C-empty"))), "{} lists it: {r:?}", list.display());
                }
            }
            // AC-5: without the override the list stays in the platform folder, and still loads
            _ => {
                assert_eq!(recent_file(), Some(platform.join("recent-catalogs.json")));
                assert_eq!(startup_catalog(), Some(a));
            }
        }
        println!("recent case {case} ok");
    }
}
