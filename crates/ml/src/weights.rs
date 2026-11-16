//! Clean-room positional `.safetensors` reader with strict shape, bounds and finiteness validation.
//!
//! Only the JSON header is held in memory when a checkpoint is opened; each tensor is read by offset on demand,
//! checked for exact byte length and finite `f32` values, and placed onto the target [`candle_core::Device`].

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use candle_core::{Device as CandleDevice, Tensor};
use serde::Deserialize;
use thiserror::Error;

/// Maximum accepted `.safetensors` JSON header size (16 MiB).
pub const MAX_SAFETENSORS_HEADER_BYTES: u64 = 16 * 1024 * 1024;

/// Maximum accepted single tensor payload size (512 MiB).
pub const MAX_TENSOR_BYTES: u64 = 512 * 1024 * 1024;

/// Maximum number of tensors allowed in one `.safetensors` header.
pub const MAX_TENSOR_COUNT: usize = 4096;

#[derive(Debug, Error)]
pub enum WeightsError {
    #[error("cannot read weights file: {0}")]
    Io(String),
    #[error("safetensors header length {0} is invalid or exceeds {MAX_SAFETENSORS_HEADER_BYTES} bytes")]
    InvalidHeaderLength(u64),
    #[error("invalid safetensors header JSON: {0}")]
    HeaderJson(String),
    #[error("safetensors header declares too many tensors ({0} > {MAX_TENSOR_COUNT})")]
    TooManyTensors(usize),
    #[error("unsupported tensor dtype {dtype:?} for {name:?}")]
    UnsupportedDtype { name: String, dtype: String },
    #[error("tensor {name:?} has invalid data offsets [{start}, {end}] (file data size {data_len})")]
    OutOfBounds { name: String, start: u64, end: u64, data_len: u64 },
    #[error("tensor {name:?} byte length {actual} does not match shape {shape:?} ({expected} bytes)")]
    ByteSizeMismatch { name: String, shape: Vec<usize>, expected: u64, actual: u64 },
    #[error("missing required tensor {0:?}")]
    MissingTensor(String),
    #[error("tensor {name:?} has shape {found:?}, expected {expected:?}")]
    ShapeMismatch { name: String, expected: Vec<usize>, found: Vec<usize> },
    #[error("tensor {name:?} contains NaN or infinite weights")]
    NonFiniteWeights { name: String },
    #[error("device tensor error on {name:?}: {message}")]
    Device { name: String, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dtype {
    F32,
    F16,
    Bf16,
    F64,
    I64,
    U64,
    I32,
    U32,
    I16,
    U16,
    I8,
    U8,
    Bool,
}

impl Dtype {
    fn from_str(name: &str, s: &str) -> Result<Self, WeightsError> {
        match s {
            "F32" => Ok(Self::F32),
            "F16" => Ok(Self::F16),
            "BF16" => Ok(Self::Bf16),
            "F64" => Ok(Self::F64),
            "I64" => Ok(Self::I64),
            "U64" => Ok(Self::U64),
            "I32" => Ok(Self::I32),
            "U32" => Ok(Self::U32),
            "I16" => Ok(Self::I16),
            "U16" => Ok(Self::U16),
            "I8" => Ok(Self::I8),
            "U8" => Ok(Self::U8),
            "BOOL" => Ok(Self::Bool),
            other => Err(WeightsError::UnsupportedDtype { name: name.to_string(), dtype: other.to_string() }),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::F32 => "F32",
            Self::F16 => "F16",
            Self::Bf16 => "BF16",
            Self::F64 => "F64",
            Self::I64 => "I64",
            Self::U64 => "U64",
            Self::I32 => "I32",
            Self::U32 => "U32",
            Self::I16 => "I16",
            Self::U16 => "U16",
            Self::I8 => "I8",
            Self::U8 => "U8",
            Self::Bool => "BOOL",
        }
    }

    fn bytes_per_elem(self) -> u64 {
        match self {
            Self::F64 | Self::I64 | Self::U64 => 8,
            Self::F32 | Self::I32 | Self::U32 => 4,
            Self::F16 | Self::Bf16 | Self::I16 | Self::U16 => 2,
            Self::I8 | Self::U8 | Self::Bool => 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TensorInfo {
    pub dtype: Dtype,
    pub shape: Vec<usize>,
    pub start: u64,
    pub end: u64,
}

#[derive(Deserialize)]
struct RawEntry {
    dtype: String,
    shape: Vec<usize>,
    data_offsets: [u64; 2],
}

/// Validated `.safetensors` file handle supporting positional reads.
pub struct SafetensorsFile {
    file: File,
    data_start: u64,
    entries: HashMap<String, TensorInfo>,
}

impl SafetensorsFile {
    /// Open a `.safetensors` file and validate its header and all declared tensor spans against the file size.
    pub fn open(path: &Path) -> Result<Self, WeightsError> {
        let mut file = File::open(path).map_err(|e| WeightsError::Io(e.to_string()))?;
        let file_len = file.metadata().map_err(|e| WeightsError::Io(e.to_string()))?.len();
        if file_len < 8 {
            return Err(WeightsError::InvalidHeaderLength(file_len));
        }
        let mut len_buf = [0u8; 8];
        file.read_exact(&mut len_buf).map_err(|e| WeightsError::Io(e.to_string()))?;
        let header_len = u64::from_le_bytes(len_buf);
        if header_len == 0 || header_len > MAX_SAFETENSORS_HEADER_BYTES {
            return Err(WeightsError::InvalidHeaderLength(header_len));
        }
        let data_start = 8u64.checked_add(header_len).ok_or(WeightsError::InvalidHeaderLength(header_len))?;
        if data_start > file_len {
            return Err(WeightsError::InvalidHeaderLength(header_len));
        }
        let data_len = file_len - data_start;

        let mut header_bytes = vec![0u8; header_len as usize];
        file.read_exact(&mut header_bytes).map_err(|e| WeightsError::Io(e.to_string()))?;
        let raw_map: HashMap<String, serde_json::Value> =
            serde_json::from_slice(&header_bytes).map_err(|e| WeightsError::HeaderJson(e.to_string()))?;
        if raw_map.len() > MAX_TENSOR_COUNT {
            return Err(WeightsError::TooManyTensors(raw_map.len()));
        }

        let mut entries = HashMap::with_capacity(raw_map.len());
        for (name, val) in raw_map {
            if name == "__metadata__" {
                continue;
            }
            let raw: RawEntry = serde_json::from_value(val).map_err(|e| WeightsError::HeaderJson(format!("{name}: {e}")))?;
            let dtype = Dtype::from_str(&name, &raw.dtype)?;
            let [start, end] = raw.data_offsets;
            if start > end || end > data_len || (end - start) > MAX_TENSOR_BYTES {
                return Err(WeightsError::OutOfBounds { name, start, end, data_len });
            }
            let elem_count = raw.shape.iter().try_fold(1u64, |acc, &d| acc.checked_mul(d as u64));
            let expected_bytes = elem_count.and_then(|c| c.checked_mul(dtype.bytes_per_elem())).ok_or_else(|| WeightsError::ByteSizeMismatch {
                name: name.clone(),
                shape: raw.shape.clone(),
                expected: u64::MAX,
                actual: end - start,
            })?;
            if end - start != expected_bytes {
                return Err(WeightsError::ByteSizeMismatch { name, shape: raw.shape, expected: expected_bytes, actual: end - start });
            }
            entries.insert(name, TensorInfo { dtype, shape: raw.shape, start, end });
        }

        Ok(Self { file, data_start, entries })
    }

    pub fn info(&self, name: &str) -> Option<&TensorInfo> {
        self.entries.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Read a tensor by name, verify its exact shape and finiteness, and materialize it on `device` as `F32`.
    pub fn load_tensor(&mut self, name: &str, expected_shape: &[usize], device: &CandleDevice) -> Result<Tensor, WeightsError> {
        let info = self.entries.get(name).ok_or_else(|| WeightsError::MissingTensor(name.to_string()))?.clone();
        if info.shape != expected_shape {
            return Err(WeightsError::ShapeMismatch { name: name.to_string(), expected: expected_shape.to_vec(), found: info.shape });
        }
        let byte_len = (info.end - info.start) as usize;
        let offset = self.data_start.checked_add(info.start).ok_or_else(|| WeightsError::Io("offset overflow".into()))?;
        self.file.seek(SeekFrom::Start(offset)).map_err(|e| WeightsError::Io(e.to_string()))?;
        let mut raw = vec![0u8; byte_len];
        self.file.read_exact(&mut raw).map_err(|e| WeightsError::Io(e.to_string()))?;

        let floats = decode_to_f32(name, info.dtype, &raw)?;
        Tensor::from_vec(floats, expected_shape, device).map_err(|e| WeightsError::Device { name: name.to_string(), message: e.to_string() })
    }
}

fn decode_to_f32(name: &str, dtype: Dtype, raw: &[u8]) -> Result<Vec<f32>, WeightsError> {
    match dtype {
        Dtype::F32 => {
            let mut out = Vec::with_capacity(raw.len() / 4);
            for &chunk in raw.as_chunks::<4>().0 {
                let v = f32::from_le_bytes(chunk);
                if !v.is_finite() {
                    return Err(WeightsError::NonFiniteWeights { name: name.to_string() });
                }
                out.push(v);
            }
            Ok(out)
        }
        Dtype::F16 => {
            let mut out = Vec::with_capacity(raw.len() / 2);
            for &chunk in raw.as_chunks::<2>().0 {
                let bits = u16::from_le_bytes(chunk);
                let v = f16_to_f32(bits);
                if !v.is_finite() {
                    return Err(WeightsError::NonFiniteWeights { name: name.to_string() });
                }
                out.push(v);
            }
            Ok(out)
        }
        Dtype::Bf16 => {
            let mut out = Vec::with_capacity(raw.len() / 2);
            for &chunk in raw.as_chunks::<2>().0 {
                let bits = u16::from_le_bytes(chunk);
                let v = f32::from_bits((bits as u32) << 16);
                if !v.is_finite() {
                    return Err(WeightsError::NonFiniteWeights { name: name.to_string() });
                }
                out.push(v);
            }
            Ok(out)
        }
        Dtype::F64 => {
            let mut out = Vec::with_capacity(raw.len() / 8);
            for &chunk in raw.as_chunks::<8>().0 {
                let v = f64::from_le_bytes(chunk) as f32;
                if !v.is_finite() {
                    return Err(WeightsError::NonFiniteWeights { name: name.to_string() });
                }
                out.push(v);
            }
            Ok(out)
        }
        other => Err(WeightsError::UnsupportedDtype { name: name.to_string(), dtype: other.as_str().to_string() }),
    }
}

/// Pure-Rust IEEE 754 binary16 to binary32 conversion.
fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits & 0x8000) as u32) << 16;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let mant = (bits & 0x03ff) as u32;
    if exp == 0 {
        if mant == 0 {
            return f32::from_bits(sign);
        }
        // Subnormal f16
        let val = (mant as f32) * (1.0 / 16_777_216.0);
        return if sign != 0 { -val } else { val };
    }
    if exp == 0x1f {
        return if mant == 0 { f32::from_bits(sign | 0x7f80_0000) } else { f32::NAN };
    }
    let f32_exp = (exp + (127 - 15)) << 23;
    let f32_mant = mant << 13;
    f32::from_bits(sign | f32_exp | f32_mant)
}

/// Serialize a list of `(name, shape, f32_values)` tensors into a valid `.safetensors` byte buffer.
pub fn encode_f32_safetensors(tensors: &[(&str, &[usize], &[f32])]) -> Vec<u8> {
    let mut header_map = serde_json::Map::new();
    let mut offset = 0u64;
    for &(name, shape, data) in tensors {
        let byte_len = (data.len() as u64) * 4;
        let end = offset + byte_len;
        let mut obj = serde_json::Map::new();
        obj.insert("dtype".into(), serde_json::Value::String("F32".into()));
        obj.insert("shape".into(), serde_json::Value::Array(shape.iter().map(|&d| serde_json::Value::Number((d as u64).into())).collect()));
        obj.insert(
            "data_offsets".into(),
            serde_json::Value::Array(vec![serde_json::Value::Number(offset.into()), serde_json::Value::Number(end.into())]),
        );
        header_map.insert(name.to_string(), serde_json::Value::Object(obj));
        offset = end;
    }
    let header_bytes = serde_json::to_vec(&serde_json::Value::Object(header_map)).unwrap_or_default();
    let mut out = Vec::with_capacity(8 + header_bytes.len() + (offset as usize));
    out.extend_from_slice(&(header_bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(&header_bytes);
    for &(_, _, data) in tensors {
        for &v in data {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_reads_f32_safetensors_and_rejects_nan() {
        let dir = std::env::temp_dir().join(format!("lcc-ml-weights-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let ok_path = dir.join("ok.safetensors");
        let bytes = encode_f32_safetensors(&[("w", &[2, 3], &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0])]);
        std::fs::write(&ok_path, &bytes).unwrap();

        let mut st = SafetensorsFile::open(&ok_path).unwrap();
        let t = st.load_tensor("w", &[2, 3], &CandleDevice::Cpu).unwrap();
        assert_eq!(t.dims(), &[2, 3]);

        // Shape mismatch
        assert!(matches!(st.load_tensor("w", &[3, 2], &CandleDevice::Cpu), Err(WeightsError::ShapeMismatch { .. })));

        // NaN weight rejection
        let nan_path = dir.join("nan.safetensors");
        let nan_bytes = encode_f32_safetensors(&[("w", &[2], &[1.0, f32::NAN])]);
        std::fs::write(&nan_path, &nan_bytes).unwrap();
        let mut st_nan = SafetensorsFile::open(&nan_path).unwrap();
        assert!(matches!(st_nan.load_tensor("w", &[2], &CandleDevice::Cpu), Err(WeightsError::NonFiniteWeights { .. })));

        // Huge header length rejection
        let bad_path = dir.join("huge.safetensors");
        std::fs::write(&bad_path, (u64::MAX).to_le_bytes()).unwrap();
        assert!(matches!(SafetensorsFile::open(&bad_path), Err(WeightsError::InvalidHeaderLength(_))));

        // Checkpoints with I64 index buffers (`position_ids`) alongside F32 weights open cleanly
        let i64_path = dir.join("with_i64.safetensors");
        let header = serde_json::json!({
            "vision_model.embeddings.position_ids": {
                "dtype": "I64",
                "shape": [1, 2],
                "data_offsets": [0, 16]
            },
            "w": {
                "dtype": "F32",
                "shape": [2],
                "data_offsets": [16, 24]
            }
        });
        let h_bytes = serde_json::to_vec(&header).unwrap();
        let mut raw = Vec::new();
        raw.extend_from_slice(&(h_bytes.len() as u64).to_le_bytes());
        raw.extend_from_slice(&h_bytes);
        raw.extend_from_slice(&0i64.to_le_bytes());
        raw.extend_from_slice(&1i64.to_le_bytes());
        raw.extend_from_slice(&1.5f32.to_le_bytes());
        raw.extend_from_slice(&2.5f32.to_le_bytes());
        std::fs::write(&i64_path, &raw).unwrap();
        let mut st_i64 = SafetensorsFile::open(&i64_path).unwrap();
        let w = st_i64.load_tensor("w", &[2], &CandleDevice::Cpu).unwrap();
        assert_eq!(w.dims(), &[2]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
