//! Writes ONNX models (`.onnx` protobuf bytes) from Rust, so a model whose weights we already hold in another form
//! (CLIP's `.safetensors`) can run through ONNX Runtime without a second download or a Python export step.
//!
//! Only what our exports need: float32 and int64 initializers, nodes with int, ints and float attributes, and float32
//! graph inputs and outputs whose dimensions are fixed or named. Field numbers are those of ONNX's `onnx.proto`
//! (Apache-2.0; the message layout is read as a reference, nothing is copied). Pure safe Rust, no `ort`: it builds in
//! every target, the web included, and its output can be checked by any ONNX reader.

/// ONNX `TensorProto.DataType` values used here.
const FLOAT: u64 = 1;
const INT64: u64 = 7;

/// A node attribute.
#[derive(Debug, Clone, PartialEq)]
pub enum Attr {
    Int(&'static str, i64),
    Ints(&'static str, Vec<i64>),
    Float(&'static str, f32),
}

/// A graph input or output dimension: a fixed size or a name that ONNX Runtime fills in at run time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dim {
    Fixed(u64),
    Named(&'static str),
}

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

fn key(out: &mut Vec<u8>, field: u64, wire: u64) {
    varint(out, (field << 3) | wire);
}

fn int(out: &mut Vec<u8>, field: u64, v: i64) {
    key(out, field, 0);
    // negative int64s are ten-byte two's-complement varints in protobuf
    varint(out, v as u64);
}

fn bytes(out: &mut Vec<u8>, field: u64, v: &[u8]) {
    key(out, field, 2);
    varint(out, v.len() as u64);
    out.extend_from_slice(v);
}

fn float(out: &mut Vec<u8>, field: u64, v: f32) {
    key(out, field, 5);
    out.extend_from_slice(&v.to_le_bytes());
}

fn value_info(name: &str, dims: &[Dim]) -> Vec<u8> {
    let mut shape = Vec::new();
    for d in dims {
        let mut dim = Vec::new();
        match d {
            Dim::Fixed(n) => int(&mut dim, 1, i64::try_from(*n).unwrap_or(i64::MAX)),
            Dim::Named(s) => bytes(&mut dim, 2, s.as_bytes()),
        }
        bytes(&mut shape, 1, &dim);
    }
    let mut tensor = Vec::new();
    int(&mut tensor, 1, FLOAT as i64);
    bytes(&mut tensor, 2, &shape);
    let mut ty = Vec::new();
    bytes(&mut ty, 1, &tensor);
    let mut vi = Vec::new();
    bytes(&mut vi, 1, name.as_bytes());
    bytes(&mut vi, 2, &ty);
    vi
}

/// An ONNX graph under construction. Names given by [`Graph::node`] are unique (`t0`, `t1`, …); initializer and
/// input names are the caller's and must not start with `t` followed by a digit.
#[derive(Debug, Default)]
pub struct Graph {
    /// Each encoded `NodeProto`, `TensorProto`, input and output `ValueInfoProto`, kept apart so the model is
    /// assembled in one buffer at the end (an initializer's bytes are copied once more, not once per nesting level).
    nodes: Vec<Vec<u8>>,
    initializers: Vec<Vec<u8>>,
    inputs: Vec<Vec<u8>>,
    outputs: Vec<Vec<u8>>,
    next: usize,
}

impl Graph {
    pub fn new() -> Graph {
        Graph::default()
    }

    fn fresh(&mut self) -> String {
        let n = self.next;
        self.next += 1;
        format!("t{n}")
    }

    fn tensor(name: &str, dims: &[usize], data_type: u64, raw: &[u8]) -> Vec<u8> {
        let mut t = Vec::with_capacity(raw.len() + name.len() + 16 + dims.len() * 4);
        for d in dims {
            int(&mut t, 1, i64::try_from(*d).unwrap_or(i64::MAX));
        }
        int(&mut t, 2, data_type as i64);
        bytes(&mut t, 8, name.as_bytes());
        bytes(&mut t, 9, raw);
        t
    }

    /// A float32 constant `name` of shape `dims` (row-major `data`). An error when `data` doesn't fill `dims`.
    pub fn init_f32(&mut self, name: &str, dims: &[usize], data: &[f32]) -> Result<String, String> {
        let n = dims.iter().try_fold(1usize, |a, d| a.checked_mul(*d)).ok_or("initializer shape overflows")?;
        if n != data.len() {
            return Err(format!("initializer {name}: {} values for shape {dims:?}", data.len()));
        }
        let mut raw = Vec::new();
        raw.try_reserve_exact(data.len().saturating_mul(4)).map_err(|_| format!("not enough memory for initializer {name}"))?;
        for v in data {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        self.initializers.push(Graph::tensor(name, dims, FLOAT, &raw));
        Ok(name.to_string())
    }

    /// An int64 constant `name` of shape `dims` (a shape for `Reshape`, indices for `Gather`; `dims` empty: a scalar).
    pub fn init_i64(&mut self, name: &str, dims: &[usize], data: &[i64]) -> Result<String, String> {
        let n = dims.iter().try_fold(1usize, |a, d| a.checked_mul(*d)).ok_or("initializer shape overflows")?;
        if n != data.len() {
            return Err(format!("initializer {name}: {} values for shape {dims:?}", data.len()));
        }
        let raw: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.initializers.push(Graph::tensor(name, dims, INT64, &raw));
        Ok(name.to_string())
    }

    /// A node `op(inputs)` with `attrs`; returns its (single) output's name.
    pub fn node(&mut self, op: &str, inputs: &[&str], attrs: &[Attr]) -> String {
        let out = self.fresh();
        self.node_named(op, inputs, attrs, &out);
        out
    }

    /// A node whose output is named `output` (a graph output).
    pub fn node_named(&mut self, op: &str, inputs: &[&str], attrs: &[Attr], output: &str) {
        let mut n = Vec::new();
        for i in inputs {
            bytes(&mut n, 1, i.as_bytes());
        }
        bytes(&mut n, 2, output.as_bytes());
        bytes(&mut n, 4, op.as_bytes());
        for a in attrs {
            let mut at = Vec::new();
            match a {
                Attr::Int(name, v) => {
                    bytes(&mut at, 1, name.as_bytes());
                    int(&mut at, 3, *v);
                    int(&mut at, 20, 2);
                }
                Attr::Ints(name, vs) => {
                    bytes(&mut at, 1, name.as_bytes());
                    for v in vs {
                        int(&mut at, 8, *v);
                    }
                    int(&mut at, 20, 7);
                }
                Attr::Float(name, v) => {
                    bytes(&mut at, 1, name.as_bytes());
                    float(&mut at, 2, *v);
                    int(&mut at, 20, 1);
                }
            }
            bytes(&mut n, 5, &at);
        }
        self.nodes.push(n);
    }

    /// A float32 graph input.
    pub fn input(&mut self, name: &str, dims: &[Dim]) {
        self.inputs.push(value_info(name, dims));
    }

    /// A float32 graph output.
    pub fn output(&mut self, name: &str, dims: &[Dim]) {
        self.outputs.push(value_info(name, dims));
    }

    /// The model's bytes: IR version 8, the default operator set at `opset`.
    pub fn finish(self, name: &str, opset: i64) -> Result<Vec<u8>, String> {
        let field_len = |len: usize| 1 + varint_len(len as u64) + len;
        let mut graph_len = field_len(name.len());
        for part in self.nodes.iter().chain(&self.initializers).chain(&self.inputs).chain(&self.outputs) {
            graph_len = graph_len.checked_add(field_len(part.len())).ok_or("the model is too large")?;
        }
        let mut opset_msg = Vec::new();
        int(&mut opset_msg, 2, opset);
        let producer = lcc_brand::DISPLAY_NAME.as_bytes();
        let total = 2 + field_len(producer.len()) + field_len(opset_msg.len()) + 1 + varint_len(graph_len as u64) + graph_len;
        let mut model = Vec::new();
        model.try_reserve_exact(total).map_err(|_| "not enough memory to assemble the model".to_string())?;
        int(&mut model, 1, 8);
        bytes(&mut model, 2, producer);
        key(&mut model, 7, 2);
        varint(&mut model, graph_len as u64);
        for n in &self.nodes {
            bytes(&mut model, 1, n);
        }
        bytes(&mut model, 2, name.as_bytes());
        for t in &self.initializers {
            bytes(&mut model, 5, t);
        }
        for i in &self.inputs {
            bytes(&mut model, 11, i);
        }
        for o in &self.outputs {
            bytes(&mut model, 12, o);
        }
        bytes(&mut model, 8, &opset_msg);
        Ok(model)
    }
}

fn varint_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_and_negative_ints_encode_as_protobuf_does() {
        let mut v = Vec::new();
        varint(&mut v, 300);
        assert_eq!(v, [0xac, 0x02]);
        let mut n = Vec::new();
        int(&mut n, 1, -1);
        assert_eq!(n.len(), 11, "a negative int64 is a ten-byte varint after its key");
        assert_eq!(varint_len(0), 1);
        assert_eq!(varint_len(127), 1);
        assert_eq!(varint_len(128), 2);
    }

    #[test]
    fn initializers_must_fill_their_shape() {
        let mut g = Graph::new();
        assert!(g.init_f32("w", &[2, 3], &[0.0; 6]).is_ok());
        assert!(g.init_f32("w", &[2, 3], &[0.0; 5]).is_err());
        assert!(g.init_f32("w", &[usize::MAX, 3], &[]).is_err());
        assert!(g.init_i64("s", &[], &[4]).is_ok());
        assert!(g.init_i64("s", &[2], &[4]).is_err());
    }

    #[test]
    fn a_model_assembles() {
        let mut g = Graph::new();
        let w = g.init_f32("w", &[2], &[1.0, 2.0]).unwrap_or_default();
        g.input("x", &[Dim::Named("n"), Dim::Fixed(2)]);
        let m = g.node("Mul", &["x", &w], &[Attr::Int("a", -3), Attr::Ints("b", vec![1, -1]), Attr::Float("c", 0.5)]);
        g.node_named("Relu", &[&m], &[], "y");
        g.output("y", &[Dim::Named("n"), Dim::Fixed(2)]);
        let bytes = g.finish("test", 17).unwrap_or_default();
        assert!(!bytes.is_empty());
    }
}
