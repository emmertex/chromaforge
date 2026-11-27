//! The calls into the C shim (`shim/shim.cpp`), which calls LibRaw. The only `unsafe` in the crate.
#![allow(unsafe_code)]

use std::ffi::{CStr, c_char, c_void};

use crate::layout::{Facts, Unpacked};

/// `struct lcc_libraw_info` in `shim/shim.cpp`, field for field.
#[repr(C)]
struct Info {
    raw_width: u32,
    raw_height: u32,
    left_margin: u32,
    top_margin: u32,
    width: u32,
    height: u32,
    raw_pitch: u32,
    filters: u32,
    colors: i32,
    is_mosaic: i32,
    fuji_width: u32,
    pattern: [[u8; 48]; 48],
    black: u32,
    cblack: [u32; 6 + 256],
    maximum: u32,
    raw_bps: u32,
    cam_mul: [f32; 4],
    cam_xyz: [[f32; 3]; 4],
}

unsafe extern "C" {
    fn lcc_libraw_version() -> *const c_char;
    fn lcc_libraw_open(buf: *const u8, len: usize, out: *mut Info, err: *mut c_char, err_len: usize) -> *mut c_void;
    fn lcc_libraw_copy(handle: *mut c_void, dst: *mut u16, dst_len: usize) -> i32;
    fn lcc_libraw_close(handle: *mut c_void);
}

/// An open LibRaw processor, closed when dropped.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from `lcc_libraw_open` (non-null, checked there) and is closed exactly once, here.
        unsafe { lcc_libraw_close(self.0) }
    }
}

pub(crate) fn version() -> String {
    // SAFETY: LibRaw::version() returns a pointer to a static NUL-terminated string (or null, checked).
    let p = unsafe { lcc_libraw_version() };
    if p.is_null() {
        return "LibRaw".into();
    }
    // SAFETY: non-null, static and NUL-terminated (above); only read.
    let v = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    format!("LibRaw {v}")
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Unpacked, String> {
    if bytes.is_empty() {
        return Err("empty file".into());
    }
    let mut info = Info {
        raw_width: 0,
        raw_height: 0,
        left_margin: 0,
        top_margin: 0,
        width: 0,
        height: 0,
        raw_pitch: 0,
        filters: 0,
        colors: 0,
        is_mosaic: 0,
        fuji_width: 0,
        pattern: [[0; 48]; 48],
        black: 0,
        cblack: [0; 6 + 256],
        maximum: 0,
        raw_bps: 0,
        cam_mul: [0.0; 4],
        cam_xyz: [[0.0; 3]; 4],
    };
    let mut err = [0 as c_char; 256];
    // SAFETY: `bytes` is valid for `bytes.len()` reads and outlives the call (LibRaw's buffer datastream reads it
    // during open_buffer and unpack only, both inside this call); `info` and `err` are valid for writes of their
    // sizes, which are passed; the shim writes a NUL-terminated message of at most `err.len()` bytes.
    let handle = unsafe { lcc_libraw_open(bytes.as_ptr(), bytes.len(), &mut info, err.as_mut_ptr(), err.len()) };
    if handle.is_null() {
        // SAFETY: `err` is NUL-terminated by the shim (snprintf), or still all zero.
        let why = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().into_owned();
        return Err(if why.is_empty() { "LibRaw could not read the file".into() } else { format!("LibRaw: {why}") });
    }
    let handle = Handle(handle);
    let facts = Facts {
        raw_width: info.raw_width as usize,
        raw_height: info.raw_height as usize,
        left_margin: info.left_margin as usize,
        top_margin: info.top_margin as usize,
        width: info.width as usize,
        height: info.height as usize,
        filters: info.filters,
        colors: info.colors,
        is_mosaic: info.is_mosaic != 0,
        fuji_width: info.fuji_width as usize,
        pattern: info.pattern,
        black: info.black,
        cblack: info.cblack.to_vec(),
        maximum: info.maximum,
        raw_bps: info.raw_bps,
        cam_mul: info.cam_mul,
        cam_xyz: info.cam_xyz,
    };
    // check everything before the allocation the sizes ask for
    crate::layout::describe(&facts)?;
    let n = facts.samples()?;
    let mut data = Vec::new();
    data.try_reserve_exact(n).map_err(|_| format!("no memory for {n} raw samples"))?;
    data.resize(n, 0u16);
    // SAFETY: `handle.0` is open; `data` is valid for `n` u16 writes and the shim copies at most `n` samples (it
    // checks `raw_width × raw_height` against the length passed).
    let rc = unsafe { lcc_libraw_copy(handle.0, data.as_mut_ptr(), data.len()) };
    drop(handle);
    if rc != 0 {
        return Err(format!("LibRaw could not hand over its raw data (code {rc})"));
    }
    facts.unpacked(data, version())
}
