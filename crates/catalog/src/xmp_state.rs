//! Metadata vs XMP: is a photo's sidecar in step with the catalog?
//!
//! Whenever the app reads metadata from a photo's XMP or writes it there, it records an
//! [`XmpStamp`] ([`crate::Op::SetXmpStamp`]): the fingerprint of the photo's XMP-relevant state
//! (rating, flag, label, descriptive metadata, develop settings) and the sidecar's modification
//! time and size at that moment. Comparing both with now gives the badge:
//! - **changed in catalog**: the fingerprint moved (edited in the app since, not saved to file);
//! - **changed on disk**: the sidecar's time or size moved (another app wrote it);
//! - **conflict**: both — the "Read Metadata from File / Save to File" dialog asks.
//!
//! An import that puts another catalog's values over what a photo's sidecar said (a Lightroom catalog import) compares
//! the two and, where the sidecar says something else, records the stamp with [`XmpStamp::disagrees`]: the photo
//! starts as a conflict, as Lightroom Classic flags a sidecar that disagrees with its catalog and lets the user choose
//! (issue #1312). Not "changed on disk": that is read automatically, and the import must not take the sidecar's
//! values on its own.

use serde::{Deserialize, Serialize};

use crate::Photo;

/// The state at the last read from / write to the sidecar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XmpStamp {
    /// [`Photo::xmp_fingerprint`] then.
    pub fingerprint: u64,
    /// The sidecar's modification time (seconds since the Unix epoch) and size then; `None` = no
    /// sidecar existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<SidecarStat>,
    /// The sidecar said something other than the library when this was recorded, and neither was chosen: a conflict
    /// until the next read from or write to the sidecar records a new stamp.
    #[serde(default, skip_serializing_if = "is_false")]
    pub disagrees: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// What the file system says about a sidecar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarStat {
    pub mtime: i64,
    pub size: u64,
}

/// The badge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum XmpStatus {
    /// Never read from or written to a sidecar.
    Unknown,
    InSync,
    ChangedInCatalog,
    ChangedOnDisk,
    Conflict,
}

/// FNV-1a over the bytes written to it.
pub(crate) struct Fnv(pub u64);

impl Fnv {
    pub fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl std::io::Write for Fnv {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for b in buf {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Photo {
    /// Fingerprint of what XMP carries: rating, flag, label, descriptive metadata and develop
    /// settings (not the catalog-only fields).
    pub fn xmp_fingerprint(&self) -> u64 {
        let mut h = Fnv::new();
        let r = serde_json::to_writer(&mut h, &(self.rating, self.flag, self.label, &self.meta, &self.develop));
        if r.is_err() { 0 } else { h.0 }
    }

    /// The stamp to record right after reading from / writing to the sidecar (`file`: its stat
    /// after the write, `None` if there is none).
    pub fn xmp_stamp_now(&self, file: Option<SidecarStat>) -> XmpStamp {
        XmpStamp { fingerprint: self.xmp_fingerprint(), file, disagrees: false }
    }

    /// The badge, given the sidecar's stat now (`None`: no sidecar). A virtual copy has no sidecar of its own (the
    /// file next to the original is the original's), so it has none: a write of the original's sidecar is no news
    /// for the copy, and reading that sidecar would put the original's metadata on it.
    pub fn xmp_status(&self, file_now: Option<SidecarStat>) -> XmpStatus {
        if self.copy_of.is_some() {
            return XmpStatus::Unknown;
        }
        let Some(stamp) = self.xmp else { return XmpStatus::Unknown };
        let catalog = stamp.fingerprint != self.xmp_fingerprint();
        if stamp.disagrees {
            return XmpStatus::Conflict;
        }
        let disk = stamp.file != file_now;
        match (catalog, disk) {
            (false, false) => XmpStatus::InSync,
            (true, false) => XmpStatus::ChangedInCatalog,
            (false, true) => XmpStatus::ChangedOnDisk,
            (true, true) => XmpStatus::Conflict,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PhotoId, Source};

    #[test]
    fn badge_follows_both_sides() {
        let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "a.jpg", "JPEG", 10, 10, "2026-01-01T00:00:00");
        assert_eq!(p.xmp_status(None), XmpStatus::Unknown);
        let st = SidecarStat { mtime: 100, size: 2000 };
        p.xmp = Some(p.xmp_stamp_now(Some(st)));
        assert_eq!(p.xmp_status(Some(st)), XmpStatus::InSync);
        assert_eq!(p.xmp_status(Some(SidecarStat { mtime: 101, size: 2000 })), XmpStatus::ChangedOnDisk);
        assert_eq!(p.xmp_status(None), XmpStatus::ChangedOnDisk);
        p.rating = 3;
        assert_eq!(p.xmp_status(Some(st)), XmpStatus::ChangedInCatalog);
        assert_eq!(p.xmp_status(Some(SidecarStat { mtime: 5, size: 1 })), XmpStatus::Conflict);
        // catalog-only fields don't count
        p.rating = 0;
        p.file_size = 99;
        assert_eq!(p.xmp_status(Some(st)), XmpStatus::InSync);
    }

    /// Issue #1312: a stamp recorded while the sidecar and the library disagreed is a conflict whatever happens on
    /// either side, until a read or write records a new one.
    #[test]
    fn a_disagreeing_stamp_is_a_conflict_until_settled() {
        let mut p = Photo::new(PhotoId(3), Source::Demo { scene: 0 }, "a.jpg", "JPEG", 10, 10, "2026-01-01T00:00:00");
        let st = SidecarStat { mtime: 100, size: 2000 };
        p.xmp = Some(XmpStamp { disagrees: true, ..p.xmp_stamp_now(Some(st)) });
        assert_eq!(p.xmp_status(Some(st)), XmpStatus::Conflict);
        assert_eq!(p.xmp_status(Some(SidecarStat { mtime: 101, size: 2000 })), XmpStatus::Conflict);
        p.rating = 2;
        assert_eq!(p.xmp_status(Some(st)), XmpStatus::Conflict);
        p.xmp = Some(p.xmp_stamp_now(Some(st)));
        assert_eq!(p.xmp_status(Some(st)), XmpStatus::InSync);
        // it round-trips, and an ordinary stamp is stored as before
        let json = serde_json::to_string(&XmpStamp { disagrees: true, ..XmpStamp::default() }).unwrap();
        assert!(json.contains("\"disagrees\":true"), "{json}");
        assert!(!serde_json::to_string(&XmpStamp::default()).unwrap().contains("disagrees"));
        assert!(serde_json::from_str::<XmpStamp>(&json).unwrap().disagrees);
    }

    /// A virtual copy made after a sidecar sync carries the original's stamp; the original's sidecar changing (a
    /// write of its own metadata) must not flag the copy as changed on disk, or an automatic read would put the
    /// original's metadata on the copy.
    #[test]
    fn a_virtual_copy_never_reads_the_originals_sidecar_as_its_own() {
        let mut p = Photo::new(PhotoId(2), Source::Demo { scene: 0 }, "a.jpg", "JPEG", 10, 10, "2026-01-01T00:00:00");
        let st = SidecarStat { mtime: 100, size: 2000 };
        p.xmp = Some(p.xmp_stamp_now(Some(st)));
        p.copy_of = Some(PhotoId(1));
        assert_eq!(p.xmp_status(Some(SidecarStat { mtime: 101, size: 2100 })), XmpStatus::Unknown);
    }
}
