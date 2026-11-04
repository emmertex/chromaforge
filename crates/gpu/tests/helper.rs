//! Two devices: the second graphics card's renders match the editing card's, batch work goes there
//! while it is ready, and its loss (or turning it off) leaves the editing card alone. On a machine
//! with one card the second device is made on that same card (`same`: the two-device logic, not
//! two cards' speed). Own process: the device and the second card are once per process.

use std::sync::Arc;
use std::time::{Duration, Instant};

use lcc_develop::DevelopSettings;
use lcc_pipeline::{RenderRequest, SourceInfo};

fn wait_for(what: &str, ok: impl Fn() -> bool) {
    let t = Instant::now();
    while !ok() {
        assert!(t.elapsed() < Duration::from_secs(60), "{what}: still {:?}", lcc_gpu::helper_status());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_second_device_renders_alike_and_its_loss_leaves_the_editing_card() {
    if !lcc_gpu::available() {
        lcc_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    let editing = lcc_gpu::device_generation();
    // a second device on the same card (this machine may have one card)
    assert!(lcc_gpu::set_helper("same"));
    wait_for("the second device starts", || lcc_gpu::helper_status().0 != "starting" && lcc_gpu::helper_status().0 != "off");
    assert_eq!(lcc_gpu::helper_status().0, "ready", "{:?}", lcc_gpu::helper_status());

    let raw = SourceInfo { raw: true, ..Default::default() };
    let mut s = DevelopSettings::default();
    s.light.exposure = 0.4;
    s.effects.clarity = 15.0;
    s.detail.nr_luminance = 15.0;
    let src = Arc::new(lcc_scenes::demo_library()[1].render(900, 600));
    let req = RenderRequest::fit(900, 600);
    let on_editing = lcc_gpu::try_render(&src, &raw, &s, &req, None).expect("editing card");
    let before = lcc_gpu::helper_usage();
    let on_second = lcc_gpu::as_batch(|| lcc_gpu::try_render(&src, &raw, &s, &req, None)).expect("second device");
    assert_eq!(before.0, 0);
    assert!(on_second.image.data == on_editing.image.data, "both devices render the same pixels");
    assert_eq!(on_second.histogram, on_editing.histogram);
    // AI models go to the second card (here the same one)
    assert_eq!(lcc_gpu::ai_card(), lcc_gpu::adapters().iter().find(|e| e.usable).map(|e| e.name.clone()));
    // a photo's turn on the second device: its renders run there
    let turn = lcc_gpu::as_batch(|| lcc_gpu::try_batch_turn(900 * 600, 900 * 600)).expect("a free turn");
    assert_eq!(lcc_gpu::helper_usage().0, 1, "the turn is the second device's");
    drop(turn);
    assert_eq!(lcc_gpu::helper_usage().0, 0);

    // the second device fails: batch work goes back to the editing card, which is untouched
    assert!(lcc_gpu::simulate_helper_loss("test: the second card stopped"));
    assert_eq!(lcc_gpu::helper_status().0, "lost");
    assert_eq!(lcc_gpu::device_generation(), editing, "the editing card's device stays");
    assert!(lcc_gpu::lost().is_none(), "the editing card is not lost");
    let after = lcc_gpu::as_batch(|| lcc_gpu::try_render(&src, &raw, &s, &req, None)).expect("editing card again");
    assert!(after.image.data == on_editing.image.data);

    // chosen again, then turned off
    assert!(lcc_gpu::set_helper("auto"));
    assert!(lcc_gpu::set_helper("same"));
    wait_for("the second device starts again", || lcc_gpu::helper_status().0 == "ready");
    assert!(lcc_gpu::set_helper("off"));
    wait_for("the second device stops", || lcc_gpu::helper_status().0 == "off");
    assert_eq!(lcc_gpu::device_generation(), editing);
}

/// Hostile settings never panic: text that is no card is Automatic, and a card that isn't there
/// leaves the device in use.
#[test]
fn unknown_card_settings_are_harmless() {
    assert_eq!(lcc_gpu::cards::HelperChoice::parse_setting("dx12:zz:zz:#0"), lcc_gpu::cards::HelperChoice::Auto);
    assert!(lcc_gpu::cards::AdapterKey::parse("vulkan:1002:1586:nothing#99999999999").is_none());
    assert!(lcc_gpu::cards::AdapterKey::parse(":::").is_none());
}
