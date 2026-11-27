//! Photo Merge ▸ Pixel Shift through the commands the menus run: the refusals and failures are said on the window, a
//! result that exists is not merged again, the question a series raises for the other merge kinds, Delete Merge
//! Sources, and the frames a merged file finds again when it is imported on its own. Synthetic frames only: their
//! headers carry the pixel-shift numbers, they have no pixels.

use std::time::Duration;

use lcc_catalog::{Op, PhotoId};
use serde_json::json;

use crate::headless::Headless;
use crate::{LccApp, Services};

const T: Duration = Duration::from_secs(20);
/// The command Photo ▸ Photo Merge ▸ Delete Merge Sources runs.
const DELETE: &str = "merge.deleteSourcesNow";

/// A header-only synthetic NEF with the pixel-shift numbers of frame `index` of a 16-shot series (all values invented).
fn frame_bytes(index: u8, shutter: f64) -> Vec<u8> {
    frame_bytes_of(index, 16, shutter)
}

/// The same for a series of `shots` (4, 8, 16 or 32), as the Z 8 writes the tags: `0x0034` says 16 in every mode, the mode is
/// byte 4 of `0x005e` and its byte 5 is the frame's slot (the number, 17 to 20 for frames 5 to 8 of 8 shots).
fn frame_bytes_of(index: u8, shots: u16, shutter: f64) -> Vec<u8> {
    use lcc_tiff::{ByteOrder, IfdBuilder, TiffWriter, Value, tags as t, writer::rational};
    let mut note = IfdBuilder::new();
    note.set(0x0034, Value::Short(vec![16]));
    let mut a = vec![0u8; 16];
    a[8] = index;
    note.set(0x0056, Value::Undefined(a));
    let mut b = vec![0u8; 28];
    b[4] = match shots {
        4 => 1,
        8 => 2,
        16 => 3,
        _ => 4,
    };
    b[5] = if shots == 8 && index > 4 { index + 12 } else { index };
    note.set(0x005e, Value::Undefined(b));
    note.set(0x00a7, Value::Long(vec![5000 + u32::from(index)]));
    let mut maker = b"Nikon\0\x02\0\0\0".to_vec();
    maker.extend(TiffWriter::new(ByteOrder::Little, false).write(&[note]).unwrap());
    let mut exif = IfdBuilder::new();
    exif.set(t::MAKER_NOTE, Value::Undefined(maker));
    exif.set(t::EXPOSURE_TIME, Value::Rational(vec![rational(shutter)]));
    exif.set(t::DATE_TIME_ORIGINAL, Value::Ascii("2030:05:06 07:08:09".into()));
    exif.set(0x9291, Value::Ascii(format!("{:02}", u32::from(index) * 6)));
    let mut ifd0 = IfdBuilder::new();
    ifd0.set(t::MAKE, Value::Ascii("NIKON CORPORATION".into()));
    ifd0.set(t::MODEL, Value::Ascii("NIKON Z 8".into()));
    ifd0.set_child(t::EXIF_IFD, exif);
    TiffWriter::new(ByteOrder::Little, false).write(&[ifd0]).unwrap()
}

fn captured(index: u8) -> String {
    format!("2030-05-06T07:08:09.{:03}+02:00", u32::from(index) * 6)
}

/// A library of the 16 frames of a series (`frame-01.nef` …) on disk, in the catalog and not yet selected.
fn series(tag: &str) -> (lcc_testkit::TempDir, Headless, Vec<PhotoId>) {
    series_of(tag, 16)
}

/// The same for a series of `shots` frames (4, 8, 16 or 32), written as the Z 8 writes them.
fn series_of(tag: &str, shots: u8) -> (lcc_testkit::TempDir, Headless, Vec<PhotoId>) {
    let dir = lcc_testkit::TempDir::new(&format!("lc-pixelshift-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = lcc_engine::Session::new().with_fs();
    let mut ids = Vec::new();
    for i in 1..=shots {
        let name = format!("frame-{i:02}.nef");
        let path = dir.join(&name).to_string_lossy().to_string();
        std::fs::write(&path, frame_bytes_of(i, u16::from(shots), 0.01)).unwrap();
        ids.push(add_photo(&mut session, &path, &name, "NEF", Some(captured(i))));
    }
    let app = LccApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.settle(T);
    (dir, h, ids)
}

fn add_photo(session: &mut lcc_engine::Session, path: &str, name: &str, format: &str, captured: Option<String>) -> PhotoId {
    let id = session.catalog.alloc_photo_id();
    let mut photo = lcc_catalog::Photo::new(id, lcc_catalog::Source::File { path: path.into() }, name, format, 8256, 5504, "2031-01-01T00:00:00");
    photo.captured = captured;
    session.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    id
}

fn select(h: &mut Headless, ids: &[PhotoId]) {
    h.app.session.execute("library.select", &json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>()})).unwrap();
}

/// The text of the message the window shows (the latest).
fn said(h: &Headless) -> String {
    h.app.ui.toast().map(|t| t.text.clone()).unwrap_or_default()
}

#[test]
fn a_selection_that_is_not_one_series_says_why_and_merges_nothing() {
    let (dir, mut h, ids) = series("refusals");
    // a frame is missing
    select(&mut h, &ids[..15]);
    assert!(h.app.run("merge.pixelShiftNow", json!({})).is_err());
    assert_eq!(said(&h), "Pixel Shift: Frame 16 of the series is missing");
    // a file that is not a pixel-shift frame
    h.app.ui.toasts.clear();
    let plain = dir.join("plain.nef");
    std::fs::write(&plain, b"not a tiff").unwrap();
    let plain_id = add_photo(&mut h.app.session, &plain.to_string_lossy(), "plain.nef", "NEF", None);
    let mut with_plain = ids[..15].to_vec();
    with_plain.push(plain_id);
    select(&mut h, &with_plain);
    assert!(h.app.run("merge.pixelShiftNow", json!({})).is_err());
    assert_eq!(said(&h), "Pixel Shift: plain.nef is not a pixel-shift frame");
    // another exposure among the frames
    h.app.ui.toasts.clear();
    let other = dir.join("frame-05.nef");
    std::fs::write(&other, frame_bytes(5, 0.02)).unwrap();
    select(&mut h, &ids);
    assert!(h.app.run("merge.pixelShiftNow", json!({})).is_err());
    assert_eq!(said(&h), "Pixel Shift: The photos have different exposure settings");
    // one photo
    h.app.ui.toasts.clear();
    select(&mut h, &ids[..1]);
    assert!(h.app.run("merge.pixelShiftNow", json!({})).is_err());
    assert_eq!(said(&h), "Pixel Shift: Select the frames of a pixel-shift series");
    assert!(h.app.merge.final_task.is_none(), "nothing started");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_merge_whose_result_exists_is_not_refused_and_never_touches_it() {
    let (dir, mut h, ids) = series("exists");
    std::fs::write(dir.join("frame-01-PixelShift.dng"), b"an earlier result").unwrap();
    std::fs::write(dir.join("frame-01-PixelShift-Plain.dng"), b"an earlier plain result").unwrap();
    select(&mut h, &ids);
    for command in ["merge.pixelShiftNow", crate::menus::PIXEL_SHIFT_PLAIN_ID] {
        h.app.run(command, json!({})).unwrap();
        assert!(h.app.merge.final_task.is_some(), "{command}: started, not refused");
        // (the frames have no pixels: it fails when it decodes)
        assert!(h.step_until(T, |h| h.app.merge.final_task.is_none()));
    }
    assert_eq!(std::fs::read(dir.join("frame-01-PixelShift.dng")).unwrap(), b"an earlier result", "not replaced");
    assert_eq!(std::fs::read(dir.join("frame-01-PixelShift-Plain.dng")).unwrap(), b"an earlier plain result", "not replaced");
    assert!(
        !dir.join("frame-01-PixelShift-2.dng").exists() && !dir.join("frame-01-PixelShift-Plain-2.dng").exists(),
        "a merge that failed wrote nothing"
    );
    assert_eq!(h.app.session.catalog.len(), 16, "no photo added");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_merge_that_fails_while_running_says_so() {
    // the frames have no pixels: the merge starts (the headers are fine) and fails when it decodes
    let (dir, mut h, ids) = series("fails");
    select(&mut h, &ids);
    h.app.run("merge.pixelShiftNow", json!({})).unwrap();
    assert!(h.step_until(T, |h| h.app.merge.final_task.is_none()));
    assert!(said(&h).starts_with("Pixel shift merge failed: "), "{}", said(&h));
    assert_eq!(h.app.session.catalog.len(), 16, "no photo added");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_question_for_another_kind_of_merge_on_a_series_has_three_answers() {
    let (dir, mut h, ids) = series("offer");
    // exactly one complete series: asked, nothing opened or started
    select(&mut h, &ids);
    h.app.run("dialog.mergeHdr", json!({})).unwrap();
    assert!(format!("{:?}", h.app.ui.dialog).contains("PixelShiftOffer"), "{:?}", h.app.ui.dialog);
    assert!(h.app.merge.final_task.is_none());
    // Cancel: nothing
    h.step();
    assert!(h.click_as_user("button:dialogCancel"), "Cancel is there");
    assert!(h.app.ui.dialog.is_none() && h.app.merge.final_task.is_none() && h.app.merge.preview_task.is_none());
    // Continue: the HDR dialog, as without the question
    h.app.run("dialog.mergeFocusStack", json!({})).unwrap();
    h.step();
    assert!(h.click_as_user("button:dialogContinueMerge"), "Continue is there");
    assert!(matches!(&h.app.ui.dialog, Some(crate::state::Dialog::Merge { opts }) if opts.command == "merge.focusStack"), "{:?}", h.app.ui.dialog);
    h.app.ui.dialog = None;
    // Pixel Shift: the pixel-shift merge starts
    h.app.run("dialog.mergePanorama", json!({})).unwrap();
    h.step();
    assert!(h.click_as_user("button:dialogOk"), "Pixel Shift is there");
    // (the frames have no pixels: it starts and fails at once, and says which merge it was)
    assert!(h.step_until(T, |h| h.app.merge.final_task.is_none()));
    assert!(said(&h).starts_with("Pixel shift merge failed: "), "{}", said(&h));
    assert!(!h.app.session.last_merge.contains_key("merge.panorama"), "with its own options, not the panorama's");
    assert!(h.step_until(T, |h| h.app.merge.final_task.is_none()));
    // another selection: no question, the dialog of the kind asked for
    h.app.ui.dialog = None;
    select(&mut h, &ids[..6]);
    h.app.run("dialog.mergeHdr", json!({})).unwrap();
    assert!(matches!(&h.app.ui.dialog, Some(crate::state::Dialog::Merge { .. })), "{:?}", h.app.ui.dialog);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The merged photo of a series: frame 01 and it stacked, its sources remembered (as `finish_merge` leaves them).
fn merged(h: &mut Headless, dir: &std::path::Path, frames: &[PhotoId]) -> PhotoId {
    let path = dir.join("frame-01-PixelShift.dng").to_string_lossy().to_string();
    let id = add_photo(&mut h.app.session, &path, "frame-01-PixelShift.dng", "DNG", Some(captured(1)));
    let sources = lcc_catalog::MergeSources {
        engine: 1,
        shot_count: 16,
        reference: 1,
        scale: 2,
        frames: frames
            .iter()
            .enumerate()
            .map(|(k, f)| lcc_catalog::MergeSource {
                photo: Some(*f),
                index: k as u16 + 1,
                file_name: format!("frame-{:02}.nef", k + 1),
                captured: captured(k as u8 + 1),
                counter: 5000 + k as u32 + 1,
            })
            .collect(),
        registration: None,
    };
    let stack = h.app.session.catalog.stack_with_ops(id, frames).unwrap();
    h.app.session.catalog.apply(stack).unwrap();
    h.app.session.catalog.apply(Op::SetMergeSources { id, sources: Some(Box::new(sources)) }).unwrap();
    select(h, &[id]);
    id
}

#[test]
fn delete_merge_sources_moves_frames_two_to_sixteen_to_recently_deleted() {
    for confirm in [false, true] {
        let (dir, mut h, ids) = series(&format!("delete-{confirm}"));
        let id = merged(&mut h, &dir, &ids);
        h.app.ui.settings.confirm_delete = confirm;
        assert!(crate::menus::ui_enabled(&h.app, DELETE), "enabled on the merged photo");
        h.app.run(DELETE, json!({})).unwrap();
        if confirm {
            // asked first, nothing deleted until the question is answered
            assert!(format!("{:?}", h.app.ui.dialog).contains("ConfirmDeleteSources"), "{:?}", h.app.ui.dialog);
            assert!(ids.iter().all(|i| !h.app.session.catalog.photo(*i).unwrap().deleted));
            h.step();
            assert!(h.click_as_user("button:dialogOk"));
        } else {
            assert!(h.app.ui.dialog.is_none(), "not asked with the setting off");
        }
        let deleted = |i: &PhotoId| h.app.session.catalog.photo(*i).unwrap().deleted;
        assert!(!deleted(&ids[0]), "frame 01 stays");
        assert!(ids[1..].iter().all(deleted), "frames 02–16 are in Recently Deleted (confirm: {confirm})");
        assert!(!h.app.session.catalog.photo(id).unwrap().deleted, "the merged photo stays");
        let stack = h.app.session.catalog.stack_of(id).expect("still stacked");
        let live: Vec<PhotoId> = stack.photos.iter().copied().filter(|p| !h.app.session.catalog.photo(*p).unwrap().deleted).collect();
        assert_eq!(live.len(), 2, "the merged photo and frame 01: {live:?}");
        assert!(live.contains(&id) && live.contains(&ids[0]));
        assert_eq!(said(&h), "Frames moved to Recently Deleted: 15");
        // nothing but frame 01 is left: greyed out
        assert!(!crate::menus::ui_enabled(&h.app, DELETE));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn delete_merge_sources_with_some_frames_gone_says_how_many() {
    let (dir, mut h, ids) = series("partial");
    let id = merged(&mut h, &dir, &ids);
    h.app.ui.settings.confirm_delete = false;
    // three frames are already in Recently Deleted or no longer in the library
    h.app.session.execute("photo.delete", &json!({"ids": [ids[2].0, ids[3].0]})).unwrap();
    h.app.session.catalog.apply(h.app.session.catalog.delete_photos_permanently_ops(&[ids[9]])).unwrap();
    select(&mut h, &[id]);
    h.app.run(DELETE, json!({})).unwrap();
    assert!(ids[1..].iter().all(|i| h.app.session.catalog.photo(*i).is_none_or(|p| p.deleted)), "every frame is gone now");
    assert!(!h.app.session.catalog.photo(ids[0]).unwrap().deleted);
    assert_eq!(said(&h), "Frames moved to Recently Deleted: 12; already gone: 3");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_merge_sources_is_greyed_out_for_other_photos() {
    let (dir, mut h, ids) = series("greyed");
    select(&mut h, &ids[..1]);
    assert!(!crate::menus::ui_enabled(&h.app, DELETE), "a plain photo");
    let err = h.app.run(DELETE, json!({})).unwrap_err();
    assert_eq!(err, "This photo is not a pixel-shift merge");
    assert_eq!(said(&h), err, "said on the window too");
    // a merged photo whose frames other than 01 are all gone
    let id = merged(&mut h, &dir, &ids);
    h.app.ui.settings.confirm_delete = false;
    h.app.run(DELETE, json!({})).unwrap();
    assert!(!crate::menus::ui_enabled(&h.app, DELETE));
    select(&mut h, &[id]);
    assert!(h.app.run(DELETE, json!({})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_merged_file_imported_on_its_own_finds_its_frames_again() {
    use lcc_geom::Orientation;
    use lcc_merge::pixelshift::{DngPacking, DngSource, Merger, SHOT_16, write_dng};
    use lcc_raw::{BlackLevel, Cfa, ColorData, RawData, RawFormat, RawImage, Rect};
    let (dir, mut h, ids) = series("provenance");
    // the library has all frames but 3 and 7 (index 3 and 7)
    for gone in [ids[2], ids[6]] {
        h.app.session.catalog.apply(h.app.session.catalog.delete_photos_permanently_ops(&[gone])).unwrap();
    }
    let (w, ht) = (24usize, 24usize);
    let frame = |k: usize| RawImage {
        format: RawFormat::Nef,
        width: w,
        height: ht,
        cpp: 1,
        data: RawData::U16((0..w * ht).map(|i| 1100 + ((i * 7 + k * 13) % 3000) as u16).collect()),
        cfa: Cfa::bayer("RGGB"),
        bits: 14,
        black: BlackLevel::uniform(1008.0),
        white: vec![16383.0],
        active_area: Rect::new(0, 0, w, ht),
        crop: Rect::new(2, 2, 20, 20),
        orientation: Orientation::Normal,
        color: ColorData::default(),
        wb_multipliers: None,
        linearized: false,
        pixel_aspect: 1.0,
        opcodes: Default::default(),
        metadata: Default::default(),
        lens_metadata: Default::default(),
    };
    let mut merger = Merger::new(&SHOT_16, &frame(0)).unwrap();
    for k in 0..16 {
        merger.add_frame(k, &frame(k)).unwrap();
    }
    let metadata = lcc_meta::Metadata {
        make: Some("NIKON CORPORATION".into()),
        model: Some("NIKON Z 8".into()),
        capture_time: lcc_meta::DateTime::parse_iso(&captured(1)),
        ..Default::default()
    };
    let provenance = lcc_raw::merge_provenance::Provenance {
        engine: 1,
        shot_count: 16,
        reference: 1,
        scale: 2,
        window: None,
        frames: (1..=16u8)
            .map(|i| lcc_raw::merge_provenance::SourceFrame {
                index: u16::from(i),
                file_name: format!("frame-{i:02}.nef"),
                captured: captured(i),
                counter: 5000 + u32::from(i),
            })
            .collect(),
        registration: None,
    };
    let dng = write_dng(
        merger.finish().unwrap(),
        DngSource { metadata, color: ColorData::default(), orientation: Orientation::Normal, lens: vec![], provenance, preview: None },
        DngPacking::Deflate,
    )
    .unwrap();
    // beside its frames (a block that names frames elsewhere finds none of them)
    let merged_path = dir.join("frame-01-PixelShift.dng");
    std::fs::write(&merged_path, &dng).unwrap();
    let r = h.app.session.execute("library.import", &json!({"paths": [merged_path.to_string_lossy()]})).unwrap();
    let id = PhotoId(r["imported"][0].as_u64().expect("imported"));
    let sources = h.app.session.catalog.photo(id).unwrap().merge_sources.clone().expect("its sources are restored");
    assert_eq!((sources.shot_count, sources.reference, sources.frames.len()), (16, 1, 16));
    for (k, f) in sources.frames.iter().enumerate() {
        let expected = if k == 2 || k == 6 { None } else { Some(ids[k]) };
        assert_eq!(f.photo, expected, "frame {}", k + 1);
        assert_eq!((f.index, f.counter), (k as u16 + 1, 5000 + k as u32 + 1));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A series of another pixel-shift mode is refused by name, whichever of its frames are chosen: the first 16 frames of a
/// 32-shot series are not a 16-shot series (the Z 8 writes 16 in its old shot-count tag in every mode), and nothing starts.
#[test]
fn a_series_of_another_mode_is_refused_by_name_and_never_merged_as_16_shots() {
    for (shots, selections) in [(32u8, vec![(0, 16), (16, 32), (0, 32)]), (8, vec![(0, 8), (0, 4)]), (4, vec![(0, 4)])] {
        let (dir, mut h, ids) = series_of(&format!("mode{shots}"), shots);
        for (from, to) in selections {
            h.app.ui.toasts.clear();
            select(&mut h, &ids[from..to]);
            assert!(h.app.run("merge.pixelShiftNow", json!({})).is_err(), "{shots}-shot frames {from}..{to}");
            assert_eq!(said(&h), format!("Pixel Shift: {shots}-shot pixel-shift series are not supported yet"), "{shots}-shot frames {from}..{to}");
            assert!(h.app.merge.final_task.is_none() && h.app.session.activity.list().is_empty(), "nothing started");
        }
        assert_eq!(h.app.session.catalog.len(), usize::from(shots), "no photo added");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// What the job of the merge running now will do.
fn running_kind(h: &Headless) -> lcc_engine::merge::MergeKind {
    h.app.merge.final_task.as_ref().expect("a merge is running").job.kind.clone()
}

/// Photo Merge ▸ Pixel Shift registers the series; Pixel Shift, Plain Merge is one entry away, in the same menu, the grid's
/// context menu and the command palette, and its choice is not remembered by the next Pixel Shift.
#[test]
fn a_pixel_shift_merge_is_registered_by_default_and_the_plain_merge_is_one_entry_away() {
    use lcc_engine::merge::MergeKind;
    let (dir, mut h, ids) = series("default");
    select(&mut h, &ids);
    h.app.run("merge.pixelShiftNow", json!({})).unwrap();
    assert_eq!(running_kind(&h), MergeKind::PixelShift { registered: true }, "registered by default");
    assert_eq!(h.app.merge.final_task.as_ref().unwrap().options.method, "registered");
    // (the frames have no pixels: it fails when it decodes)
    assert!(h.step_until(T, |h| h.app.merge.final_task.is_none()));
    // the plain merge, from its own entry
    h.app.run(crate::menus::PIXEL_SHIFT_PLAIN_ID, json!({})).unwrap();
    assert_eq!(running_kind(&h), MergeKind::PixelShift { registered: false }, "the plain merge");
    assert_eq!(h.app.merge.final_task.as_ref().unwrap().options.method, "plain");
    assert_eq!(h.app.session.last_merge["merge.pixelShift"]["method"], "registered", "the plain merge is not what is remembered");
    assert!(h.step_until(T, |h| h.app.merge.final_task.is_none()));
    // and the next Pixel Shift is registered again
    h.app.run("merge.pixelShiftNow", json!({})).unwrap();
    assert_eq!(running_kind(&h), MergeKind::PixelShift { registered: true });
    assert!(h.step_until(T, |h| h.app.merge.final_task.is_none()));
    // both entries are in the Photo Merge menu (and so the palette), the plain one after the registered one
    let place = |id: &str| crate::menus::UI_COMMANDS.iter().position(|c| c.0 == id).unwrap_or_else(|| panic!("{id} is a menu command"));
    assert_eq!(place(crate::menus::PIXEL_SHIFT_PLAIN_ID), place(crate::menus::PIXEL_SHIFT_ID) + 1);
    let entry = crate::menus::UI_COMMANDS[place(crate::menus::PIXEL_SHIFT_PLAIN_ID)];
    assert_eq!((entry.1, entry.3), ("Pixel Shift, Plain Merge…", "Photo>Photo Merge"));
    // a selection that is not a series is refused by the plain entry as by the other, with the same words
    h.app.ui.toasts.clear();
    select(&mut h, &ids[..15]);
    assert!(h.app.run(crate::menus::PIXEL_SHIFT_PLAIN_ID, json!({})).is_err());
    assert_eq!(said(&h), "Pixel Shift: Frame 16 of the series is missing");
    assert!(h.app.merge.final_task.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Stopping a pixel-shift merge leaves no file and no photo behind, whether it is stopped at once or fails on its own.
#[test]
fn a_cancelled_pixel_shift_merge_leaves_no_file_and_no_photo() {
    let (dir, mut h, ids) = series("cancel");
    select(&mut h, &ids);
    let files = |dir: &std::path::Path| std::fs::read_dir(dir).unwrap().count();
    let before = files(&dir);
    h.app.run("merge.pixelShiftNow", json!({})).unwrap();
    h.app.merge.final_task.as_ref().unwrap().cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(h.step_until(T, |h| h.app.merge.final_task.is_none()));
    // stopped at once the merge says it was cancelled; the frames have no pixels, so one that got as far as decoding
    // says it failed: either way nothing was written or added
    assert!(said(&h).starts_with("Pixel shift merge cancelled") || said(&h).starts_with("Pixel shift merge failed: "), "{}", said(&h));
    assert_eq!((files(&dir), h.app.session.catalog.len()), (before, 16));
    assert!(h.app.session.activity.list().is_empty(), "its row in the activity stack is gone");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A high-ISO series gets a hint with its registered merge and not with the plain one; it blocks nothing.
#[test]
fn the_iso_hint_comes_with_a_registered_merge_of_high_iso_frames_only() {
    let (dir, mut h, ids) = series("iso");
    let set_iso = |h: &mut Headless, iso: u32| {
        for id in &ids {
            let mut meta = h.app.session.catalog.photo(*id).unwrap().meta.clone();
            meta.iso = Some(iso);
            h.app.session.catalog.apply(Op::SetMeta { id: *id, meta: Box::new(meta) }).unwrap();
        }
    };
    select(&mut h, &ids);
    for (iso, command, hint) in [
        (3200, "merge.pixelShiftNow", true),
        (3200, crate::menus::PIXEL_SHIFT_PLAIN_ID, false),
        (1600, "merge.pixelShiftNow", true),
        (800, "merge.pixelShiftNow", false),
        (64, "merge.pixelShiftNow", false),
    ] {
        set_iso(&mut h, iso);
        h.app.ui.toasts.clear();
        h.app.run(command, json!({})).unwrap();
        assert!(h.app.merge.final_task.is_some(), "the hint blocks nothing (ISO {iso})");
        assert_eq!(said(&h).starts_with(&format!("ISO {iso}: ")), hint, "ISO {iso}, {command}: {}", said(&h));
        assert!(h.step_until(T, |h| h.app.merge.final_task.is_none()));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// What the window says when a merge finished: nothing more than "added" for a merge that was registered as asked, and the
/// reason in plain words, in the person's language, for one that was not.
#[test]
fn the_message_after_a_pixel_shift_merge_says_why_it_was_not_registered() {
    let added = json!({"kind": "pixelShift", "method": "registered", "registration": {"registered": true, "verdict": "better"}});
    assert_eq!(crate::merge::finished_message("Pixel shift", &added), "Pixel shift merge added");
    let plain_asked = json!({"kind": "pixelShift", "method": "plain", "registration": {"registered": false}});
    assert_eq!(
        crate::merge::finished_message("Pixel shift", &plain_asked),
        "Pixel shift merge added",
        "no word about a merge that was asked to be plain"
    );
    for (code, words) in [
        ("noTexture", "not enough texture to measure the frame positions"),
        ("tooFewTiles", "not enough texture to measure the frame positions"),
        ("noMatch", "the frames could not be matched to each other"),
        ("measuredWorse", "the measured positions were less reliable than the camera's own pattern"),
        ("memory", "there was not enough memory for the registered merge"),
    ] {
        let info =
            json!({"kind": "pixelShift", "method": "plain", "fallback": {"reason": code, "message": lcc_engine::merge::fallback_message(code)}});
        let text = crate::merge::finished_message("Pixel shift", &info);
        assert_eq!(text, format!("Pixel shift merge added, but not registered: {words}"), "{code}");
        let info = crate::toast::MessageSettings::default().info_seconds();
        let short = crate::toast::stay_seconds(crate::ToastKind::Message, "Pixel shift merge added", info);
        assert!(crate::toast::stay_seconds(crate::ToastKind::Message, &text, info) > short, "long enough to read");
    }
    // in another language the same message is translated, reason and all
    crate::i18n::set_language(crate::i18n::Locale::De);
    let info = json!({"fallback": {"reason": "measuredWorse", "message": lcc_engine::merge::fallback_message("measuredWorse")}});
    let text = crate::merge::finished_message("Pixel shift", &info);
    crate::i18n::set_language(crate::i18n::Locale::En);
    assert!(text.contains("nicht registriert") && text.contains("zuverlässig"), "{text}");
}
