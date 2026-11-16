use super::*;
use crate::graph::{Attr, Weight};
fn weight(shape: &[usize], values: Vec<f32>) -> Weight {
    Weight { dims: shape.to_vec(), data: Data::F32(values) }
}
fn graph(op: &str, extra: &[&str], weights: Vec<(&str, Weight)>, attrs: Vec<(&str, Attr)>) -> Graph {
    Graph {
        nodes: vec![Node {
            op: op.into(),
            inputs: std::iter::once("x").chain(extra.iter().copied()).map(str::to_string).collect(),
            outputs: vec!["y".into()],
            attrs: attrs.into_iter().map(|(s, a)| (s.into(), a)).collect(),
        }],
        weights: weights.into_iter().map(|(s, w)| (s.into(), w)).collect(),
        inputs: vec![("x".into(), vec![1, 3, 2, 2])],
        outputs: vec!["y".into()],
    }
}
fn execute(g: Graph, shape: &[usize], values: Vec<f32>) -> Tensor {
    Net::new(g).unwrap().run(Tensor::new(shape.to_vec(), values).unwrap()).unwrap().remove(0).1
}
#[test]
fn batch_norm_matches_the_inference_formula_for_images_and_matrices() {
    for shape in [vec![1, 3, 2, 5], vec![2, 3]] {
        let values: Vec<_> = (0..shape.iter().product()).map(|i| i as f32 * 0.07 - 0.8).collect();
        let g = graph(
            "BatchNormalization",
            &["scale", "bias", "mean", "var"],
            vec![
                ("scale", weight(&[3], vec![0.7, -1.1, 2.3])),
                ("bias", weight(&[3], vec![-0.4, 0.6, 0.2])),
                ("mean", weight(&[3], vec![0.5, -0.3, 0.9])),
                ("var", weight(&[3], vec![0.2, 0.0, 1.4])),
            ],
            vec![("epsilon", Attr::Float(1e-3))],
        );
        let out = execute(g.clone(), &shape, values.clone());
        let spatial: usize = shape.get(2..).unwrap_or(&[]).iter().product();
        for (i, (&value, &got)) in values.iter().zip(&out.data).enumerate() {
            let c = (i / spatial) % 3;
            let [s, b, m, v] = ["scale", "bias", "mean", "var"].map(|name| {
                let Data::F32(v) = &g.weights[name].data else { panic!() };
                v[c]
            });
            let want = (value - m) * s / (v + 1e-3).sqrt() + b;
            assert!((got - want).abs() < 1e-5);
        }
    }
}
#[test]
fn prelu_obeys_trailing_broadcasting_including_spatial_slopes() {
    for (shape, slopes) in
        [(vec![3, 1, 1], vec![0.1, 0.2, 0.3]), (vec![1, 3, 1, 1], vec![0.1, 0.2, 0.3]), (vec![1], vec![0.3]), (vec![2, 2], vec![0.1, 0.2, 0.3, 0.4])]
    {
        let values: Vec<_> = (0..12).map(|i| i as f32 - 6.0).collect();
        let out = execute(graph("PRelu", &["s"], vec![("s", weight(&shape, slopes.clone()))], vec![]), &[1, 3, 2, 2], values.clone());
        for (i, (&v, &got)) in values.iter().zip(&out.data).enumerate() {
            let alpha = if slopes.len() == 1 {
                slopes[0]
            } else if slopes.len() == 3 {
                slopes[i / 4]
            } else {
                slopes[i % 4]
            };
            assert_eq!(got, if v < 0.0 { v * alpha } else { v });
        }
    }
}
#[test]
fn gemm_transpositions_scalars_vectors_matrices_and_optional_bias() {
    for ta in [0, 1] {
        for tb in [0, 1] {
            for bias_shape in [vec![], vec![2], vec![1, 2], vec![2, 1], vec![2, 2]] {
                let (ash, bsh) = (if ta == 0 { vec![2, 3] } else { vec![3, 2] }, if tb == 0 { vec![3, 2] } else { vec![2, 3] });
                let a: Vec<_> = (0..6).map(|i| i as f32 * 0.4 - 0.8).collect();
                let b: Vec<_> = (0..6).map(|i| i as f32 * 0.1 - 0.3).collect();
                let bias: Vec<_> = (0..bias_shape.iter().product()).map(|i| i as f32 * 0.25 + 0.3).collect();
                let g = graph(
                    "Gemm",
                    &["w", "b"],
                    vec![("w", weight(&bsh, b.clone())), ("b", weight(&bias_shape, bias.clone()))],
                    vec![("transA", Attr::Int(ta)), ("transB", Attr::Int(tb)), ("alpha", Attr::Float(0.7)), ("beta", Attr::Float(-0.4))],
                );
                let out = execute(g, &ash, a.clone());
                assert_eq!(out.shape, vec![2, 2]);
                for i in 0..2 {
                    for j in 0..2 {
                        let sum = (0..3)
                            .map(|k| a[if ta == 0 { i * 3 + k } else { k * 2 + i }] * b[if tb == 0 { k * 2 + j } else { j * 3 + k }])
                            .sum::<f32>();
                        let off = match bias_shape.as_slice() {
                            [] => 0,
                            [2] | [1, 2] => j,
                            [2, 1] => i,
                            _ => i * 2 + j,
                        };
                        assert!((out.data[i * 2 + j] - (0.7 * sum - 0.4 * bias[off])).abs() < 1e-6);
                    }
                }
            }
        }
    }
    let out = execute(graph("Gemm", &["w"], vec![("w", weight(&[2, 1], vec![2.0, 3.0]))], vec![]), &[1, 2], vec![4.0, 5.0]);
    assert_eq!(out.data, vec![23.0]);
}
#[test]
fn arithmetic_broadcasts_and_keeps_operand_order() {
    for op in ["Add", "Sub", "Mul"] {
        let out =
            execute(graph(op, &["w"], vec![("w", weight(&[3, 1], vec![0.5, 1.5, 2.5]))], vec![]), &[1, 3, 2], vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        for (i, &got) in out.data.iter().enumerate() {
            let x = (i + 1) as f32;
            let y = (i / 2) as f32 + 0.5;
            let want = match op {
                "Add" => x + y,
                "Sub" => x - y,
                _ => x * y,
            };
            assert_eq!(got, want);
        }
    }
}
#[test]
fn flatten_negative_axis_and_default_transpose_are_correct() {
    let out = execute(graph("Flatten", &[], vec![], vec![("axis", Attr::Int(-1))]), &[1, 3, 2, 5], (0..30).map(|i| i as f32).collect());
    assert_eq!(out.shape, vec![6, 5]);
    let out = execute(graph("Transpose", &[], vec![], vec![]), &[2, 3], (0..6).map(|i| i as f32).collect());
    assert_eq!(out.shape, vec![3, 2]);
    assert_eq!(out.data, vec![0.0, 3.0, 1.0, 4.0, 2.0, 5.0]);
}
#[test]
fn all_recognition_operators_run_on_seeded_synthetic_networks() {
    let input = Tensor::new(vec![1, 3, 112, 112], (0..3 * 112 * 112).map(|i| (i * 37 % 256) as f32).collect()).unwrap();
    for seed in 0..20 {
        let net = Net::new(crate::graph::load(&crate::synthetic::recognition_model(seed)).unwrap()).unwrap();
        let a = net.run(input.clone()).unwrap();
        let b = net.run(input.clone()).unwrap();
        assert_eq!(a, b);
        assert_eq!(a[0].1.shape, vec![1, 8]);
    }
}
#[test]
fn hostile_attributes_weights_shapes_and_public_tensors_return_errors() {
    for (name, attr) in [
        ("strides", Attr::Ints(vec![0, 1])),
        ("pads", Attr::Ints(vec![i64::MAX; 4])),
        ("group", Attr::Int(0)),
        ("kernel_shape", Attr::Ints(vec![7, 7])),
        ("strides", Attr::Float(1.0)),
        ("new_attribute", Attr::Int(1)),
    ] {
        let g = graph("Conv", &["w"], vec![("w", weight(&[3, 3, 1, 1], vec![0.1; 9]))], vec![(name, attr)]);
        let result = Net::new(g).and_then(|n| n.validate_input(&[1, 3, 3, 3]));
        assert!(result.is_err(), "{name}");
    }
    for op in ["Loop", "MatMul", "If", "Concat", "Unknown"] {
        assert!(Net::new(graph(op, &[], vec![], vec![])).is_err());
    }
    for value in [f32::NAN, f32::INFINITY] {
        assert!(Net::new(graph("PRelu", &["w"], vec![("w", weight(&[1], vec![value]))], vec![])).is_err());
    }
    let net = Net::new(graph("Relu", &[], vec![], vec![])).unwrap();
    for input in [
        Tensor { shape: vec![usize::MAX, 2], data: vec![] },
        Tensor { shape: vec![0, 3], data: vec![] },
        Tensor { shape: vec![1], data: vec![f32::NAN] },
        Tensor { shape: vec![1; 9], data: vec![1.0] },
        Tensor { shape: vec![2], data: vec![1.0] },
    ] {
        assert!(net.run(input).is_err());
    }
    let mut g = graph("Relu", &[], vec![], vec![]);
    g.nodes.push(g.nodes[0].clone());
    assert!(Net::new(g).is_err());
    let mut g = graph("Relu", &[], vec![], vec![]);
    g.nodes[0].inputs[0] = "future".into();
    assert!(Net::new(g).is_err());
}
#[test]
fn malformed_normalization_prelu_matrix_dropout_and_axes_are_errors() {
    let bn = || {
        graph(
            "BatchNormalization",
            &["s", "b", "m", "v"],
            vec![
                ("s", weight(&[3], vec![1.0; 3])),
                ("b", weight(&[3], vec![0.0; 3])),
                ("m", weight(&[3], vec![0.0; 3])),
                ("v", weight(&[3], vec![1.0; 3])),
            ],
            vec![],
        )
    };
    for mutation in 0..4 {
        let mut g = bn();
        match mutation {
            0 => {
                g.nodes[0].attrs.insert("training_mode".into(), Attr::Int(1));
            }
            1 => {
                g.nodes[0].attrs.insert("epsilon".into(), Attr::Float(0.0));
            }
            2 => {
                g.weights.insert("v".into(), weight(&[3], vec![-1.0; 3]));
            }
            _ => {
                g.weights.insert("b".into(), weight(&[1, 3], vec![0.0; 3]));
            }
        };
        assert!(Net::new(g).and_then(|n| n.validate_input(&[1, 3, 2, 2])).is_err());
    }
    for g in [
        graph("PRelu", &["s"], vec![("s", weight(&[3], vec![0.1; 3]))], vec![]),
        graph("Gemm", &["w"], vec![("w", weight(&[4, 2], vec![0.1; 8]))], vec![]),
        graph("Flatten", &[], vec![], vec![("axis", Attr::Int(i64::MIN))]),
        graph("Dropout", &[], vec![], vec![("ratio", Attr::Float(1.0))]),
        graph("Transpose", &[], vec![], vec![("perm", Attr::Ints(vec![0, 1, 1, 3]))]),
    ] {
        assert!(Net::new(g).and_then(|n| n.validate_input(&[1, 3, 2, 2])).is_err());
    }
}
#[test]
fn arithmetic_overflow_is_reported_and_next_run_succeeds() {
    let net = Net::new(graph("Mul", &["w"], vec![("w", weight(&[1], vec![f32::MAX]))], vec![])).unwrap();
    assert!(net.run(Tensor::new(vec![1], vec![2.0]).unwrap()).is_err());
    assert!(net.run(Tensor::new(vec![1], vec![0.0]).unwrap()).is_ok());
}

#[test]
fn multiple_batches_prelu_keeps_channels_independent() {
    let out = execute(graph("PRelu", &["s"], vec![("s", weight(&[3, 1, 1], vec![0.1, 0.2, 0.3]))], vec![]), &[2, 3, 2, 2], vec![-1.0; 24]);
    for (i, &v) in out.data.iter().enumerate() {
        assert_eq!(v, -[0.1, 0.2, 0.3][i / 4 % 3]);
    }
}
#[test]
fn integer_activations_and_constant_outputs_fail_at_load() {
    let integer = Weight { dims: vec![1], data: Data::I64(vec![1]) };
    assert!(Net::new(graph("Add", &["w"], vec![("w", integer.clone())], vec![])).is_err());
    let mut g = graph("Relu", &[], vec![("w", integer)], vec![]);
    g.outputs = vec!["w".into()];
    assert!(Net::new(g).is_err());
}
#[test]
fn normalisation_coefficient_and_working_memory_overflows_fail_before_running() {
    let g = graph(
        "BatchNormalization",
        &["s", "b", "m", "v"],
        vec![
            ("s", weight(&[3], vec![f32::MAX; 3])),
            ("b", weight(&[3], vec![0.0; 3])),
            ("m", weight(&[3], vec![f32::MAX; 3])),
            ("v", weight(&[3], vec![0.0; 3])),
        ],
        vec![],
    );
    assert!(Net::new(g).unwrap().validate_input(&[1, 3, 2, 2]).is_err());
    let g = graph("Conv", &["w"], vec![("w", weight(&[4096, 3, 1, 1], vec![0.1; 4096 * 3]))], vec![]);
    assert!(Net::new(g).unwrap().validate_input(&[1, 3, 4096, 4096]).is_err());
}

#[test]
fn empty_broadcast_operands_are_rejected_before_allocation() {
    for op in ["Add", "Sub", "Mul", "PRelu"] {
        let g = graph(op, &["w"], vec![("w", weight(&[0, 3, 1, 1], vec![]))], vec![]);
        assert!(Net::new(g).and_then(|n| n.validate_input(&[1, 3, 2, 2])).is_err());
    }
}

#[test]
fn odd_maps_cross_panel_boundaries_and_reused_padding_is_zero() {
    for (c, co, groups) in [(3, 11, 1), (6, 10, 2)] {
        let weights: Vec<_> = (0..co * (c / groups) * 9).map(|i| (i as f32 % 19.0 - 9.0) * 0.013).collect();
        let g = graph(
            "Conv",
            &["w", "b"],
            vec![("w", weight(&[co, c / groups, 3, 3], weights)), ("b", weight(&[co], vec![0.1; co]))],
            vec![("group", Attr::Int(groups as i64)), ("strides", Attr::Ints(vec![1, 1])), ("pads", Attr::Ints(vec![2, 1, 0, 1]))],
        );
        let mut scratch = kernels::Scratch::default();
        for zero in [false, true, false] {
            let input = Tensor::new(vec![1, c, 23, 29], (0..c * 23 * 29).map(|i| if zero { 0.0 } else { (i as f32 % 29.0 - 14.0) * 0.12 }).collect())
                .unwrap();
            let got = kernels::conv(&g.nodes[0], &input, &g, &mut scratch).unwrap();
            let want = conv_direct(&g.nodes[0], &input, &g).unwrap();
            assert_eq!(got.shape, want.shape);
            for (&a, &b) in got.data.iter().zip(&want.data) {
                assert!((a - b).abs() < 1e-5, "{a} vs {b}");
            }
        }
    }
}

#[test]
fn retained_convolution_scratch_counts_towards_the_working_limit() {
    let network = |output_channels: usize| {
        let pads = vec![("pads", Attr::Ints(vec![1, 0, 0, 0]))];
        let mut g = graph("Conv", &["w"], vec![("w", weight(&[2, 130_000, 2, 1], vec![0.0; 2 * 130_000 * 2]))], pads.clone());
        g.inputs[0].1 = vec![1, 130_000, 2, 256];
        g.weights.insert("v".into(), weight(&[output_channels, 2, 2, 1], vec![0.0; output_channels * 4]));
        g.nodes.push(Node {
            op: "Conv".into(),
            inputs: vec!["y".into(), "v".into()],
            outputs: vec!["z".into()],
            attrs: pads.into_iter().map(|(s, a)| (s.into(), a)).collect(),
        });
        g.outputs = vec!["z".into()];
        Net::new(g).unwrap()
    };
    // The first layer retains a large im2col buffer; the second grows the product buffer.
    // Both layers individually fit, but keeping both scratch peaks would exceed 512 MiB.
    let shape = [1, 130_000, 2, 256];
    assert!(network(131_072).validate_input(&shape).is_err());
    assert!(network(65_536).validate_input(&shape).is_ok());
}

#[test]
fn constant_input_copies_count_towards_the_working_limit() {
    let network = |retained_outputs: usize| {
        let dims = [1, 1, 2048, 2048];
        let mut g = graph("Identity", &[], vec![("w", weight(&dims, vec![0.0; 2048 * 2048]))], vec![]);
        g.inputs[0].1 = dims.to_vec();
        g.nodes.clear();
        g.outputs.clear();
        for i in 0..retained_outputs {
            let name = format!("retained-{i}");
            g.nodes.push(Node { op: "Identity".into(), inputs: vec!["x".into()], outputs: vec![name.clone()], attrs: HashMap::new() });
            g.outputs.push(name);
        }
        g.nodes.push(Node { op: "Add".into(), inputs: vec!["x".into(), "w".into()], outputs: vec!["z".into()], attrs: HashMap::new() });
        g.outputs.push("z".into());
        Net::new(g).unwrap()
    };
    // Infer only: no pixel tensors are allocated. The constant's temporary copy must be counted.
    assert!(network(30).validate_input(&[1, 1, 2048, 2048]).is_err());
    assert!(network(29).validate_input(&[1, 1, 2048, 2048]).is_ok());
}
