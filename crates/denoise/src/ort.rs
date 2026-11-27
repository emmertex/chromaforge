//! A denoise model on ONNX Runtime ([`lcc_onnxrt`]): the same `.onnx` file the pure-Rust runners read, run by ONNX
//! Runtime on the graphics card (its WebGPU plug-in, Core ML, CUDA) or on its CPU kernels.
//!
//! It is one more [`TileRunner`]; the engine checks its answer on a check tile against the pure-Rust CPU runner, the
//! reference, before a photo uses it, and runs any tile it gets wrong on the CPU. In a build without ONNX Runtime, or
//! without the library, [`OrtRunner::load`] says why and the other runners are used.

use std::path::Path;

use lcc_onnxrt::{Session, Tensor, Want};

use crate::manifest::DenoiserManifest;
use crate::run::{Error, TileRunner};

/// A denoise model loaded into ONNX Runtime.
pub struct OrtRunner {
    session: Session,
    /// Planes in, cells across a tile in, and values a tile's answer has.
    channels: usize,
    tile: usize,
    out_len: usize,
}

impl OrtRunner {
    /// Load the model at `path`, described by `manifest`, on `want`.
    pub fn load(path: &Path, manifest: &DenoiserManifest, want: Want) -> Result<OrtRunner, Error> {
        crate::manifest::validate(manifest).map_err(|e| Error::Load(e.to_string()))?;
        let tile = manifest.tile as usize;
        let channels = manifest.domain.in_channels();
        let out_side = manifest.domain.scale().checked_mul(tile).ok_or_else(|| Error::Load("the tile is too large".into()))?;
        let out_len = out_side.checked_mul(out_side).and_then(|n| n.checked_mul(3)).ok_or_else(|| Error::Load("the tile is too large".into()))?;
        let session = Session::load(path, want).map_err(|e| Error::Load(e.to_string()))?;
        if session.inputs().len() != 1 || session.outputs().len() != 1 {
            return Err(Error::Load("the model does not have one input and one output".into()));
        }
        Ok(OrtRunner { session, channels, tile, out_len })
    }

    /// "ONNX Runtime 1.32.0 (WebGPU)".
    pub fn describe(&self) -> String {
        self.session.describe()
    }

    /// Whether it runs on the graphics card.
    pub fn on_gpu(&self) -> bool {
        self.session.provider().is_gpu()
    }
}

impl TileRunner for OrtRunner {
    fn run(&self, input: &[f32]) -> Result<Vec<f32>, Error> {
        let want = self.channels.saturating_mul(self.tile).saturating_mul(self.tile);
        if input.len() != want {
            return Err(Error::Input(format!("a tile of {} values, the model takes {want}", input.len())));
        }
        let x = Tensor::new(vec![1, self.channels, self.tile, self.tile], input.to_vec()).map_err(|e| Error::Input(e.to_string()))?;
        let y = self.session.run_one(x).map_err(|e| Error::Runtime(e.to_string()))?;
        if y.data.len() != self.out_len {
            return Err(Error::Model(format!("{} values for a tile, {} were expected", y.data.len(), self.out_len)));
        }
        Ok(y.data)
    }
}

#[cfg(all(test, feature = "runtime"))]
mod tests {
    use super::*;
    use crate::manifest::{Domain, Gain};

    fn manifest(tile: u32) -> DenoiserManifest {
        DenoiserManifest {
            id: "synthetic".into(),
            name: "Synthetic".into(),
            version: "1".into(),
            licence: Default::default(),
            source: None,
            sha256: None,
            size_bytes: None,
            provenance: "synthetic test".into(),
            domain: Domain::BayerToRgb,
            tile,
            overlap: 16,
            gain: Gain::None,
        }
    }

    fn model(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("lc-ort-{name}-{}.onnx", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn agrees_with_the_cpu_runner() {
        let path = model("unet", &crate::synthetic::unet_onnx(64, 5, 1, 7));
        let m = manifest(64);
        let ort = match OrtRunner::load(&path, &m, Want::Cpu { threads: 1 }) {
            Ok(r) => r,
            Err(e) => {
                lcc_testkit::skip!(Model, "no ONNX Runtime: {e}");
                let _ = std::fs::remove_file(&path);
                return;
            }
        };
        let cpu = crate::runtime::CpuRunner::load(&path, &m).unwrap();
        let x = crate::runtime::check_tile(64);
        let (a, b) = (ort.run(&x).unwrap(), cpu.run(&x).unwrap());
        assert_eq!(a.len(), b.len());
        let scale = b.iter().fold(0f32, |m, v| m.max(v.abs())).max(1e-12);
        let worst = a.iter().zip(&b).fold(0f32, |m, (p, q)| m.max((p - q).abs())) / scale;
        assert!(worst < 1e-4, "ONNX Runtime and the CPU runner differ by {worst}");
        // a tile of the wrong size is an error
        assert!(ort.run(&x[..10]).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn hostile_files_and_manifests_are_errors() {
        let path = model("garbage", b"\x08\xff\xff\xff\xff\xff\xff\xff\xff\xff\x01garbage");
        assert!(OrtRunner::load(&path, &manifest(64), Want::Cpu { threads: 1 }).is_err());
        assert!(OrtRunner::load(&path, &manifest(u32::MAX), Want::Gpu).is_err());
        assert!(OrtRunner::load(Path::new("/nonexistent/model.onnx"), &manifest(64), Want::Gpu).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
