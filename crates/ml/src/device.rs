//! Which device the AI jobs run on: the GPU when there is a usable one and the preference allows it, else the CPU,
//! with the reason kept so status displays can say why.

use std::panic::{AssertUnwindSafe, catch_unwind};

/// What the user asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preference {
    /// The GPU when there is a usable one, else the CPU.
    #[default]
    Auto,
    /// Always the CPU (a GPU that misbehaves, or one the user wants to keep free).
    Cpu,
}

impl Preference {
    /// `auto` or `cpu`.
    pub fn parse(name: &str) -> Option<Preference> {
        match name.trim() {
            "auto" | "gpu" => Some(Preference::Auto),
            "cpu" => Some(Preference::Cpu),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Preference::Auto => "auto",
            Preference::Cpu => "cpu",
        }
    }
}

/// The kind of device a job runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Gpu,
    Cpu,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Gpu => "gpu",
            Kind::Cpu => "cpu",
        }
    }
}

/// The device the queue runs its jobs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub kind: Kind,
    /// The adapter's name ("Apple M4 Pro (Metal)"), or "CPU".
    pub name: String,
    /// Why the jobs run on the CPU (no GPU, switched off, the GPU failed), when they do.
    pub why_cpu: Option<String>,
    /// Device ordinal / index when multiple GPUs are present (0-indexed).
    pub index: usize,
}

impl Device {
    pub fn cpu(why: Option<String>) -> Device {
        Device { kind: Kind::Cpu, name: "CPU".into(), why_cpu: why, index: 0 }
    }

    pub fn gpu(name: impl Into<String>) -> Device {
        Device { kind: Kind::Gpu, name: name.into(), why_cpu: None, index: 0 }
    }

    pub fn gpu_with_index(name: impl Into<String>, index: usize) -> Device {
        Device { kind: Kind::Gpu, name: name.into(), why_cpu: None, index }
    }

    pub fn is_gpu(&self) -> bool {
        self.kind == Kind::Gpu
    }
}

/// Finds the GPU: its name, or why there is none to use. It may be slow (it can create a device), so the queue calls
/// it on its worker thread, once, before the first job; it must not panic, but one that does means "no GPU".
pub type Probe = Box<dyn Fn() -> Result<String, String> + Send + Sync>;

/// The device for `preference`, asking `probe` only when the GPU may be used.
pub fn select(preference: Preference, probe: &(dyn Fn() -> Result<String, String> + Send + Sync)) -> Device {
    match preference {
        Preference::Cpu => Device::cpu(Some("the AI device preference is the CPU".into())),
        Preference::Auto => match catch_unwind(AssertUnwindSafe(probe)) {
            Ok(Ok(name)) => {
                if let Some((base, idx_str)) = name.rsplit_once('#')
                    && let Ok(idx) = idx_str.trim().parse::<usize>()
                {
                    return Device::gpu_with_index(base.trim(), idx);
                }
                Device::gpu(name)
            }
            Ok(Err(why)) => Device::cpu(Some(why)),
            Err(_) => Device::cpu(Some("looking for a GPU failed".into())),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_takes_the_gpu_the_probe_finds() {
        let d = select(Preference::Auto, &|| Ok("Test GPU (Vulkan)".into()));
        assert!(d.is_gpu());
        assert_eq!(d.why_cpu, None);
        assert_eq!(d.index, 0);
    }

    #[test]
    fn auto_parses_device_index_from_probe() {
        let d = select(Preference::Auto, &|| Ok("Apple M4 Pro (Metal)#1".into()));
        assert!(d.is_gpu());
        assert_eq!(d.name, "Apple M4 Pro (Metal)");
        assert_eq!(d.index, 1);
    }

    #[test]
    fn no_gpu_falls_back_to_the_cpu_with_the_reason() {
        let d = select(Preference::Auto, &|| Err("no adapter".into()));
        assert_eq!(d.kind, Kind::Cpu);
        assert_eq!(d.why_cpu.as_deref(), Some("no adapter"));
    }

    #[test]
    fn the_cpu_preference_never_probes() {
        let d = select(Preference::Cpu, &|| panic!("probed"));
        assert_eq!(d.kind, Kind::Cpu);
        assert!(d.why_cpu.is_some());
    }

    #[test]
    fn a_probe_that_panics_means_the_cpu() {
        let d = select(Preference::Auto, &|| panic!("driver blew up"));
        assert_eq!(d.kind, Kind::Cpu);
        assert!(d.why_cpu.is_some());
    }

    #[test]
    fn preferences_round_trip_and_reject_junk() {
        for p in [Preference::Auto, Preference::Cpu] {
            assert_eq!(Preference::parse(p.as_str()), Some(p));
        }
        assert_eq!(Preference::parse("tpu"), None);
    }
}
