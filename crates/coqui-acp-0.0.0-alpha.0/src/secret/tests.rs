//! `vectors/secret.json`: RFC 9180's base-mode vector run through this crate's suite, and the extension's own bytes (context digests,
//! key bindings, sealed fields, value bindings, refusals) that the Dart client must reproduce or refuse alike.
//!
//! The RFC section is copied from the RFC. The rest is written by `vectors_are_current` when `COQUI_ACP_WRITE_VECTORS` is set, from
//! the fixed inputs below, and is read back and checked independently of how it was made.

use super::binding::{Kind, LinkKey, LinkRole, Method, Prover, Statement, Verifier};
use super::context::Context;
use super::form::{Form, Limits, base64_canonical, base64url_canonical, json_integer, json_number};
use super::seal::testing::{key_pair, open_raw, open_with, seal_raw, seal_with};
use super::seal::{Receiver, Recipient, Sealed, values_digest};
use super::unprotected;
use super::{DEFAULT_LIFETIME, Error, IdentityId};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::time::Duration;
use data_encoding::HEXLOWER;
use hpke::Serializable;
use serde_json::{Map, Value, json};

const VECTORS: &str = include_str!("../../../../vectors/secret.json");

fn hex(text: &str) -> Vec<u8> {
    HEXLOWER.decode(text.as_bytes()).expect("hex")
}

pub(crate) fn kind(error: &Error) -> &'static str {
    match error {
        Error::Schema(_) => "schema",
        Error::Malformed(_) => "malformed",
        Error::Unsupported(_) => "unsupported",
        Error::Key(_) => "key",
        Error::UnknownIdentity => "unknown_identity",
        Error::BadProof => "bad_proof",
        Error::Expired => "expired",
        Error::WrongAudience => "wrong_audience",
        Error::Field(_) => "field",
        Error::Seal => "seal",
        Error::Open => "open",
        Error::TooLarge(_) => "too_large",
    }
}

fn schema_text() -> String {
    json!({
        "type": "object",
        "title": "Registry login",
        "properties": {
            "user": {"type": "string", "title": "User", "minLength": 1},
            "password": {"type": "string", "title": "Password", "writeOnly": true, "minLength": 4},
            "token": {"type": "string", "x-sensitive": true, "contentEncoding": "base64"},
            "pin": {"type": "integer", "writeOnly": true, "minimum": 0, "maximum": 999999}
        },
        "required": ["user", "password"]
    })
    .to_string()
}

/// Published test keys (RFC 6979's P-256 key, RFC 7748's two X25519 keys, and a second P-256 scalar); none protects anything.
const ECDSA_RECEIVER: &str = "c9afa9d845ba75166b5c215767b1d6934e50c3db36e89b127b8a622b120f6721";
const ECDSA_SENDER: &str = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
const X25519_RECEIVER: &str = "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a";
const X25519_SENDER: &str = "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb";
/// A link secret as the host mints it (any 16 bytes or more), and another link's.
const LINK_SECRET: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90112233445566778899aabbccddeeff00";
const OTHER_LINK_SECRET: &str = "00ffeeddccbbaa99887766554433221100908f7e6d5c4b3a2918f7e6d5c4b3a2";
const NOW: u64 = 1_700_000_000;
const LIFETIME: u64 = 240;

fn ikm(start: u8) -> Vec<u8> {
    (start..start + 32).collect()
}

fn context(method: &str, request_id: &str) -> Context {
    Context {
        session_id: "s1".into(),
        request_id: request_id.into(),
        message: if method == "show" {
            "Your new key"
        } else {
            "The deploy needs a login"
        }
        .into(),
        schema: schema_text(),
        form: Some(r#"{"sections":[]}"#.into()),
        values: (method == "show").then(|| PLAIN.into()),
    }
}

/// The plain values of an answer (and of a show) as the JSON object text that is sent.
const PLAIN: &str = r#"{"user":"ana"}"#;

fn method_of(text: &str) -> Method {
    if text == "show" {
        Method::Show
    } else {
        Method::Request
    }
}

/// The kinds of identity a case uses: the receiver's and the sender's private keys.
fn provers(case: &Value) -> (Prover, Prover) {
    let (receiver, sender) = (
        hex(case["receiver_private"].as_str().unwrap()),
        hex(case["sender_private"].as_str().unwrap()),
    );
    if case["kind"] == "link" {
        // The one secret both ends hold: the host receives a request, the application answers it.
        let link = LinkKey::derive(&receiver).unwrap();
        return (
            Prover::linked(&link, LinkRole::Host),
            Prover::linked(&link, LinkRole::App),
        );
    }
    if case["kind"] == "ecdsa" {
        return (
            unprotected::ecdsa_p256(&receiver).unwrap(),
            unprotected::ecdsa_p256(&sender).unwrap(),
        );
    }
    let (receiver_spki, _) = unprotected::x25519_agreement(&receiver).unwrap();
    let (sender_spki, _) = unprotected::x25519_agreement(&sender).unwrap();
    (
        unprotected::x25519(&receiver, &sender_spki).unwrap(),
        unprotected::x25519(&sender, &receiver_spki).unwrap(),
    )
}

/// What a test changes about a verifier.
#[derive(Default)]
struct Tuning {
    own: Option<IdentityId>,
    trusts: Option<IdentityId>,
    no_agreement: bool,
    max_lifetime: Option<u64>,
    skew: Option<u64>,
    /// A link's secret other than the case's (another link).
    secret: Option<Vec<u8>>,
    /// The verifier is the host's end of a link; else the application's.
    receiver: bool,
}

/// The verifier of one end of the in-process link. With `trusts` it knows the other end and, unless told otherwise, holds the link.
fn link_verifier(private: &str, tuning: &Tuning) -> Verifier {
    let link = LinkKey::derive(&tuning.secret.clone().unwrap_or_else(|| hex(private))).unwrap();
    let role = if tuning.receiver {
        LinkRole::Host
    } else {
        LinkRole::App
    };
    let other = link.identity(if tuning.receiver {
        LinkRole::App
    } else {
        LinkRole::Host
    });
    let builder = Verifier::builder(tuning.own.unwrap_or_else(|| link.identity(role)));
    if tuning.trusts.is_none() {
        return builder.build();
    }
    if tuning.own.is_some() || tuning.no_agreement {
        return builder.trust(other).build();
    }
    builder.link(&link, role).unwrap().build()
}

/// The verifier of the side whose private key is `private`: it is itself (or `own`), knows `trusts`, and for X25519 holds its agreement.
fn verifier(kind: &str, private: &str, tuning: &Tuning) -> Verifier {
    if kind == "link" {
        return link_verifier(private, tuning);
    }
    let secret = hex(private);
    let (spki, agree) = unprotected::x25519_agreement(&secret).unwrap();
    let me = if kind == "ecdsa" {
        unprotected::ecdsa_p256(&secret).unwrap().id()
    } else {
        IdentityId::of(&spki)
    };
    let mut builder = Verifier::builder(tuning.own.unwrap_or(me));
    if kind != "ecdsa" && tuning.own.is_none() && !tuning.no_agreement {
        builder = builder.agreement(&spki, agree).unwrap();
    }
    if let Some(trusts) = tuning.trusts {
        builder = builder.trust(trusts);
    }
    if let Some(seconds) = tuning.max_lifetime {
        builder = builder.max_lifetime(Duration::from_secs(seconds));
    }
    if let Some(seconds) = tuning.skew {
        builder = builder.skew(Duration::from_secs(seconds));
    }
    builder.build()
}

fn base_cases() -> Vec<Value> {
    let mut out = Vec::new();
    for (name, kind, receiver, sender, method, request_id) in [
        (
            "a webcrypto identity signs a request's key",
            "ecdsa",
            ECDSA_RECEIVER,
            ECDSA_SENDER,
            "request",
            "6f1c8b6e-2d0e-4a52-9a43-0a6d3a8b8d11",
        ),
        (
            "a noise identity proves a show's key by Diffie-Hellman",
            "dh",
            X25519_RECEIVER,
            X25519_SENDER,
            "show",
            "show-1",
        ),
        (
            "the in-process link's own secret proves a request's key",
            "link",
            LINK_SECRET,
            LINK_SECRET,
            "request",
            "link-1",
        ),
    ] {
        let case = json!({"name": name, "kind": kind, "receiver_private": receiver, "sender_private": sender});
        let (prover_r, prover_s) = provers(&case);
        let (id_r, id_s) = (prover_r.id(), prover_s.id());
        let ctx = context(method, request_id);
        let digest = ctx.digest(method_of(method));
        let (private, public) = key_pair(&ikm(0));
        let made_receiver = Receiver::from_key_pair(
            method_of(method),
            &ctx,
            &Limits::new(),
            &prover_r,
            &id_s,
            NOW,
            DEFAULT_LIFETIME,
            private,
            &public,
        )
        .unwrap();
        let recipient = made_receiver.recipient().clone();
        let key_statement = Statement {
            kind: Kind::Key,
            method: method_of(method),
            request_id,
            digest: &digest,
            key: &recipient.key,
            payload: &[],
        }
        .bytes(recipient.binding.expires, &id_s);
        let mut sealed = BTreeMap::new();
        let mut fields = Vec::new();
        for (field, value) in [
            ("password", "correct horse"),
            ("token", "c2VjcmV0LXRva2Vu"),
            ("pin", "4821"),
        ] {
            let made = seal_with(
                &recipient.key,
                method_of(method),
                request_id,
                &digest,
                field,
                value.as_bytes(),
                &ikm(32),
            );
            fields.push(json!({"field": field, "value": value, "ephemeral_ikm": HEXLOWER.encode(&ikm(32)), "sealed": made.to_value()}));
            sealed.insert(field.to_string(), made);
        }
        let payload = values_digest(PLAIN, &sealed);
        let values_statement = Statement {
            kind: Kind::Values,
            method: method_of(method),
            request_id,
            digest: &digest,
            key: &recipient.key,
            payload: &payload,
        }
        .bytes(NOW + LIFETIME, &id_r);
        let auth = prover_s
            .bind(
                &Statement {
                    kind: Kind::Values,
                    method: method_of(method),
                    request_id,
                    digest: &digest,
                    key: &recipient.key,
                    payload: &payload,
                },
                NOW + LIFETIME,
                &id_r,
            )
            .unwrap();
        out.push(json!({
            "name": name, "kind": kind, "receiver_private": receiver, "sender_private": sender,
            "method": method, "context": {"session_id": ctx.session_id, "request_id": ctx.request_id, "message": ctx.message, "schema": ctx.schema, "form": ctx.form, "values": ctx.values},
            "context_digest": HEXLOWER.encode(&digest),
            "recipient_ikm": HEXLOWER.encode(&ikm(0)), "now": NOW, "lifetime": LIFETIME,
            "receiver_id": id_r.to_string(), "sender_id": id_s.to_string(),
            "key_statement": HEXLOWER.encode(&key_statement), "recipient": recipient.to_value(),
            "plain": PLAIN, "fields": fields,
            "values_digest": HEXLOWER.encode(&payload), "values_statement": HEXLOWER.encode(&values_statement), "auth": auth.to_value(),
        }));
        if kind == "link" {
            // The tags name the two ends and are public; the MAC key behind the proofs is reproduced by the proofs alone.
            let last = out.last_mut().and_then(Value::as_object_mut).unwrap();
            last.insert(
                "link".into(),
                json!({"secret": LINK_SECRET, "host_tag": HEXLOWER.encode(prover_r.spki()), "app_tag": HEXLOWER.encode(prover_s.spki())}),
            );
        }
    }
    out
}

fn grammar() -> Value {
    json!([
        {"kind": "integer", "text": "0", "valid": true},
        {"kind": "integer", "text": "-0", "valid": true},
        {"kind": "integer", "text": "4821", "valid": true},
        {"kind": "integer", "text": "9007199254740991", "valid": true},
        {"kind": "integer", "text": "-9007199254740991", "valid": true},
        {"kind": "integer", "text": "9007199254740992", "valid": false},
        {"kind": "integer", "text": "+5", "valid": false},
        {"kind": "integer", "text": "05", "valid": false},
        {"kind": "integer", "text": " 5", "valid": false},
        {"kind": "integer", "text": "5 ", "valid": false},
        {"kind": "integer", "text": "5.0", "valid": false},
        {"kind": "integer", "text": "", "valid": false},
        {"kind": "integer", "text": "-", "valid": false},
        {"kind": "integer", "text": "0x10", "valid": false},
        {"kind": "number", "text": "1.5", "valid": true},
        {"kind": "number", "text": "-0.25e3", "valid": true},
        {"kind": "number", "text": "10E+2", "valid": true},
        {"kind": "number", "text": "0", "valid": true},
        {"kind": "number", "text": ".5", "valid": false},
        {"kind": "number", "text": "5.", "valid": false},
        {"kind": "number", "text": "+1", "valid": false},
        {"kind": "number", "text": "01", "valid": false},
        {"kind": "number", "text": "1e", "valid": false},
        {"kind": "number", "text": "1e999", "valid": false},
        {"kind": "number", "text": "NaN", "valid": false},
        {"kind": "number", "text": "Infinity", "valid": false},
        {"kind": "number", "text": " 1", "valid": false},
        {"kind": "base64url", "text": "", "valid": true},
        {"kind": "base64url", "text": "AA", "valid": true},
        {"kind": "base64url", "text": "AQID", "valid": true},
        {"kind": "base64url", "text": "AA==", "valid": false},
        {"kind": "base64url", "text": "AB", "valid": false},
        {"kind": "base64url", "text": "A", "valid": false},
        {"kind": "base64url", "text": "A+/A", "valid": false},
        {"kind": "base64", "text": "AA==", "valid": true},
        {"kind": "base64", "text": "AQID", "valid": true},
        {"kind": "base64", "text": "AA", "valid": false},
        {"kind": "base64", "text": "AB==", "valid": false},
        {"kind": "base64", "text": "A-_A", "valid": false}
    ])
}

fn forms() -> Value {
    let flat =
        |property: Value| json!({"type": "object", "properties": {"f": property}}).to_string();
    json!({
        "accepted": [
            {"name": "plain and secret fields", "schema": schema_text(), "secret": ["password", "token", "pin"], "plain": ["user"]},
            {"name": "a secret marked x-sensitive", "schema": flat(json!({"type": "string", "x-sensitive": true})), "secret": ["f"], "plain": []},
            {"name": "a plain choice", "schema": flat(json!({"type": "string", "oneOf": [{"const": "a", "title": "A"}]})), "secret": [], "plain": ["f"]},
            {"name": "writeOnly false is plain", "schema": flat(json!({"type": "string", "writeOnly": false})), "secret": [], "plain": ["f"]},
            {"name": "a secret with the password format", "schema": flat(json!({"type": "string", "writeOnly": true, "format": "password", "maxLength": 40})), "secret": ["f"], "plain": []}
        ],
        "refused": [
            {"name": "not JSON", "schema": "{", "kind": "schema"},
            {"name": "not an object", "schema": "[]", "kind": "schema"},
            {"name": "no properties", "schema": "{\"type\":\"object\"}", "kind": "schema"},
            {"name": "no questions", "schema": "{\"type\":\"object\",\"properties\":{}}", "kind": "schema"},
            {"name": "a type that is not object", "schema": "{\"type\":\"array\",\"properties\":{\"f\":{\"type\":\"string\"}}}", "kind": "schema"},
            {"name": "a property that is not an object", "schema": "{\"properties\":{\"f\":3}}", "kind": "schema"},
            {"name": "a type no control shows", "schema": flat(json!({"type": "object"})), "kind": "schema"},
            {"name": "an empty name", "schema": "{\"properties\":{\"\":{\"type\":\"string\"}}}", "kind": "schema"},
            {"name": "required names a missing property", "schema": "{\"properties\":{\"f\":{\"type\":\"string\"}},\"required\":[\"g\"]}", "kind": "schema"},
            {"name": "required is not a list", "schema": "{\"properties\":{\"f\":{\"type\":\"string\"}},\"required\":\"f\"}", "kind": "schema"},
            {"name": "a secret choice", "schema": flat(json!({"type": "string", "writeOnly": true, "enum": ["a", "b"]})), "kind": "schema"},
            {"name": "a secret flag", "schema": flat(json!({"type": "boolean", "writeOnly": true})), "kind": "schema"},
            {"name": "a secret with a default", "schema": flat(json!({"type": "string", "writeOnly": true, "default": "hunter2"})), "kind": "schema"},
            {"name": "a secret with a const", "schema": flat(json!({"type": "string", "x-sensitive": true, "const": "x"})), "kind": "schema"},
            {"name": "a secret with examples", "schema": flat(json!({"type": "string", "writeOnly": true, "examples": ["x"]})), "kind": "schema"},
            {"name": "a secret with anyOf", "schema": flat(json!({"type": "string", "writeOnly": true, "anyOf": [{"minLength": 1}]})), "kind": "schema"},
            {"name": "a secret with allOf", "schema": flat(json!({"type": "string", "writeOnly": true, "allOf": []})), "kind": "schema"},
            {"name": "a secret with a pattern", "schema": flat(json!({"type": "string", "writeOnly": true, "pattern": "^a"})), "kind": "schema"},
            {"name": "a secret with a conditional", "schema": flat(json!({"type": "string", "writeOnly": true, "if": {}})), "kind": "schema"},
            {"name": "a secret with not", "schema": flat(json!({"type": "string", "writeOnly": true, "not": {}})), "kind": "schema"},
            {"name": "a secret with a reference", "schema": flat(json!({"type": "string", "writeOnly": true, "$ref": "#/x"})), "kind": "schema"},
            {"name": "a secret with dependent schemas", "schema": flat(json!({"type": "string", "writeOnly": true, "dependentSchemas": {}})), "kind": "schema"},
            {"name": "writeOnly that is not a boolean", "schema": flat(json!({"type": "string", "writeOnly": "yes"})), "kind": "schema"},
            {"name": "x-sensitive that is not a boolean", "schema": flat(json!({"type": "string", "x-sensitive": 1})), "kind": "schema"},
            {"name": "a password format with no mark", "schema": flat(json!({"type": "string", "format": "password"})), "kind": "schema"},
            {"name": "a password format marked not secret", "schema": flat(json!({"type": "string", "format": "password", "writeOnly": false})), "kind": "schema"},
            {"name": "a key twice: a plain question then a secret one", "schema": "{\"properties\":{\"a\":{\"type\":\"string\",\"title\":\"Password\"},\"a\":{\"type\":\"string\",\"writeOnly\":true,\"title\":\"PIN\"}}}", "kind": "schema"},
            {"name": "a key twice inside a question", "schema": "{\"properties\":{\"a\":{\"type\":\"string\",\"type\":\"string\"}}}", "kind": "schema"},
            {"name": "a lone surrogate in a name", "schema": "{\"properties\":{\"\\ud800\":{\"type\":\"string\"}}}", "kind": "schema"},
            {"name": "a lone surrogate in a title", "schema": "{\"properties\":{\"a\":{\"type\":\"string\",\"title\":\"\\udc00\"}}}", "kind": "schema"},
            {"name": "a root default", "schema": "{\"default\":{},\"properties\":{\"f\":{\"type\":\"string\"}}}", "kind": "schema"},
            {"name": "a root allOf", "schema": "{\"allOf\":[],\"properties\":{\"f\":{\"type\":\"string\"}}}", "kind": "schema"},
            {"name": "a root if", "schema": "{\"if\":{},\"properties\":{\"f\":{\"type\":\"string\"}}}", "kind": "schema"},
            {"name": "a root dependentSchemas", "schema": "{\"dependentSchemas\":{},\"properties\":{\"f\":{\"type\":\"string\"}}}", "kind": "schema"},
            {"name": "a root $defs", "schema": "{\"$defs\":{},\"properties\":{\"f\":{\"type\":\"string\"}}}", "kind": "schema"},
            {"name": "a root additionalProperties", "schema": "{\"additionalProperties\":true,\"properties\":{\"f\":{\"type\":\"string\"}}}", "kind": "schema"},
            {"name": "a secret with $dynamicRef", "schema": flat(json!({"type": "string", "writeOnly": true, "$dynamicRef": "#x"})), "kind": "schema"},
            {"name": "a secret with dependencies", "schema": flat(json!({"type": "string", "writeOnly": true, "dependencies": {}})), "kind": "schema"},
            {"name": "a secret with contentSchema", "schema": flat(json!({"type": "string", "writeOnly": true, "contentSchema": {}})), "kind": "schema"},
            {"name": "a secret with multipleOf", "schema": flat(json!({"type": "integer", "writeOnly": true, "multipleOf": 3})), "kind": "schema"},
            {"name": "a secret with x-placeholder", "schema": flat(json!({"type": "string", "writeOnly": true, "x-placeholder": "hunter2"})), "kind": "schema"},
            {"name": "a secret with readOnly", "schema": flat(json!({"type": "string", "writeOnly": true, "readOnly": true})), "kind": "schema"},
            {"name": "a secret with a format other than password", "schema": flat(json!({"type": "string", "writeOnly": true, "format": "email"})), "kind": "schema"},
            {"name": "an integer bound past 2^53", "schema": flat(json!({"type": "integer", "maximum": 9007199254740992i64})), "kind": "schema"}
        ]
    })
}

fn layouts() -> Value {
    let layout = |value: Value| value.to_string();
    json!({
        "accepted": [
            {"name": "an empty form document", "schema": schema_text(), "layout": "{}"},
            {"name": "sections only", "schema": schema_text(), "layout": layout(json!({"sections": [{"id": "", "title": "Login"}, {"id": "more", "description": "x"}]}))},
            {"name": "a password widget on a secret question", "schema": schema_text(), "layout": layout(json!({"fields": {"password": {"widget": "password", "placeholder": "..."}, "token": {}}}))},
            {"name": "hints on a plain question", "schema": schema_text(), "layout": layout(json!({"fields": {"user": {"widget": "text", "unit": "name", "placeholder": "who"}}}))},
            {"name": "a condition on a plain question", "schema": schema_text(), "layout": layout(json!({"fields": {"password": {"visibleWhen": {"field": "user", "in": ["ana", "bo"]}}}}))}
        ],
        "refused": [
            {"name": "not JSON", "schema": schema_text(), "layout": "{", "kind": "schema"},
            {"name": "not an object", "schema": schema_text(), "layout": "[]", "kind": "schema"},
            {"name": "a key twice", "schema": schema_text(), "layout": "{\"sections\":[],\"sections\":[]}", "kind": "schema"},
            {"name": "a key twice in a hint", "schema": schema_text(), "layout": "{\"fields\":{\"user\":{\"widget\":\"text\",\"widget\":\"password\"}}}", "kind": "schema"},
            {"name": "a lone surrogate", "schema": schema_text(), "layout": "{\"sections\":[{\"id\":\"\\ud800\"}]}", "kind": "schema"},
            {"name": "a key the document does not define", "schema": schema_text(), "layout": "{\"extra\":1}", "kind": "schema"},
            {"name": "a section with no id", "schema": schema_text(), "layout": "{\"sections\":[{\"title\":\"x\"}]}", "kind": "schema"},
            {"name": "a section id that is no text", "schema": schema_text(), "layout": "{\"sections\":[{\"id\":1}]}", "kind": "schema"},
            {"name": "a section with another key", "schema": schema_text(), "layout": "{\"sections\":[{\"id\":\"a\",\"x\":1}]}", "kind": "schema"},
            {"name": "sections that are no list", "schema": schema_text(), "layout": "{\"sections\":{}}", "kind": "schema"},
            {"name": "a field the form lacks", "schema": schema_text(), "layout": layout(json!({"fields": {"nothing": {}}})), "kind": "schema"},
            {"name": "a nested form in a flat form", "schema": schema_text(), "layout": layout(json!({"fields": {"user": {"form": {}}}})), "kind": "schema"},
            {"name": "an unknown hint", "schema": schema_text(), "layout": layout(json!({"fields": {"user": {"hidden": true}}})), "kind": "schema"},
            {"name": "a masking widget on a plain question", "schema": schema_text(), "layout": layout(json!({"fields": {"user": {"widget": "password"}}})), "kind": "schema"},
            {"name": "a plain widget on a secret question", "schema": schema_text(), "layout": layout(json!({"fields": {"password": {"widget": "text"}}})), "kind": "schema"},
            {"name": "a textarea on a secret question", "schema": schema_text(), "layout": layout(json!({"fields": {"token": {"widget": "textarea"}}})), "kind": "schema"},
            {"name": "a widget nobody defines on a plain question", "schema": schema_text(), "layout": layout(json!({"fields": {"user": {"widget": "secret"}}})), "kind": "schema"},
            {"name": "a widget that is no text", "schema": schema_text(), "layout": layout(json!({"fields": {"user": {"widget": 1}}})), "kind": "schema"},
            {"name": "a placeholder that is no text", "schema": schema_text(), "layout": layout(json!({"fields": {"user": {"placeholder": 1}}})), "kind": "schema"},
            {"name": "a condition on a secret question's value", "schema": schema_text(), "layout": layout(json!({"fields": {"user": {"visibleWhen": {"field": "password", "equals": "x"}}}})), "kind": "schema"},
            {"name": "a condition on a question the form lacks", "schema": schema_text(), "layout": layout(json!({"fields": {"user": {"visibleWhen": {"field": "nothing", "equals": "x"}}}})), "kind": "schema"},
            {"name": "a condition with both equals and in", "schema": schema_text(), "layout": layout(json!({"fields": {"password": {"visibleWhen": {"field": "user", "equals": "x", "in": []}}}})), "kind": "schema"},
            {"name": "a condition with neither", "schema": schema_text(), "layout": layout(json!({"fields": {"password": {"visibleWhen": {"field": "user"}}}})), "kind": "schema"}
        ]
    })
}

fn refusals() -> Value {
    let a = "a webcrypto identity signs a request's key";
    let b = "a noise identity proves a show's key by Diffie-Hellman";
    let c = "the in-process link's own secret proves a request's key";
    json!([
        {"case": "verify", "name": "an identity nobody knows", "base": a, "change": {"trusted": false}, "expect": "unknown_identity"},
        {"case": "verify", "name": "a known identity that is not the one expected", "base": a, "change": {"expected": "sha256:0000000000000000000000000000000000000000000000000000000000000000"}, "expect": "unknown_identity"},
        {"case": "verify", "name": "a key changed after it was bound", "base": a, "change": {"flip_key": true}, "expect": "bad_proof"},
        {"case": "verify", "name": "a proof changed", "base": a, "change": {"flip_proof": true}, "expect": "bad_proof"},
        {"case": "verify", "name": "a signature written with a high s", "base": a, "change": {"high_s": true}, "expect": "bad_proof"},
        {"case": "verify", "name": "a binding for another request", "base": a, "change": {"request_id": "another"}, "expect": "bad_proof"},
        {"case": "verify", "name": "a binding for the other method", "base": a, "change": {"method": "show"}, "expect": "bad_proof"},
        {"case": "verify", "name": "a form changed on the way: a secret made plain", "base": a, "change": {"schema": schema_text().replace("\"writeOnly\":true", "\"writeOnly\":false")}, "expect": "bad_proof"},
        {"case": "verify", "name": "a form changed on the way: the schema re-serialized", "base": a, "change": {"schema": serde_json::to_string_pretty(&serde_json::from_str::<Value>(&schema_text()).unwrap()).unwrap()}, "expect": "bad_proof"},
        {"case": "verify", "name": "a message changed on the way", "base": a, "change": {"message": "Send the money"}, "expect": "bad_proof"},
        {"case": "verify", "name": "a session changed on the way", "base": a, "change": {"session_id": "s2"}, "expect": "bad_proof"},
        {"case": "verify", "name": "a layout changed on the way", "base": a, "change": {"form": "{\"sections\":[{\"id\":\"x\"}]}"}, "expect": "bad_proof"},
        {"case": "verify", "name": "a layout taken off", "base": a, "change": {"form": null}, "expect": "bad_proof"},
        {"case": "verify", "name": "a binding that ended", "base": a, "change": {"now": NOW + LIFETIME + 31}, "expect": "expired"},
        {"case": "verify", "name": "a binding at the edge of the skew is taken", "base": a, "change": {"now": NOW + LIFETIME + 30}, "expect": "ok"},
        {"case": "verify", "name": "a binding that lasts too long", "base": a, "change": {"max_lifetime": 209, "skew": 0}, "expect": "expired"},
        {"case": "verify", "name": "a binding that lasts exactly as long as allowed", "base": a, "change": {"max_lifetime": 240, "skew": 0}, "expect": "ok"},
        {"case": "verify", "name": "a binding made for someone else", "base": a, "change": {"own_id": "sha256:1111111111111111111111111111111111111111111111111111111111111111"}, "expect": "wrong_audience"},
        {"case": "verify", "name": "a binding method nobody has", "base": a, "change": {"binding_method": "rsa"}, "expect": "unsupported"},
        {"case": "verify", "name": "a key of another shape", "base": a, "change": {"truncate_binding_key": true}, "expect": "key"},
        {"case": "verify", "name": "a noise proof changed", "base": b, "change": {"flip_proof": true}, "expect": "bad_proof"},
        {"case": "verify", "name": "a noise proof for another request", "base": b, "change": {"request_id": "another"}, "expect": "bad_proof"},
        {"case": "verify", "name": "a noise proof for another collector", "base": b, "change": {"own_id": "sha256:1111111111111111111111111111111111111111111111111111111111111111"}, "expect": "wrong_audience"},
        {"case": "verify", "name": "a collector with no key agreement of its own", "base": b, "change": {"no_agreement": true}, "expect": "unsupported"},
        {"case": "open", "name": "values untouched are opened", "base": a, "change": {}, "expect": "ok"},
        {"case": "open", "name": "an answer opened after the receiver's own binding ended and the skew", "base": a, "change": {"open_now": NOW + LIFETIME + 31}, "expect": "expired"},
        {"case": "open", "name": "an answer opened at the edge of the receiver's binding plus the skew", "base": a, "change": {"open_now": NOW + LIFETIME + 30}, "expect": "ok"},
        {"case": "open", "name": "a sender whose binding outlasts the receiver's is still late", "base": a, "change": {"authenticated": true, "auth_lifetime": 290, "open_now": NOW + LIFETIME + 31}, "expect": "expired"},
        {"case": "open", "name": "an authenticated plain value for a secret question", "base": a, "change": {"authenticated": true, "plain": "{\"user\":\"ana\",\"password\":\"x\"}"}, "expect": "field"},
        {"case": "open", "name": "an authenticated answer with no required plain value", "base": a, "change": {"authenticated": true, "plain": "{}"}, "expect": "field"},
        {"case": "open", "name": "an authenticated answer with a required secret left out", "base": a, "change": {"authenticated": true, "authenticated_drop": "password"}, "expect": "field"},
        {"case": "open", "name": "an authenticated plain text that repeats a key", "base": a, "change": {"authenticated": true, "plain": "{\"user\":\"mallory\",\"user\":\"ana\"}"}, "expect": "malformed"},
        {"case": "open", "name": "an authenticated plain text that is no object", "base": a, "change": {"authenticated": true, "plain": "[]"}, "expect": "malformed"},
        {"case": "authenticate", "name": "a sender asked to sign a plain value for a secret question", "base": a, "change": {"plain": "{\"user\":\"ana\",\"password\":\"x\"}"}, "expect": "field"},
        {"case": "authenticate", "name": "a sender asked to sign an answer with a required secret left out", "base": a, "change": {"drop": "password"}, "expect": "field"},
        {"case": "authenticate", "name": "a sender asked to sign an answer with no required plain value", "base": a, "change": {"plain": "{}"}, "expect": "field"},
        {"case": "authenticate", "name": "a sender asked to sign plain text that repeats a key", "base": a, "change": {"plain": "{\"user\":\"a\",\"user\":\"b\"}"}, "expect": "malformed"},
        {"case": "receiver", "name": "a context that is fine", "base": a, "change": {}, "expect": "ok"},
        {"case": "receiver", "name": "a layout that masks a plain question", "base": a, "change": {"form": "{\"fields\":{\"user\":{\"widget\":\"password\"}}}"}, "expect": "schema"},
        {"case": "receiver", "name": "a layout that shows a secret question as text", "base": a, "change": {"form": "{\"fields\":{\"password\":{\"widget\":\"text\"}}}"}, "expect": "schema"},
        {"case": "receiver", "name": "a layout that is no JSON", "base": a, "change": {"form": "{"}, "expect": "schema"},
        {"case": "receiver", "name": "a request that carries plain values", "base": a, "change": {"values": "{\"user\":\"ana\"}"}, "expect": "malformed"},
        {"case": "receiver", "name": "a show with no plain values", "base": b, "change": {"values": null}, "expect": "malformed"},
        {"case": "receiver", "name": "a show whose plain values repeat a key", "base": b, "change": {"values": "{\"user\":\"a\",\"user\":\"b\"}"}, "expect": "malformed"},
        {"case": "receiver", "name": "a show whose plain values name a secret question", "base": b, "change": {"values": "{\"user\":\"a\",\"password\":\"b\"}"}, "expect": "field"},
        {"case": "receiver", "name": "a schema that repeats a key", "base": a, "change": {"schema": "{\"properties\":{\"a\":{\"type\":\"string\"},\"a\":{\"type\":\"string\",\"writeOnly\":true}}}"}, "expect": "schema"},
        {"case": "open", "name": "a plain value changed after the answer was authenticated", "base": a, "change": {"plain": "{\"user\":\"mallory\"}"}, "expect": "bad_proof"},
        {"case": "open", "name": "a plain value added after the answer was authenticated", "base": a, "change": {"plain": "{\"user\":\"ana\",\"x\":1}"}, "expect": "bad_proof"},
        {"case": "open", "name": "the plain text written another way", "base": a, "change": {"plain": "{ \"user\": \"ana\" }"}, "expect": "bad_proof"},
        {"case": "verify", "name": "a shown plain value changed on the way", "base": b, "change": {"values": "{\"user\":\"mallory\"}"}, "expect": "bad_proof"},
        {"case": "verify", "name": "shown plain values taken off", "base": b, "change": {"values": null}, "expect": "bad_proof"},
        {"case": "open", "name": "a ciphertext changed after it was authenticated", "base": a, "change": {"flip_ct": "password"}, "expect": "bad_proof"},
        {"case": "open", "name": "an encapsulated key changed after it was authenticated", "base": a, "change": {"flip_enc": "password"}, "expect": "bad_proof"},
        {"case": "open", "name": "two ciphertexts swapped after they were authenticated", "base": a, "change": {"swap": ["password", "token"]}, "expect": "bad_proof"},
        {"case": "open", "name": "a value removed after it was authenticated", "base": a, "change": {"drop": "pin"}, "expect": "bad_proof"},
        {"case": "open", "name": "values authenticated for another request", "base": a, "change": {"receiver_request_id": "another"}, "expect": "bad_proof"},
        {"case": "open", "name": "values from a sender nobody expected", "base": a, "change": {"expected_sender": "sha256:0000000000000000000000000000000000000000000000000000000000000000"}, "expect": "unknown_identity"},
        {"case": "open", "name": "values authenticated by a sender nobody knows", "base": a, "change": {"trusted": false}, "expect": "unknown_identity"},
        {"case": "open", "name": "values for another receiver key", "base": a, "change": {"other_receiver": true}, "expect": "bad_proof"},
        {"case": "open", "name": "an authenticated ciphertext made for another field", "base": a, "change": {"reseal_as_field": "token"}, "expect": "open"},
        {"case": "open", "name": "an authenticated ciphertext made for another request", "base": a, "change": {"reseal_request_id": "another"}, "expect": "open"},
        {"case": "open", "name": "an authenticated ciphertext made for the other method", "base": a, "change": {"reseal_method": "show"}, "expect": "open"},
        {"case": "open", "name": "an authenticated ciphertext made for another form", "base": a, "change": {"reseal_message": "Send the money"}, "expect": "open"},
        {"case": "open", "name": "an authenticated value that does not fit its question", "base": a, "change": {"seal_value": "not a number"}, "expect": "field"},
        {"case": "verify", "name": "a link binding from an end that is not known", "base": c, "change": {"trusted": false}, "expect": "unknown_identity"},
        {"case": "verify", "name": "a link binding under another link's secret", "base": c, "change": {"other_secret": OTHER_LINK_SECRET}, "expect": "unknown_identity"},
        {"case": "verify", "name": "a link binding whose proof changed", "base": c, "change": {"flip_proof": true}, "expect": "bad_proof"},
        {"case": "verify", "name": "a link request key changed after it was bound", "base": c, "change": {"flip_key": true}, "expect": "bad_proof"},
        {"case": "verify", "name": "a link binding for another request", "base": c, "change": {"request_id": "another"}, "expect": "bad_proof"},
        {"case": "verify", "name": "a link binding for another session", "base": c, "change": {"session_id": "s2"}, "expect": "bad_proof"},
        {"case": "verify", "name": "a link binding for another form", "base": c, "change": {"schema": schema_text().replace("\"writeOnly\":true", "\"writeOnly\":false")}, "expect": "bad_proof"},
        {"case": "verify", "name": "a link binding for another message", "base": c, "change": {"message": "Send the money"}, "expect": "bad_proof"},
        {"case": "verify", "name": "a link binding for the other method", "base": c, "change": {"method": "show"}, "expect": "bad_proof"},
        {"case": "verify", "name": "a link binding that ended", "base": c, "change": {"now": NOW + LIFETIME + 31}, "expect": "expired"},
        {"case": "verify", "name": "a link binding made for another end", "base": c, "change": {"own_id": "sha256:1111111111111111111111111111111111111111111111111111111111111111"}, "expect": "wrong_audience"},
        {"case": "verify", "name": "a link binding taken by a side that is not an end of the link", "base": c, "change": {"no_agreement": true}, "expect": "unsupported"},
        {"case": "verify", "name": "a link binding with a tag of another shape", "base": c, "change": {"truncate_binding_key": true}, "expect": "key"},
        {"case": "verify", "name": "a link binding named as the Diffie-Hellman method", "base": c, "change": {"binding_method": "x25519-hmac-sha256"}, "expect": "key"},
        {"case": "verify", "name": "a link binding named as the signature method", "base": c, "change": {"binding_method": "ecdsa-p256-sha256"}, "expect": "key"},
        {"case": "open", "name": "values for the link's host, untouched", "base": c, "change": {}, "expect": "ok"},
        {"case": "open", "name": "link values under another link's secret", "base": c, "change": {"other_secret": OTHER_LINK_SECRET}, "expect": "unknown_identity"},
        {"case": "open", "name": "link values from an end nobody knows", "base": c, "change": {"trusted": false}, "expect": "unknown_identity"},
        {"case": "open", "name": "link values authenticated for another request", "base": c, "change": {"receiver_request_id": "another"}, "expect": "bad_proof"},
        {"case": "open", "name": "a link ciphertext changed after it was authenticated", "base": c, "change": {"flip_ct": "password"}, "expect": "bad_proof"},
        {"case": "open", "name": "a link plain value changed after it was authenticated", "base": c, "change": {"plain": "{\"user\":\"mallory\"}"}, "expect": "bad_proof"},
        {"case": "open", "name": "link values authenticated for another form", "base": c, "change": {"reseal_message": "Send the money"}, "expect": "open"},
        {"case": "open", "name": "values for a noise receiver, untouched", "base": b, "change": {}, "expect": "ok"},
        {"case": "open", "name": "a noise ciphertext changed after it was authenticated", "base": b, "change": {"flip_ct": "token"}, "expect": "bad_proof"}
    ])
}

fn flipped(mut bytes: Vec<u8>) -> Vec<u8> {
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    bytes
}

fn find<'a>(items: &'a Value, name: &str) -> &'a Value {
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == name)
        .unwrap_or_else(|| panic!("no case {name}"))
}

fn context_of(case: &Value, change: &Value) -> Context {
    let pick = |key: &str| {
        change
            .get(key)
            .map_or_else(|| case["context"][key].clone(), Clone::clone)
    };
    Context {
        session_id: pick("session_id").as_str().unwrap().into(),
        request_id: pick("request_id").as_str().unwrap().into(),
        message: pick("message").as_str().unwrap().into(),
        schema: pick("schema").as_str().unwrap().into(),
        form: pick("form").as_str().map(String::from),
        values: pick("values").as_str().map(String::from),
    }
}

fn id_of(text: &str) -> IdentityId {
    text.parse().unwrap()
}

/// The sender's side verifies the receiver's key binding.
fn verify_case(case: &Value, change: &Value) -> Result<IdentityId, Error> {
    let ctx = context_of(case, change);
    let method = method_of(
        change["method"]
            .as_str()
            .unwrap_or_else(|| case["method"].as_str().unwrap()),
    );
    let kind_name = case["kind"].as_str().unwrap();
    let tuning = Tuning {
        own: change["own_id"].as_str().map(id_of),
        trusts: (change["trusted"] != false).then(|| id_of(case["receiver_id"].as_str().unwrap())),
        no_agreement: change["no_agreement"] == true,
        max_lifetime: change["max_lifetime"].as_u64(),
        skew: change["skew"].as_u64(),
        secret: change["other_secret"].as_str().map(hex),
        receiver: false,
    };
    let builder_verifier = verifier(kind_name, case["sender_private"].as_str().unwrap(), &tuning);
    let mut recipient = Recipient::from_value(&case["recipient"]).unwrap();
    if change["flip_key"] == true {
        recipient.key = flipped(recipient.key.to_vec()).try_into().unwrap();
    }
    if change["flip_proof"] == true {
        recipient.binding.proof = flipped(recipient.binding.proof.clone());
    }
    if change["high_s"] == true {
        // The same signature written with s' = n - s.
        let signature = p256::ecdsa::Signature::from_slice(&recipient.binding.proof).unwrap();
        let (r, s) = signature.split_scalars();
        recipient.binding.proof = p256::ecdsa::Signature::from_scalars(r, -*s)
            .unwrap()
            .to_bytes()
            .to_vec();
    }
    if let Some(name) = change["binding_method"].as_str() {
        recipient.binding.method = name.into();
    }
    if change["truncate_binding_key"] == true {
        recipient.binding.key.truncate(10);
    }
    let expected = change["expected"]
        .as_str()
        .map_or_else(|| id_of(case["receiver_id"].as_str().unwrap()), id_of);
    let now = change["now"]
        .as_u64()
        .unwrap_or_else(|| case["now"].as_u64().unwrap());
    builder_verifier
        .collector(&expected, method, &ctx, &recipient, now)
        .map(|collector| collector.holder())
}

/// The receiver's side opens the sender's values.
fn open_case(case: &Value, change: &Value) -> Result<(), Error> {
    let method = method_of(case["method"].as_str().unwrap());
    let kind_name = case["kind"].as_str().unwrap();
    let ctx = context_of(case, &json!({}));
    let digest = ctx.digest(method);
    let (prover_r, prover_s) = provers(case);
    let (id_r, id_s) = (prover_r.id(), prover_s.id());
    let ikm_r = if change["other_receiver"] == true {
        ikm(100)
    } else {
        hex(case["recipient_ikm"].as_str().unwrap())
    };
    let (private, public) = key_pair(&ikm_r);
    let receiver_ctx = if let Some(id) = change["receiver_request_id"].as_str() {
        Context {
            request_id: id.into(),
            ..ctx.clone()
        }
    } else {
        ctx.clone()
    };
    let receiver = Receiver::from_key_pair(
        method,
        &receiver_ctx,
        &Limits::new(),
        &prover_r,
        &id_s,
        NOW,
        DEFAULT_LIFETIME,
        private,
        &public,
    )?;
    let own_recipient = Recipient::from_value(&case["recipient"]).unwrap();
    let tuning = Tuning {
        trusts: (change["trusted"] != false).then_some(id_s),
        secret: change["other_secret"].as_str().map(hex),
        receiver: true,
        ..Tuning::default()
    };
    let verifier_r = verifier(
        kind_name,
        case["receiver_private"].as_str().unwrap(),
        &tuning,
    );

    let mut sealed = BTreeMap::new();
    for field in case["fields"].as_array().unwrap() {
        sealed.insert(
            field["field"].as_str().unwrap().to_string(),
            Sealed::from_value(&field["sealed"]).unwrap(),
        );
    }
    let plain_used = change["plain"]
        .as_str()
        .unwrap_or_else(|| case["plain"].as_str().unwrap());
    let reseal = change.as_object().is_some_and(|map| {
        map.keys()
            .any(|key| key.starts_with("reseal_") || key == "seal_value" || key == "authenticated")
    });
    let mut auth = super::Binding::from_value(&case["auth"]).unwrap();
    if reseal {
        // An authenticated sender who sealed the wrong thing: the binding is valid, the ciphertext is not for this field.
        let (as_field, request_id, seal_method, seal_digest) = (
            change["reseal_as_field"].as_str(),
            change["reseal_request_id"]
                .as_str()
                .unwrap_or(&ctx.request_id),
            change["reseal_method"].as_str().map_or(method, method_of),
            change["reseal_message"].as_str().map_or(digest, |message| {
                Context {
                    message: message.into(),
                    ..ctx.clone()
                }
                .digest(method)
            }),
        );
        for (name, value) in case["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f["field"].as_str().unwrap(), f["value"].as_str().unwrap()))
            .filter(|(name, _)| change["authenticated_drop"].as_str() != Some(name))
        {
            let target = if name == "password" {
                as_field.unwrap_or(name)
            } else {
                name
            };
            let value = if name == "pin" {
                change["seal_value"].as_str().unwrap_or(value)
            } else {
                value
            };
            let made = seal_with(
                &own_recipient.key,
                seal_method,
                request_id,
                &seal_digest,
                target,
                value.as_bytes(),
                &ikm(32),
            );
            sealed.insert(name.to_string(), made);
        }
        if let Some(name) = change["authenticated_drop"].as_str() {
            sealed.remove(name);
        }
        let payload = values_digest(plain_used, &sealed);
        auth = prover_s
            .bind(
                &Statement {
                    kind: Kind::Values,
                    method,
                    request_id: &ctx.request_id,
                    digest: &digest,
                    key: &own_recipient.key,
                    payload: &payload,
                },
                NOW + change["auth_lifetime"].as_u64().unwrap_or(LIFETIME),
                &id_r,
            )
            .unwrap();
    }
    if let Some(name) = change["flip_ct"].as_str() {
        let value = sealed.get_mut(name).unwrap();
        value.ct = flipped(value.ct.clone());
    }
    if let Some(name) = change["flip_enc"].as_str() {
        let value = sealed.get_mut(name).unwrap();
        value.enc = flipped(value.enc.to_vec()).try_into().unwrap();
    }
    if let Some(pair) = change["swap"].as_array() {
        let (x, y) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
        let (vx, vy) = (sealed[x].clone(), sealed[y].clone());
        sealed.insert(x.into(), vy);
        sealed.insert(y.into(), vx);
    }
    if let Some(name) = change["drop"].as_str() {
        sealed.remove(name);
    }
    let expected = change["expected_sender"].as_str().map_or(id_s, id_of);
    receiver
        .open_values(
            &verifier_r,
            &expected,
            plain_used,
            &sealed,
            &auth,
            change["open_now"].as_u64().unwrap_or(NOW),
        )
        .map(|_| ())
}

/// A receiver made for a context with one thing changed: the context itself is refused, before any binding.
fn receiver_case(case: &Value, change: &Value) -> Result<(), Error> {
    let method = method_of(case["method"].as_str().unwrap());
    let ctx = context_of(case, change);
    let (prover_r, prover_s) = provers(case);
    let (private, public) = key_pair(&hex(case["recipient_ikm"].as_str().unwrap()));
    Receiver::from_key_pair(
        method,
        &ctx,
        &Limits::new(),
        &prover_r,
        &prover_s.id(),
        NOW,
        DEFAULT_LIFETIME,
        private,
        &public,
    )
    .map(|_| ())
}

/// A sender asked to sign an answer the form does not allow.
fn authenticate_case(case: &Value, change: &Value) -> Result<(), Error> {
    let method = method_of(case["method"].as_str().unwrap());
    let ctx = context_of(case, &json!({}));
    let (prover_r, prover_s) = provers(case);
    let recipient = Recipient::from_value(&case["recipient"]).unwrap();
    let tuning = Tuning {
        trusts: Some(prover_r.id()),
        ..Tuning::default()
    };
    let sender = verifier(
        case["kind"].as_str().unwrap(),
        case["sender_private"].as_str().unwrap(),
        &tuning,
    );
    let collector = sender
        .collector(&prover_r.id(), method, &ctx, &recipient, NOW)
        .unwrap();
    let mut sealed = BTreeMap::new();
    for field in case["fields"].as_array().unwrap() {
        let name = field["field"].as_str().unwrap();
        if change["drop"].as_str() != Some(name) {
            sealed.insert(
                name.to_string(),
                Sealed::from_value(&field["sealed"]).unwrap(),
            );
        }
    }
    let plain = change["plain"]
        .as_str()
        .unwrap_or_else(|| case["plain"].as_str().unwrap());
    collector
        .authenticate(&prover_s, plain, &sealed, NOW, DEFAULT_LIFETIME)
        .map(|_| ())
}

/// Everything the vectors say, checked from the file alone.
fn check(doc: &Value) {
    // RFC 9180: the key schedule, the encapsulation and the AEAD of the suite, through hpke with the RFC's ephemeral key.
    let rfc = &doc["hpke"];
    let (sk_r, pk_r) = key_pair(&hex(rfc["ikmR"].as_str().unwrap()));
    assert_eq!(
        HEXLOWER.encode(&pk_r.to_bytes()),
        rfc["pkRm"].as_str().unwrap()
    );
    let (_, pk_e) = key_pair(&hex(rfc["ikmE"].as_str().unwrap()));
    assert_eq!(
        HEXLOWER.encode(&pk_e.to_bytes()),
        rfc["pkEm"].as_str().unwrap()
    );
    let first = &rfc["encryptions"][0];
    assert_eq!(first["seq"], 0);
    let aad = hex(first["aad"].as_str().unwrap());
    let info = hex(rfc["info"].as_str().unwrap());
    let pk: [u8; 32] = pk_r.to_bytes().as_slice().try_into().unwrap();
    let made = seal_raw(
        &pk,
        &info,
        &hex(first["pt"].as_str().unwrap()),
        &aad,
        &hex(rfc["ikmE"].as_str().unwrap()),
    );
    assert_eq!(HEXLOWER.encode(&made.enc), rfc["enc"].as_str().unwrap());
    assert_eq!(HEXLOWER.encode(&made.ct), first["ct"].as_str().unwrap());
    assert_eq!(
        open_raw(&sk_r, &info, &aad, &made).unwrap(),
        hex(first["pt"].as_str().unwrap())
    );
    assert!(
        open_raw(&sk_r, &info, b"Count-1", &made).is_err(),
        "the aad is bound"
    );

    for item in doc["grammar"].as_array().unwrap() {
        let text = item["text"].as_str().unwrap();
        let valid = match item["kind"].as_str().unwrap() {
            "integer" => json_integer(text).is_some(),
            "number" => json_number(text).is_some(),
            "base64url" => base64url_canonical(text),
            _ => base64_canonical(text),
        };
        assert_eq!(valid, item["valid"], "{} {text:?}", item["kind"]);
    }

    for case in doc["bindings"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let method = method_of(case["method"].as_str().unwrap());
        let ctx = context_of(case, &json!({}));
        assert_eq!(
            HEXLOWER.encode(&ctx.digest(method)),
            case["context_digest"].as_str().unwrap(),
            "{name}"
        );
        let (prover_r, prover_s) = provers(case);
        assert_eq!(
            prover_r.id().to_string(),
            case["receiver_id"].as_str().unwrap()
        );
        assert_eq!(
            prover_s.id().to_string(),
            case["sender_id"].as_str().unwrap()
        );

        // The receiver's key binding is the bytes the vector holds, and verifies at the sender.
        let (private, public) = key_pair(&hex(case["recipient_ikm"].as_str().unwrap()));
        let remade = Receiver::from_key_pair(
            method,
            &ctx,
            &Limits::new(),
            &prover_r,
            &prover_s.id(),
            NOW,
            DEFAULT_LIFETIME,
            private,
            &public,
        )
        .unwrap();
        assert_eq!(remade.recipient().to_value(), case["recipient"], "{name}");
        assert_eq!(
            verify_case(case, &json!({})).unwrap(),
            prover_r.id(),
            "{name}"
        );
        open_case(case, &json!({})).unwrap_or_else(|error| panic!("{name}: {error}"));

        // Sealing again with the same ephemeral keys, and authenticating again, gives the same bytes.
        let tuning = Tuning {
            trusts: Some(prover_r.id()),
            ..Tuning::default()
        };
        let verifier_s = verifier(
            case["kind"].as_str().unwrap(),
            case["sender_private"].as_str().unwrap(),
            &tuning,
        );
        let collector = verifier_s
            .collector(&prover_r.id(), method, &ctx, remade.recipient(), NOW)
            .unwrap();
        let mut sealed = BTreeMap::new();
        for field in case["fields"].as_array().unwrap() {
            let (field_name, value) = (
                field["field"].as_str().unwrap(),
                field["value"].as_str().unwrap(),
            );
            let stored = Sealed::from_value(&field["sealed"]).unwrap();
            let fresh = collector.seal(field_name, value).unwrap();
            assert_ne!(fresh, stored, "a fresh sealing uses a fresh ephemeral key");
            sealed.insert(field_name.to_string(), stored);
        }
        assert_eq!(
            HEXLOWER.encode(&values_digest(case["plain"].as_str().unwrap(), &sealed)),
            case["values_digest"].as_str().unwrap()
        );
        let auth = collector
            .authenticate(
                &prover_s,
                case["plain"].as_str().unwrap(),
                &sealed,
                NOW,
                DEFAULT_LIFETIME,
            )
            .unwrap();
        assert_eq!(auth.to_value(), case["auth"], "{name}");
        // The values open for the receiver.
        let (private, public) = key_pair(&hex(case["recipient_ikm"].as_str().unwrap()));
        let digest = ctx.digest(method);
        for field in case["fields"].as_array().unwrap() {
            let opened = open_with(
                &private,
                method,
                &ctx.request_id,
                &digest,
                field["field"].as_str().unwrap(),
                &Sealed::from_value(&field["sealed"]).unwrap(),
            )
            .unwrap();
            assert_eq!(opened, field["value"].as_str().unwrap().as_bytes());
        }
        let _ = public;
    }

    for item in doc["forms"]["accepted"].as_array().unwrap() {
        let form = Form::parse_text(item["schema"].as_str().unwrap(), &Limits::new())
            .unwrap_or_else(|error| panic!("{}: {error}", item["name"]));
        let names = |secret: bool| -> Vec<String> {
            form.fields
                .iter()
                .filter(|f| f.secret == secret)
                .map(|f| f.field.name.clone())
                .collect()
        };
        assert_eq!(
            names(true),
            item["secret"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            names(false),
            item["plain"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        );
    }
    for item in doc["forms"]["refused"].as_array().unwrap() {
        let error = Form::parse_text(item["schema"].as_str().unwrap(), &Limits::new())
            .expect_err(item["name"].as_str().unwrap());
        assert_eq!(
            kind(&error),
            item["kind"].as_str().unwrap(),
            "{}",
            item["name"]
        );
    }

    for item in doc["layouts"]["accepted"].as_array().unwrap() {
        let form = Form::parse_text(item["schema"].as_str().unwrap(), &Limits::new()).unwrap();
        form.check_layout(item["layout"].as_str().unwrap())
            .unwrap_or_else(|error| panic!("{}: {error}", item["name"]));
    }
    for item in doc["layouts"]["refused"].as_array().unwrap() {
        let form = Form::parse_text(item["schema"].as_str().unwrap(), &Limits::new()).unwrap();
        let error = form
            .check_layout(item["layout"].as_str().unwrap())
            .expect_err(item["name"].as_str().unwrap());
        assert_eq!(
            kind(&error),
            item["kind"].as_str().unwrap(),
            "{}",
            item["name"]
        );
    }

    for item in doc["refusals"].as_array().unwrap() {
        let case = find(&doc["bindings"], item["base"].as_str().unwrap());
        let result = match item["case"].as_str().unwrap() {
            "verify" => verify_case(case, &item["change"]).map(|_| ()),
            "open" => open_case(case, &item["change"]),
            "receiver" => receiver_case(case, &item["change"]),
            _ => authenticate_case(case, &item["change"]),
        };
        let name = item["name"].as_str().unwrap();
        match (item["expect"].as_str().unwrap(), result) {
            ("ok", Ok(())) => {}
            ("ok", Err(error)) => panic!("{name}: refused: {error}"),
            (_, Ok(())) => panic!("{name}: taken"),
            (want, Err(error)) => assert_eq!(kind(&error), want, "{name}: {error}"),
        }
    }
}

/// The vectors this crate writes: everything but the RFC's section.
fn generate() -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("grammar".into(), grammar());
    out.insert("bindings".into(), Value::Array(base_cases()));
    out.insert("forms".into(), forms());
    out.insert("layouts".into(), layouts());
    out.insert("refusals".into(), refusals());
    // A lone surrogate cannot be written in JSON or held by a Rust string: the Rust tests only check the escape is refused where a
    // text is read (forms.refused, layouts.refused). Clients whose strings can hold one must refuse it in every hashed part.
    out.insert(
        "surrogates".into(),
        json!([
            {"part": "session_id", "units": "d800", "expect": "malformed"},
            {"part": "request_id", "units": "dc00", "expect": "malformed"},
            {"part": "message", "units": "d800", "expect": "malformed"},
            {"part": "schema", "units": "d83d", "expect": "malformed"},
            {"part": "form", "units": "dfff", "expect": "malformed"},
            {"part": "values", "units": "d800", "expect": "malformed"},
            {"part": "plain", "units": "d800", "expect": "malformed"},
            {"part": "field_name", "units": "d800", "expect": "schema"}
        ]),
    );
    out
}

#[test]
fn vectors_hold() {
    check(&serde_json::from_str(VECTORS).expect("vectors/secret.json is JSON"));
}

/// Writes `vectors/secret.json` when `COQUI_ACP_WRITE_VECTORS` is set; otherwise says the file is what this code makes.
#[test]
fn vectors_are_current() {
    let mut doc: Map<String, Value> =
        serde_json::from_str(VECTORS).expect("vectors/secret.json is JSON");
    let made = generate();
    if std::env::var_os("COQUI_ACP_WRITE_VECTORS").is_some() {
        doc.remove("sealed");
        doc.remove("info");
        for (key, value) in made {
            doc.insert(key, value);
        }
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vectors/secret.json");
        let mut text = serde_json::to_string_pretty(&Value::Object(doc)).unwrap();
        text.push('\n');
        std::fs::write(path, text).unwrap();
        return;
    }
    for (key, value) in made {
        assert_eq!(
            doc.get(&key),
            Some(&value),
            "{key} differs from what the code makes; rerun with COQUI_ACP_WRITE_VECTORS=1"
        );
    }
}

/// A recipient key of low order, bound as a receiver would, is refused when a collector seals to it.
#[test]
fn a_recipient_key_of_low_order_is_a_seal_refusal() {
    let case = &base_cases()[0];
    let (prover_r, prover_s) = provers(case);
    let ctx = context_of(case, &json!({}));
    let method = method_of(case["method"].as_str().unwrap());
    let digest = ctx.digest(method);
    let key = [0u8; 32];
    let binding = prover_r
        .bind(
            &Statement {
                kind: Kind::Key,
                method,
                request_id: &ctx.request_id,
                digest: &digest,
                key: &key,
                payload: &[],
            },
            NOW + LIFETIME,
            &prover_s.id(),
        )
        .unwrap();
    let tuning = Tuning {
        trusts: Some(prover_r.id()),
        ..Tuning::default()
    };
    let collector = verifier(
        case["kind"].as_str().unwrap(),
        case["sender_private"].as_str().unwrap(),
        &tuning,
    )
    .collector(
        &prover_r.id(),
        method,
        &ctx,
        &Recipient { key, binding },
        NOW,
    )
    .unwrap();
    assert_eq!(
        collector.seal("password", "correct horse").unwrap_err(),
        Error::Seal
    );
}

#[test]
fn a_link_secret_must_be_long_enough_and_each_end_is_its_own_role() {
    assert!(matches!(LinkKey::derive(&[7u8; 15]), Err(Error::Key(_))));
    let link = LinkKey::derive(&hex(LINK_SECRET)).unwrap();
    assert_ne!(link.identity(LinkRole::Host), link.identity(LinkRole::App));
    // A verifier is built for the role it holds.
    assert!(
        Verifier::builder(link.identity(LinkRole::Host))
            .link(&link, LinkRole::App)
            .is_err()
    );
    // Another secret is another pair of ends.
    let other = LinkKey::derive(&hex(OTHER_LINK_SECRET)).unwrap();
    assert_ne!(
        link.identity(LinkRole::Host),
        other.identity(LinkRole::Host)
    );
}

/// Both tags are on the wire, so a party that has seen one request still cannot make a binding without the secret.
#[test]
fn a_binding_made_from_the_public_tags_alone_is_refused() {
    let case = &base_cases()[2];
    let (prover_r, prover_s) = provers(case);
    let link = LinkKey::derive(&hex(LINK_SECRET)).unwrap();
    let ctx = context_of(case, &json!({}));
    let method = method_of(case["method"].as_str().unwrap());
    let digest = ctx.digest(method);
    let (_, public) = key_pair(&ikm(0));
    let key: [u8; 32] = public.to_bytes().as_slice().try_into().unwrap();
    let statement = Statement {
        kind: Kind::Key,
        method,
        request_id: &ctx.request_id,
        digest: &digest,
        key: &key,
        payload: &[],
    };
    let mut forged = prover_r
        .bind(&statement, NOW + LIFETIME, &prover_s.id())
        .unwrap();
    // The tags are public: the proof of someone without the secret is a MAC under some other key.
    let thief = Prover::linked(
        &LinkKey::derive(&hex(OTHER_LINK_SECRET)).unwrap(),
        LinkRole::Host,
    );
    let other_id = LinkKey::derive(&hex(OTHER_LINK_SECRET))
        .unwrap()
        .identity(LinkRole::App);
    forged.proof = thief
        .bind(&statement, NOW + LIFETIME, &other_id)
        .unwrap()
        .proof;
    let app = Verifier::builder(link.identity(LinkRole::App))
        .link(&link, LinkRole::App)
        .unwrap()
        .build();
    assert_eq!(
        app.verify(&statement, &forged, NOW).unwrap_err(),
        Error::BadProof
    );
    // A prover refuses to bind for anyone but the other end.
    assert_eq!(
        prover_r
            .bind(&statement, NOW + LIFETIME, &link.identity(LinkRole::Host))
            .unwrap_err(),
        Error::WrongAudience
    );
    // The host's own binding does not verify at the host: no reflection.
    let own = prover_r
        .bind(&statement, NOW + LIFETIME, &prover_s.id())
        .unwrap();
    let host = Verifier::builder(link.identity(LinkRole::Host))
        .link(&link, LinkRole::Host)
        .unwrap()
        .build();
    assert!(host.verify(&statement, &own, NOW).is_err());
    assert!(app.verify(&statement, &own, NOW).is_ok());
}
