//! Every wgpu instance the app makes comes from [`new_instance`].
//!
//! The Vulkan loader (`vulkan-1.dll` on Windows) crashed with STATUS_ACCESS_VIOLATION while three
//! window-renderer tests each made an instance and a device on their own thread (Windows 11, loader
//! 1.4.303, NVIDIA and AMD drivers, OBS's capture layer installed; about 2 % of runs under load).
//! Every crash was at the same loader address, reading a pointer of -1 or 2, inside wgpu naming a
//! new Vulkan object (`vkSetDebugUtilsObjectNameEXT`, from command buffer, buffer and bind group
//! creation): the loader finds the device for a name by walking the devices of every instance in the
//! process, and a device made or dropped on another thread meanwhile is read after it is freed.
//! The app makes instances and devices on several threads too: the window's renderer probe, the
//! editing device and its helper, the AI models.
//!
//! So Vulkan instances name nothing unless `WGPU_DEBUG` asks for names, instances are made one at a
//! time, and the first one that asks for Vulkan also makes a Vulkan instance that lives as long as
//! the process (the loader, its drivers and its layers then stay loaded instead of loading and
//! unloading with the instances that come and go).

use std::sync::{Mutex, OnceLock, PoisonError};

/// A wgpu instance for `desc`, made while no other thread makes one, without Vulkan debug names
/// unless `WGPU_DEBUG` asks for them, and with the Vulkan loader kept loaded when `desc` asks for
/// Vulkan. Use this instead of `wgpu::Instance::new` (clippy enforces it).
pub fn new_instance(mut desc: wgpu::InstanceDescriptor) -> wgpu::Instance {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    if desc.backends.contains(wgpu::Backends::VULKAN) {
        desc.flags = without_vulkan_debug_names(desc.flags, std::env::var("WGPU_DEBUG").ok().as_deref());
        keep_vulkan_loaded();
    }
    #[allow(clippy::disallowed_methods)] // the one place instances are made
    wgpu::Instance::new(desc)
}

/// `flags` without wgpu's `DEBUG` (on in debug builds) unless `WGPU_DEBUG` asks for it (`wgpu_debug`,
/// any value but `0`, as wgpu reads it). With `DEBUG`, wgpu names every Vulkan object it makes
/// (`vkSetDebugUtilsObjectNameEXT`), and the loader handles each name by walking the devices of every
/// instance in the process; a device made or dropped on another thread meanwhile made that walk read
/// freed memory (the crash: stacks from buffer, bind group and command buffer creation, each at the
/// loader's same address).
fn without_vulkan_debug_names(mut flags: wgpu::InstanceFlags, wgpu_debug: Option<&str>) -> wgpu::InstanceFlags {
    if wgpu_debug.is_none_or(|v| v.trim() == "0") {
        flags.remove(wgpu::InstanceFlags::DEBUG);
    }
    flags
}

/// Make, once, the Vulkan instance that keeps the loader, its drivers and layers loaded until the
/// process ends. Where there is no Vulkan it holds no backend and costs nothing.
fn keep_vulkan_loaded() {
    static KEEP: OnceLock<wgpu::Instance> = OnceLock::new();
    KEEP.get_or_init(|| {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::VULKAN;
        desc.flags = without_vulkan_debug_names(crate::backend::instance_flags_from_env(desc.flags), std::env::var("WGPU_DEBUG").ok().as_deref());
        #[allow(clippy::disallowed_methods)] // see `new_instance`
        wgpu::Instance::new(desc)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vulkan instances name nothing unless `WGPU_DEBUG` asks for names; other flags stay.
    #[test]
    fn vulkan_instances_name_objects_only_when_wgpu_debug_asks() {
        let both = wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION;
        assert_eq!(without_vulkan_debug_names(both, None), wgpu::InstanceFlags::VALIDATION);
        assert_eq!(without_vulkan_debug_names(both, Some("0")), wgpu::InstanceFlags::VALIDATION);
        assert_eq!(without_vulkan_debug_names(both, Some("1")), both);
        assert_eq!(without_vulkan_debug_names(wgpu::InstanceFlags::empty(), Some("1")), wgpu::InstanceFlags::empty());
    }

    /// Threads that make and drop instances and devices at once, as the window probe, the editing
    /// device and the AI models can, finish without crashing the process (the Vulkan loader did,
    /// inside `vulkan-1.dll`, before instances were made one at a time with the loader kept loaded).
    #[test]
    fn instances_made_and_dropped_on_many_threads_at_once_are_safe() {
        let backends = wgpu::Backends::VULKAN | wgpu::Backends::DX12 | wgpu::Backends::METAL;
        let probe = new_instance(wgpu::InstanceDescriptor { backends, ..wgpu::InstanceDescriptor::new_without_display_handle() });
        if pollster::block_on(probe.enumerate_adapters(backends)).is_empty() {
            lcc_testkit::skip!(Gpu, "no GPU adapter");
            return;
        }
        let threads: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(move || {
                    for _ in 0..3 {
                        let instance = new_instance(wgpu::InstanceDescriptor { backends, ..wgpu::InstanceDescriptor::new_without_display_handle() });
                        if let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) {
                            drop(pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())));
                        }
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
    }
}
