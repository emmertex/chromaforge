//! The denoise models the app knows about: what each one is, who may use it, and where to fetch it.
//!
//! None is part of the app. A model with an address here can be downloaded when the user asks and has accepted
//! its terms; anything else is brought by the user as an `.onnx` file with a `denoise-model.json` beside it.

#[cfg(any(feature = "rawnind-model", feature = "own-model", test))]
use crate::licence::{Commercial, Licence};

use crate::manifest::DenoiserManifest;
#[cfg(any(feature = "rawnind-model", feature = "own-model", test))]
use crate::manifest::{Domain, Gain};

/// A known model, and how to get it.
pub struct Known {
    pub manifest: DenoiserManifest,
    /// What to fetch (an archive or the `.onnx` itself); `None` when the user must bring the file.
    pub download: Option<Download>,
    /// The `.onnx` inside the downloaded archive (`None`: the download is the `.onnx`).
    pub entry: Option<&'static str>,
}

/// RawNIND's UtNet2 for Bayer sensors, as published for darktable (`rawdenoise-nind.dtmodel`, a zip with a Bayer and a
/// linear model). Its weights are GPL-3.0; the training pictures are CC BY 4.0 and CC0.
#[cfg(any(feature = "rawnind-model", test))]
fn rawnind_bayer() -> Known {
    let manifest = DenoiserManifest {
        id: "rawnind-bayer".into(),
        name: "RawNIND UtNet2 (Bayer)".into(),
        version: "1.0".into(),
        licence: Licence {
            name: "GPL-3.0".into(),
            commercial: Commercial::Yes,
            url: Some("https://www.gnu.org/licenses/gpl-3.0.html".into()),
            notice: "The weights are published under the GNU General Public License v3. The app does not include them: they \
                     are downloaded to your computer at your request and used by it only. If you pass the model file on, the \
                     GPL applies to that copy. Commercial use is subject to the GPL terms, including applicable source, licence and notice obligations on redistribution."
                .into(),
        },
        source: Some("https://github.com/darktable-org/darktable-ai/tree/master/models/rawdenoise-nind".into()),
        sha256: Some("da27509dab6a2915da67e988acd86cf71f9d5bbc8d1aa0ed32933578a887b901".into()),
        size_bytes: Some(31_056_425),
        provenance: "Trained on RawNIND, real photographs of the same scenes taken noisy and clean (images CC BY 4.0 and CC0, \
                     published on Wikimedia Commons and by the University of Louvain). Paper: arXiv 2501.08924."
            .into(),
        domain: Domain::BayerToRgb,
        tile: 512,
        overlap: 64,
        gain: Gain::MatchMean { nominal: 1.0e6, max_deviation: 0.05 },
    };
    let download = Download {
        id: manifest.id.clone(),
        url: "https://github.com/darktable-org/darktable-ai/releases/download/release-5.6.0/rawdenoise-nind.dtmodel".into(),
        fallbacks: Vec::new(),
        file_name: "rawdenoise-nind.dtmodel".into(),
        size_bytes: 57_700_134,
        sha256: "d71b5f1e727c85a359e6f74dca9e2016c9d8fc3e2f7ac3e9b347d80ceca969af".into(),
    };
    Known { manifest, download: Some(download), entry: Some("model_bayer.onnx") }
}

/// The upstream project's lower-case name, as its model's id and file name spell it.
fn upstream_slug() -> String {
    lcc_brand::UPSTREAM_NAME.to_ascii_lowercase()
}

/// Where the upstream project's own model is downloaded from first: a release asset of that project (its maintainers
/// upload the file there; until they do, this address fails and the next one is used).
pub fn own_bayer_url() -> String {
    format!("{}/releases/download/denoise-model-0.1.0/{}", lcc_brand::UPSTREAM_REPOSITORY, own_bayer_file())
}

/// The second source, used when the first fails: radfilms.com.au, the trainer's site, serving the same file. Its size
/// and SHA-256 (below) are checked whichever source served it.
pub fn own_bayer_fallback_url() -> String {
    format!("https://radfilms.com.au/{}/models/{}", upstream_slug(), own_bayer_file())
}

/// The file name of that model.
fn own_bayer_file() -> String {
    format!("{}-bayer-0.1.0.onnx", upstream_slug())
}

/// The environment variable (after `<PREFIX>_`) that replaces both sources with one (an `https://` address), so a
/// build or a maintainer can host the same file elsewhere. There is no other source.
pub const OWN_BAYER_URL_ENV: &str = "DENOISE_MODEL_URL";

/// The addresses the own model is fetched from, in order: `<PREFIX>_`[`OWN_BAYER_URL_ENV`] alone when it holds an
/// `https://` address, else [`own_bayer_url`] then [`own_bayer_fallback_url`].
pub fn own_bayer_urls() -> Vec<String> {
    urls_or_default(lcc_brand::env(OWN_BAYER_URL_ENV).as_deref())
}

fn urls_or_default(set: Option<&str>) -> Vec<String> {
    match set.map(str::trim) {
        Some(u) if u.starts_with("https://") && u.len() > "https://".len() && !u.contains(char::is_whitespace) => vec![u.to_string()],
        _ => vec![own_bayer_url(), own_bayer_fallback_url()],
    }
}

/// SHA-256 and size of that `.onnx` (`tools/denoise-train/export.py` prints both).
#[cfg(any(feature = "own-model", test))]
const OWN_BAYER_SHA256: &str = "abd3c4cdea163b27439cefae8972a21755510bce18ee036e991d20cf3247dea9";
#[cfg(any(feature = "own-model", test))]
const OWN_BAYER_BYTES: u64 = 31_091_398;

/// The upstream project's own Bayer denoiser: a U-Net trained by its contributors with `tools/denoise-train` on CC0
/// raw samples from raw.pixls.us with synthetic, calibrated sensor noise. The weights are MIT OR Apache-2.0; the
/// download is the `.onnx` itself.
#[cfg(any(feature = "own-model", test))]
fn own_bayer() -> Known {
    let up = lcc_brand::UPSTREAM_NAME;
    let repo = lcc_brand::UPSTREAM_REPOSITORY;
    let manifest = DenoiserManifest {
        id: format!("{}-bayer", upstream_slug()),
        name: format!("{up} Denoise (Bayer)"),
        version: "0.1.0".into(),
        licence: Licence {
            name: "MIT OR Apache-2.0".into(),
            commercial: Commercial::Yes,
            url: Some(format!("{repo}/blob/main/LICENSE-MIT")),
            notice: format!(
                "{up}'s own denoise model, trained by the {up} contributors. The weights are licensed MIT OR Apache-2.0: \
                 you may use them for any purpose, including commercial work, and pass them on with this notice. Training \
                 pictures: low-ISO raw samples from raw.pixls.us, each dedicated to the public domain (CC0) by its \
                 contributor; thank you."
            ),
        },
        source: Some(format!("{repo}/tree/main/tools/denoise-train")),
        sha256: Some(OWN_BAYER_SHA256.into()),
        size_bytes: Some(OWN_BAYER_BYTES),
        provenance: format!(
            "Trained by {up} contributors with their open training tools (tools/denoise-train) on clean low-ISO raw \
             photos published on raw.pixls.us under CC0, with synthetic sensor noise calibrated on real cameras (numbers \
             only). No photos with other licences were used."
        ),
        domain: Domain::BayerToRgb,
        tile: 512,
        overlap: 64,
        gain: Gain::None,
    };
    let mut urls = own_bayer_urls().into_iter();
    let download = Download {
        id: manifest.id.clone(),
        url: urls.next().unwrap_or_else(own_bayer_url),
        fallbacks: urls.collect(),
        file_name: own_bayer_file(),
        size_bytes: OWN_BAYER_BYTES,
        sha256: OWN_BAYER_SHA256.into(),
    };
    Known { manifest, download: Some(download), entry: None }
}

/// Every model the app knows (none in a default build: each is offered by its own feature).
pub fn all() -> Vec<Known> {
    Vec::from([
        #[cfg(feature = "own-model")]
        own_bayer(),
        #[cfg(feature = "rawnind-model")]
        rawnind_bayer(),
    ])
}

/// The known model with this id.
pub fn find(id: &str) -> Option<Known> {
    all().into_iter().find(|k| k.manifest.id == id)
}

/// The known model whose `.onnx` has this SHA-256.
pub fn by_sha256(sha256: &str) -> Option<DenoiserManifest> {
    all().into_iter().map(|k| k.manifest).find(|m| m.sha256.as_deref() == Some(sha256))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::validate;

    #[test]
    fn known_models_are_valid_unique_and_downloaded_over_https() {
        let all = all();
        assert_eq!(all.is_empty(), !cfg!(any(feature = "rawnind-model", feature = "own-model")));
        validate(&rawnind_bayer().manifest).unwrap();
        validate(&own_bayer().manifest).unwrap();
        for (i, k) in all.iter().enumerate() {
            validate(&k.manifest).unwrap();
            assert!(all.iter().skip(i + 1).all(|o| o.manifest.id != k.manifest.id), "unique ids");
            assert!(!k.manifest.licence.name.is_empty() && !k.manifest.licence.notice.is_empty(), "terms are shown before use");
            if let Some(d) = &k.download {
                assert_eq!(d.id, k.manifest.id);
                assert!(d.url.starts_with("https://") && d.sha256.len() == 64 && d.size_bytes > 0);
                // a name that cannot leave the staging folder
                assert!(!d.file_name.contains(['/', '\\']) && !d.file_name.starts_with('.'));
            }
            // a model that may not be used commercially is never offered for download
            if k.manifest.licence.commercial == Commercial::No {
                assert!(k.download.is_none(), "{}", k.manifest.id);
            }
        }
        assert_eq!(find("rawnind-bayer").is_some(), cfg!(feature = "rawnind-model"));
        assert_eq!(find(&format!("{}-bayer", upstream_slug())).is_some(), cfg!(feature = "own-model"));
        assert!(find("nothing").is_none());
        assert_eq!(
            by_sha256("da27509dab6a2915da67e988acd86cf71f9d5bbc8d1aa0ed32933578a887b901").map(|m| m.id),
            if cfg!(feature = "rawnind-model") { Some("rawnind-bayer".into()) } else { None }
        );
    }

    #[test]
    fn own_model_is_permissive_and_its_download_is_the_onnx_it_names() {
        let k = own_bayer();
        let (m, d) = (&k.manifest, k.download.as_ref().unwrap());
        crate::manifest::validate(m).unwrap();
        assert_eq!(m.licence.commercial, Commercial::Yes);
        assert_eq!(m.licence.name, "MIT OR Apache-2.0");
        assert!(m.provenance.contains("CC0") && m.licence.notice.contains("CC0"), "the data credit is shown");
        // the download is the model file itself, so its hash and size are the manifest's
        assert!(k.entry.is_none());
        assert_eq!(m.sha256.as_deref(), Some(d.sha256.as_str()));
        assert_eq!(m.size_bytes, Some(d.size_bytes));
        assert!(d.url.starts_with("https://") && d.file_name.ends_with(".onnx"));
        assert_eq!(m.gain, Gain::None);
    }

    #[test]
    fn the_model_comes_from_the_project_then_its_mirror_or_one_address_given() {
        let both = vec![own_bayer_url(), own_bayer_fallback_url()];
        assert_eq!(urls_or_default(None), both);
        assert_eq!(host(&own_bayer_url()), "github.com");
        assert_eq!(host(&own_bayer_fallback_url()), "radfilms.com.au");
        // one address given replaces both: no silent second source
        assert_eq!(urls_or_default(Some("https://example.org/m.onnx")), vec!["https://example.org/m.onnx".to_string()]);
        assert_eq!(urls_or_default(Some("http://example.org/m.onnx")), both, "only https");
        assert_eq!(urls_or_default(Some("https://")), both);
        assert_eq!(urls_or_default(Some("https://a b")), both);
        let k = own_bayer();
        let d = k.download.as_ref().unwrap();
        assert_eq!((d.url.as_str(), d.fallbacks.as_slice()), (own_bayer_url().as_str(), &[own_bayer_fallback_url()][..]));
        // the same file wherever it comes from: both end in its name
        assert!(both.iter().all(|u| u.ends_with(&format!("/{}", d.file_name))));
        assert!(!own_bayer().manifest.licence.notice.contains("  "), "one space between words");
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Download {
    pub id: String,
    pub url: String,
    /// Further sources of the same file, tried in order when the one before fails (unreachable, refused, or a file
    /// whose size or SHA-256 isn't this one's). The file is kept only when its SHA-256 is [`Download::sha256`].
    pub fallbacks: Vec<String>,
    /// The name the file is kept under while it waits for the user to accept its terms.
    pub file_name: String,
    pub size_bytes: u64,
    pub sha256: String,
}

pub fn host(url: &str) -> &str {
    url.strip_prefix("https://").and_then(|r| r.split('/').next()).unwrap_or("")
}
