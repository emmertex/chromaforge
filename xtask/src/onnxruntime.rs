//! `cargo xtask onnxruntime fetch --dest DIR [--platform P]`: the ONNX Runtime library and its WebGPU plug-in that
//! desktop packages ship beside the app (issue #491, `docs/onnxruntime.md`).
//!
//! Both come from Microsoft's own builds on PyPI (`onnxruntime`, `onnxruntime-ep-webgpu`; MIT, Dawn inside the
//! plug-in BSD-3-Clause, DXC on Windows NCSA): each wheel is pinned by URL and SHA-256, downloaded once into
//! `target/onnxruntime/`, and only the listed files are taken out of it (never the Python modules, never the preview
//! D3D12 Agility SDK the Windows plug-in wheel carries). The licence texts go next to the libraries.
//!
//! A platform with no pinned build (riscv64, FreeBSD) prints a note and succeeds: the app then runs its pure-Rust
//! runners, as it does whenever the library is missing.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The pinned ONNX Runtime version (keep in step with `lcc-onnxrt`'s `ort` API level, at least 1.24).
const ORT_VERSION: &str = "1.32.0";
/// The pinned WebGPU plug-in version (needs ONNX Runtime 1.24.4 or later).
const WEBGPU_VERSION: &str = "0.6.0";

/// One wheel and the files taken from it: (member in the wheel, file name written).
struct Wheel {
    url: &'static str,
    sha256: &'static str,
    files: &'static [(&'static str, &'static str)],
}

struct Platform {
    key: &'static str,
    runtime: Wheel,
    webgpu: Wheel,
}

const LICENCES: &[(&str, &str)] =
    &[("onnxruntime/LICENSE", "LICENSE-onnxruntime.txt"), ("onnxruntime/ThirdPartyNotices.txt", "ThirdPartyNotices-onnxruntime.txt")];

const PLATFORMS: &[Platform] = &[
    Platform {
        key: "windows-x86_64",
        runtime: Wheel {
            url: "https://files.pythonhosted.org/packages/6f/ee/a776ccc71ba54de3739c20c7c815d9695c55db1d08e2b3c621452676c23f/onnxruntime-1.32.0-cp313-cp313-win_amd64.whl",
            sha256: "c7f9aa98036ba2a47aa041a4a8c6d0a1f7bfe1bde44c131dcd15ce334f2f148d",
            files: &[
                ("onnxruntime/capi/onnxruntime.dll", "onnxruntime.dll"),
                ("onnxruntime/capi/onnxruntime_providers_shared.dll", "onnxruntime_providers_shared.dll"),
            ],
        },
        webgpu: Wheel {
            url: "https://files.pythonhosted.org/packages/0e/12/25312b2220bdf969a6cdc937e0faad9d6456cadbd602e88bced36f1011af/onnxruntime_ep_webgpu-0.6.0-py3-none-win_amd64.whl",
            sha256: "00853e242f74f4b9072e1620bce9222547c2f988265eb674b5749680c8d62fb9",
            files: &[
                ("onnxruntime_ep_webgpu/onnxruntime_providers_webgpu.dll", "onnxruntime_providers_webgpu.dll"),
                ("onnxruntime_ep_webgpu/dxcompiler.dll", "dxcompiler.dll"),
            ],
        },
    },
    Platform {
        key: "windows-aarch64",
        runtime: Wheel {
            url: "https://files.pythonhosted.org/packages/9b/08/9474c3f1a098da173cfd91047e1c6badaf0cfb4f5bd5598c54581a24c6d2/onnxruntime-1.32.0-cp313-cp313-win_arm64.whl",
            sha256: "29e4b0fdd3bea834ace358bd8c48641266ec327f730363ab0ab5d0ca574d38fe",
            files: &[
                ("onnxruntime/capi/onnxruntime.dll", "onnxruntime.dll"),
                ("onnxruntime/capi/onnxruntime_providers_shared.dll", "onnxruntime_providers_shared.dll"),
            ],
        },
        webgpu: Wheel {
            url: "https://files.pythonhosted.org/packages/22/bf/4633c7635bdc690ac881ac6a7ae56975721753aad3742d2b5a750d77575f/onnxruntime_ep_webgpu-0.6.0-py3-none-win_arm64.whl",
            sha256: "f7a32f04edd5cb9d2b43f60f64d6f186ac93f32029b066f243c04b6e4bac24a3",
            files: &[
                ("onnxruntime_ep_webgpu/onnxruntime_providers_webgpu.dll", "onnxruntime_providers_webgpu.dll"),
                ("onnxruntime_ep_webgpu/dxcompiler.dll", "dxcompiler.dll"),
            ],
        },
    },
    Platform {
        key: "linux-x86_64",
        runtime: Wheel {
            url: "https://files.pythonhosted.org/packages/92/74/1f7c51522822e93aa91f633ebcd75688e2deebeca5901b2e6da8a8c5ad7c/onnxruntime-1.32.0-cp313-cp313-manylinux_2_28_x86_64.whl",
            sha256: "02ea12a28c069ea8510093d95a7f6ff8126bfd9fc5c0d3381c03b80fb2b424ba",
            files: &[("onnxruntime/capi/libonnxruntime.so.1.32.0", "libonnxruntime.so")],
        },
        webgpu: Wheel {
            url: "https://files.pythonhosted.org/packages/98/57/656e7b4071d754674fc9f749bcd9dc0229efc685b084ca71932a34d7cad4/onnxruntime_ep_webgpu-0.6.0-py3-none-manylinux_2_28_x86_64.whl",
            sha256: "791d991f2779ae23cdd273d4689413e798080cfa4b736012ec53da8cd14a7c79",
            files: &[("onnxruntime_ep_webgpu/libonnxruntime_providers_webgpu.so", "libonnxruntime_providers_webgpu.so")],
        },
    },
    Platform {
        key: "linux-aarch64",
        runtime: Wheel {
            url: "https://files.pythonhosted.org/packages/96/c7/26be6be3ebe42597a521645a7bb18701ce0716af87af0cd2c9281d2e84a5/onnxruntime-1.32.0-cp313-cp313-manylinux_2_28_aarch64.whl",
            sha256: "54d02baff2df38777b4c3cc7af326406fe8b8a48f54720f4c446d2d327920a79",
            files: &[("onnxruntime/capi/libonnxruntime.so.1.32.0", "libonnxruntime.so")],
        },
        webgpu: Wheel {
            url: "https://files.pythonhosted.org/packages/0b/03/9386f307447c5805774f8a870bd1eb0e853db0848a3213d419e17f6f113c/onnxruntime_ep_webgpu-0.6.0-py3-none-manylinux_2_28_aarch64.whl",
            sha256: "974d8d2cdc280bdb962ef19a306e5bd98ed17bfdfd40c8f540010fb48283740a",
            files: &[("onnxruntime_ep_webgpu/libonnxruntime_providers_webgpu.so", "libonnxruntime_providers_webgpu.so")],
        },
    },
    Platform {
        key: "macos-aarch64",
        runtime: Wheel {
            url: "https://files.pythonhosted.org/packages/f1/0c/97eb4ac42266545d7630e8231f8cc969f5c3f144d2ee6cb568ef45f81360/onnxruntime-1.32.0-cp313-cp313-macosx_14_0_arm64.whl",
            sha256: "27bf5dad46355a8a435c9c4e1556464d99ca834ea44e0f452909712068b8821b",
            files: &[("onnxruntime/capi/libonnxruntime.1.32.0.dylib", "libonnxruntime.dylib")],
        },
        webgpu: Wheel {
            url: "https://files.pythonhosted.org/packages/b7/36/87398c782af00f7f49efbff0b2e17f650b22788939e9b440a3e576ff73db/onnxruntime_ep_webgpu-0.6.0-py3-none-macosx_14_0_universal2.whl",
            sha256: "a4cb7f57f74dcebf6b420413dbc6c7624fa8153ff546dbff2a21109935274cc5",
            files: &[("onnxruntime_ep_webgpu/libonnxruntime_providers_webgpu.dylib", "libonnxruntime_providers_webgpu.dylib")],
        },
    },
];

/// This machine's platform key (`linux-x86_64`, …).
fn host() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let usage =
        "usage: cargo xtask onnxruntime fetch --dest DIR [--platform windows-x86_64|windows-aarch64|linux-x86_64|linux-aarch64|macos-aarch64]";
    let mut dest = None;
    let mut platform = host();
    let mut it = args.iter();
    match it.next() {
        Some(&"fetch") => {}
        _ => return Err(usage.into()),
    }
    while let Some(a) = it.next() {
        match *a {
            "--dest" => dest = it.next().map(PathBuf::from),
            "--platform" => platform = it.next().ok_or(usage)?.to_string(),
            _ => return Err(usage.into()),
        }
    }
    let dest = dest.ok_or(usage)?;
    let Some(p) = PLATFORMS.iter().find(|p| p.key == platform) else {
        println!("onnxruntime: no pinned ONNX Runtime for {platform}; the package ships without it (the app uses its own runners)");
        return Ok(());
    };
    let cache = crate::dist::target_dir(root).join("onnxruntime");
    std::fs::create_dir_all(&cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    std::fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let runtime = download(&cache, &p.runtime)?;
    extract(&runtime, p.runtime.files, &dest)?;
    extract(&runtime, LICENCES, &dest)?;
    let webgpu = download(&cache, &p.webgpu)?;
    extract(&webgpu, p.webgpu.files, &dest)?;
    println!("onnxruntime: ONNX Runtime {ORT_VERSION} and its WebGPU plug-in {WEBGPU_VERSION} for {platform} in {}", dest.display());
    Ok(())
}

fn sha256(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(buf.get(..n)?);
    }
    Some(format!("{:x}", h.finalize()))
}

/// The wheel, from the cache or downloaded, checked against its pin.
fn download(cache: &Path, w: &Wheel) -> Result<PathBuf, String> {
    let name = w.url.rsplit('/').next().unwrap_or("wheel.whl");
    let path = cache.join(name);
    if sha256(&path).as_deref() == Some(w.sha256) {
        return Ok(path);
    }
    let partial = path.with_extension("download");
    let status = Command::new("curl").args(["-sSfL", "--retry", "3", "-o"]).arg(&partial).arg(w.url).status().map_err(|e| format!("curl: {e}"))?;
    if !status.success() {
        return Err(format!("download of {name} failed"));
    }
    if sha256(&partial).as_deref() != Some(w.sha256) {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("{name}: the download doesn't match its pinned sha256"));
    }
    std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Take `files` out of the wheel at `wheel` into `dest`.
fn extract(wheel: &Path, files: &[(&str, &str)], dest: &Path) -> Result<(), String> {
    let f = std::fs::File::open(wheel).map_err(|e| format!("{}: {e}", wheel.display()))?;
    let mut zip = zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", wheel.display()))?;
    for (member, out) in files {
        let mut entry = zip.by_name(member).map_err(|e| format!("{}: {member}: {e}", wheel.display()))?;
        let to = dest.join(out);
        let mut file = std::fs::File::create(&to).map_err(|e| format!("{}: {e}", to.display()))?;
        std::io::copy(&mut entry, &mut file).map_err(|e| format!("{}: {e}", to.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if out.ends_with(".so") || out.ends_with(".dylib") {
                let _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_are_well_formed() {
        for p in PLATFORMS {
            for w in [&p.runtime, &p.webgpu] {
                assert!(w.url.starts_with("https://files.pythonhosted.org/"), "{}", w.url);
                assert_eq!(w.sha256.len(), 64);
                assert!(w.sha256.chars().all(|c| c.is_ascii_hexdigit()));
                assert!(!w.files.is_empty());
            }
            assert!(p.runtime.url.contains(ORT_VERSION));
            assert!(p.webgpu.url.contains(WEBGPU_VERSION));
            // never the preview D3D12 Agility SDK
            assert!(p.webgpu.files.iter().all(|(m, _)| !m.contains("D3D12")));
        }
    }
}
