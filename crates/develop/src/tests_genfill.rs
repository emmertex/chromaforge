//! A generative spot's results: kept across Generate Again, offered as variations, put back after a move, carried
//! through a change of coordinates.

use crate::{GenFill, GenStroke, Spot, SpotMode};
use lcc_geom::Point;

fn spot() -> Spot {
    Spot { mode: SpotMode::Generate, points: vec![Point::new(0.4, 0.5), Point::new(0.42, 0.5)], size: 0.02, ..Default::default() }
}

fn made(sp: &Spot, id: &str, variants: u32) -> GenFill {
    GenFill {
        id: id.into(),
        variants,
        stroke: sp.stroke_key(),
        made_for: Some(GenStroke { points: sp.points.clone(), size: sp.size }),
        scale: 1.0,
        ..Default::default()
    }
}

/// Generate Again keeps the earlier, paid results; every result of every generation can be chosen.
#[test]
fn every_generation_is_kept_and_can_be_chosen() {
    let sp = spot();
    let first = made(&sp, "00000000000000a1", 2);
    let second = first.followed_by(made(&sp, "00000000000000b2", 1));
    assert_eq!(second.id, "00000000000000b2");
    assert_eq!(second.results(), (3, 2), "two earlier variations, then this one");
    let ids = |g: &GenFill| g.generations().iter().map(|g| g.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&second), ["00000000000000a1", "00000000000000b2"]);
    let mut g = second.clone();
    assert!(g.choose(1));
    assert_eq!((g.id.as_str(), g.variant, g.results()), ("00000000000000a1", 1, (3, 1)));
    // the order of the results stays put
    assert_eq!(ids(&g), ["00000000000000a1", "00000000000000b2"]);
    assert!(g.choose(2));
    assert_eq!((g.id.as_str(), g.results()), ("00000000000000b2", (3, 2)));
    assert!(!g.choose(3));
    // a third generation follows all of them, none nested
    let third = g.followed_by(made(&sp, "00000000000000c3", 1));
    assert_eq!(ids(&third), ["00000000000000a1", "00000000000000b2", "00000000000000c3"]);
    assert!(third.earlier.iter().all(|e| e.earlier.is_empty()));
    // bounded
    let mut many = made(&sp, "0000000000000000", 1);
    for i in 1..60u32 {
        many = many.followed_by(made(&sp, &format!("{i:016x}"), 1));
    }
    assert_eq!(many.generations().len(), crate::settings::MAX_GENERATIONS);
    assert_eq!(many.id, format!("{:016x}", 59));
}

/// A spot resized or moved since it was generated stops showing its fill; Put Back returns it to the stroke the
/// fill was made for, and it shows again.
#[test]
fn a_moved_or_resized_spot_can_be_put_back() {
    let mut sp = spot();
    sp.generated = Some(made(&sp, "00000000000000a1", 1));
    assert!(sp.shown_gen().is_some());
    assert!(!sp.put_back(), "nothing to do");
    sp.size = 0.004;
    sp.points.iter_mut().for_each(|p| p.x += 0.003);
    assert!(sp.shown_gen().is_none());
    assert!(sp.put_back());
    assert!(sp.shown_gen().is_some());
    assert_eq!(sp.size, 0.02);
    // a fill made before strokes were kept can't be put back this way
    let mut old = spot();
    old.generated = Some(GenFill { made_for: None, ..made(&old, "00000000000000a1", 1) });
    old.size = 0.01;
    assert!(!old.put_back());
}

/// Moving every point with a change of coordinates keeps a shown fill shown (and earlier strokes move along).
#[test]
fn a_change_of_coordinates_keeps_the_fill() {
    let mut sp = spot();
    let first = made(&sp, "00000000000000a1", 1);
    sp.generated = Some(first.followed_by(made(&sp, "00000000000000b2", 1)));
    sp.map_points(|p| Point::new(p.y, 1.0 - p.x));
    assert!(sp.shown_gen().is_some());
    let mut g = sp.generated.clone().unwrap();
    assert!(g.choose(0));
    sp.generated = Some(g);
    assert!(sp.shown_gen().is_some(), "the earlier generation's stroke moved along");
}

/// Settings written before these fields existed still read, and the new fields stay out of the file when empty.
#[test]
fn older_settings_read_and_empty_fields_are_not_written() {
    let v = serde_json::json!({"id": "00000000000000a1", "variants": 2, "variant": 1, "provider": "bfl", "stroke": 7, "scale": 1.0});
    let g: GenFill = serde_json::from_value(v).unwrap();
    assert_eq!((g.made_for.clone(), g.earlier.len(), g.place), (None, 0, 0));
    assert_eq!(g.results(), (2, 1));
    let back = serde_json::to_value(&g).unwrap();
    assert!(back.get("earlier").is_none() && back.get("place").is_none() && back.get("made_for").is_none(), "{back}");
}
