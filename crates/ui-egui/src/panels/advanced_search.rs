//! Search ▸ Advanced Search (issue #684): the modal's structured field filters.
//!
//! Each collapsible category edits one part of the [`lcc_catalog::Filter`] the library grid shows;
//! the dialog's confirm button composes them into one `library.filter` patch. The natural-language
//! bar and the AI model are a later PR.

use egui::{RichText, Ui};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::theme::Tokens;

/// The modal's state: one field per control. An empty string means "no constraint".
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AdvancedSearch {
    /// File, Album, Tags
    pub folder: String,
    pub keyword: String,
    /// Item Properties
    pub kind: String,
    pub edited: String,
    pub rating: u8,
    pub rating_op: String,
    pub flag: String,
    pub label: String,
    /// Caption, Comment, Title
    pub caption: String,
    /// Photograph Information
    pub camera: String,
    pub lens: String,
    pub iso: String,
    pub focal: String,
    pub aperture: String,
    pub date_from: String,
    pub date_to: String,
    /// Geographic Position
    pub location: String,
    /// Face Regions
    pub faces: String,
    pub person: String,
}

fn kind_of(s: &str) -> Option<lcc_catalog::MediaKind> {
    match s {
        "image" => Some(lcc_catalog::MediaKind::Image),
        "raw" => Some(lcc_catalog::MediaKind::Raw),
        "video" => Some(lcc_catalog::MediaKind::Video),
        _ => None,
    }
}

fn rating_op_of(s: &str) -> Option<lcc_catalog::RatingOp> {
    match s {
        "atLeast" => Some(lcc_catalog::RatingOp::AtLeast),
        "exactly" => Some(lcc_catalog::RatingOp::Exactly),
        "atMost" => Some(lcc_catalog::RatingOp::AtMost),
        _ => None,
    }
}

impl AdvancedSearch {
    /// Whether every field is empty (nothing to search by).
    pub fn is_empty(&self) -> bool {
        self.folder.trim().is_empty()
            && self.keyword.trim().is_empty()
            && self.kind.is_empty()
            && self.edited.is_empty()
            && self.rating == 0
            && self.flag.is_empty()
            && self.label.is_empty()
            && self.caption.trim().is_empty()
            && self.camera.trim().is_empty()
            && self.lens.trim().is_empty()
            && self.iso.trim().is_empty()
            && self.focal.trim().is_empty()
            && self.aperture.trim().is_empty()
            && self.date_from.trim().is_empty()
            && self.date_to.trim().is_empty()
            && self.location.trim().is_empty()
            && self.faces.trim().is_empty()
            && self.person.trim().is_empty()
    }

    /// The `library.filter` patch these fields describe (only the set ones).
    pub fn to_patch(&self) -> Value {
        let mut o = serde_json::Map::new();
        // `text` carries the caption words and the fielded EXIF tokens; every token must match.
        let mut text = String::new();
        let add = |text: &mut String, s: &str| {
            let s = s.trim();
            if !s.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(s);
            }
        };
        add(&mut text, &self.caption);
        if !self.camera.trim().is_empty() {
            add(&mut text, &format!("camera:{}", self.camera.trim()));
        }
        if !self.lens.trim().is_empty() {
            add(&mut text, &format!("lens:{}", self.lens.trim()));
        }
        if !self.iso.trim().is_empty() {
            add(&mut text, &format!("iso:{}", self.iso.trim()));
        }
        if !self.focal.trim().is_empty() {
            add(&mut text, &format!("focal:{}", self.focal.trim()));
        }
        if !self.aperture.trim().is_empty() {
            add(&mut text, &format!("f:{}", self.aperture.trim()));
        }
        if !text.trim().is_empty() {
            o.insert("text".into(), Value::String(text));
        }

        let put = |o: &mut serde_json::Map<String, Value>, k: &str, v: &str| {
            if !v.trim().is_empty() {
                o.insert(k.into(), Value::String(v.trim().to_string()));
            }
        };
        put(&mut o, "folder", &self.folder);
        put(&mut o, "keyword", &self.keyword);
        put(&mut o, "location", &self.location);
        put(&mut o, "dateFrom", &self.date_from);
        put(&mut o, "dateTo", &self.date_to);
        put(&mut o, "faces", &self.faces);
        put(&mut o, "person", &self.person);
        if let Some(k) = kind_of(&self.kind) {
            o.insert("kind".into(), json!(k));
        }
        match self.edited.as_str() {
            "yes" => {
                o.insert("edited".into(), json!(true));
            }
            "no" => {
                o.insert("edited".into(), json!(false));
            }
            _ => {}
        }
        if self.rating > 0 {
            o.insert("rating".into(), json!(self.rating));
            o.insert("ratingOp".into(), json!(rating_op_of(&self.rating_op).unwrap_or(lcc_catalog::RatingOp::AtLeast)));
        }
        if let Some(f) = lcc_catalog::Flag::parse(&self.flag) {
            o.insert("flag".into(), json!(f));
        }
        if let Some(l) = lcc_catalog::ColorLabel::parse(&self.label) {
            o.insert("label".into(), json!(l));
        }
        Value::Object(o)
    }
}

/// One text field: a label above it. `id` is the accessibility id.
fn text_row(ui: &mut Ui, t: &Tokens, value: &mut String, label: &str, id: &str) {
    ui.label(RichText::new(crate::i18n::tr(label)).font(t.font(12.5)).color(t.text_label));
    let r = ui.add(egui::TextEdit::singleline(value));
    crate::widgets::register(ui.ctx(), format!("advsearch:{id}"), r.rect);
}

/// A label followed by one selectable chip per option (`("", "Any")` first). `value` holds the
/// selected option's key (`""` = none).
fn choice_row(ui: &mut Ui, t: &Tokens, value: &mut String, label: &str, options: &[(&str, &str)]) {
    ui.label(RichText::new(crate::i18n::tr(label)).font(t.font(12.5)).color(t.text_label));
    ui.horizontal(|ui| {
        for (key, name) in options {
            let on = *value == *key;
            if ui
                .selectable_label(on, RichText::new(crate::i18n::tr(name)).font(t.font(12.0)).color(if on { t.accent } else { t.text }))
                .clicked()
            {
                *value = key.to_string();
            }
        }
    });
}

/// The modal's body: one collapsible per category.
pub fn body(ui: &mut Ui, s: &mut AdvancedSearch) {
    let t = Tokens::get(ui.ctx());
    ui.collapsible(crate::i18n::tr("File, Album, Tags")).show(ui, |ui| {
        text_row(ui, &t, &mut s.folder, "Folder path", "folder");
        text_row(ui, &t, &mut s.keyword, "Keyword / tag", "keyword");
    });
    ui.collapsible(crate::i18n::tr("Item Properties")).show(ui, |ui| {
        choice_row(ui, &t, &mut s.kind, "File type", &[("", "Any"), ("image", "Photo"), ("raw", "Raw"), ("video", "Video")]);
        choice_row(ui, &t, &mut s.edited, "Edited", &[("", "Any"), ("yes", "Edited"), ("no", "Unedited")]);
        ui.label(RichText::new(crate::i18n::tr("Rating")).font(t.font(12.5)).color(t.text_label));
        ui.horizontal(|ui| {
            for (key, name) in [("atLeast", "At least"), ("exactly", "Exactly"), ("atMost", "At most")] {
                if ui
                    .selectable_label(s.rating_op == key, RichText::new(crate::i18n::tr(name)).font(t.font(12.0)).color(t.text))
                    .clicked()
                {
                    s.rating_op = key.to_string();
                }
            }
            let r = ui.add(egui::Slider::new(&mut s.rating, 0..=5).text(crate::i18n::tr("Stars")));
            crate::widgets::register(ui.ctx(), "advsearch:rating", r.rect);
        });
        choice_row(ui, &t, &mut s.flag, "Flag", &[("", "Any"), ("pick", "Picked"), ("reject", "Rejected")]);
        choice_row(
            ui,
            &t,
            &mut s.label,
            "Color label",
            &[("", "Any"), ("red", "Red"), ("yellow", "Yellow"), ("green", "Green"), ("blue", "Blue"), ("purple", "Purple")],
        );
    });
    ui.collapsible(crate::i18n::tr("Caption, Comment, Title")).show(ui, |ui| {
        text_row(ui, &t, &mut s.caption, "Text (title, caption, comment)", "caption");
    });
    ui.collapsible(crate::i18n::tr("Photograph Information")).show(ui, |ui| {
        text_row(ui, &t, &mut s.camera, "Camera", "camera");
        text_row(ui, &t, &mut s.lens, "Lens", "lens");
        text_row(ui, &t, &mut s.iso, "ISO (e.g. >800)", "iso");
        text_row(ui, &t, &mut s.focal, "Focal length mm (e.g. 85)", "focal");
        text_row(ui, &t, &mut s.aperture, "Aperture (e.g. 1.8)", "aperture");
        text_row(ui, &t, &mut s.date_from, "Captured from (2026-04-01)", "dateFrom");
        text_row(ui, &t, &mut s.date_to, "Captured until (2026-04)", "dateTo");
    });
    ui.collapsible(crate::i18n::tr("Geographic Position")).show(ui, |ui| {
        text_row(ui, &t, &mut s.location, "Location (country, city)", "location");
    });
    ui.collapsible(crate::i18n::tr("Face Regions")).show(ui, |ui| {
        text_row(ui, &t, &mut s.faces, "Face count (e.g. 2, 3+, <=2)", "faces");
        text_row(ui, &t, &mut s.person, "Person", "person");
    });
}

#[cfg(test)]
mod tests {
    use super::AdvancedSearch;
    use serde_json::json;

    #[test]
    fn an_empty_state_gives_an_empty_patch() {
        assert_eq!(AdvancedSearch::default().to_patch(), json!({}));
        assert!(AdvancedSearch::default().is_empty());
    }

    #[test]
    fn text_and_exif_tokens_share_the_text_field() {
        let mut s = AdvancedSearch::default();
        s.caption = "sunset".into();
        s.camera = "Canon".into();
        s.iso = ">800".into();
        let patch = s.to_patch();
        assert_eq!(patch["text"], json!("sunset camera:Canon iso:>800"));
        assert!(patch.get("camera").is_none());
    }

    #[test]
    fn structured_choices_become_filter_fields() {
        let mut s = AdvancedSearch::default();
        s.kind = "video".into();
        s.edited = "no".into();
        s.rating = 4;
        s.rating_op = "exactly".into();
        s.flag = "pick".into();
        s.label = "red".into();
        s.location = "Paris".into();
        s.faces = "3+".into();
        s.person = "Jane".into();
        s.date_from = "2026-01-01".into();
        let patch = s.to_patch();
        assert_eq!(patch["kind"], json!("video"));
        assert_eq!(patch["edited"], json!(false));
        assert_eq!(patch["rating"], json!(4));
        assert_eq!(patch["ratingOp"], json!("exactly"));
        assert_eq!(patch["flag"], json!("pick"));
        assert_eq!(patch["label"], json!("red"));
        assert_eq!(patch["location"], json!("Paris"));
        assert_eq!(patch["faces"], json!("3+"));
        assert_eq!(patch["person"], json!("Jane"));
        assert_eq!(patch["dateFrom"], json!("2026-01-01"));
    }
}
