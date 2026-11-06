// SPDX-License-Identifier: GPL-3.0-or-later

//! Scoped keys against vectors of `typesense-js`.

// Tests unwrap; a panic is the failure we want.
#![allow(clippy::unwrap_used)]

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use serde::Deserialize;
use serde_json::{Value, json};
use tessera_core::keys::scoped::{
    DecodeError, ParentProblem, ScopedError, canonical_params, decode, generate,
};

const VECTORS: &str = include_str!("fixtures/scoped_keys.json");
const PARENT: &str = "tessera-parent-0001";

#[derive(Deserialize)]
struct Vector {
    parent: String,
    input: String,
    params: String,
    scoped: String,
}

fn vectors() -> Vec<Vector> {
    serde_json::from_str(VECTORS).unwrap()
}

/// A key signed over `text`, as any library could make it.
fn by_hand(parent: &str, text: &str) -> String {
    use hmac::{Hmac, KeyInit as _, Mac as _};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(parent.as_bytes()).unwrap();
    mac.update(text.as_bytes());
    let digest = STANDARD.encode(mac.finalize().into_bytes());
    STANDARD.encode(format!("{digest}{}{text}", &parent[..4]))
}

#[test]
fn vectors_match_typesense_js() {
    for (i, v) in vectors().iter().enumerate() {
        let input: Value = serde_json::from_str(&v.input).unwrap();
        assert_eq!(canonical_params(&input).unwrap(), v.params, "vector {i}");
        assert_eq!(generate(&v.parent, &input).unwrap(), v.scoped, "vector {i}");
    }
}

#[test]
fn decode_round_trips_every_vector() {
    for (i, v) in vectors().iter().enumerate() {
        let key = decode(&v.scoped).unwrap();
        assert_eq!(key.text, v.params, "vector {i}");
        assert_eq!(key.prefix.as_bytes(), &v.parent.as_bytes()[..4]);
        assert_eq!(key.digest.len(), 44);
        assert_eq!(
            key.params,
            serde_json::from_str::<Value>(&v.params).unwrap()
        );
        assert!(key.verify(&v.parent), "vector {i}");
        let mut other = v.parent.clone();
        other.pop();
        other.push('!');
        assert!(!key.verify(&other), "vector {i}");
    }
}

#[test]
fn decode_reads_the_expiry() {
    let key = decode(&generate(PARENT, &json!({"expires_at": 1906054106})).unwrap()).unwrap();
    assert_eq!(key.expires_at(), Some(Some(1906054106)));
    assert_eq!(
        decode(&generate(PARENT, &json!({})).unwrap())
            .unwrap()
            .expires_at(),
        None
    );
}

#[test]
fn a_bad_expiry_differs_from_none() {
    for bad in [r#""soon""#, "1.5", "18446744073709551615"] {
        let raw = format!("{}abcd{{\"expires_at\":{bad}}}", "A".repeat(44));
        let key = decode(&STANDARD.encode(raw)).unwrap();
        assert_eq!(key.expires_at(), Some(None), "{bad}");
    }
}

#[test]
fn verify_uses_the_stored_text() {
    let text = "{ \"filter_by\" : \"a:=1\" }";
    let key = decode(&by_hand(PARENT, text)).unwrap();
    assert_eq!(key.text, text);
    assert!(key.verify(PARENT));
}

#[test]
fn unpadded_key_decodes() {
    let v = &vectors()[0];
    let bytes = STANDARD.decode(&v.scoped).unwrap();
    let unpadded = STANDARD_NO_PAD.encode(bytes);
    assert_ne!(unpadded, v.scoped);
    assert!(decode(&unpadded).unwrap().verify(&v.parent));
    assert!(decode(&format!("  {}\n", v.scoped)).is_ok());
}

#[test]
fn tampered_digest_fails() {
    let mut key = decode(&generate(PARENT, &json!({"a": 1})).unwrap()).unwrap();
    let first = if key.digest.starts_with('A') {
        "B"
    } else {
        "A"
    };
    key.digest.replace_range(..1, first);
    assert!(!key.verify(PARENT));
    key.digest = "not base64".into();
    assert!(!key.verify(PARENT));
}

#[test]
fn wrong_prefix_fails() {
    let mut key = decode(&generate(PARENT, &json!({"a": 1})).unwrap()).unwrap();
    key.prefix = "xxxx".into();
    assert!(!key.verify(PARENT));
    assert!(!key.verify("abc"));
}

#[test]
fn debug_holds_no_digest() {
    let key = decode(&generate(PARENT, &json!({"a": 1})).unwrap()).unwrap();
    assert!(!format!("{key:?}").contains(&key.digest));
}

#[test]
fn a_non_object_is_refused() {
    assert_eq!(generate(PARENT, &json!([1])), Err(ScopedError::NotAnObject));
}

#[test]
fn a_bad_parent_is_refused() {
    let params = json!({});
    assert_eq!(
        generate("abc", &params),
        Err(ScopedError::Parent(ParentProblem::Short))
    );
    assert_eq!(
        generate("üüüü", &params),
        Err(ScopedError::Parent(ParentProblem::NotAscii))
    );
}

#[test]
fn numbers_javascript_prints_otherwise_are_refused() {
    let text = |t: &str| serde_json::from_str::<Value>(t).unwrap();
    for input in [
        r#"{"a":1e21}"#,
        r#"{"a":1e-7}"#,
        r#"{"a":9007199254740992}"#,
        r#"{"a":-9007199254740992}"#,
        r#"{"a":18446744073709551615}"#,
        r#"{"nested":{"a":[1e21]}}"#,
        r#"{"num_typos":[1e21]}"#,
    ] {
        assert_eq!(
            canonical_params(&text(input)),
            Err(ScopedError::Number),
            "{input}"
        );
    }
    assert_eq!(
        canonical_params(&text(r#"{"a":9007199254740991,"b":1e-6}"#)).unwrap(),
        r#"{"a":9007199254740991,"b":0.000001}"#
    );
}

#[test]
fn an_index_key_is_refused_at_any_depth() {
    for input in [json!({"1": "a"}), json!({"a": [{"0": 1}]})] {
        assert_eq!(canonical_params(&input), Err(ScopedError::IndexKey));
    }
    let kept = json!({"01": 1, "4294967295": 2});
    assert_eq!(
        canonical_params(&kept).unwrap(),
        r#"{"01":1,"4294967295":2}"#
    );
}

#[test]
fn a_join_of_something_else_is_refused() {
    assert_eq!(
        canonical_params(&json!({"query_by": [null]})),
        Err(ScopedError::ArrayItem("query_by".into()))
    );
}

#[test]
fn only_top_level_names_are_joined() {
    assert_eq!(
        canonical_params(&json!({"a": {"query_by": ["x", "y"]}})).unwrap(),
        r#"{"a":{"query_by":["x","y"]}}"#
    );
}

#[test]
fn expires_at_must_be_an_integer() {
    for input in [
        json!({"expires_at": "1"}),
        json!({"expires_at": 1.5}),
        json!({"expires_at": null}),
    ] {
        assert_eq!(canonical_params(&input), Err(ScopedError::ExpiresAt));
    }
}

#[test]
fn decode_says_what_is_wrong() {
    assert_eq!(decode("$$$").unwrap_err(), DecodeError::Base64);
    assert_eq!(
        decode(&STANDARD.encode("a".repeat(49))).unwrap_err(),
        DecodeError::TooShort
    );
    let mut bytes = vec![b'a'; 48];
    bytes.extend([0xff, 0xff]);
    assert_eq!(
        decode(&STANDARD.encode(&bytes)).unwrap_err(),
        DecodeError::NotUtf8
    );
    assert_eq!(
        decode(&STANDARD.encode("a".repeat(50))).unwrap_err(),
        DecodeError::NotJson
    );
}
