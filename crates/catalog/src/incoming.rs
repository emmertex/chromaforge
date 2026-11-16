//! Files still being copied into a folder (issue #123): folder scans, Synchronize Folder and
//! adding photos where they are leave them out, and a later scan picks them up once complete.
//!
//! Three signals, cheapest first, none of which reads more than the import does anyway:
//! - the name: hidden files and temporary names (rsync's `.name.XXXXXX`, browsers' `.part`,
//!   `.crdownload`, …) are never photos yet ([`temp_name`]);
//! - a lock check, one open and no reads ([`writer_active`]): on macOS / BSD an open with a
//!   shared lock (`O_SHLOCK | O_NONBLOCK`) fails with `EAGAIN` while a writer has the file open
//!   on a network share (SMB asks the server for deny-write); on Windows an open that refuses to
//!   share writing fails with a sharing violation while a writer has it open. Linux has no such
//!   check (locks there are advisory and copiers take none);
//! - the backstop, from the header the import reads anyway ([`declared_end_past`]): a TIFF-based
//!   raw says where its image data ends (strip / tile offsets plus byte counts); a file shorter
//!   than that is still being written. Only while the file changed in the last [`SETTLE`]: an
//!   older file that is still short is corrupt and reported as unreadable, not skipped forever
//!   ([`recently_modified`]).
//!
//! Sources: own design, from the tested approach in issue #123 (shared-lock open on SMB) and the
//! TIFF 6.0 specification (IFD layout, StripOffsets / TileOffsets, SubIFDs).

use std::path::Path;

/// The error a probe returns for a file that is still being written; callers skip it quietly.
pub const STILL_BEING_WRITTEN: &str = "still being copied";

/// Whether a probe error says the file is still being written ([`STILL_BEING_WRITTEN`]).
pub fn is_still_being_written(error: &str) -> bool {
    error.ends_with(STILL_BEING_WRITTEN)
}

/// Whether `path` looks like a file still being copied, from its name and (where the platform
/// can tell) an open writer. One open at most; nothing is read.
pub fn still_being_written(path: &Path) -> bool {
    temp_name(path) || writer_active(path)
}

/// A hidden or temporary name that copiers and downloaders write under before renaming.
pub fn temp_name(path: &Path) -> bool {
    let Some(name) = path.file_name() else { return false };
    let name = name.to_string_lossy().to_lowercase();
    if name.starts_with('.') || name.starts_with("~$") {
        return true;
    }
    const SUFFIXES: [&str; 8] = [".part", ".partial", ".crdownload", ".download", ".tmp", ".temp", ".filepart", ".!sync"];
    SUFFIXES.iter().any(|s| name.ends_with(s))
}

/// Whether another program has `path` open for writing, where the platform can tell (macOS and
/// the BSDs on network shares, Windows); `false` elsewhere and on any other error.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
pub fn writer_active(path: &Path) -> bool {
    use std::os::unix::fs::OpenOptionsExt;
    // (the same values on Darwin and the BSDs: <fcntl.h> O_SHLOCK 0x10, O_NONBLOCK 0x4;
    // <errno.h> EAGAIN = EWOULDBLOCK = 35)
    const O_SHLOCK: i32 = 0x0010;
    const O_NONBLOCK: i32 = 0x0004;
    const EAGAIN: i32 = 35;
    match std::fs::OpenOptions::new().read(true).custom_flags(O_SHLOCK | O_NONBLOCK).open(path) {
        Ok(_) => false,
        Err(e) => e.raw_os_error() == Some(EAGAIN),
    }
}

/// Whether another program has `path` open for writing (Windows: an open that does not share
/// writing fails with a sharing violation).
#[cfg(windows)]
pub fn writer_active(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_DELETE: u32 = 0x4;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    match std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE).open(path) {
        Ok(_) => false,
        Err(e) => e.raw_os_error() == Some(ERROR_SHARING_VIOLATION),
    }
}

/// No lock check on this platform (Linux, Android, the web): the header backstop decides.
#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
pub fn writer_active(_path: &Path) -> bool {
    false
}

/// How long after its last change a short file is still taken for one being copied in. Copies stall
/// on slow links for a while; ten minutes of no change means the copy is over.
pub const SETTLE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Whether `path` changed within [`SETTLE`] (or claims a time in the future, as a NAS with a
/// skewed clock can). `false` when the filesystem gives no time, so a short file is reported.
pub fn recently_modified(path: &Path) -> bool {
    let Ok(t) = std::fs::metadata(path).and_then(|m| m.modified()) else { return false };
    std::time::SystemTime::now().duration_since(t).map_or(true, |age| age < SETTLE)
}

/// The furthest a classic TIFF's 32-bit offsets and counts can reach: a declared end past it is impossible, not a
/// file still being written.
const MAX_TIFF_END: u64 = 1 << 32;

/// IFDs followed at most (IFD chains plus SubIFDs): raws carry a handful.
const MAX_IFDS: usize = 64;
/// Entries read per IFD at most.
const MAX_ENTRIES: usize = 512;
/// Offsets / counts read per strip or tile tag at most.
const MAX_VALUES: usize = 1 << 16;

/// Whether `head` (the start of a file, or all of it) is a TIFF-based file whose image data, by
/// its own strip / tile offsets and byte counts, ends past `file_len`: a file still being
/// written. `false` for anything else, including headers that can't be read (never a panic).
pub fn declared_end_past(head: &[u8], file_len: u64) -> bool {
    declared_end(head).is_some_and(|end| end > file_len)
}

/// The end of the image data a TIFF-based header declares (the largest offset + byte count of its
/// strips, tiles and JPEG thumbnail, over IFD0's chain and its SubIFDs).
pub fn declared_end(head: &[u8]) -> Option<u64> {
    let le = match head.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let r = Reader { b: head, le };
    // 42 for TIFF, plus the magic numbers of TIFF-based raws (ORF, RW2)
    let magic = r.u16(2)?;
    if !matches!(magic, 42 | 0x4f52 | 0x5352 | 0x55) {
        return None;
    }
    // a Panasonic RW2 (magic 0x55) can give its strip offset as 0xFFFFFFFF and the real start of the raw data in its own
    // tag 0x0118 (RawDataOffset); in a plain TIFF 0x0118 is MinSampleValue, so it is only read for RW2
    let panasonic = magic == 0x55;
    let mut queue = vec![u64::from(r.u32(4)?)];
    let mut seen = std::collections::HashSet::new();
    let mut end: Option<u64> = None;
    while let Some(ifd) = queue.pop() {
        if ifd == 0 || seen.len() >= MAX_IFDS || !seen.insert(ifd) {
            continue;
        }
        let Ok(at) = usize::try_from(ifd) else { continue };
        let Some(n) = r.u16(at) else { continue };
        let mut tags: [(Vec<u64>, Vec<u64>); 3] = Default::default(); // strips, tiles, JPEG
        let mut raw_data_offset = None;
        for i in 0..usize::from(n).min(MAX_ENTRIES) {
            let Some(e) = i.checked_mul(12).and_then(|o| o.checked_add(at)).and_then(|o| o.checked_add(2)) else { break };
            let (Some(tag), Some(vals)) = (r.u16(e), r.values(e)) else { continue };
            match tag {
                0x0111 => tags[0].0 = vals,
                0x0117 => tags[0].1 = vals,
                0x0144 => tags[1].0 = vals,
                0x0145 => tags[1].1 = vals,
                0x0201 => tags[2].0 = vals,
                0x0202 => tags[2].1 = vals,
                0x014a => queue.extend(vals),
                0x0118 if panasonic => raw_data_offset = vals.first().copied(),
                _ => {}
            }
        }
        for (offsets, counts) in &tags {
            for (o, c) in offsets.iter().zip(counts) {
                // the placeholder offset says where nothing is: RW2's own offset, else no end from this strip
                let o = match (*o == u64::from(u32::MAX), raw_data_offset) {
                    (false, _) => *o,
                    (true, Some(raw)) => raw,
                    (true, None) => continue,
                };
                // a classic TIFF addresses at most 4 GiB with its 32-bit offsets: an end past that is no end at all
                if let Some(e) = o.checked_add(*c).filter(|e| *e <= MAX_TIFF_END) {
                    end = Some(end.map_or(e, |m| m.max(e)));
                }
            }
        }
        let next = usize::from(n).checked_mul(12).and_then(|x| x.checked_add(at)).and_then(|x| x.checked_add(2));
        if let Some(next) = next.and_then(|x| r.u32(x)) {
            queue.push(u64::from(next));
        }
    }
    end
}

struct Reader<'a> {
    b: &'a [u8],
    le: bool,
}

impl Reader<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let s: [u8; 2] = self.b.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if self.le { u16::from_le_bytes(s) } else { u16::from_be_bytes(s) })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let s: [u8; 4] = self.b.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if self.le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
    }

    /// The SHORT / LONG (or IFD) values of the entry at `e`; `None` for other types.
    fn values(&self, e: usize) -> Option<Vec<u64>> {
        let ty = self.u16(e.checked_add(2)?)?;
        let count = usize::try_from(self.u32(e.checked_add(4)?)?).ok()?.min(MAX_VALUES);
        let size = match ty {
            3 => 2,
            4 | 13 => 4,
            _ => return None,
        };
        let inline = count.checked_mul(size)? <= 4;
        let base = if inline { e.checked_add(8)? } else { usize::try_from(self.u32(e.checked_add(8)?)?).ok()? };
        (0..count)
            .map(|i| {
                let at = i.checked_mul(size)?.checked_add(base)?;
                if size == 2 { self.u16(at).map(u64::from) } else { self.u32(at).map(u64::from) }
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "tests_incoming.rs"]
mod tests;
