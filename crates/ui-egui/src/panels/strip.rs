//! The tool panels of a module: Develop's editing tools, drawn as the row under its histogram
//! (`classic_tools`). The vertical strip that once stood at the far right is gone: its show/hide
//! button is in the top bar, Library's Versions in its left column.

use crate::LccApp;
use crate::icons::Icon;
use crate::module::{ModuleId, PanelId};
use crate::state::RightPanel;

/// The panels a module's tool strip lists. Develop's are its editing tools; Library's are its own
/// panels, which open over its Classic right column. The other modules have no strip.
pub fn strip_declared(module: ModuleId) -> &'static [PanelId] {
    match module {
        ModuleId::Develop => &[PanelId::Edit, PanelId::Crop, PanelId::Remove, PanelId::RedEye, PanelId::Masking],
        _ => &[],
    }
}

/// The active module's strip panels, in the user's order and without hidden ones.
pub fn strip_panels(app: &LccApp) -> Vec<PanelId> {
    let declared = strip_declared(app.ui.module);
    let mut out: Vec<PanelId> = app.ui.panel_order.iter().copied().filter(|p| declared.contains(p)).collect();
    for p in declared {
        if !out.contains(p) {
            out.push(*p);
        }
    }
    out.retain(|p| !app.ui.hidden_panels.contains(p));
    out
}

/// The strip button of a strip panel: (widget id, icon, panel, tooltip, needs a photo).
pub(crate) fn entry(p: PanelId) -> Option<(&'static str, Icon, RightPanel, &'static str, bool)> {
    use crate::module::PanelId as P;
    Some(match p {
        P::Edit => ("edit", Icon::Sliders, RightPanel::Edit, "Edit", true),
        P::Crop => ("crop", Icon::Crop, RightPanel::Crop, "Crop & Rotate", true),
        P::Remove => ("remove", Icon::Eraser, RightPanel::Remove, "Remove", true),
        P::Masking => ("masking", Icon::Mask, RightPanel::Masking, "Masking", true),
        P::RedEye => ("redeye", Icon::RedEye, RightPanel::RedEye, "Red Eye", true),
        P::Versions => ("versions", Icon::Versions, RightPanel::Versions, "Versions (Shift+V)", true),
        P::Activity => ("activity", Icon::Activity, RightPanel::Activity, "History & Activity", false),
        _ => return None,
    })
}
