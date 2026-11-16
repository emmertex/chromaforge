//! The idle trim frees the second graphics card's recycled buffers too: after a two-card export or
//! 1:1 build the second card kept up to the pool limit (2 GB with a 16 GB budget) for the rest of
//! the session. On a machine with one card the second device is made on that same card. Own
//! process: the device and the second card are once per process.

use std::sync::Arc;
use std::time::{Duration, Instant};

use lcc_develop::DevelopSettings;
use lcc_pipeline::{RenderRequest, SourceInfo};

#[test]
fn trimming_the_pool_frees_the_second_cards_buffers_too() {
    if !lcc_gpu::available() {
        lcc_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    lcc_gpu::set_pool_limit(512 << 20);
    assert!(lcc_gpu::set_helper("same"));
    let t = Instant::now();
    while matches!(lcc_gpu::helper_status().0, "starting" | "off") {
        assert!(t.elapsed() < Duration::from_secs(60), "the second device starts: {:?}", lcc_gpu::helper_status());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(lcc_gpu::helper_status().0, "ready", "{:?}", lcc_gpu::helper_status());
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut s = DevelopSettings::default();
    s.effects.clarity = 30.0;
    let src = Arc::new(lcc_scenes::demo_library()[1].render(1200, 800));
    let req = RenderRequest::fit(1200, 800);
    // batch work renders on the second device, whose buffers then sit in its pool
    let before = lcc_gpu::helper_usage();
    lcc_gpu::as_batch(|| lcc_gpu::try_render(&src, &info, &s, &req, None)).expect("render on the second device");
    assert_eq!(before.0, 0);
    assert!(lcc_gpu::memory().pooled > 0, "the second device's render left recycled buffers");
    lcc_gpu::trim_pool(0);
    assert_eq!(lcc_gpu::memory().pooled, 0, "buffers stayed pooled on the second card");
}
