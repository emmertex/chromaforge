//! Renders handed to the window as textures (`try_render_display` on an adopted device): the
//! texture holds exactly the pixels a readback render returns, the histogram matches, and what
//! can't be a texture (overlays, a window grown by spots) comes back as pixels. Own process:
//! adopting a device is once per process. Skips without a hardware adapter.

use std::sync::Arc;

use lcc_develop::{DevelopSettings, Spot};
use lcc_geom::Point;
use lcc_gpu::wgpu;
use lcc_pipeline::{Overlay, PixelWindow, Quality, RenderRequest, SourceInfo, StageCache};

/// A device made the way eframe makes the window's (adapter limits, the window's backends).
fn window_device() -> Option<(wgpu::Device, wgpu::Queue, wgpu::Adapter)> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = lcc_gpu::backend::window_backends();
    desc.backend_options = lcc_gpu::backend::backend_options();
    let instance = wgpu::Instance::new(desc);
    let adapter = pollster::block_on(
        instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }),
    )
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("test window"),
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .ok()?;
    Some((device, queue, adapter))
}

#[test]
fn display_renders_match_readback_renders() {
    let Some((device, queue, adapter)) = window_device() else {
        lcc_testkit::skip!(Gpu, "no GPU adapter");
        return;
    };
    if adapter.get_info().device_type == wgpu::DeviceType::Cpu {
        lcc_testkit::skip!(Gpu, "software adapter");
        return;
    }
    lcc_gpu::adopt_window_device(device.clone(), queue.clone(), &adapter).expect("adopted");
    assert!(lcc_gpu::on_window_device());
    let raw = SourceInfo { raw: true, ..Default::default() };
    let mut s = DevelopSettings::default();
    s.light.exposure = 0.5;
    s.light.highlights = -40.0;
    s.effects.clarity = 20.0;
    s.detail.nr_luminance = 20.0;
    s.grain.amount = 20.0;
    // widths that are and aren't a multiple of 64 texels (row padding), draft and full quality
    for (w, h, quality) in [(640, 427, Quality::Full), (1000, 667, Quality::Draft), (1024, 683, Quality::Full)] {
        let src = Arc::new(lcc_scenes::demo_library()[0].render(w, h));
        let req = RenderRequest { quality, ..RenderRequest::fit(w, h) };
        let stages = StageCache::default();
        let pixels = lcc_gpu::try_render(&src, &raw, &s, &req, Some(&stages)).expect("gpu render");
        let (shown, frame) = lcc_gpu::try_render_display(&src, &raw, &s, &req, Some(&stages)).expect("display render");
        let frame = frame.expect("a texture");
        assert_eq!((shown.image.width, shown.image.height), (0, 0), "no pixels came back");
        assert_eq!((frame.width, frame.height), (pixels.image.width, pixels.image.height));
        let read = lcc_gpu::read_texture(&device, &queue, &frame.texture, frame.width, frame.height).expect("readback");
        assert!(read.data == pixels.image.data, "{w}×{h}: the texture holds the readback's pixels");
        assert_eq!(shown.histogram, pixels.histogram);
    }
    // overlays are drawn on the CPU, and a window that spots grew is cut there: pixels
    let src = Arc::new(lcc_scenes::demo_library()[0].render(3000, 2000));
    let req = RenderRequest { overlay: Overlay::Spots(50), ..RenderRequest::fit(800, 800) };
    let (r, frame) = lcc_gpu::try_render_display(&src, &raw, &s, &req, None).expect("overlay render");
    assert!(frame.is_none() && r.image.width > 0, "an overlay comes back as pixels");
    s.spots = vec![Spot { points: vec![Point::new(0.5, 0.5)], size: 0.02, source_offset: Some(Point::new(0.15, 0.0)), ..Default::default() }];
    let req = RenderRequest { window: Some(PixelWindow { x: 1200, y: 800, w: 500, h: 400 }), ..RenderRequest::fit(3000, 2000) };
    let (r, frame) = lcc_gpu::try_render_display(&src, &raw, &s, &req, None).expect("window render");
    assert!(frame.is_none(), "a grown window comes back as pixels");
    assert_eq!((r.image.width, r.image.height), (500, 400));
    // hostile sizes: an empty or oversized texture read is an error, not a panic
    assert!(lcc_gpu::read_texture(&device, &queue, &frame_texture(&device), 0, 0).is_err());
    assert!(lcc_gpu::read_texture(&device, &queue, &frame_texture(&device), 1 << 20, 4).is_err());
}

fn frame_texture(device: &wgpu::Device) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
