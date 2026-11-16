//! The kept search text ([`crate::index`]: free-text search fields and a Text rule's text) gives
//! the answers reading each photo's own fields gives, through edits, inverses, adds and removes.

use proptest::prelude::*;

use super::*;
use crate::rules::{Match, Rule, RuleSet};

const WORDS: [&str; 6] = ["Ocean", "straße", "İstanbul", "ΣΟΦΟΣ", "alps", ""];

fn library(n: u64) -> Catalog {
    let mut c = Catalog::new();
    for i in 0..n {
        let p = Photo::new(PhotoId(i + 1), Source::Demo { scene: 1 }, &format!("IMG_{i}.jpg"), "JPEG", 60, 40, "2026-01-01T00:00:00");
        c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let rules =
        RuleSet { mode: Match::All, rules: vec![Rule::Field { field: "text".into(), op: "contains".into(), value: serde_json::json!("ocean") }] };
    let album = Album { smart: Some(Box::new(Filter { rule_set: Some(rules), ..Default::default() })), ..Album::new(AlbumId(1), "Text") };
    c.apply(Op::AddAlbum { album }).unwrap();
    c
}

/// Every answer worked out on a copy (which keeps no text) and on the catalog itself.
fn answers(c: &Catalog) -> Vec<(Vec<PhotoId>, Vec<PhotoId>)> {
    let fresh = c.clone();
    let queries = ["ocean", "STRASSE", "straße", "i̇stanbul", "σοφος", "img_1", "name:img", "x:y", "\"ocean alps\"", ""];
    let mut out: Vec<(Vec<PhotoId>, Vec<PhotoId>)> = queries
        .iter()
        .map(|q| {
            let f = Filter { text: (*q).into(), ..Default::default() };
            (c.query(&f, &Sort::default()), fresh.photos().filter(|p| f.matches(p, &fresh)).map(|p| p.id).collect::<Vec<_>>())
        })
        .map(|(mut a, mut b)| {
            a.sort();
            b.sort();
            (a, b)
        })
        .collect();
    let mut a = c.album_photos(AlbumId(1));
    let mut b = fresh.album_photos(AlbumId(1));
    a.sort();
    b.sort();
    out.push((a, b));
    out
}

proptest! {
    #[test]
    fn kept_text_answers_as_each_photo_reads(steps in prop::collection::vec((0u8..4, 1u64..7, 0usize..6, 0usize..6), 0..24)) {
        let mut c = library(5);
        let mut undo = Vec::new();
        for (kind, id, w1, w2) in steps {
            let id = PhotoId(id);
            // a query first, so the kept text exists while the ops change what it holds
            for (kept, read) in answers(&c) {
                prop_assert_eq!(kept, read);
            }
            let op = match kind {
                0 => {
                    let Some(p) = c.photo(id) else { continue };
                    let mut m = p.meta.clone();
                    m.title = WORDS[w1].into();
                    m.keywords = vec![WORDS[w2].into()];
                    Op::SetMeta { id, meta: Box::new(m) }
                }
                1 => Op::SetRating { id, rating: (w1 % 6) as u8 },
                2 => match undo.pop() {
                    Some(inv) => inv,
                    None => continue,
                },
                _ => {
                    if c.photo(id).is_some() {
                        Op::RemovePhoto { id }
                    } else {
                        let mut p = Photo::new(id, Source::Demo { scene: 1 }, &format!("{}.jpg", WORDS[w1]), "JPEG", 60, 40, "2026-01-01T00:00:00");
                        p.meta.caption = WORDS[w2].into();
                        Op::AddPhoto { photo: Box::new(p) }
                    }
                }
            };
            if let Ok(inv) = c.apply(op) {
                undo.push(inv);
            }
        }
        for (kept, read) in answers(&c) {
            prop_assert_eq!(kept, read);
        }
    }
}

/// A photo that is not the catalog's own (a copy with another title) is read from its own fields.
#[test]
fn a_copy_of_a_photo_is_read_from_its_own_fields() {
    let c = library(3);
    let f = Filter { text: "ocean".into(), ..Default::default() };
    assert!(c.query(&f, &Sort::default()).is_empty(), "the kept text is made");
    let mut copy = (**c.photos.get(&PhotoId(1)).unwrap()).clone();
    copy.meta.title = "Ocean".into();
    assert!(f.matches(&copy, &c));
    assert!(c.album(AlbumId(1)).and_then(|a| a.smart.as_ref()).unwrap().matches(&copy, &c));
}
