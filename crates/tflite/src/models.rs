//! The official MediaPipe model files this interpreter runs, pinned by size and SHA-256, with
//! where Google publishes them and their licence. Plain data: the engine's model download
//! (Settings ▸ AI models) turns these into fetch specs, and nothing is downloaded unless the user
//! asks for it.
//!
//! Licences checked on Google's model cards on 2026-11-03: every card says "Licensed under
//! Apache License, Version 2.0" (Selfie Multiclass, FaceMesh V2, BlazeFace short and full range).
//! Download locations are Google's own `storage.googleapis.com/mediapipe-models` and
//! `mediapipe-assets` buckets, the ones MediaPipe's documentation links.

/// One file of a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    /// File name in the model's folder.
    pub name: &'static str,
    /// Exact size in bytes.
    pub size: u64,
    /// SHA-256 of the file, lower-case hex.
    pub sha256: &'static str,
    /// Where Google publishes it.
    pub url: &'static str,
}

/// A MediaPipe model the People masks and Faces can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaPipeModel {
    /// Stable key (`ai.models.*` ids, settings).
    pub key: &'static str,
    /// Its folder inside the models folder.
    pub dir_name: &'static str,
    pub files: &'static [ModelFile],
    pub license_name: &'static str,
    pub license_url: &'static str,
    pub page_url: &'static str,
}

/// Selfie Multiclass 256 (float32, v1): hair, face skin, body skin, clothes.
pub const PEOPLE: MediaPipeModel = MediaPipeModel {
    key: "people",
    dir_name: "mediapipe-selfie-multiclass",
    files: &[ModelFile {
        name: "selfie_multiclass_256x256.tflite",
        size: 16_371_837,
        sha256: "c6748b1253a99067ef71f7e26ca71096cd449baefa8f101900ea23016507e0e0",
        url: "https://storage.googleapis.com/mediapipe-models/image_segmenter/selfie_multiclass_256x256/float32/1/selfie_multiclass_256x256.tflite",
    }],
    license_name: "Apache-2.0",
    license_url: "https://storage.googleapis.com/mediapipe-assets/Model%20Card%20Multiclass%20Segmentation.pdf",
    page_url: "https://ai.google.dev/edge/mediapipe/solutions/vision/image_segmenter",
};

/// Face Landmarker bundle (float16, v1: BlazeFace short range + FaceMesh-V2, read with
/// [`crate::zip`]) and BlazeFace full range for the small faces of group photos.
pub const FACE: MediaPipeModel = MediaPipeModel {
    key: "face",
    dir_name: "mediapipe-face-landmarker",
    files: &[
        ModelFile {
            name: "face_landmarker.task",
            size: 3_758_596,
            sha256: "64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff",
            url: "https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1/face_landmarker.task",
        },
        ModelFile {
            name: "face_detection_full_range.tflite",
            size: 1_083_786,
            sha256: "3698b18f063835bc609069ef052228fbe86d9c9a6dc8dcb7c7c2d69aed2b181b",
            url: "https://storage.googleapis.com/mediapipe-assets/face_detection_full_range.tflite",
        },
    ],
    license_name: "Apache-2.0",
    license_url: "https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Face%20Mesh%20V2.pdf",
    page_url: "https://ai.google.dev/edge/mediapipe/solutions/vision/face_landmarker",
};

/// Every MediaPipe model, in the order the app lists them.
pub const MODELS: &[MediaPipeModel] = &[PEOPLE, FACE];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_is_pinned_and_official() {
        for m in MODELS {
            assert_eq!(m.license_name, "Apache-2.0");
            for f in m.files {
                assert!(f.url.starts_with("https://storage.googleapis.com/mediapipe-"), "{}", f.url);
                assert!(f.url.ends_with(f.name), "{}", f.url);
                assert!(f.sha256.len() == 64 && f.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
                assert!(f.size > 0);
            }
        }
    }
}
