//! ModEngine.Templating–compatible renderer.
//!
//! Ports the observable behavior of `ModEngine.Templating` 0.0.0-preview.0.9
//! (Fluid-based) as used by the Sicario merger's `PatchTemplateBehaviour`:
//!
//! * `{{ value | filter: args }}` segments are rendered with the Sicario
//!   filter set (`float`, `string`, `int`, `amp`, `row`, `bool`, `int16`,
//!   `not`, `byte`, `random`, `word`, `array`, `join`) plus Fluid's default
//!   `times`.
//! * `vars.x` resolves against the mod's rendered variables; `inputs.x`
//!   against request inputs. Unknown identifiers render as empty strings
//!   (Fluid non-strict default) — this is why hosted-app builtins like
//!   `{{DB_Aircraft.CannonType}}` become empty in local builds.
//! * If the input fails to parse as a template (unknown filter, unsupported
//!   tag), the *entire* input is returned unchanged (TryParse-fail path).
//!
//! Hex-format outputs use .NET `BitConverter.ToString` conventions:
//! uppercase, dash-separated (`B4-43-00-00`).

use std::collections::HashMap;

pub type Vars = HashMap<String, String>;

/// Render a template string the way the C# merger does.
pub fn render(input: &str, inputs: &Vars, vars: &Vars) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Unsupported tag (`{% ... %}`) anywhere => whole input unrendered.
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'%' {
            return input.to_string();
        }
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            let rest = &input[i + 2..];
            let Some(end) = rest.find("}}") else {
                // Unclosed => Fluid parse fails => raw.
                return input.to_string();
            };
            let expr = rest[..end].trim();
            match render_expr(expr, inputs, vars) {
                Some(text) => out.push_str(&text),
                // Parse failure => whole input raw (TryParse-fail semantics).
                None => return input.to_string(),
            }
            i += 2 + end + 2;
            continue;
        }
        // Copy one UTF-8 char.
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&input[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// A Fluid-ish value: string or number.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Str(String),
    Int(i64),
    Float(f64),
}

impl Value {
    fn to_string_value(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => fmt_float(*f),
        }
    }

    fn to_number(&self) -> f64 {
        match self {
            Value::Int(n) => *n as f64,
            Value::Float(f) => *f,
            Value::Str(s) => s.trim().parse::<f64>().unwrap_or(0.0),
        }
    }

    fn is_integer(&self) -> bool {
        matches!(self, Value::Int(_))
    }
}

/// .NET-ish float formatting (invariant, minimal decimals).
fn fmt_float(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{:.1}", f)
    } else {
        let s = format!("{}", f);
        s
    }
}

/// Render one `{{ ... }}` expression. Returns None on parse failure.
fn render_expr(expr: &str, inputs: &Vars, vars: &Vars) -> Option<String> {
    let parts = split_pipeline(expr)?;
    let (value_part, filter_parts) = parts.split_first()?;
    let mut value = parse_value(value_part.trim(), inputs, vars)?;
    for fp in filter_parts {
        value = apply_filter(fp.trim(), value)?;
    }
    Some(value.to_string_value())
}

/// Split `value | filter: a, b | filter2` on top-level `|` (outside quotes).
fn split_pipeline(expr: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in expr.chars() {
        match quote {
            Some(q) => {
                cur.push(ch);
                if ch == q {
                    quote = None;
                }
            }
            None => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    cur.push(ch);
                }
                '|' => {
                    parts.push(cur.trim().to_string());
                    cur.clear();
                }
                _ => cur.push(ch),
            },
        }
    }
    parts.push(cur.trim().to_string());
    Some(parts)
}

/// Parse a literal / identifier.
fn parse_value(s: &str, inputs: &Vars, vars: &Vars) -> Option<Value> {
    if s.is_empty() {
        return None;
    }
    if let Some(stripped) = s.strip_prefix('\'') {
        return Some(Value::Str(stripped.strip_suffix('\'')?.to_string()));
    }
    if let Some(stripped) = s.strip_prefix('"') {
        return Some(Value::Str(stripped.strip_suffix('"')?.to_string()));
    }
    if let Ok(n) = s.parse::<i64>() {
        return Some(Value::Int(n));
    }
    if let Ok(f) = s.parse::<f64>() {
        return Some(Value::Float(f));
    }
    // Identifiers: vars.x / inputs.x / anything else => undefined => empty.
    if let Some(key) = s.strip_prefix("vars.") {
        return Some(Value::Str(vars.get(key).cloned().unwrap_or_default()));
    }
    if let Some(key) = s.strip_prefix("inputs.") {
        return Some(Value::Str(inputs.get(key).cloned().unwrap_or_default()));
    }
    // Fluid non-strict: undefined identifiers render empty.
    Some(Value::Str(String::new()))
}

fn parse_args(argstr: &str) -> Vec<Value> {
    if argstr.trim().is_empty() {
        return Vec::new();
    }
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in argstr.chars() {
        match quote {
            Some(q) => {
                cur.push(ch);
                if ch == q {
                    quote = None;
                }
            }
            None => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    cur.push(ch);
                }
                ',' => {
                    args.push(cur.trim().to_string());
                    cur.clear();
                }
                _ => cur.push(ch),
            },
        }
    }
    args.push(cur.trim().to_string());
    args.into_iter()
        .map(|a| {
            let a = a.trim();
            if let Some(s) = a.strip_prefix('\'') {
                Value::Str(s.strip_suffix('\'').unwrap_or(s).to_string())
            } else if let Some(s) = a.strip_prefix('"') {
                Value::Str(s.strip_suffix('"').unwrap_or(s).to_string())
            } else if let Ok(n) = a.parse::<i64>() {
                Value::Int(n)
            } else if let Ok(f) = a.parse::<f64>() {
                Value::Float(f)
            } else {
                Value::Str(a.to_string())
            }
        })
        .collect()
}

/// .NET `BitConverter.ToString` format: uppercase, dash-separated.
fn dash_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join("-")
}

/// The exact filter set from ModEngine.Templating's `WithFilters`.
fn apply_filter(spec: &str, input: Value) -> Option<Value> {
    let (name, argstr) = match spec.split_once(':') {
        Some((n, a)) => (n.trim(), a),
        None => (spec.trim(), ""),
    };
    let args = parse_args(argstr);
    let out = match name {
        "float" => {
            let f = input.to_number() as f32;
            Value::Str(dash_hex(&f.to_le_bytes()))
        }
        "string" => Value::Str(dash_hex(input.to_string_value().as_bytes())),
        "int" => {
            // Convert.ToInt32: banker's rounding.
            let n = round_to_i32(input.to_number());
            Value::Str(dash_hex(&n.to_le_bytes()))
        }
        "int16" => {
            let n = round_to_i32(input.to_number()) as i16;
            Value::Str(dash_hex(&n.to_le_bytes()))
        }
        "byte" => {
            let s = input.to_string_value();
            let n: u8 = s.trim().parse().ok()?;
            Value::Str(dash_hex(&[n]))
        }
        "bool" => {
            let s = input.to_string_value();
            let b = parse_bool(&s)?;
            Value::Str(dash_hex(&[if b { 1 } else { 0 }]))
        }
        "not" => {
            let s = input.to_string_value();
            match parse_bool(&s) {
                Some(b) => Value::Str(if b { "False" } else { "True" }.to_string()),
                None => Value::Str(s),
            }
        }
        "amp" => {
            // AmplifyInput: n = arg0; total = arg1 ?? 100;
            // x > total ? x*n : x*(total-n)
            let n = args.first().map(|v| v.to_number()).unwrap_or(0.0);
            let total = args.get(1).map(|v| v.to_number()).unwrap_or(100.0);
            let x = input.to_number();
            let out = if x > total { x * n } else { x * (total - n) };
            if out.fract() == 0.0 {
                Value::Int(out as i64)
            } else {
                Value::Float(out)
            }
        }
        "row" | "word" => {
            // len+1 as i32 LE + bytes, no terminator.
            let s = input.to_string_value();
            let b = s.as_bytes();
            let mut v = Vec::with_capacity(4 + b.len());
            v.extend_from_slice(&((b.len() + 1) as i32).to_le_bytes());
            v.extend_from_slice(b);
            Value::Str(dash_hex(&v))
        }
        "random" => {
            let a = args.first().map(|v| v.to_number()).unwrap_or(0.0);
            let b = args.get(1).map(|v| v.to_number()).unwrap_or(0.0);
            let both_int = args.len() >= 2
                && args
                    .iter()
                    .take(2)
                    .all(|v| v.is_integer() && v.to_number() >= 0.0);
            if both_int {
                // Random.Next(min, max) — max exclusive.
                let (lo, hi) = (a as i64, b as i64);
                Value::Int(rand_int(lo, hi))
            } else {
                Value::Float(rand_float(a as f32, b as f32))
            }
        }
        "array" => {
            let filler = args
                .first()
                .map(|v| v.to_string_value())
                .unwrap_or_else(|| "16 02 00 00 00 00 00 00 00".to_string());
            let s = input.to_string_value();
            let items: Vec<&str> = s.split('|').collect();
            let mut list: Vec<u8> = Vec::new();
            list.extend_from_slice(&(items.len() as i32).to_le_bytes());
            for item in items {
                if !item.is_empty() && item.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(n) = item.parse::<i32>() {
                        list.extend_from_slice(&n.to_le_bytes());
                        continue;
                    }
                }
                let b = item.as_bytes();
                list.extend_from_slice(&((b.len() + 1) as i32).to_le_bytes());
                list.extend_from_slice(b);
                list.push(0);
            }
            let total = (list.len() as i64).to_le_bytes();
            Some(Value::Str(format!(
                "{}{}{}",
                dash_hex(&total),
                filler,
                dash_hex(&list)
            )))?
        }
        "join" => {
            let count = args.first().map(|v| v.to_number()).unwrap_or(0.0) as usize;
            let sep = args.get(1).map(|v| v.to_string_value()).unwrap_or_default();
            let s = input.to_string_value();
            let joined = std::iter::repeat_n(s, count).collect::<Vec<_>>().join(&sep);
            Value::Str(joined)
        }
        "times" => {
            let n = args.first().map(|v| v.to_number()).unwrap_or(0.0);
            let x = input.to_number();
            let out = x * n;
            if input.is_integer() && args.first().map(|v| v.is_integer()).unwrap_or(false) {
                Value::Int(out as i64)
            } else {
                // Fluid keeps decimal scale (e.g. 3.0m renders as "3.0").
                Value::Float(out)
            }
        }
        _ => return None,
    };
    Some(out)
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// .NET `Convert.ToInt32(double)` uses banker's rounding.
fn round_to_i32(x: f64) -> i32 {
    let rounded = x.round_ties_even();
    rounded as i32
}

fn rand_int(lo: i64, hi: i64) -> i64 {
    if hi <= lo {
        return lo;
    }
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let span = (hi - lo) as u64;
    lo + (nanos
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
        % span) as i64
}

fn rand_float(lo: f32, hi: f32) -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let frac = (nanos % 1_000_000) as f64 / 1_000_000.0;
    let v = lo as f64 + frac * (hi as f64 - lo as f64);
    // .NET NextFloat rounds to 1 decimal place by default.
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_vars() -> Vars {
        Vars::new()
    }

    #[test]
    fn renders_vars_and_inputs() {
        let vars = [("x".to_string(), "hello".to_string())].into();
        let inputs = [("y".to_string(), "world".to_string())].into();
        assert_eq!(
            render("{{ vars.x }} {{ inputs.y }}", &inputs, &vars),
            "hello world"
        );
    }

    #[test]
    fn undefined_renders_empty() {
        let vars = no_vars();
        assert_eq!(render("ED {{DB_Aircraft.CannonType}}", &vars, &vars), "ED ");
    }

    #[test]
    fn int_filter_le() {
        let v = no_vars();
        assert_eq!(render("{{ 1 | int }}", &v, &v), "01-00-00-00");
        assert_eq!(render("{{ 180 | int }}", &v, &v), "B4-00-00-00");
    }

    #[test]
    fn float_filter_le() {
        let v = no_vars();
        assert_eq!(render("{{ 180 | float }}", &v, &v), "00-00-34-43");
        assert_eq!(render("{{ 5.5 | float }}", &v, &v), "00-00-B0-40");
    }

    #[test]
    fn row_and_word_filters() {
        let v = no_vars();
        assert_eq!(
            render("{{ 'rgps' | row }}", &v, &v),
            "05-00-00-00-72-67-70-73"
        );
        assert_eq!(
            render("{{ 'rgps' | word }}", &v, &v),
            "05-00-00-00-72-67-70-73"
        );
    }

    #[test]
    fn bool_and_not_filters() {
        let v = no_vars();
        assert_eq!(render("{{ 'true' | bool }}", &v, &v), "01");
        assert_eq!(render("{{ 'false' | bool }}", &v, &v), "00");
        assert_eq!(render("{{ 'true' | not }}", &v, &v), "False");
        assert_eq!(render("{{ 'false' | not | bool }}", &v, &v), "01");
    }

    #[test]
    fn byte_filter() {
        let v = no_vars();
        assert_eq!(render("{{ 250 | byte }}", &v, &v), "FA");
    }

    #[test]
    fn times_filter() {
        let v = no_vars();
        assert_eq!(render("{{ 3 | times: 2 }}", &v, &v), "6");
        assert_eq!(render("{{ 1.5 | times: 2 }}", &v, &v), "3.0");
    }

    #[test]
    fn amp_filter() {
        let v = no_vars();
        assert_eq!(render("{{ 180 | amp: 2 }}", &v, &v), "360");
    }

    #[test]
    fn unknown_filter_keeps_raw() {
        let v = no_vars();
        let input = "{{ 'x' | bogus }}";
        assert_eq!(render(input, &v, &v), input);
    }

    #[test]
    fn tag_keeps_raw() {
        let v = no_vars();
        let input = "{% rand:1:5 %}";
        assert_eq!(render(input, &v, &v), input);
    }

    #[test]
    fn value_rendering_order_smoke() {
        // render() itself has no ordering; pipeline order lives in template.rs.
        let v = no_vars();
        assert_eq!(render("plain", &v, &v), "plain");
    }
}
