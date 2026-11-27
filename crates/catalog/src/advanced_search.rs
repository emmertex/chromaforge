//! Advanced Search (issue #846): a structured, serializable query over the catalog's fields,
//! organised in collapsible categories, lowered into the library's [`Filter`].
//!
//! An [`AdvancedSearch`] is the query the Advanced Search panel edits and the
//! `library.advancedSearch` command runs. [`AdvancedSearch::to_filter`] turns it into the one
//! [`Filter`] the grid already renders — the global words go to [`Filter::text`], the keyword,
//! people and album picks to their own filter fields, and every technical condition (dimensions,
//! camera, lens, exposure, location…) to a smart-album [`RuleSet`] of mode "all" — so the results
//! are an ordinary filtered grid view, and matching, keeping and follow reuse the filter
//! machinery wholesale.
//!
//! A query is one search group (its categories' conditions, combined as [`AdvancedSearch::mode`] says: all, any,
//! none, or not all of them) plus further groups ([`AdvancedSearch::groups`]), combined as
//! [`AdvancedSearch::groups_match`] says, as digiKam's Advanced Search does (issue #1135). A query of one group whose
//! conditions must all match lowers as it always did; any other is lowered entirely into nested rule groups.
//!
//! Categories the catalog cannot support yet stay declared in the query ([`MediaCategory`]'s
//! codec and frame rate): setting them is an actionable error from [`AdvancedSearch::to_filter`],
//! never a crash, and the panel shows their inputs disabled.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{AlbumId, Filter, MatchOp, RatingOp, Rule, RuleSet, rules::Match};

/// The file types a [`FileCategory`] can narrow to (the `kind` rule's choices).
pub const KINDS: &[&str] = &["image", "raw", "video"];

/// The aspect ratios a [`PropertiesCategory`] can narrow to (the `aspect` rule's choices).
pub const ASPECTS: &[&str] = &["landscape", "portrait", "square"];

/// The color labels a [`PropertiesCategory`] can narrow to, plus `"none"` (unlabeled photos),
/// in [`crate::ColorLabel::ALL`] order.
pub const LABELS: &[&str] = &["red", "yellow", "green", "blue", "purple", "none"];

/// The pick flags a [`PropertiesCategory`] can narrow to (the `flag` rule's choices).
pub const FLAGS: &[&str] = &["pick", "reject", "none"];

/// How the conditions of one search group combine (issue #1135).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchMatch {
    /// Every condition is met.
    #[default]
    All,
    /// At least one condition is met.
    Any,
    /// No condition is met.
    None,
    /// At least one condition is not met.
    NotAll,
}

impl SearchMatch {
    /// Every match, in menu order.
    pub const ALL: [SearchMatch; 4] = [SearchMatch::All, SearchMatch::Any, SearchMatch::None, SearchMatch::NotAll];

    /// `conditions` combined this way, as one rule group.
    fn group(self, conditions: Vec<Rule>) -> RuleSet {
        match self {
            SearchMatch::All => RuleSet { mode: Match::All, rules: conditions },
            SearchMatch::Any => RuleSet { mode: Match::Any, rules: conditions },
            SearchMatch::None => RuleSet { mode: Match::None, rules: conditions },
            // "not all of them" is "none of (all of them)"
            SearchMatch::NotAll => RuleSet { mode: Match::None, rules: vec![Rule::Group { group: RuleSet { mode: Match::All, rules: conditions } }] },
        }
    }
}

/// A structured Advanced Search query: global words plus optional rule categories, combined as
/// [`AdvancedSearch::mode`] says (all of them by default), plus further search groups. Every field
/// defaults to "no constraint"; a query with none set matches every photo, like an empty [`Filter`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct AdvancedSearch {
    /// Find items that have associated all these words: every whitespace-separated token must
    /// appear in the file name, title, caption, keywords, camera, lens, location or format (the
    /// free-text search, [`Filter::text`]).
    #[serde(default)]
    pub words: String,
    /// File, album and tags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileCategory>,
    /// Item properties: dimensions, rating, color label, aspect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<PropertiesCategory>,
    /// Caption and title text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<DescriptionCategory>,
    /// Photograph information: camera, lens, exposure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photograph: Option<PhotographCategory>,
    /// Dates: when a photo was taken (a span, or a month and a day whatever the year), imported and edited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dates: Option<DatesCategory>,
    /// Geographic position as the catalog reads it: the location fields and "has GPS".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<LocationCategory>,
    /// Audio/video properties where supported (duration; codec and frame rate are declared but
    /// unsupported).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaCategory>,
    /// Face regions: a face count and named people.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faces: Option<FacesCategory>,
    /// How this group's conditions combine: all of them (the default), any, none, or not all (issue #1135). Each
    /// word, each category field set, and each keyword or people list counts as one condition.
    #[serde(rename = "match", skip_serializing_if = "is_default")]
    pub mode: SearchMatch,
    /// Further search groups, each a query of its own (without groups of its own), combined with this one as
    /// [`AdvancedSearch::groups_match`] says.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<AdvancedSearch>,
    /// How the search groups combine: a photo matches every group (`all`, the default) or at least one (`any`).
    #[serde(skip_serializing_if = "is_default")]
    pub groups_match: GroupsMatch,
}

/// How the search groups of a query combine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GroupsMatch {
    /// A photo matches every group.
    #[default]
    All,
    /// A photo matches at least one group.
    Any,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Most search groups in one query: a hostile query can't make every photo walk an unbounded list.
pub const MAX_GROUPS: usize = 64;

/// An inclusive numeric range: both ends optional, `min` may not exceed `max`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Range {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl Range {
    /// Whether the range constrains nothing.
    pub fn is_empty(&self) -> bool {
        self.min.is_none() && self.max.is_none()
    }
}

/// A shutter-speed range in camera notation (`1/250`, `2`) or plain seconds (`0.004`); both ends
/// optional.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ShutterRange {
    pub min: Option<String>,
    pub max: Option<String>,
}

impl ShutterRange {
    fn is_empty(&self) -> bool {
        self.min.is_none() && self.max.is_none()
    }
}

/// File, album and tags: what a photo is and where it lives.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct FileCategory {
    /// One of [`KINDS`] (`image`, `raw`, `video`).
    pub kind: Option<String>,
    /// Part of the file name (case-insensitive).
    #[serde(default)]
    pub name: String,
    /// Part of the folder path (case-insensitive).
    #[serde(default)]
    pub folder: String,
    /// A collection by id: photos in it, not in its folders of albums. The id must name an album
    /// the library has — the caller checks ([`AdvancedSearch::to_filter`] stays catalog-free).
    pub album: Option<AlbumId>,
    /// Keywords; hierarchical keywords match their children too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// How [`FileCategory::keywords`] combine: `any` (a photo with at least one) or `all`
    /// (a photo with every one).
    pub keyword_op: MatchOp,
    /// Photos without a keyword too (issue #1135): alone, only those; beside keywords, those or the keywords' photos.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub untagged: bool,
    /// File formats picked from the catalog's (`JPEG`, `CR3`, …), any of them; exact, case-insensitive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<String>,
}

impl FileCategory {
    fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.name.trim().is_empty()
            && self.folder.trim().is_empty()
            && self.album.is_none()
            && self.keywords.iter().all(|k| k.trim().is_empty())
            && !self.untagged
            && self.formats.iter().all(|f| f.trim().is_empty())
    }
}

/// A span of dates, each end a year (`2026`), a month (`2026-05`) or a day (`2026-05-14`), either left out; both
/// ends included.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DateSpan {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

impl DateSpan {
    pub fn is_empty(&self) -> bool {
        self.from.as_deref().is_none_or(|d| d.trim().is_empty()) && self.to.as_deref().is_none_or(|d| d.trim().is_empty())
    }
}

/// Dates (issue #1135): when a photo was taken, imported and last edited.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct DatesCategory {
    /// Taken in this span.
    #[serde(skip_serializing_if = "DateSpan::is_empty")]
    pub captured: DateSpan,
    /// Taken in this month (1–12) of any year.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub month: Option<u8>,
    /// Taken on this day of the month (1–31), any month: with `month`, "on this day".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day: Option<u8>,
    /// Imported in this span.
    #[serde(skip_serializing_if = "DateSpan::is_empty")]
    pub imported: DateSpan,
    /// Last edited in this span.
    #[serde(skip_serializing_if = "DateSpan::is_empty")]
    pub edited: DateSpan,
}

impl DatesCategory {
    fn is_empty(&self) -> bool {
        self.captured.is_empty() && self.month.is_none() && self.day.is_none() && self.imported.is_empty() && self.edited.is_empty()
    }
}

/// Item properties: dimensions and shape, rating, color label.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PropertiesCategory {
    /// Width in pixels (the sensor's or the crop's, as the rules read it).
    pub width: Range,
    /// Height in pixels.
    pub height: Range,
    /// Megapixels.
    pub megapixels: Range,
    /// Stars, 0–5.
    pub rating: Option<u8>,
    /// How [`PropertiesCategory::rating`] compares.
    pub rating_op: RatingOp,
    /// One of [`LABELS`].
    pub label: Option<String>,
    /// One of [`ASPECTS`].
    pub aspect: Option<String>,
    /// The pick flag: one of [`FLAGS`] (issue #1135).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    /// File size, megabytes (10⁶ bytes).
    #[serde(skip_serializing_if = "Range::is_empty")]
    pub file_size_mb: Range,
}

impl PropertiesCategory {
    fn is_empty(&self) -> bool {
        self.width.is_empty()
            && self.height.is_empty()
            && self.megapixels.is_empty()
            && self.rating.is_none()
            && self.rating_op == RatingOp::default()
            && self.label.is_none()
            && self.aspect.is_none()
            && self.flag.is_none()
            && self.file_size_mb.is_empty()
    }
}

/// Caption and title text (case-insensitive substrings).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct DescriptionCategory {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub caption: String,
}

impl DescriptionCategory {
    fn is_empty(&self) -> bool {
        self.title.trim().is_empty() && self.caption.trim().is_empty()
    }
}

/// Photograph information: the camera, the lens and the exposure.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PhotographCategory {
    /// Camera make or model, as a case-insensitive substring of the camera name.
    #[serde(default)]
    pub camera: String,
    /// Lens, as a case-insensitive substring.
    #[serde(default)]
    pub lens: String,
    /// Focal length, mm.
    pub focal_mm: Range,
    /// ISO speed.
    pub iso: Range,
    /// Aperture, f-number (`2.8` for f/2.8).
    pub aperture: Range,
    /// Shutter speed ([`ShutterRange`]).
    pub shutter: ShutterRange,
    /// Cameras picked from the catalog's, any of them; exact, case-insensitive (issue #1135).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cameras: Vec<String>,
    /// Lenses picked from the catalog's, any of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lenses: Vec<String>,
    /// Photos with no camera recorded too: alone, only those; beside cameras, those or the cameras' photos.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_camera: bool,
    /// Photos with no lens recorded too, as [`PhotographCategory::no_camera`] is for cameras.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_lens: bool,
    /// Whether the flash fired (`None` = either).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flash: Option<bool>,
    /// Exposure bias (compensation), EV; negative values allowed.
    #[serde(skip_serializing_if = "Range::is_empty")]
    pub exposure_bias: Range,
}

impl PhotographCategory {
    fn is_empty(&self) -> bool {
        self.camera.trim().is_empty()
            && self.lens.trim().is_empty()
            && self.focal_mm.is_empty()
            && self.iso.is_empty()
            && self.aperture.is_empty()
            && self.shutter.is_empty()
            && self.cameras.iter().all(|c| c.trim().is_empty())
            && self.lenses.iter().all(|l| l.trim().is_empty())
            && !self.no_camera
            && !self.no_lens
            && self.flash.is_none()
            && self.exposure_bias.is_empty()
    }
}

/// Geographic position as the catalog reads it: the IPTC location fields and whether a photo
/// carries GPS coordinates. (Coordinate-radius search is not supported yet.)
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct LocationCategory {
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub sublocation: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub country: String,
    /// `true`: photos with GPS coordinates, `false`: without.
    pub has_gps: Option<bool>,
}

impl LocationCategory {
    fn is_empty(&self) -> bool {
        self.location.trim().is_empty()
            && self.sublocation.trim().is_empty()
            && self.city.trim().is_empty()
            && self.state.trim().is_empty()
            && self.country.trim().is_empty()
            && self.has_gps.is_none()
    }
}

/// Audio/video properties. Only the duration is supported: a codec or frame-rate condition is an
/// "unsupported" error, and the panel keeps those inputs disabled.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaCategory {
    /// Duration, seconds.
    pub duration_s: Range,
    /// Unsupported: the video codec (declared for forward compatibility).
    pub codec: Option<String>,
    /// Unsupported: the frame rate (declared for forward compatibility).
    pub frame_rate: Option<f64>,
}

impl MediaCategory {
    fn is_empty(&self) -> bool {
        self.duration_s.is_empty() && self.codec.is_none() && self.frame_rate.is_none()
    }
}

/// Face regions: how many detected faces a photo has, and who is named on them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct FacesCategory {
    /// A face-count condition: a count (`2`), a minimum (`3+`, `>=3`), a maximum (`<=2`), a range
    /// (`1-5`) or `none` / `any` — the forms [`crate::parse_face_spec`] reads.
    pub count: Option<String>,
    /// People (named face regions); hierarchical names compare whole, case-insensitively.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<String>,
    /// How [`FacesCategory::people`] combine.
    pub people_op: MatchOp,
}

impl FacesCategory {
    fn is_empty(&self) -> bool {
        self.count.as_deref().is_none_or(|c| c.trim().is_empty()) && self.people.iter().all(|p| p.trim().is_empty())
    }
}

impl AdvancedSearch {
    /// Whether the query constrains nothing at all.
    pub fn is_empty(&self) -> bool {
        self.group_is_empty() && self.groups.iter().all(AdvancedSearch::group_is_empty)
    }

    /// Whether this group's own categories constrain nothing (its further groups aside).
    fn group_is_empty(&self) -> bool {
        self.words.trim().is_empty()
            && self.file.as_ref().is_none_or(FileCategory::is_empty)
            && self.properties.as_ref().is_none_or(PropertiesCategory::is_empty)
            && self.description.as_ref().is_none_or(DescriptionCategory::is_empty)
            && self.photograph.as_ref().is_none_or(PhotographCategory::is_empty)
            && self.dates.as_ref().is_none_or(DatesCategory::is_empty)
            && self.location.as_ref().is_none_or(LocationCategory::is_empty)
            && self.media.as_ref().is_none_or(MediaCategory::is_empty)
            && self.faces.as_ref().is_none_or(FacesCategory::is_empty)
    }

    /// The query as the [`Filter`] the grid renders (see the module docs). A value that can't
    /// match anything — a non-finite or negative number, a rating above 5, an unknown label or
    /// aspect, a shutter speed that isn't a time, a face-count spec that can't be read, a range
    /// whose ends are reversed, or an unsupported audio/video condition — is an actionable error
    /// naming the field, never an empty grid or a panic.
    pub fn to_filter(&self) -> Result<Filter, String> {
        if self.groups.is_empty() && self.mode == SearchMatch::All && !self.untagged_or_keywords() {
            return self.all_of_one_group();
        }
        if self.groups.len() > MAX_GROUPS {
            return Err(format!("search groups: at most {MAX_GROUPS}"));
        }
        if self.groups.iter().any(|g| !g.groups.is_empty()) {
            return Err("search groups: a group can't hold groups of its own".into());
        }
        let mut sets = Vec::new();
        for (n, group) in std::iter::once(self).chain(&self.groups).enumerate() {
            let conditions = group.conditions().map_err(|e| if n == 0 { e } else { format!("search group {}: {e}", n + 1) })?;
            // a group with no condition constrains nothing, whatever its match
            if !conditions.is_empty() {
                sets.push(Rule::Group { group: group.mode.group(conditions) });
            }
        }
        if sets.is_empty() {
            return Ok(Filter::default());
        }
        let mode = match self.groups_match {
            GroupsMatch::All => Match::All,
            GroupsMatch::Any => Match::Any,
        };
        Ok(Filter { rule_set: Some(RuleSet { mode, rules: sets }), ..Filter::default() })
    }

    /// One group's conditions, each one rule (issue #1135): a word, a category field, the keyword or people list
    /// (combined as its own any / all says), the collection, the face count. Checked as [`Self::to_filter`] checks
    /// them.
    fn conditions(&self) -> Result<Vec<Rule>, String> {
        let mut out: Vec<Rule> = Vec::new();
        let field = |field: &str, op: &str, value: Value| Rule::Field { field: field.to_string(), op: op.to_string(), value };
        let list = |op: MatchOp, rules: Vec<Rule>| -> Option<Rule> {
            match rules.len() {
                0 => None,
                1 => rules.into_iter().next(),
                _ => Some(Rule::Group { group: RuleSet { mode: if op == MatchOp::All { Match::All } else { Match::Any }, rules } }),
            }
        };
        out.extend(self.words.split_whitespace().map(|w| field("text", "contains", json!(w))));
        if let Some(f) = &self.file {
            if let Some(album) = f.album {
                out.push(field("album", "is", json!(album.0)));
            }
            let keywords = trimmed(&f.keywords).unwrap_or_default().into_iter().map(|k| field("keywords", "is", json!(k))).collect();
            let keywords = list(f.keyword_op, keywords);
            if f.untagged {
                let none = field("keywords", "isEmpty", Value::Null);
                out.extend(list(MatchOp::Any, keywords.into_iter().chain([none]).collect()));
            } else {
                out.extend(keywords);
            }
        }
        out.extend(self.pick_rules());
        out.extend(self.field_rules(false)?.into_iter().map(rule_of));
        if let Some(f) = &self.faces {
            if let Some(spec) = f.count.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                let spec = crate::query::parse_face_spec(spec).map_err(|e| format!("faces count: {e}"))?;
                let mut ends = vec![field("faceCount", "gte", json!(spec.min))];
                if let Some(max) = spec.max {
                    ends.push(field("faceCount", "lte", json!(max)));
                }
                out.extend(list(MatchOp::All, ends));
            }
            let people = trimmed(&f.people).unwrap_or_default().into_iter().map(|p| field("person", "is", json!(p))).collect();
            out.extend(list(f.people_op, people));
        }
        Ok(out)
    }

    /// The categories' field conditions as `{"field", "op", "value"}` triples: everything but the words, the
    /// collection, the people and the face count, and the keywords only when `keywords` (a group of one search
    /// whose conditions all match lists them here, as it always has).
    fn field_rules(&self, keywords: bool) -> Result<Vec<Value>, String> {
        let mut rules: Vec<Value> = Vec::new();
        if let Some(f) = &self.file
            && !f.is_empty()
        {
            if let Some(kind) = f.kind.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
                if !KINDS.contains(&kind) {
                    return Err(format!("file kind: unknown type {kind:?} (image, raw or video)"));
                }
                rules.push(json!({"field": "kind", "op": "is", "value": kind}));
            }
            if let Some(rule) = text_rule("fileName", &f.name) {
                rules.push(rule);
            }
            if let Some(rule) = text_rule("folder", &f.folder) {
                rules.push(rule);
            }
            if keywords && let Some(list) = trimmed(&f.keywords) {
                for k in list {
                    rules.push(json!({"field": "keywords", "op": "contains", "value": k}));
                }
            }
        }
        if let Some(p) = &self.properties
            && !p.is_empty()
        {
            range_rules(&mut rules, "width", &p.width)?;
            range_rules(&mut rules, "height", &p.height)?;
            range_rules(&mut rules, "megapixels", &p.megapixels)?;
            if let Some(rating) = p.rating {
                if rating > 5 {
                    return Err(format!("rating: {rating} is above 5 stars"));
                }
                let op = match p.rating_op {
                    RatingOp::AtLeast => "gte",
                    RatingOp::Exactly => "is",
                    RatingOp::AtMost => "lte",
                };
                rules.push(json!({"field": "rating", "op": op, "value": rating}));
            }
            if let Some(label) = p.label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
                if !LABELS.contains(&label) {
                    return Err(format!("color label: unknown label {label:?} (red, yellow, green, blue, purple or none)"));
                }
                rules.push(json!({"field": "label", "op": "is", "value": label}));
            }
            if let Some(aspect) = p.aspect.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
                if !ASPECTS.contains(&aspect) {
                    return Err(format!("aspect: unknown ratio {aspect:?} (landscape, portrait or square)"));
                }
                rules.push(json!({"field": "aspect", "op": "is", "value": aspect}));
            }
            if let Some(flag) = p.flag.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
                if !FLAGS.contains(&flag) {
                    return Err(format!("pick flag: unknown flag {flag:?} (pick, reject or none)"));
                }
                rules.push(json!({"field": "flag", "op": "is", "value": flag}));
            }
            range_rules(&mut rules, "fileSize", &p.file_size_mb)?;
        }
        if let Some(d) = &self.description
            && !d.is_empty()
        {
            if let Some(rule) = text_rule("title", &d.title) {
                rules.push(rule);
            }
            if let Some(rule) = text_rule("caption", &d.caption) {
                rules.push(rule);
            }
        }
        if let Some(p) = &self.photograph
            && !p.is_empty()
        {
            if let Some(rule) = text_rule("camera", &p.camera) {
                rules.push(rule);
            }
            if let Some(rule) = text_rule("lens", &p.lens) {
                rules.push(rule);
            }
            range_rules(&mut rules, "focalLength", &p.focal_mm)?;
            range_rules(&mut rules, "iso", &p.iso)?;
            range_rules(&mut rules, "aperture", &p.aperture)?;
            shutter_rules(&mut rules, &p.shutter)?;
            if let Some(fired) = p.flash {
                rules.push(json!({"field": "flash", "op": "is", "value": fired}));
            }
            signed_range_rules(&mut rules, "exposureBias", &p.exposure_bias)?;
        }
        if let Some(d) = &self.dates
            && !d.is_empty()
        {
            date_rules(&mut rules, "captureDate", &d.captured)?;
            if let Some(month) = d.month {
                if !(1..=12).contains(&month) {
                    return Err(format!("capture month: {month} is not a month (1–12)"));
                }
                rules.push(json!({"field": "captureMonth", "op": "is", "value": month}));
            }
            if let Some(day) = d.day {
                if !(1..=31).contains(&day) {
                    return Err(format!("capture day: {day} is not a day of the month (1–31)"));
                }
                rules.push(json!({"field": "captureDay", "op": "is", "value": day}));
            }
            date_rules(&mut rules, "importDate", &d.imported)?;
            date_rules(&mut rules, "editDate", &d.edited)?;
        }
        if let Some(l) = &self.location
            && !l.is_empty()
        {
            for (field, value) in
                [("location", &l.location), ("sublocation", &l.sublocation), ("city", &l.city), ("state", &l.state), ("country", &l.country)]
            {
                if let Some(rule) = text_rule(field, value) {
                    rules.push(rule);
                }
            }
            if let Some(has) = l.has_gps {
                rules.push(json!({"field": "hasGps", "op": "is", "value": has}));
            }
        }
        if let Some(m) = &self.media {
            if m.codec.is_some() {
                return Err("audio/video codec: filtering by codec is not supported yet".into());
            }
            if m.frame_rate.is_some() {
                return Err("audio/video frame rate: filtering by frame rate is not supported yet".into());
            }
            if !m.duration_s.is_empty() {
                range_rules(&mut rules, "duration", &m.duration_s)?;
            }
        }
        Ok(rules)
    }

    /// [`Self::to_filter`] for a query of one group whose conditions must all match: the words, keywords, people,
    /// collection and face count go to their own filter fields, the rest to one rule set.
    /// Whether "without keywords" is asked for beside picked keywords: one group's All can't say "these or none" by
    /// the plain filter's keyword list, so the query goes the rule way.
    fn untagged_or_keywords(&self) -> bool {
        self.file.as_ref().is_some_and(|f| f.untagged && trimmed(&f.keywords).is_some())
    }

    /// The values picked from the catalog's lists (issue #1135): each list one condition, matched by any of its
    /// values exactly (case-insensitively), or by an empty field when "without" is ticked beside it.
    fn pick_rules(&self) -> Vec<Rule> {
        let field = |field: &str, op: &str, value: Value| Rule::Field { field: field.to_string(), op: op.to_string(), value };
        let any = |name: &str, values: &[String], none: bool| -> Option<Rule> {
            let mut rules: Vec<Rule> = trimmed(values).unwrap_or_default().into_iter().map(|v| field(name, "is", json!(v))).collect();
            if none {
                rules.push(field(name, "isEmpty", Value::Null));
            }
            match rules.len() {
                0 => None,
                1 => rules.into_iter().next(),
                _ => Some(Rule::Group { group: RuleSet { mode: Match::Any, rules } }),
            }
        };
        let mut out = Vec::new();
        if let Some(f) = &self.file {
            out.extend(any("format", &f.formats, false));
        }
        if let Some(p) = &self.photograph {
            out.extend(any("camera", &p.cameras, p.no_camera));
            out.extend(any("lens", &p.lenses, p.no_lens));
        }
        out
    }

    fn all_of_one_group(&self) -> Result<Filter, String> {
        let rules = self.field_rules(true)?;
        let mut filter = Filter { text: self.words.trim().to_string(), ..Filter::default() };
        if let Some(f) = &self.file
            && !f.is_empty()
        {
            if let Some(album) = f.album {
                filter.album = Some(album);
            }
            if let Some(list) = trimmed(&f.keywords) {
                filter.keywords = list.iter().map(|k| k.to_string()).collect();
                filter.keyword_op = f.keyword_op;
            }
        }
        if let Some(f) = &self.faces {
            if let Some(spec) = f.count.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                filter.faces = Some(spec.to_string());
                // checked here, so a spec nothing can read is an error, not an empty grid
                let _ = crate::query::parse_face_spec(spec).map_err(|e| format!("faces count: {e}"))?;
            }
            if let Some(list) = trimmed(&f.people) {
                filter.people = list.iter().map(|p| p.to_string()).collect();
                filter.person_op = f.people_op;
            }
        }
        let mut rules: Vec<Rule> = rules.into_iter().map(rule_of).collect();
        rules.extend(self.pick_rules());
        if let Some(f) = &self.file
            && f.untagged
        {
            // no keywords picked beside it (`to_filter` sends that case the other way): the photos without any
            rules.push(Rule::Field { field: "keywords".into(), op: "isEmpty".into(), value: Value::Null });
        }
        if !rules.is_empty() {
            filter.rule_set = Some(RuleSet { mode: Match::All, rules });
        }
        Ok(filter)
    }
}

/// A [`Rule::Field`] over a lowered `{"field", "op", "value"}` triple.
fn rule_of(v: Value) -> Rule {
    let o = v.as_object();
    let field = o.and_then(|o| o.get("field")).and_then(Value::as_str).unwrap_or("").to_string();
    let op = o.and_then(|o| o.get("op")).and_then(Value::as_str).unwrap_or("").to_string();
    let value = o.and_then(|o| o.get("value")).cloned().unwrap_or(Value::Null);
    Rule::Field { field, op, value }
}

/// A Text rule's value, or `None` when the field constrains nothing.
fn text_rule(field: &str, value: &str) -> Option<Value> {
    let v = value.trim();
    (!v.is_empty()).then(|| json!({"field": field, "op": "contains", "value": v}))
}

/// The keywords/people entries worth keeping: trimmed, empties dropped; `None` when none is left.
fn trimmed(list: &[String]) -> Option<Vec<&str>> {
    let v: Vec<&str> = list.iter().map(String::as_str).map(str::trim).filter(|s| !s.is_empty()).collect();
    (!v.is_empty()).then_some(v)
}

/// A hostile number: a whole, non-negative, finite value, or an error naming `field`.
/// A signed range's rules (exposure bias), each end a finite number.
fn signed_range_rules(rules: &mut Vec<Value>, field: &str, range: &Range) -> Result<(), String> {
    let check = |v: Option<f64>| match v {
        Some(v) if !v.is_finite() => Err(format!("{field}: {v} is not a usable number")),
        v => Ok(v),
    };
    let (min, max) = (check(range.min)?, check(range.max)?);
    if let (Some(min), Some(max)) = (min, max)
        && min > max
    {
        return Err(format!("{field}: the range {min} – {max} is reversed (the minimum is above the maximum)"));
    }
    match (min, max) {
        (Some(v), Some(w)) => rules.push(json!({"field": field, "op": "between", "value": [v, w]})),
        (Some(v), None) => rules.push(json!({"field": field, "op": "gte", "value": v})),
        (None, Some(v)) => rules.push(json!({"field": field, "op": "lte", "value": v})),
        (None, None) => {}
    }
    Ok(())
}

/// A date span's rule: one `between`, an open end standing for the first or the last day there is, both ends
/// included (a year or a month as an end takes all of it). Each end is checked as the rules read dates.
fn date_rules(rules: &mut Vec<Value>, field: &str, span: &DateSpan) -> Result<(), String> {
    let end = |v: &Option<String>| -> Result<Option<String>, String> {
        match v.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            None => Ok(None),
            Some(d) => crate::rules::rule_date(d)
                .map(|_| Some(d.to_string()))
                .map_err(|_| format!("{field}: {d:?} is not a date (a year 2026, a month 2026-05 or a day 2026-05-14)")),
        }
    };
    let (from, to) = (end(&span.from)?, end(&span.to)?);
    if let (Some(a), Some(b)) = (&from, &to)
        && a > b
        && !a.starts_with(b.as_str())
    {
        return Err(format!("{field}: the span {a} – {b} is reversed"));
    }
    if from.is_none() && to.is_none() {
        return Ok(());
    }
    let from = from.unwrap_or_else(|| "0001".into());
    let to = to.unwrap_or_else(|| "9999".into());
    rules.push(json!({"field": field, "op": "between", "value": [from, to]}));
    Ok(())
}

fn non_negative(field: &str, v: f64) -> Result<f64, String> {
    if !v.is_finite() {
        return Err(format!("{field}: {v} is not a usable number"));
    }
    if v < 0.0 {
        return Err(format!("{field}: {v} is negative"));
    }
    Ok(v)
}

/// One rule per end of `range` over `field`, checking both ends.
fn range_rules(rules: &mut Vec<Value>, field: &str, range: &Range) -> Result<(), String> {
    let check = |v: Option<f64>| v.map(|v| non_negative(field, v)).transpose();
    let (min, max) = (check(range.min)?, check(range.max)?);
    if let (Some(min), Some(max)) = (min, max)
        && min > max
    {
        return Err(format!("{field}: the range {min} – {max} is reversed (the minimum is above the maximum)"));
    }
    match (min, max) {
        (Some(v), Some(w)) if v == w => rules.push(json!({"field": field, "op": "is", "value": v})),
        (Some(v), Some(w)) => rules.push(json!({"field": field, "op": "between", "value": [v, w]})),
        (Some(v), None) => rules.push(json!({"field": field, "op": "gte", "value": v})),
        (None, Some(v)) => rules.push(json!({"field": field, "op": "lte", "value": v})),
        (None, None) => {}
    }
    Ok(())
}

/// The shutter-speed range's rules, each end checked as a time (`1/250` or seconds).
fn shutter_rules(rules: &mut Vec<Value>, range: &ShutterRange) -> Result<(), String> {
    let read = |end: &str, v: &Option<String>| -> Result<Option<f64>, String> {
        match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(None),
            Some(s) => crate::parse_shutter_seconds(s)
                .filter(|secs| secs.is_finite() && *secs >= 0.0)
                .map(Some)
                .ok_or_else(|| format!("shutter speed {end}: {s:?} is not a time like 1/250 or 2")),
        }
    };
    let (min, max) = (read("minimum", &range.min)?, read("maximum", &range.max)?);
    if let (Some(min), Some(max)) = (min, max)
        && min > max
    {
        return Err(format!("shutter speed: {min} s – {max} s is reversed (the minimum is above the maximum)"));
    }
    match (range.min.as_deref().map(str::trim).filter(|s| !s.is_empty()), range.max.as_deref().map(str::trim).filter(|s| !s.is_empty())) {
        (Some(min), Some(max)) => rules.push(json!({"field": "shutterSpeed", "op": "between", "value": [min, max]})),
        (Some(min), None) => rules.push(json!({"field": "shutterSpeed", "op": "gte", "value": min})),
        (None, Some(max)) => rules.push(json!({"field": "shutterSpeed", "op": "lte", "value": max})),
        (None, None) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Catalog, Op, Photo, PhotoId, Sort, Source};

    fn catalog() -> Catalog {
        let mut c = Catalog::new();
        let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 1 }, "IMG_4001.jpg", "JPEG", 6000, 4000, "2026-05-01T10:00:00");
        p.rating = 4;
        p.meta.camera = "Canon EOS R6".into();
        p.meta.lens = "RF 85mm F1.2".into();
        p.meta.focal_mm = Some(85.0);
        p.meta.aperture = Some(1.8);
        p.meta.shutter = "1/250".into();
        p.meta.iso = Some(400);
        p.meta.title = "Harbour".into();
        p.meta.caption = "Morning at the pier".into();
        p.meta.keywords = vec!["travel".into(), "travel|italy".into()];
        p.meta.country = "Italy".into();
        p.meta.gps = Some((45.0, 9.0));
        c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        let mut v = Photo::new(PhotoId(2), Source::Demo { scene: 2 }, "clip.mp4", "MP4", 1920, 1080, "2026-06-01T10:00:00");
        v.kind = crate::MediaKind::Video;
        v.duration = Some(12.5);
        c.apply(Op::AddPhoto { photo: Box::new(v) }).unwrap();
        c
    }

    fn matches(c: &Catalog, q: &AdvancedSearch) -> Vec<PhotoId> {
        let f = q.to_filter().unwrap();
        let mut ids = c.query(&f, &Sort::default());
        ids.sort();
        ids
    }

    fn q() -> AdvancedSearch {
        AdvancedSearch::default()
    }

    #[test]
    fn an_empty_query_matches_everything() {
        let c = catalog();
        assert_eq!(matches(&c, &q()), vec![PhotoId(1), PhotoId(2)]);
        assert!(q().is_empty());
        assert_eq!(q().to_filter().unwrap(), Filter::default());
        // categories with nothing set add nothing
        let mut query = q();
        query.file = Some(FileCategory::default());
        query.photograph = Some(PhotographCategory::default());
        query.media = Some(MediaCategory::default());
        assert!(query.is_empty());
        assert_eq!(query.to_filter().unwrap(), Filter::default());
        // an unknown field is a refused query, not a silently ignored one
        assert!(serde_json::from_value::<AdvancedSearch>(serde_json::json!({"words": "x", "magic": true})).is_err());
    }

    #[test]
    fn global_words_match_across_fields() {
        let c = catalog();
        let mut query = q();
        query.words = "harbour italy".into();
        assert_eq!(matches(&c, &query), vec![PhotoId(1)], "title and keyword together");
        query.words = "harbour clip".into();
        assert!(matches(&c, &query).is_empty(), "words must all match one photo");
    }

    #[test]
    fn photograph_information_narrows_by_camera_lens_and_exposure() {
        let c = catalog();
        let mut query = q();
        query.photograph = Some(PhotographCategory { camera: "canon".into(), lens: "85mm".into(), ..Default::default() });
        assert_eq!(matches(&c, &query), vec![PhotoId(1)]);
        query.photograph = Some(PhotographCategory {
            camera: "canon".into(),
            focal_mm: Range { min: Some(50.0), max: Some(100.0) },
            aperture: Range { min: Some(1.8), max: None },
            iso: Range { min: Some(100.0), max: Some(800.0) },
            shutter: ShutterRange { min: Some("1/500".into()), max: None },
            lens: String::new(),
            ..Default::default()
        });
        assert_eq!(matches(&c, &query), vec![PhotoId(1)], "f/1.8 ≥ 1.8, 1/250 s ≥ 1/500 s, ISO 400 in 100–800");
        query.photograph.as_mut().unwrap().shutter.min = Some("1/125".into());
        assert!(matches(&c, &query).is_empty(), "1/250 s is slower than 1/125 s, so it is below the minimum");
        query.photograph.as_mut().unwrap().lens = "nikon".into();
        assert!(matches(&c, &query).is_empty());
    }

    #[test]
    fn item_properties_narrow_by_size_rating_and_label() {
        let c = catalog();
        let mut query = q();
        query.properties = Some(PropertiesCategory {
            megapixels: Range { min: Some(20.0), max: None },
            width: Range { min: Some(1000.0), max: None },
            ..Default::default()
        });
        assert_eq!(matches(&c, &query), vec![PhotoId(1)]);
        query.properties.as_mut().unwrap().rating = Some(3);
        query.properties.as_mut().unwrap().rating_op = RatingOp::AtLeast;
        assert_eq!(matches(&c, &query), vec![PhotoId(1)]);
        query.properties.as_mut().unwrap().rating_op = RatingOp::AtMost;
        assert!(matches(&c, &query).is_empty(), "at most 3 stars excludes a 4-star photo");
        query.properties.as_mut().unwrap().label = Some("red".into());
        assert!(matches(&c, &query).is_empty(), "ANDed: no photo is both at most 3 stars and red");
    }

    #[test]
    fn file_category_narrows_by_type_name_and_keywords() {
        let c = catalog();
        let mut query = q();
        query.file = Some(FileCategory { kind: Some("video".into()), ..Default::default() });
        assert_eq!(matches(&c, &query), vec![PhotoId(2)]);
        query.file = Some(FileCategory { name: "img_".into(), keywords: vec!["travel".into()], keyword_op: MatchOp::Any, ..Default::default() });
        assert_eq!(matches(&c, &query), vec![PhotoId(1)], "hierarchical keywords match their children too");
        query.file.as_mut().unwrap().keyword_op = MatchOp::All;
        query.file.as_mut().unwrap().keywords.push("travel|italy".into());
        assert_eq!(matches(&c, &query), vec![PhotoId(1)]);
        query.file.as_mut().unwrap().keywords.push("ocean".into());
        assert!(matches(&c, &query).is_empty());
    }

    #[test]
    fn description_location_faces_and_media_narrow() {
        let c = catalog();
        let mut query = q();
        query.description = Some(DescriptionCategory { title: "harbour".into(), caption: "pier".into() });
        assert_eq!(matches(&c, &query), vec![PhotoId(1)]);
        query.location = Some(LocationCategory { country: "italy".into(), has_gps: Some(true), ..Default::default() });
        assert_eq!(matches(&c, &query), vec![PhotoId(1)]);
        // faces and media stand on their own: the description and location fields above are cleared
        query.description = None;
        query.location = None;
        query.faces = Some(FacesCategory { count: Some("none".into()), ..Default::default() });
        assert_eq!(matches(&c, &query), vec![PhotoId(1), PhotoId(2)], "no faces on either photo yet");
        query.faces.as_mut().unwrap().count = Some("1+".into());
        assert!(matches(&c, &query).is_empty());
        query.faces = None;
        query.media = Some(MediaCategory { duration_s: Range { min: Some(10.0), max: Some(20.0) }, ..Default::default() });
        assert_eq!(matches(&c, &query), vec![PhotoId(2)]);
    }

    #[test]
    fn a_reversed_or_impossible_value_is_an_error_naming_the_field() {
        let mut query = q();
        query.properties = Some(PropertiesCategory { width: Range { min: Some(100.0), max: Some(50.0) }, ..Default::default() });
        let e = query.to_filter().unwrap_err();
        assert!(e.contains("width"), "{e}");
        let mut query = q();
        query.properties = Some(PropertiesCategory { rating: Some(9), ..Default::default() });
        assert!(query.to_filter().unwrap_err().contains("rating"));
        let mut query = q();
        query.properties = Some(PropertiesCategory { label: Some("crimson".into()), ..Default::default() });
        assert!(query.to_filter().unwrap_err().contains("color label"));
        let mut query = q();
        query.photograph = Some(PhotographCategory { shutter: ShutterRange { min: Some("fast".into()), max: None }, ..Default::default() });
        assert!(query.to_filter().unwrap_err().contains("shutter speed"));
        let mut query = q();
        query.faces = Some(FacesCategory { count: Some("lots".into()), ..Default::default() });
        assert!(query.to_filter().unwrap_err().contains("faces count"));
        let mut query = q();
        query.file = Some(FileCategory { kind: Some("movie".into()), ..Default::default() });
        assert!(query.to_filter().unwrap_err().contains("kind"));
        let mut query = q();
        query.photograph = Some(PhotographCategory { focal_mm: Range { min: Some(-5.0), max: None }, ..Default::default() });
        assert!(query.to_filter().unwrap_err().contains("focalLength"));
        let mut query = q();
        query.photograph = Some(PhotographCategory { iso: Range { min: Some(f64::NAN), max: None }, ..Default::default() });
        assert!(query.to_filter().unwrap_err().contains("iso"));
    }

    /// Issue #1135: a group's conditions combine as its match says (all, any, none, not all), and search groups
    /// combine as the query's `groupsMatch` says.
    #[test]
    fn a_groups_conditions_combine_as_its_match_says() {
        let c = catalog();
        // photo 1: 4 stars, Canon, Italy; photo 2: a video, unrated, nowhere
        let mut query = q();
        query.properties = Some(PropertiesCategory { rating: Some(4), rating_op: RatingOp::AtLeast, ..Default::default() });
        query.file = Some(FileCategory { kind: Some("video".into()), ..Default::default() });
        assert!(matches(&c, &query).is_empty(), "all: no photo is both 4 stars and a video");
        query.mode = SearchMatch::Any;
        assert_eq!(matches(&c, &query), vec![PhotoId(1), PhotoId(2)], "any: one or the other");
        query.mode = SearchMatch::None;
        assert!(matches(&c, &query).is_empty(), "none: each photo meets one of them");
        query.mode = SearchMatch::NotAll;
        assert_eq!(matches(&c, &query), vec![PhotoId(1), PhotoId(2)], "not all: neither meets both");
        query.file = Some(FileCategory { name: "img_".into(), ..Default::default() });
        query.mode = SearchMatch::NotAll;
        assert_eq!(matches(&c, &query), vec![PhotoId(2)], "photo 1 meets both, so only photo 2 misses one");
        query.mode = SearchMatch::None;
        assert_eq!(matches(&c, &query), vec![PhotoId(2)]);
        // words, keywords, people and the face count are conditions like any other
        let mut query = q();
        query.mode = SearchMatch::Any;
        query.words = "harbour".into();
        query.media = Some(MediaCategory { duration_s: Range { min: Some(10.0), max: None }, ..Default::default() });
        assert_eq!(matches(&c, &query), vec![PhotoId(1), PhotoId(2)]);
        query.mode = SearchMatch::None;
        query.words = String::new();
        query.media = None;
        query.file = Some(FileCategory { keywords: vec!["travel".into(), "ocean".into()], keyword_op: MatchOp::Any, ..Default::default() });
        assert_eq!(matches(&c, &query), vec![PhotoId(2)], "none: photo 1 has one of the keywords (a child of travel)");
        query.faces = Some(FacesCategory { count: Some("1+".into()), ..Default::default() });
        query.file = None;
        assert_eq!(matches(&c, &query), vec![PhotoId(1), PhotoId(2)], "none has a face");
    }

    #[test]
    fn search_groups_combine_as_the_query_says() {
        let c = catalog();
        let mut query = q();
        query.photograph = Some(PhotographCategory { camera: "canon".into(), ..Default::default() });
        let video = AdvancedSearch { file: Some(FileCategory { kind: Some("video".into()), ..Default::default() }), ..q() };
        query.groups.push(video);
        assert!(matches(&c, &query).is_empty(), "all groups: no photo is both");
        query.groups_match = GroupsMatch::Any;
        assert_eq!(matches(&c, &query), vec![PhotoId(1), PhotoId(2)], "any group");
        // a group's own match applies inside it
        query.groups[0].mode = SearchMatch::None;
        assert_eq!(matches(&c, &query), vec![PhotoId(1)], "canon, or not a video");
        // an empty group constrains nothing; an empty query of groups matches every photo
        let mut empty = q();
        empty.groups.push(q());
        empty.groups_match = GroupsMatch::Any;
        assert!(empty.is_empty());
        assert_eq!(empty.to_filter().unwrap(), Filter::default());
        // groups don't nest, are bounded, and a problem names its group
        let mut nested = q();
        nested.groups.push(AdvancedSearch { groups: vec![q()], ..q() });
        assert!(nested.to_filter().unwrap_err().contains("groups of its own"));
        let mut many = q();
        many.groups = vec![q(); MAX_GROUPS + 1];
        assert!(many.to_filter().is_err());
        let mut bad = q();
        bad.groups.push(AdvancedSearch { faces: Some(FacesCategory { count: Some("lots".into()), ..Default::default() }), ..q() });
        let e = bad.to_filter().unwrap_err();
        assert!(e.contains("search group 2") && e.contains("faces count"), "{e}");
        // the match and the groups travel as JSON
        let json = serde_json::to_value(&query).unwrap();
        assert_eq!(json["groupsMatch"], "any");
        assert_eq!(json["groups"][0]["match"], "none");
        assert_eq!(serde_json::from_value::<AdvancedSearch>(json).unwrap(), query);
        assert!(serde_json::to_value(q()).unwrap().get("match").is_none(), "the default is left out");
    }

    /// Issue #1135 phase 2: values picked from the catalog's lists match exactly, any of them, and "without" adds the
    /// photos with nothing recorded; alone it asks for only those.
    #[test]
    fn picked_values_match_any_of_them_and_without_adds_the_empty_ones() {
        let c = catalog();
        let pick = |p: PhotographCategory| AdvancedSearch { photograph: Some(p), ..q() };
        let cam = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // exact, case-insensitive, any of the list
        assert_eq!(matches(&c, &pick(PhotographCategory { cameras: cam(&["canon eos r6", "Nikon Z8"]), ..Default::default() })), vec![PhotoId(1)]);
        assert!(matches(&c, &pick(PhotographCategory { cameras: cam(&["Canon"]), ..Default::default() })).is_empty(), "a pick is whole");
        // without a camera: alone only the clip; beside a camera, either
        assert_eq!(matches(&c, &pick(PhotographCategory { no_camera: true, ..Default::default() })), vec![PhotoId(2)]);
        let both = PhotographCategory { cameras: cam(&["Canon EOS R6"]), no_camera: true, ..Default::default() };
        assert_eq!(matches(&c, &pick(both)), vec![PhotoId(1), PhotoId(2)]);
        assert_eq!(matches(&c, &pick(PhotographCategory { lenses: cam(&["RF 85mm F1.2"]), ..Default::default() })), vec![PhotoId(1)]);
        assert_eq!(matches(&c, &pick(PhotographCategory { no_lens: true, ..Default::default() })), vec![PhotoId(2)]);
        // formats
        let formats = AdvancedSearch { file: Some(FileCategory { formats: cam(&["mp4"]), ..Default::default() }), ..q() };
        assert_eq!(matches(&c, &formats), vec![PhotoId(2)]);
        // not tagged: alone, the clip; beside a keyword, the keyword's photo too, in one group or several
        let untagged = |keywords: &[&str]| FileCategory { keywords: cam(keywords), untagged: true, ..Default::default() };
        assert_eq!(matches(&c, &AdvancedSearch { file: Some(untagged(&[])), ..q() }), vec![PhotoId(2)]);
        assert_eq!(matches(&c, &AdvancedSearch { file: Some(untagged(&["italy"])), ..q() }), vec![PhotoId(1), PhotoId(2)]);
        let mut grouped = AdvancedSearch { file: Some(untagged(&[])), ..q() };
        grouped.groups.push(pick(PhotographCategory { no_lens: true, ..Default::default() }));
        assert_eq!(matches(&c, &grouped), vec![PhotoId(2)]);
        // a group's None turns the picks round: not this camera
        let none = AdvancedSearch { mode: SearchMatch::None, ..pick(PhotographCategory { cameras: cam(&["Canon EOS R6"]), ..Default::default() }) };
        assert_eq!(matches(&c, &none), vec![PhotoId(2)]);
        // they count as conditions, and travel as JSON
        assert!(!pick(PhotographCategory { no_lens: true, ..Default::default() }).is_empty());
        let json = serde_json::to_value(pick(PhotographCategory { cameras: cam(&["A"]), no_camera: true, ..Default::default() })).unwrap();
        assert_eq!((json["photograph"]["cameras"][0].clone(), json["photograph"]["noCamera"].clone()), (json!("A"), json!(true)));
        assert!(serde_json::to_value(q()).unwrap().get("photograph").is_none());
    }

    #[test]
    fn dates_flags_file_size_flash_and_bias_narrow_the_grid() {
        let mut c = catalog();
        c.photo_mut(PhotoId(1)).unwrap().captured = Some("2026-05-01T10:00:00".into());
        c.photo_mut(PhotoId(2)).unwrap().captured = Some("2026-06-01T10:00:00".into());
        c.photo_mut(PhotoId(1)).unwrap().file_size = 24_000_000;
        c.photo_mut(PhotoId(1)).unwrap().flag = crate::Flag::Pick;
        c.photo_mut(PhotoId(1)).unwrap().meta.flash = Some(true);
        c.photo_mut(PhotoId(1)).unwrap().meta.exposure_bias = Some(-0.7);
        c.photo_mut(PhotoId(2)).unwrap().file_size = 80_000_000;
        c.photo_mut(PhotoId(2)).unwrap().imported = "2026-07-02T08:00:00".into();
        c.photo_mut(PhotoId(2)).unwrap().edited = Some("2026-07-03T09:00:00".into());
        let dates = |d: DatesCategory| AdvancedSearch { dates: Some(d), ..q() };
        let span =
            |from: &str, to: &str| DateSpan { from: (!from.is_empty()).then(|| from.to_string()), to: (!to.is_empty()).then(|| to.to_string()) };
        // a span, either end open, a month as an end taking all of it
        assert_eq!(matches(&c, &dates(DatesCategory { captured: span("2026-05", "2026-05"), ..Default::default() })), vec![PhotoId(1)]);
        assert_eq!(matches(&c, &dates(DatesCategory { captured: span("2026-05-15", ""), ..Default::default() })), vec![PhotoId(2)]);
        assert_eq!(matches(&c, &dates(DatesCategory { captured: span("", "2026-05-31"), ..Default::default() })), vec![PhotoId(1)]);
        assert_eq!(matches(&c, &dates(DatesCategory { captured: span("2026", "2026"), ..Default::default() })), vec![PhotoId(1), PhotoId(2)]);
        // a month and a day, whatever the year
        assert_eq!(matches(&c, &dates(DatesCategory { month: Some(6), ..Default::default() })), vec![PhotoId(2)]);
        assert_eq!(matches(&c, &dates(DatesCategory { month: Some(5), day: Some(1), ..Default::default() })), vec![PhotoId(1)]);
        assert!(matches(&c, &dates(DatesCategory { day: Some(2), ..Default::default() })).is_empty());
        // import and edit spans
        assert_eq!(matches(&c, &dates(DatesCategory { imported: span("2026-07-01", "2026-07-31"), ..Default::default() })), vec![PhotoId(2)]);
        assert_eq!(matches(&c, &dates(DatesCategory { edited: span("2026-07", ""), ..Default::default() })), vec![PhotoId(2)]);
        // bad dates, months and days are actionable errors
        for bad in [
            DatesCategory { captured: span("May 2026", ""), ..Default::default() },
            DatesCategory { captured: span("2026-06", "2026-05"), ..Default::default() },
            DatesCategory { month: Some(13), ..Default::default() },
            DatesCategory { day: Some(0), ..Default::default() },
        ] {
            assert!(dates(bad.clone()).to_filter().is_err(), "{bad:?}");
        }
        // pick flag and file size
        let props = |p: PropertiesCategory| AdvancedSearch { properties: Some(p), ..q() };
        assert_eq!(matches(&c, &props(PropertiesCategory { flag: Some("pick".into()), ..Default::default() })), vec![PhotoId(1)]);
        assert_eq!(matches(&c, &props(PropertiesCategory { flag: Some("none".into()), ..Default::default() })), vec![PhotoId(2)]);
        assert!(props(PropertiesCategory { flag: Some("starred".into()), ..Default::default() }).to_filter().is_err());
        let size = PropertiesCategory { file_size_mb: Range { min: Some(50.0), max: None }, ..Default::default() };
        assert_eq!(matches(&c, &props(size)), vec![PhotoId(2)]);
        // flash and a negative exposure bias
        let photo = |p: PhotographCategory| AdvancedSearch { photograph: Some(p), ..q() };
        assert_eq!(matches(&c, &photo(PhotographCategory { flash: Some(true), ..Default::default() })), vec![PhotoId(1)]);
        assert_eq!(matches(&c, &photo(PhotographCategory { flash: Some(false), ..Default::default() })), vec![PhotoId(2)]);
        let bias = PhotographCategory { exposure_bias: Range { min: Some(-1.0), max: Some(-0.3) }, ..Default::default() };
        assert_eq!(matches(&c, &photo(bias)), vec![PhotoId(1)]);
        let reversed = PhotographCategory { exposure_bias: Range { min: Some(1.0), max: Some(-1.0) }, ..Default::default() };
        assert!(photo(reversed).to_filter().is_err());
        // a capture dated only to the month has that month but no day; one dated to the year has neither
        c.photo_mut(PhotoId(2)).unwrap().captured = Some("1983-04".into());
        assert_eq!(matches(&c, &dates(DatesCategory { month: Some(4), ..Default::default() })), vec![PhotoId(2)]);
        assert!(matches(&c, &dates(DatesCategory { day: Some(1), month: Some(4), ..Default::default() })).is_empty());
        c.photo_mut(PhotoId(2)).unwrap().captured = Some("1983".into());
        assert!(matches(&c, &dates(DatesCategory { month: Some(1), ..Default::default() })).is_empty());
        // they count as conditions, and travel as JSON in camelCase
        let d = dates(DatesCategory { month: Some(5), ..Default::default() });
        assert!(!d.is_empty());
        assert_eq!(serde_json::to_value(&d).unwrap()["dates"]["month"], json!(5));
        let p = props(PropertiesCategory { file_size_mb: Range { min: Some(1.0), max: None }, ..Default::default() });
        assert_eq!(serde_json::to_value(&p).unwrap()["properties"]["fileSizeMb"]["min"], json!(1.0));
    }

    #[test]
    fn unsupported_audio_video_conditions_are_rejected_actionably() {
        let mut query = q();
        query.media = Some(MediaCategory { codec: Some("h264".into()), ..Default::default() });
        assert!(query.to_filter().unwrap_err().contains("codec"));
        let mut query = q();
        query.media = Some(MediaCategory { frame_rate: Some(24.0), ..Default::default() });
        assert!(query.to_filter().unwrap_err().contains("frame rate"));
    }
}
