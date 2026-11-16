use egui::{UserData, ViewportId};
use epaint::ColorImage;
use std::sync::{Arc, mpsc};
use wgpu::{BindGroupLayout, MultisampleState, StoreOp};

/// A texture and a buffer for reading the rendered frame back to the cpu.
///
/// The texture is required since [`wgpu::TextureUsages::COPY_SRC`] is not an allowed
/// flag for the surface texture on all platforms. This means that anytime we want to
/// capture the frame, we first render it to this texture, and then we can copy it to
/// both the surface texture (via a render pass) and the buffer (via a texture to buffer copy),
/// from where we can pull it back
/// to the cpu.
pub struct CaptureState {
    padding: BufferPadding,
    pub texture: wgpu::Texture,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
}

pub type CaptureReceiver = mpsc::Receiver<(ViewportId, Vec<UserData>, ColorImage)>;
pub type CaptureSender = mpsc::Sender<(ViewportId, Vec<UserData>, ColorImage)>;
pub use mpsc::channel as capture_channel;

impl CaptureState {
    pub fn new(device: &wgpu::Device, surface_texture: &wgpu::Texture) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("texture_copy.wgsl"));

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("texture_copy"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(surface_texture.format().into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let bind_group_layout = pipeline.get_bind_group_layout(0);

        let (texture, padding, bind_group) =
            Self::create_texture(device, surface_texture, &bind_group_layout);

        Self {
            padding,
            texture,
            pipeline,
            bind_group,
        }
    }

    fn create_texture(
        device: &wgpu::Device,
        surface_texture: &wgpu::Texture,
        layout: &BindGroupLayout,
    ) -> (wgpu::Texture, BufferPadding, wgpu::BindGroup) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("egui_screen_capture_texture"),
            size: surface_texture.size(),
            mip_level_count: surface_texture.mip_level_count(),
            sample_count: surface_texture.sample_count(),
            dimension: surface_texture.dimension(),
            format: surface_texture.format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let padding = BufferPadding::new(surface_texture.width(), surface_texture.format());

        let view = texture.create_view(&Default::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
            label: None,
        });

        (texture, padding, bind_group)
    }

    /// Updates the [`CaptureState`] if the size of the surface texture has changed
    pub fn update(&mut self, device: &wgpu::Device, texture: &wgpu::Texture) {
        if self.texture.size() != texture.size() {
            let (new_texture, padding, bind_group) =
                Self::create_texture(device, texture, &self.pipeline.get_bind_group_layout(0));
            self.texture = new_texture;
            self.padding = padding;
            self.bind_group = bind_group;
        }
    }

    /// Handles copying from the [`CaptureState`] texture to the surface texture and the buffer.
    /// Pass the returned buffer to [`CaptureState::read_screen_rgba`] to read the data back to the cpu.
    pub fn copy_textures(
        &mut self,
        device: &wgpu::Device,
        output_frame: &wgpu::SurfaceTexture,
        encoder: &mut wgpu::CommandEncoder,
    ) -> wgpu::Buffer {
        debug_assert_eq!(
            self.texture.size(),
            output_frame.texture.size(),
            "Texture sizes must match, `CaptureState::update` was probably not called"
        );

        // It would be more efficient to reuse the Buffer, e.g. via some kind of ring buffer, but
        // for most screenshot use cases this should be fine. When taking many screenshots (e.g. for a video)
        // it might make sense to revisit this and implement a more efficient solution.
        #[allow(clippy::allow_attributes, clippy::arc_with_non_send_sync)] // For wasm
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("egui_screen_capture_buffer"),
            size: (self.padding.padded_bytes_per_row * self.texture.height()) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let padding = self.padding;
        let tex = &mut self.texture;

        let tex_extent = tex.size();

        encoder.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padding.padded_bytes_per_row),
                    rows_per_image: None,
                },
            },
            tex_extent,
        );

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("texture_copy"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &output_frame.texture.create_view(&Default::default()),
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);

        buffer
    }

    /// Handles copying from the [`CaptureState`] texture to the surface texture and the cpu
    /// This function is non-blocking and will send the data to the given sender when it's ready.
    /// Pass in the buffer returned from [`CaptureState::copy_textures`].
    /// Make sure to call this after the encoder has been submitted.
    pub fn read_screen_rgba(
        &self,
        ctx: egui::Context,
        buffer: wgpu::Buffer,
        data: Vec<UserData>,
        tx: CaptureSender,
        viewport_id: ViewportId,
        output_scale: f32,
        pq_output: bool,
    ) {
        #[allow(clippy::allow_attributes, clippy::arc_with_non_send_sync)] // For wasm
        let buffer = Arc::new(buffer);
        let buffer_clone = Arc::clone(&buffer);
        let buffer_slice = buffer_clone.slice(..);
        let format = self.texture.format();
        let tex_extent = self.texture.size();
        let padding = self.padding;
        let is_float = matches!(format, wgpu::TextureFormat::Rgba16Float);
        let to_rgba = match format {
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba16Float => [0, 1, 2, 3],
            wgpu::TextureFormat::Bgra8Unorm => [2, 1, 0, 3],
            _ => {
                log::error!(
                    "Screen can't be captured unless the surface format is Rgba8Unorm, Bgra8Unorm, or Rgba16Float. Current surface format is {format:?}"
                );
                return;
            }
        };
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            if let Err(err) = result {
                log::error!("Failed to map buffer for reading: {err}");
                return;
            }
            let buffer_slice = buffer.slice(..);

            let mut pixels = Vec::with_capacity((tex_extent.width * tex_extent.height) as usize);
            let mapped_range = match buffer_slice.get_mapped_range() {
                Ok(range) => range,
                Err(err) => {
                    log::error!("Failed to get mapped range for reading: {err}");
                    return;
                }
            };
            for padded_row in mapped_range.chunks(padding.padded_bytes_per_row as usize) {
                let row = &padded_row[..padding.unpadded_bytes_per_row as usize];
                if is_float {
                    // Rgba16Float holds extended linear sRGB: encode to gamma sRGB bytes (HDR
                    // highlights above SDR white clip)
                    for color in row.chunks(8) {
                        let c = |i: usize| f16_to_f32(u16::from_le_bytes([color[i], color[i + 1]]));
                        // Patch (HDR display output): a BT.2100 PQ surface holds absolute cd/m² in
                        // BT.2020: decode, back to sRGB primaries, SDR white (`output_scale`, cd/m²) = 1
                        let lin = if pq_output {
                            bt2020_to_srgb([c(0), c(2), c(4)].map(pq_to_nits)).map(|v| v / output_scale)
                        } else {
                            [c(0), c(2), c(4)]
                        };
                        let [r, g, b] = lin.map(epaint::ecolor::gamma_u8_from_linear_f32);
                        let a = epaint::ecolor::linear_u8_from_linear_f32(c(6));
                        pixels.push(epaint::Color32::from_rgba_premultiplied(r, g, b, a));
                    }
                } else {
                    for color in row.chunks(4) {
                        pixels.push(epaint::Color32::from_rgba_premultiplied(
                            color[to_rgba[0]],
                            color[to_rgba[1]],
                            color[to_rgba[2]],
                            color[to_rgba[3]],
                        ));
                    }
                }
            }
            drop(mapped_range);
            buffer.unmap();

            tx.send((
                viewport_id,
                data,
                ColorImage::new(
                    [tex_extent.width as usize, tex_extent.height as usize],
                    pixels,
                ),
            ))
            .ok();
            ctx.request_repaint();
        });
    }
}

/// Decode a half float (f16) bit pattern. Subnormals flush to zero and NaN to zero; infinities keep
/// their sign (the byte encoders clamp them).
fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits >> 15 == 1 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 31);
    let mantissa = f32::from(bits & 0x3FF);
    match exp {
        0 => 0.0,
        31 if mantissa == 0.0 => sign * f32::INFINITY,
        31 => 0.0,
        _ => sign * (1.0 + mantissa / 1024.0) * 2f32.powi(exp - 15),
    }
}

#[derive(Copy, Clone)]
struct BufferPadding {
    unpadded_bytes_per_row: u32,
    padded_bytes_per_row: u32,
}

impl BufferPadding {
    fn new(width: u32, format: wgpu::TextureFormat) -> Self {
        let bytes_per_pixel = format.block_copy_size(None).unwrap_or(4);
        let unpadded_bytes_per_row = width * bytes_per_pixel;
        let padded_bytes_per_row =
            wgpu::util::align_to(unpadded_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        Self {
            unpadded_bytes_per_row,
            padded_bytes_per_row,
        }
    }
}

/// Patch (HDR display output): SMPTE ST 2084 EOTF, PQ signal (0..1) to cd/m² (the inverse of the
/// shader's `pq_from_nits`).
fn pq_to_nits(signal: f32) -> f32 {
    const M1: f32 = 0.159_301_76;
    const M2: f32 = 78.84375;
    const C1: f32 = 0.835_937_5;
    const C2: f32 = 18.851_562;
    const C3: f32 = 18.6875;
    if !signal.is_finite() {
        return 0.0;
    }
    let p = signal.clamp(0.0, 1.0).powf(1.0 / M2);
    let y = ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1);
    y * 10_000.0
}

/// Patch (HDR display output): linear BT.2020 to linear sRGB / Rec.709 primaries.
fn bt2020_to_srgb(c: [f32; 3]) -> [f32; 3] {
    [
        1.660_491 * c[0] - 0.587_641 * c[1] - 0.072_850 * c[2],
        -0.124_550 * c[0] + 1.132_900 * c[1] - 0.008_349 * c[2],
        -0.018_151 * c[0] - 0.100_579 * c[1] + 1.118_730 * c[2],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_padding_uses_format_bytes_per_pixel() {
        // u32 (Rgba8Unorm) = 4 bytes per pixel
        let p8 = BufferPadding::new(1280, wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(p8.unpadded_bytes_per_row, 5120);

        // Rgba16Float = 8 bytes per pixel
        let p16 = BufferPadding::new(1280, wgpu::TextureFormat::Rgba16Float);
        assert_eq!(p16.unpadded_bytes_per_row, 10240);
    }

    #[test]
    fn f16_decodes_sign_range_and_specials() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0x3800), 0.5);
        assert_eq!(f16_to_f32(0x4000), 2.0);
        assert_eq!(f16_to_f32(0xB800), -0.5);
        assert_eq!(f16_to_f32(0x7C00), f32::INFINITY);
        assert_eq!(f16_to_f32(0x7E00), 0.0);
        // negative (out-of-gamut scRGB) and above-white values clamp when encoded
        assert_eq!(epaint::ecolor::gamma_u8_from_linear_f32(f16_to_f32(0xB800)), 0);
        assert_eq!(epaint::ecolor::gamma_u8_from_linear_f32(f16_to_f32(0x4000)), 255);
    }

    #[test]
    fn pq_decoding_matches_the_standard() {
        assert_eq!(pq_to_nits(0.0), 0.0);
        assert!((pq_to_nits(1.0) - 10_000.0).abs() < 1.0);
        // 203 cd/m² (SDR reference white) is a PQ signal of about 0.5807
        assert!((pq_to_nits(0.5807) - 203.0).abs() < 1.0, "{}", pq_to_nits(0.5807));
        assert_eq!(pq_to_nits(f32::NAN), 0.0);
        // white stays white across primaries
        let w = bt2020_to_srgb([1.0; 3]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-3), "{w:?}");
    }


    /// The shader's `pq_from_nits` (egui.wgsl), written out here: the decoder must undo it.
    fn pq_from_nits(nits: f32) -> f32 {
        let y = (nits.clamp(0.0, 10_000.0) / 10_000.0).powf(0.159_301_76);
        ((0.835_937_5 + 18.851_562 * y) / (1.0 + 18.6875 * y)).powf(78.84375)
    }

    #[test]
    fn pq_encoding_round_trips_and_the_shader_uses_the_same_constants() {
        assert!((pq_from_nits(203.0) - 0.5807).abs() < 1e-3);
        assert_eq!(pq_to_nits(pq_from_nits(0.0)), 0.0);
        assert!((pq_from_nits(10_000.0) - 1.0).abs() < 1e-5);
        for nits in [0.5, 10.0, 100.0, 203.0, 650.0, 1000.0, 4000.0] {
            let back = pq_to_nits(pq_from_nits(nits));
            assert!((back - nits).abs() / nits < 5e-3, "{nits} -> {back}");
        }
        let shader = include_str!("egui.wgsl");
        for c in ["0.1593017578125", "78.84375", "0.8359375", "18.8515625", "18.6875", "0.627404, 0.329283, 0.043313", "0.069097, 0.919540, 0.011362", "0.016391, 0.088013, 0.895595"] {
            assert!(shader.contains(c), "egui.wgsl lacks {c}");
        }
    }
}
