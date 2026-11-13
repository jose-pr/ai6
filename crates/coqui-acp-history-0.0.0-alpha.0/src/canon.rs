//! The canonical spelling of a line: compact, UTF-8 left as it is, the keys of a line's own
//! envelope in a fixed order, every other object's keys sorted by code point, and numbers spelled
//! by the rule in `docs/session-history.md`.
//!
//! It does not depend on the build: `serde_json`'s object order changes with its
//! `preserve_order` feature, which another crate of a build may turn on, and its number output is
//! not relied on either.

use serde_json::{Number, Value};

use crate::strict::beyond_parser;

/// `value` in canonical spelling, its top-level keys first in the order `top` gives and the rest
/// sorted. `None` when the value nests deeper than the parser would read back.
pub(crate) fn to_string(value: &Value, top: &[&str]) -> Option<String> {
    if beyond_parser(value) {
        return None;
    }
    let mut out = String::new();
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by_key(|key| {
                let place = top
                    .iter()
                    .position(|first| first == key)
                    .unwrap_or(top.len());
                (place, key.as_str())
            });
            write_object(&mut out, map, &keys);
        }
        other => write(&mut out, other),
    }
    Some(out)
}

fn write_object(out: &mut String, map: &serde_json::Map<String, Value>, keys: &[&String]) {
    out.push('{');
    for (index, key) in keys.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        string(out, key);
        out.push(':');
        if let Some(value) = map.get(key.as_str()) {
            write(out, value);
        }
    }
    out.push('}');
}

fn write(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => number(out, n),
        Value::String(text) => string(out, text),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            write_object(out, map, &keys);
        }
    }
}

fn string(out: &mut String, text: &str) {
    // A string always serialises.
    out.push_str(&serde_json::to_string(text).unwrap_or_default());
}

/// An integer in `i64` or `u64` as its digits; any other number as a double, by [`double`].
fn number(out: &mut String, n: &Number) {
    if n.is_i64() || n.is_u64() {
        out.push_str(&n.to_string());
    } else {
        double(out, n.as_f64().unwrap_or(0.0));
    }
}

/// A finite double: its shortest digits that read back to it (the closest to its exact value if
/// several, the even one if two are equally close), written plainly when the decimal exponent of
/// its first digit is from -5 to 15 (always with a point and a digit after it), else as `d.ddde`
/// and the exponent, with no `+` and no leading zeros.
pub(crate) fn double(out: &mut String, value: f64) {
    if value.is_sign_negative() {
        out.push('-');
    }
    let magnitude = value.abs();
    if magnitude == 0.0 {
        out.push_str("0.0");
        return;
    }
    let (digits, exponent) = shortest(magnitude);
    if !(-5..=15).contains(&exponent) {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push_str(&exponent.to_string());
    } else if exponent < 0 {
        out.push_str("0.");
        for _ in 0..(-exponent - 1) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let whole = exponent as usize + 1;
        if digits.len() <= whole {
            out.push_str(&digits);
            for _ in digits.len()..whole {
                out.push('0');
            }
            out.push_str(".0");
        } else {
            out.push_str(&digits[..whole]);
            out.push('.');
            out.push_str(&digits[whole..]);
        }
    }
}

/// The significant digits of a finite, positive double, with no leading or trailing zero, and the
/// decimal exponent of the first of them. They are the digits `serde_json` writes, which are the
/// shortest that read back and, among those, the closest to the double.
fn shortest(magnitude: f64) -> (String, i32) {
    let text =
        Number::from_f64(magnitude).map_or_else(|| format!("{magnitude:e}"), |n| n.to_string());
    let (mantissa, exponent) =
        text.split_once(['e', 'E'])
            .map_or((text.as_str(), 0), |(mantissa, exponent)| {
                (
                    mantissa,
                    exponent.trim_start_matches('+').parse().unwrap_or(0),
                )
            });
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let all = format!("{whole}{fraction}");
    let significant = all.trim_start_matches('0');
    let leading = all.len() - significant.len();
    let digits = significant.trim_end_matches('0');
    let exponent = exponent + whole.len() as i32 - 1 - leading as i32;
    (digits.to_owned(), exponent)
}

#[cfg(test)]
mod tests {
    use super::double;

    fn spelled(value: f64) -> String {
        let mut out = String::new();
        double(&mut out, value);
        out
    }

    #[test]
    fn the_rule_spells_the_edges() {
        for (value, text) in [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (100.0, "100.0"),
            (0.1, "0.1"),
            (0.00001, "0.00001"),
            (0.000001, "1e-6"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e16"),
            (1.5e-7, "1.5e-7"),
            (-1.25e20, "-1.25e20"),
            (f64::MAX, "1.7976931348623157e308"),
            (5e-324, "5e-324"),
            (18_446_744_073_709_551_616.0, "1.8446744073709552e19"),
        ] {
            assert_eq!(spelled(value), text, "{value:e}");
        }
    }

    #[test]
    fn the_rule_reads_back_to_the_same_double_and_keeps_serde_jsons_notation() {
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        for _ in 0..50_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let value = f64::from_bits(state);
            if !value.is_finite() {
                continue;
            }
            let text = spelled(value);
            assert_eq!(text.parse::<f64>().unwrap().to_bits(), value.to_bits());
            // serde_json writes a positive exponent as e+19: the only difference.
            let theirs = serde_json::to_string(&value).unwrap().replace("e+", "e");
            assert_eq!(text, theirs, "{value:e}");
        }
    }
}
