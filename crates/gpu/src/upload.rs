//! Buffers made with their contents, without wgpu's panic (issue #1029).
//!
//! `wgpu::util::DeviceExt::create_buffer_init` maps the new buffer at creation and panics when the
//! buffer is invalid ("Failed to get mapped range … Buffer with '' label is invalid"): that happens
//! when the device runs out of memory or is lost (seen on a Qualcomm Adreno X1-45 under Vulkan).
//! [`buffer_init`] queues the contents with `Queue::write_buffer` instead, which reports an invalid
//! buffer as a device error: the render's error scopes or the device's error handler see it and the
//! render fails over to the CPU, and nothing panics.

/// A new buffer holding `contents` (padded with zeros to 4 bytes, at least 4 bytes long). The write
/// lands before the next submit's work, as `create_buffer_init`'s did; `COPY_DST` is added to `usage`.
pub(crate) fn buffer_init(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: Option<&str>,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let padded = padded(contents);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label,
        size: padded.len() as u64,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, &padded);
    buffer
}

/// `contents` grown with zeros to a non-zero multiple of `wgpu::COPY_BUFFER_ALIGNMENT`.
fn padded(contents: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let align = wgpu::COPY_BUFFER_ALIGNMENT as usize;
    let len = contents.len().max(1).div_ceil(align).saturating_mul(align);
    if len == contents.len() {
        return std::borrow::Cow::Borrowed(contents);
    }
    let mut v = contents.to_vec();
    v.resize(len, 0);
    std::borrow::Cow::Owned(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes need a non-zero length that is a multiple of 4; the contents are kept in front.
    #[test]
    fn padding_is_aligned_and_keeps_the_contents() {
        for n in 0..12usize {
            let data: Vec<u8> = (1..=n as u8).collect();
            let p = padded(&data);
            assert!(!p.is_empty() && p.len().is_multiple_of(4), "{n} bytes → {}", p.len());
            assert_eq!(&p[..n], &data[..]);
            assert!(p[n..].iter().all(|b| *b == 0));
        }
    }
}
