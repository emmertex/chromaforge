//! Photo Grid (justified rows) and Square Grid. Virtualized: only visible cells request thumbnails.

#[cfg(test)]
#[path = "grid_header_tests.rs"]
mod header_tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lcc_catalog::{Catalog, ColorLabel, DateRun, Flag, GroupBy, PhotoId, SortKey, StackId};
use serde_json::json;

use crate::LccApp;
use crate::icons::{Icon, paint};
use crate::render::Slot;
use crate::state::ViewMode;
use crate::theme::Tokens;
use crate::widgets::register;

/// Thumbnail render size (pixels, long edge) for a cell of `pts` points.
fn thumb_px(pts: f32, ppp: f32) -> usize {
    let px = (pts * ppp).ceil() as usize;
    if px <= 256 {
        256
    } else if px <= 384 {
        384
    } else {
        512
    }
}

/// Counters of the grid's work, cumulative over frames (benchmarks and tests: an unchanged frame
/// must not rebuild the view, its layout or its indexes, and visits only the cells near the screen).
#[derive(Clone, Debug, Default)]
pub struct GridStats {
    /// Frames the grid was drawn.
    pub frames: u64,
    /// Cells looked at (tested against the viewport or drawn).
    pub cells_visited: u64,
    /// Layouts computed.
    pub layout_builds: u64,
    /// Per-view data (date runs, row index) built.
    pub view_builds: u64,
    /// Photos whose date and shape were read from the catalog (every photo of a list read whole;
    /// only the photos an edit touched of one followed: [`GridCache`]).
    pub photos_read: u64,
    /// Stack indexes built.
    pub stack_index_builds: u64,
    /// Wall-clock time of the last [`show`].
    pub last_show: web_time::Duration,
}

pub fn show(app: &mut LccApp, ui: &mut egui::Ui) {
    let t0 = web_time::Instant::now();
    // the filmstrip runs under the Grid too, as in Lightroom Classic (the fork's)
    let full = ui.max_rect();
    let film_h = Tokens::get(ui.ctx()).film_h;
    if crate::module::edge_visible(app, crate::module::Edge::Bottom) && !app.ui.fullscreen && full.height() > film_h * 2.0 {
        let grid = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - film_h));
        ui.scope_builder(egui::UiBuilder::new().max_rect(grid), |ui| show_inner(app, ui));
        super::detail::filmstrip(app, ui, Rect::from_min_max(pos2(full.left(), grid.bottom()), full.max));
    } else {
        show_inner(app, ui);
    }
    app.caches.grid_stats.frames += 1;
    app.caches.grid_stats.last_show = t0.elapsed();
}

/// Photo → (stack, position in the stack).
type StackIndex = HashMap<PhotoId, (StackId, usize)>;

/// What the date runs depend on: the visible list's generation, the date the grid groups by
/// (`None`: no headers) and the grouping.
type RunsKey = (u64, Option<SortKey>, GroupBy);

/// The most photos an edit may touch for the grid to read only those again (each costs a lookup
/// and a date parse; past this, reading the whole list is as quick).
const MAX_FOLLOWED: usize = 4096;

/// What the grid derives from the visible photos, kept across frames. Everything is keyed by the
/// session's visible-list generation ([`lcc_engine::Session::visible_shared`]), which
/// changes whenever the list is recomputed (any catalog change: crops, orientation, dates, stacks;
/// or another source, filter or sort) — so an unchanged frame neither copies nor hashes the ids.
#[derive(Default)]
pub struct GridCache {
    runs: Option<(RunsKey, Arc<Vec<DateRun>>)>,
    /// Layout by (runs key, width, thumbnail size, square).
    layout: Option<((RunsKey, u32, u32, bool), Arc<GridLayout>)>,
    /// Stack index by generation.
    stacks: Option<(u64, Arc<StackIndex>)>,
    /// How many of the photos are browsed (Local) ones, by generation.
    local: Option<(u64, usize)>,
    /// Collections (plain albums) holding each photo, by generation (the collection badge).
    albums: Option<(u64, Arc<HashMap<PhotoId, usize>>)>,
    /// A published collection's sections (new / modified / published), by generation.
    publish: Option<(u64, Option<Arc<Vec<DateRun>>>)>,
    /// What was read of each photo of the list last shown.
    read: Option<ListRead>,
}

/// Header keys of a published collection's sections, in the order the engine sorts them.
const PUBLISH_SECTIONS: [&str; 4] = ["publish:new", "publish:modified", "publish:published", "publish:remove"];

/// The header of a published collection's section, as Lightroom Classic names them.
fn publish_section_label(key: &str) -> Option<&'static str> {
    Some(match key {
        "publish:new" => "New Photos to Publish",
        "publish:modified" => "Modified Photos to Re-Publish",
        "publish:published" => "Published Photos",
        "publish:remove" => "Deleted Photos to Remove",
        _ => return None,
    })
}

/// A published collection's photos as runs of one section each (`ids` in the engine's order:
/// new, modified, published, to remove); photos in no section are left out.
pub fn publish_runs(sections: &HashMap<PhotoId, u8>, ids: &[PhotoId]) -> Vec<DateRun> {
    let mut runs: Vec<DateRun> = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let Some(key) = sections.get(id).and_then(|r| PUBLISH_SECTIONS.get(usize::from(*r))) else { continue };
        match runs.last_mut() {
            Some(last) if last.key == *key && last.start + last.count == i => last.count += 1,
            _ => runs.push(DateRun { key: (*key).to_string(), label: String::new(), start: i, count: 1 }),
        }
    }
    runs
}

/// What the grid read of each photo of a list, position by position: its own date group (a
/// stack's members take their first photo's when the runs are made) and its cell's shape. A new
/// list that differs from this one by the photos an edit touched (an import's batch, a capture
/// time, a crop) is read again only for those: at 180,000 photos, reading every photo again took
/// most of a 100 ms frame for each batch an import added.
struct ListRead {
    ids: Arc<[PhotoId]>,
    /// The catalog revision it was read at.
    revision: u64,
    /// The date and grouping of the groups (`None`: the list has no date runs, no group is read).
    dates: Option<(SortKey, GroupBy)>,
    /// Each photo's group, an index into `names`.
    groups: Vec<u32>,
    names: Vec<String>,
    named: HashMap<String, u32>,
    /// Each photo's shape; `None` for the square grid, whose cells are all alike.
    shapes: Option<Vec<f32>>,
}

impl ListRead {
    fn new(ids: Arc<[PhotoId]>, revision: u64, dates: Option<(SortKey, GroupBy)>, shaped: bool) -> ListRead {
        let n = ids.len();
        let groups = Vec::with_capacity(if dates.is_some() { n } else { 0 });
        ListRead { ids, revision, dates, groups, names: Vec::new(), named: HashMap::new(), shapes: shaped.then(|| Vec::with_capacity(n)) }
    }

    /// Read photo `id` from the catalog and put it next.
    fn read(&mut self, cat: &Catalog, id: PhotoId) {
        if let Some((key, by)) = self.dates {
            let name = cat.date_group(id, key, by);
            let next = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
            let g = match self.named.get(&name) {
                Some(g) => *g,
                None => {
                    self.named.insert(name.clone(), next);
                    self.names.push(name);
                    next
                }
            };
            self.groups.push(g);
        }
        if let Some(shapes) = self.shapes.as_mut() {
            shapes.push(shape_of(cat, id));
        }
    }

    /// Read every photo of `ids`.
    fn whole(stats: &mut GridStats, cat: &Catalog, ids: &Arc<[PhotoId]>, dates: Option<(SortKey, GroupBy)>, shaped: bool) -> ListRead {
        let mut r = ListRead::new(ids.clone(), cat.revision, dates, shaped);
        if r.dates.is_some() || shaped {
            for id in ids.iter() {
                r.read(cat, *id);
            }
            stats.photos_read += ids.len() as u64;
        }
        r
    }

    /// `ids` from what was read of the list before it: the photos the catalog says changed since
    /// are read again, the others are taken over. `None` when the list is not this one with
    /// those photos changed, added or removed (another view, order or grouping; an edit to more
    /// than photos of its own, such as a stack): read it whole then.
    fn follow(self, stats: &mut GridStats, cat: &Catalog, ids: &Arc<[PhotoId]>, dates: Option<(SortKey, GroupBy)>, shaped: bool) -> Option<ListRead> {
        if self.dates != dates || self.shapes.is_some() != shaped {
            // (a switch between the photo grid and the square grid reads every photo once, on
            // purpose: the other mode's data was never read, so there is nothing to follow)
            return None;
        }
        let touched: HashSet<PhotoId> = cat.touched_since(self.revision)?.into_iter().collect();
        if touched.len() > MAX_FOLLOWED {
            return None;
        }
        let mut r = ListRead::new(ids.clone(), cat.revision, dates, shaped);
        // (the groups' names go on, unpruned: pruning would renumber every photo's group; they are
        // only reset by a whole read, and grow by a short string per day or month an edit puts a
        // photo in that no photo was in before)
        r.names = self.names;
        r.named = self.named;
        let mut was = 0;
        let mut fresh = 0u64;
        for id in ids.iter() {
            if touched.contains(id) {
                r.read(cat, *id);
                fresh += 1;
                continue;
            }
            // (the photos touched are read again wherever they are now)
            while self.ids.get(was).is_some_and(|w| touched.contains(w)) {
                was += 1;
            }
            // an untouched photo is where it was, among the others: else this is another list
            if self.ids.get(was) != Some(id) {
                return None;
            }
            if dates.is_some() {
                r.groups.push(*self.groups.get(was)?);
            }
            if let (Some(to), Some(from)) = (r.shapes.as_mut(), self.shapes.as_ref()) {
                to.push(*from.get(was)?);
            }
            was += 1;
        }
        // no untouched photo left the list
        if self.ids.get(was..).is_some_and(|rest| rest.iter().any(|w| !touched.contains(w))) {
            return None;
        }
        stats.photos_read += fresh;
        Some(r)
    }
}

impl GridCache {
    /// The date runs of `ids`, reading what has to be read of them (`square`: the cells' shapes
    /// are not wanted).
    fn runs(&mut self, stats: &mut GridStats, cat: &Catalog, ids: &Arc<[PhotoId]>, key: RunsKey, square: bool) -> Arc<Vec<DateRun>> {
        let shaped = !square;
        match &self.runs {
            Some((k, r)) if *k == key && self.read.as_ref().is_some_and(|read| read.shapes.is_some() == shaped) => r.clone(),
            _ => {
                stats.view_builds += 1;
                let dates = key.1.map(|sort| (sort, key.2)).filter(|(sort, by)| lcc_catalog::dates::groups_by_date(*sort, *by));
                let read = match self.read.take() {
                    // (the same list, read at this revision)
                    Some(r) if Arc::ptr_eq(&r.ids, ids) && r.revision == cat.revision && r.dates == dates && r.shapes.is_some() == shaped => r,
                    Some(r) => r.follow(stats, cat, ids, dates, shaped).unwrap_or_else(|| ListRead::whole(stats, cat, ids, dates, shaped)),
                    None => ListRead::whole(stats, cat, ids, dates, shaped),
                };
                let r = Arc::new(match dates {
                    Some(_) if !ids.is_empty() => {
                        let stacks = self.stacks(stats, cat, key.0);
                        let group = |i: usize, _: PhotoId| read.groups.get(i).and_then(|g| read.names.get(*g as usize)).map_or("", String::as_str);
                        lcc_catalog::dates::date_runs_of(ids, &stacks, group)
                    }
                    _ => Vec::new(),
                });
                self.read = Some(read);
                self.runs = Some((key, r.clone()));
                r
            }
        }
    }

    fn publish(&mut self, session: &lcc_engine::Session, ids: &[PhotoId], generation: u64) -> Option<Arc<Vec<DateRun>>> {
        match &self.publish {
            Some((g, r)) if *g == generation => r.clone(),
            _ => {
                let r = match session.source {
                    #[cfg(not(target_arch = "wasm32"))]
                    lcc_engine::LibrarySource::Album(a) => lcc_engine::cmd::publish::sections(session, a).map(|m| Arc::new(publish_runs(&m, ids))),
                    _ => None,
                };
                self.publish = Some((generation, r.clone()));
                r
            }
        }
    }

    /// The shape of each cell of the list last shown (width / height), in list order; empty for
    /// the square grid.
    fn shapes(&self) -> &[f32] {
        self.read.as_ref().and_then(|r| r.shapes.as_deref()).unwrap_or(&[])
    }

    /// The date runs and the cell shapes the grid shows now (tests compare them with a grid made
    /// afresh).
    #[cfg(test)]
    pub(crate) fn runs_and_shapes(&self) -> Option<(Arc<Vec<DateRun>>, Vec<f32>)> {
        Some((self.runs.as_ref()?.1.clone(), self.shapes().to_vec()))
    }

    fn stacks(&mut self, stats: &mut GridStats, cat: &Catalog, generation: u64) -> Arc<StackIndex> {
        match &self.stacks {
            Some((g, s)) if *g == generation => s.clone(),
            _ => {
                stats.stack_index_builds += 1;
                let s = Arc::new(cat.stack_index());
                self.stacks = Some((generation, s.clone()));
                s
            }
        }
    }

    fn albums(&mut self, cat: &Catalog, generation: u64) -> Arc<HashMap<PhotoId, usize>> {
        match &self.albums {
            Some((g, m)) if *g == generation => m.clone(),
            _ => {
                let mut m: HashMap<PhotoId, usize> = HashMap::new();
                for al in cat.albums().filter(|a| !a.is_smart() && !a.folder) {
                    for p in &al.photos {
                        *m.entry(*p).or_default() += 1;
                    }
                }
                let m = Arc::new(m);
                self.albums = Some((generation, m.clone()));
                m
            }
        }
    }

    fn local(&mut self, cat: &Catalog, ids: &[PhotoId], generation: u64) -> usize {
        match self.local {
            Some((g, n)) if g == generation => n,
            _ => {
                let n = ids.iter().filter(|id| cat.photo(**id).is_some_and(|p| p.local)).count();
                self.local = Some((generation, n));
                n
            }
        }
    }
}

/// Width / height of a photo's cell: its cropped, oriented shape.
pub(crate) fn shape_of(cat: &Catalog, id: PhotoId) -> f32 {
    let p = cat.photo(id);
    let (w, h) = p.map(|p| (p.width.max(1) as f32, p.height.max(1) as f32)).unwrap_or((3.0, 2.0));
    let swap = p.is_some_and(|p| p.develop.orientation.swaps_axes());
    let crop = p.map(|p| p.develop.crop.geometry.rect).unwrap_or(lcc_geom::Rect::UNIT);
    let (w, h) = if swap { (h, w) } else { (w, h) };
    (w * crop.width() as f32) / (h * crop.height() as f32).max(1e-3)
}

/// The rows (of a layout's `rows`, top to bottom) that meet the y range `top..=bottom`, found by
/// binary search.
pub fn rows_between(rows: &[GridRow], top: f32, bottom: f32) -> &[GridRow] {
    let first = rows.partition_point(|r| r.bottom < top);
    let rest = rows.get(first..).unwrap_or(&[]);
    let n = rest.partition_point(|r| r.top <= bottom);
    rest.get(..n).unwrap_or(&[])
}

/// The photo the grid keeps in place on screen while its layout reflows: the active photo when
/// it is on screen, otherwise the one in the middle of the view. Picked again only when the user
/// scrolls or the active photo changes, never by a reflow, so a run of small resizes does not
/// drift from photo to photo.
#[derive(Clone, Debug, PartialEq)]
pub struct GridAnchor {
    /// Which layout it was last seen in (changes when the layout is rebuilt).
    pub lay: (usize, usize, u32),
    pub photo: PhotoId,
    /// Its index in the photo list last seen (a hint; looked up again when it no longer matches).
    pub idx: usize,
    /// The photo's top minus the view's top, in points.
    pub dy: f32,
    pub seen_top: f32,
    pub active: Option<PhotoId>,
}

impl GridAnchor {
    fn index_in(&self, ids: &[PhotoId]) -> Option<usize> {
        if ids.get(self.idx) == Some(&self.photo) { Some(self.idx) } else { ids.iter().position(|p| *p == self.photo) }
    }

    /// The scroll offset that keeps the anchor where it was, when the layout changed since.
    pub fn reflow_offset(&self, lay_id: (usize, usize, u32), ids: &[PhotoId], lay: &GridLayout) -> Option<f32> {
        // a grid scrolled to the very top stays there (new photos arriving above show)
        if self.lay == lay_id || self.seen_top <= 0.5 {
            return None;
        }
        let cell = lay.cells.get(self.index_in(ids)?)?;
        let y = cell.top() - self.dy;
        y.is_finite().then_some(y.max(0.0))
    }

    /// The anchor after this frame (`viewport`: the grid's visible part, in layout coordinates).
    pub fn follow(
        last: Option<GridAnchor>,
        lay_id: (usize, usize, u32),
        ids: &[PhotoId],
        lay: &GridLayout,
        viewport: Rect,
        active: Option<PhotoId>,
    ) -> Option<GridAnchor> {
        let top = viewport.top();
        if let Some(a) = last {
            let reflowed = a.lay != lay_id;
            let user_moved = !reflowed && (a.seen_top != top || a.active != active);
            if !user_moved && let Some(idx) = a.index_in(ids) {
                return Some(GridAnchor { lay: lay_id, idx, seen_top: top, ..a });
            }
        }
        let on_screen = |i: usize| lay.cells.get(i).is_some_and(|c| c.intersects(viewport));
        let idx = active.and_then(|p| ids.iter().position(|x| *x == p)).filter(|i| on_screen(*i)).or_else(|| {
            let mid = viewport.center().y;
            let row = rows_between(&lay.rows, mid, mid).first().or_else(|| rows_between(&lay.rows, top, viewport.bottom()).first())?;
            Some(row.start + (row.end.saturating_sub(row.start)) / 2)
        })?;
        let (photo, cell) = (*ids.get(idx)?, lay.cells.get(idx)?);
        Some(GridAnchor { lay: lay_id, photo, idx, dy: cell.top() - top, seen_top: top, active })
    }
}

fn show_inner(app: &mut LccApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let (generation, ids) = app.session.visible_shared();
    // header: source title + count
    let sel_n = app.session.selection.ids.len();
    let chips = lcc_engine::filter_chips(&app.session.filter, &app.session.catalog);
    let total = app.session.source_total();
    let counted = super::chips::count_text(ids.len(), total, !chips.is_empty());
    let cnt = if sel_n > 1 { crate::i18n::tr_format!("{sel_n} selected · {counted}", counted = counted, sel_n = sel_n) } else { counted };
    match app.session.browse.clone().filter(|_| app.session.source == lcc_engine::LibrarySource::Folder) {
        Some(b) => {
            let local = app.caches.grid.local(&app.session.catalog, &ids, generation);
            folder_header(app, ui, &b, &ids, local, &cnt);
        }
        None => {
            let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
            ui.painter().rect_filled(hr, 0.0, t.canvas);
            let full = crate::i18n::source_title(&app.session);
            let mut count_w = ui.painter().layout_no_wrap(cnt.clone(), t.font(12.5), t.text_dim).size().x;
            // Recently Deleted: an Empty button left of the count (it asks first)
            if app.session.source == lcc_engine::LibrarySource::RecentlyDeleted && !ids.is_empty() {
                let label = crate::i18n::tr("Empty Recently Deleted");
                let bw = ui.painter().layout_no_wrap(label.to_string(), t.semibold(11.5), t.text).size().x + 18.0;
                let br = egui::Rect::from_center_size(pos2(hr.right() - 30.0 - count_w - bw / 2.0, hr.center().y), vec2(bw, 24.0));
                let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(br));
                if crate::widgets::text_button(&mut bar, "emptyRecentlyDeleted", "Empty Recently Deleted", false).clicked() {
                    let _ = crate::menubar::run_item(app, "library.emptyRecentlyDeleted", serde_json::json!({}));
                }
                count_w += bw + 10.0;
            }
            // the title gives way to the count: a long album or folder name is trimmed, in full on hover
            let room = (hr.width() - 20.0 - count_w - 36.0).max(0.0);
            let font = t.semibold(17.0);
            let title = crate::widgets::elide_head(&full, room, |s| ui.painter().layout_no_wrap(s.to_string(), font.clone(), t.text).size().x);
            let title_rect = ui.painter().text(pos2(hr.left() + 20.0, hr.center().y), Align2::LEFT_CENTER, &title, font, t.text);
            let count_rect = ui.painter().text(pos2(hr.right() - 20.0, hr.center().y), Align2::RIGHT_CENTER, cnt, t.font(12.5), t.text_dim);
            crate::widgets::register(ui.ctx(), "grid:title", title_rect);
            crate::widgets::register(ui.ctx(), "grid:count", count_rect);
            if title != full {
                ui.interact(title_rect, ui.id().with("grid-title"), Sense::hover()).on_hover_text(full);
            }
        }
    }
    super::chips::show(app, ui, &chips);
    crate::libtools::end_stroke(app, ui);
    crate::libtools::painter_bar(app, ui);
    if app.ui.filter_bar {
        super::filterbar::show(app, ui);
    }
    app.canvas_rect = Some(ui.max_rect());
    if ids.is_empty() {
        if !chips.is_empty() {
            let body = match total {
                Some(n) if n > 0 => {
                    crate::i18n::tr_format!("{n} photos in {} are hidden by the filters above", crate::i18n::source_title(&app.session), n = n)
                }
                _ => "Remove a filter above, or choose Clear all".to_string(),
            };
            super::empty_message(ui, ui.max_rect(), "No photos match the active filters", &body);
        } else if app.session.filter != Default::default() {
            super::empty_message(
                ui,
                ui.max_rect(),
                "No matching photos",
                "A filter is hiding this view's photos: change it, or clear it (View → Clear Filters)",
            );
        } else if app.session.source == lcc_engine::LibrarySource::Folder {
            super::empty_message(ui, ui.max_rect(), "No photos in this folder", "Turn on Include subfolders, or pick another folder under Local");
        } else if super::empty_library::shown(&app.session) {
            super::empty_library::show(app, ui, ui.max_rect());
        } else {
            super::empty_message(ui, ui.max_rect(), "No photos", "Import photos with File → Import Photos… (Cmd+Shift+I), or drop them here");
        }
        return;
    }
    let ppp = ui.ctx().pixels_per_point();
    let square = app.ui.view == ViewMode::SquareGrid;
    let target = app.ui.thumb_size;
    let avail_w = ui.available_width() - 8.0;
    let by = resolve_group(app.session.sort.group, target);
    let group_key = match app.session.source {
        lcc_engine::LibrarySource::RecentlyDeleted => None,
        lcc_engine::LibrarySource::RecentlyAdded => Some(SortKey::ImportDate),
        _ => Some(app.session.sort.key),
    };
    let runs_key = (generation, group_key, by);
    let stats = &mut app.caches.grid_stats;
    // (read even for a published collection: the cell shapes the layout uses come with the runs)
    let date_runs = app.caches.grid.runs(stats, &app.session.catalog, &ids, runs_key, square);
    let runs = app.caches.grid.publish(&app.session, &ids, generation).unwrap_or(date_runs);
    // the layout only changes with the photos (and their shapes), the grouping, the width and
    // the thumbnail size
    let lay_key = (runs_key, avail_w.to_bits(), target.to_bits(), square);
    let lay = match &app.caches.grid.layout {
        Some((k, l)) if *k == lay_key => l.clone(),
        _ => {
            stats.layout_builds += 1;
            let ones;
            let shapes = if square {
                ones = vec![1.0; ids.len()];
                &ones[..]
            } else {
                app.caches.grid.shapes()
            };
            let spans: Vec<(usize, usize)> = runs.iter().map(|r| (r.start, r.count)).collect();
            let l = Arc::new(layout(shapes, &spans, avail_w, target, square));
            app.caches.grid.layout = Some((lay_key, l.clone()));
            l
        }
    };
    let stacks = app.caches.grid.stacks(stats, &app.session.catalog, generation);
    let albums = if app.ui.lib.cell_badges { Some(app.caches.grid.albums(&app.session.catalog, generation)) } else { None };
    let total_h = lay.height + 12.0;
    let active = app.session.selection.active;
    // bring the active photo into view when it changes (keyboard, click, command) or the grid
    // comes back on screen; otherwise the scroll position is the user's
    let scroll_to: Option<Rect> = if follow_active(ui.ctx(), egui::Id::new("grid-follow-active"), active, app.session.source) {
        active.and_then(|a| ids.iter().position(|x| *x == a)).and_then(|i| lay.cells.get(i).copied())
    } else {
        None
    };
    let mut visible_ids = HashSet::new();
    let mut visited = 0u64;
    // a wheel notch scrolls a third of the grid's height
    let scroll_rect = ui.available_rect_before_wrap();
    register(ui.ctx(), "gridArea", scroll_rect);
    crate::widgets::register(ui.ctx(), "grid:scroll", scroll_rect);
    let wheel = wheel_step(ui, egui::Id::new("grid-wheel"), scroll_rect, scroll_rect.height(), false, app.ui.settings.wheel_scroll_speed);
    // keep a photo where it is on screen when the layout reflows (a side panel or the window
    // resized, the thumbnail size or the filter changed): the pixel offset alone would land elsewhere
    let anchor_id = egui::Id::new("grid-anchor");
    let lay_id = (Arc::as_ptr(&lay) as usize, ids.len(), lay.height.to_bits());
    let last: Option<GridAnchor> = ui.ctx().data(|d| d.get_temp(anchor_id));
    let mut area_ui = egui::ScrollArea::vertical().id_salt("grid-scroll").auto_shrink([false, false]);
    if scroll_to.is_none()
        && let Some(y) = last.as_ref().and_then(|a| a.reflow_offset(lay_id, &ids, &lay))
    {
        area_ui = area_ui.vertical_scroll_offset(y);
    }
    area_ui.show_viewport(ui, |ui, viewport| {
        let next = GridAnchor::follow(last, lay_id, &ids, &lay, viewport, active);
        ui.ctx().data_mut(|d| match next {
            Some(a) => {
                d.insert_temp(anchor_id, a);
            }
            None => {
                d.remove::<GridAnchor>(anchor_id);
            }
        });
        if wheel != egui::Vec2::ZERO {
            ui.scroll_with_delta_animation(wheel, egui::style::ScrollAnimation::none());
        }
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), total_h), Sense::hover());
        let origin = area.min;
        app.grid_scroll = Some(viewport.top());
        if let Some(r) = scroll_to {
            ui.scroll_to_rect(r.translate(origin.to_vec2()), None);
        }
        // only the rows on screen and a screen above and below it (their thumbnails are prefetched)
        let prefetch = viewport.expand2(vec2(0.0, viewport.height()));
        for row in rows_between(&lay.rows, prefetch.top(), prefetch.bottom()) {
            for i in row.start..row.end {
                let (Some(&id), Some(&rect)) = (ids.get(i), lay.cells.get(i)) else { continue };
                visited += 1;
                if !rect.intersects(prefetch) {
                    continue;
                }
                visible_ids.insert(id);
                let r = rect.translate(origin.to_vec2());
                let onscreen = rect.intersects(viewport);
                cell(app, ui, id, r, square, onscreen, ppp, i);
                if onscreen {
                    super::cells::extras(app, ui, id, r, square, i, albums.as_deref());
                }
                if let Some((sid, pos)) = stacks.get(&id) {
                    stack_badge(app, ui, id, *sid, *pos, r, square);
                }
            }
        }
        // date headers (top to bottom); the current group's header sticks to the top while its
        // photos scroll by
        let first = lay.headers.partition_point(|h| h.bottom() < viewport.top());
        for (g, hr) in lay.headers.iter().enumerate().skip(first) {
            if hr.top() > viewport.bottom() {
                break;
            }
            if let Some(run) = runs.get(g) {
                group_header(app, ui, run, &ids, hr.translate(origin.to_vec2()), false);
            }
        }
        let sticky = lay.headers.partition_point(|h| h.top() <= viewport.top()).checked_sub(1);
        if let Some(g) = sticky
            && let Some(head) = lay.headers.get(g)
            && head.top() < viewport.top()
            && let Some(run) = runs.get(g)
        {
            // pushed up by the next header as it arrives
            let next_top = lay.headers.get(g + 1).map(|h| h.top()).unwrap_or(f32::INFINITY);
            let y = viewport.top().min(next_top - HEADER_H);
            let hr = Rect::from_min_size(pos2(0.0, y), vec2(area.width(), HEADER_H)).translate(origin.to_vec2());
            group_header(app, ui, run, &ids, hr, true);
        }
    });
    app.caches.grid_stats.cells_visited += visited;
    app.renderer.evict_thumbs(&visible_ids, 600);
    let _ = (Color32::BLACK, StrokeKind::Inside, Stroke::NONE);
}

/// Should a scrolling photo strip (`key`: the grid, the filmstrip) bring the active photo into
/// view on this pass? Yes when the active photo changed since the strip was last drawn, or when
/// the strip was not drawn on the previous pass (view switch, panel shown again). Otherwise the
/// scroll position is the user's: time passing, finished renders, edits to the photo or a released
/// scroll bar never move it (issue #11).
pub(crate) fn follow_active(ctx: &egui::Context, key: egui::Id, active: Option<PhotoId>, source: lcc_engine::LibrarySource) -> bool {
    let pass = ctx.cumulative_pass_nr();
    let last: Option<(Option<PhotoId>, lcc_engine::LibrarySource, u64)> = ctx.data(|d| d.get_temp(key));
    ctx.data_mut(|d| d.insert_temp(key, (active, source, pass)));
    match last {
        Some((was, where_, drawn)) => follow_changed(was, where_, drawn, active, source, pass),
        None => true,
    }
}

/// Scroll when the active photo changes, the strip was not drawn last pass, or the same photo
/// sits in a different library source (another album) (#1142).
pub(crate) fn follow_changed(
    was: Option<PhotoId>,
    where_was: lcc_engine::LibrarySource,
    drawn: u64,
    active: Option<PhotoId>,
    source: lcc_engine::LibrarySource,
    pass: u64,
) -> bool {
    was != active || where_was != source || drawn + 1 < pass
}

/// How far a mouse wheel scrolls a photo strip this frame (`key`: the grid, the filmstrip; `area`: its
/// rect on screen; `span`: its visible height, or width when `sideways`), for `Ui::scroll_with_delta_animation`
/// inside its `ScrollArea`. A notch scrolls [`crate::theme::WHEEL_STRIP_SHARE`] (a third) of `span`, as in
/// Lightroom (about two rows of a six-row grid), instead of the [`crate::theme::WHEEL_LINE_POINTS`] a side
/// panel scrolls. Not whole rows: the photos land somewhere else after each notch, so the move shows. It
/// jumps at once instead of gliding, so the photos stay sharp while scrolling; egui's glide of the notch is
/// swallowed until it ends. Trackpads report points and scroll as before. A vertical wheel scrolls a
/// `sideways` strip. `speed` scales the step (the Wheel scroll speed preference; clamped, 1 when not finite).
pub(crate) fn wheel_step(ui: &egui::Ui, key: egui::Id, area: Rect, span: f32, sideways: bool, speed: f32) -> egui::Vec2 {
    if !ui.rect_contains_pointer(area) || !span.is_finite() {
        ui.data_mut(|d| d.remove_temp::<bool>(key));
        return egui::Vec2::ZERO;
    }
    let (notches, gliding) = ui.input(|i| {
        let notches = i.events.iter().fold(egui::Vec2::ZERO, |sum, e| match e {
            egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta, modifiers, .. }
                if *modifiers == egui::Modifiers::NONE && delta.is_finite() =>
            {
                sum + *delta
            }
            _ => sum,
        });
        (notches, i.smooth_scroll_delta != egui::Vec2::ZERO)
    });
    if notches != egui::Vec2::ZERO {
        ui.data_mut(|d| d.insert_temp(key, true));
    } else if !gliding {
        ui.data_mut(|d| d.remove_temp::<bool>(key));
    }
    if !ui.data(|d| d.get_temp::<bool>(key)).unwrap_or(false) {
        return egui::Vec2::ZERO;
    }
    ui.ctx().input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    let speed = if speed.is_finite() { speed.clamp(0.25, 3.0) } else { 1.0 };
    let step = span * crate::theme::WHEEL_STRIP_SHARE * speed;
    if sideways { egui::vec2((notches.x + notches.y) * step, 0.0) } else { egui::vec2(0.0, notches.y * step) }
}

/// Height of a date header row (points).
pub const HEADER_H: f32 = 40.0;

/// Cell rectangles (one per photo, in order), date-header rectangles (one per group, top to
/// bottom), the rows of cells (top to bottom, for finding the visible ones by binary search) and
/// the total height of the grid content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridLayout {
    pub cells: Vec<Rect>,
    pub headers: Vec<Rect>,
    pub rows: Vec<GridRow>,
    pub height: f32,
}

/// The cell straight above (`down` false) or below cell `i`: in the adjacent row, the one whose
/// centre is nearest horizontally (justified rows don't line up in columns). None in the first /
/// last row or for an index outside the layout.
pub fn vertical_neighbor(lay: &GridLayout, i: usize, down: bool) -> Option<usize> {
    let row = lay.rows.iter().position(|r| (r.start..r.end).contains(&i))?;
    let next = if down { row.checked_add(1)? } else { row.checked_sub(1)? };
    let target = lay.rows.get(next)?;
    let x = lay.cells.get(i)?.center().x;
    (target.start..target.end).filter_map(|k| Some((k, (lay.cells.get(k)?.center().x - x).abs()))).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(k, _)| k)
}

/// Up / Down in the grids: how many photos `library.up` / `library.down` move to reach the cell
/// above / below the active one in the grid as last laid out; None when the grid isn't laid out
/// for the current photos (the filmstrip then steps one photo).
pub fn row_distance(app: &mut LccApp, down: bool) -> Option<u64> {
    if !matches!(app.ui.view, ViewMode::PhotoGrid | ViewMode::SquareGrid) {
        return None;
    }
    let active = app.session.selection.active?;
    let (generation, _) = app.session.visible_shared();
    let i = app.session.visible_position(active)?;
    let (key, lay) = app.caches.grid.layout.as_ref()?;
    if key.0.0 != generation {
        return None;
    }
    // at the top / bottom row there's no cell to go to: stay put rather than step sideways
    let j = vertical_neighbor(lay, i, down).unwrap_or(i);
    Some(j.abs_diff(i) as u64)
}

/// One row of cells: `cells[start..end]`, between `top` and `bottom`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridRow {
    pub top: f32,
    pub bottom: f32,
    pub start: usize,
    pub end: usize,
}

/// The grouping actually shown: `Auto` is days with large thumbnails, months with medium ones and
/// years when zoomed far out.
pub fn resolve_group(by: GroupBy, thumb_size: f32) -> GroupBy {
    match by {
        GroupBy::Auto if thumb_size >= 150.0 => GroupBy::Day,
        GroupBy::Auto if thumb_size >= 110.0 => GroupBy::Month,
        GroupBy::Auto => GroupBy::Year,
        other => other,
    }
}

/// Lay out the grid: `aspects` (width / height per photo; ignored for the square grid) in
/// `groups` (`(start, count)` runs; empty = one group without a header), `avail_w` wide, rows
/// about `target` high (square cells about `target` wide). Every group starts on a new row
/// below its header. Justified rows fill the width exactly except a group's last row, which keeps
/// the target height.
pub fn layout(aspects: &[f32], groups: &[(usize, usize)], avail_w: f32, target: f32, square: bool) -> GridLayout {
    let n = aspects.len();
    let whole = [(0, n)];
    let (groups, headed) = if groups.is_empty() { (&whole[..], false) } else { (groups, true) };
    let gap = if square { 1.0 } else { 6.0 };
    let mut out = GridLayout { cells: vec![Rect::NOTHING; n], headers: Vec::with_capacity(groups.len()), rows: Vec::new(), height: 0.0 };
    let mut y = 4.0f32;
    for &(start, count) in groups {
        let end = (start + count).min(n);
        if headed {
            out.headers.push(Rect::from_min_size(pos2(0.0, y), vec2(avail_w + 8.0, HEADER_H)));
            y += HEADER_H;
        }
        if start >= end {
            continue;
        }
        if square {
            let cols = ((avail_w + gap) / (target + gap)).floor().max(1.0);
            let cw = (avail_w - gap * (cols - 1.0)) / cols;
            for i in start..end {
                let k = (i - start) as f32;
                let (c, r) = (k % cols, (k / cols).floor());
                out.cells[i] = Rect::from_min_size(pos2(4.0 + c * (cw + gap), y + r * (cw + gap)), vec2(cw, cw));
            }
            let per_row = (cols as usize).max(1);
            for (r, row_start) in (start..end).step_by(per_row).enumerate() {
                let top = y + r as f32 * (cw + gap);
                out.rows.push(GridRow { top, bottom: top + cw, start: row_start, end: (row_start + per_row).min(end) });
            }
            let rows = ((end - start) as f32 / cols).ceil();
            y += rows * (cw + gap);
        } else {
            // justified rows: accumulate aspect ratios until the row is full
            let mut i = start;
            while i < end {
                let mut sum = 0.0;
                let mut j = i;
                while j < end {
                    sum += aspects[j].max(0.05);
                    let h = (avail_w - gap * (j - i) as f32) / sum;
                    j += 1;
                    if h <= target {
                        break;
                    }
                }
                let row_h = (avail_w - gap * (j - i - 1) as f32) / sum;
                let h = if j < end || row_h <= target { row_h } else { target };
                let mut x = 4.0;
                for k in i..j {
                    let w = aspects[k].max(0.05) * h;
                    out.cells[k] = Rect::from_min_size(pos2(x, y), vec2(w, h));
                    x += w + gap;
                }
                out.rows.push(GridRow { top: y, bottom: y + h, start: i, end: j });
                y += h + gap;
                i = j;
            }
        }
        if headed {
            y += 8.0;
        }
    }
    out.height = y;
    out
}

/// The ring round a selected photo, drawn outside `photo`. The active one is 3 pt in
/// [`Tokens::ring_active`] with a 1 pt line in [`Tokens::ring_inner`] over its inner edge, against
/// the photo (so the ring reads on a white photo and on a black one). The others are a single 1.5 pt line in
/// [`Tokens::ring_selected`]; the two are told apart by weight, not by lightness alone.
pub(crate) fn selection_ring(p: &egui::Painter, t: &Tokens, photo: Rect, state: lcc_engine::SelectionState) {
    use lcc_engine::SelectionState;
    match state {
        SelectionState::Active => {
            // both on the same rect, so no seam opens between them at a fractional scale
            p.rect_stroke(photo, 0.0, Stroke::new(3.0, t.ring_active), StrokeKind::Outside);
            p.rect_stroke(photo, 0.0, Stroke::new(1.0, t.ring_inner), StrokeKind::Outside);
        }
        SelectionState::Selected => {
            p.rect_stroke(photo, 0.0, Stroke::new(1.5, t.ring_selected), StrokeKind::Outside);
        }
        SelectionState::NotSelected => {}
    }
}

/// A date header: "Wednesday, 30 September 2026 · 12 photos". Clicking it selects the group
/// (Cmd/Shift: adds to the selection).
fn group_header(app: &mut LccApp, ui: &mut egui::Ui, run: &DateRun, ids: &[PhotoId], r: Rect, pinned: bool) {
    let t = Tokens::get(ui.ctx());
    let resp = ui.interact(r, egui::Id::new(("group-header", run.start, pinned)), Sense::click());
    crate::widgets::announce(&resp, if run.key.is_empty() { crate::i18n::tr("Unknown date").to_string() } else { run.key.clone() });
    if !pinned {
        register(ui.ctx(), format!("group:{}", if run.key.is_empty() { "unknown" } else { &run.key }), r);
    }
    let p = ui.painter();
    if pinned {
        p.rect_filled(r, 0.0, t.canvas);
        p.hline(r.x_range(), r.bottom(), Stroke::new(1.0, t.divider));
    }
    let label = match publish_section_label(&run.key) {
        Some(l) => crate::i18n::tr(l).to_string(),
        None => crate::i18n::date_group_label(&run.key, false),
    };
    let g = p.layout_no_wrap(label, t.semibold(15.0), if resp.hovered() { t.text } else { t.text_label });
    let x = r.left() + 8.0;
    let gw = g.size().x;
    p.galley(pos2(x, r.center().y - g.size().y / 2.0), g, t.text);
    let n = run.count;
    p.text(
        pos2(x + gw + 10.0, r.center().y),
        Align2::LEFT_CENTER,
        crate::i18n::tr_format!("· {n} photo{}", if n == 1 { "" } else { "s" }, n = n),
        t.font(12.5),
        t.text_dim,
    );
    if resp.clicked() {
        let group: Vec<u64> = ids[run.start..(run.start + run.count).min(ids.len())].iter().map(|p| p.0).collect();
        let m = ui.input(|i| i.modifiers);
        let mode = if m.command || m.shift { "add" } else { "replace" };
        let _ = app.run("library.select", json!({"ids": group, "mode": mode}));
    }
}

/// Request a grid/filmstrip thumbnail at `priority`. An unedited raw without a thumbnail texture
/// gets its embedded camera preview, and the render of the raw behind it follows in the background
/// — except where that render would only redraw what the embedded JPEG already shows (see
/// [`lcc_engine::media::Session::embedded_thumb_is_final`]), which for a folder of raws on a
/// network share is the difference between reading every file whole and reading a preview each.
pub fn request_thumb(app: &mut LccApp, id: PhotoId, size: usize, priority: u32) {
    // A slider drag changes the photo every frame: its thumbnail follows once the drag ends, so its
    // renders (a raw decode the first time) don't compete with the loupe's drafts.
    if app.session.interaction.as_ref().is_some_and(|i| i.photo == id) && app.renderer.thumb(id).is_some() {
        return;
    }
    // never less than the photo has or is getting: a smaller ask (the filmstrip beside a larger view) moving the
    // job up would otherwise swap it for a smaller one, and the larger view swap it back
    let size = app.session.catalog.photo(id).and_then(|p| app.renderer.thumb_bucket_kept(p)).map_or(size, |kept| kept.max(size));
    let bucket = lcc_engine::media::thumb_bucket(size);
    if let Some(photo) = app.session.catalog.photo(id)
        && (app.renderer.thumb_current(photo, bucket, priority) || app.renderer.thumb_covers(photo, bucket))
    {
        return;
    }
    // The camera's embedded preview is this photo's thumbnail (unedited raw): once its quick render
    // has arrived there is nothing left to ask for. Without this the grid would rebuild the render
    // job — and its cache key — on every frame, which is what the embedded-thumbnail record settles.
    if let Some(photo) = app.session.catalog.photo(id)
        && (app.renderer.thumb_quick_is_final(photo, bucket, priority) || app.renderer.thumb_quick_pending(photo, bucket, priority))
    {
        return;
    }
    let Some(job) = app.session.thumb_job(id, size) else { return };
    #[cfg(test)]
    {
        app.renderer.thumb_jobs_built += 1;
    }
    let quick = if app.renderer.textures.contains_key(&Slot::Thumb(id)) { None } else { app.session.quick_thumb_job(&job) };
    // A cached render is a stand-in for whatever comes next; an embedded camera preview of an
    // unedited raw is the picture itself, so no sensor decode is scheduled behind it.
    let embedded_is_final =
        quick.is_some() && app.session.catalog.photo(id).is_some_and(|p| skips_the_real_render(app.session.embedded_thumb_is_final(p), true));
    if let Some(photo) = app.session.catalog.photo(id) {
        if embedded_is_final {
            app.renderer.remember_embedded_thumb(photo, bucket, job.key);
        } else {
            app.renderer.remember_thumb(photo, bucket, job.key, quick.is_some());
        }
    }
    match quick {
        Some(q) => {
            app.renderer.request_quick(Slot::ThumbQuick(id), q, priority + 1);
            if !embedded_is_final && !app.renderer.is_pending(Slot::ThumbQuick(id)) {
                app.renderer.request(Slot::Thumb(id), job, BACKGROUND_THUMB_PRIORITY);
            }
        }
        None => app.renderer.request(Slot::Thumb(id), job, priority),
    }
}

/// Whether a photo's grid thumbnail is already the picture it should show, so the render of the raw
/// behind it is skipped. `embedded_is_final` is the engine's answer
/// ([`lcc_engine::media::Session::embedded_thumb_is_final`]: the photo is an unedited raw
/// whose camera JPEG the viewer is showing) and `from_embedded` says the stand-in actually came from
/// the embedded preview rather than a cached render of the file.
///
/// Skipping matters where the file is expensive to read: decoding a 56 MB Fuji RAF to draw a 512 px
/// thumbnail — which the 4 416 × 2 944 embedded JPEG already covers — cost 45 s a photo over a
/// 1.2 MB/s share, and the queue it built delayed the render the user was waiting for.
pub(crate) fn skips_the_real_render(embedded_is_final: bool, from_embedded: bool) -> bool {
    embedded_is_final && from_embedded
}

/// Rendered thumbnails replacing embedded previews: after everything on screen.
pub const BACKGROUND_THUMB_PRIORITY: u32 = 3;

#[allow(clippy::too_many_arguments)]
/// A quiet cue on a thumbnail's placeholder while the picture is still on its way (an unreadable one
/// says "!" instead): three small dots in the middle, in the icon color (3:1 on every theme's placeholder). It stands still, so
/// Reduce motion has nothing to stop. It is left off tiles too small to hold it: judged by the
/// `tile` (the placeholder's cell, smaller than 32 pt either way) and by the `rect` (the
/// placeholder, which must have room for the dots). Registered as `{prefix}{id}` (the grid's
/// `badge:loading:{id}`, the filmstrip's `film:badge:loading:{id}`).
pub(crate) fn paint_loading_cue(p: &egui::Painter, t: &Tokens, tile: Rect, rect: Rect, prefix: &str, id: u64) {
    const RADIUS: f32 = 1.75;
    const GAP: f32 = 7.0;
    const CUE_W: f32 = 2.0 * (GAP + RADIUS);
    const CUE_H: f32 = 2.0 * RADIUS;
    if tile.width().min(tile.height()) < 32.0 || rect.width() < CUE_W || rect.height() < CUE_H {
        return;
    }
    let middle = rect.center();
    for dx in [-GAP, 0.0, GAP] {
        p.circle_filled(middle + vec2(dx, 0.0), RADIUS, t.icon);
    }
    register(p.ctx(), format!("{prefix}{id}"), Rect::from_center_size(middle, vec2(CUE_W, CUE_H)));
}

fn cell(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId, r: Rect, square: bool, onscreen: bool, ppp: f32, index: usize) {
    let t = Tokens::get(ui.ctx());
    let Some(photo) = app.session.catalog.photo(id).cloned() else { return };
    let resp = ui.interact(r, egui::Id::new(("cell", id.0)), Sense::click_and_drag());
    if resp.hovered() {
        // the secondary window's Live loupe follows the pointer
        app.ui.hovered_photo = Some(id.0);
    }
    register(ui.ctx(), format!("thumb:{}", id.0), r);
    let state = app.session.selection.state_of(id);
    let selected = state != lcc_engine::SelectionState::NotSelected;
    // screen readers: the file, then rating / flag / label
    let mut spoken = photo.file_name.clone();
    if photo.rating > 0 {
        spoken.push_str(&crate::i18n::tr_format!(", {} star{}", photo.rating, if photo.rating == 1 { "" } else { "s" }));
    }
    match photo.flag {
        lcc_catalog::Flag::Pick => spoken.push_str(crate::i18n::tr(", picked")),
        lcc_catalog::Flag::Reject => spoken.push_str(crate::i18n::tr(", rejected")),
        lcc_catalog::Flag::None => {}
    }
    if let Some(l) = photo.label {
        spoken.push_str(&crate::i18n::tr_format!(", {} label", crate::i18n::color_label(&app.session.catalog, l)));
    }
    spoken.push_str(&crate::video_ui::spoken_suffix(&photo));
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &spoken));
    let p = ui.painter();
    let img_rect = if square {
        let base = if selected { t.cell_selected } else { t.cell };
        p.rect_filled(r, 0.0, crate::theme::label_background(base, photo.label, selected));
        super::cells::image_rect(app, r)
    } else {
        r
    };
    // thumbnail
    let size = thumb_px(img_rect.width().max(img_rect.height()), ppp);
    request_thumb(app, id, size, if onscreen { 10 } else { 5 });
    if let Some(tex) = app.renderer.thumb(id) {
        let [tw, th] = tex.size;
        // Preserve the decoded preview's ratio when it differs from the catalog dimensions.
        let fit = super::detail::fit_texture_rect(img_rect, [tw, th]);
        p.image(tex.tex.id(), fit, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        selection_ring(p, &t, fit, state);
    } else {
        let ph = img_rect.shrink(if square { 20.0 } else { 0.0 });
        p.rect_filled(ph, 0.0, t.skeleton);
        // not drawn yet (or unreadable): the selection is still visible
        selection_ring(p, &t, ph, state);
        if app.renderer.thumb_failure(id).is_some() {
            // unreadable / missing file
            p.text(ph.center(), Align2::CENTER_CENTER, "!", t.semibold(18.0), t.text);
        } else {
            paint_loading_cue(p, &t, img_rect, ph, "badge:loading:", id.0);
        }
    }
    crate::video_ui::grid_badge(ui, img_rect, &photo);
    // labels and badges
    if square && app.ui.lib.cell_style == crate::libtools::CellStyle::Expanded {
        super::cells::expanded_header(app, ui, &photo, r, index);
    } else if square && app.ui.show_filenames {
        let m = &photo.meta;
        let name = match app.ui.grid_info.as_str() {
            "exposure" => {
                let parts: Vec<String> = [
                    // whole seconds read "8 s"; the camera notation (1/250, 1.3″) already carries its unit
                    (!m.shutter.is_empty()).then(|| {
                        if m.shutter.contains('/') || m.shutter.ends_with(['s', '″']) { m.shutter.clone() } else { format!("{} s", m.shutter) }
                    }),
                    m.aperture.map(|a| format!("f/{a:.1}").replace(".0", "")),
                    m.iso.map(|i| format!("ISO {i}")),
                    m.focal_mm.map(|f| format!("{f:.0} mm")),
                ]
                .into_iter()
                .flatten()
                .collect();
                if parts.is_empty() { "—".to_string() } else { parts.join(" · ") }
            }
            "date" => photo.captured.as_deref().map(crate::i18n::display_time).unwrap_or_else(|| crate::i18n::tr("No date").into()),
            _ => photo.file_name.rsplit_once('.').map(|(n, _)| n.to_string()).unwrap_or(photo.file_name.clone()),
        };
        p.text(pos2(r.left() + 8.0, r.top() + 12.0), Align2::LEFT_CENTER, name, t.font(10.5), t.text_dim);
        let fmt = photo.format.clone();
        let g = p.layout_no_wrap(fmt, t.semibold(9.0), t.text_label);
        let br = Rect::from_min_size(pos2(r.right() - g.size().x - 14.0, r.top() + 5.0), g.size() + vec2(8.0, 3.0));
        p.rect_filled(br, 2.0, Color32::from_gray(26));
        p.galley(br.min + vec2(4.0, 1.5), g, t.text_label);
    }
    let expanded = square && app.ui.lib.cell_style == crate::libtools::CellStyle::Expanded;
    let show_badges = expanded
        || match app.ui.settings.grid_badges {
            crate::state::GridBadges::Auto => resp.hovered() || selected || photo.rating > 0 || photo.flag != Flag::None || photo.label.is_some(),
            crate::state::GridBadges::Always => true,
            crate::state::GridBadges::Never => false,
        };
    let badge_bar = Rect::from_min_max(pos2(img_rect.left(), img_rect.bottom() - 24.0), img_rect.right_bottom());
    if !square && photo.label.is_some() {
        // Justified photos fill the whole cell, so the label shows as a translucent tinted footer
        // over the photo's bottom edge (opaque, it would hide 24 px of every labelled photo).
        p.rect_filled(badge_bar, 0.0, crate::theme::label_background(t.cell, photo.label, selected).gamma_multiply(0.7));
    }
    if show_badges {
        let bar = badge_bar;
        if (resp.hovered() || selected) && (square || photo.label.is_none()) {
            p.rect_filled(bar, 0.0, Color32::from_black_alpha(120));
        }
        let mut x = bar.left() + 6.0;
        x += crate::rating::paint_rating(p, pos2(x, bar.center().y), photo.rating, crate::rating::RatingSize::Medium, t.star) + 4.0;
        match photo.flag {
            Flag::Pick => paint(p, Rect::from_min_size(pos2(x, bar.center().y - 7.0), vec2(14.0, 14.0)), Icon::FlagPickFilled, t.pick),
            Flag::Reject => paint(p, Rect::from_min_size(pos2(x, bar.center().y - 7.0), vec2(14.0, 14.0)), Icon::FlagReject, t.reject),
            Flag::None => {}
        }
        let mut right = bar.right();
        if photo.is_edited() {
            paint(p, Rect::from_min_size(pos2(right - 20.0, bar.center().y - 7.0), vec2(14.0, 14.0)), Icon::Sliders, t.text_label);
            right -= 20.0;
        }
        if let Some(l) = photo.label {
            p.circle(pos2(right - 12.0, bar.center().y), 5.0, label_color(l), Stroke::new(1.0, Color32::from_black_alpha(120)));
        }
    }
    if let Some(name) = &photo.copy_name {
        // virtual copy: a folded-corner tag at the top right
        let g = p.layout_no_wrap(name.clone(), t.semibold(10.5), Color32::WHITE);
        let tr = if square { img_rect.right_top() + vec2(-6.0, 6.0) } else { img_rect.right_top() + vec2(-5.0, 5.0) };
        let br = Rect::from_min_max(pos2(tr.x - g.size().x - 26.0, tr.y), pos2(tr.x, tr.y + 20.0));
        p.rect_filled(br, 10.0, Color32::from_black_alpha(165));
        let c = pos2(br.left() + 13.0, br.center().y);
        let s = 5.0;
        p.add(egui::Shape::convex_polygon(
            vec![c + vec2(-s, -s), c + vec2(s * 0.3, -s), c + vec2(s, -s * 0.3), c + vec2(s, s), c + vec2(-s, s)],
            Color32::TRANSPARENT,
            Stroke::new(1.2, Color32::WHITE),
        ));
        p.line_segment([c + vec2(s * 0.3, -s), c + vec2(s * 0.3, -s * 0.3)], Stroke::new(1.2, Color32::WHITE));
        p.line_segment([c + vec2(s * 0.3, -s * 0.3), c + vec2(s, -s * 0.3)], Stroke::new(1.2, Color32::WHITE));
        p.galley(pos2(br.left() + 22.0, br.center().y - g.size().y / 2.0), g, Color32::WHITE);
    }
    if let Some(why) = &photo.preview_only {
        // a raw shown from its embedded JPEG: a small amber "Preview" pill at the image's bottom
        // left, above the badge bar (always shown: it changes what editing does)
        let g = p.layout_no_wrap(crate::i18n::tr("Preview").into(), t.semibold(9.5), Color32::WHITE);
        let at = pos2(img_rect.left() + 6.0, img_rect.bottom() - 30.0 - 16.0);
        let br = Rect::from_min_size(at, vec2(g.size().x + 24.0, 16.0));
        p.rect_filled(br, 8.0, Color32::from_black_alpha(170));
        paint(p, Rect::from_center_size(pos2(br.left() + 9.0, br.center().y), vec2(12.0, 12.0)), Icon::Info, t.caution);
        p.galley(pos2(br.left() + 17.0, br.center().y - g.size().y / 2.0), g, Color32::WHITE);
        register(ui.ctx(), format!("badge:previewOnly:{}", id.0), br);
        ui.interact(br, egui::Id::new(("preview-only-badge", id.0)), Sense::hover())
            .on_hover_text(crate::i18n::tr_format!("Preview only — {}", crate::widgets::preview_only_explanation(why)));
    }
    #[cfg(not(target_arch = "wasm32"))]
    super::connections::grid_badge(app, ui, id, img_rect);
    if photo.flag == Flag::Reject {
        p.rect_filled(img_rect, 0.0, Color32::from_black_alpha(110));
    }
    // interaction
    // painting: a click or a drag sprays the painter's value instead of selecting
    if crate::libtools::paint_cell(app, ui, &resp, id) {
        return;
    }
    if resp.clicked() {
        let m = ui.input(|i| i.modifiers);
        let mode = if m.shift {
            "range"
        } else if m.command {
            "toggle"
        } else {
            "replace"
        };
        let _ = app.run("library.select", json!({"ids": [id.0], "mode": mode}));
    }
    if resp.double_clicked() {
        open_photo(app, id);
    }
    // drag the selection (this photo joins it, or replaces it when it wasn't selected)
    if resp.drag_started() {
        if !app.session.selection.contains(id) {
            let _ = app.run("library.select", json!({"ids": [id.0]}));
        }
        app.ui.dragging_photos = Some(app.session.selection.ids.iter().map(|p| p.0).collect());
    } else if let Some(dragged) = app.ui.dragging_photos.clone() {
        reorder_drop(app, ui, id, r, &dragged);
    }
    crate::widgets::context_menu(&resp, |ui| context_menu(app, ui, id));
}

/// A double-click on a grid photo: select it and show it in the loupe, or in Develop when the
/// user chose that in Preferences → Interface (issue #144).
pub fn open_photo(app: &mut LccApp, id: PhotoId) {
    let _ = app.run("library.select", json!({"ids": [id.0]}));
    let cmd = if app.ui.settings.double_click_develop { "module.develop" } else { "view.detail" };
    let _ = app.run(cmd, json!({}));
}

/// The regular album the library shows: the one whose photos can be put in Custom Order.
pub fn custom_order_album(app: &LccApp) -> Option<lcc_catalog::AlbumId> {
    match app.session.source {
        lcc_engine::LibrarySource::Album(a) if app.session.catalog.has_custom_order(a) => Some(a),
        _ => None,
    }
}

/// Photos dragged over a thumbnail of a regular album: a line on the side they would land
/// (before this photo, or after it), and on release they move there in Custom Order (switching the
/// sort to it, as Lightroom Classic does).
pub(crate) fn reorder_drop(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId, r: Rect, dragged: &[u64]) {
    let Some(album) = custom_order_album(app).filter(|_| !dragged.contains(&id.0)) else { return };
    let (pos, released) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.any_released()));
    let Some(pos) = pos.filter(|p| r.contains(*p) && ui.clip_rect().contains(*p)) else { return };
    let t = Tokens::get(ui.ctx());
    let left = pos.x < r.center().x;
    let x = if left { r.left() + 1.5 } else { r.right() - 1.5 };
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("reorder-drop"))).with_clip_rect(ui.clip_rect());
    painter.line_segment([egui::pos2(x, r.top()), egui::pos2(x, r.bottom())], Stroke::new(3.0, t.accent));
    if !released {
        return;
    }
    let before = if left {
        Some(id)
    } else {
        let vis = app.session.visible_cloned();
        vis.iter().position(|v| *v == id).and_then(|i| vis.get(i + 1).copied())
    };
    app.ui.dragging_photos = None;
    if let Err(e) = app.run("album.orderPhotos", json!({"album": album.0, "ids": dragged, "before": before.map(|b| b.0)})) {
        app.toast(ui.ctx(), e);
    }
}

/// The header of a Local folder view: the path as a breadcrumb (each part opens that folder),
/// Include subfolders, Add to My Photos.
fn folder_header(app: &mut LccApp, ui: &mut egui::Ui, b: &lcc_engine::Browse, ids: &[PhotoId], local_n: usize, cnt: &str) -> Rect {
    let t = Tokens::get(ui.ctx());
    // The path/count and actions need independent rows. The frame reserves the actual height,
    // including any wrapped actions, so the grid below never paints under the header.
    let header = egui::Frame::NONE.fill(t.canvas).inner_margin(egui::Margin::symmetric(16, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let count_w =
                ui.painter().layout_no_wrap(cnt.to_owned(), t.font(12.5), t.text_dim).size().x.ceil().min((ui.available_width() * 0.45).floor());
            let path_w = (ui.available_width() - count_w - 8.0).max(0.0).floor();
            ui.allocate_ui_with_layout(vec2(path_w, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                folder_breadcrumbs(app, ui, &b.path);
            });
            let count = ui.add_sized(vec2(count_w, 24.0), egui::Label::new(egui::RichText::new(cnt).size(12.5).color(t.text_dim)).truncate());
            register(ui.ctx(), "grid:count", count.rect);
            count.on_hover_text(cnt);
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            let mut sub = b.subfolders;
            let c = ui.checkbox(&mut sub, crate::i18n::tr("Include subfolders"));
            register(ui.ctx(), "check:includeSubfolders", c.rect);
            if c.changed() {
                let _ = app.run("library.browse", json!({"path": b.path, "subfolders": sub}));
            }
            // Counted once per view (cached); the ids are only gathered on a click.
            if local_n > 0 {
                let label = crate::i18n::tr_format!("Add {local_n} to My Photos", local_n = local_n);
                let button =
                    ui.add(egui::Button::new(egui::RichText::new(label).font(t.semibold(11.5)).color(t.text)).wrap().min_size(vec2(37.0, 24.0)));
                register(ui.ctx(), "button:addToLibrary", button.rect);
                if button.clicked() {
                    let local: Vec<u64> = ids.iter().filter(|id| app.session.catalog.photo(**id).is_some_and(|p| p.local)).map(|id| id.0).collect();
                    let n = local.len();
                    match app.run("photo.addToLibrary", json!({"ids": local})) {
                        Ok(_) => app.toast(ui.ctx(), crate::i18n::tr_format!("Added {n} photo{} to My Photos", if n == 1 { "" } else { "s" }, n = n)),
                        Err(e) => app.toast(ui.ctx(), e),
                    }
                }
            }
        });
    });
    register(ui.ctx(), "grid:folderHeader", header.response.rect);
    header.response.rect
}

/// The folders a breadcrumb shows for `path`: each name with the path that opens it, spelled as
/// in `path` (its separators, a leading `/` or a UNC `\\server\share`; issue #538). A bare drive
/// (`C:`) gets its separator back: on its own it means the current folder on that drive.
pub(crate) fn crumbs(path: &str) -> Vec<(&str, String)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in path.char_indices().chain(std::iter::once((path.len(), '/'))) {
        if c != '/' && c != '\\' {
            continue;
        }
        if let Some(part) = path.get(start..i).filter(|p| !p.is_empty()) {
            let mut to = path.get(..i).unwrap_or(path).to_string();
            if out.is_empty() && part.ends_with(':') && to == part {
                to.push(if path.contains('\\') { '\\' } else { '/' });
            }
            out.push((part, to));
        }
        start = i + c.len_utf8();
    }
    out
}

fn folder_breadcrumbs(app: &mut LccApp, ui: &mut egui::Ui, path: &str) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing.x = 4.0;
    let crumbs = crumbs(path);
    let parts: Vec<&str> = crumbs.iter().map(|(name, _)| *name).collect();
    // With all sidebars open even four ellipses and their separators can exceed the row.
    // Prefer the current folder, showing more ancestors as space permits.
    let visible = ((ui.available_width() / 80.0) as usize).clamp(1, 4);
    let skip = parts.len().saturating_sub(visible);
    let separator = |ui: &mut egui::Ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 24.0), Sense::hover());
        paint(ui.painter(), Rect::from_center_size(r.center(), vec2(16.0, 16.0)), Icon::ChevronRight, t.text_dim);
    };
    if skip > 0 {
        ui.label(egui::RichText::new("…").color(t.text_dim)).on_hover_text(path);
        separator(ui);
    }
    let separator_w = 16.0 + 2.0 * ui.spacing().item_spacing.x;
    for (i, part) in parts.iter().enumerate().skip(skip) {
        let remaining = parts.len() - i;
        let last = remaining == 1;
        // Share the bounded row among the remaining crumbs, leaving room for every separator.
        let width = ((ui.available_width() - separator_w * (remaining - 1) as f32) / remaining as f32).max(0.0).floor();
        let text = egui::RichText::new(*part).size(13.0).color(if last { t.text } else { t.text_label });
        let r = ui.add_sized(vec2(width, 24.0), egui::Label::new(if last { text.strong() } else { text }).truncate().sense(Sense::click()));
        register(ui.ctx(), format!("crumb:{i}"), r.rect);
        if !last {
            if r.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if r.clicked()
                && let Some((_, to)) = crumbs.get(i)
            {
                let _ = app.run("library.browse", json!({"path": to}));
            }
            separator(ui);
        }
        r.on_hover_text(path);
    }
}

/// While photos are dragged: a badge at the pointer; the drag ends when the button is up
/// (drop targets act on the release frame, before this runs).
/// What the label following dragged photos says this frame instead of how many there are: where a
/// drop would put them (set by the drop target under the pointer, read by [`drag_feedback`]).
pub fn drag_hint(ctx: &egui::Context, text: String) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("drag-photos-hint"), (frame, text)));
}

pub fn drag_feedback(app: &mut LccApp, ctx: &egui::Context) {
    let Some(ids) = &app.ui.dragging_photos else { return };
    let (released, down, pos) = ctx.input(|i| (i.pointer.any_released(), i.pointer.any_down(), i.pointer.latest_pos()));
    if released || !down {
        app.ui.dragging_photos = None;
        return;
    }
    let Some(pos) = pos else { return };
    let t = Tokens::get(ctx);
    let n = ids.len();
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    // a drop target under the pointer says where they would go (only this frame's)
    let frame = ctx.cumulative_frame_nr();
    let hint = ctx.data(|d| d.get_temp::<(u64, String)>(egui::Id::new("drag-photos-hint"))).filter(|(f, _)| *f == frame).map(|(_, text)| text);
    let text = hint.unwrap_or_else(|| crate::i18n::tr_format!("{n} photo{}", if n == 1 { "" } else { "s" }, n = n));
    egui::Area::new(egui::Id::new("drag-photos")).order(egui::Order::Tooltip).interactable(false).fixed_pos(pos + vec2(14.0, 10.0)).show(ctx, |ui| {
        egui::Frame::NONE.fill(t.accent).corner_radius(10.0).inner_margin(egui::Margin::symmetric(9, 3)).show(ui, |ui| {
            let label = ui.add(egui::Label::new(egui::RichText::new(text).color(Color32::WHITE).font(t.semibold(12.0))).extend());
            crate::widgets::register(ui.ctx(), "dragPhotos", label.rect);
        });
    });
}

pub use super::filterbar::label_color;

/// Albums hold library photos (issue #684): true when `ids` are photos and all of them are
/// browsed (Local) ones, which album views never show.
pub fn local_only(app: &LccApp, ids: &[PhotoId]) -> bool {
    !ids.is_empty() && ids.iter().all(|id| app.session.catalog.photo(*id).is_some_and(|p| p.local))
}

/// Why an album action is off for Local photos.
pub fn albums_hint() -> String {
    crate::i18n::tr("Albums hold library photos: Add to My Photos first").to_string()
}

/// Tell the user when an album command skipped Local photos of the selection (`skipped` in its
/// result).
pub fn toast_skipped(app: &mut LccApp, ctx: &egui::Context, r: &serde_json::Value) {
    if let Some(n) = r.get("skipped").and_then(serde_json::Value::as_u64).filter(|n| *n > 0) {
        app.toast(
            ctx,
            crate::i18n::tr_format!(
                "{n} Local photo{} skipped: albums hold library photos (Add to My Photos first)",
                if n == 1 { "" } else { "s" },
                n = n
            ),
        );
    }
}

/// "Set Color Label" items for the active photo, shared by context menus.
pub fn label_menu(app: &mut LccApp, ui: &mut egui::Ui) {
    let current = app.session.active().and_then(|id| app.session.catalog.photo(id)).and_then(|p| p.label);
    if let Some(l) = label_items(app, ui, current, "photoLabel") {
        let mut params = json!({"label": label_param(l)});
        crate::panels::compare::target_active(app, &mut params);
        let _ = app.run("photo.label", params);
        ui.close();
    }
}

/// The colour label items of a menu (coloured dot + the label's name, then None and Edit Label
/// Names…), `current` checked; each colour is widget `<widget>:<colour>` (`<widget>:none`) for
/// the control channel. Returns the choice (`Some(None)`: None).
pub fn label_items(app: &mut LccApp, ui: &mut egui::Ui, current: Option<ColorLabel>, widget: &str) -> Option<Option<ColorLabel>> {
    let mut chosen = None;
    for l in ColorLabel::ALL {
        let name = crate::i18n::color_label(&app.session.catalog, l);
        let resp = ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 5.0, label_color(l));
            ui.selectable_label(current == Some(l), name)
        });
        register(ui.ctx(), format!("{widget}:{}", label_param(Some(l))), resp.inner.rect);
        if resp.inner.clicked() {
            chosen = Some(Some(l));
        }
    }
    let none = ui.selectable_label(current.is_none(), crate::i18n::tr("None"));
    register(ui.ctx(), format!("{widget}:none"), none.rect);
    if none.clicked() {
        chosen = Some(None);
    }
    ui.separator();
    if ui.button(crate::i18n::tr("Edit Label Names…")).clicked() {
        let _ = app.run("dialog.labelNames", json!({}));
    }
    chosen
}

/// A label as the `label` parameter of `photo.label` / `folder.label` names it.
pub fn label_param(l: Option<ColorLabel>) -> String {
    l.map_or_else(|| "none".to_string(), |l| format!("{l:?}").to_lowercase())
}

/// Stack badge at the cell's top-left: the photo count on a collapsed stack's top, `i/n` on the
/// members of an expanded stack. Clicking or double-clicking it expands/collapses the stack.
pub fn stack_badge(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId, sid: lcc_catalog::StackId, pos: usize, r: Rect, square: bool) {
    let t = Tokens::get(ui.ctx());
    let Some(st) = app.session.catalog.stack(sid) else { return };
    let n = st.photos.len();
    let collapsed = st.collapsed;
    let text = if collapsed { n.to_string() } else { format!("{}/{n}", pos + 1) };
    let p = ui.painter();
    let g = p.layout_no_wrap(text, t.semibold(11.0), Color32::WHITE);
    let origin = if square { r.min + vec2(10.0, 26.0) } else { r.min + vec2(5.0, 5.0) };
    let br = Rect::from_min_size(origin, vec2(g.size().x + 30.0, 20.0));
    let resp = ui.interact(br, egui::Id::new(("stack-badge", id.0)), Sense::click()).on_hover_text(if collapsed {
        "Stack — click to expand (S)"
    } else {
        "Stack — click to collapse (S)"
    });
    crate::widgets::announce(&resp, crate::i18n::tr(if collapsed { "Expand Stack" } else { "Collapse Stack" }));
    register(ui.ctx(), format!("stack:{}", id.0), br);
    let fill = if resp.hovered() { Color32::from_black_alpha(220) } else { Color32::from_black_alpha(165) };
    p.rect_filled(br, 10.0, fill);
    if pos == 0 && !collapsed {
        p.rect_stroke(br, 10.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    paint(p, Rect::from_min_size(br.min + vec2(6.0, 3.0), vec2(14.0, 14.0)), Icon::Stack, Color32::WHITE);
    p.galley(pos2(br.min.x + 24.0, br.center().y - g.size().y / 2.0), g, Color32::WHITE);
    // a double-click (Lightroom's way) toggles once: its second click would undo the first
    if resp.clicked() && !resp.double_clicked() {
        toggle_stack(app, ui.ctx(), id);
    }
}

/// A click on a stack badge (grid or filmstrip): expand / collapse that photo's stack, selected or
/// not. The selection stays as it is (Compare keeps its pair).
pub fn toggle_stack(app: &mut LccApp, ctx: &egui::Context, id: PhotoId) {
    if let Err(e) = app.run("stack.toggle", json!({"ids": [id.0]})) {
        app.toast(ctx, e);
    }
}

pub fn context_menu(app: &mut LccApp, ui: &mut egui::Ui, id: PhotoId) {
    if !app.session.selection.contains(id) {
        let _ = app.run("library.select", json!({"ids": [id.0]}));
    }
    if ui.button(crate::i18n::tr("Open in Detail")).clicked() {
        let _ = app.run("view.detail", json!({}));
    }
    // Photo ▸ Edit In: a copy with this photo's edits, opened in PhotoCraft or another editor
    // (desktop only: the web can't start programs)
    ui.add_enabled_ui(crate::menus::ui_enabled(app, "photo.editInPhotoCraft"), |ui| {
        crate::widgets::menu_button(ui, crate::i18n::tr("Edit In"), |ui| {
            if ui.button(crate::i18n::tr("Edit in PhotoCraft")).clicked() {
                let _ = app.run("photo.editInPhotoCraft", json!({}));
            }
            if ui.button(crate::i18n::tr("Edit in Other Editor")).clicked() {
                let _ = app.run("photo.editInExternal", json!({}));
            }
            // the raw itself, unrendered: for a raw developer set as the other editor
            if ui
                .add_enabled(
                    crate::menus::ui_enabled(app, "photo.openOriginalInExternal"),
                    egui::Button::new(crate::i18n::tr("Open Original in Other Editor (RAW, no copy)")),
                )
                .clicked()
            {
                let _ = app.run("photo.openOriginalInExternal", json!({}));
            }
        });
    });
    ui.separator();
    crate::widgets::menu_button(ui, crate::i18n::tr("Set Flag"), |ui| {
        for (l, f) in [("Pick", "pick"), ("Reject", "reject"), ("Unflagged", "none")] {
            if ui.button(crate::i18n::tr(l)).clicked() {
                let _ = app.run("photo.flag", json!({"flag": f}));
            }
        }
    });
    crate::widgets::menu_button(ui, crate::i18n::tr("Set Rating"), |ui| {
        for r in 0..=5 {
            if ui.button(if r == 0 { crate::i18n::tr("No Stars").to_string() } else { "★".repeat(r) }).clicked() {
                let _ = app.run("photo.rate", json!({"rating": r}));
            }
        }
    });
    crate::widgets::menu_button(ui, crate::i18n::tr("Set Color Label"), |ui| label_menu(app, ui));
    // the keyword shortcut (Set Keyword Shortcut…), given to the selected photos
    let keywords = app.session.keyword_shortcut.clone();
    if !keywords.is_empty() {
        let names = keywords.iter().map(|k| k.replace('|', " › ")).collect::<Vec<_>>().join(", ");
        let add = ui.button(crate::i18n::tr_format!("Add Keyword “{keywords}”", keywords = names));
        register(ui.ctx(), "button:contextAddKeywordShortcut", add.rect);
        if add.clicked() {
            let _ = app.run("photo.setMeta", json!({"addKeywords": keywords}));
            ui.close();
        }
    }
    ui.separator();
    quick_collection_item(app, ui);
    if local_only(app, &app.session.selected()) {
        // albums hold library photos (issue #684): nothing here to add until they join My Photos
        let add = ui.add_enabled(false, egui::Button::new(crate::i18n::tr("Add to Collection"))).on_disabled_hover_text(albums_hint());
        register(ui.ctx(), "button:contextAddToAlbum", add.rect);
    } else {
        let add = crate::widgets::menu_button(ui, crate::i18n::tr("Add to Collection"), |ui| {
            let albums: Vec<_> = app.session.catalog.albums().filter(|a| !a.folder && !a.is_smart()).map(|a| (a.id.0, a.name.clone())).collect();
            let any = !albums.is_empty();
            for (aid, name) in albums {
                let b = ui.button(name);
                register(ui.ctx(), format!("button:contextAlbum:{aid}"), b.rect);
                if b.clicked() {
                    match app.run("album.addPhotos", json!({"id": aid})) {
                        Ok(r) => toast_skipped(app, ui.ctx(), &r),
                        Err(e) => app.toast(ui.ctx(), e),
                    }
                }
            }
            if any {
                ui.separator();
            }
            // as in the Photo menu: a new album holding the selection, also when there is none yet
            // to pick (issue #676: the submenu came up empty)
            let new = ui.button(crate::i18n::tr("New Collection…"));
            register(ui.ctx(), "button:contextNewAlbum", new.rect);
            if new.clicked() {
                let _ = app.run("dialog.newAlbum", json!({}));
            }
            // in a (non-smart) album: take the selection out of it, or make this photo its cover
            if let lcc_engine::LibrarySource::Album(aid) = app.session.source
                && app.session.catalog.album(aid).is_some_and(|a| !a.folder && !a.is_smart())
            {
                if ui.button(crate::i18n::tr("Remove from Collection")).clicked() {
                    let _ = app.run("album.removePhotos", json!({"id": aid.0}));
                }
                if ui.button(crate::i18n::tr("Set as Album Cover")).clicked() {
                    let _ = app.run("album.setCover", json!({"id": aid.0, "photo": id.0}));
                }
            }
        });
        register(ui.ctx(), "button:contextAddToAlbum", add.response.rect);
    }
    crate::widgets::menu_button(ui, crate::i18n::tr("Virtual Copy & Version"), |ui| {
        if ui.button(crate::i18n::tr("Create Virtual Copy")).clicked() {
            let _ = app.run("photo.virtualCopy", json!({}));
        }
        if ui.button(crate::i18n::tr("Create Version")).clicked() {
            let _ = app.run("version.create", json!({}));
        }
    });
    crate::widgets::menu_button(ui, crate::i18n::tr("Stack"), |ui| {
        let stacked = app.session.catalog.stack_of(id).is_some();
        let several = app.session.selection.ids.len() > 1;
        for (label, cmd, on) in [
            ("Group into Stack", "stack.group", several),
            ("Ungroup Stack", "stack.ungroup", stacked),
            ("Remove from Stack", "stack.remove", stacked),
            ("Set as Top of Stack", "stack.setTop", stacked),
            ("Move Up in Stack", "stack.moveUp", stacked),
            ("Move Down in Stack", "stack.moveDown", stacked),
            ("Split Stack", "stack.split", stacked),
            ("Expand/Collapse Stack", "stack.toggle", stacked),
        ] {
            if ui.add_enabled(on, egui::Button::new(crate::i18n::tr(label))).clicked() {
                let _ = app.run(cmd, json!({}));
            }
        }
        ui.separator();
        if ui.button(crate::i18n::tr("Auto-Stack by Capture Time…")).clicked() {
            let _ = app.run("dialog.autoStack", json!({}));
        }
    });
    ui.separator();
    if ui.button(crate::i18n::tr("Copy Edit Settings")).clicked() {
        let _ = app.run("develop.copy", json!({}));
    }
    if ui.add_enabled(app.session.clipboard.is_some(), egui::Button::new(crate::i18n::tr("Paste Edit Settings"))).clicked() {
        let _ = app.run("develop.paste", json!({}));
    }
    crate::widgets::menu_button(ui, crate::i18n::tr("Develop Settings"), |ui| {
        if ui.add_enabled(app.session.clipboard.is_some(), egui::Button::new(crate::i18n::tr("Paste Selected Settings…"))).clicked() {
            let _ = app.run("dialog.pasteSettings", json!({}));
        }
        if ui.button(crate::i18n::tr("Reset Edits")).clicked() {
            let _ = app.run("develop.reset", json!({}));
        }
        let live = super::compare::live(app, id);
        let set_reference = ui
            .add_enabled(live, egui::Button::new(crate::i18n::tr("Set as Reference Photo")))
            .on_disabled_hover_text(crate::i18n::tr("Restore the photo to use it as the reference"));
        if set_reference.clicked()
            && let Err(e) = app.run("photo.setReference", json!({"id": id.0}))
        {
            app.toast(ui.ctx(), e);
        }
    });
    crate::widgets::menu_button(ui, crate::i18n::tr("Photo Merge"), |ui| {
        let n = app.session.selected().len();
        for (id, label) in [
            ("dialog.mergeHdr", "HDR…"),
            ("dialog.mergePanorama", "Panorama…"),
            ("dialog.mergeHdrPanorama", "HDR Panorama…"),
            ("dialog.mergeFocusStack", "Focus Stack…"),
        ] {
            if ui.add_enabled(n >= 2, egui::Button::new(crate::i18n::tr(label))).clicked() {
                let _ = app.run(id, json!({}));
            }
        }
    });
    ui.separator();
    crate::widgets::menu_button(ui, crate::i18n::tr("Rotate"), |ui| {
        if ui.button(crate::i18n::tr("Rotate Left")).clicked() {
            let _ = app.run("photo.rotateLeft", json!({}));
        }
        if ui.button(crate::i18n::tr("Rotate Right")).clicked() {
            let _ = app.run("photo.rotateRight", json!({}));
        }
    });
    crate::widgets::menu_button(ui, crate::i18n::tr("File"), |ui| {
        if ui.button(crate::i18n::tr("Rename…")).clicked() {
            let _ = app.run("dialog.rename", json!({}));
        }
        let can_pick = app.services.pick_folder.is_some() || app.services.picker.is_some();
        if can_pick && ui.button(crate::i18n::tr("Move to Folder…")).clicked() {
            // the folder dialog goes through the shared picker (off the UI thread where the host has
            // one: then photo.moveToFolder runs when it closes)
            let req = crate::pick::PickRequest::folder(crate::i18n::tr("Move to Folder…"));
            let picked =
                crate::pick::ask(app, "photo.moveToFolder", &json!({}), "folder", req, |s| s.pick_folder.as_mut().and_then(|f| f()).map(|x| vec![x]));
            if let crate::pick::Picked::Now(v) = picked
                && let Some(into) = v.into_iter().next()
            {
                match app.run("photo.moveToFolder", json!({"folder": into})) {
                    Ok(r) => app.toast(ui.ctx(), crate::i18n::tr_format!("Moved {} photo(s)", r["moved"])),
                    Err(e) => app.toast(ui.ctx(), e),
                }
            }
        }
        if ui.button(crate::i18n::tr("Reload from Disk")).clicked() {
            // the files are read in the background (activity row `reload`)
            if let Err(e) = app.run("photo.reload", json!({})) {
                app.toast_error(ui.ctx(), e);
            }
        }
        // the original moved or its drive is gone: point the photo at the file again
        // (a cached answer, checked off the UI thread)
        let missing = matches!(&app.session.catalog.photo(id).map(|p| p.source.clone()), Some(lcc_catalog::Source::File { path }) if app.session.media.availability.is_offline(path));
        if missing && ui.button(crate::i18n::tr("Locate Missing File…")).clicked() {
            let _ = app.run("photo.locate", json!({}));
        }
        if ui
            .add_enabled(crate::menus::ui_enabled(app, "app.showInFinder"), egui::Button::new(crate::i18n::tr(crate::menus::reveal_label())))
            .clicked()
        {
            let _ = app.run("app.showInFinder", json!({}));
        }
        // the photo's folder in the sidebar's Folders section, as Lightroom's Go to Folder in Library
        // (the photo right-clicked, which a multi-selection may leave other than the active one)
        if ui.add_enabled(crate::menus::show_in_folders_enabled(app, Some(id)), egui::Button::new(crate::i18n::tr("Show in Folders"))).clicked() {
            let _ = app.run("app.showInFolders", json!({"id": id.0}));
        }
    });
    if ui.button(crate::i18n::tr("Find Similar Photos")).clicked() {
        match app.run("library.findSimilar", json!({"id": id.0})) {
            Ok(r) => {
                let n = r["photos"].as_array().map_or(0, Vec::len);
                app.ui.view = crate::state::ViewMode::PhotoGrid;
                app.toast(
                    ui.ctx(),
                    crate::i18n::tr_format!("{n} similar photo{} · View ▸ Clear Filters to see all", if n == 1 { "" } else { "s" }, n = n),
                );
            }
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
    ui.separator();
    if ui.button(crate::i18n::tr("Export…")).clicked() {
        let _ = app.run("dialog.export", json!({}));
    }
    crate::widgets::menu_button(ui, crate::i18n::tr("Export with Preset"), |ui| {
        for (p, _) in app.session.all_export_presets() {
            if ui.button(&p.name).clicked() {
                match app.run("app.export", json!({"preset": p.name, "background": true})) {
                    Ok(r) if r.get("background").is_some() => {}
                    Ok(r) => {
                        let n = r["files"].as_array().map_or(0, Vec::len);
                        app.toast(ui.ctx(), crate::i18n::tr_format!("Exported {n} photo{}", if n == 1 { "" } else { "s" }, n = n));
                    }
                    Err(e) => app.toast(ui.ctx(), e),
                }
            }
        }
    });
    ui.separator();
    if crate::menubar::selection_deleted(app) {
        if ui.button(crate::i18n::tr("Restore")).clicked() {
            let _ = app.run("photo.restore", json!({}));
        }
        if ui.button(crate::i18n::tr("Delete Permanently")).clicked() {
            let _ = crate::menubar::run_item(app, "photo.deletePermanently", json!({}));
        }
    } else {
        let delete = ui.button(crate::i18n::tr("Delete Photo"));
        register(ui.ctx(), "button:contextDeletePhoto", delete.rect);
        if delete.clicked() {
            let _ = crate::menubar::run_item(app, "photo.delete", json!({}));
        }
    }
}

/// Add to / Remove from Quick Collection (B), as Lightroom Classic's photo menu has it: the target
/// album when one is set, labelled by whether the whole selection is already in it.
fn quick_collection_item(app: &mut LccApp, ui: &mut egui::Ui) {
    let s = &app.session;
    let target = s.target_album.filter(|a| s.catalog.album(*a).is_some_and(|al| !al.is_smart() && !al.folder));
    let quick = target.is_none();
    let album = target.or_else(|| s.catalog.quick_collection()).and_then(|a| s.catalog.album(a));
    let sel = s.selected();
    let all_in = !sel.is_empty() && album.is_some_and(|al| sel.iter().all(|t| al.photos.contains(t)));
    let text = match (all_in, quick) {
        (false, true) => "Add to Quick Collection",
        (true, true) => "Remove from Quick Collection",
        (false, false) => "Add to Target Collection",
        (true, false) => "Remove from Target Collection",
    };
    let mac = ui.ctx().os() == egui::os::OperatingSystem::Mac;
    let mut b = egui::Button::new(crate::i18n::tr(text));
    if let Some(sc) = crate::shortcuts::shortcut_of(&app.ui.settings.keymap, "album.toggleTarget") {
        b = b.shortcut_text(crate::menubar::shortcut_text(sc, mac));
    }
    let r = ui.add_enabled(crate::menus::ui_enabled(app, "album.toggleTarget"), b);
    register(ui.ctx(), "button:contextQuickCollection", r.rect);
    if r.clicked() {
        match app.run("album.toggleTarget", json!({})) {
            Ok(r) => toast_skipped(app, ui.ctx(), &r),
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_photo_keeps_its_place_through_a_run_of_small_resizes() {
        let ids: Vec<PhotoId> = (1..=600).map(PhotoId).collect();
        let aspects: Vec<f32> = (0..600).map(|i| [1.5, 0.67, 1.0, 1.78][i % 4]).collect();
        let view_h = 800.0;
        let mut w = 900.0;
        let lay = layout(&aspects, &[(0, 600)], w, 160.0, false);
        let active = ids.get(250).copied();
        let top0 = lay.cells.get(250).unwrap().top() - 300.0;
        let mut anchor = GridAnchor::follow(None, (0, 600, 0), &ids, &lay, Rect::from_min_size(pos2(0.0, top0), vec2(w, view_h)), active);
        let start_dy = anchor.as_ref().unwrap().dy;
        for step in 1..=20 {
            w *= 1.05;
            let lay = layout(&aspects, &[(0, 600)], w, 160.0, false);
            let id = (step, 600, 0);
            let top = anchor.as_ref().unwrap().reflow_offset(id, &ids, &lay).unwrap();
            let view = Rect::from_min_size(pos2(0.0, top), vec2(w, view_h));
            anchor = GridAnchor::follow(anchor, id, &ids, &lay, view, active);
            let a = anchor.as_ref().unwrap();
            assert_eq!(a.photo, PhotoId(251), "step {step}: the selected photo stays the anchor");
            let on_screen = lay.cells.get(250).unwrap().top() - top;
            assert!((on_screen - start_dy).abs() < 0.5, "step {step}: moved from {start_dy} to {on_screen}");
        }
    }

    #[test]
    fn a_filter_keeps_the_anchor_photo_in_place_and_survives_it_leaving() {
        let ids: Vec<PhotoId> = (1..=400).map(PhotoId).collect();
        let lay = layout(&[1.5; 400], &[(0, 400)], 1000.0, 160.0, false);
        let view = Rect::from_min_size(pos2(0.0, lay.cells.get(200).unwrap().top() - 100.0), vec2(1000.0, 600.0));
        let a = GridAnchor::follow(None, (0, 400, 0), &ids, &lay, view, None).unwrap();
        // every other photo filtered out
        let kept: Vec<PhotoId> = ids.iter().copied().filter(|p| p.0 % 2 == a.photo.0 % 2).collect();
        let lay2 = layout(&vec![1.5; kept.len()], &[(0, kept.len())], 1000.0, 160.0, false);
        let y = a.reflow_offset((1, kept.len(), 0), &kept, &lay2).unwrap();
        let i = kept.iter().position(|p| *p == a.photo).unwrap();
        assert!((lay2.cells.get(i).unwrap().top() - y - a.dy).abs() < 0.5);
        // anchor filtered out: no jump, and a new one is picked
        let none: Vec<PhotoId> = ids.iter().copied().filter(|p| *p != a.photo).collect();
        assert!(a.reflow_offset((2, none.len(), 0), &none, &lay).is_none());
        assert!(GridAnchor::follow(Some(a), (2, none.len(), 0), &none, &lay, view, None).is_some());
        assert!(GridAnchor::follow(None, (3, 0, 0), &[], &layout(&[], &[], 1000.0, 160.0, false), view, None).is_none());
    }

    #[test]
    fn the_same_photo_in_another_source_scrolls_into_view() {
        let id = PhotoId(3);
        let all = lcc_engine::LibrarySource::All;
        let album = lcc_engine::LibrarySource::Album(lcc_catalog::AlbumId(9));
        assert!(!follow_changed(Some(id), all, 4, Some(id), all, 5));
        assert!(follow_changed(Some(id), all, 4, Some(id), album, 5));
    }

    #[test]
    fn ungrouped_justified_rows_fill_the_width() {
        let aspects = [1.5f32, 1.5, 0.66, 1.0, 1.5, 1.5, 1.5];
        let l = layout(&aspects, &[], 600.0, 150.0, false);
        assert!(l.headers.is_empty());
        assert_eq!(l.cells.len(), aspects.len());
        // full rows end at the right edge; rows are at most the target height
        let first_row: Vec<&Rect> = l.cells.iter().filter(|c| (c.top() - l.cells[0].top()).abs() < 0.01).collect();
        assert!(first_row.len() >= 2);
        assert!((first_row.last().unwrap().right() - 604.0).abs() < 0.5, "{first_row:?}");
        assert!(first_row.iter().all(|c| c.height() <= 150.0 + 0.01));
        let last = l.cells.last().unwrap();
        assert!(last.right() <= 604.0 + 0.5);
        assert!((l.height - (last.bottom() + 6.0)).abs() < 0.01);
    }

    #[test]
    fn groups_start_new_rows_below_their_headers() {
        let aspects = [1.5f32; 7];
        let l = layout(&aspects, &[(0, 2), (2, 5)], 1000.0, 200.0, false);
        assert_eq!(l.headers.len(), 2);
        assert_eq!(l.headers[0].top(), 4.0);
        assert_eq!(l.headers[0].height(), HEADER_H);
        // group 1: two photos in a short row below its header (not stretched)
        assert_eq!(l.cells[0].top(), l.headers[0].bottom());
        assert!((l.cells[0].height() - 200.0).abs() < 0.01);
        assert_eq!(l.cells[1].top(), l.cells[0].top());
        // group 2 starts on a new row below its own header, after group 1's photos
        assert!(l.headers[1].top() >= l.cells[1].bottom());
        assert_eq!(l.cells[2].top(), l.headers[1].bottom());
        assert!((l.cells[2].left() - 4.0).abs() < 0.01, "a group's first photo starts the row");
        for c in &l.cells[2..] {
            assert!(c.top() >= l.headers[1].bottom());
        }
        // every photo has a cell; nothing overlaps
        for (i, a) in l.cells.iter().enumerate() {
            assert!(a.width() > 0.0);
            for b in &l.cells[i + 1..] {
                assert!(!a.shrink(0.5).intersects(b.shrink(0.5)), "{a:?} {b:?}");
            }
            for h in &l.headers {
                assert!(!a.shrink(0.5).intersects(*h));
            }
        }
    }

    /// The row index covers every cell once, in order and top to bottom, so the visible cells
    /// are found by binary search.
    #[test]
    fn rows_index_every_cell_top_to_bottom() {
        let aspects: Vec<f32> = (0..500).map(|i| [1.5f32, 0.66, 1.0, 1.78][i % 4]).collect();
        let groups = [(0, 7), (7, 200), (207, 1), (208, 292)];
        for square in [false, true] {
            let l = layout(&aspects, &groups, 900.0, 140.0, square);
            let mut next = 0;
            for (k, row) in l.rows.iter().enumerate() {
                assert_eq!(row.start, next, "rows are contiguous");
                assert!(row.end > row.start);
                for c in &l.cells[row.start..row.end] {
                    assert!((c.top() - row.top).abs() < 0.01 && (c.bottom() - row.bottom).abs() < 0.01, "{c:?} {row:?}");
                }
                if let Some(prev) = k.checked_sub(1).map(|p| l.rows[p]) {
                    assert!(row.top > prev.bottom, "{prev:?} {row:?}");
                }
                next = row.end;
            }
            assert_eq!(next, aspects.len());
            // the binary search finds exactly the cells a linear scan does
            for (top, bottom) in [(0.0, 300.0), (1234.0, 2100.0), (l.height - 50.0, l.height + 900.0), (-500.0, -1.0)] {
                let band = Rect::from_min_max(pos2(0.0, top), pos2(2000.0, bottom));
                let want: Vec<usize> = (0..aspects.len()).filter(|i| l.cells[*i].intersects(band)).collect();
                let got: Vec<usize> = rows_between(&l.rows, top, bottom).iter().flat_map(|r| r.start..r.end).collect();
                assert_eq!(got, want, "square {square} {top}..{bottom}");
            }
        }
    }

    #[test]
    fn square_grid_groups_and_auto_levels() {
        let l = layout(&[1.0; 5], &[(0, 3), (3, 2)], 400.0, 100.0, true);
        // 4 columns: group 1 fills 3 cells of a row, group 2 starts a new row
        assert_eq!(l.cells[0].top(), l.cells[2].top());
        assert_eq!(l.cells[3].left(), 4.0);
        assert!(l.cells[3].top() > l.cells[0].bottom() + HEADER_H - 1.0);
        assert!((l.cells[0].width() - l.cells[0].height()).abs() < 0.01);
        assert_eq!(resolve_group(GroupBy::Auto, 220.0), GroupBy::Day);
        assert_eq!(resolve_group(GroupBy::Auto, 120.0), GroupBy::Month);
        assert_eq!(resolve_group(GroupBy::Auto, 90.0), GroupBy::Year);
        assert_eq!(resolve_group(GroupBy::None, 220.0), GroupBy::None);
        assert_eq!(resolve_group(GroupBy::Year, 400.0), GroupBy::Year);
    }
}
