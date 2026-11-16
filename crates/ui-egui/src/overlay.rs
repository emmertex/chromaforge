//! One painter for the small info boxes the modules put in a corner of their view: the Map's
//! info and key, the Book's info overlay, the Print module's page info. A box is a translucent
//! panel (the theme's chrome colour, so it follows light and dark) holding rows; a row is a line
//! of text, optionally led by a small symbol (the Map key's legend).

use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2, pos2, vec2};

use crate::theme::Tokens;

/// Which corner of the view the box sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// The symbol that leads a legend row, drawn in a square of [`SYMBOL`] points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Symbol {
    /// A filled disc with a white rim, like a map pin; `label` is written in it (a cluster's count).
    Disc { fill: Color32, label: Option<&'static str> },
    /// A short thick line, like a track.
    Line { color: Color32 },
    /// A translucent disc with a coloured rim, like a saved location's circle.
    Ring { color: Color32 },
}

/// One row: text, with a symbol in a legend.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub symbol: Option<Symbol>,
    pub text: String,
}

impl Row {
    pub fn text(text: impl Into<String>) -> Row {
        Row { symbol: None, text: text.into() }
    }

    pub fn symbol(symbol: Symbol, text: impl Into<String>) -> Row {
        Row { symbol: Some(symbol), text: text.into() }
    }
}

/// Width and height of a legend symbol.
pub const SYMBOL: f32 = 16.0;
const PAD: f32 = 8.0;
const GAP: f32 = 8.0;
const LINE_GAP: f32 = 3.0;
/// Distance of the box from the view's edge.
pub const MARGIN: f32 = 10.0;

/// Where a box of `size` goes inside `area`, `offset` further in from its corner. The box always
/// stays inside the area (a view smaller than the box pins it to the corner).
pub fn place(area: Rect, corner: Corner, size: Vec2, offset: Vec2) -> Rect {
    let x = match corner {
        Corner::TopLeft | Corner::BottomLeft => area.left() + MARGIN + offset.x,
        Corner::TopRight | Corner::BottomRight => area.right() - MARGIN - offset.x - size.x,
    };
    let y = match corner {
        Corner::TopLeft | Corner::TopRight => area.top() + MARGIN + offset.y,
        Corner::BottomLeft | Corner::BottomRight => area.bottom() - MARGIN - offset.y - size.y,
    };
    let x = x.min(area.right() - size.x).max(area.left());
    let y = y.min(area.bottom() - size.y).max(area.top());
    Rect::from_min_size(pos2(x, y), size)
}

/// Paint the box (clipped to `area`) and return its rectangle, or `None` with nothing to show.
/// `offset` keeps it clear of controls drawn in the same corner.
pub fn paint(painter: &Painter, t: &Tokens, area: Rect, corner: Corner, offset: Vec2, rows: &[Row]) -> Option<Rect> {
    if rows.is_empty() {
        return None;
    }
    let font = t.font(11.0);
    let galleys: Vec<_> = rows.iter().map(|r| painter.layout_no_wrap(r.text.clone(), font.clone(), t.text)).collect();
    let lead = if rows.iter().any(|r| r.symbol.is_some()) { SYMBOL + GAP } else { 0.0 };
    let text_w = galleys.iter().map(|g| g.size().x).fold(0.0_f32, f32::max);
    let rows_h: f32 =
        galleys.iter().map(|g| g.size().y.max(if lead > 0.0 { SYMBOL } else { 0.0 })).sum::<f32>() + LINE_GAP * (galleys.len() - 1) as f32;
    let size = vec2(text_w + lead + 2.0 * PAD, rows_h + 2.0 * PAD);
    let r = place(area, corner, size, offset);
    let p = painter.with_clip_rect(area);
    p.rect(r, CornerRadius::same(4), t.chrome.gamma_multiply(0.86), Stroke::new(1.0, t.divider), StrokeKind::Inside);
    let mut y = r.top() + PAD;
    for (row, g) in rows.iter().zip(galleys) {
        let h = g.size().y.max(if lead > 0.0 { SYMBOL } else { 0.0 });
        if let Some(s) = row.symbol {
            draw_symbol(&p, t, s, Rect::from_min_size(pos2(r.left() + PAD, y + (h - SYMBOL) / 2.0), vec2(SYMBOL, SYMBOL)));
        }
        p.galley(pos2(r.left() + PAD + lead, y + (h - g.size().y) / 2.0), g, t.text);
        y += h + LINE_GAP;
    }
    Some(r)
}

fn draw_symbol(p: &Painter, t: &Tokens, s: Symbol, r: Rect) {
    let c: Pos2 = r.center();
    match s {
        Symbol::Disc { fill, label } => {
            p.circle(c, SYMBOL / 2.0 - 1.0, fill, Stroke::new(1.5, Color32::WHITE));
            if let Some(l) = label {
                p.text(c, Align2::CENTER_CENTER, l, t.semibold(9.0), Color32::WHITE);
            }
        }
        Symbol::Line { color } => {
            p.line_segment(
                [pos2(r.left() + 1.0, c.y), pos2(r.right() - 1.0, c.y)],
                Stroke::new(4.0, Color32::from_rgba_unmultiplied(255, 255, 255, 170)),
            );
            p.line_segment([pos2(r.left() + 1.0, c.y), pos2(r.right() - 1.0, c.y)], Stroke::new(2.0, color));
        }
        Symbol::Ring { color } => {
            p.circle(c, SYMBOL / 2.0 - 1.5, color.gamma_multiply(0.15), Stroke::new(1.5, color));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rows: &[Row], area: Rect, corner: Corner) -> Option<Rect> {
        let ctx = egui::Context::default();
        let mut out = None;
        let mut full = ctx.run_ui(egui::RawInput::default(), |ui| {
            let ctx = ui.ctx().clone();
            let t = Tokens::get(&ctx);
            out = paint(&ctx.layer_painter(egui::LayerId::background()), &t, area, corner, Vec2::ZERO, rows);
        });
        full.textures_delta.clear();
        out
    }

    #[test]
    fn the_box_sits_in_its_corner_inside_the_view() {
        let area = Rect::from_min_size(pos2(100.0, 50.0), vec2(600.0, 400.0));
        let rows = [Row::text("Paris, France"), Row::symbol(Symbol::Line { color: Color32::RED }, "Tracklog")];
        for corner in [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight] {
            let r = run(&rows, area, corner).expect("a box");
            assert!(area.contains_rect(r), "{corner:?}: {r:?} outside {area:?}");
            let (right, bottom) = (r.center().x > area.center().x, r.center().y > area.center().y);
            assert_eq!(right, matches!(corner, Corner::TopRight | Corner::BottomRight), "{corner:?} on the wrong side");
            assert_eq!(bottom, matches!(corner, Corner::BottomLeft | Corner::BottomRight), "{corner:?} on the wrong edge");
        }
    }

    #[test]
    fn more_rows_make_a_taller_box_and_a_tiny_view_still_holds_it() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0));
        let one = run(&[Row::text("a")], area, Corner::TopLeft).expect("a box");
        let three = run(&[Row::text("a"), Row::text("b"), Row::text("c")], area, Corner::TopLeft).expect("a box");
        assert!(three.height() > one.height());
        let tiny = Rect::from_min_size(pos2(5.0, 5.0), vec2(30.0, 20.0));
        let r = run(&[Row::text("a wide line of text")], tiny, Corner::BottomRight).expect("a box");
        assert_eq!((r.left(), r.top()), (tiny.left(), tiny.top()), "pinned to the view's corner, never outside it");
        assert!(run(&[], area, Corner::TopLeft).is_none());
    }
}
