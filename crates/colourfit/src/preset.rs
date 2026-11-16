//! The baseline Lightroom develop preset (XMP, `crs` namespace) contributors apply before exporting: every tone and
//! colour slider at zero, linear tone curve, no sharpening, noise reduction, lens corrections, vignette, grain or
//! crop, white balance As Shot, and no camera profile (each export folder picks its own profile).
//!
//! Sources: own design; the XMP Camera Raw settings namespace as documented in the XMP specification part 2.

/// Fixed identity of the preset (so re-importing replaces rather than duplicates it).
const UUID: &str = "6C0A1B7E2F4D4E0C9A3B5D7F1E2C4A60";

/// `crs` attributes (name, value) the preset sets.
const SETTINGS: &[(&str, &str)] = &[
    ("WhiteBalance", "As Shot"),
    ("Exposure2012", "0.00"),
    ("Contrast2012", "0"),
    ("Highlights2012", "0"),
    ("Shadows2012", "0"),
    ("Whites2012", "0"),
    ("Blacks2012", "0"),
    ("Texture", "0"),
    ("Clarity2012", "0"),
    ("Dehaze", "0"),
    ("Vibrance", "0"),
    ("Saturation", "0"),
    ("ParametricShadows", "0"),
    ("ParametricDarks", "0"),
    ("ParametricLights", "0"),
    ("ParametricHighlights", "0"),
    ("Sharpness", "0"),
    ("LuminanceSmoothing", "0"),
    ("ColorNoiseReduction", "0"),
    ("LensProfileEnable", "0"),
    ("AutoLateralCA", "0"),
    ("VignetteAmount", "0"),
    ("PostCropVignetteAmount", "0"),
    ("GrainAmount", "0"),
    ("ConvertToGrayscale", "False"),
    ("HasCrop", "False"),
];

/// The preset's XMP text.
pub fn baseline_xmp() -> String {
    let mut attrs = String::new();
    for (k, v) in SETTINGS {
        attrs.push_str(&format!("\n    crs:{k}=\"{v}\""));
    }
    let curve = |name: &str| {
        format!("   <crs:{name}>\n    <rdf:Seq>\n     <rdf:li>0, 0</rdf:li>\n     <rdf:li>255, 255</rdf:li>\n    </rdf:Seq>\n   </crs:{name}>\n")
    };
    let curves: String = ["ToneCurvePV2012", "ToneCurvePV2012Red", "ToneCurvePV2012Green", "ToneCurvePV2012Blue"].iter().map(|n| curve(n)).collect();
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:PresetType="Normal"
    crs:Cluster=""
    crs:UUID="{UUID}"
    crs:SupportsAmount="False"
    crs:SupportsColor="True"
    crs:SupportsMonochrome="True"
    crs:SupportsHighDynamicRange="True"
    crs:SupportsNormalDynamicRange="True"
    crs:SupportsSceneReferred="True"
    crs:SupportsOutputReferred="True"
    crs:CameraModelRestriction=""
    crs:Copyright=""
    crs:ContactInfo=""
    crs:Version="15.0"
    crs:ProcessVersion="11.0"{attrs}
    crs:HasSettings="True">
   <crs:Name>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Calibration baseline</rdf:li>
    </rdf:Alt>
   </crs:Name>
   <crs:ShortName>
    <rdf:Alt>
     <rdf:li xml:lang="x-default"/>
    </rdf:Alt>
   </crs:ShortName>
   <crs:SortName>
    <rdf:Alt>
     <rdf:li xml:lang="x-default"/>
    </rdf:Alt>
   </crs:SortName>
   <crs:Group>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Calibration</rdf:li>
    </rdf:Alt>
   </crs:Group>
   <crs:Description>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Neutral baseline for camera colour calibration exports</rdf:li>
    </rdf:Alt>
   </crs:Description>
{curves}  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_back_with_the_preset_importer() {
        let x = baseline_xmp();
        let p = lcc_engine::crs::preset_from_xmp(&x, "fallback").expect("preset parses");
        assert_eq!(p.name, "Calibration baseline");
        assert_eq!(p.group, "Calibration");
        let props = lcc_meta::parse_xmp(&x).unwrap().properties;
        assert_eq!(props.get("crs:PresetType").and_then(|v| v.first()).map(String::as_str), Some("Normal"));
        assert!(!props.contains_key("crs:CameraProfile"));
        // what the collector checks: the preset itself is at the baseline
        assert!(crate::dataset::baseline_problem(&props).is_none());
    }
}
