//! A full, bounded read of a small ONNX model: its nodes, attributes and weights.
//!
//! The detector and installed recognisers share this reader and [`crate::net`]. Files are capped at
//! 512 MiB, with at most 64 million weight elements. The bytes are still treated as hostile: the
//! number of nodes, weight tensors and elements is capped, every length is checked against its parent,
//! and a weight blob whose size disagrees with its shape is an error.

use std::collections::HashMap;
use std::io::Cursor;

use crate::onnx::{ProbeError, Walker, Wire};

type Result<T> = std::result::Result<T, ProbeError>;

const MAX_NODES: usize = 20_000;
const MAX_TENSORS: usize = 20_000;
/// Elements across all weight tensors (4 bytes each as float32).
const MAX_ELEMENTS: usize = 64 * 1024 * 1024;
const MAX_NAME: usize = 4096;
const MAX_LIST: usize = 1_000_000;

#[derive(Clone, Debug, PartialEq)]
pub enum Attr {
    Int(i64),
    Float(f32),
    Str(String),
    Ints(Vec<i64>),
    Floats(Vec<f32>),
    /// A kind we do not read (a tensor, a sub-graph…).
    Other,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub op: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub attrs: HashMap<String, Attr>,
}

impl Node {
    pub fn int(&self, name: &str) -> Option<i64> {
        match self.attrs.get(name) {
            Some(Attr::Int(v)) => Some(*v),
            _ => None,
        }
    }
    pub fn ints(&self, name: &str) -> Option<&[i64]> {
        match self.attrs.get(name) {
            Some(Attr::Ints(v)) => Some(v),
            _ => None,
        }
    }
    pub fn string(&self, name: &str) -> Option<&str> {
        match self.attrs.get(name) {
            Some(Attr::Str(v)) => Some(v),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    F32(Vec<f32>),
    I64(Vec<i64>),
}

/// A weight tensor.
#[derive(Clone, Debug, PartialEq)]
pub struct Weight {
    pub dims: Vec<usize>,
    pub data: Data,
}

#[derive(Clone, Debug)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub weights: HashMap<String, Weight>,
    /// The inputs the caller feeds (weights some exporters also list as inputs are left out), and their shapes
    /// (0 for a dynamic size).
    pub inputs: Vec<(String, Vec<usize>)>,
    pub outputs: Vec<String>,
}

fn malformed<T>(why: &'static str) -> Result<T> {
    Err(ProbeError::Malformed(why))
}

fn f32s(raw: &[u8]) -> Vec<f32> {
    raw.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
}

fn i64s(raw: &[u8]) -> Vec<i64> {
    raw.as_chunks::<8>().0.iter().map(|b| i64::from_le_bytes(*b)).collect()
}

/// A list of varints in a packed field (`start..end`), appended to `out`.
fn packed_varints(w: &mut Walker<Cursor<&[u8]>>, end: u64, out: &mut Vec<u64>) -> Result<()> {
    while w.pos()? < end {
        if out.len() >= MAX_LIST {
            return malformed("a list is too long");
        }
        out.push(w.varint()?);
        if w.pos()? > end {
            return malformed("a packed number runs past its field");
        }
    }
    Ok(())
}

fn attribute(w: &mut Walker<Cursor<&[u8]>>, end: u64) -> Result<(String, Attr)> {
    let (mut name, mut kind) = (String::new(), 0u64);
    let (mut i, mut f, mut s) = (0i64, 0f32, String::new());
    let (mut ints, mut floats): (Vec<u64>, Vec<f32>) = (Vec::new(), Vec::new());
    while let Some((n, f_)) = w.next(end)? {
        match (n, f_) {
            (1, Wire::Len { start, end: e }) => {
                name = w.string(start, e)?;
                w.seek_to(e)?;
            }
            (2, Wire::Fixed32(v)) => f = f32::from_bits(v),
            (3, Wire::Varint(v)) => i = v as i64,
            (4, Wire::Len { start, end: e }) => {
                s = w.string(start, e)?;
                w.seek_to(e)?;
            }
            (7, Wire::Len { end: e, .. }) => {
                // packed floats
                let at = w.pos()?;
                let raw = w.bytes(at, e, (MAX_LIST * 4) as u64)?;
                if !raw.len().is_multiple_of(4) || floats.len().saturating_add(raw.len() / 4) > MAX_LIST {
                    return malformed("an attribute float list is invalid or too long");
                }
                floats.extend(f32s(&raw));
            }
            (7, Wire::Fixed32(v)) => {
                if floats.len() >= MAX_LIST {
                    return malformed("an attribute float list is too long");
                }
                floats.push(f32::from_bits(v));
            }
            (8, Wire::Len { end: e, .. }) => {
                packed_varints(w, e, &mut ints)?;
                w.seek_to(e)?;
            }
            (8, Wire::Varint(v)) => {
                if ints.len() >= MAX_LIST {
                    return malformed("a list is too long");
                }
                ints.push(v);
            }
            (20, Wire::Varint(v)) => kind = v,
            (_, Wire::Len { end: e, .. }) => w.seek_to(e)?,
            _ => {}
        }
    }
    let ints = ints.into_iter().map(|v| v as i64).collect();
    let value = match kind {
        1 => Attr::Float(f),
        2 => Attr::Int(i),
        3 => Attr::Str(s),
        6 => Attr::Floats(floats),
        7 => Attr::Ints(ints),
        _ => Attr::Other,
    };
    Ok((name, value))
}

fn node(w: &mut Walker<Cursor<&[u8]>>, end: u64) -> Result<Node> {
    let mut n = Node { op: String::new(), inputs: Vec::new(), outputs: Vec::new(), attrs: HashMap::new() };
    while let Some((num, f)) = w.next(end)? {
        match (num, f) {
            (1, Wire::Len { start, end: e }) => {
                n.inputs.push(w.string(start, e)?);
                w.seek_to(e)?;
            }
            (2, Wire::Len { start, end: e }) => {
                n.outputs.push(w.string(start, e)?);
                w.seek_to(e)?;
            }
            (4, Wire::Len { start, end: e }) => {
                n.op = w.string(start, e)?;
                w.seek_to(e)?;
            }
            (5, Wire::Len { end: e, .. }) => {
                let (k, v) = attribute(w, e)?;
                if k.is_empty() || n.attrs.insert(k, v).is_some() {
                    return malformed("an attribute name is empty or repeated");
                }
                w.seek_to(e)?;
            }
            (7, Wire::Len { start, end: e }) => {
                let domain = w.string(start, e)?;
                if !domain.is_empty() && domain != "ai.onnx" {
                    return malformed("custom operator domains are unsupported");
                }
                w.seek_to(e)?;
            }
            (_, Wire::Len { end: e, .. }) => w.seek_to(e)?,
            _ => {}
        }
        if n.inputs.len() > 64 || n.outputs.len() > 64 || n.attrs.len() > 64 {
            return malformed("a node is too large");
        }
    }
    Ok(n)
}

/// A weight tensor (`TensorProto`): its name, shape and float32 or int64 values.
fn tensor(w: &mut Walker<Cursor<&[u8]>>, end: u64, budget: &mut usize) -> Result<(String, Option<Weight>)> {
    let (mut name, mut dtype) = (String::new(), 0u64);
    let mut dims: Vec<u64> = Vec::new();
    let mut raw: Option<&[u8]> = None;
    let (mut float_data, mut int_data): (Vec<f32>, Vec<u64>) = (Vec::new(), Vec::new());
    let max_bytes = (*budget as u64).saturating_mul(8);
    while let Some((n, f)) = w.next(end)? {
        match (n, f) {
            (1, Wire::Len { end: e, .. }) => {
                packed_varints(w, e, &mut dims)?;
                w.seek_to(e)?;
            }
            (1, Wire::Varint(v)) => dims.push(v),
            (2, Wire::Varint(v)) => dtype = v,
            (4, Wire::Len { end: e, .. }) => {
                let at = w.pos()?;
                let bytes = w.bytes(at, e, max_bytes)?;
                if !bytes.len().is_multiple_of(4) || float_data.len().saturating_add(bytes.len() / 4) > *budget {
                    return malformed("invalid or excessive packed float weights");
                }
                float_data.extend(f32s(&bytes));
            }
            (4, Wire::Fixed32(v)) => {
                if float_data.len() >= *budget {
                    return malformed("too many float weights");
                }
                float_data.push(f32::from_bits(v));
            }
            (7, Wire::Varint(v)) => {
                if int_data.len() >= *budget {
                    return malformed("too many integer weights");
                }
                int_data.push(v);
            }
            (7, Wire::Len { end: e, .. }) => {
                packed_varints(w, e, &mut int_data)?;
                w.seek_to(e)?;
            }
            (8, Wire::Len { start, end: e }) => {
                name = w.string(start, e)?;
                w.seek_to(e)?;
            }
            (9, Wire::Len { start, end: e }) => {
                if raw.is_some() || e.saturating_sub(start) > max_bytes {
                    return malformed("repeated or excessive raw weights");
                }
                let from = usize::try_from(start).map_err(|_| ProbeError::Malformed("weight offset overflow"))?;
                let to = usize::try_from(e).map_err(|_| ProbeError::Malformed("weight offset overflow"))?;
                raw = Some((*w.r.get_ref()).get(from..to).ok_or(ProbeError::Malformed("short raw weights"))?);
                w.seek_to(e)?;
            }
            (13, Wire::Len { .. }) | (14, Wire::Varint(1..)) => return malformed("external tensor data is unsupported"),
            (_, Wire::Len { end: e, .. }) => w.seek_to(e)?,
            _ => {}
        }
        if dims.len() > 8 {
            return malformed("a tensor has too many dimensions");
        }
    }
    let mut elements: usize = 1;
    let mut shape = Vec::new();
    for d in &dims {
        let d = usize::try_from(*d).map_err(|_| ProbeError::Malformed("a tensor dimension is too large"))?;
        elements = elements.checked_mul(d).ok_or(ProbeError::Malformed("a tensor is too large"))?;
        shape.push(d);
    }
    if elements > *budget {
        return malformed("the weights are too large");
    }
    *budget -= elements;
    if raw.is_some() && (!float_data.is_empty() || !int_data.is_empty()) {
        return malformed("ambiguous raw and typed weights");
    }
    let data = match dtype {
        1 => {
            let v = if let Some(raw) = raw {
                if elements.checked_mul(4) != Some(raw.len()) {
                    return malformed("float weight bytes disagree with shape");
                }
                f32s(raw)
            } else {
                float_data
            };
            if v.len() != elements {
                return malformed("a weight tensor's size disagrees with its shape");
            }
            Some(Data::F32(v))
        }
        7 => {
            let v = if let Some(raw) = raw {
                if elements.checked_mul(8) != Some(raw.len()) {
                    return malformed("integer weight bytes disagree with shape");
                }
                i64s(raw)
            } else {
                int_data.into_iter().map(|v| v as i64).collect()
            };
            if v.len() != elements {
                return malformed("a weight tensor's size disagrees with its shape");
            }
            Some(Data::I64(v))
        }
        _ => return malformed("only float32 and int64 initializers are supported"),
    };
    if name.is_empty() {
        return malformed("a weight name is empty");
    }
    Ok((name, data.map(|data| Weight { dims: shape, data })))
}

fn value_info(w: &mut Walker<Cursor<&[u8]>>, end: u64) -> Result<(String, Vec<usize>)> {
    let (mut name, mut shape) = (String::new(), Vec::new());
    while let Some((n, f)) = w.next(end)? {
        match (n, f) {
            (1, Wire::Len { start, end: e }) => {
                name = w.string(start, e)?;
                w.seek_to(e)?;
            }
            (2, Wire::Len { end: e, .. }) => {
                // TypeProto > tensor_type (1) > shape (2) > dim (1) > dim_value (1)
                let mut info = crate::onnx::TensorInfo { name: String::new(), elem_type: 0, shape: Vec::new() };
                crate::onnx::type_proto(w, e, &mut info)?;
                shape = info
                    .shape
                    .iter()
                    .map(|d| match d {
                        crate::onnx::Dim::Fixed(v) => {
                            usize::try_from(*v).map_err(|_| ProbeError::Malformed("a declared dimension exceeds this platform"))
                        }
                        crate::onnx::Dim::Dynamic(_) => Ok(0),
                    })
                    .collect::<Result<_>>()?;
                w.seek_to(e)?;
            }
            (_, Wire::Len { end: e, .. }) => w.seek_to(e)?,
            _ => {}
        }
    }
    if name.len() > MAX_NAME {
        return malformed("a name is too long");
    }
    Ok((name, shape))
}

/// Read a bounded float32 ONNX inference model.
pub fn load(bytes: &[u8]) -> Result<Graph> {
    let total = bytes.len() as u64;
    if total > 512 * 1024 * 1024 {
        return malformed("the model exceeds 512 MiB");
    }
    let info = crate::onnx::probe_reader(&mut Cursor::new(bytes))?;
    if info.opset.is_none_or(|v| !(10..=17).contains(&v)) {
        return malformed("only ONNX default-domain opsets 10 through 17 are supported");
    }
    if info.inputs.iter().chain(&info.outputs).any(|v| v.shape.iter().any(|d| matches!(d, crate::onnx::Dim::Fixed(0)))) {
        return malformed("a declared input/output has a zero dimension");
    }
    if info.inputs.iter().chain(&info.outputs).any(|v| v.elem_type != 1) {
        return malformed("model inputs and outputs must be float32");
    }
    if total == 0 {
        return malformed("the file is empty");
    }
    let mut cursor = Cursor::new(bytes);
    let mut w = Walker { r: &mut cursor, total, fields: 0 };
    let (mut nodes, mut weights, mut inputs, mut outputs) = (Vec::new(), HashMap::new(), Vec::new(), Vec::new());
    let mut budget = MAX_ELEMENTS;
    let mut saw_graph = false;
    while let Some((n, f)) = w.next(total)? {
        match (n, f) {
            (7, Wire::Len { end, .. }) => {
                if saw_graph {
                    return malformed("the model contains multiple graphs");
                }
                saw_graph = true;
                while let Some((gn, gf)) = w.next(end)? {
                    match (gn, gf) {
                        (1, Wire::Len { end: e, .. }) => {
                            if nodes.len() >= MAX_NODES {
                                return malformed("too many nodes");
                            }
                            nodes.push(node(&mut w, e)?);
                            w.seek_to(e)?;
                        }
                        (5, Wire::Len { end: e, .. }) => {
                            if weights.len() >= MAX_TENSORS {
                                return malformed("too many weight tensors");
                            }
                            let (name, wt) = tensor(&mut w, e, &mut budget)?;
                            if let Some(wt) = wt
                                && weights.insert(name, wt).is_some()
                            {
                                return malformed("a weight is defined twice");
                            }
                            w.seek_to(e)?;
                        }
                        (11, Wire::Len { end: e, .. }) => {
                            if inputs.len() >= MAX_TENSORS {
                                return malformed("too many graph inputs");
                            }
                            inputs.push(value_info(&mut w, e)?);
                            w.seek_to(e)?;
                        }
                        (12, Wire::Len { end: e, .. }) => {
                            if outputs.len() >= 64 {
                                return malformed("too many graph outputs");
                            }
                            outputs.push(value_info(&mut w, e)?.0);
                            w.seek_to(e)?;
                        }
                        (15, Wire::Len { .. }) => return malformed("sparse initializers are unsupported"),
                        (_, Wire::Len { end: e, .. }) => w.seek_to(e)?,
                        _ => {}
                    }
                }
                w.seek_to(end)?;
            }
            (_, Wire::Len { end, .. }) => w.seek_to(end)?,
            _ => {}
        }
    }
    if !saw_graph || nodes.is_empty() {
        return malformed("no graph");
    }
    inputs.retain(|(name, _)| !weights.contains_key(name));
    if inputs.is_empty() || outputs.is_empty() {
        return malformed("the graph has no inputs or no outputs");
    }
    Ok(Graph { nodes, weights, inputs, outputs })
}

#[cfg(test)]
mod tests;
