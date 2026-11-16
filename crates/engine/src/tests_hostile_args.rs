//! Hostile arguments to the develop and mask commands (what MCP, the control channel and the CLI forward
//! unchecked) are errors that change nothing: no panic, no edit, and no other mask or photo acted on in
//! their place (a mask id of 2^32 + k used to alias mask k).

use serde_json::{Value, json};

use crate::Session;

fn state(s: &Session) -> (Value, Option<u32>) {
    let id = s.active().unwrap();
    (serde_json::to_value(&*s.develop_of(id).unwrap()).unwrap(), s.active_mask)
}

#[test]
fn hostile_develop_and_mask_arguments_error_and_change_nothing() {
    let mut s = Session::with_demo();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    s.execute("mask.select", &json!({"id": 2})).unwrap();
    let before = state(&s);
    let mut table: Vec<(&str, Value)> = vec![
        ("develop.set", json!({"values": {"light.exposure": "1"}})),
        ("develop.set", json!({"values": {"light.exposure": null}})),
        ("develop.set", json!({"values": {"light.exposure": [1]}})),
        ("develop.set", json!({"values": {"no.such": 1}})),
        ("develop.set", json!({"values": []})),
        ("develop.set", json!({"control": "light.exposure", "value": "high"})),
        ("develop.set", json!({"control": 7, "value": 1})),
        ("develop.set", json!(null)),
    ];
    // Ids that are not a mask: out of u32 range (aliasing mask 1 or 2), negative, fractional, text, list.
    for bad in [json!(4_294_967_297u64), json!(4_294_967_298u64), json!(-1), json!(1.5), json!("1"), json!([1]), json!(true)] {
        table.push(("mask.select", json!({"id": bad})));
        table.push(("mask.delete", json!({"id": bad})));
        table.push(("mask.rename", json!({"id": bad, "name": "x"})));
        table.push(("mask.invert", json!({"id": bad})));
        table.push(("mask.visible", json!({"id": bad, "visible": false})));
        table.push(("mask.move", json!({"id": bad, "to": 0})));
        table.push(("mask.adjust", json!({"id": bad, "values": {"light.exposure": 1.0}})));
        table.push(("mask.brushStroke", json!({"id": bad, "points": [[0.1, 0.1], [0.2, 0.2]]})));
    }
    for (cmd, params) in &table {
        let e = s.execute(cmd, params).err().unwrap_or_else(|| panic!("{cmd} {params} must be refused"));
        assert!(!e.to_string().is_empty(), "{cmd} {params}: empty error");
        assert_eq!(state(&s), before, "{cmd} {params} changed the session");
    }
    // Out-of-range numbers are clamped to the control's range, never stored as given.
    for v in [1e308, -1e308, 1e19] {
        s.execute("develop.set", &json!({"values": {"light.exposure": v}})).unwrap();
        let ev = s.develop_of(s.active().unwrap()).unwrap().light.exposure;
        assert!((-5.0..=5.0).contains(&ev), "{v} stored as {ev}");
    }
}
