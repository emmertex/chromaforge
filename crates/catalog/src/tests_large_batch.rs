//! Issue #1361: a batch too large for one log record (a Lightroom import of millions of photos is one) is written as
//! several records of consecutive ops, which load back to what the whole batch did; a crash while they are written
//! keeps the records before it.

use crate::journal::LOG;
use crate::*;

/// One batch adding `n` photos, each with a caption of `caption` bytes, applied to `c`.
fn big_batch(c: &mut Catalog, n: usize, caption: usize) -> Op {
    let ops = (0..n)
        .map(|i| {
            let id = c.alloc_photo_id();
            let mut p = Photo::new(id, Source::Demo { scene: 1 }, &format!("IMG_{i:05}.jpg"), "JPEG", 60, 40, "2026-11-27T10:00:00");
            p.meta.caption = format!("{i} {}", "x".repeat(caption));
            Op::AddPhoto { photo: Box::new(p) }
        })
        .collect();
    let op = Op::Batch { ops };
    c.apply(op.clone()).unwrap();
    op
}

fn lines(log: &[u8]) -> Vec<&[u8]> {
    log.split(|b| *b == b'\n').filter(|l| !l.is_empty()).collect()
}

#[test]
fn a_large_batch_is_written_as_several_records_that_load_back_whole() {
    let m = MemStore::new();
    let (mut j, mut c, _) = Journal::open(Box::new(m.clone())).unwrap();
    let seq0 = j.seq();
    // about 10 MiB of ops: more than one record holds
    let op = big_batch(&mut c, 200, 50_000);
    j.append(&[op]).unwrap();
    j.flush().unwrap();
    let log = m.get(LOG).unwrap();
    let records = lines(&log);
    assert!(records.len() > 1, "split into {} record(s)", records.len());
    for r in &records {
        assert!(r.len() < (5 << 20), "a record holds about 4 MiB of ops at most, not {} bytes", r.len());
    }
    assert_eq!(j.seq(), seq0 + records.len() as u64, "one sequence number a record");

    let (_, loaded, report) = Journal::open(Box::new(m.clone())).unwrap();
    assert_eq!(loaded.photos().count(), 200);
    assert_eq!(report.replayed, records.len());
    assert!(loaded.photos().all(|p| p.meta.caption.len() > 50_000));

    // a small batch is still one record, as it always was
    let small = big_batch(&mut c, 3, 10);
    j.append(&[small]).unwrap();
    j.flush().unwrap();
    assert_eq!(lines(&m.get(LOG).unwrap()).len(), records.len() + 1);
}

#[test]
fn a_crash_while_a_large_batch_is_written_keeps_the_records_before_it() {
    let m = MemStore::new();
    let (mut j, mut c, _) = Journal::open(Box::new(m.clone())).unwrap();
    let op = big_batch(&mut c, 200, 50_000);
    j.append(&[op]).unwrap();
    j.flush().unwrap();
    drop(j);
    let log = m.get(LOG).unwrap();
    let first = lines(&log)[0].len() + 1;
    // the write stopped half way through the second record
    m.set(LOG, log[..first + (lines(&log)[1].len() / 2)].to_vec());
    let (_, loaded, report) = Journal::open(Box::new(m)).unwrap();
    let kept = loaded.photos().count();
    assert!(kept > 0 && kept < 200, "the first record's photos are back, the torn one's are not ({kept})");
    assert!(report.torn_bytes > 0);
    assert!(loaded.photos().all(|p| p.meta.caption.len() > 50_000), "each photo comes back whole");
}
