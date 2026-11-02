//! The openly licensed AI models: what each one is for, the files it needs (pinned by size and
//! SHA-256), where it is downloaded from, and its licence.
//!
//! None of them is part of the app: each is downloaded once, into its own folder of the models
//! folder, only after the user asked for it (the app names the model, its size and its licence
//! first). SAM 3 (gated, SAM License) and BiRefNet keep their own download paths ([`crate::fetch`]).
//!
//! Both models here are Apache-2.0 and ungated; their default download location is the
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
}

impl ModelInfo {
    /// Bytes to download.
    pub fn size(&self) -> u64 {
        self.files.iter().filter_map(|f| f.size).sum()
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
};

pub const DEPTH: ModelInfo = ModelInfo {
    key: "depth",
    dir_name: "depth-anything-v2-small",
    title: "Depth Anything V2 Small",
    purpose: "Relative depth of a photo (not used by masks yet)",
    files: DEPTH_FILES,
    license_name: "Apache-2.0",
    license_url: "https://huggingface.co/depth-anything/Depth-Anything-V2-Small-hf",
    page_url: "https://huggingface.co/depth-anything/Depth-Anything-V2-Small-hf",
    default_mirrors: &["https://huggingface.co/depth-anything/Depth-Anything-V2-Small-hf/resolve/5426e4f0f36572d16453bbda7a8389317b1bef99"],
    mirrors_env: "DEPTH_MIRRORS",
    offered: false,
};

/// MediaPipe Selfie Multiclass 256 (float32, version 1, Apache-2.0, Google).
pub const PEOPLE_FILES: &[FileSpec<'static>] = &[FileSpec {
    name: "selfie_multiclass_256x256.tflite",
    size: Some(16_371_837),
    sha256: Some("c6748b1253a99067ef71f7e26ca71096cd449baefa8f101900ea23016507e0e0"),
    max: 16_371_837,
}];

/// MediaPipe Face Landmarker bundle (float16, version 1, Apache-2.0, Google): a zip of BlazeFace
/// (short range), FaceMesh-V2 and the blendshapes model (unused).
pub const FACE_FILES: &[FileSpec<'static>] = &[FileSpec {
    name: "face_landmarker.task",
    size: Some(3_758_596),
    sha256: Some("64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff"),
    max: 3_758_596,
}];

/// MediaPipe BlazeFace full range (float32, Apache-2.0, Google): the small faces of group photos
/// that the bundle's short-range detector misses. Kept in the faces model's folder.
pub const FACE_FAR_FILES: &[FileSpec<'static>] = &[FileSpec {
    name: "face_detection_full_range.tflite",
    size: Some(1_083_786),
    sha256: Some("3698b18f063835bc609069ef052228fbe86d9c9a6dc8dcb7c7c2d69aed2b181b"),
    max: 1_083_786,
}];

pub const PEOPLE: ModelInfo = ModelInfo {
    key: "people",
    dir_name: "mediapipe-selfie-multiclass",
    title: "MediaPipe people",
    purpose: "People: hair, face skin, body skin and clothes",
    files: PEOPLE_FILES,
    license_name: "Apache-2.0",
    license_url: "https://storage.googleapis.com/mediapipe-assets/Model%20Card%20Multiclass%20Segmentation.pdf",
    page_url: "https://ai.google.dev/edge/mediapipe/solutions/vision/image_segmenter",
    default_mirrors: &["https://storage.googleapis.com/mediapipe-models/image_segmenter/selfie_multiclass_256x256/float32/1"],
    mirrors_env: "PEOPLE_MIRRORS",
    offered: true,
};

pub const FACE: ModelInfo = ModelInfo {
    key: "face",
    dir_name: "mediapipe-face-landmarker",
    title: "MediaPipe faces",
    purpose: "People and eye focus: faces, eyes, irises, eyebrows, lips and teeth",
    files: FACE_FILES,
    license_name: "Apache-2.0",
    license_url: "https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Face%20Mesh%20V2.pdf",
    page_url: "https://ai.google.dev/edge/mediapipe/solutions/vision/face_landmarker",
    default_mirrors: &["https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1"],
    mirrors_env: "FACE_MIRRORS",
    offered: true,
};

/// Fetched with [`FACE`] (same folder): not listed on its own.
pub const FACE_FAR: ModelInfo = ModelInfo {
    key: "faceFar",
    dir_name: "mediapipe-face-landmarker",
    title: "MediaPipe small faces",
    purpose: "Eye focus and People: the small faces of group photos",
    files: FACE_FAR_FILES,
    license_name: "Apache-2.0",
    license_url: "https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Face%20Mesh%20V2.pdf",
    page_url: "https://ai.google.dev/edge/mediapipe/solutions/vision/face_detector",
    default_mirrors: &["https://storage.googleapis.com/mediapipe-assets"],
    mirrors_env: "FACE_FAR_MIRRORS",
    offered: false,
};

/// Every model of this registry, in display order.
pub const MODELS: &[&ModelInfo] = &[&PEOPLE, &FACE, &FACE_FAR, &SAM2, &DEPTH];

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
