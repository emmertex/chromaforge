use super::*;

/// Protobuf writers for a tiny ONNX model (the field numbers are ONNX's `onnx.proto`).
mod proto {
    fn varint(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    pub fn int(out: &mut Vec<u8>, field: u64, v: u64) {
        varint(out, field << 3);
        varint(out, v);
    }

    pub fn bytes(out: &mut Vec<u8>, field: u64, v: &[u8]) {
        varint(out, (field << 3) | 2);
        varint(out, v.len() as u64);
        out.extend_from_slice(v);
    }

    /// A float32 tensor value-info named `name` whose dimensions are all dynamic.
    fn value_info(name: &str, rank: usize) -> Vec<u8> {
        let mut shape = Vec::new();
        for i in 0..rank {
            let mut dim = Vec::new();
            bytes(&mut dim, 2, format!("d{i}").as_bytes());
            bytes(&mut shape, 1, &dim);
        }
        let mut tensor = Vec::new();
        int(&mut tensor, 1, 1); // FLOAT
        bytes(&mut tensor, 2, &shape);
        let mut ty = Vec::new();
        bytes(&mut ty, 1, &tensor);
        let mut vi = Vec::new();
        bytes(&mut vi, 1, name.as_bytes());
        bytes(&mut vi, 2, &ty);
        vi
    }

    fn node(inputs: &[&str], output: &str, op: &str) -> Vec<u8> {
        let mut n = Vec::new();
        for i in inputs {
            bytes(&mut n, 1, i.as_bytes());
        }
        bytes(&mut n, 2, output.as_bytes());
        bytes(&mut n, 4, op.as_bytes());
        n
    }

    /// `y = Relu(x · 2)` for a rank-`rank` float tensor `x`.
    pub fn relu_double(rank: usize) -> Vec<u8> {
        let mut two = Vec::new();
        int(&mut two, 2, 1); // FLOAT, a scalar
        bytes(&mut two, 8, b"two");
        bytes(&mut two, 9, &2f32.to_le_bytes());
        let mut graph = Vec::new();
        bytes(&mut graph, 1, &node(&["x", "two"], "m", "Mul"));
        bytes(&mut graph, 1, &node(&["m"], "y", "Relu"));
        bytes(&mut graph, 2, b"relu_double");
        bytes(&mut graph, 5, &two);
        bytes(&mut graph, 11, &value_info("x", rank));
        bytes(&mut graph, 12, &value_info("y", rank));
        let mut opset = Vec::new();
        int(&mut opset, 2, 17);
        let mut model = Vec::new();
        int(&mut model, 1, 8);
        bytes(&mut model, 7, &graph);
        bytes(&mut model, 8, &opset);
        model
    }
}

#[test]
fn shapes_are_checked() {
    assert!(Tensor::new(vec![1, 2, 3], vec![0.0; 6]).is_ok());
    assert!(Tensor::new(vec![1, 2, 3], vec![0.0; 5]).is_err());
    assert!(Tensor::new(vec![], vec![]).is_err());
    assert!(Tensor::new(vec![0, 3], vec![]).is_err());
    assert!(Tensor::new(vec![1; MAX_RANK + 1], vec![0.0]).is_err());
    assert!(check_shape(&[usize::MAX, 2], 4).is_err());
    assert!(check_shape(&[MAX_ELEMENTS + 1], MAX_ELEMENTS + 1).is_err());
}

#[test]
fn candidates_follow_the_setting() {
    let names = ["libonnxruntime.so"];
    assert!(candidates(Some(std::ffi::OsStr::new("off")), None, &names, true).is_empty());
    assert!(candidates(Some(std::ffi::OsStr::new(" OFF ")), None, &names, true).is_empty());
    assert_eq!(candidates(Some(std::ffi::OsStr::new("/x/libort.so")), None, &names, true), vec![PathBuf::from("/x/libort.so")]);
    // nothing set, no executable folder: only the bare names, and those only where allowed
    assert_eq!(candidates(None, None, &names, true), vec![PathBuf::from("libonnxruntime.so")]);
    assert!(candidates(None, None, &names, false).is_empty());
    // a folder that has the library
    let dir = std::env::temp_dir().join(format!("onnxrt-candidates-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(dir.join("lib").join("libonnxruntime.so"), b"").unwrap();
    let found = candidates(None, Some(&dir), &names, false);
    assert_eq!(found, vec![dir.join("lib").join("libonnxruntime.so")]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn providers_have_names_and_gpu_candidates() {
    for p in Provider::gpu_candidates() {
        assert!(p.is_gpu());
        assert!(!p.label().is_empty());
    }
    assert!(!Provider::Cpu.is_gpu());
}

#[test]
fn version_comes_from_the_build_info() {
    if !compiled_in() {
        return;
    }
    assert_eq!(imp::parse_version("ORT Build Info: git-branch=rel-1.32.0, git-commit-id=abc").as_deref(), Some("1.32.0"));
    assert_eq!(imp::parse_version("garbage"), None);
    assert_eq!(imp::parse_version("rel-"), None);
}

#[test]
fn hostile_model_files_are_errors() {
    let dir = std::env::temp_dir().join(format!("onnxrt-hostile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let empty = dir.join("empty.onnx");
    std::fs::write(&empty, b"").unwrap();
    assert!(Session::load(&empty, Want::Cpu { threads: 1 }).is_err());
    assert!(Session::load(&dir.join("missing.onnx"), Want::Cpu { threads: 1 }).is_err());
    assert!(Session::from_bytes(&[], Want::Cpu { threads: 1 }).is_err());
    // whatever the runtime: garbage, a truncated model, a model with a byte flipped are errors, never a panic
    let good = proto::relu_double(2);
    let mut cases: Vec<Vec<u8>> = vec![vec![0xff; 64], good[..good.len() / 2].to_vec(), b"not an onnx file".to_vec()];
    for i in (0..good.len()).step_by(7) {
        let mut flipped = good.clone();
        flipped[i] ^= 0x5a;
        cases.push(flipped);
    }
    for bytes in cases {
        let _ = Session::from_bytes(&bytes, Want::Cpu { threads: 1 }).and_then(|s| s.run_one(Tensor::new(vec![1, 2], vec![1.0, -1.0])?));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The library for the tests that run a model, or a skip.
fn library() -> Option<&'static Runtime> {
    if !compiled_in() {
        lcc_testkit::skip!(Feature, "built without the `ort` feature");
        return None;
    }
    match runtime() {
        Ok(rt) => Some(rt),
        Err(e) => {
            lcc_testkit::skip!(Model, "no ONNX Runtime library ({e}); set {} to one", lcc_brand::env_var("ONNXRUNTIME"));
            None
        }
    }
}

#[test]
fn a_model_runs_on_the_cpu() {
    let Some(rt) = library() else { return };
    assert!(rt.providers.contains(&Provider::Cpu));
    let s = Session::from_bytes(&proto::relu_double(2), Want::Cpu { threads: 1 }).unwrap();
    assert_eq!(s.provider(), Provider::Cpu);
    assert_eq!(s.inputs(), ["x"]);
    assert_eq!(s.outputs(), ["y"]);
    let y = s.run_one(Tensor::new(vec![2, 3], vec![1.0, -1.0, 0.5, -0.5, 3.0, 0.0]).unwrap()).unwrap();
    assert_eq!(y.shape, vec![2, 3]);
    assert_eq!(y.data, vec![2.0, 0.0, 1.0, 0.0, 6.0, 0.0]);
    assert!(s.describe().contains(&rt.version));
}

#[test]
fn bad_inputs_are_errors() {
    let Some(_) = library() else { return };
    let s = Session::from_bytes(&proto::relu_double(2), Want::Cpu { threads: 1 }).unwrap();
    // the wrong number of inputs, the wrong rank
    assert!(s.run(&[]).is_err());
    let t = Tensor::new(vec![2], vec![1.0, 2.0]).unwrap();
    assert!(s.run(&[t.clone(), t.clone()]).is_err());
    assert!(s.run_one(t).is_err());
    // a tensor whose data does not fill its shape never reaches the runtime
    assert!(s.run(&[Tensor { shape: vec![2, 2], data: vec![1.0] }]).is_err());
    // still works afterwards
    assert!(s.run_one(Tensor::new(vec![1, 1], vec![1.0]).unwrap()).is_ok());
}

#[test]
fn the_gpu_is_tried_and_says_why_not() {
    let Some(rt) = library() else { return };
    match Session::from_bytes(&proto::relu_double(4), Want::Gpu) {
        Ok(s) => {
            assert!(s.provider().is_gpu());
            let y = s.run_one(Tensor::new(vec![1, 1, 1, 2], vec![-1.0, 1.5]).unwrap()).unwrap();
            assert_eq!(y.data, vec![0.0, 3.0]);
        }
        Err(e) => {
            assert!(!e.to_string().is_empty());
            lcc_testkit::skip!(Gpu, "no ONNX Runtime GPU provider here ({e}); {} has {:?}", rt.library.display(), rt.providers);
        }
    }
}

#[test]
fn sessions_run_from_many_threads() {
    let Some(_) = library() else { return };
    let s = std::sync::Arc::new(Session::from_bytes(&proto::relu_double(1), Want::Cpu { threads: 1 }).unwrap());
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let s = s.clone();
            std::thread::spawn(move || s.run_one(Tensor::new(vec![1], vec![i as f32]).unwrap()).unwrap().data[0])
        })
        .collect();
    let got: Vec<f32> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(got, vec![0.0, 2.0, 4.0, 6.0]);
}
