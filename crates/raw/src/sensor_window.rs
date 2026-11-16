//! Where a raw's stored frame sits on its camera's full sensor.
//!
//! A full-frame body shooting a smaller image area (Nikon's 5:4, 1:1, 16:9 and DX crops) stores only that part of the
//! sensor. Anything defined over the whole sensor (a lens profile, a flat field) must then be placed on the stored
//! frame by where the frame sits, and scaled by the full sensor's size, not the frame's. [`sensor_window`] reads that
//! position from the file; only Nikon's maker note records it so far.

use serde::{Deserialize, Serialize};

/// Where a stored frame (a raw's active area) sits on the camera's full sensor, in sensor pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SensorWindow {
    /// The full sensor's width and height.
    pub sensor: [usize; 2],
    /// The stored frame's top-left corner on it.
    pub origin: [usize; 2],
    /// The stored frame's width and height.
    pub size: [usize; 2],
}

impl SensorWindow {
    /// Whether the window is non-empty and lies inside its sensor.
    pub fn is_valid(&self) -> bool {
        let fits = |o: usize, s: usize, full: usize| s > 0 && o.checked_add(s).is_some_and(|e| e <= full);
        fits(self.origin[0], self.size[0], self.sensor[0]) && fits(self.origin[1], self.size[1], self.sensor[1])
    }

    /// Whether the window is the whole sensor (the camera's normal image area).
    pub fn is_full(&self) -> bool {
        self.origin == [0, 0] && self.size == self.sensor
    }

    /// Whether `other` lies within this window (both on the same sensor).
    pub fn contains(&self, other: &SensorWindow) -> bool {
        let within = |a: usize, s: usize, b: usize, t: usize| b >= a && b.saturating_add(t) <= a.saturating_add(s);
        self.sensor == other.sensor
            && within(self.origin[0], self.size[0], other.origin[0], other.size[0])
            && within(self.origin[1], self.size[1], other.origin[1], other.size[1])
    }
}

/// Where a raw file's active area sits on its camera's full sensor, read from the file (Nikon's maker note, the only
/// source so far). `active` is the active area's size as [`crate::RawImage::normalized`] returns it (the raw's
/// `active_area`): a window of another size is not returned, so a camera whose stored frame and active area differ
/// never gets a wrong offset.
pub fn sensor_window(bytes: &[u8], active: [usize; 2]) -> Option<SensorWindow> {
    let (sensor, origin, size) = crate::vendor::nef::sensor_window(bytes)?;
    let w = SensorWindow { sensor, origin, size };
    (w.size == active && w.is_valid()).then_some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_know_whether_they_are_whole_or_inside() {
        let full = SensorWindow { sensor: [8288, 5520], origin: [0, 0], size: [8288, 5520] };
        let area = SensorWindow { sensor: [8288, 5520], origin: [688, 0], size: [6912, 5520] };
        assert!(full.is_valid() && full.is_full() && area.is_valid() && !area.is_full());
        assert!(full.contains(&area) && !area.contains(&full));
        let other_body = SensorWindow { sensor: [8280, 5520], origin: [0, 0], size: [8280, 5520] };
        assert!(!full.contains(&other_body));
        for bad in [
            SensorWindow { sensor: [8288, 5520], origin: [1400, 0], size: [6912, 5520] },
            SensorWindow { sensor: [8288, 5520], origin: [0, 0], size: [0, 5520] },
            SensorWindow { sensor: [8288, 5520], origin: [usize::MAX, 0], size: [2, 5520] },
        ] {
            assert!(!bad.is_valid(), "{bad:?}");
        }
        assert_eq!(sensor_window(b"", [1, 1]), None);
    }
}
