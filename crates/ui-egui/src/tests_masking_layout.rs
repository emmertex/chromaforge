//! Check painted text as well as widget rectangles: custom painters can overflow a valid widget.

use egui::{Rect, pos2, vec2};
use serde_json::json;

use crate::{LccApp, Services, headless::HeadlessView, i18n::Locale};

const LONG_NAME: &str = "The person standing beside the very long fence across the background";

fn text_shapes(shape: &egui::Shape, out: &mut Vec<egui::epaint::TextShape>) {
    match shape {
        egui::Shape::Text(text) => out.push(text.clone()),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| text_shapes(s, out)),
        _ => {}
    }
}

fn panel(width: f32, scale: f32, locale: Locale, describe: bool) -> (Vec<egui::epaint::TextShape>, Vec<(String, Rect)>) {
    let mut app = LccApp::new(lcc_engine::Session::with_demo(), Services::default());
    app.run("mask.add", json!({"kind": "subject", "name": LONG_NAME})).unwrap();
    app.ui.describe = describe.then(|| ("new".into(), LONG_NAME.repeat(4)));
    app.ui.detail_due = Some((100.0, 1));
    let photo = app.session.active().unwrap();
    let ctx = egui::Context::default();
    crate::i18n::set_language(locale);
    crate::theme::install_fonts(&ctx);
    crate::theme::apply(&ctx);
    let mut texts = Vec::new();
    let mut widgets = Vec::new();
    for frame in 0..3 {
        let raw = HeadlessView::raw_input(vec2(width, 1600.0), scale, frame as f64 / 60.0, vec![]);
        let mut out = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                crate::panels::masking::show(&mut app, ui, photo);
            });
        });
        out.textures_delta.clear();
        texts.clear();
        for shape in &out.shapes {
            text_shapes(&shape.shape, &mut texts);
        }
        widgets = crate::widgets::take_registry(&ctx);
    }
    if describe {
        assert_eq!(app.ui.describe.as_ref().unwrap().1, LONG_NAME.repeat(4), "layout must not shorten the actual prompt");
    }
    crate::i18n::set_language(Locale::En);
    (texts, widgets)
}

#[test]
fn masking_tile_labels_fit_without_eliding_at_supported_panel_widths() {
    for locale in [Locale::En, Locale::De, Locale::PtBr] {
        for width in [250.0, 270.0, 330.0, 520.0] {
            for scale in [1.0, 2.0] {
                let (texts, widgets) = panel(width, scale, locale, false);
                for (kind, label) in [("background", "Background"), ("luminanceRange", "Luminance"), ("prompt", "Describe")] {
                    crate::i18n::set_language(locale);
                    let label = crate::i18n::tr(label).to_string();
                    crate::i18n::set_language(Locale::En);
                    let tile = widgets.iter().find(|(id, _)| id == &format!("maskNew:{kind}")).unwrap().1;
                    let text = texts.iter().find(|t| t.galley.job.text == label && t.pos.y < tile.bottom()).unwrap();
                    assert!(!text.galley.elided, "{locale:?} {width} {label} should be readable in full");
                    assert!(
                        tile.shrink2(vec2(2.0, 0.0)).contains_rect(text.visual_bounding_rect()),
                        "{locale:?} {width} {scale}: {label} exceeds {tile:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn masking_long_names_and_describe_controls_stay_inside_the_panel() {
    for locale in [Locale::En, Locale::De, Locale::PtBr] {
        for width in [250.0, 270.0, 520.0] {
            let (texts, widgets) = panel(width, 2.0, locale, true);
            let row = widgets.iter().find(|(id, _)| id == "mask:1").unwrap().1;
            // the name in the mask row (the panel also titles the mask's sliders with it, above)
            let name = texts.iter().find(|t| t.galley.job.text == LONG_NAME && row.y_range().contains(t.pos.y + 1.0)).unwrap();
            assert!(name.galley.elided, "long mask names should be elided");
            assert!(
                Rect::from_min_max(pos2(row.left() + 32.0, row.top()), pos2(row.right() - 28.0, row.bottom()))
                    .contains_rect(name.visual_bounding_rect())
            );
            let field = widgets.iter().find(|(id, _)| id == "maskDescribe").unwrap().1;
            let button = widgets.iter().find(|(id, _)| id == "button:maskDescribeGo").unwrap().1;
            assert!(field.right() <= button.left(), "the prompt must not overlap Select");
            // (inside the panel's 12 pt left and 16 pt right margins)
            assert!(field.left() >= 12.0 && button.right() <= width - 16.0, "{locale:?} {width}: {field:?} {button:?}");
        }
    }
}

fn panel_with_lum(width: f32) -> Vec<(String, Rect)> {
    let mut app = LccApp::new(lcc_engine::Session::with_demo(), Services::default());
    app.run("mask.add", json!({"kind": "luminanceRange", "lo": 0.2, "hi": 0.8})).unwrap();
    let photo = app.session.active().unwrap();
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    crate::theme::apply(&ctx);
    let mut widgets = Vec::new();
    for frame in 0..3 {
        let raw = HeadlessView::raw_input(vec2(width, 1600.0), 1.0, frame as f64 / 60.0, vec![]);
        let mut out = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                crate::panels::masking::show(&mut app, ui, photo);
            });
        });
        out.textures_delta.clear();
        widgets = crate::widgets::take_registry(&ctx);
    }
    widgets
}

#[test]
fn masking_tiles_block_is_centered_in_panel() {
    for width in [270.0, 330.0, 420.0, 520.0] {
        let (_, widgets) = panel(width, 1.0, Locale::En, false);
        let object_rect = widgets.iter().find(|(id, _)| id == "maskNew:object").unwrap().1;
        let row1_right = widgets
            .iter()
            .filter(|(id, r)| id.starts_with("maskNew:") && !id.contains("Label") && (r.top() - object_rect.top()).abs() < 1.0)
            .map(|(_, r)| r.right())
            .fold(0.0, f32::max);
        let left_pad = object_rect.left();
        let right_pad = width - row1_right;
        assert!((left_pad - right_pad).abs() <= 2.0, "width {width}: left_pad {left_pad} != right_pad {right_pad}");
    }
}

#[test]
fn luminance_range_slider_fills_panel_width_without_arbitrary_cap() {
    for width in [300.0, 400.0, 500.0] {
        let widgets = panel_with_lum(width);
        let lum_rect = widgets.iter().find(|(id, _)| id == "lumRange:0").unwrap().1;
        assert!(lum_rect.width() > 240.0, "width {width}: lum_rect.width() = {} should exceed 240.0", lum_rect.width());
        assert!(lum_rect.left() >= 12.0 && lum_rect.right() <= width - 12.0);
        let expected_w = width - 16.0 - 16.0;
        assert!((lum_rect.width() - expected_w).abs() <= 1.0, "width {width}: expected width {expected_w}, got {}", lum_rect.width());
    }
}
