//! Decode checks for the public camera-coverage sweep.
//!
//! `docs/raw/coverage-sweep.tsv` records the file URLs, SHA-256 hashes, decoded dimensions and
//! camera-image comparisons measured on main 25ef65c8. This test checks each file decodes to the
//! recorded geometry and sample type, validates, has sane levels, and that probing agrees with the
//! decode. It does not pin sample values.
//! Fetch the CC0 files with `cargo xtask corpus --download`. Missing files are reported.

use lcc_raw::{RawData, decode, probe_info};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const RECORDS: &str = include_str!("../../../../docs/raw/coverage-sweep.tsv");

#[test]
fn coverage_sweep_files_decode_to_the_recorded_geometry() {
    let corpus = lcc_brand::env_os("CORPUS").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"));
    let mut checked = 0;
    for line in RECORDS.lines().filter(|line| !line.starts_with('#')).skip(1) {
        let columns: Vec<_> = line.split('\t').collect();
        assert_eq!(columns.len(), 14, "invalid coverage record: {line}");
        let name = columns[3];
        let path = corpus.join("raw").join(name);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                lcc_testkit::skip!(Corpus, "{} absent", path.display());
                continue;
            }
            Err(error) => panic!("{}: {error}", path.display()),
        };
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), columns[5], "{name}: file SHA-256");
        let raw = decode(&bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        raw.validate().unwrap();
        assert_eq!(raw.info(), probe_info(&bytes).unwrap(), "{name}: probe and decode disagree");
        let dimensions: Vec<usize> = columns[6..9].iter().map(|value| value.parse().unwrap()).collect();
        assert_eq!([raw.width, raw.height, raw.cpp], dimensions.as_slice(), "{name}: sensor dimensions");
        let expected_type = columns[9];
        match &raw.data {
            RawData::U16(_) => assert_eq!(expected_type, "u16", "{name}: sample type"),
            RawData::F32(samples) => {
                assert_eq!(expected_type, "f32", "{name}: sample type");
                assert!(samples.iter().all(|sample| sample.is_finite()), "{name}: non-finite sample");
            }
        }
        assert!(raw.white_at(0) > raw.black.mean(), "{name}: invalid black/white levels");
        checked += 1;
        eprintln!("checked {name}: {}x{}", raw.width, raw.height);
    }
    eprintln!("camera-coverage sweep: {checked} files checked");
}
