//! Photo metadata: EXIF (TIFF/raw files, JPEG APP1, PNG `eXIf`, WebP `EXIF`, HEIF `Exif` items), XMP packets
//! (read + write, standard namespaces plus our own `lc:` namespace) and basic IPTC-IIM.
//!
//! Entry points:
//! - [`read_exif`] — a TIFF-structured Exif block → [`Metadata`]; [`from_tiff`] for an already parsed stream.
//! - [`embedded`] / [`jpeg_segments`] / [`png_chunks`] / [`webp_chunks`] — locate Exif / XMP / ICC / IPTC blocks.
//! - [`parse_xmp`] / [`write_xmp`] — XMP packets (interchange fields + the opaque `lc:settings` JSON).
//! - [`parse_iptc`] — IPTC-IIM record 2 datasets.
//! - [`parse_gpx`] — GPS track logs (GPX) for geotagging by capture time.
//! - [`extract`] — all of the above for a whole file (JPEG, PNG, WebP, TIFF/DNG/raw, PSD/PSB, Canon CR3, HEIF/HEIC/AVIF).
//! - [`cr3`] — the Canon CR3 container (metadata blocks, XMP, the JPEG and raw tracks).
#![forbid(unsafe_code)]

mod container;
pub mod cr3;
mod datetime;
mod exif;
pub mod focus;
mod focus_read;
mod fuji;
mod gpx;
mod heif;
mod iptc;
mod legacy;
pub mod mrw;
mod psd;
pub mod tags;
mod xmp;
mod xmp_merge;

pub use container::{Embedded, embedded, jpeg_segments, png_chunks, webp_chunks};
pub use datetime::DateTime;
pub use exif::{from_tiff, read_exif, strip_exif_header, try_read_exif, write_exif};
pub use focus::FocusPoint;
pub use fuji::{FujiRecipe, FujiRow, read_recipe_file, recipe_from_bytes, recipe_from_exif};
pub use gpx::{GpxError, Match, TrackPoint, Tracklog, parse_gpx};
pub use iptc::parse_iptc;
pub use lcc_geom::{Orientation, Rect};
pub use tags::{TagRow, file_tag_rows, tag_rows};
pub use xmp::{
    CRS_NS, FILTERS_PREFIX, LC_NS, MAX_FILTER_PART, VIDEO_PREFIX, XmpData, XmpError, XmpPluginFilter, XmpValue, parse_xmp, plugin_fields,
    plugin_fields_ns, plugin_filters, plugin_filters_ns, video_ns, write_xmp, write_xmp_ext, write_xmp_filters, write_xmp_lc, write_xmp_video,
};
pub use xmp_merge::{MergeRules, merge_xmp};

use serde::{Deserialize, Serialize};

/// GPS position (WGS 84). Latitude/longitude in signed decimal degrees (north / east positive).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Gps {
    pub latitude: f64,
    pub longitude: f64,
    /// Metres above sea level (negative below).
    pub altitude: Option<f64>,
}

/// Exif `Flash` value decoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flash {
    pub fired: bool,
    /// The raw Exif `Flash` bit field.
    pub raw: u16,
}

/// A named region of interest on a photo (MWG Region Guidelines, `mwg-rs:Regions`): most often a face,
/// drawn by Lightroom, digiKam, Picasa or similar tools. Read-only for now — the app does not write
/// regions yet (see `docs/xmp-interop.md`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Region {
    /// Normalized to the image's full, oriented frame (`Metadata::width`/`height`), y-down, 0..1 on
    /// both axes — MWG's own convention, and what `AppliedToDimensions` areas are converted to.
    pub rect: Rect,
    pub kind: RegionKind,
    /// `mwg-rs:Name` (e.g. the person's name).
    pub name: Option<String>,
    pub description: Option<String>,
}

/// `mwg-rs:Type`. Unrecognised values are kept verbatim rather than dropped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegionKind {
    Face,
    Pet,
    Focus,
    BarCode,
    /// `mwg-rs:Type` was missing, empty, or a value we don't recognise.
    Other(String),
}

/// The creator's contact details (IPTC Core `Iptc4xmpCore:CreatorContactInfo`) and job title
/// (`photoshop:AuthorsPosition`). Empty strings are "not set".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CreatorContact {
    pub job_title: String,
    pub address: String,
    pub city: String,
    pub state: String,
    pub postal_code: String,
    pub country: String,
    pub phone: String,
    pub email: String,
    pub website: String,
}

impl CreatorContact {
    /// The `photo.setMeta` / metadata-preset keys of the fields, in display order.
    pub const KEYS: &'static [&'static str] = &[
        "creatorJobTitle",
        "creatorAddress",
        "creatorCity",
        "creatorState",
        "creatorPostalCode",
        "creatorCountry",
        "creatorPhone",
        "creatorEmail",
        "creatorWebsite",
    ];

    /// Every field empty (after trimming).
    pub fn is_empty(&self) -> bool {
        Self::KEYS.iter().all(|k| self.get(k).is_none_or(|v| v.trim().is_empty()))
    }

    /// The field a [`CreatorContact::KEYS`] key names.
    pub fn get(&self, key: &str) -> Option<&String> {
        Some(match key {
            "creatorJobTitle" => &self.job_title,
            "creatorAddress" => &self.address,
            "creatorCity" => &self.city,
            "creatorState" => &self.state,
            "creatorPostalCode" => &self.postal_code,
            "creatorCountry" => &self.country,
            "creatorPhone" => &self.phone,
            "creatorEmail" => &self.email,
            "creatorWebsite" => &self.website,
            _ => return None,
        })
    }

    /// The field a [`CreatorContact::KEYS`] key names, to change.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut String> {
        Some(match key {
            "creatorJobTitle" => &mut self.job_title,
            "creatorAddress" => &mut self.address,
            "creatorCity" => &mut self.city,
            "creatorState" => &mut self.state,
            "creatorPostalCode" => &mut self.postal_code,
            "creatorCountry" => &mut self.country,
            "creatorPhone" => &mut self.phone,
            "creatorEmail" => &mut self.email,
            "creatorWebsite" => &mut self.website,
            _ => return None,
        })
    }
}

/// Everything the app shows or searches about a photo. All fields are optional; unknown = `None`/empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    // camera
    pub make: Option<String>,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub software: Option<String>,
    // lens
    pub lens_make: Option<String>,
    pub lens_model: Option<String>,
    pub lens_serial_number: Option<String>,
    /// Min/max focal length (mm) and min/max f-number at those, as in Exif `LensSpecification`.
    pub lens_spec: Option<[f64; 4]>,
    // capture
    pub capture_time: Option<DateTime>,
    /// Seconds.
    pub exposure_time: Option<f64>,
    pub f_number: Option<f64>,
    pub iso: Option<u32>,
    /// Millimetres.
    pub focal_length: Option<f64>,
    pub focal_length_35mm: Option<f64>,
    pub flash: Option<Flash>,
    /// EV.
    pub exposure_bias: Option<f64>,
    /// Exif `ExposureProgram` code.
    pub exposure_program: Option<u16>,
    /// Exif `MeteringMode` code.
    pub metering_mode: Option<u16>,
    /// Exif `WhiteBalance` (0 auto, 1 manual).
    pub white_balance: Option<u16>,
    pub orientation: Option<Orientation>,
    pub gps: Option<Gps>,
    /// Pixel dimensions as recorded (not oriented).
    pub width: Option<u32>,
    pub height: Option<u32>,
    // descriptive / user
    pub artist: Option<String>,
    pub copyright: Option<String>,
    /// Copyright status (`xmpRights:Marked`): `true` = copyrighted, `false` = public domain, `None` = unknown.
    pub copyright_marked: Option<bool>,
    /// Rights usage terms (`xmpRights:UsageTerms`).
    pub usage_terms: Option<String>,
    /// Copyright info URL (`xmpRights:WebStatement`).
    pub copyright_url: Option<String>,
    pub title: Option<String>,
    pub caption: Option<String>,
    /// Accessibility text (`Iptc4xmpCore:AltTextAccessibility`).
    pub alt_text: Option<String>,
    /// Long accessibility description (`Iptc4xmpCore:ExtDescrAccessibility`).
    pub extended_description: Option<String>,
    /// Place within the city (`Iptc4xmpCore:Location`).
    pub sublocation: Option<String>,
    /// `photoshop:City`, `photoshop:State`, `photoshop:Country`.
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    /// The creator's contact details and job title (IPTC Core).
    #[serde(default, skip_serializing_if = "CreatorContact::is_empty")]
    pub contact: CreatorContact,
    pub keywords: Vec<String>,
    /// Hierarchical keywords, `|`-separated paths (`lr:hierarchicalSubject` convention).
    pub hierarchical_keywords: Vec<String>,
    /// -1 = rejected, 0 = unrated, 1..=5 stars.
    pub rating: Option<i8>,
    /// Colour label name (e.g. "Red").
    pub label: Option<String>,
    /// Face/pet/focus/barcode regions (`mwg-rs:Regions`), read from XMP only.
    pub regions: Vec<Region>,
    /// Where the camera focused, from its maker notes ([`focus`]; Sony only for now), on the upright photo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<FocusPoint>,
    /// Fujifilm in-camera film recipe from the MakerNote. Not written back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuji_recipe: Option<FujiRecipe>,
}

macro_rules! fill {
    ($self:ident, $o:ident, $($f:ident),*) => { $( if $self.$f.is_none() { $self.$f = $o.$f.clone(); } )* };
}
macro_rules! overlay {
    ($self:ident, $o:ident, $($f:ident),*) => { $( if $o.$f.is_some() { $self.$f = $o.$f.clone(); } )* };
}

impl Metadata {
    /// Fill every empty field of `self` from `other`.
    pub fn fill_missing(&mut self, other: &Metadata) {
        fill!(
            self,
            other,
            make,
            model,
            serial_number,
            software,
            lens_make,
            lens_model,
            lens_serial_number,
            lens_spec,
            capture_time,
            exposure_time,
            f_number,
            iso,
            focal_length,
            focal_length_35mm,
            flash,
            exposure_bias,
            exposure_program,
            metering_mode,
            white_balance,
            orientation,
            gps,
            width,
            height,
            artist,
            copyright,
            copyright_marked,
            usage_terms,
            copyright_url,
            title,
            caption,
            alt_text,
            extended_description,
            sublocation,
            city,
            state,
            country,
            rating,
            label
        );
        if self.keywords.is_empty() {
            self.keywords = other.keywords.clone();
        }
        if self.contact.is_empty() {
            self.contact.clone_from(&other.contact);
        }
        if self.hierarchical_keywords.is_empty() {
            self.hierarchical_keywords = other.hierarchical_keywords.clone();
        }
        if self.regions.is_empty() {
            self.regions = other.regions.clone();
        }
        if self.focus.is_none() {
            self.focus.clone_from(&other.focus);
        }
        if self.fuji_recipe.is_none() {
            self.fuji_recipe.clone_from(&other.fuji_recipe);
        }
    }

    /// Replace the user-editable fields (rating, label, title, caption, artist, copyright, keywords, GPS) with
    /// `other`'s where `other` has them — the XMP-over-EXIF precedence rule.
    pub fn overlay_user_fields(&mut self, other: &Metadata) {
        overlay!(self, other, rating, label, title, caption, artist, copyright, copyright_marked, usage_terms, copyright_url, gps);
        if !other.keywords.is_empty() {
            self.keywords = other.keywords.clone();
        }
        if !other.contact.is_empty() {
            self.contact.clone_from(&other.contact);
        }
        if !other.hierarchical_keywords.is_empty() {
            self.hierarchical_keywords = other.hierarchical_keywords.clone();
        }
        if !other.regions.is_empty() {
            self.regions = other.regions.clone();
        }
    }

    /// Exposure time formatted like cameras show it: `1/250`, `0.5`, `2″`, `1.3″`.
    pub fn exposure_display(&self) -> Option<String> {
        exposure_display_of(self.exposure_time?)
    }

    /// Human name of the metering mode.
    pub fn metering_name(&self) -> Option<&'static str> {
        Some(match self.metering_mode? {
            1 => "Average",
            2 => "Center-weighted average",
            3 => "Spot",
            4 => "Multi-spot",
            5 => "Pattern",
            6 => "Partial",
            255 => "Other",
            _ => "Unknown",
        })
    }

    /// Human name of the exposure program.
    pub fn exposure_program_name(&self) -> Option<&'static str> {
        Some(match self.exposure_program? {
            1 => "Manual",
            2 => "Program AE",
            3 => "Aperture-priority AE",
            4 => "Shutter speed priority AE",
            5 => "Creative (slow speed)",
            6 => "Action (high speed)",
            7 => "Portrait",
            8 => "Landscape",
            9 => "Bulb",
            _ => "Not defined",
        })
    }
}

/// An exposure time in seconds, written the way cameras show it: `1/250`, `0.5`, `2″`, `1.3″`. The third-stop
/// speeds between 1/4 s and 4 s keep their step (`0.4`, `0.8`, `1.3″`) instead of rounding to a whole one —
/// `″` marks a whole second, so a reader showing the string beside " s" must not add a second unit.
/// `None` for anything that isn't a positive, finite time.
pub fn exposure_display_of(t: f64) -> Option<String> {
    if !t.is_finite() || t <= 0.0 {
        return None;
    }
    Some(if t < 0.3 {
        format!("1/{}", (1.0 / t).round())
    } else if t < 1.0 {
        format!("{}", (t * 10.0).round() / 10.0)
    } else {
        format!("{}″", (t * 10.0).round() / 10.0)
    })
}

/// Read everything we understand from a whole file (JPEG, PNG, WebP, TIFF-based incl. DNG/raw).
/// Precedence: EXIF, then IPTC fills gaps, then XMP overrides user fields and fills the rest.
pub fn extract(bytes: &[u8]) -> Metadata {
    let e = embedded(bytes);
    let exif = e.exif.as_deref().map(read_exif).unwrap_or_default();
    let xmp = e.xmp.as_deref().and_then(|xmp| parse_xmp(xmp).ok()).map(|xmp| xmp.metadata);
    extract_embedded(e, exif, xmp)
}

/// Read metadata while reporting malformed PSD/PSB containers, embedded EXIF, or embedded XMP instead of treating
/// them as empty. Other container formats retain [`extract`]'s best-effort behavior.
pub fn try_extract(bytes: &[u8]) -> Result<Metadata, ExtractError> {
    let e = container::try_embedded(bytes)?;
    let is_psd = bytes.starts_with(b"8BPS");
    let exif = match e.exif.as_deref() {
        Some(exif) if is_psd => try_read_exif(exif).map_err(|_| ExtractError::MalformedPsd)?,
        Some(exif) => read_exif(exif),
        None => Metadata::default(),
    };
    let xmp = match e.xmp.as_deref() {
        Some(xmp) if is_psd => Some(parse_xmp(xmp).map_err(|_| ExtractError::MalformedPsd)?.metadata),
        Some(xmp) => parse_xmp(xmp).ok().map(|xmp| xmp.metadata),
        None => None,
    };
    Ok(extract_embedded(e, exif, xmp))
}

fn extract_embedded(e: Embedded, mut m: Metadata, xmp: Option<Metadata>) -> Metadata {
    if let Some(i) = e.iptc.as_deref() {
        m.fill_missing(&parse_iptc(i));
    }
    if let Some(xmp) = xmp {
        m.overlay_user_fields(&xmp);
        m.fill_missing(&xmp);
    }
    if let Some(exif) = e.exif.as_deref() {
        m.fuji_recipe = fuji::recipe_from_exif(exif);
    }
    m
}

/// Does the file embed a GPS position anywhere it can be read from? Export's "remove location"
/// asks before copying an original byte for byte. For HEIF, HEIC and AVIF that is any EXIF or XMP
/// item, whichever image it describes (a thumbnail's too), and an item that can't be read counts
/// as a location; for other files, what [`extract`] reads.
pub fn embeds_location(bytes: &[u8]) -> bool {
    heif::embeds_location(bytes).unwrap_or_else(|| extract(bytes).gps.is_some())
}

/// A container-level error reported by [`try_extract`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtractError {
    /// The PSD/PSB header, image-resource section, or embedded EXIF/XMP structure is malformed.
    MalformedPsd,
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedPsd => f.write_str("malformed PSD/PSB header, image-resource section, or embedded EXIF/XMP structure"),
        }
    }
}

impl std::error::Error for ExtractError {}

/// A lens name from its focal and aperture range (Exif LensSpecification, or a Nikon maker note's Lens), written the
/// way Lightroom shows a lens it has no name for: `50.0 mm f/1.8`, `18.0-135.0 mm f/3.5-5.6`.
pub fn lens_spec_name(spec: [f64; 4]) -> Option<String> {
    let [short, long, wide, narrow] = spec;
    if ![short, long, wide, narrow].iter().all(|v| v.is_finite() && *v > 0.0) || long < short {
        return None;
    }
    let focal = if (long - short).abs() < 0.05 { format!("{short:.1}") } else { format!("{short:.1}-{long:.1}") };
    let aperture = if (narrow - wide).abs() < 0.05 { format!("{wide:.1}") } else { format!("{wide:.1}-{narrow:.1}") };
    Some(format!("{focal} mm f/{aperture}"))
}

/// Lenient number parsing used by XMP/IPTC: `"28/10"`, `"2.8"`, `"+0.7"`.
pub(crate) fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some((n, d)) = s.split_once('/') {
        let (n, d) = (n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?);
        return (d != 0.0).then_some(n / d).filter(|v| v.is_finite());
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lens_names_from_a_spec_read_like_lightroom() {
        assert_eq!(lens_spec_name([50.0, 50.0, 1.8, 1.8]).as_deref(), Some("50.0 mm f/1.8"));
        assert_eq!(lens_spec_name([18.0, 135.0, 3.5, 5.6]).as_deref(), Some("18.0-135.0 mm f/3.5-5.6"));
        assert_eq!(lens_spec_name([24.0, 70.0, 2.8, 2.8]).as_deref(), Some("24.0-70.0 mm f/2.8"));
        assert_eq!(lens_spec_name([70.0, 210.0, 4.0, 5.6]).as_deref(), Some("70.0-210.0 mm f/4.0-5.6"));
        assert_eq!(lens_spec_name([0.0, 0.0, 0.0, 0.0]), None);
        assert_eq!(lens_spec_name([200.0, 80.0, 2.8, 2.8]), None);
        assert_eq!(lens_spec_name([f64::NAN, 50.0, 1.8, 1.8]), None);
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_number("28/10"), Some(2.8));
        assert_eq!(parse_number(" +0.7 "), Some(0.7));
        assert_eq!(parse_number("1/0"), None);
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number("inf"), None);
    }

    /// The third-stop speeds between 1/4 s and 4 s keep their step, written as cameras show them (issue
    /// #1619): 0.4 s is "0.4", not "1/2" (1/0.4 = 2.5, rounded to 2), and a whole second carries the ″.
    #[test]
    fn exposure_times_keep_their_step() {
        for (t, want) in [
            (1.0 / 250.0, "1/250"),
            (0.3, "0.3"),
            (0.4, "0.4"),
            (0.5, "0.5"),
            (0.8, "0.8"),
            (1.0, "1\u{2033}"),
            (1.3, "1.3\u{2033}"),
            (2.5, "2.5\u{2033}"),
            (30.0, "30\u{2033}"),
        ] {
            assert_eq!(exposure_display_of(t).as_deref(), Some(want), "{t} s");
            assert_eq!(Metadata { exposure_time: Some(t), ..Default::default() }.exposure_display().as_deref(), Some(want), "{t} s");
        }
        for t in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(exposure_display_of(t), None, "{t}");
        }
    }

    #[test]
    fn try_extract_keeps_non_psd_exif_best_effort() {
        let payload = b"Exif\0\0not a TIFF stream";
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        jpeg.extend_from_slice(payload);
        jpeg.extend_from_slice(&[0xff, 0xd9]);

        assert!(matches!(try_extract(&jpeg), Ok(metadata) if metadata == Metadata::default()));
    }

    #[test]
    fn merge_rules() {
        let mut a = Metadata { make: Some("A".into()), rating: Some(1), ..Default::default() };
        let b = Metadata { make: Some("B".into()), model: Some("M".into()), rating: Some(4), keywords: vec!["k".into()], ..Default::default() };
        a.fill_missing(&b);
        assert_eq!(a.make.as_deref(), Some("A"));
        assert_eq!(a.model.as_deref(), Some("M"));
        assert_eq!(a.rating, Some(1));
        a.overlay_user_fields(&b);
        assert_eq!(a.rating, Some(4));
        assert_eq!(a.make.as_deref(), Some("A"));
        assert_eq!(a.keywords, vec!["k".to_string()]);
    }
}
