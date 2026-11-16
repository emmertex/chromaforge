//! Portrait retouch of the parts of a person (beyond Lightroom), ported from OneSlips: Teeth so far,
//! each a sub-section of the Edit panel's Retouch section with plain sliders, a person (everyone, or
//! one, counted from the left by their face) and its own eye. Smooth Skin ([`crate::SkinSmoothing`])
//! is the Skin sub-section.
//!
//! The sliders are develop settings ([`Retouch`], in the Retouch settings group: copied, pasted,
//! synced and saved in presets). The parts the models found on a photo ([`RetouchAnalysis`]) are the
//! photo's own data, like Smooth Skin's skin: in no settings group, found again when a section asks
//! for a part or person it doesn't hold, so rendering and exporting never need the models.

use lcc_geom::Orientation;
use serde::{Deserialize, Serialize};

use crate::SegMask;

/// A part of a person a retouch section works on, as the People masks find it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RetouchPart {
    Teeth,
}

impl RetouchPart {
    /// Its People part name (`lcc_segment::people::PeopleRequest::parse`).
    pub fn people_name(self) -> &'static str {
        match self {
            RetouchPart::Teeth => "Teeth",
        }
    }
}

/// One part to find: which, and whose (0: everyone; n: the n-th person from the left).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PartRequest {
    pub part: RetouchPart,
    pub person: u32,
}

fn on(v: f64) -> bool {
    v.is_finite() && v != 0.0
}

/// Teeth: whiter (less yellow) and a little brighter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetouchTeeth {
    /// 0: everyone; n: the n-th person from the left.
    pub person: u32,
    /// 0..100: takes the yellow out.
    pub whiten: f64,
    /// 0..100: lifts their brightness gently.
    pub brighten: f64,
}

impl RetouchTeeth {
    pub fn active(&self) -> bool {
        on(self.whiten) || on(self.brighten)
    }
}

/// The retouch sections' settings. All zero is off (and not written in a photo's settings, so
/// settings from before it existed keep their JSON and hash). Every field is written otherwise, so
/// copying a section that is off switches it off where it is pasted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Retouch {
    pub teeth: RetouchTeeth,
}

impl Retouch {
    /// Most people a section tells apart (Person 1 to 4, as the People mask offers).
    pub const MAX_PERSON: u32 = 4;
    /// The sections, by their key (also the section id of its eye).
    pub const SECTIONS: [&'static str; 1] = ["teeth"];

    pub fn is_default(&self) -> bool {
        *self == Retouch::default()
    }

    /// Whether section `key` changes anything.
    pub fn section_active(&self, key: &str) -> bool {
        match key {
            "teeth" => self.teeth.active(),
            _ => false,
        }
    }

    /// The parts section `key` works on, and whose.
    pub fn section_parts(&self, key: &str) -> Vec<PartRequest> {
        let req = |part, person: u32| PartRequest { part, person: person.min(Retouch::MAX_PERSON) };
        match key {
            "teeth" => vec![req(RetouchPart::Teeth, self.teeth.person)],
            _ => Vec::new(),
        }
    }

    /// The parts of the active sections whose eye is on (`on(section key)`), sorted.
    pub fn wanted(&self, on: impl Fn(&str) -> bool) -> Vec<PartRequest> {
        let mut out: Vec<PartRequest> =
            Retouch::SECTIONS.iter().filter(|k| self.section_active(k) && on(k)).flat_map(|k| self.section_parts(k)).collect();
        out.sort();
        out.dedup();
        out
    }
}

/// The sections' sliders: (section, parameter name of `retouch.set`, develop control), in panel
/// order.
pub const SLIDERS: &[(&str, &str, &str)] = &[("teeth", "whiten", "retouch.teethWhiten"), ("teeth", "brighten", "retouch.teethBrighten")];

/// The develop control holding section `key`'s person.
pub fn person_control(key: &str) -> String {
    format!("retouch.{key}Person")
}

/// A part found on a photo (`seg: None`: not there, e.g. no teeth showing, or no such person).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetouchMask {
    pub part: RetouchPart,
    pub person: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seg: Option<SegMask>,
}

/// Most parts a stored analysis keeps (a hostile document can't make rendering explode).
pub const MAX_RETOUCH_MASKS: usize = 24;

/// The parts the models found for the retouch sections on one photo, with the photo turned to
/// `orientation`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetouchAnalysis {
    pub orientation: Orientation,
    #[serde(default, deserialize_with = "de_masks")]
    pub masks: Vec<RetouchMask>,
}

impl RetouchAnalysis {
    /// The part `r`, when it was looked for (its `seg` says whether it was found).
    pub fn mask(&self, r: PartRequest) -> Option<&RetouchMask> {
        self.masks.iter().find(|m| m.part == r.part && m.person == r.person)
    }

    /// The parts of `wanted` it doesn't hold for a photo turned to `orientation` (all of them
    /// when it was made for another orientation).
    pub fn missing(&self, wanted: &[PartRequest], orientation: Orientation) -> Vec<PartRequest> {
        if self.orientation != orientation {
            return wanted.to_vec();
        }
        wanted.iter().filter(|r| self.mask(**r).is_none()).copied().collect()
    }

    /// This analysis with `other`'s parts added (replacing its own of the same part and person),
    /// when both are for the same orientation; else `other`. At most [`MAX_RETOUCH_MASKS`] parts
    /// are kept, the newest.
    pub fn merged(&self, other: &RetouchAnalysis) -> RetouchAnalysis {
        if self.orientation != other.orientation {
            return other.clone();
        }
        let mut masks: Vec<RetouchMask> =
            self.masks.iter().filter(|m| other.mask(PartRequest { part: m.part, person: m.person }).is_none()).cloned().collect();
        masks.extend(other.masks.iter().cloned());
        let skip = masks.len().saturating_sub(MAX_RETOUCH_MASKS);
        RetouchAnalysis { orientation: other.orientation, masks: masks.into_iter().skip(skip).collect() }
    }
}

/// Keep at most [`MAX_RETOUCH_MASKS`] parts (the rest are skipped unread; an unknown or damaged
/// one costs only itself).
fn de_masks<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<RetouchMask>, D::Error> {
    struct Capped;
    impl<'de> serde::de::Visitor<'de> for Capped {
        type Value = Vec<RetouchMask>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a list of parts")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<RetouchMask>, A::Error> {
            let mut out = Vec::new();
            while out.len() < MAX_RETOUCH_MASKS {
                match seq.next_element::<serde_json::Value>()? {
                    Some(v) => out.extend(serde_json::from_value::<RetouchMask>(v).ok()),
                    None => return Ok(out),
                }
            }
            while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
            Ok(out)
        }
    }
    d.deserialize_seq(Capped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DevelopSettings, SettingsGroup, controls, extract_groups};
    use serde_json::json;

    fn found(part: RetouchPart, person: u32, there: bool) -> RetouchMask {
        RetouchMask { part, person, seg: there.then(|| SegMask::from_logits(2, &[4.0, -4.0, 4.0, -4.0])) }
    }

    #[test]
    fn off_is_not_written_and_every_field_is_when_on() {
        let s = DevelopSettings::default();
        assert!(s.to_json().get("retouch").is_none() && s.to_json().get("retouch_analysis").is_none());
        // settings written before Retouch existed read back the same, with the same hash
        let old = DevelopSettings::from_json(&s.to_json()).unwrap();
        assert_eq!(old.hash64(), s.hash64());
        let mut on = s.clone();
        assert!(controls::set(&mut on, "retouch.teethWhiten", 30.0));
        assert_eq!(on.to_json()["retouch"]["teeth"], json!({"person": 0, "whiten": 30.0, "brighten": 0.0}));
        assert_ne!(on.hash64(), s.hash64());
        // the person is a control too, clamped to 0..4
        assert!(controls::set(&mut on, "retouch.teethPerson", 9.0));
        assert_eq!(on.retouch.teeth.person, 4);
        assert_eq!(controls::get(&on, &person_control("teeth")), Some(4.0));
    }

    #[test]
    fn sections_want_their_parts_when_active_and_their_eye_is_on() {
        let mut r = Retouch::default();
        assert!(r.wanted(|_| true).is_empty());
        r.teeth = RetouchTeeth { person: 2, whiten: 20.0, brighten: 0.0 };
        assert_eq!(r.wanted(|_| true), vec![PartRequest { part: RetouchPart::Teeth, person: 2 }]);
        assert!(r.wanted(|k| k != "teeth").is_empty(), "its eye is off");
        r.teeth.whiten = f64::NAN;
        assert!(!r.section_active("teeth"));
    }

    #[test]
    fn an_analysis_says_what_is_missing_and_merges_newer_parts() {
        let teeth = |p| PartRequest { part: RetouchPart::Teeth, person: p };
        let a = RetouchAnalysis { orientation: Orientation::Normal, masks: vec![found(RetouchPart::Teeth, 0, false)] };
        assert!(a.missing(&[teeth(0)], Orientation::Normal).is_empty(), "looked for, not found: not asked again");
        assert_eq!(a.missing(&[teeth(0), teeth(1)], Orientation::Normal), vec![teeth(1)]);
        assert_eq!(a.missing(&[teeth(0)], Orientation::Rotate90), vec![teeth(0)], "turned: everything again");
        let b =
            RetouchAnalysis { orientation: Orientation::Normal, masks: vec![found(RetouchPart::Teeth, 0, true), found(RetouchPart::Teeth, 1, true)] };
        let m = a.merged(&b);
        assert_eq!(m.masks.len(), 2);
        assert!(m.mask(teeth(0)).is_some_and(|m| m.seg.is_some()), "the newer replaces the older");
        let turned = RetouchAnalysis { orientation: Orientation::Rotate90, ..b.clone() };
        assert_eq!(a.merged(&turned), turned);
        let many = RetouchAnalysis { orientation: Orientation::Normal, masks: (0..40).map(|p| found(RetouchPart::Teeth, p, true)).collect() };
        assert_eq!(a.merged(&many).masks.len(), MAX_RETOUCH_MASKS);
    }

    #[test]
    fn damaged_or_hostile_parts_cost_only_themselves() {
        let masks: Vec<serde_json::Value> =
            (0..100).map(|i| if i == 1 { json!({"part": "elbows", "person": 0}) } else { json!({"part": "teeth", "person": 0}) }).collect();
        let v = json!({"orientation": "Normal", "masks": masks});
        let a: RetouchAnalysis = serde_json::from_value(v).unwrap();
        assert_eq!(a.masks.len(), MAX_RETOUCH_MASKS, "capped, the unknown one skipped");
        assert!(a.masks.iter().all(|m| m.part == RetouchPart::Teeth));
        let s = DevelopSettings::from_json(&json!({"light": {"exposure": 1.0}, "retouch": {"teeth": {"whiten": "lots"}}, "retouch_analysis": 7}))
            .unwrap();
        assert_eq!(s.light.exposure, 1.0);
        assert!(s.retouch.is_default() && s.retouch_analysis.is_none());
    }

    #[test]
    fn the_retouch_group_copies_the_sliders_never_the_parts_found() {
        let mut a = DevelopSettings::default();
        a.retouch.teeth = RetouchTeeth { person: 1, whiten: 40.0, brighten: 10.0 };
        a.retouch_analysis = Some(RetouchAnalysis { orientation: Orientation::Normal, masks: vec![found(RetouchPart::Teeth, 1, true)] });
        let copied = extract_groups(&a, &[SettingsGroup::SmoothSkin]);
        assert!(copied.get("retouch_analysis").is_none());
        let mut target = DevelopSettings::default();
        target.retouch.teeth.brighten = 70.0;
        let pasted = target.merged(&copied).unwrap();
        assert_eq!(pasted.retouch, a.retouch);
        let off = extract_groups(&DevelopSettings::default(), &[SettingsGroup::SmoothSkin]);
        assert!(pasted.merged(&off).unwrap().retouch.is_default());
    }
}
