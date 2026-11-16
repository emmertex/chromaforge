//! How much memory a GPU offers the app, for features that keep textures on it (a zoomed HDR
//! view's recent windows): enough to turn them on, or a reason not to.
//!
//! wgpu 30 has no portable query for it, so this crate goes through wgpu-hal:
//! - **Vulkan** (Linux, BSD, Android, Windows): the size of the adapter's device-local memory
//!   heaps (dedicated memory on a discrete GPU; the share of system memory an integrated one uses).
//! - **DX12** (Windows): the local memory budget DXGI gives the process (`QueryVideoMemoryInfo`).
//! - **Metal** (macOS, iOS): the device's recommended working set
//!   (`recommendedMaxWorkingSetSize`; on Apple silicon, the part of unified memory the GPU gets).
//!
//! The API is safe; on GL, WebGPU or on failure it answers `None` (unknown). Together with
//! `crates/sysmem` and `crates/hdrmeta` it is one of the workspace's few crates allowed `unsafe`
//! (the backend modules, each call a borrow of the adapter's own hal object).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// The memory `adapter`'s GPU offers, in bytes: `None` when its backend can't say (GL, WebGPU)
/// or the query fails.
pub fn adapter_memory(adapter: &wgpu::Adapter) -> Option<u64> {
    let bytes = match adapter.get_info().backend {
        #[cfg(any(windows, all(unix, not(target_vendor = "apple"), not(target_arch = "wasm32"))))]
        wgpu::Backend::Vulkan => vulkan::memory(adapter),
        #[cfg(windows)]
        wgpu::Backend::Dx12 => dx12::memory(adapter),
        #[cfg(target_vendor = "apple")]
        wgpu::Backend::Metal => metal::memory(adapter),
        _ => None,
    };
    bytes.filter(|b| *b > 0)
}

#[cfg(any(windows, all(unix, not(target_vendor = "apple"), not(target_arch = "wasm32"))))]
#[allow(unsafe_code)]
mod vulkan {
    use ash::vk;

    pub fn memory(adapter: &wgpu::Adapter) -> Option<u64> {
        // SAFETY: `as_hal` borrows the adapter's Vulkan object (None for another backend) while
        // `adapter` stays alive for the whole block, as the wgpu docs require. The physical device
        // and instance are that adapter's own, and the query only fills a struct we own.
        let props = unsafe {
            let hal = adapter.as_hal::<wgpu_hal::vulkan::Api>()?;
            hal.shared_instance().raw_instance().get_physical_device_memory_properties(hal.raw_physical_device())
        };
        let local = props.memory_heaps_as_slice().iter().filter(|h| h.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL));
        Some(local.fold(0u64, |sum, h| sum.saturating_add(h.size)))
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod dx12 {
    pub fn memory(adapter: &wgpu::Adapter) -> Option<u64> {
        // SAFETY: `as_hal` borrows the adapter's DX12 object (None for another backend) while
        // `adapter` stays alive for the whole block; `query_video_memory_info` is wgpu-hal's own
        // safe wrapper of `IDXGIAdapter3::QueryVideoMemoryInfo` on that adapter.
        let info = unsafe {
            let hal = adapter.as_hal::<wgpu_hal::dx12::Api>()?;
            hal.raw_adapter().query_video_memory_info(windows::Win32::Graphics::Dxgi::DXGI_MEMORY_SEGMENT_GROUP_LOCAL).ok()?
        };
        Some(info.Budget)
    }
}

#[cfg(target_vendor = "apple")]
#[allow(unsafe_code)]
mod metal {
    use objc2_metal::MTLDevice;

    pub fn memory(adapter: &wgpu::Adapter) -> Option<u64> {
        // SAFETY: `as_hal` borrows the adapter's Metal object (None for another backend) while
        // `adapter` stays alive for the whole block; reading a property of its `MTLDevice` is a
        // plain, thread-safe getter.
        unsafe {
            let hal = adapter.as_hal::<wgpu_hal::metal::Api>()?;
            Some(hal.raw_device().recommendedMaxWorkingSetSize())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Given a real adapter, the backends that can say report a size, and the others don't make
    /// one up. Skipped where there is no GPU.
    #[test]
    fn a_gpu_reports_its_memory_where_the_backend_can_say() {
        let instance = wgpu::Instance::default();
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
            lcc_testkit::skip!(Gpu, "no GPU adapter");
            return;
        };
        let info = adapter.get_info();
        let memory = adapter_memory(&adapter);
        match info.backend {
            wgpu::Backend::Vulkan | wgpu::Backend::Dx12 | wgpu::Backend::Metal => {
                assert!(memory.is_some_and(|b| b >= 64 << 20), "{} on {:?}: {memory:?}", info.name, info.backend);
            }
            _ => assert_eq!(memory, None, "{:?} can't say", info.backend),
        }
    }
}
