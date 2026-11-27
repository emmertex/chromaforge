//! Settings ▸ search: the field over the sections' list finds a section by any text its page shows,
//! in the app's language, and marks the matches in the open page.
//!
//! There is no list of words to keep. Every page is drawn by `tab_body`, and [`around`] reads what
//! it drew: the text of every text shape, and the ids of the controls it registered (an SDK
//! command id such as `surface.enabled` finds the control's section too). The sections that aren't
//! open are drawn once, off screen ([`ensure`]), and what each page said is kept per language.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::epaint::{Galley, TextShape};
use egui::{Context, Id, LayerId, Order, Pos2, Rect, Shape, Ui, UiBuilder, pos2, vec2};

use crate::LccApp;
use crate::theme::Tokens;
use crate::widgets::Registry;

/// One page's entry: the lowercased text and command ids, one per line, and a signature of the
/// text it was made from (digits left out, so a counter doesn't count as a change).
#[derive(Clone, Default)]
struct Page {
    said: Arc<str>,
    /// The ids of the page's controls: found only by a query word that has a dot in it, so that
    /// `settings` doesn't list every page for ids it never shows.
    ids: Arc<str>,
    signature: u64,
}

/// What each page said, by (language, section). Shared by `Arc`: reading it costs no copy.
#[derive(Clone, Default)]
struct Index(Arc<HashMap<(String, String), Page>>);

fn index_id() -> Id {
    Id::new("settings-search-index")
}

fn indexing_id() -> Id {
    Id::new("settings-search-indexing")
}

fn highlights_id() -> Id {
    Id::new("settings-search-highlights")
}

fn registry_id() -> Id {
    Id::new("lc-registry")
}

fn language() -> String {
    format!("{:?}", crate::i18n::language())
}

/// `text` for matching: lowercase, one char for each char it had (so a position in the folded text
/// is a position in the original).
pub fn fold(text: &str) -> String {
    text.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

/// The words of a search query, folded; empty when there is nothing to look for.
pub fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(fold).collect()
}

/// Whether the page being drawn is being read off screen for the index: nobody sees it, so it must
/// not start anything (a scan, a discovery, a count on a thread) that opening the page would.
pub(crate) fn off_screen(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(indexing_id())).unwrap_or(false)
}

/// Whether `section` (named `name` in the list) holds every word of `words`; `None` while its page
/// hasn't been read yet.
pub fn matches(ctx: &Context, section: &str, name: &str, words: &[String]) -> Option<bool> {
    let key = (language(), section.to_string());
    let index = ctx.data(|d| d.get_temp::<Index>(index_id()))?;
    let page = index.0.get(&key)?;
    let name = fold(name);
    Some(words.iter().all(|w| name.contains(w.as_str()) || page.said.contains(w.as_str()) || (w.contains('.') && page.ids.contains(w.as_str()))))
}

/// How many sections have been read (for tests).
#[cfg(test)]
pub fn indexed_sections(ctx: &Context) -> usize {
    ctx.data(|d| d.get_temp::<Index>(index_id())).map_or(0, |i| i.0.len())
}

/// Drops what the pages said (Settings closed): the next search reads them again.
pub fn forget(ctx: &Context) {
    if ctx.data(|d| d.get_temp::<Index>(index_id()).is_some()) {
        ctx.data_mut(|d| d.remove::<Index>(index_id()));
    }
}

/// How many spans the last page drawn marked.
#[cfg(test)]
pub fn highlighted(ctx: &Context) -> usize {
    ctx.data(|d| d.get_temp::<usize>(highlights_id())).unwrap_or(0)
}

fn indexed(ctx: &Context, section: &str) -> bool {
    let key = (language(), section.to_string());
    ctx.data(|d| d.get_temp::<Index>(index_id()).is_some_and(|i| i.0.contains_key(&key)))
}

/// Puts the off-screen flag back however the page's drawing ends (a panic the last-resort guard
/// catches must not leave the open page thought of as off screen).
struct OffScreen<'a>(&'a Context);

impl<'a> OffScreen<'a> {
    fn begin(ctx: &'a Context) -> Self {
        ctx.data_mut(|d| d.insert_temp(indexing_id(), true));
        Self(ctx)
    }
}

impl Drop for OffScreen<'_> {
    fn drop(&mut self) {
        self.0.data_mut(|d| d.insert_temp(indexing_id(), false));
    }
}

/// Reads every section's page that isn't `open` and hasn't been read in this language, off screen.
/// (The open page is read as it is drawn for real: the same widgets can't be on screen twice. No
/// two sections share a literal widget id, so the pages read here and the open one never clash.)
/// A page is read once while Settings is open; [`forget`] drops it when Settings closes.
pub fn ensure(app: &mut LccApp, ui: &mut Ui, open: &str, sections: &[&'static str]) {
    let ctx = ui.ctx().clone();
    for section in sections.iter().copied().filter(|s| *s != open && !indexed(&ctx, s)) {
        let t = Tokens::get(&ctx);
        let layer = LayerId::new(Order::Background, Id::new("settings-search-offscreen"));
        // a page's room far outside the screen; its widgets are laid out and drawn there, which no one sees
        let room = Rect::from_min_size(pos2(-60000.0, -60000.0), vec2(560.0, 40000.0));
        let mut child = ui.new_child(UiBuilder::new().id_salt(("settingsSearchPage", section)).layer_id(layer).max_rect(room));
        child.set_clip_rect(room);
        let _flag = OffScreen::begin(&ctx);
        crate::dialog_frame::plain_form(&mut child, crate::panels::settings::LABEL_W, |ui| {
            crate::dialog_frame::group_begin(ui, None);
            crate::panels::settings::tab_body(app, ui, &t, section);
        });
    }
}

/// The text shapes drawn in `layer` from shape number `first` on: where, clipped to what, and the text.
fn drawn_text(ctx: &Context, layer: LayerId, first: usize) -> Vec<(Rect, Pos2, Arc<Galley>)> {
    let mut out = Vec::new();
    ctx.graphics(|g| {
        for c in g.get(layer).into_iter().flat_map(|l| l.all_entries().skip(first)) {
            texts_of(&c.shape, &mut |t| out.push((c.clip_rect, t.pos, Arc::clone(&t.galley))));
        }
    });
    out
}

/// Draws a page with `draw`, reads what it drew into the index while a search is on, and marks the
/// matches in it. (Not nested: a page that drew another page through this would lose that page's
/// registered ids when read off screen.)
pub fn around(app: &mut LccApp, ui: &mut Ui, tab: &str, draw: impl FnOnce(&mut LccApp, &mut Ui)) {
    let ctx = ui.ctx().clone();
    let words = words(&app.modern_sheets.settings.search);
    let off = off_screen(&ctx);
    if words.is_empty() && !off {
        draw(app, ui);
        ctx.data_mut(|d| d.insert_temp(highlights_id(), 0usize));
        return;
    }
    let layer = ui.layer_id();
    let first = ctx.graphics_mut(|g| g.entry(layer).next_idx()).0;
    let registered = ctx.data_mut(|d| d.get_temp_mut_or_default::<Registry>(registry_id()).0.len());
    draw(app, ui);
    let texts = drawn_text(&ctx, layer, first);
    let ids: Vec<String> = ctx.data_mut(|d| {
        let registry = d.get_temp_mut_or_default::<Registry>(registry_id());
        let ids = registry.0.iter().skip(registered).filter_map(|(id, _)| command_id(id)).collect();
        if off {
            // (its widgets answer to nothing: the page's ids are not the screen's)
            registry.0.truncate(registered);
        }
        ids
    });
    remember(&ctx, tab, &texts, ids);
    if off {
        return;
    }
    let t = Tokens::get(&ctx);
    let edge = egui::Stroke::new(1.0, t.find_edge);
    let mut spans = 0;
    for (clip, pos, galley) in &texts {
        let marks = spans_of(*pos, galley, &words);
        spans += marks.len();
        for r in marks {
            ctx.graphics_mut(|g| {
                g.entry(layer).add(*clip, Shape::Rect(egui::epaint::RectShape::new(r.expand(1.0), 2.0, t.find, edge, egui::StrokeKind::Outside)))
            });
        }
    }
    ctx.data_mut(|d| d.insert_temp(highlights_id(), spans));
}

/// Keeps what a page said, when it said something else than the last time. A change in digits only
/// (a counter, a size) isn't one, and neither asks for another frame nor rebuilds the text: a name
/// that changes only in a digit ("SAM 2" to "SAM 3") is found by its old digit until the next visit.
fn remember(ctx: &Context, tab: &str, texts: &[(Rect, Pos2, Arc<Galley>)], ids: Vec<String>) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (_, _, galley) in texts {
        galley.text().chars().filter(|c| !c.is_ascii_digit()).for_each(|c| c.hash(&mut hasher));
        '\n'.hash(&mut hasher);
    }
    ids.hash(&mut hasher);
    let signature = hasher.finish();
    let key = (language(), crate::settings_screen::section_of(tab).to_string());
    let same = ctx.data(|d| d.get_temp::<Index>(index_id()).and_then(|i| i.0.get(&key).map(|p| p.signature)) == Some(signature));
    if same {
        return;
    }
    let mut said: Vec<String> = texts.iter().map(|(_, _, g)| fold(g.text())).collect();
    said.sort();
    said.dedup();
    let ids: Vec<String> = ids.iter().map(|i| fold(i)).collect();
    let page = Page { said: said.join("\n").into(), ids: ids.join("\n").into(), signature };
    ctx.data_mut(|d| {
        let index = d.get_temp_mut_or_default::<Index>(index_id());
        Arc::make_mut(&mut index.0).insert(key, page);
    });
    // (the list was drawn before this page, with what it knew)
    ctx.request_repaint();
}

/// `id` of a registered widget (`check:surface.enabled`) as the command id it names, when it names one.
fn command_id(widget: &str) -> Option<String> {
    let (_, id) = widget.split_once(':')?;
    let command = id.contains('.') && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    command.then(|| id.to_string())
}

fn texts_of<'a>(shape: &'a Shape, f: &mut impl FnMut(&'a TextShape)) {
    match shape {
        Shape::Text(t) => f(t),
        Shape::Vec(v) => v.iter().for_each(|s| texts_of(s, f)),
        _ => {}
    }
}

/// The rectangles, on screen, of every place in `text` where one of `words` is.
fn spans_of(pos: Pos2, galley: &Galley, words: &[String]) -> Vec<Rect> {
    let folded: Vec<char> = fold(galley.text()).chars().collect();
    let mut marked = vec![false; folded.len()];
    for word in words {
        let word: Vec<char> = word.chars().collect();
        if word.is_empty() || word.len() > folded.len() {
            continue;
        }
        for start in 0..=(folded.len() - word.len()) {
            if folded.get(start..start + word.len()) == Some(word.as_slice()) {
                marked.iter_mut().skip(start).take(word.len()).for_each(|m| *m = true);
            }
        }
    }
    let at = pos.to_vec2();
    let mut out: Vec<Rect> = Vec::new();
    for i in marked.iter().enumerate().filter(|(_, m)| **m).map(|(i, _)| i) {
        // (the first char of a wrapped row starts that row, not the end of the one before)
        let mut at_start = egui::text::CCursor::new(i);
        at_start.prefer_next_row = true;
        let from = galley.pos_from_cursor(at_start);
        let to = galley.pos_from_cursor(egui::text::CCursor::new(i + 1));
        if to.min.y > from.min.y + 0.5 {
            // (the char ends on a later row than it starts: nothing of it here)
            continue;
        }
        let r = Rect::from_min_max(pos2(from.min.x, from.min.y), pos2(to.min.x.max(from.min.x + 1.0), from.max.y)).translate(at);
        match out.last_mut() {
            Some(last) if (last.top() - r.top()).abs() < 0.5 && r.left() <= last.right() + 0.5 => *last = last.union(r),
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn galley(text: &str, wrap: f32) -> Arc<Galley> {
        let ctx = Context::default();
        let mut out = None;
        let mut pass = ctx.run_ui(Default::default(), |ui| {
            let ctx = ui.ctx();
            let job = egui::text::LayoutJob::simple(text.to_string(), egui::FontId::proportional(14.0), egui::Color32::WHITE, wrap);
            out = Some(ctx.fonts_mut(|f| f.layout_job(job)));
        });
        pass.textures_delta.clear();
        out.expect("the pass ran")
    }

    #[test]
    fn a_match_that_starts_a_wrapped_row_is_marked_from_its_first_letter() {
        // each word on a row of its own
        let g = galley("alpha beta gamma", 40.0);
        assert!(g.rows.len() >= 3, "the text wrapped: {} rows", g.rows.len());
        let whole = spans_of(pos2(0.0, 0.0), &g, &["beta".to_string()]);
        let first_letter_only = spans_of(pos2(0.0, 0.0), &g, &["b".to_string()]);
        assert_eq!(whole.len(), 1, "one mark for one word: {whole:?}");
        let w = whole[0];
        let b = first_letter_only.iter().find(|r| (r.top() - w.top()).abs() < 0.5).copied();
        let b = b.expect("the b of beta is marked, on beta's row");
        assert!((w.left() - b.left()).abs() < 0.5, "the mark starts at the first letter: {w:?} vs {b:?}");
        assert!(w.width() > 2.0 * b.width(), "and covers the word, not one letter: {w:?} vs {b:?}");
    }

    #[test]
    fn marks_follow_chars_not_bytes() {
        let g = galley("Größe der Miniaturen", 400.0);
        let marks = spans_of(pos2(0.0, 0.0), &g, &["größe".to_string()]);
        assert_eq!(marks.len(), 1);
        let plain = spans_of(pos2(0.0, 0.0), &g, &["der".to_string()]);
        assert!(marks[0].right() <= plain[0].left() + 0.5, "the mark ends before the next word starts");
    }
}
