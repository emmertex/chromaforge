//! Photo ▸ People: the items that act on "the chosen face" (fork-owned). One accessor,
//! [`chosen_face`], says which face that is, so every People item agrees:
//! - in the People view, the one unnamed face selected;
//! - in the loupe, the face whose name box is open, else the face box the pointer last rested on.
//!
//! Without a chosen face the items are disabled.

use serde_json::{Value, json};

use crate::LccApp;
use crate::menus::UiCommand;
use crate::module::ModuleId;
use crate::state::{NameEdit, ViewMode};

pub const MOVE_TO_UNNAMED: &str = "face.moveToUnnamed";
pub const REMOVE_REGION: &str = "face.removeRegion";
pub const EDIT_NAME: &str = "face.editName";

pub const COMMANDS: &[UiCommand] = &[
    (MOVE_TO_UNNAMED, "Move to Unnamed People", None, ""),
    (REMOVE_REGION, "Remove Face Region", None, ""),
    (EDIT_NAME, "Edit Name", Some("Shift+O"), ""),
];

/// The face the People items act on: (photo, region index), when it is a face region that exists.
pub fn chosen_face(app: &LccApp) -> Option<(u64, usize)> {
    let key = match app.ui.view {
        ViewMode::People => {
            let mut it = app.ui.unnamed_selected.iter();
            match (it.next(), it.next()) {
                (Some(k), None) => *k,
                _ => return None,
            }
        }
        ViewMode::Detail if app.ui.module == ModuleId::Library => {
            let active = app.session.active()?.0;
            let key = app.ui.name_edit.as_ref().map(|e| (e.photo, e.index)).or(app.ui.face_focus)?;
            if key.0 != active {
                return None;
            }
            key
        }
        _ => return None,
    };
    let photo = app.session.catalog.photo(lcc_catalog::PhotoId(key.0))?;
    photo.meta.regions.get(key.1).filter(|r| r.kind == lcc_meta::RegionKind::Face).map(|_| key)
}

fn region_name(app: &LccApp, (photo, index): (u64, usize)) -> Option<String> {
    app.session.catalog.photo(lcc_catalog::PhotoId(photo))?.meta.regions.get(index)?.name.clone()
}

pub fn enabled(app: &LccApp, id: &str) -> Option<bool> {
    let face = chosen_face(app);
    match id {
        MOVE_TO_UNNAMED => Some(face.and_then(|f| region_name(app, f)).is_some()),
        REMOVE_REGION | EDIT_NAME => Some(face.is_some()),
        _ => None,
    }
}

pub fn run(app: &mut LccApp, id: &str, _p: &Value) -> Option<Result<Value, String>> {
    if !matches!(id, MOVE_TO_UNNAMED | REMOVE_REGION | EDIT_NAME) {
        return None;
    }
    let Some((photo, index)) = chosen_face(app) else {
        return Some(Err("no face is chosen (select one unnamed face, or point at a face box in the loupe)".to_string()));
    };
    Some(match id {
        MOVE_TO_UNNAMED => app.run("faces.setName", json!({"id": photo, "index": index, "name": null})),
        REMOVE_REGION => {
            app.ui.unnamed_selected.remove(&(photo, index));
            app.ui.face_focus = None;
            app.ui.name_edit = None;
            app.run("photo.removeRegion", json!({"id": photo, "index": index}))
        }
        _ => {
            if app.ui.view == ViewMode::People {
                // the naming bar's box takes the keyboard
                app.ui.unnamed_focus = true;
            } else {
                let text = region_name(app, (photo, index)).unwrap_or_default();
                app.ui.name_edit = Some(NameEdit { photo, index, text, fresh: true });
            }
            Ok(json!({"photo": photo, "index": index}))
        }
    })
}
