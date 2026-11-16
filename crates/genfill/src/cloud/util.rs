//! Small encoders the provider requests need: base64, multipart/form-data, PNG.

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let idx = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (k, i) in idx.iter().enumerate() {
            if k <= chunk.len() {
                out.push(B64.get(*i as usize).copied().map_or('A', char::from));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decode standard (or URL-safe) base64; whitespace is skipped. `None` for anything else.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The bytes of a `data:<mime>;base64,<…>` URI.
pub fn data_uri_bytes(uri: &str) -> Option<Vec<u8>> {
    let rest = uri.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    if !meta.ends_with(";base64") {
        return None;
    }
    b64_decode(data)
}

/// A multipart/form-data body.
pub struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Multipart {
    pub fn new(seed: u64) -> Multipart {
        Multipart { boundary: format!("----Genfill{seed:016x}"), body: Vec::new() }
    }

    pub fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    pub fn text(&mut self, name: &str, value: &str) -> &mut Self {
        self.body.extend_from_slice(format!("--{}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n", self.boundary).as_bytes());
        self
    }

    pub fn file(&mut self, name: &str, filename: &str, mime: &str, data: &[u8]) -> &mut Self {
        self.body.extend_from_slice(
            format!(
                "--{}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n",
                self.boundary
            )
            .as_bytes(),
        );
        self.body.extend_from_slice(data);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    pub fn finish(mut self) -> (String, Vec<u8>) {
        self.body.extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        (self.content_type(), self.body)
    }
}

/// One part of a parsed multipart body (the mock server reads requests with it).
#[derive(Debug, Clone)]
pub struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub data: Vec<u8>,
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

/// Parse a multipart/form-data body with `boundary`.
pub fn parse_multipart(body: &[u8], boundary: &str) -> Vec<Part> {
    let delim = format!("--{boundary}");
    let mut parts = Vec::new();
    let mut pos = match find(body, delim.as_bytes(), 0) {
        Some(p) => p + delim.len(),
        None => return parts,
    };
    while parts.len() < 64 {
        if body.get(pos..pos + 2) == Some(b"--") {
            break;
        }
        let Some(head_end) = find(body, b"\r\n\r\n", pos) else { break };
        let head = String::from_utf8_lossy(body.get(pos..head_end).unwrap_or_default()).into_owned();
        let Some(next) = find(body, format!("\r\n{delim}").as_bytes(), head_end + 4) else { break };
        let data = body.get(head_end + 4..next).unwrap_or_default().to_vec();
        let attr = |key: &str| {
            let k = format!("{key}=\"");
            head.find(&k).and_then(|i| head.get(i + k.len()..)).and_then(|r| r.split('"').next()).map(str::to_string)
        };
        parts.push(Part { name: attr("name").unwrap_or_default(), filename: attr("filename"), data });
        pos = next + 2 + delim.len();
    }
    parts
}

/// PNG of interleaved 8-bit pixels with `channels` (1 grey, 3 RGB, 4 RGBA).
pub fn png_encode(w: usize, h: usize, channels: usize, data: &[u8]) -> Result<Vec<u8>, String> {
    let color = match channels {
        1 => png::ColorType::Grayscale,
        3 => png::ColorType::Rgb,
        4 => png::ColorType::Rgba,
        _ => return Err("unsupported channel count".into()),
    };
    if Some(data.len()) != w.checked_mul(h).and_then(|n| n.checked_mul(channels)) {
        return Err("pixel buffer does not match its size".into());
    }
    let (w32, h32) = (u32::try_from(w).map_err(|_| "too wide")?, u32::try_from(h).map_err(|_| "too tall")?);
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w32, h32);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(data).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Largest image a provider may send back (pixels).
pub const MAX_DECODE_PIXELS: u64 = 40_000_000;

/// Decode a PNG to interleaved 8-bit RGB `(w, h, rgb)`; grey, alpha and 16-bit PNGs are
/// converted. Anything that is not a PNG, or is larger than [`MAX_DECODE_PIXELS`], is an error.
pub fn png_decode_rgb(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    if bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n") {
        let kind = if bytes.get(..3) == Some(b"\xff\xd8\xff") { "a JPEG" } else { "not a PNG" };
        return Err(format!("the provider sent {kind} image; the app asks for PNG"));
    }
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| format!("damaged PNG: {e}"))?;
    let info = reader.info();
    let (w, h) = (info.width as usize, info.height as usize);
    if (w as u64) * (h as u64) > MAX_DECODE_PIXELS || w == 0 || h == 0 {
        return Err(format!("the provider sent a {w}×{h} image"));
    }
    let size = reader.output_buffer_size().ok_or("damaged PNG")?;
    let mut buf = vec![0u8; size];
    let frame = reader.next_frame(&mut buf).map_err(|e| format!("damaged PNG: {e}"))?;
    let data = buf.get(..frame.buffer_size()).unwrap_or_default();
    let ch = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("unexpected palette PNG".into()),
    };
    let rgb: Vec<u8> = data
        .chunks_exact(ch)
        .flat_map(|p| match ch {
            1 | 2 => {
                let v = p.first().copied().unwrap_or(0);
                [v, v, v]
            }
            _ => [p.first().copied().unwrap_or(0), p.get(1).copied().unwrap_or(0), p.get(2).copied().unwrap_or(0)],
        })
        .collect();
    if rgb.len() != w * h * 3 {
        return Err("damaged PNG".into());
    }
    Ok((w, h, rgb))
}

/// Decode a PNG mask (any colour type): `(w, h, v)` with the first channel (or luminance) per pixel,
/// and for RGBA its alpha (`Some`).
pub fn png_decode_mask(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>, Option<Vec<u8>>), String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| format!("damaged PNG: {e}"))?;
    let (w, h) = (reader.info().width as usize, reader.info().height as usize);
    if (w as u64) * (h as u64) > MAX_DECODE_PIXELS {
        return Err("mask too large".into());
    }
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or("damaged PNG")?];
    let frame = reader.next_frame(&mut buf).map_err(|e| format!("damaged PNG: {e}"))?;
    let ch = frame.color_type.samples();
    let data = buf.get(..frame.buffer_size()).unwrap_or_default();
    let v: Vec<u8> = data.chunks_exact(ch).map(|p| p.first().copied().unwrap_or(0)).collect();
    let alpha = (ch == 4 || ch == 2).then(|| data.chunks_exact(ch).map(|p| p.last().copied().unwrap_or(255)).collect());
    Ok((w, h, v, alpha))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for s in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0u8, 255, 128, 7][..]] {
            assert_eq!(b64_decode(&b64_encode(s)).unwrap(), s);
        }
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert!(b64_decode("a*b").is_none());
        assert_eq!(data_uri_bytes("data:image/png;base64,Zm9v").unwrap(), b"foo");
        assert!(data_uri_bytes("https://x/y.png").is_none());
    }

    #[test]
    fn multipart_round_trips() {
        let mut m = Multipart::new(42);
        m.text("prompt", "a lawn").file("image", "image.png", "image/png", &[0, 1, 2, 13, 10, 45, 45]);
        let (ct, body) = m.finish();
        let boundary = ct.split("boundary=").nth(1).unwrap();
        let parts = parse_multipart(&body, boundary);
        assert_eq!(parts.len(), 2);
        assert_eq!((parts[0].name.as_str(), parts[0].data.as_slice()), ("prompt", &b"a lawn"[..]));
        assert_eq!((parts[1].filename.as_deref(), parts[1].data.as_slice()), (Some("image.png"), &[0u8, 1, 2, 13, 10, 45, 45][..]));
        assert!(parse_multipart(b"garbage", "x").is_empty());
    }

    #[test]
    fn png_round_trips_and_refuses_other_files() {
        let rgb: Vec<u8> = (0..4 * 3 * 3).map(|i| i as u8 * 7).collect();
        let p = png_encode(4, 3, 3, &rgb).unwrap();
        assert_eq!(png_decode_rgb(&p).unwrap(), (4, 3, rgb));
        let grey = png_encode(2, 2, 1, &[0, 255, 10, 20]).unwrap();
        assert_eq!(png_decode_rgb(&grey).unwrap().2, vec![0, 0, 0, 255, 255, 255, 10, 10, 10, 20, 20, 20]);
        assert!(png_decode_rgb(b"\xff\xd8\xff\xe0 jpeg").unwrap_err().contains("JPEG"));
        assert!(png_decode_rgb(&p[..p.len() / 2]).is_err());
        assert!(png_encode(2, 2, 3, &[0; 5]).is_err());
        let rgba = png_encode(1, 2, 4, &[1, 2, 3, 0, 4, 5, 6, 255]).unwrap();
        assert_eq!(png_decode_mask(&rgba).unwrap(), (1, 2, vec![1, 4], Some(vec![0, 255])));
    }
}
