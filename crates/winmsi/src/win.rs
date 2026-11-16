//! The msi.dll calls. Every pointer handed to Windows points into a buffer this module owns for the
//! whole call.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use windows_sys::Win32::System::ApplicationInstallationAndServicing::{
    INSTALLSTATE, INSTALLSTATE_ABSENT, INSTALLSTATE_ADVERTISED, INSTALLSTATE_LOCAL, INSTALLSTATE_MOREDATA, INSTALLSTATE_NOTUSED, INSTALLSTATE_SOURCE,
    INSTALLSTATE_UNKNOWN, MsiEnumClientsW, MsiGetComponentPathW,
};

use crate::{Error, MAX_PRODUCTS, Result};

/// The first path buffer, in UTF-16 units; a longer path is asked for again at its length.
const FIRST: u32 = 520;
/// The longest path accepted (Windows' extended-length limit).
const LONGEST: u32 = 32_768;
/// A product code: a braced GUID (38 units) and its NUL.
const PRODUCT_CODE: usize = 39;

const ERROR_SUCCESS: u32 = 0;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const ERROR_UNKNOWN_COMPONENT: u32 = 1607;

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// The text before the first NUL of `buf`, at most `len` units.
fn text(buf: &[u16], len: usize) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len()).min(len);
    String::from_utf16_lossy(buf.get(..end).unwrap_or(&[]))
}

pub fn component_paths(component: &str) -> Result<Vec<PathBuf>> {
    let code = wide(component);
    let mut paths = Vec::new();
    for index in 0..MAX_PRODUCTS {
        let mut product = [0u16; PRODUCT_CODE];
        // SAFETY: `code` is NUL-terminated and lives for the call; `product` holds the 39 units the
        // documentation requires for a product code.
        let status = unsafe { MsiEnumClientsW(code.as_ptr(), index, product.as_mut_ptr()) };
        match status {
            ERROR_SUCCESS => {}
            ERROR_NO_MORE_ITEMS | ERROR_UNKNOWN_COMPONENT => return Ok(paths),
            other => return Err(Error::System(format!("listing the products that have the component failed (error {other})"))),
        }
        let product = wide(&text(&product, PRODUCT_CODE));
        if let Some(path) = product_path(&product, &code)? {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Where `product` (a NUL-terminated product code) has `component`'s key file, when it is installed.
fn product_path(product: &[u16], component: &[u16]) -> Result<Option<PathBuf>> {
    let mut size = FIRST;
    // at most twice: the second call has the length the first one reported
    for _ in 0..2 {
        let mut buf = vec![0u16; size as usize];
        let mut len = size;
        // SAFETY: `product` and `component` are NUL-terminated and live for the call; `buf` holds
        // `len` units, as `len` says, and Windows writes at most that many (MOREDATA otherwise).
        let state: INSTALLSTATE = unsafe { MsiGetComponentPathW(product.as_ptr(), component.as_ptr(), buf.as_mut_ptr(), &mut len) };
        match state {
            INSTALLSTATE_LOCAL | INSTALLSTATE_SOURCE => {
                let path = text(&buf, len as usize);
                return Ok((!path.is_empty()).then(|| PathBuf::from(path)));
            }
            INSTALLSTATE_MOREDATA => {
                // `len` is the path's length without its NUL
                size = len.saturating_add(1);
                if size > LONGEST {
                    return Err(Error::System(format!("the component's path is {len} characters long")));
                }
            }
            INSTALLSTATE_UNKNOWN | INSTALLSTATE_ABSENT | INSTALLSTATE_ADVERTISED | INSTALLSTATE_NOTUSED => return Ok(None),
            other => return Err(Error::System(format!("the component's install state is {other}"))),
        }
    }
    Err(Error::System("the component's path kept growing".into()))
}
