//! Clean-room CLIP byte-level BPE tokenizer (`vocab.json` + `merges.txt`).
//!
//! Built from the CLIP paper (Radford et al., ICML 2021) and the standard CLIP BPE specification:
//! whitespace-normalized lowercase text is split into contractions, letter runs, single digits and punctuation runs;
//! each token's UTF-8 bytes are mapped through the 256-entry printable byte table, the final character of each word
//! carries `</w>`, and adjacent symbol pairs are merged in `merges.txt` rank order before looking up token IDs in
//! `vocab.json`.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use thiserror::Error;

/// Default CLIP context window length (`77` tokens including BOS and EOS).
pub const DEFAULT_CONTEXT_LEN: usize = 77;

/// Maximum accepted `vocab.json` or `merges.txt` size (16 MiB).
pub const MAX_TOKENIZER_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Maximum input query length in bytes accepted by [`Tokenizer::encode`].
pub const MAX_QUERY_BYTES: usize = 8 * 1024;

/// Maximum BPE symbols per pre-token (bounds merge work on adversarial long words).
const MAX_WORD_SYMBOLS: usize = 256;

#[derive(Debug, Error)]
pub enum TokenizerError {
    #[error("cannot read tokenizer file {path}: {message}")]
    Io { path: String, message: String },
    #[error("tokenizer file {path} exceeds {MAX_TOKENIZER_FILE_BYTES} bytes")]
    TooLarge { path: String },
    #[error("invalid vocab.json: {0}")]
    InvalidVocab(String),
    #[error("missing special token {0:?} in vocab.json")]
    MissingSpecialToken(&'static str),
    #[error("invalid merges.txt line {line}: {reason}")]
    InvalidMerge { line: usize, reason: String },
}

/// Tokenized and padded prompt ready for the CLIP text transformer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedText {
    /// Padded token IDs of length `context_len`.
    pub ids: Vec<u32>,
    /// Index of the `<|endoftext|>` token in `ids` (`1..context_len`), used to pool the text representation.
    pub eos_index: usize,
}

/// CLIP byte-level BPE tokenizer.
#[derive(Debug, Clone)]
pub struct Tokenizer {
    vocab: HashMap<String, u32>,
    bpe_ranks: HashMap<(String, String), usize>,
    byte_encoder: [char; 256],
    bos_id: u32,
    eos_id: u32,
    context_len: usize,
}

impl Tokenizer {
    /// Load `vocab.json` and `merges.txt` from `dir`.
    pub fn load(dir: &Path) -> Result<Self, TokenizerError> {
        let vocab_path = dir.join("vocab.json");
        let merges_path = dir.join("merges.txt");
        let vocab_bytes = read_bounded(&vocab_path)?;
        let merges_bytes = read_bounded(&merges_path)?;
        let merges_str = std::str::from_utf8(&merges_bytes).map_err(|e| TokenizerError::InvalidMerge { line: 0, reason: e.to_string() })?;
        Self::from_json_and_merges(&vocab_bytes, merges_str, DEFAULT_CONTEXT_LEN)
    }

    /// Parse `vocab.json` bytes and `merges.txt` text with a custom `context_len`.
    pub fn from_json_and_merges(vocab_json: &[u8], merges_txt: &str, context_len: usize) -> Result<Self, TokenizerError> {
        if vocab_json.len() as u64 > MAX_TOKENIZER_FILE_BYTES {
            return Err(TokenizerError::TooLarge { path: "vocab.json".into() });
        }
        if merges_txt.len() as u64 > MAX_TOKENIZER_FILE_BYTES {
            return Err(TokenizerError::TooLarge { path: "merges.txt".into() });
        }
        let vocab: HashMap<String, u32> = serde_json::from_slice(vocab_json).map_err(|e| TokenizerError::InvalidVocab(e.to_string()))?;
        if vocab.is_empty() {
            return Err(TokenizerError::InvalidVocab("empty vocabulary".into()));
        }
        let bos_id = *vocab.get("<|startoftext|>").ok_or(TokenizerError::MissingSpecialToken("<|startoftext|>"))?;
        let eos_id = *vocab.get("<|endoftext|>").ok_or(TokenizerError::MissingSpecialToken("<|endoftext|>"))?;

        let mut bpe_ranks = HashMap::new();
        let mut rank = 0usize;
        for (line_idx, raw_line) in merges_txt.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with("#version") {
                continue;
            }
            let mut parts = line.split_whitespace();
            let Some(first) = parts.next() else {
                continue;
            };
            let Some(second) = parts.next() else {
                return Err(TokenizerError::InvalidMerge { line: line_idx + 1, reason: "expected two whitespace-separated tokens".into() });
            };
            if parts.next().is_some() {
                return Err(TokenizerError::InvalidMerge { line: line_idx + 1, reason: "unexpected third column in merge rule".into() });
            }
            bpe_ranks.entry((first.to_string(), second.to_string())).or_insert(rank);
            rank += 1;
        }

        Ok(Self { vocab, bpe_ranks, byte_encoder: build_byte_encoder(), bos_id, eos_id, context_len: context_len.max(4) })
    }

    pub fn bos_id(&self) -> u32 {
        self.bos_id
    }

    pub fn eos_id(&self) -> u32 {
        self.eos_id
    }

    pub fn context_len(&self) -> usize {
        self.context_len
    }

    pub fn vocab_size(&self) -> usize {
        self.vocab.len()
    }

    /// Encode `text` into a padded sequence of length `context_len` with `<|startoftext|>` at index 0 and
    /// `<|endoftext|>` at `eos_index`.
    pub fn encode(&self, text: &str) -> EncodedText {
        let capped = clamp_utf8(text, MAX_QUERY_BYTES);
        let cleaned = clean_whitespace_and_lowercase(capped);
        let max_body = self.context_len.saturating_sub(2);
        let mut ids = Vec::with_capacity(self.context_len);
        ids.push(self.bos_id);

        'words: for word in pre_tokenize(&cleaned) {
            if ids.len() > max_body {
                break;
            }
            if word == "<|startoftext|>" || word == "<|endoftext|>" {
                continue;
            }
            let mut encoded_chars = Vec::with_capacity(word.len().min(MAX_WORD_SYMBOLS));
            for &b in word.as_bytes().iter().take(MAX_WORD_SYMBOLS) {
                encoded_chars.push(self.byte_encoder[b as usize]);
            }
            if encoded_chars.is_empty() {
                continue;
            }
            let mut symbols: Vec<String> = encoded_chars.iter().map(|c| c.to_string()).collect();
            if let Some(last) = symbols.last_mut() {
                last.push_str("</w>");
            }
            let merged = self.bpe_merge(symbols);
            for piece in merged {
                if let Some(&id) = self.vocab.get(&piece) {
                    ids.push(id);
                    if ids.len() > max_body {
                        break 'words;
                    }
                }
            }
        }

        let eos_index = ids.len().min(self.context_len.saturating_sub(1));
        ids.push(self.eos_id);
        ids.resize(self.context_len, self.eos_id);
        EncodedText { ids, eos_index }
    }

    fn bpe_merge(&self, mut word: Vec<String>) -> Vec<String> {
        while word.len() >= 2 {
            let mut best_rank = usize::MAX;
            let mut best_pair: Option<(String, String)> = None;
            for pair in word.windows(2) {
                let key = (pair[0].clone(), pair[1].clone());
                if let Some(&r) = self.bpe_ranks.get(&key)
                    && r < best_rank
                {
                    best_rank = r;
                    best_pair = Some(key);
                }
            }
            let Some((first, second)) = best_pair else {
                break;
            };
            let mut next = Vec::with_capacity(word.len());
            let mut i = 0usize;
            while i < word.len() {
                if i + 1 < word.len() && word[i] == first && word[i + 1] == second {
                    let mut combined = first.clone();
                    combined.push_str(&second);
                    next.push(combined);
                    i += 2;
                } else {
                    if let Some(s) = word.get(i) {
                        next.push(s.clone());
                    }
                    i += 1;
                }
            }
            word = next;
        }
        word
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, TokenizerError> {
    let file = std::fs::File::open(path).map_err(|e| TokenizerError::Io { path: path.display().to_string(), message: e.to_string() })?;
    let mut bytes = Vec::new();
    file.take(MAX_TOKENIZER_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| TokenizerError::Io { path: path.display().to_string(), message: e.to_string() })?;
    if bytes.len() as u64 > MAX_TOKENIZER_FILE_BYTES {
        return Err(TokenizerError::TooLarge { path: path.display().to_string() });
    }
    Ok(bytes)
}

fn clamp_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or("")
}

fn clean_whitespace_and_lowercase(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
            prev_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Split lowercase text according to CLIP's pattern:
/// `<|startoftext|>|<|endoftext|>|'s|'t|'re|'ve|'m|'ll|'d|[\p{L}]+|[\p{N}]|[^\s\p{L}\p{N}]+`
fn pre_tokenize(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < text.len() {
        let Some(rest) = text.get(i..) else {
            break;
        };
        let Some(first) = rest.chars().next() else {
            break;
        };
        if first.is_whitespace() {
            i += first.len_utf8();
            continue;
        }
        // Special tokens
        for special in ["<|startoftext|>", "<|endoftext|>"] {
            if rest.starts_with(special) {
                tokens.push(special);
                i += special.len();
                continue;
            }
        }
        if rest.starts_with("<|startoftext|>") || rest.starts_with("<|endoftext|>") {
            continue;
        }
        // Contractions
        if first == '\'' {
            let mut matched_contraction: Option<&str> = None;
            for c in ["'re", "'ve", "'ll", "'s", "'t", "'m", "'d"] {
                if rest.starts_with(c) {
                    matched_contraction = Some(c);
                    break;
                }
            }
            if let Some(c) = matched_contraction {
                tokens.push(c);
                i += c.len();
                continue;
            }
        }
        // Letter sequence [\p{L}]+
        if first.is_alphabetic() {
            let mut end = i + first.len_utf8();
            while let Some(sub) = text.get(end..) {
                let Some(ch) = sub.chars().next() else {
                    break;
                };
                if ch.is_alphabetic() {
                    end += ch.len_utf8();
                } else {
                    break;
                }
            }
            if let Some(slice) = text.get(i..end) {
                tokens.push(slice);
            }
            i = end;
            continue;
        }
        // Single numeric character [\p{N}]
        if first.is_numeric() {
            let end = i + first.len_utf8();
            if let Some(slice) = text.get(i..end) {
                tokens.push(slice);
            }
            i = end;
            continue;
        }
        // Non-whitespace, non-letter, non-numeric run [^\s\p{L}\p{N}]+
        let mut end = i + first.len_utf8();
        while let Some(sub) = text.get(end..) {
            let Some(ch) = sub.chars().next() else {
                break;
            };
            if !ch.is_whitespace() && !ch.is_alphabetic() && !ch.is_numeric() {
                end += ch.len_utf8();
            } else {
                break;
            }
        }
        if let Some(slice) = text.get(i..end) {
            tokens.push(slice);
        }
        i = end;
    }
    tokens
}

/// Standard 256-entry printable byte-to-Unicode lookup table used by GPT-2 / CLIP BPE.
fn build_byte_encoder() -> [char; 256] {
    let mut table = ['\0'; 256];
    let mut extra = 0u32;
    for b in 0u16..256 {
        let direct = (b'!' as u16..=b'~' as u16).contains(&b) || (0xA1..=0xAC).contains(&b) || (0xAE..=0xFF).contains(&b);
        let ch = if direct {
            char::from_u32(b as u32).unwrap_or('\u{FFFD}')
        } else {
            let c = char::from_u32(256 + extra).unwrap_or('\u{FFFD}');
            extra += 1;
            c
        };
        table[b as usize] = ch;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tokenizer() -> Tokenizer {
        let vocab_json = br#"{
            "s": 1,
            "u": 2,
            "n": 3,
            "s</w>": 4,
            "n</w>": 5,
            "su": 6,
            "sun</w>": 7,
            "cat</w>": 8,
            "c": 9,
            "a": 10,
            "t</w>": 11,
            "ca": 12,
            "<|startoftext|>": 13,
            "<|endoftext|>": 14
        }"#;
        let merges = "#version: 0.2\ns u\nsu n</w>\nc a\nca t</w>\n";
        Tokenizer::from_json_and_merges(vocab_json, merges, 8).unwrap()
    }

    #[test]
    fn merges_bpe_tokens_and_tracks_eos_position() {
        let tok = sample_tokenizer();
        let enc = tok.encode("  Sun   CAT  ");
        assert_eq!(enc.ids.len(), 8);
        assert_eq!(enc.ids[0], 13); // BOS
        assert_eq!(enc.ids[1], 7); // "sun</w>"
        assert_eq!(enc.ids[2], 8); // "cat</w>"
        assert_eq!(enc.ids[3], 14); // EOS
        assert_eq!(enc.eos_index, 3);
    }

    #[test]
    fn truncates_long_queries_cleanly_without_panicking() {
        let tok = sample_tokenizer();
        let long = "sun cat ".repeat(2000);
        let enc = tok.encode(&long);
        assert_eq!(enc.ids.len(), 8);
        assert_eq!(enc.ids[0], 13);
        assert_eq!(enc.eos_index, 7);
        assert_eq!(enc.ids[enc.eos_index], 14);
    }

    #[test]
    fn rejects_invalid_tokenizer_files() {
        assert!(Tokenizer::from_json_and_merges(b"{}", "", 8).is_err());
        let bad_merges = "a\n";
        let vocab = br#"{"<|startoftext|>": 0, "<|endoftext|>": 1}"#;
        assert!(Tokenizer::from_json_and_merges(vocab, bad_merges, 8).is_err());
    }
}
