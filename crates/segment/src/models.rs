//! The openly licensed AI models: what each one is for, the files it needs (pinned by size and
//! SHA-256), where it is downloaded from, and its licence.
//!
//! None of them is part of the app: each is downloaded once, into its own folder of the models
//! folder, only after the user asked for it (the app names the model, its size and its licence
//! first). SAM 3 (gated, SAM License) and BiRefNet keep their own download paths ([`crate::fetch`]).
//!
//! The models here are Apache-2.0 or MIT and ungated; their default download location is the
//! publisher's own Hugging Face repository, pinned to a commit (licences checked on the model
//! cards and repositories on 2026-10-14 by OneSlips, and again on 2026-11-03).

use std::path::Path;

use lcc_fetch::FileSpec;

/// One downloadable model.
#[derive(Debug)]
pub struct ModelInfo {
    /// Stable id for commands (`ai.models.download {"id": …}`).
    pub key: &'static str,
    /// Folder inside the models folder.
    pub dir_name: &'static str,
    pub title: &'static str,
    /// What it does, for the model dialog.
    pub purpose: &'static str,
    pub files: &'static [FileSpec<'static>],
    pub license_name: &'static str,
    pub license_url: &'static str,
    /// The publisher's page for the model.
    pub page_url: &'static str,
    /// Built-in download locations (file `f` is at `<base>/<f>`), in order.
    pub default_mirrors: &'static [&'static str],
    /// Environment variable (without the brand prefix) with extra mirrors, tried first.
    pub mirrors_env: &'static str,
    /// Whether the app offers it in the model dialog (a model nothing uses yet is only
    /// downloadable by command).
    pub offered: bool,
    /// Extra files from the same publisher under the same licence, each fetched from its own
    /// address with the model. The model works without them, so they are not part of
    /// [`ModelInfo::installed_in`].
    pub companions: &'static [Companion],
    /// Credit for what the model was trained on, shown with its licence (empty: none needed).
    pub credit: &'static str,
}

/// An optional file that comes with a model (see [`ModelInfo::companions`]).
#[derive(Debug)]
pub struct Companion {
    pub file: FileSpec<'static>,
    /// The folder it is published in (the file is at `<base>/<name>`).
    pub base: &'static str,
}

impl ModelInfo {
    /// Bytes to download (the files and the companions).
    pub fn size(&self) -> u64 {
        self.files.iter().chain(self.companions.iter().map(|c| &c.file)).filter_map(|f| f.size).sum()
    }

    /// The companions missing from `dir` (or there with the wrong size).
    pub fn missing_companions(&self, dir: &Path) -> Vec<&'static Companion> {
        self.companions
            .iter()
            .filter(|c| !matches!((std::fs::metadata(dir.join(c.file.name)), c.file.size), (Ok(m), Some(size)) if m.is_file() && m.len() == size))
            .collect()
    }

    /// Whether `dir` holds the model: every file is there, pinned ones with their exact size
    /// (the SHA-256 is checked when a file is downloaded).
    pub fn installed_in(&self, dir: &Path) -> bool {
        self.files.iter().all(|f| match (std::fs::metadata(dir.join(f.name)), f.size) {
            (Ok(m), Some(size)) => m.is_file() && m.len() == size,
            (Ok(m), None) => m.is_file(),
            (Err(_), _) => false,
        })
    }

    /// The mirrors to try, in order: the environment variable's, the mirrors file's, the built-in ones.
    pub fn mirrors(&self, env: Option<&str>, file: Option<&Path>) -> Vec<String> {
        lcc_fetch::mirrors(env, file, self.default_mirrors)
    }

    /// The user's mirror list for this model, next to the model folders.
    pub fn mirrors_file_name(&self) -> String {
        format!("{}-mirrors.txt", self.dir_name)
    }
}

/// SAM 2.1 Hiera-Tiny, transformers format (`facebook/sam2.1-hiera-tiny`).
pub const SAM2_FILES: &[FileSpec<'static>] = &[
    FileSpec { name: "config.json", size: Some(5_695), sha256: Some("860aff9751b139d83a4ad7df1e5535416fded533e0ead02625edbefcb9953cce"), max: 5_695 },
    FileSpec {
        name: "model.safetensors",
        size: Some(155_908_064),
        sha256: Some("48c14467e5cf9e51870511feb72c89688e82dd74523142c0538b663e193ac2a7"),
        max: 155_908_064,
    },
];

/// OWLv2 base patch16 ensemble, transformers format (`google/owlv2-base-patch16-ensemble`).
pub const DESCRIBE_FILES: &[FileSpec<'static>] = &[
    FileSpec { name: "config.json", size: Some(414), sha256: Some("ba9df8c25a4b8461887dd0a93d9252c9cd84697fe8d49a9d8794ce409af9acb2"), max: 414 },
    FileSpec {
        name: "vocab.json",
        size: Some(1_059_962),
        sha256: Some("e089ad92ba36837a0d31433e555c8f45fe601ab5c221d4f607ded32d9f7a4349"),
        max: 1_059_962,
    },
    FileSpec {
        name: "merges.txt",
        size: Some(524_619),
        sha256: Some("9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a"),
        max: 524_619,
    },
    FileSpec {
        name: "model.safetensors",
        size: Some(619_918_824),
        sha256: Some("e1e130b9e404cf91a75ad45644c1da9d7fa5284085eecc864266a6923efb99e7"),
        max: 619_918_824,
    },
];

/// OWLv2 (Apache-2.0 code and weights, by Google): finds boxes around what a Describe mask names,
/// which SAM 2.1 then segments, so Describe works without SAM 3 (ported from OneSlips' fork,
/// MASK-DESCRIBE). Trained on web image-text pairs (WebLI) with pseudo-labels: offered as an
/// on-request download like the other open models.
pub const DESCRIBE: ModelInfo = ModelInfo {
    key: "describe",
    dir_name: "owlv2-base-patch16-ensemble",
    title: "OWLv2 object finder",
    purpose: "Describe: select what you type (with SAM 2.1)",
    files: DESCRIBE_FILES,
    license_name: "Apache-2.0",
    license_url: "https://huggingface.co/google/owlv2-base-patch16-ensemble",
    page_url: "https://huggingface.co/google/owlv2-base-patch16-ensemble",
    default_mirrors: &["https://huggingface.co/google/owlv2-base-patch16-ensemble/resolve/cfd3195ba4ea9592eec887ded089f4c08eff231d"],
    mirrors_env: "DESCRIBE_MIRRORS",
    offered: true,
    companions: &[],
    credit: "",
};

/// Depth Anything V2 Small, transformers format (`depth-anything/Depth-Anything-V2-Small-hf`).
pub const DEPTH_FILES: &[FileSpec<'static>] = &[
    FileSpec { name: "config.json", size: Some(950), sha256: Some("c56698d3643dde1f83ea2212759e6b31a22b8f827246a36dd007ee8a22b3ff75"), max: 950 },
    FileSpec {
        name: "model.safetensors",
        size: Some(99_173_660),
        sha256: Some("3152477ce0d8d6978d76b995120de97cb5b928701fd0f817769f59e249a16b70"),
        max: 99_173_660,
    },
];

pub const SAM2: ModelInfo = ModelInfo {
    key: "sam2",
    dir_name: "sam2.1-hiera-tiny",
    title: "SAM 2.1",
    purpose: "Object masks: click what to select (an open alternative to SAM 3)",
    files: SAM2_FILES,
    license_name: "Apache-2.0",
    license_url: "https://github.com/facebookresearch/sam2/blob/main/LICENSE",
    page_url: "https://huggingface.co/facebook/sam2.1-hiera-tiny",
    default_mirrors: &["https://huggingface.co/facebook/sam2.1-hiera-tiny/resolve/de431c4043854a71d8101e17995dfe596bf101a5"],
    mirrors_env: "SAM2_MIRRORS",
    offered: true,
    companions: &[],
    credit: "",
};

pub const DEPTH: ModelInfo = ModelInfo {
    key: "depth",
    dir_name: "depth-anything-v2-small",
    title: "Depth Anything V2 Small",
    purpose: "Depth Range masks: how near or far each part of a photo is",
    files: DEPTH_FILES,
    license_name: "Apache-2.0",
    license_url: "https://huggingface.co/depth-anything/Depth-Anything-V2-Small-hf",
    page_url: "https://huggingface.co/depth-anything/Depth-Anything-V2-Small-hf",
    default_mirrors: &["https://huggingface.co/depth-anything/Depth-Anything-V2-Small-hf/resolve/5426e4f0f36572d16453bbda7a8389317b1bef99"],
    mirrors_env: "DEPTH_MIRRORS",
    offered: true,
    companions: &[],
    credit: "",
};

/// EoMT-S 2× COCO panoptic, 640 px (`tue-mps/coco_panoptic_eomt_small_640_2x`); `config.json`
/// is checked by the loader (the right architecture).
pub const SCENE_FILES: &[FileSpec<'static>] = &[
    FileSpec { name: "config.json", size: Some(6_426), sha256: Some("de222958bcd41d63e57059ffb42bfedbe6dddc6f8ac61529af8558e86667d2cc"), max: 6_426 },
    FileSpec {
        name: "model.safetensors",
        size: Some(95_927_796),
        sha256: Some("516b0b23acace639cc27739593881dda1c0010b3bec06aae138b615717fb8855"),
        max: 95_927_796,
    },
];

/// EoMT-S (MIT code and weights, by the TU/e Mobile Perception Systems lab), trained on COCO
/// panoptic: its annotations are CC BY 4.0, credited with the model (maintainer decision
/// 2026-11-03: offered as an on-request download).
pub const SCENE: ModelInfo = ModelInfo {
    key: "scene",
    dir_name: "eomt-s-coco",
    title: "EoMT scene",
    purpose: "Sky, Landscape and People masks: what is sky, water, plants or a person",
    files: SCENE_FILES,
    license_name: "MIT",
    license_url: "https://github.com/tue-mps/eomt/blob/master/LICENSE",
    page_url: "https://huggingface.co/tue-mps/coco_panoptic_eomt_small_640_2x",
    default_mirrors: &["https://huggingface.co/tue-mps/coco_panoptic_eomt_small_640_2x/resolve/10f5326794a8ce7e0e975791033ae1c13aec3767"],
    mirrors_env: "SCENE_MIRRORS",
    offered: true,
    companions: &[],
    credit: "Trained on COCO (cocodataset.org); COCO annotations © the COCO Consortium, CC BY 4.0",
};

/// A pinned file of [`lcc_tflite::models`] as a download spec.
const fn spec(f: &lcc_tflite::models::ModelFile) -> FileSpec<'static> {
    FileSpec { name: f.name, size: Some(f.size), sha256: Some(f.sha256), max: f.size }
}

const TF_PEOPLE: &lcc_tflite::models::ModelFile = &lcc_tflite::models::PEOPLE.files[0];
const TF_FACE: &lcc_tflite::models::ModelFile = &lcc_tflite::models::FACE.files[0];
const TF_FACE_FAR: &lcc_tflite::models::ModelFile = &lcc_tflite::models::FACE.files[1];

/// MediaPipe Selfie Multiclass 256 (float32, v1).
pub const PEOPLE_FILES: &[FileSpec<'static>] = &[spec(TF_PEOPLE)];
/// MediaPipe Face Landmarker bundle (float16, v1: BlazeFace short range and FaceMesh-V2).
pub const FACE_FILES: &[FileSpec<'static>] = &[spec(TF_FACE)];

/// Skin, hair and clothes of people: what Smooth Skin and Find Blemishes look on.
pub const PEOPLE: ModelInfo = ModelInfo {
    key: lcc_tflite::models::PEOPLE.key,
    dir_name: lcc_tflite::models::PEOPLE.dir_name,
    title: "MediaPipe people",
    purpose: "Retouch: face and body skin (Smooth Skin, Find Blemishes)",
    files: PEOPLE_FILES,
    license_name: lcc_tflite::models::PEOPLE.license_name,
    license_url: lcc_tflite::models::PEOPLE.license_url,
    page_url: lcc_tflite::models::PEOPLE.page_url,
    default_mirrors: &["https://storage.googleapis.com/mediapipe-models/image_segmenter/selfie_multiclass_256x256/float32/1"],
    mirrors_env: "PEOPLE_MIRRORS",
    offered: true,
    companions: &[],
    credit: "",
};

/// Faces with their landmarks: Detect Faces, the face scan, and the parts of a face Retouch works on.
pub const FACE: ModelInfo = ModelInfo {
    key: lcc_tflite::models::FACE.key,
    dir_name: lcc_tflite::models::FACE.dir_name,
    title: "MediaPipe faces",
    purpose: "Faces: Detect Faces, eyes, lips and teeth for Retouch",
    files: FACE_FILES,
    license_name: lcc_tflite::models::FACE.license_name,
    license_url: lcc_tflite::models::FACE.license_url,
    page_url: lcc_tflite::models::FACE.page_url,
    default_mirrors: &["https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1"],
    mirrors_env: "FACE_MIRRORS",
    offered: true,
    // BlazeFace full range, for the small faces of group photos
    companions: &[Companion { file: spec(TF_FACE_FAR), base: "https://storage.googleapis.com/mediapipe-assets" }],
    credit: "",
};

/// Every model of this registry, in display order.
pub const MODELS: &[&ModelInfo] = &[&PEOPLE, &FACE, &SAM2, &DESCRIBE, &SCENE, &DEPTH];

/// The model with command key `key`.
pub fn by_key(key: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().copied().find(|m| m.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_is_pinned_and_has_a_location() {
        for m in MODELS {
            assert!(by_key(m.key).is_some());
            assert!(!m.default_mirrors.is_empty() && m.default_mirrors.iter().all(|u| u.starts_with("https://")), "{}", m.key);
            assert!(m.files.iter().all(|f| f.size.is_some() && f.sha256.is_some()), "{}", m.key);
            assert!(m.size() > 0);
        }
        assert!(by_key("nope").is_none());
    }

    #[test]
    fn the_mediapipe_entries_download_what_lcc_tflite_pins() {
        for (m, tf) in [(&PEOPLE, &lcc_tflite::models::PEOPLE), (&FACE, &lcc_tflite::models::FACE)] {
            let names: Vec<&str> = m.files.iter().chain(m.companions.iter().map(|c| &c.file)).map(|f| f.name).collect();
            let pinned: Vec<&str> = tf.files.iter().map(|f| f.name).collect();
            assert_eq!(names, pinned, "{}", m.key);
            for f in tf.files {
                let base = m.companions.iter().find(|c| c.file.name == f.name).map_or(m.default_mirrors[0], |c| c.base);
                assert_eq!(format!("{base}/{}", f.name), f.url);
            }
        }
    }

    /// Describe's open detector downloads the files its loader reads, pinned, from the publisher.
    #[test]
    fn the_describe_entry_downloads_what_the_detector_loads() {
        let d = by_key("describe").unwrap();
        assert!(d.offered && d.default_mirrors.iter().all(|u| u.starts_with("https://huggingface.co/google/")));
        assert!(crate::owl::OWL_FILES.iter().all(|f| d.files.iter().any(|s| s.name == *f)), "the loader's files are the pinned ones");
    }

    #[test]
    fn installed_needs_every_file_at_its_size() {
        let dir = std::env::temp_dir().join(format!("lcc-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!SAM2.installed_in(&dir));
        std::fs::write(dir.join("config.json"), vec![b' '; 5_695]).unwrap();
        std::fs::write(dir.join("model.safetensors"), b"short").unwrap();
        assert!(!SAM2.installed_in(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
