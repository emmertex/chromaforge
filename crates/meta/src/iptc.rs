#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects)]
//! IPTC-IIM (Information Interchange Model 4.2) — the record-2 datasets people still carry in JPEGs.
//!
//! Stream of `0x1C record dataset size(u16 BE, or extended) data` tags. Text is UTF-8 when the record-1
//! `CodedCharacterSet` (1:90) is `ESC % G`, otherwise decoded as UTF-8 when valid and Latin-1 otherwise.

use crate::{DateTime, Metadata};

fn decode(b: &[u8], utf8: bool) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.trim_end_matches('\0').trim().to_string(),
        Err(_) if utf8 => String::from_utf8_lossy(b).trim_end_matches('\0').trim().to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect::<String>().trim_end_matches('\0').trim().to_string(),
    }
}

/// IPTC `Date Created` (+ optional `Time Created`) as a capture time. The whole time must be valid: blank or
/// absent keeps the date alone, six digits are a local clock, eleven characters `HHMMSS+HHMM` carry an offset;
/// anything else (partial offset, extra digits, trailing text) gives `None`, so it cannot block a valid XMP
/// or file-date fallback.
fn parse_created(date: Option<&str>, time: Option<&str>) -> Option<DateTime> {
    let date = date.filter(|value| value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()))?;
    let time = time.filter(|value| !value.is_empty());
    if let Some(time) = time {
        let bytes = time.as_bytes();
        let clock = bytes.get(..6)?;
        if !clock.iter().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        match bytes.len() {
            6 => {}
            11 => {
                let offset = bytes.get(6..11)?;
                if !matches!(offset.first().copied(), Some(b'+' | b'-')) || !offset.get(1..)?.iter().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
            }
            _ => return None,
        }
    }

    let mut iso = format!("{}-{}-{}", date.get(..4)?, date.get(4..6)?, date.get(6..8)?);
    if let Some(time) = time {
        iso.push('T');
        iso.push_str(time.get(..2)?);
        iso.push(':');
        iso.push_str(time.get(2..4)?);
        iso.push(':');
        iso.push_str(time.get(4..6)?);
        if time.len() == 11 {
            let offset = time.get(6..)?;
            iso.push_str(offset.get(..3)?);
            iso.push(':');
            iso.push_str(offset.get(3..)?);
        }
    }
    DateTime::parse_iso(&iso)
}

/// Parse an IIM stream into the fields [`Metadata`] carries (title, keywords, artist, copyright, caption,
/// subject codes, creation date/time).
pub fn parse_iptc(b: &[u8]) -> Metadata {
    let mut m = Metadata::default();
    let mut utf8 = false;
    let mut date: Option<String> = None;
    let mut time: Option<String> = None;
    let mut i = 0usize;
    while let Some(&[marker, rec, ds, s0, s1]) = i.checked_add(5).and_then(|e| b.get(i..e)) {
        if marker != 0x1c {
            break;
        }
        let mut size = u16::from_be_bytes([s0, s1]) as usize;
        let Some(mut at) = i.checked_add(5) else { break };
        if size & 0x8000 != 0 {
            // extended dataset: next (size & 0x7fff) bytes hold the length
            let n = size & 0x7fff;
            let Some(len) = at.checked_add(n).and_then(|e| b.get(at..e)).filter(|l| (1..=4).contains(&l.len())) else { break };
            size = len.iter().fold(0usize, |a, &c| (a << 8) | c as usize);
            at = at.saturating_add(n);
        }
        let Some(data) = b.get(at..at.saturating_add(size)) else { break };
        match (rec, ds) {
            (1, 90) => utf8 = data == b"\x1b%G",
            (2, 5) => m.title = Some(decode(data, utf8)).filter(|s| !s.is_empty()),
            // `IPR:number:name:matter:detail`: the subject code is the reference number
            (2, 12) => {
                let number = decode(data, utf8);
                let number = number.split(':').nth(1).unwrap_or("").trim();
                if !number.is_empty() && !m.subject_code.iter().any(|c| c == number) {
                    m.subject_code.push(number.to_string());
                }
            }
            (2, 15) => m.category = Some(decode(data, utf8)).filter(|s| !s.is_empty()),
            (2, 20) => {
                let c = decode(data, utf8);
                if !c.is_empty() && !m.supplemental_categories.contains(&c) {
                    m.supplemental_categories.push(c);
                }
            }
            (2, 25) => {
                let k = decode(data, utf8);
                if !k.is_empty() && !m.keywords.contains(&k) {
                    m.keywords.push(k);
                }
            }
            (2, 105) => m.headline = Some(decode(data, utf8)).filter(|s| !s.is_empty()),
            (2, 80) => m.artist = Some(decode(data, utf8)).filter(|s| !s.is_empty()),
            (2, 116) => m.copyright = Some(decode(data, utf8)).filter(|s| !s.is_empty()),
            (2, 120) => m.caption = Some(decode(data, utf8)).filter(|s| !s.is_empty()),
            (2, 122) => m.description_writer = Some(decode(data, utf8)).filter(|s| !s.is_empty()),
            (2, 55) => date = Some(decode(data, utf8)),
            (2, 60) => time = Some(decode(data, utf8)),
            _ => {}
        }
        i = at.saturating_add(size);
    }
    m.capture_time = parse_created(date.as_deref(), time.as_deref());
    m
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn ds(out: &mut Vec<u8>, rec: u8, ds: u8, data: &[u8]) {
        out.extend_from_slice(&[0x1c, rec, ds]);
        out.extend_from_slice(&(data.len() as u16).to_be_bytes());
        out.extend_from_slice(data);
    }

    pub(crate) fn sample_iim() -> Vec<u8> {
        let mut v = Vec::new();
        ds(&mut v, 1, 90, b"\x1b%G");
        ds(&mut v, 2, 0, &[0, 4]);
        ds(&mut v, 2, 5, "Tïtle".as_bytes());
        ds(&mut v, 2, 15, b"CAT");
        ds(&mut v, 2, 20, b"Sports");
        ds(&mut v, 2, 25, b"alpha");
        ds(&mut v, 2, 25, b"beta");
        ds(&mut v, 2, 25, b"alpha");
        ds(&mut v, 2, 80, b"Photographer");
        ds(&mut v, 2, 116, b"CC0");
        ds(&mut v, 2, 105, b"A headline");
        ds(&mut v, 2, 120, b"A caption");
        ds(&mut v, 2, 122, b"Jane Doe");
        ds(&mut v, 2, 55, b"20210304");
        ds(&mut v, 2, 60, b"101112+0100");
        v
    }

    #[test]
    fn parses_record2() {
        let m = parse_iptc(&sample_iim());
        assert_eq!(m.title.as_deref(), Some("Tïtle"));
        assert_eq!(m.category.as_deref(), Some("CAT"));
        assert_eq!(m.supplemental_categories, vec!["Sports".to_string()]);
        assert_eq!(m.headline.as_deref(), Some("A headline"));
        assert_eq!(m.description_writer.as_deref(), Some("Jane Doe"));
        assert_eq!(m.keywords, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(m.artist.as_deref(), Some("Photographer"));
        assert_eq!(m.copyright.as_deref(), Some("CC0"));
        assert_eq!(m.caption.as_deref(), Some("A caption"));
        assert_eq!(m.capture_time.unwrap().to_iso(), "2021-03-04T10:11:12+01:00");
    }

    fn capture_time(time: Option<&[u8]>) -> Metadata {
        let mut iim = Vec::new();
        ds(&mut iim, 2, 55, b"20260101");
        if let Some(time) = time {
            ds(&mut iim, 2, 60, time);
        }
        ds(&mut iim, 2, 5, b"kept title");
        ds(&mut iim, 2, 25, b"kept keyword");
        parse_iptc(&iim)
    }

    #[test]
    fn iptc_time_created_rejects_incomplete_or_trailing_text() {
        for time in
            [&b"121314+01"[..], b"121314junk", b"12131499", b"121314+0100junk", b"121314+", b"121314+010", b"121314+01000", b"12345", b"121314+1801"]
        {
            let metadata = capture_time(Some(time));
            assert_eq!(metadata.capture_time, None, "accepted malformed Time Created {time:?}");
            assert_eq!(metadata.title.as_deref(), Some("kept title"), "unrelated metadata was lost for {time:?}");
            assert_eq!(metadata.keywords, ["kept keyword"], "unrelated metadata was lost for {time:?}");
        }
    }

    #[test]
    fn iptc_time_created_preserves_supported_forms() {
        let cases: &[(Option<&[u8]>, Option<&str>)] = &[
            (None, Some("2026-01-01T00:00:00")),
            (Some(b""), Some("2026-01-01T00:00:00")),
            (Some(b" 	"), Some("2026-01-01T00:00:00")),
            (Some(b"121314"), Some("2026-01-01T12:13:14")),
            (Some(b"121314+0100"), Some("2026-01-01T12:13:14+01:00")),
            (Some(b"121314-0530"), Some("2026-01-01T12:13:14-05:30")),
            (Some(b"121314+1800"), Some("2026-01-01T12:13:14+18:00")),
            (Some(b" 121314+0000 \0"), Some("2026-01-01T12:13:14+00:00")),
        ];
        for (time, expected) in cases {
            let metadata = capture_time(*time);
            assert_eq!(metadata.capture_time.map(|value| value.to_iso()).as_deref(), *expected, "Time Created {time:?}");
        }
    }

    #[test]
    fn subject_codes_are_the_reference_numbers() {
        let mut v = Vec::new();
        ds(&mut v, 2, 12, b"IPTC:17000000:weather::");
        ds(&mut v, 2, 12, b"IPTC:06006000:environmental issue:water:");
        ds(&mut v, 2, 12, b"IPTC:17000000:weather::");
        ds(&mut v, 2, 12, b"no reference number");
        ds(&mut v, 2, 12, b"IPTC::weather::");
        assert_eq!(parse_iptc(&v).subject_code, ["17000000", "06006000"]);
    }

    #[test]
    fn non_utf8_text_drops_trailing_nul() {
        let mut v = Vec::new();
        ds(&mut v, 2, 5, &[0x43, 0xe9, 0x00]);
        ds(&mut v, 2, 25, &[0x6b, 0xe9, 0x00]);
        let m = parse_iptc(&v);
        assert_eq!(m.title.as_deref(), Some("Cé"));
        assert_eq!(m.keywords, vec!["ké".to_string()]);
        let mut u = Vec::new();
        ds(&mut u, 1, 90, b"\x1b%G");
        ds(&mut u, 2, 5, &[0x43, 0xff, 0x00]);
        assert_eq!(parse_iptc(&u).title.as_deref(), Some("C\u{fffd}"));
    }

    #[test]
    fn latin1_and_extended_and_garbage() {
        let mut v = Vec::new();
        ds(&mut v, 2, 5, &[0x43, 0xe9]); // "Cé" in Latin-1
        v.extend_from_slice(&[0x1c, 2, 120, 0x80, 0x02, 0x00, 0x03]);
        v.extend_from_slice(b"abc");
        let m = parse_iptc(&v);
        assert_eq!(m.title.as_deref(), Some("Cé"));
        assert_eq!(m.caption.as_deref(), Some("abc"));
        let s = sample_iim();
        for n in 0..s.len() {
            let _ = parse_iptc(&s[..n]);
        }
        assert_eq!(parse_iptc(&[0x1c, 2, 5, 0xff, 0xff]), Metadata::default());
    }
}
