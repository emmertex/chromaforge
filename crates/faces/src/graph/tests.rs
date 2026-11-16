use super::*;
use crate::synthetic::{len_field, value_info_bytes, varint, varint_field};
fn node() -> Vec<u8> {
    let mut n = Vec::new();
    for (id, v) in [(1, "x"), (2, "y"), (4, "Identity")] {
        len_field(id, v.as_bytes(), &mut n);
    }
    n
}
fn weight(raw: &[u8]) -> Vec<u8> {
    let mut t = Vec::new();
    varint_field(1, 1, &mut t);
    varint_field(2, 1, &mut t);
    len_field(8, b"w", &mut t);
    len_field(9, raw, &mut t);
    t
}
fn model(node: &[u8], weights: &[Vec<u8>]) -> Vec<u8> {
    let mut g = Vec::new();
    len_field(1, node, &mut g);
    for t in weights {
        len_field(5, t, &mut g);
    }
    for (id, name) in [(11, "x"), (12, "y")] {
        len_field(id, &value_info_bytes(name, 1, &[Ok(1), Ok(3), Ok(112), Ok(112)]), &mut g);
    }
    let mut ops = Vec::new();
    varint_field(2, 11, &mut ops);
    let mut out = Vec::new();
    len_field(7, &g, &mut out);
    len_field(8, &ops, &mut out);
    out
}
#[test]
fn raw_and_typed_weights_are_exact_not_truncated_or_ambiguous() {
    assert!(load(&model(&node(), &[weight(&1f32.to_le_bytes())])).is_ok());
    for raw in [vec![0; 3], vec![0; 5], vec![0; 8]] {
        assert!(load(&model(&node(), &[weight(&raw)])).is_err());
    }
    let mut t = weight(&1f32.to_le_bytes());
    varint(4 << 3 | 5, &mut t);
    t.extend_from_slice(&1f32.to_le_bytes());
    assert!(load(&model(&node(), &[t])).is_err());
    let mut t = weight(&1f32.to_le_bytes());
    len_field(9, &1f32.to_le_bytes(), &mut t);
    assert!(load(&model(&node(), &[t])).is_err());
    // The protobuf float_data form must work as well as raw_data.
    let mut t = Vec::new();
    varint_field(1, 1, &mut t);
    varint_field(2, 1, &mut t);
    len_field(8, b"w", &mut t);
    varint(4 << 3 | 5, &mut t);
    t.extend_from_slice(&1f32.to_le_bytes());
    assert!(load(&model(&node(), &[t])).is_ok());
}
#[test]
fn external_sparse_duplicate_custom_domain_and_wrong_dtype_are_errors() {
    let t = weight(&1f32.to_le_bytes());
    assert!(load(&model(&node(), &[t.clone(), t.clone()])).is_err());
    for field in [13, 14] {
        let mut bad = t.clone();
        if field == 13 {
            len_field(field, b"external", &mut bad);
        } else {
            varint_field(field, 1, &mut bad);
        }
        assert!(load(&model(&node(), &[bad])).is_err());
    }
    let mut bad = t.clone();
    varint_field(2, 10, &mut bad);
    assert!(load(&model(&node(), &[bad])).is_err());
    let mut n = node();
    len_field(7, b"vendor.example", &mut n);
    assert!(load(&model(&n, &[])).is_err());
    let mut attr = Vec::new();
    len_field(1, b"ratio", &mut attr);
    varint_field(20, 1, &mut attr);
    let mut n = node();
    len_field(5, &attr, &mut n);
    len_field(5, &attr, &mut n);
    assert!(load(&model(&n, &[])).is_err());
    let mut g = Vec::new();
    len_field(15, b"sparse", &mut g);
    let mut bad = model(&node(), &[]);
    len_field(7, &g, &mut bad);
    assert!(load(&bad).is_err());
}
#[test]
fn overflowing_and_cross_boundary_varints_long_strings_and_bad_shapes_are_errors() {
    assert!(load(&[0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02]).is_err());
    let mut t = Vec::new();
    len_field(1, &[0x80], &mut t);
    varint_field(2, 1, &mut t);
    len_field(8, b"w", &mut t);
    assert!(load(&model(&node(), &[t])).is_err());
    let mut n = node();
    len_field(1, &vec![b'x'; 4097], &mut n);
    assert!(load(&model(&n, &[])).is_err());
    let mut n = node();
    len_field(1, &[0xff], &mut n);
    assert!(load(&model(&n, &[])).is_err());
    let mut t = Vec::new();
    varint_field(1, u64::MAX, &mut t);
    varint_field(1, 8, &mut t);
    varint_field(2, 1, &mut t);
    len_field(8, b"w", &mut t);
    assert!(load(&model(&node(), &[t])).is_err());
    let mut bad = model(&node(), &[]);
    let mut ops = Vec::new();
    varint_field(2, 999, &mut ops);
    len_field(8, &ops, &mut bad);
    assert!(load(&bad).is_err());
}
proptest::proptest! {
    #[test]
    fn arbitrary_graph_bytes_do_not_panic(bytes in proptest::collection::vec(proptest::num::u8::ANY,0..4096)) { let _=load(&bytes); }
}
