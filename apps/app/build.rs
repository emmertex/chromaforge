//! Windows only: embed the app icon and version info (VERSIONINFO) into the desktop executable, so
//! Explorer, the taskbar, the Start menu and Alt-Tab show the lynx, and the manifest that puts the
//! app on the Segment Heap ([`MANIFEST`]).
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `<PREFIX>_REQUIRE_WINRES=1` turns it
//! into an error (for release builds).

/// The application manifest: the Segment Heap (Windows 10 2004 and later; older versions ignore
/// it). On the default NT heap every large allocation and free takes the one process-wide heap lock,
/// and while Build Previews or an export ran, the render workers' image buffers kept it busy: the
/// window's thread spent ~45 % of its time waiting on that lock (frames of 50–300 ms, the slider
/// handle trailing the pointer). The Segment Heap has no such lock for large blocks.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <heapType xmlns="http://schemas.microsoft.com/SMI/2020/WindowsSettings">SegmentHeap</heapType>
    </windowsSettings>
  </application>
</assembly>
"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/app.ico");
    let require = lcc_brand::env_var("REQUIRE_WINRES");
    println!("cargo:rerun-if-env-changed={require}");
    for prefix in lcc_brand::LEGACY_ENV_PREFIXES {
        println!("cargo:rerun-if-env-changed={prefix}_REQUIRE_WINRES");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/app.ico")
        .set("ProductName", lcc_brand::DISPLAY_NAME)
        .set("FileDescription", &format!("{}: {}", lcc_brand::DISPLAY_NAME, lcc_brand::TAGLINE))
        .set("CompanyName", lcc_brand::VENDOR)
        .set("LegalCopyright", &format!("Copyright (c) {}. MIT AND Apache-2.0.", lcc_brand::VENDOR))
        .set("OriginalFilename", &format!("{}.exe", lcc_brand::BINARY))
        .set("InternalName", lcc_brand::BINARY)
        .set_manifest(MANIFEST);
    if let Err(e) = res.compile() {
        if lcc_brand::env_is_set("REQUIRE_WINRES") {
            panic!("embedding Windows resources failed: {e}");
        }
        println!("cargo:warning={}.exe built without icon/version resources: {e}", lcc_brand::BINARY);
    }
}
