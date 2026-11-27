//! The canonical request digest of the durable outcome registry (ADR-0009, D2).
//!
//! A registry answers "what did this request do" by binding a caller's
//! `request_id` to a digest of the request it named. The digest is recomputed
//! from the received request and never supplied by the caller, so the binding
//! proves the retry carried the same request, not merely the same id.
//!
//! The digests are SHA-256 over the compact UTF-8 JSON of
//! `2brain.mutation-request/v1` with object keys sorted at every level and no
//! insignificant whitespace. Sorting is done here rather than trusted to
//! `serde_json`: the workspace can enable `preserve_order` through feature
//! unification, which would make `Map` iterate in insertion order and silently
//! change every digest. Keys sort bytewise over their UTF-8 encoding.
//!
//! Only mutations reach this module, so `arguments` and `preconditions` must be
//! JSON objects; a non-object is a malformed request, not a digest input.
//!
// The registry slice wires this to the mutation path; until then its API is
// exercised by tests.
#![allow(dead_code)]

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The document schema version, fixed in the digest so a future request shape
/// can never collide with this one.
const SCHEMA: &str = "2brain.mutation-request/v1";

/// The SHA-256 of a canonical request document.
///
/// The bytes are the identity; [`hex`](Self::hex) is only a rendering for
/// storage, logs and test vectors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RequestDigest([u8; 32]);

impl RequestDigest {
    /// The raw 32 bytes the digest is.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The digest as exactly 64 lowercase hex characters.
    pub fn hex(&self) -> String {
        use std::fmt::Write as _;

        let mut text = String::with_capacity(64);
        for byte in self.0 {
            write!(text, "{byte:02x}").expect("writing to a String cannot fail");
        }
        text
    }
}

/// The digest of one mutation request.
///
/// `arguments` and `preconditions` must be JSON objects and `verb` and
/// `workspace` must be non-empty. Nothing here accepts a caller-supplied
/// digest: this function is the only producer of [`RequestDigest`].
pub fn request_digest(
    verb: &str,
    workspace: &str,
    arguments: &Value,
    preconditions: &Value,
) -> Result<RequestDigest, String> {
    let document = canonical_document(verb, workspace, arguments, preconditions)?;
    let bytes: [u8; 32] = Sha256::digest(document.as_bytes()).into();
    Ok(RequestDigest(bytes))
}

/// The canonical JSON document the digest is taken over.
fn canonical_document(
    verb: &str,
    workspace: &str,
    arguments: &Value,
    preconditions: &Value,
) -> Result<String, String> {
    if verb.is_empty() {
        return Err("request verb must not be empty".to_owned());
    }
    if workspace.is_empty() {
        return Err("request workspace must not be empty".to_owned());
    }
    if !arguments.is_object() {
        return Err(format!(
            "request arguments must be a JSON object, got {}",
            json_kind(arguments)
        ));
    }
    if !preconditions.is_object() {
        return Err(format!(
            "request preconditions must be a JSON object, got {}",
            json_kind(preconditions)
        ));
    }

    let mut document = serde_json::Map::new();
    document.insert("schema".to_owned(), Value::String(SCHEMA.to_owned()));
    document.insert("verb".to_owned(), Value::String(verb.to_owned()));
    document.insert("workspace".to_owned(), Value::String(workspace.to_owned()));
    document.insert("arguments".to_owned(), arguments.clone());
    document.insert("preconditions".to_owned(), preconditions.clone());

    let mut text = String::new();
    write_canonical(&Value::Object(document), &mut text);
    Ok(text)
}

/// Append `value` to `out` as compact JSON with object keys sorted at every
/// level. Scalars use `serde_json`'s own encoding, so strings keep its escaping
/// and numbers keep their parsed representation.
fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => {
            out.push_str(&serde_json::to_string(text).expect("a string is always valid JSON"))
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(
                    &serde_json::to_string(key).expect("a string key is always valid JSON"),
                );
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
    }
}

/// The JSON type name, for an error that says what arrived.
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn digest(
        verb: &str,
        workspace: &str,
        arguments: Value,
        preconditions: Value,
    ) -> RequestDigest {
        request_digest(verb, workspace, &arguments, &preconditions)
            .expect("a well-formed request digests")
    }

    #[test]
    fn the_canonical_document_sorts_keys_at_every_level() {
        let document = canonical_document(
            "note.create",
            "ws",
            &json!({"b": 2, "a": {"z": 1, "y": 2}}),
            &json!({}),
        )
        .unwrap();

        assert_eq!(
            document,
            concat!(
                r#"{"arguments":{"a":{"y":2,"z":1},"b":2},"#,
                r#""preconditions":{},"#,
                r#""schema":"2brain.mutation-request/v1","#,
                r#""verb":"note.create","workspace":"ws"}"#,
            ),
            "compact JSON with sorted keys and no insignificant whitespace"
        );
    }

    #[test]
    fn a_different_written_key_order_yields_the_same_digest() {
        let one = digest(
            "note.create",
            "ws",
            json!({"title": "hello", "body": "text"}),
            json!({"expected_absent": true}),
        );
        let other = digest(
            "note.create",
            "ws",
            json!({"body": "text", "title": "hello"}),
            json!({"expected_absent": true}),
        );

        assert_eq!(one, other);
    }

    #[test]
    fn a_different_nested_key_order_yields_the_same_digest() {
        let one = digest(
            "note.update",
            "ws",
            json!({"patch": {"a": 1, "b": {"x": [1, 2], "y": null}}}),
            json!({}),
        );
        let other = digest(
            "note.update",
            "ws",
            json!({"patch": {"b": {"y": null, "x": [1, 2]}, "a": 1}}),
            json!({}),
        );

        assert_eq!(one, other);
    }

    #[test]
    fn each_request_component_changes_the_digest() {
        let base = digest(
            "note.create",
            "ws",
            json!({"title": "hello"}),
            json!({"expected_absent": true}),
        );

        assert_ne!(
            base,
            digest(
                "note.update",
                "ws",
                json!({"title": "hello"}),
                json!({"expected_absent": true})
            ),
            "verb is digested"
        );
        assert_ne!(
            base,
            digest(
                "note.create",
                "other-ws",
                json!({"title": "hello"}),
                json!({"expected_absent": true})
            ),
            "workspace is digested"
        );
        assert_ne!(
            base,
            digest(
                "note.create",
                "ws",
                json!({"title": "hello, world"}),
                json!({"expected_absent": true})
            ),
            "arguments are digested"
        );
        assert_ne!(
            base,
            digest(
                "note.create",
                "ws",
                json!({"title": "hello"}),
                json!({"expected_hash": "abc"})
            ),
            "preconditions are digested"
        );
    }

    #[test]
    fn the_digest_is_sha256_over_the_canonical_document() {
        let digest = digest(
            "note.create",
            "ws",
            json!({"title": "hello"}),
            json!({"expected_hash": "abc"}),
        );

        assert_eq!(
            digest.hex(),
            "e08d3bbee00bcaeb82772048269c3951a1807ec97e6799ae94da34d1c42d969b"
        );
    }

    #[test]
    fn unicode_roundtrips_and_escaped_input_digests_the_same() {
        let plain = digest(
            "note.create",
            "рабочее пространство",
            json!({"title": "Привет, 世界 🧠"}),
            json!({}),
        );
        // The same string as JSON escapes. Parsing restores the characters, so
        // the digest cannot depend on which spelling the caller sent.
        let escaped: Value = serde_json::from_str(
            r#"{"title":"\u041f\u0440\u0438\u0432\u0435\u0442, \u4e16\u754c \ud83e\udde0"}"#,
        )
        .unwrap();
        let reparsed = digest("note.create", "рабочее пространство", escaped, json!({}));

        assert_eq!(plain, reparsed);
    }

    #[test]
    fn hex_is_64_lowercase_characters_and_two_calls_agree() {
        let arguments = json!({"title": "hello"});
        let preconditions = json!({});
        let first = request_digest("note.create", "ws", &arguments, &preconditions).unwrap();
        let second = request_digest("note.create", "ws", &arguments, &preconditions).unwrap();

        assert_eq!(first, second);
        let hex = first.hex();
        assert_eq!(hex.len(), 64);
        assert!(
            hex.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "lowercase hex only: {hex}"
        );
        assert_eq!(first.as_bytes().len(), 32);
    }

    #[test]
    fn an_empty_verb_or_workspace_is_rejected() {
        assert!(
            request_digest("", "ws", &json!({}), &json!({})).is_err(),
            "empty verb"
        );
        assert!(
            request_digest("note.create", "", &json!({}), &json!({})).is_err(),
            "empty workspace"
        );
    }

    #[test]
    fn non_object_arguments_or_preconditions_are_rejected() {
        assert!(
            request_digest("note.create", "ws", &json!([1, 2]), &json!({})).is_err(),
            "array arguments"
        );
        assert!(
            request_digest("note.create", "ws", &json!(null), &json!({})).is_err(),
            "null arguments"
        );
        assert!(
            request_digest("note.create", "ws", &json!({}), &json!("hash")).is_err(),
            "string preconditions"
        );
    }
}
