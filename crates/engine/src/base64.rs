//! Standard padded base64 for images returned inside JSON replies (`slideshow.preview`, the control
//! channel's `ui.render {inline: true}`), where a file path would name a file on the other machine.

/// Encode `bytes` as standard padded base64 (RFC 4648).
pub fn encode(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        let at = |shift: u32| A.get((n >> shift) as usize & 63).copied().unwrap_or(b'=') as char;
        out.push(at(18));
        out.push(at(12));
        out.push(if chunk.len() > 1 { at(6) } else { '=' });
        out.push(if chunk.len() > 2 { at(0) } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc4648_vectors() {
        for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")]
        {
            assert_eq!(super::encode(i.as_bytes()), o);
        }
        assert_eq!(super::encode(&[0xff, 0xfe, 0xfd]), "//79");
    }
}
