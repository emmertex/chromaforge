//! Whether Windows Installer owns the running program: an MSI install is updated by running the
//! new MSI; any other copy on Windows was unpacked from the portable zip and updates itself in
//! place ([`crate::install`]).
//!
//! Windows Installer is asked where every product that has the app's main component keeps its key
//! file ([`lcc_winmsi::component_paths`]), and the running program is compared with each. The MSI
//! registers that component on install and removes it on uninstall, so the answer is Windows' own
//! record: nothing extra is installed to mark an MSI copy, a zip copy can't be taken for one even
//! when the MSI is installed elsewhere, and an MSI built from the same template (upstream's, which
//! shares the component's GUID) can't hide this one.

use std::path::Path;

/// The `Guid` of the MSI component whose key path is the app's executable
/// (`packaging/windows/app.wxs.in`, `<Component Id="LccApp">`). It has not changed in any release,
/// and must not: Windows Installer identifies the component by it.
pub const APP_COMPONENT: &str = "{8A1D3A39-F7C1-47B1-97AE-40C80BF82F4A}";

/// Whether the MSI installed `exe` (this program's path). `Ok(false)` off Windows and for any copy
/// the MSI did not install; `Err` when Windows Installer could not say (a broken install).
pub fn installed_by_msi(exe: &Path) -> Result<bool, String> {
    match lcc_winmsi::component_paths(APP_COMPONENT) {
        Ok(installed) => Ok(installed.iter().any(|p| same_file(p, exe))),
        Err(lcc_winmsi::Error::Unsupported) => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// Whether `a` and `b` name one file. Windows paths compare without case; the canonical form
/// resolves `..`, short (8.3) names and links when both exist.
fn same_file(a: &Path, b: &Path) -> bool {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (a, b) = (canonical(a), canonical(b));
    if cfg!(windows) { a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()) } else { a == b }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GUID asked about is the one the MSI is built with.
    #[test]
    fn the_component_is_the_one_the_msi_installs() {
        let wxs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/windows/app.wxs.in");
        let text = std::fs::read_to_string(&wxs).unwrap();
        let guid = APP_COMPONENT.trim_matches(['{', '}']);
        let component =
            text.lines().find(|l| l.contains(&format!("Guid=\"{guid}\""))).unwrap_or_else(|| panic!("{guid} is not in {}", wxs.display()));
        assert!(component.contains("<Component "), "{component}");
        // its key path is the program
        let after = text.split(&format!("Guid=\"{guid}\"")).nth(1).unwrap();
        let file = after.lines().nth(1).unwrap();
        assert!(file.contains("{{binary}}.exe") && file.contains("KeyPath=\"yes\""), "{file}");
    }

    #[test]
    fn this_test_program_was_not_installed_by_the_msi() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(installed_by_msi(&exe), Ok(false));
    }

    #[test]
    fn one_file_by_two_spellings_is_the_same_file() {
        let exe = std::env::current_exe().unwrap();
        let dir = exe.parent().unwrap();
        let roundabout = dir.join("..").join(dir.file_name().unwrap()).join(exe.file_name().unwrap());
        assert!(same_file(&exe, &roundabout));
        if cfg!(windows) {
            let upper = std::path::PathBuf::from(exe.to_string_lossy().to_uppercase());
            assert!(same_file(&exe, &upper));
        }
        assert!(!same_file(&exe, &dir.join("not-this-program.exe")));
    }
}
