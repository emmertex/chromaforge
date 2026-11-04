//! Known open-weights ML checkpoints with pinned upstream URLs, exact byte sizes, SHA-256 digests and licence metadata.
//!
//! No model bytes are bundled in the repository or binary. The catalogue lets the user download a known model from its
//! upstream host (with the licence and byte size shown first) or install a local directory they already have on disk.

use crate::manifest::{Commercial, InputSpec, Licence, MANIFEST_VERSION, ModelFile, ModelManifest, Role};

/// Canonical model id for the default CLIP ViT-B/32 vision-text embedding model.
pub const CLIP_VIT_B32_ID: &str = "clip-vit-b32-laion2b";

/// Pinned HuggingFace revision for `laion/CLIP-ViT-B-32-laion2B-s34B-b79K`.
pub const CLIP_VIT_B32_REVISION: &str = "1a25a446712ba5ee05982a381eed697ef9b435cf";

/// Base HuggingFace download URL for the pinned CLIP ViT-B/32 revision.
pub const CLIP_VIT_B32_BASE_URL: &str =
    "https://huggingface.co/laion/CLIP-ViT-B-32-laion2B-s34B-b79K/resolve/1a25a446712ba5ee05982a381eed697ef9b435cf";

/// One file to fetch when downloading a known model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadFile {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size_bytes: u64,
}

/// Complete download specification for a known model.
#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    pub manifest: ModelManifest,
    pub files: Vec<DownloadFile>,
}

/// Built-in manifest for `laion/CLIP-ViT-B-32-laion2B-s34B-b79K` (MIT, 512-dim joint vision + text encoder).
pub fn clip_vit_b32() -> ModelManifest {
    ModelManifest {
        version: MANIFEST_VERSION,
        id: CLIP_VIT_B32_ID.into(),
        name: "CLIP ViT-B/32 (LAION-2B)".into(),
        role: Role::Embedder,
        version_tag: "laion2b-s34b-b79k".into(),
        embed_dim: 512,
        input: InputSpec { width: 224, height: 224, mean: [0.48145466, 0.4578275, 0.40821073], std: [0.26862954, 0.26130258, 0.27577711] },
        files: vec![
            ModelFile {
                name: "model.safetensors".into(),
                sha256: "74813fbcdc750f235c9784c367ca1394d2a5c25eb0aac92761752ac239db7cff".into(),
                size_bytes: 605_157_884,
            },
            ModelFile {
                name: "vocab.json".into(),
                sha256: "5047b556ce86ccaf6aa22b3ffccfc52d391ea4accdab9c2f2407da5b742d4363".into(),
                size_bytes: 862_328,
            },
            ModelFile {
                name: "merges.txt".into(),
                sha256: "f1cd414c2d113a28b296afc0fd66ae854c0e0c34d5de373a2f5b7f62e0c02344".into(),
                size_bytes: 524_618,
            },
        ],
        licence: Licence {
            spdx: "MIT".into(),
            commercial: Commercial::Allowed,
            url: "https://huggingface.co/laion/CLIP-ViT-B-32-laion2B-s34B-b79K".into(),
            attribution: "LAION OpenCLIP ViT-B/32 trained on LAION-2B (Radford et al. 2021; Cherti et al. 2023), MIT Licence".into(),
        },
        notes: "Joint 512-d image + text encoder for semantic search, similarity culling and aesthetic scoring.".into(),
    }
}

/// All known open-weights ML models in the catalogue.
pub fn all() -> Vec<ModelManifest> {
    vec![clip_vit_b32()]
}

/// Look up a known model manifest by id.
pub fn get(id: &str) -> Option<ModelManifest> {
    all().into_iter().find(|m| m.id == id)
}

/// Look up a known model manifest by the SHA-256 of its primary weights file.
pub fn lookup(sha256: &str) -> Option<ModelManifest> {
    let lower = sha256.to_ascii_lowercase();
    all().into_iter().find(|m| m.primary_sha256() == lower)
}

/// Return the upstream download URLs and expected hashes for a known model id.
pub fn download(id: &str) -> Option<Download> {
    match id {
        CLIP_VIT_B32_ID => Some(Download {
            manifest: clip_vit_b32(),
            files: vec![
                DownloadFile {
                    name: "model.safetensors",
                    url: const_str_url!("model.safetensors"),
                    sha256: "74813fbcdc750f235c9784c367ca1394d2a5c25eb0aac92761752ac239db7cff",
                    size_bytes: 605_157_884,
                },
                DownloadFile {
                    name: "vocab.json",
                    url: const_str_url!("vocab.json"),
                    sha256: "5047b556ce86ccaf6aa22b3ffccfc52d391ea4accdab9c2f2407da5b742d4363",
                    size_bytes: 862_328,
                },
                DownloadFile {
                    name: "merges.txt",
                    url: const_str_url!("merges.txt"),
                    sha256: "f1cd414c2d113a28b296afc0fd66ae854c0e0c34d5de373a2f5b7f62e0c02344",
                    size_bytes: 524_618,
                },
            ],
        }),
        _ => None,
    }
}

macro_rules! const_str_url {
    ($file:literal) => {
        concat!("https://huggingface.co/laion/CLIP-ViT-B-32-laion2B-s34B-b79K/resolve/1a25a446712ba5ee05982a381eed697ef9b435cf/", $file)
    };
}
use const_str_url;

/// Extract the hostname from an `https://` URL for UI attribution.
pub fn host(url: &str) -> &str {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
    rest.split('/').next().unwrap_or(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_manifests_validate_and_match_download_entries() {
        for m in all() {
            m.validate().expect("known manifest must validate");
            let dl = download(&m.id).expect("known manifest must have download spec");
            assert_eq!(dl.manifest, m);
            assert_eq!(dl.files.len(), m.files.len());
            for (df, mf) in dl.files.iter().zip(m.files.iter()) {
                assert_eq!(df.name, mf.name);
                assert_eq!(df.sha256, mf.sha256);
                assert_eq!(df.size_bytes, mf.size_bytes);
                assert!(df.url.starts_with(CLIP_VIT_B32_BASE_URL));
                assert_eq!(host(df.url), "huggingface.co");
            }
        }
        assert!(get(CLIP_VIT_B32_ID).is_some());
        assert!(get("unknown-model").is_none());
        assert!(lookup("74813fbcdc750f235c9784c367ca1394d2a5c25eb0aac92761752ac239db7cff").is_some());
    }
}
