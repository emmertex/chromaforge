//! `cargo run --release -p lcc-raw --example exifscan -- FILE...` (or file names on stdin with `-`)
//!
//! Reads only the first 1 MB of each file and prints one tab-separated line: path, model, serial, f-number, exposure
//! (s), ISO, capture time. For picking photos out of a large archive (on a network drive) without reading them whole.

use std::io::{BufRead, Read};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-") {
        args = std::io::stdin().lock().lines().map_while(Result::ok).collect();
    }
    for path in args {
        let Ok(mut f) = std::fs::File::open(&path) else { continue };
        let mut buf = vec![0u8; 1 << 20];
        let mut n = 0;
        while n < buf.len() {
            match f.read(&mut buf[n..]) {
                Ok(0) | Err(_) => break,
                Ok(k) => n += k,
            }
        }
        buf.truncate(n);
        let m = lcc_meta::extract(&buf);
        let m = if m.model.is_none() { lcc_raw::probe_info(&buf).map(|i| i.metadata).unwrap_or(m) } else { m };
        println!(
            "{path}\t{}\t{}\t{}\t{}\t{}\t{}",
            m.model.unwrap_or_default(),
            m.serial_number.unwrap_or_default(),
            m.f_number.map_or(String::new(), |v| format!("{v:.1}")),
            m.exposure_time.map_or(String::new(), |v| format!("{v}")),
            m.iso.map_or(String::new(), |v| v.to_string()),
            m.capture_time.map(|t| t.to_iso()).unwrap_or_default()
        );
    }
}
