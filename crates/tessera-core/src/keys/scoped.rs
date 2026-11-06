// SPDX-License-Identifier: GPL-3.0-or-later

//! Scoped search keys, byte for byte as `typesense-js` makes them.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_PAD_INDIFFERENT};
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde_json::{Map, Number, Value};
use sha2::Sha256;

/// Length of the base64 text of a digest.
const DIGEST_LEN: usize = 44;
/// Where the parameters start in a decoded key.
const TEXT_AT: usize = DIGEST_LEN + 4;
/// The largest integer JavaScript prints exactly.
const MAX_SAFE: u64 = 9_007_199_254_740_991;
/// The parameters `typesense-js` joins with commas.
const ARRAYABLE: [&str; 16] = [
    "query_by",
    "query_by_weights",
    "facet_by",
    "group_by",
    "include_fields",
    "exclude_fields",
    "highlight_fields",
    "highlight_full_fields",
    "pinned_hits",
    "hidden_hits",
    "infix",
    "override_tags",
    "num_typos",
    "prefix",
    "synonym_sets",
    "sort_by",
];

/// Why no key can be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopedError {
    /// The parameters are no JSON object.
    NotAnObject,
    /// See [`ParentProblem`].
    Parent(ParentProblem),
    /// A number JavaScript would print differently from Rust.
    Number,
    /// An object key JavaScript would move to the front.
    IndexKey,
    /// This parameter is joined, and holds something a join cannot print.
    ArrayItem(String),
    /// `expires_at` is no integer, which the server refuses.
    ExpiresAt,
}

/// Why a key cannot sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentProblem {
    /// Under 4 bytes.
    Short,
    /// The server takes four bytes and the libraries four characters.
    NotAscii,
}

/// Why a text is no scoped key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// It is not base64.
    Base64,
    /// It ends before the parameters.
    TooShort,
    /// A part is not text.
    NotUtf8,
    /// The parameters are not JSON.
    NotJson,
}

/// The parts of a scoped key.
#[derive(Clone, PartialEq, Eq)]
pub struct DecodedKey {
    /// The base64 text of the signature.
    pub digest: String,
    /// The first 4 characters of the parent.
    pub prefix: String,
    /// The parameters as signed.
    pub text: String,
    /// The parameters.
    pub params: Value,
}

// No digest: with the parameters it makes the key again.
impl fmt::Debug for DecodedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodedKey")
            .field("prefix", &self.prefix)
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

fn is_index(key: &str) -> bool {
    key.parse::<u32>()
        .is_ok_and(|n| n < u32::MAX && n.to_string() == key)
}

fn string(text: &str) -> String {
    // Serializing a `str` cannot fail.
    serde_json::to_string(text).unwrap_or_default()
}

/// `n` as `JSON.stringify` prints it, or refused where that is not certain.
fn number(n: &Number) -> Result<String, ScopedError> {
    if let Some(i) = n.as_i64() {
        return (i.unsigned_abs() <= MAX_SAFE)
            .then(|| i.to_string())
            .ok_or(ScopedError::Number);
    }
    if n.is_u64() {
        return Err(ScopedError::Number);
    }
    let x = n.as_f64().ok_or(ScopedError::Number)?;
    if x == 0.0 {
        Ok("0".to_string())
    } else if x.is_finite() && (1e-6..1e21).contains(&x.abs()) {
        Ok(format!("{x}"))
    } else {
        Err(ScopedError::Number)
    }
}

fn write_value(value: &Value, out: &mut String) -> Result<(), ScopedError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n)?),
        Value::String(s) => out.push_str(&string(s)),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => write_object(map, |_, v, out| write_value(v, out), out)?,
    }
    Ok(())
}

fn write_object(
    map: &Map<String, Value>,
    write: impl Fn(&str, &Value, &mut String) -> Result<(), ScopedError>,
    out: &mut String,
) -> Result<(), ScopedError> {
    out.push('{');
    for (i, (key, value)) in map.iter().enumerate() {
        if is_index(key) {
            return Err(ScopedError::IndexKey);
        }
        if i > 0 {
            out.push(',');
        }
        out.push_str(&string(key));
        out.push(':');
        write(key, value, out)?;
    }
    out.push('}');
    Ok(())
}

/// The join of `items`, as `Array.prototype.join` prints it.
fn joined(name: &str, items: &[Value]) -> Result<String, ScopedError> {
    let item = |value: &Value| match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => number(n),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(ScopedError::ArrayItem(name.to_string())),
    };
    let items: Vec<String> = items.iter().map(item).collect::<Result<_, _>>()?;
    Ok(items.join(","))
}

/// The text that gets signed: `input` with arrayable arrays joined, and no whitespace.
pub fn canonical_params(input: &Value) -> Result<String, ScopedError> {
    let Value::Object(map) = input else {
        return Err(ScopedError::NotAnObject);
    };
    let top = |key: &str, value: &Value, out: &mut String| match value {
        Value::Array(items) if ARRAYABLE.contains(&key) => {
            out.push_str(&string(&joined(key, items)?));
            Ok(())
        }
        Value::Number(n) if key == "expires_at" && (n.is_i64() || n.is_u64()) => {
            write_value(value, out)
        }
        _ if key == "expires_at" => Err(ScopedError::ExpiresAt),
        _ => write_value(value, out),
    };
    let mut out = String::new();
    write_object(map, top, &mut out)?;
    Ok(out)
}

fn signer(parent: &str) -> Hmac<Sha256> {
    let mac = Hmac::<Sha256>::new_from_slice(parent.as_bytes());
    mac.unwrap_or_else(|_| unreachable!("HMAC takes a key of any length"))
}

/// A scoped key of `parent` with `params` embedded.
pub fn generate(parent: &str, params: &Value) -> Result<String, ScopedError> {
    if parent.len() < 4 {
        return Err(ScopedError::Parent(ParentProblem::Short));
    }
    if !parent.is_ascii() {
        return Err(ScopedError::Parent(ParentProblem::NotAscii));
    }
    let text = canonical_params(params)?;
    let mut mac = signer(parent);
    mac.update(text.as_bytes());
    let digest = STANDARD.encode(mac.finalize().into_bytes());
    Ok(STANDARD.encode(format!("{digest}{}{text}", &parent[..4])))
}

/// The parts of `scoped`, with or without padding.
pub fn decode(scoped: &str) -> Result<DecodedKey, DecodeError> {
    let bytes = STANDARD_PAD_INDIFFERENT
        .decode(scoped.trim())
        .map_err(|_| DecodeError::Base64)?;
    if bytes.len() < TEXT_AT + 2 {
        return Err(DecodeError::TooShort);
    }
    let part = |bytes: &[u8]| String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::NotUtf8);
    let digest = part(&bytes[..DIGEST_LEN])?;
    let prefix = part(&bytes[DIGEST_LEN..TEXT_AT])?;
    let text = part(&bytes[TEXT_AT..])?;
    let params: Value = serde_json::from_str(&text).map_err(|_| DecodeError::NotJson)?;
    Ok(DecodedKey {
        digest,
        prefix,
        text,
        params,
    })
}

impl DecodedKey {
    /// The embedded `expires_at`: `None` without one, `Some(None)` when it is no whole number
    /// a server could hold.
    pub fn expires_at(&self) -> Option<Option<i64>> {
        self.params.get("expires_at").map(Value::as_i64)
    }

    /// Whether `parent` signed this key: over the stored text, so a key of any library holds.
    pub fn verify(&self, parent: &str) -> bool {
        let Ok(digest) = STANDARD.decode(&self.digest) else {
            return false;
        };
        let mut mac = signer(parent);
        mac.update(self.text.as_bytes());
        parent.as_bytes().get(..4) == Some(self.prefix.as_bytes())
            && mac.verify_slice(&digest).is_ok()
    }
}
