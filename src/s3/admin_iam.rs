//! MinIO admin API: users, groups, IAM policies, service accounts, access keys (IAM area).
//!
//! Request/response shapes follow madmin-go (`user-commands.go`, `group-commands.go`,
//! `policy-commands.go`, `idp-commands.go`). JSON documents that mc embeds verbatim (IAM
//! policies, `json.RawMessage`) are kept as [`GoJson`], which preserves key order. Times are
//! kept as the server's RFC 3339 strings; [`GoTime`] parses them for display.

use super::admin::AdminClient;
use crate::error::McError;
use anyhow::{Result, anyhow};
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// `null` decodes to the default value (Go decodes `null` into a nil slice).
fn null_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// madmin `ErrInvalidArgument(message)` (client-side validation).
pub fn madmin_invalid_argument(message: &str) -> McError {
    McError::with_detail(
        message,
        crate::detail![
            ("Code", "InvalidArgument"),
            ("Message", message),
            ("BucketName", ""),
            ("Key", ""),
            ("RequestID", "minio"),
            ("HostID", ""),
            ("Region", ""),
        ],
    )
    .with_code("InvalidArgument")
}

// ---------------------------------------------------------------------------
// Order-preserving JSON (Go json.RawMessage)
// ---------------------------------------------------------------------------

/// A JSON value that keeps object key order and duplicate keys, so documents mc passes
/// through as `json.RawMessage` re-serialize like Go's `json.Compact`.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum GoJson {
    #[default]
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<GoJson>),
    Object(Vec<(String, GoJson)>),
}

impl GoJson {
    pub fn is_null(&self) -> bool {
        matches!(self, GoJson::Null)
    }

    /// Value of the first key matching `name` case-insensitively (Go's decoding rule).
    pub fn get(&self, name: &str) -> Option<&GoJson> {
        match self {
            GoJson::Object(fields) => fields
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            GoJson::String(text) => Some(text),
            _ => None,
        }
    }

    /// Parses a JSON document.
    pub fn parse(text: &[u8]) -> serde_json::Result<GoJson> {
        serde_json::from_slice(text)
    }

    /// Compact JSON text (Go `json.Compact` with HTML escaping).
    pub fn compact(&self) -> String {
        crate::output::format_json(self, true).unwrap_or_default()
    }
}

impl Serialize for GoJson {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            GoJson::Null => serializer.serialize_unit(),
            GoJson::Bool(value) => serializer.serialize_bool(*value),
            GoJson::Number(value) => value.serialize(serializer),
            GoJson::String(value) => serializer.serialize_str(value),
            GoJson::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            GoJson::Object(fields) => {
                let mut map = serializer.serialize_map(Some(fields.len()))?;
                for (key, value) in fields {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for GoJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct GoJsonVisitor;

        impl<'de> Visitor<'de> for GoJsonVisitor {
            type Value = GoJson;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any JSON value")
            }

            fn visit_unit<E>(self) -> std::result::Result<GoJson, E> {
                Ok(GoJson::Null)
            }

            fn visit_none<E>(self) -> std::result::Result<GoJson, E> {
                Ok(GoJson::Null)
            }

            fn visit_some<D: Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> std::result::Result<GoJson, D::Error> {
                GoJson::deserialize(deserializer)
            }

            fn visit_bool<E>(self, value: bool) -> std::result::Result<GoJson, E> {
                Ok(GoJson::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> std::result::Result<GoJson, E> {
                Ok(GoJson::Number(value.into()))
            }

            fn visit_u64<E>(self, value: u64) -> std::result::Result<GoJson, E> {
                Ok(GoJson::Number(value.into()))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<GoJson, E> {
                serde_json::Number::from_f64(value)
                    .map(GoJson::Number)
                    .ok_or_else(|| E::custom("invalid number"))
            }

            fn visit_str<E>(self, value: &str) -> std::result::Result<GoJson, E> {
                Ok(GoJson::String(value.to_string()))
            }

            fn visit_string<E>(self, value: String) -> std::result::Result<GoJson, E> {
                Ok(GoJson::String(value))
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<GoJson, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(GoJson::Array(items))
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<GoJson, A::Error> {
                let mut fields = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, GoJson>()? {
                    fields.push((key, value));
                }
                Ok(GoJson::Object(fields))
            }
        }

        deserializer.deserialize_any(GoJsonVisitor)
    }
}

/// A policy string from the server as mc embeds it (`json.RawMessage(policy)`): `None` when
/// empty (omitted), else the parsed document (invalid JSON is kept as a string).
pub fn raw_policy(text: &str) -> Option<GoJson> {
    if text.is_empty() {
        return None;
    }
    Some(GoJson::parse(text.as_bytes()).unwrap_or_else(|_| GoJson::String(text.to_string())))
}

// ---------------------------------------------------------------------------
// IAM policy documents (minio/pkg policy.Policy)
// ---------------------------------------------------------------------------

/// `policy.Policy` as minio/pkg marshals it: `ID` (omitempty), `Version`, `Statement`. Sets
/// (actions, resources, condition values) are unordered in Go; mx keeps first-seen order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IamPolicy {
    pub id: String,
    pub version: String,
    /// `None` marshals as `null` (Go nil slice).
    pub statements: Option<Vec<Statement>>,
}

/// Condition functions: name -> key -> values (Go marshals the maps with sorted keys).
pub type Conditions = BTreeMap<String, BTreeMap<String, Vec<GoJson>>>;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Statement {
    pub sid: String,
    pub effect: String,
    pub actions: Vec<String>,
    pub not_actions: Vec<String>,
    pub resources: Vec<String>,
    pub not_resources: Vec<String>,
    pub conditions: Conditions,
}

impl Serialize for IamPolicy {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if !self.id.is_empty() {
            map.serialize_entry("ID", &self.id)?;
        }
        map.serialize_entry("Version", &self.version)?;
        map.serialize_entry("Statement", &self.statements)?;
        map.end()
    }
}

impl Serialize for Statement {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if !self.sid.is_empty() {
            map.serialize_entry("Sid", &self.sid)?;
        }
        map.serialize_entry("Effect", &self.effect)?;
        map.serialize_entry("Action", &self.actions)?;
        if !self.not_actions.is_empty() {
            map.serialize_entry("NotAction", &self.not_actions)?;
        }
        if !self.resources.is_empty() {
            map.serialize_entry("Resource", &self.resources)?;
        }
        if !self.not_resources.is_empty() {
            map.serialize_entry("NotResource", &self.not_resources)?;
        }
        if !self.conditions.is_empty() {
            map.serialize_entry("Condition", &self.conditions)?;
        }
        map.end()
    }
}

/// Go `json.Unmarshal` error text for a type mismatch.
fn type_error(value: &GoJson, field: &str, go_type: &str) -> McError {
    let kind = match value {
        GoJson::Null => "null",
        GoJson::Bool(_) => "bool",
        GoJson::Number(_) => "number",
        GoJson::String(_) => "string",
        GoJson::Array(_) => "array",
        GoJson::Object(_) => "object",
    };
    McError::new(format!(
        "json: cannot unmarshal {kind} into Go struct field {field} of type {go_type}"
    ))
}

/// `set.StringSet` decoding: a string or an array of strings; duplicates dropped.
fn string_set(value: &GoJson, field: &str) -> std::result::Result<Vec<String>, McError> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |text: &str| {
        if !out.iter().any(|seen| seen == text) {
            out.push(text.to_string());
        }
    };
    match value {
        GoJson::Null => {}
        GoJson::String(text) => push(text),
        GoJson::Array(items) => {
            for item in items {
                match item {
                    GoJson::String(text) => push(text),
                    other => return Err(type_error(other, field, "string")),
                }
            }
        }
        other => return Err(type_error(other, field, "[]string")),
    }
    Ok(out)
}

/// Condition functions as sorted `name key value` entries (for `Statement.Equals`).
fn canonical_conditions(conds: &Conditions) -> Vec<String> {
    let mut entries: Vec<String> = conds
        .iter()
        .flat_map(|(name, keys)| {
            keys.iter().flat_map(move |(key, values)| {
                values
                    .iter()
                    .map(move |value| format!("{name} {key} {}", value.compact()))
            })
        })
        .collect();
    entries.sort();
    entries
}

impl Statement {
    fn from_json(value: &GoJson, strict: bool) -> std::result::Result<Self, McError> {
        let GoJson::Object(fields) = value else {
            return Err(type_error(value, "Policy.Statement", "policy.Statement"));
        };
        let mut st = Statement::default();
        for (key, value) in fields {
            match key.to_ascii_lowercase().as_str() {
                "sid" => st.sid = value.as_str().unwrap_or_default().to_string(),
                "effect" => st.effect = value.as_str().unwrap_or_default().to_string(),
                "action" => {
                    st.actions = string_set(value, "Statement.Action")?;
                    if st.actions.is_empty() {
                        return Err(McError::new("empty actions not allowed"));
                    }
                }
                "notaction" => st.not_actions = string_set(value, "Statement.NotAction")?,
                "resource" => st.resources = string_set(value, "Statement.Resource")?,
                "notresource" => st.not_resources = string_set(value, "Statement.NotResource")?,
                "condition" => st.conditions = conditions(value)?,
                _ if strict => {
                    return Err(McError::new(format!("json: unknown field \"{key}\"")));
                }
                _ => {}
            }
        }
        Ok(st)
    }

    /// Order-insensitive comparison (`Statement.Equals`).
    fn same(&self, other: &Statement) -> bool {
        let sorted = |items: &[String]| {
            let mut items = items.to_vec();
            items.sort();
            items
        };
        self.sid == other.sid
            && self.effect == other.effect
            && sorted(&self.actions) == sorted(&other.actions)
            && sorted(&self.not_actions) == sorted(&other.not_actions)
            && sorted(&self.resources) == sorted(&other.resources)
            && sorted(&self.not_resources) == sorted(&other.not_resources)
            && canonical_conditions(&self.conditions) == canonical_conditions(&other.conditions)
    }
}

/// `condition.Functions` decoding: `{"Func": {"key": value | [values]}}`.
fn conditions(value: &GoJson) -> std::result::Result<Conditions, McError> {
    let GoJson::Object(funcs) = value else {
        return Err(type_error(
            value,
            "Statement.Condition",
            "condition.Functions",
        ));
    };
    if funcs.is_empty() {
        return Err(McError::new("condition must not be empty"));
    }
    let mut out = Conditions::new();
    for (name, keys) in funcs {
        let GoJson::Object(keys) = keys else {
            return Err(type_error(
                keys,
                "Statement.Condition",
                "map[string]condition.ValueSet",
            ));
        };
        let entry = out.entry(name.clone()).or_default();
        for (key, values) in keys {
            let items = match values {
                GoJson::Array(items) => items.clone(),
                other => vec![other.clone()],
            };
            let mut set: Vec<GoJson> = Vec::new();
            for item in items {
                if !set.contains(&item) {
                    set.push(item);
                }
            }
            entry.insert(key.clone(), set);
        }
    }
    Ok(out)
}

impl IamPolicy {
    /// `json.Unmarshal` into `policy.Policy` (duplicate statements dropped); `strict` rejects
    /// unknown fields like `policy.ParseConfig`.
    pub fn from_json(value: &GoJson, strict: bool) -> std::result::Result<Self, McError> {
        let GoJson::Object(fields) = value else {
            return Err(type_error(value, "", "policy.Policy"));
        };
        let mut policy = IamPolicy::default();
        for (key, value) in fields {
            match key.to_ascii_lowercase().as_str() {
                "id" => policy.id = value.as_str().unwrap_or_default().to_string(),
                "version" => policy.version = value.as_str().unwrap_or_default().to_string(),
                "statement" => match value {
                    GoJson::Null => policy.statements = None,
                    GoJson::Array(items) => {
                        let statements = items
                            .iter()
                            .map(|item| Statement::from_json(item, strict))
                            .collect::<std::result::Result<Vec<_>, _>>()?;
                        policy.statements = Some(statements);
                    }
                    other => {
                        return Err(type_error(other, "Policy.Statement", "[]policy.Statement"));
                    }
                },
                _ if strict => {
                    return Err(McError::new(format!("json: unknown field \"{key}\"")));
                }
                _ => {}
            }
        }
        policy.drop_duplicate_statements();
        Ok(policy)
    }

    /// `policy.ParseConfig`: strict decoding plus version/effect validation.
    pub fn parse_config(text: &[u8]) -> std::result::Result<Self, McError> {
        let value = GoJson::parse(text).map_err(|err| McError::new(json_error_text(text, &err)))?;
        let policy = Self::from_json(&value, true)?;
        if !policy.version.is_empty() && policy.version != "2012-10-17" {
            return Err(McError::new(format!(
                "invalid version '{}'",
                policy.version
            )));
        }
        for st in policy.statements.iter().flatten() {
            if st.effect != "Allow" && st.effect != "Deny" {
                return Err(McError::new(format!("invalid Effect {}", st.effect)));
            }
        }
        Ok(policy)
    }

    pub fn is_empty(&self) -> bool {
        self.statements.as_ref().is_none_or(Vec::is_empty)
    }

    fn drop_duplicate_statements(&mut self) {
        if let Some(statements) = self.statements.as_mut() {
            let mut kept: Vec<Statement> = Vec::new();
            for st in statements.drain(..) {
                if !kept.iter().any(|seen| seen.same(&st)) {
                    kept.push(st);
                }
            }
            *statements = kept;
        }
    }

    /// `policy.MergePolicies`.
    pub fn merge(inputs: &[IamPolicy]) -> IamPolicy {
        let mut merged = IamPolicy::default();
        for policy in inputs {
            if merged.version.is_empty() {
                merged.version = policy.version.clone();
            }
            for st in policy.statements.iter().flatten() {
                merged
                    .statements
                    .get_or_insert_with(Vec::new)
                    .push(st.clone());
            }
        }
        merged.drop_duplicate_statements();
        merged
    }
}

/// Go `encoding/json` text for why `data` is not valid JSON (`invalid character 'x'
/// looking for beginning of value`, `unexpected end of JSON input`, ...); falls back to
/// `err` when the Go scanner would accept the text.
pub fn json_error_text(data: &[u8], err: &serde_json::Error) -> String {
    go_syntax_error(data).unwrap_or_else(|| err.to_string())
}

/// Go `quoteChar`.
fn quote_char(c: u8) -> String {
    match c {
        b'\'' => r"'\''".to_string(),
        b'"' => "'\"'".to_string(),
        b'\n' => r"'\n'".to_string(),
        b'\r' => r"'\r'".to_string(),
        b'\t' => r"'\t'".to_string(),
        0x20..=0x7e => format!("'{}'", c as char),
        0x80.. => format!("'{}'", char::from(c)),
        _ => format!(r"'\x{c:02x}'"),
    }
}

/// The first syntax error Go's JSON scanner reports for `data`.
fn go_syntax_error(data: &[u8]) -> Option<String> {
    const EOF: &str = "unexpected end of JSON input";
    struct Scanner<'a> {
        data: &'a [u8],
        pos: usize,
    }
    type Step = std::result::Result<(), String>;
    let invalid = |c: u8, context: &str| format!("invalid character {} {context}", quote_char(c));
    impl Scanner<'_> {
        fn peek(&self) -> Option<u8> {
            self.data.get(self.pos).copied()
        }
        fn skip_space(&mut self) {
            while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                self.pos += 1;
            }
        }
        /// Consumes one byte matching `ok`, else the Go error for `context`.
        fn expect(&mut self, ok: impl Fn(u8) -> bool, context: &str) -> Step {
            match self.peek() {
                None => Err(EOF.to_string()),
                Some(c) if ok(c) => {
                    self.pos += 1;
                    Ok(())
                }
                Some(c) => Err(format!("invalid character {} {context}", quote_char(c))),
            }
        }
        fn value(&mut self) -> Step {
            self.skip_space();
            match self.peek() {
                None => Err(EOF.to_string()),
                Some(b'{') => self.object(),
                Some(b'[') => self.array(),
                Some(b'"') => self.string(),
                Some(b'-' | b'0'..=b'9') => self.number(),
                Some(b't') => self.literal(b"true"),
                Some(b'f') => self.literal(b"false"),
                Some(b'n') => self.literal(b"null"),
                Some(c) => Err(format!(
                    "invalid character {} looking for beginning of value",
                    quote_char(c)
                )),
            }
        }
        fn object(&mut self) -> Step {
            self.pos += 1;
            self.skip_space();
            if self.peek() == Some(b'}') {
                self.pos += 1;
                return Ok(());
            }
            loop {
                self.skip_space();
                if self.peek() != Some(b'"') {
                    self.expect(|_| false, "looking for beginning of object key string")?;
                }
                self.string()?;
                self.skip_space();
                self.expect(|c| c == b':', "after object key")?;
                self.value()?;
                self.skip_space();
                match self.peek() {
                    Some(b',') => self.pos += 1,
                    _ => return self.expect(|c| c == b'}', "after object key:value pair"),
                }
            }
        }
        fn array(&mut self) -> Step {
            self.pos += 1;
            self.skip_space();
            if self.peek() == Some(b']') {
                self.pos += 1;
                return Ok(());
            }
            loop {
                self.value()?;
                self.skip_space();
                match self.peek() {
                    Some(b',') => self.pos += 1,
                    _ => return self.expect(|c| c == b']', "after array element"),
                }
            }
        }
        fn string(&mut self) -> Step {
            self.pos += 1;
            loop {
                match self.peek() {
                    None => return Err(EOF.to_string()),
                    Some(b'"') => {
                        self.pos += 1;
                        return Ok(());
                    }
                    Some(b'\\') => {
                        self.pos += 1;
                        if self.peek() == Some(b'u') {
                            self.pos += 1;
                            for _ in 0..4 {
                                self.expect(
                                    |c| c.is_ascii_hexdigit(),
                                    r"in \u hexadecimal character escape",
                                )?;
                            }
                        } else {
                            self.expect(|c| b"\"\\/bfnrt".contains(&c), "in string escape code")?;
                        }
                    }
                    Some(c) if c < 0x20 => {
                        return self.expect(|_| false, "in string literal");
                    }
                    Some(_) => self.pos += 1,
                }
            }
        }
        fn digits(&mut self) {
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        fn number(&mut self) -> Step {
            if self.peek() == Some(b'-') {
                self.pos += 1;
                self.expect(|c| c.is_ascii_digit(), "in numeric literal")?;
                self.pos -= 1;
            }
            if self.peek() == Some(b'0') {
                self.pos += 1;
            } else {
                self.digits();
            }
            if self.peek() == Some(b'.') {
                self.pos += 1;
                self.expect(
                    |c| c.is_ascii_digit(),
                    "after decimal point in numeric literal",
                )?;
                self.digits();
            }
            if matches!(self.peek(), Some(b'e' | b'E')) {
                self.pos += 1;
                if matches!(self.peek(), Some(b'+' | b'-')) {
                    self.pos += 1;
                }
                self.expect(|c| c.is_ascii_digit(), "in exponent of numeric literal")?;
                self.digits();
            }
            Ok(())
        }
        fn literal(&mut self, word: &[u8]) -> Step {
            self.pos += 1;
            for &want in &word[1..] {
                let context = format!(
                    "in literal {} (expecting {})",
                    String::from_utf8_lossy(word),
                    quote_char(want)
                );
                self.expect(|c| c == want, &context)?;
            }
            Ok(())
        }
    }
    let mut scanner = Scanner { data, pos: 0 };
    if let Err(err) = scanner.value() {
        return Some(err);
    }
    scanner.skip_space();
    scanner.peek().map(|c| invalid(c, "after top-level value"))
}

// ---------------------------------------------------------------------------
// Times
// ---------------------------------------------------------------------------

/// A parsed server time (UTC seconds + nanoseconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GoTime {
    pub secs: i64,
    pub nanos: u32,
}

/// Go's zero `time.Time` (`0001-01-01T00:00:00Z`).
pub const ZERO_TIME: &str = "0001-01-01T00:00:00Z";
const ZERO_SECS: i64 = -62_135_596_800;

impl GoTime {
    /// Parses an RFC 3339 time (`Z` or offset, optional fraction).
    pub fn parse(text: &str) -> Option<GoTime> {
        use aws_smithy_types::date_time::{DateTime, Format};
        if text == ZERO_TIME {
            return Some(GoTime {
                secs: ZERO_SECS,
                nanos: 0,
            });
        }
        let parsed = DateTime::from_str(text, Format::DateTimeWithOffset).ok()?;
        Some(GoTime {
            secs: parsed.secs(),
            nanos: parsed.subsec_nanos(),
        })
    }

    pub fn now() -> GoTime {
        let now = aws_smithy_types::DateTime::from(std::time::SystemTime::now());
        GoTime {
            secs: now.secs(),
            nanos: now.subsec_nanos(),
        }
    }

    pub fn is_zero(&self) -> bool {
        self.secs == ZERO_SECS && self.nanos == 0
    }

    /// mc's `timeSentinel` (`time.Unix(0, 0)`), the server's "no expiry".
    pub fn is_sentinel(&self) -> bool {
        self.secs == 0 && self.nanos == 0
    }

    fn nanos_total(&self) -> i128 {
        self.secs as i128 * 1_000_000_000 + self.nanos as i128
    }

    /// `time.Now().Add(d)`.
    pub fn after(duration_nanos: i128) -> GoTime {
        let total = GoTime::now().nanos_total() + duration_nanos;
        GoTime {
            secs: total.div_euclid(1_000_000_000) as i64,
            nanos: total.rem_euclid(1_000_000_000) as u32,
        }
    }

    fn civil(&self) -> (i64, u32, u32, u32, u32, u32) {
        let days = self.secs.div_euclid(86_400);
        let rem = self.secs.rem_euclid(86_400);
        // Howard Hinnant's civil_from_days.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = yoe + era * 400 + i64::from(month <= 2);
        (
            year,
            month,
            day,
            (rem / 3600) as u32,
            (rem % 3600 / 60) as u32,
            (rem % 60) as u32,
        )
    }

    fn fraction(&self) -> String {
        if self.nanos == 0 {
            return String::new();
        }
        let digits = format!("{:09}", self.nanos);
        format!(".{}", digits.trim_end_matches('0'))
    }

    /// Go `time.Time.String()` for a UTC time: `2026-09-27 19:25:55 +0000 UTC`.
    pub fn go_string(&self) -> String {
        let (y, mo, d, h, mi, s) = self.civil();
        format!(
            "{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}{} +0000 UTC",
            self.fraction()
        )
    }

    /// RFC 3339 with nanoseconds as Go marshals a UTC time (`2026-09-27T19:25:55.5Z`).
    pub fn rfc3339_nano(&self) -> String {
        let (y, mo, d, h, mi, s) = self.civil();
        format!(
            "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}{}Z",
            self.fraction()
        )
    }

    /// `t.Format(time.RFC3339)` (seconds precision).
    pub fn rfc3339(&self) -> String {
        let (y, mo, d, h, mi, s) = self.civil();
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
    }

    /// go-humanize `humanize.Time(t)`: `23 hours from now`, `3 days ago`, `now`.
    pub fn humanize(&self) -> String {
        humanize_rel(self.nanos_total() - GoTime::now().nanos_total())
    }
}

/// go-humanize `RelTime(then, now, "ago", "from now")` for `then - now` in nanoseconds.
pub fn humanize_rel(then_minus_now: i128) -> String {
    const SEC: i128 = 1_000_000_000;
    const MIN: i128 = 60 * SEC;
    const HOUR: i128 = 60 * MIN;
    const DAY: i128 = 24 * HOUR;
    const WEEK: i128 = 7 * DAY;
    const MONTH: i128 = 30 * DAY;
    const YEAR: i128 = 12 * MONTH;
    const LONG: i128 = 37 * YEAR;
    let (label, diff) = if then_minus_now > 0 {
        ("from now", then_minus_now)
    } else {
        ("ago", -then_minus_now)
    };
    // (upper bound, format, divisor); `{}` is the count.
    let magnitudes: [(i128, &str, i128); 17] = [
        (SEC, "now", SEC),
        (2 * SEC, "1 second", 1),
        (MIN, "{} seconds", SEC),
        (2 * MIN, "1 minute", 1),
        (HOUR, "{} minutes", MIN),
        (2 * HOUR, "1 hour", 1),
        (DAY, "{} hours", HOUR),
        (2 * DAY, "1 day", 1),
        (WEEK, "{} days", DAY),
        (2 * WEEK, "1 week", 1),
        (MONTH, "{} weeks", WEEK),
        (2 * MONTH, "1 month", 1),
        (YEAR, "{} months", MONTH),
        (18 * MONTH, "1 year", 1),
        (2 * YEAR, "2 years", 1),
        (LONG, "{} years", YEAR),
        (i128::MAX, "a long while", 1),
    ];
    let (_, format, div) = magnitudes
        .iter()
        .find(|(bound, _, _)| *bound > diff)
        .copied()
        .unwrap_or(magnitudes[16]);
    if format == "now" {
        return "now".to_string();
    }
    format!(
        "{} {label}",
        format.replace("{}", &(diff / div).to_string())
    )
}

/// mc `supportedTimeFormats` for `--expiry` (`2006-01-02`, `2006-01-02T15:04`,
/// `2006-01-02T15:04:05`, RFC 3339). Times without a zone are read as UTC (mc: local time).
pub fn parse_expiry(text: &str) -> Option<GoTime> {
    let bytes = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let layout = |len: usize| -> Option<(i64, i64, i64, i64, i64, i64)> {
        if bytes.len() != len || bytes[4] != b'-' || bytes[7] != b'-' {
            return None;
        }
        let (y, mo, d) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
        let (mut h, mut mi, mut s) = (0, 0, 0);
        if len >= 16 {
            if bytes[10] != b'T' || bytes[13] != b':' {
                return None;
            }
            h = digits(11..13)?;
            mi = digits(14..16)?;
        }
        if len == 19 {
            if bytes[16] != b':' {
                return None;
            }
            s = digits(17..19)?;
        }
        Some((y, mo, d, h, mi, s))
    };
    let Some((y, mo, d, h, mi, s)) = layout(10).or_else(|| layout(16)).or_else(|| layout(19))
    else {
        return GoTime::parse(text);
    };
    let days_in_month = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => return None,
    };
    if d < 1 || d > days_in_month || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(GoTime {
        secs: days * 86_400 + h * 3600 + mi * 60 + s,
        nanos: 0,
    })
}

/// Go `time.ParseDuration` in nanoseconds (`1h30m`, `1.5h`, `300ms`, `-2m`); `None` when
/// invalid (mc's `ctx.Duration` then yields 0).
pub fn parse_go_duration(text: &str) -> Option<i128> {
    let (negative, mut rest) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if rest == "0" {
        return Some(0);
    }
    if rest.is_empty() {
        return None;
    }
    let mut total: f64 = 0.0;
    while !rest.is_empty() {
        let number_len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let number = &rest[..number_len];
        if number.is_empty() || number == "." || number.matches('.').count() > 1 {
            return None;
        }
        let value: f64 = number.parse().ok()?;
        rest = &rest[number_len..];
        let unit_len = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let unit = match &rest[..unit_len] {
            "ns" => 1.0,
            "us" | "µs" | "μs" => 1e3,
            "ms" => 1e6,
            "s" => 1e9,
            "m" => 60e9,
            "h" => 3600e9,
            _ => return None,
        };
        rest = &rest[unit_len..];
        total += value * unit;
    }
    if total > i64::MAX as f64 {
        return None;
    }
    let nanos = total as i128;
    Some(if negative { -nanos } else { nanos })
}

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

/// madmin `UserAuthInfo`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserAuthInfo {
    #[serde(rename = "type", default)]
    pub auth_type: String,
    #[serde(rename = "authServer", default)]
    pub auth_server: String,
    #[serde(rename = "authServerUserID", default)]
    pub auth_server_user_id: String,
}

/// madmin `UserInfo`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserInfo {
    #[serde(rename = "userAuthInfo", default)]
    pub auth_info: Option<UserAuthInfo>,
    #[serde(rename = "policyName", default, deserialize_with = "null_default")]
    pub policy_name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub status: String,
    #[serde(rename = "memberOf", default, deserialize_with = "null_default")]
    pub member_of: Vec<String>,
}

/// `AddUser` (`add-user`, encrypted `AddOrUpdateUserReq`).
pub async fn add_user(client: &AdminClient, access_key: &str, secret_key: &str) -> Result<()> {
    #[derive(Serialize)]
    struct Req<'a> {
        #[serde(rename = "secretKey", skip_serializing_if = "str::is_empty")]
        secret_key: &'a str,
        status: &'a str,
    }
    client
        .request("PUT", "add-user")
        .query("accessKey", access_key)
        .encrypted_json(&Req {
            secret_key,
            status: "enabled",
        })?
        .send()
        .await?;
    Ok(())
}

/// `SetUserStatus` (`enabled` / `disabled`).
pub async fn set_user_status(client: &AdminClient, access_key: &str, status: &str) -> Result<()> {
    client
        .request("PUT", "set-user-status")
        .query("accessKey", access_key)
        .query("status", status)
        .send()
        .await?;
    Ok(())
}

pub async fn remove_user(client: &AdminClient, access_key: &str) -> Result<()> {
    client
        .request("DELETE", "remove-user")
        .query("accessKey", access_key)
        .send()
        .await?;
    Ok(())
}

/// `ListUsers` (sorted by access key; Go iterates the map in random order).
pub async fn list_users(client: &AdminClient) -> Result<BTreeMap<String, UserInfo>> {
    let users: Option<BTreeMap<String, UserInfo>> = client
        .request("GET", "list-users")
        .decrypt()
        .send_json()
        .await?;
    Ok(users.unwrap_or_default())
}

pub async fn user_info(client: &AdminClient, access_key: &str) -> Result<UserInfo> {
    client
        .request("GET", "user-info")
        .query("accessKey", access_key)
        .send_json()
        .await
}

// ---------------------------------------------------------------------------
// Groups
// ---------------------------------------------------------------------------

/// madmin `GroupDesc`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GroupDesc {
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub status: String,
    #[serde(default, deserialize_with = "null_default")]
    pub members: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub policy: String,
}

/// `UpdateGroupMembers` (plain JSON `GroupAddRemove`).
pub async fn update_group_members(
    client: &AdminClient,
    group: &str,
    members: &[String],
    is_remove: bool,
) -> Result<()> {
    #[derive(Serialize)]
    struct Req<'a> {
        group: &'a str,
        members: &'a [String],
        #[serde(rename = "groupStatus")]
        group_status: &'a str,
        #[serde(rename = "isRemove")]
        is_remove: bool,
    }
    client
        .request("PUT", "update-group-members")
        .json(&Req {
            group,
            members,
            group_status: "",
            is_remove,
        })?
        .send()
        .await?;
    Ok(())
}

pub async fn group_description(client: &AdminClient, group: &str) -> Result<GroupDesc> {
    client
        .request("GET", "group")
        .query("group", group)
        .send_json()
        .await
}

pub async fn list_groups(client: &AdminClient) -> Result<Vec<String>> {
    let groups: Option<Vec<String>> = client.request("GET", "groups").send_json().await?;
    Ok(groups.unwrap_or_default())
}

pub async fn set_group_status(client: &AdminClient, group: &str, status: &str) -> Result<()> {
    client
        .request("PUT", "set-group-status")
        .query("group", group)
        .query("status", status)
        .send()
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Canned policies
// ---------------------------------------------------------------------------

/// madmin `PolicyInfo`. Dates are the server's strings (absent = zero time); `policy_raw`
/// is the policy exactly as the server sent it (`json.RawMessage`).
#[derive(Debug, Clone, Default)]
pub struct PolicyInfo {
    pub policy_name: String,
    pub policy: GoJson,
    pub policy_raw: Vec<u8>,
    pub create_date: Option<String>,
    pub update_date: Option<String>,
}

impl<'de> Deserialize<'de> for PolicyInfo {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(rename = "PolicyName", default, deserialize_with = "null_default")]
            policy_name: String,
            #[serde(rename = "Policy", default)]
            policy: Option<Box<serde_json::value::RawValue>>,
            #[serde(rename = "CreateDate", default)]
            create_date: Option<String>,
            #[serde(rename = "UpdateDate", default)]
            update_date: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let policy_raw = wire
            .policy
            .map(|raw| raw.get().as_bytes().to_vec())
            .unwrap_or_default();
        let policy = if policy_raw.is_empty() {
            GoJson::Null
        } else {
            GoJson::parse(&policy_raw).map_err(de::Error::custom)?
        };
        Ok(PolicyInfo {
            policy_name: wire.policy_name,
            policy,
            policy_raw,
            create_date: wire.create_date,
            update_date: wire.update_date,
        })
    }
}

impl Serialize for PolicyInfo {
    /// madmin `PolicyInfo.MarshalJSON`: dates only when one of them is set.
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let is_zero = |date: &Option<String>| {
            date.as_deref()
                .is_none_or(|d| GoTime::parse(d).is_none_or(|t| t.is_zero()))
        };
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("PolicyName", &self.policy_name)?;
        map.serialize_entry("Policy", &self.policy)?;
        if !(is_zero(&self.create_date) && is_zero(&self.update_date)) {
            let date = |d: &Option<String>| d.clone().unwrap_or_else(|| ZERO_TIME.to_string());
            map.serialize_entry("CreateDate", &date(&self.create_date))?;
            map.serialize_entry("UpdateDate", &date(&self.update_date))?;
        }
        map.end()
    }
}

/// `InfoCannedPolicyV2` (`info-canned-policy?name=&v=2`).
pub async fn info_canned_policy_v2(client: &AdminClient, name: &str) -> Result<PolicyInfo> {
    client
        .request("GET", "info-canned-policy")
        .query("name", name)
        .query("v", "2")
        .send_json()
        .await
}

/// `InfoCannedPolicy` (v1): the raw policy document.
pub async fn info_canned_policy(client: &AdminClient, name: &str) -> Result<Vec<u8>> {
    let response = client
        .request("GET", "info-canned-policy")
        .query("name", name)
        .send()
        .await?;
    Ok(response.body)
}

/// mc `getPolicyInfo`: v2 info, falling back to v1 for older servers.
pub async fn policy_info(client: &AdminClient, name: &str) -> Result<PolicyInfo> {
    let mut info = info_canned_policy_v2(client, name).await?;
    if info.policy_name.is_empty() {
        info.policy_raw = info_canned_policy(client, name).await?;
        info.policy = GoJson::parse(&info.policy_raw)?;
        info.policy_name = name.to_string();
    }
    Ok(info)
}

/// `ListCannedPolicies`: policy names (sorted; Go iterates the map in random order).
pub async fn list_canned_policies(client: &AdminClient) -> Result<Vec<String>> {
    let policies: Option<BTreeMap<String, serde::de::IgnoredAny>> = client
        .request("GET", "list-canned-policies")
        .send_json()
        .await?;
    Ok(policies.unwrap_or_default().into_keys().collect())
}

pub async fn add_canned_policy(client: &AdminClient, name: &str, policy: Vec<u8>) -> Result<()> {
    if policy.is_empty() {
        return Err(madmin_invalid_argument("policy input cannot be empty").into());
    }
    client
        .request("PUT", "add-canned-policy")
        .query("name", name)
        .body(policy)
        .send()
        .await?;
    Ok(())
}

pub async fn remove_canned_policy(client: &AdminClient, name: &str) -> Result<()> {
    client
        .request("DELETE", "remove-canned-policy")
        .query("name", name)
        .send()
        .await?;
    Ok(())
}

/// madmin `PolicyAssociationReq`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PolicyAssociationReq {
    pub policies: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub user: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub group: String,
}

impl PolicyAssociationReq {
    /// `PolicyAssociationReq.IsValid`.
    pub fn validate(&self) -> std::result::Result<(), McError> {
        let error = |text: &str| Err(McError::new(text));
        if self.policies.is_empty() {
            return error("no policy names were given");
        }
        if self.policies.iter().any(String::is_empty) {
            return error("an empty policy name was given");
        }
        if self.user.is_empty() && self.group.is_empty() {
            return error("no user or group association was given");
        }
        if !self.user.is_empty() && !self.group.is_empty() {
            return error("either a group or a user association must be given, not both");
        }
        Ok(())
    }
}

/// madmin `PolicyAssociationResp` (`updatedAt` absent for older servers).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PolicyAssociationResp {
    #[serde(
        rename = "policiesAttached",
        default,
        deserialize_with = "null_default"
    )]
    pub policies_attached: Vec<String>,
    #[serde(
        rename = "policiesDetached",
        default,
        deserialize_with = "null_default"
    )]
    pub policies_detached: Vec<String>,
    #[serde(rename = "updatedAt", default)]
    pub updated_at: Option<String>,
}

impl PolicyAssociationResp {
    /// True when the server sent no result (zero `UpdatedAt`).
    pub fn is_empty(&self) -> bool {
        self.updated_at
            .as_deref()
            .is_none_or(|t| GoTime::parse(t).is_none_or(|t| t.is_zero()))
    }
}

/// `AttachPolicy` / `DetachPolicy` (`idp/builtin/policy/attach|detach`).
pub async fn attach_detach_policy(
    client: &AdminClient,
    attach: bool,
    req: &PolicyAssociationReq,
) -> Result<PolicyAssociationResp> {
    req.validate()?;
    let suffix = if attach { "attach" } else { "detach" };
    let response = client
        .request("POST", &format!("idp/builtin/policy/{suffix}"))
        .header("content-type", "application/octet-stream")
        .encrypted_json(req)?
        .send()
        .await?;
    if response.status != 200 {
        // Older servers answer 201/204 without a result.
        return Ok(PolicyAssociationResp::default());
    }
    let data = client.decrypt(&response.body)?;
    Ok(serde_json::from_slice(&data)?)
}

/// madmin `PolicyEntitiesResult` (serialized in madmin's field order).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyEntitiesResult {
    #[serde(default)]
    pub timestamp: String,
    #[serde(
        rename = "userMappings",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub user_mappings: Vec<UserPolicyEntities>,
    #[serde(
        rename = "groupMappings",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub group_mappings: Vec<GroupPolicyEntities>,
    #[serde(
        rename = "policyMappings",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub policy_mappings: Vec<PolicyEntities>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserPolicyEntities {
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub policies: Option<Vec<String>>,
    #[serde(
        rename = "memberOfMappings",
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub member_of_mappings: Vec<GroupPolicyEntities>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupPolicyEntities {
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub policies: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyEntities {
    #[serde(default)]
    pub policy: String,
    #[serde(default)]
    pub users: Option<Vec<String>>,
    #[serde(default)]
    pub groups: Option<Vec<String>>,
}

/// `GetPolicyEntities` (`idp/builtin/policy-entities`).
pub async fn policy_entities(
    client: &AdminClient,
    users: &[String],
    groups: &[String],
    policies: &[String],
) -> Result<PolicyEntitiesResult> {
    let mut request = client.request("GET", "idp/builtin/policy-entities");
    for (key, values) in [("group", groups), ("policy", policies), ("user", users)] {
        for value in values {
            request = request.query(key, value.as_str());
        }
    }
    request.decrypt().send_json().await
}

// ---------------------------------------------------------------------------
// Service accounts / access keys
// ---------------------------------------------------------------------------

/// madmin `Credentials` (`expiration` absent = zero time).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Credentials {
    #[serde(rename = "accessKey", default)]
    pub access_key: String,
    #[serde(rename = "secretKey", default)]
    pub secret_key: String,
    #[serde(default)]
    pub expiration: Option<String>,
}

/// madmin `AddServiceAccountReq`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AddServiceAccountReq {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<GoJson>,
    #[serde(rename = "targetUser", skip_serializing_if = "String::is_empty")]
    pub target_user: String,
    #[serde(rename = "accessKey", skip_serializing_if = "String::is_empty")]
    pub access_key: String,
    #[serde(rename = "secretKey", skip_serializing_if = "String::is_empty")]
    pub secret_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration: Option<String>,
}

/// madmin `UpdateServiceAccountReq`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateServiceAccountReq {
    #[serde(rename = "newPolicy", skip_serializing_if = "Option::is_none")]
    pub new_policy: Option<GoJson>,
    #[serde(rename = "newSecretKey", skip_serializing_if = "String::is_empty")]
    pub new_secret_key: String,
    #[serde(rename = "newStatus", skip_serializing_if = "String::is_empty")]
    pub new_status: String,
    #[serde(rename = "newName", skip_serializing_if = "String::is_empty")]
    pub new_name: String,
    #[serde(rename = "newDescription", skip_serializing_if = "String::is_empty")]
    pub new_description: String,
    #[serde(rename = "newExpiration", skip_serializing_if = "Option::is_none")]
    pub new_expiration: Option<String>,
}

/// madmin `validateSAName` / `validateSAExpiration` / `validateSADescription`.
fn validate_service_account(
    name: &str,
    description: &str,
    expiration: Option<&str>,
) -> std::result::Result<(), McError> {
    if !name.is_empty() {
        if name.len() > 32 {
            return Err(McError::new("name must not be longer than 32 characters"));
        }
        if !name.as_bytes()[0].is_ascii_alphabetic() {
            return Err(McError::new(
                "name must contain only ASCII letters, digits, underscores and hyphens and must start with a letter",
            ));
        }
    }
    if let Some(time) = expiration.and_then(GoTime::parse)
        && !time.is_zero()
        && !time.is_sentinel()
        && time < GoTime::now()
    {
        return Err(McError::new("the expiration time should be in the future"));
    }
    if description.len() > 256 {
        return Err(McError::new("description must be at most 256 bytes long"));
    }
    Ok(())
}

/// `AddServiceAccount` (`add-service-account`, encrypted both ways).
pub async fn add_service_account(
    client: &AdminClient,
    req: &AddServiceAccountReq,
) -> Result<Credentials> {
    validate_service_account(&req.name, &req.description, req.expiration.as_deref())?;
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        credentials: Credentials,
    }
    let resp: Resp = client
        .request("PUT", "add-service-account")
        .encrypted_json(req)?
        .decrypt()
        .send_json()
        .await?;
    Ok(resp.credentials)
}

/// `UpdateServiceAccount` (`update-service-account?accessKey=`).
pub async fn update_service_account(
    client: &AdminClient,
    access_key: &str,
    req: &UpdateServiceAccountReq,
) -> Result<()> {
    validate_service_account(
        &req.new_name,
        &req.new_description,
        req.new_expiration.as_deref(),
    )?;
    client
        .request("POST", "update-service-account")
        .query("accessKey", access_key)
        .encrypted_json(req)?
        .send()
        .await?;
    Ok(())
}

/// madmin `ServiceAccountInfo` (re-marshaled by `accesskey list --json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceAccountInfo {
    #[serde(rename = "parentUser", default)]
    pub parent_user: String,
    #[serde(rename = "accountStatus", default)]
    pub account_status: String,
    #[serde(rename = "impliedPolicy", default)]
    pub implied_policy: bool,
    #[serde(rename = "accessKey", default)]
    pub access_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiration: Option<String>,
}

pub async fn list_service_accounts(
    client: &AdminClient,
    user: &str,
) -> Result<Vec<ServiceAccountInfo>> {
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default, deserialize_with = "null_default")]
        accounts: Vec<ServiceAccountInfo>,
    }
    let resp: Resp = client
        .request("GET", "list-service-accounts")
        .query("user", user)
        .decrypt()
        .send_json()
        .await?;
    Ok(resp.accounts)
}

/// madmin `InfoServiceAccountResp` / `TemporaryAccountInfoResp`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct InfoServiceAccountResp {
    #[serde(rename = "parentUser", default)]
    pub parent_user: String,
    #[serde(rename = "accountStatus", default)]
    pub account_status: String,
    #[serde(rename = "impliedPolicy", default)]
    pub implied_policy: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub policy: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub expiration: Option<String>,
}

pub async fn info_service_account(
    client: &AdminClient,
    access_key: &str,
) -> Result<InfoServiceAccountResp> {
    client
        .request("GET", "info-service-account")
        .query("accessKey", access_key)
        .decrypt()
        .send_json()
        .await
}

pub async fn temporary_account_info(
    client: &AdminClient,
    access_key: &str,
) -> Result<InfoServiceAccountResp> {
    client
        .request("GET", "temporary-account-info")
        .query("accessKey", access_key)
        .decrypt()
        .send_json()
        .await
}

pub async fn delete_service_account(client: &AdminClient, access_key: &str) -> Result<()> {
    client
        .request("DELETE", "delete-service-account")
        .query("accessKey", access_key)
        .send()
        .await?;
    Ok(())
}

/// madmin `ListAccessKeysResp`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListAccessKeysResp {
    #[serde(rename = "serviceAccounts", default)]
    pub service_accounts: Option<Vec<ServiceAccountInfo>>,
    #[serde(rename = "stsKeys", default)]
    pub sts_keys: Option<Vec<ServiceAccountInfo>>,
}

/// madmin `ListAccessKeysOpts` for the builtin provider.
#[derive(Debug, Clone, Default)]
pub struct ListAccessKeysOpts {
    /// `users-only`, `sts-only`, `svcacc-only` or `all`.
    pub list_type: String,
    pub all: bool,
    pub config_name: String,
}

/// `ListAccessKeysBulk` (sorted by user; Go iterates the map in random order).
pub async fn list_access_keys_bulk(
    client: &AdminClient,
    users: &[String],
    opts: &ListAccessKeysOpts,
) -> Result<BTreeMap<String, ListAccessKeysResp>> {
    let list_type = if opts.list_type.is_empty() {
        "all"
    } else {
        opts.list_type.as_str()
    };
    if !["users-only", "sts-only", "svcacc-only", "all"].contains(&list_type) {
        return Err(McError::new("invalid list type").into());
    }
    if opts.all && !users.is_empty() {
        return Err(McError::new("either specify users or all, not both").into());
    }
    if !opts.config_name.is_empty() {
        return Err(McError::new(
            "configName and allConfigs are not supported for builtin provider",
        )
        .into());
    }
    let mut request = client
        .request("GET", "list-access-keys-bulk")
        .query("listType", list_type);
    for user in users {
        request = request.query("users", user.as_str());
    }
    if opts.all {
        request = request.query("all", "true");
    }
    let keys: Option<BTreeMap<String, ListAccessKeysResp>> = request.decrypt().send_json().await?;
    Ok(keys.unwrap_or_default())
}

/// madmin `InfoAccessKeyResp`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct InfoAccessKeyResp {
    #[serde(flatten)]
    pub info: InfoServiceAccountResp,
    #[serde(rename = "userType", default)]
    pub user_type: String,
    #[serde(rename = "userProvider", default)]
    pub user_provider: String,
    #[serde(rename = "ldapSpecificInfo", default)]
    pub ldap_specific_info: LdapSpecificAccessKeyInfo,
    #[serde(rename = "openIDSpecificInfo", default)]
    pub openid_specific_info: OpenIdSpecificAccessKeyInfo,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LdapSpecificAccessKeyInfo {
    #[serde(default)]
    pub username: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OpenIdSpecificAccessKeyInfo {
    #[serde(rename = "configName", default)]
    pub config_name: String,
    #[serde(rename = "userID", default)]
    pub user_id: String,
    #[serde(rename = "userIDClaim", default)]
    pub user_id_claim: String,
    #[serde(rename = "displayName", default)]
    pub display_name: String,
    #[serde(rename = "displayNameClaim", default)]
    pub display_name_claim: String,
}

pub async fn info_access_key(client: &AdminClient, access_key: &str) -> Result<InfoAccessKeyResp> {
    client
        .request("GET", "info-access-key")
        .query("accessKey", access_key)
        .decrypt()
        .send_json()
        .await
}

/// `RevokeTokens` (`revoke-tokens/builtin`) after `RevokeTokensReq.Validate`.
pub async fn revoke_tokens(
    client: &AdminClient,
    user: &str,
    token_revoke_type: &str,
    full_revoke: bool,
) -> Result<()> {
    if !user.is_empty() && token_revoke_type.is_empty() && !full_revoke {
        return Err(McError::new(
            "one of TokenRevokeType or FullRevoke must be set when User is set",
        )
        .into());
    }
    if !token_revoke_type.is_empty() && full_revoke {
        return Err(McError::new(
            "only one of TokenRevokeType or FullRevoke must be set, not both",
        )
        .into());
    }
    let mut request = client
        .request("POST", "revoke-tokens/builtin")
        .query("tokenRevokeType", token_revoke_type)
        .query("user", user);
    if full_revoke {
        request = request.query("fullRevoke", "true");
    }
    request.send().await?;
    Ok(())
}

/// mc `generateCredentials`: 20 alphanumeric characters and a 40 character base64 secret.
pub fn generate_credentials() -> Result<(String, String)> {
    use base64::Engine;
    const TABLE: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut key = [0u8; 20];
    aws_lc_rs::rand::fill(&mut key).map_err(|_| anyhow!("random generator failed"))?;
    let access_key: String = key
        .iter()
        .map(|b| TABLE[(*b % TABLE.len() as u8) as usize] as char)
        .collect();
    let mut secret = [0u8; 40];
    aws_lc_rs::rand::fill(&mut secret).map_err(|_| anyhow!("random generator failed"))?;
    let secret_key =
        base64::engine::general_purpose::STANDARD.encode(secret)[..40].replace('/', "+");
    Ok((access_key, secret_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_json_keeps_key_order() {
        let value = GoJson::parse(br#"{"b":1,"a":[true,null,"x<"],"c":{"z":1.5,"y":{}}}"#).unwrap();
        assert_eq!(
            value.compact(),
            r#"{"b":1,"a":[true,null,"x\u003c"],"c":{"z":1.5,"y":{}}}"#
        );
    }

    #[test]
    fn policy_parses_and_marshals_like_minio_pkg() {
        let text = br#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"s3:GetObject","Resource":["arn:aws:s3:::b/*","arn:aws:s3:::b/*"],
             "Condition":{"StringEquals":{"s3:prefix":"x"},"Bool":{"aws:SecureTransport":true}}},
            {"Effect":"Allow","Action":["s3:GetObject"],"Resource":"arn:aws:s3:::b/*",
             "Condition":{"Bool":{"aws:SecureTransport":[true]},"StringEquals":{"s3:prefix":["x"]}}}]}"#;
        let policy = IamPolicy::parse_config(text).unwrap();
        assert_eq!(
            serde_json::to_string(&policy).unwrap(),
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject"],"Resource":["arn:aws:s3:::b/*"],"Condition":{"Bool":{"aws:SecureTransport":[true]},"StringEquals":{"s3:prefix":["x"]}}}]}"#
        );
        assert!(!policy.is_empty());
    }

    #[test]
    fn policy_parse_errors() {
        let err = IamPolicy::parse_config(br#"{"Version":"2012-10-17","Foo":1}"#).unwrap_err();
        assert_eq!(err.message, r#"json: unknown field "Foo""#);
        let err = IamPolicy::parse_config(br#"{"Version":"1"}"#).unwrap_err();
        assert_eq!(err.message, "invalid version '1'");
        let err = IamPolicy::parse_config(b"").unwrap_err();
        assert_eq!(err.message, "unexpected end of JSON input");
        assert!(
            IamPolicy::parse_config(br#"{"Version":"2012-10-17","Statement":[]}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn syntax_errors_read_like_go() {
        let text = |data: &[u8]| go_syntax_error(data);
        assert_eq!(text(br#"{"a":1}"#), None);
        assert_eq!(text(b" [1, -2.5e+3, true, null, \"x\\u00e9\"] "), None);
        let cases: [(&[u8], &str); 12] = [
            (
                b"xx",
                "invalid character 'x' looking for beginning of value",
            ),
            (b"", "unexpected end of JSON input"),
            (b"{\"a\" 1}", "invalid character '1' after object key"),
            (
                b"{\"a\":1 \"b\"}",
                "invalid character '\"' after object key:value pair",
            ),
            (
                b"{1:2}",
                "invalid character '1' looking for beginning of object key string",
            ),
            (
                b"{\"a\":1,}",
                "invalid character '}' looking for beginning of object key string",
            ),
            (b"[1 2]", "invalid character '2' after array element"),
            (
                b"[1,]",
                "invalid character ']' looking for beginning of value",
            ),
            (b"tru", "unexpected end of JSON input"),
            (
                b"trUe",
                "invalid character 'U' in literal true (expecting 'u')",
            ),
            (b"{} x", "invalid character 'x' after top-level value"),
            (b"-a", "invalid character 'a' in numeric literal"),
        ];
        for (data, want) in cases {
            assert_eq!(
                text(data).as_deref(),
                Some(want),
                "{}",
                String::from_utf8_lossy(data)
            );
        }
        assert_eq!(
            text(b"1.x").as_deref(),
            Some("invalid character 'x' after decimal point in numeric literal")
        );
        assert_eq!(
            text(b"{\"a\":\"\\q\"}").as_deref(),
            Some("invalid character 'q' in string escape code")
        );
        assert_eq!(
            text(b"'a'").as_deref(),
            Some(r"invalid character '\'' looking for beginning of value")
        );
    }

    #[test]
    fn merge_policies_drops_duplicates() {
        let parse = |text: &[u8]| IamPolicy::from_json(&GoJson::parse(text).unwrap(), false);
        let a = parse(br#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["a","b"],"Resource":["r"]}]}"#).unwrap();
        let b = parse(br#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["b","a"],"Resource":["r"]},{"Effect":"Deny","Action":["c"]}]}"#).unwrap();
        let merged = IamPolicy::merge(&[a, b]);
        assert_eq!(
            serde_json::to_string(&merged).unwrap(),
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["a","b"],"Resource":["r"]},{"Effect":"Deny","Action":["c"]}]}"#
        );
        assert_eq!(
            serde_json::to_string(&IamPolicy::merge(&[])).unwrap(),
            r#"{"Version":"","Statement":null}"#
        );
    }

    #[test]
    fn go_time_formats() {
        let t = GoTime::parse("2026-09-27T19:25:55Z").unwrap();
        assert_eq!(t.go_string(), "2026-09-27 19:25:55 +0000 UTC");
        assert_eq!(t.rfc3339(), "2026-09-27T19:25:55Z");
        let t = GoTime::parse("2026-09-26T19:25:29.827660614Z").unwrap();
        assert_eq!(t.go_string(), "2026-09-26 19:25:29.827660614 +0000 UTC");
        assert_eq!(t.rfc3339(), "2026-09-26T19:25:29Z");
        assert_eq!(t.rfc3339_nano(), "2026-09-26T19:25:29.827660614Z");
        assert!(GoTime::parse("1970-01-01T00:00:00Z").unwrap().is_sentinel());
        assert!(GoTime::parse(ZERO_TIME).unwrap().is_zero());
        assert_eq!(
            GoTime::parse(ZERO_TIME).unwrap().go_string(),
            "0001-01-01 00:00:00 +0000 UTC"
        );
    }

    #[test]
    fn humanize_matches_go_humanize() {
        const S: i128 = 1_000_000_000;
        assert_eq!(humanize_rel(0), "now");
        assert_eq!(humanize_rel(S / 2), "now");
        assert_eq!(humanize_rel(S), "1 second from now");
        assert_eq!(humanize_rel(-30 * S), "30 seconds ago");
        assert_eq!(humanize_rel(90 * S), "1 minute from now");
        assert_eq!(humanize_rel(24 * 3600 * S - S), "23 hours from now");
        assert_eq!(humanize_rel(3 * 86_400 * S), "3 days from now");
        assert_eq!(humanize_rel(10 * 86_400 * S), "1 week from now");
        assert_eq!(humanize_rel(400 * 86_400 * S), "1 year from now");
        assert_eq!(humanize_rel(50 * 360 * 86_400 * S), "a long while from now");
    }

    #[test]
    fn parses_expiry_formats() {
        let date = |text| parse_expiry(text).map(|t| t.rfc3339());
        assert_eq!(date("2030-01-02").as_deref(), Some("2030-01-02T00:00:00Z"));
        assert_eq!(
            date("2030-01-02T03:04").as_deref(),
            Some("2030-01-02T03:04:00Z")
        );
        assert_eq!(
            date("2030-01-02T03:04:05").as_deref(),
            Some("2030-01-02T03:04:05Z")
        );
        assert_eq!(
            date("2030-01-02T03:04:05+01:00").as_deref(),
            Some("2030-01-02T02:04:05Z")
        );
        assert_eq!(date("2030-02-30"), None);
        assert_eq!(date("tomorrow"), None);
        assert_eq!(date("2030-1-2"), None);
    }

    #[test]
    fn parses_go_durations() {
        const S: i128 = 1_000_000_000;
        assert_eq!(parse_go_duration("24h"), Some(24 * 3600 * S));
        assert_eq!(parse_go_duration("1h30m"), Some(5400 * S));
        assert_eq!(parse_go_duration("1.5h"), Some(5400 * S));
        assert_eq!(parse_go_duration("300ms"), Some(300_000_000));
        assert_eq!(parse_go_duration("0"), Some(0));
        assert_eq!(parse_go_duration("-2m"), Some(-120 * S));
        assert_eq!(parse_go_duration("1d"), None);
        assert_eq!(parse_go_duration("10"), None);
        assert_eq!(parse_go_duration(""), None);
    }

    #[test]
    fn generated_credentials_have_mc_shape() {
        let (access, secret) = generate_credentials().unwrap();
        assert_eq!(access.len(), 20);
        assert!(
            access
                .bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        );
        assert_eq!(secret.len(), 40);
        assert!(!secret.contains('/'));
    }

    #[test]
    fn association_request_validation() {
        let mut req = PolicyAssociationReq {
            policies: vec!["p".into()],
            ..Default::default()
        };
        assert_eq!(
            req.validate().unwrap_err().message,
            "no user or group association was given"
        );
        req.user = "u".into();
        assert!(req.validate().is_ok());
        req.group = "g".into();
        assert!(req.validate().is_err());
    }

    #[test]
    fn policy_info_marshals_dates_only_when_set() {
        let info: PolicyInfo =
            serde_json::from_str(r#"{"PolicyName":"p","Policy":{"b":1,"a":2}}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"PolicyName":"p","Policy":{"b":1,"a":2}}"#
        );
        assert_eq!(
            serde_json::to_string(&PolicyInfo::default()).unwrap(),
            r#"{"PolicyName":"","Policy":null}"#
        );
        let info: PolicyInfo = serde_json::from_str(
            r#"{"PolicyName":"p","Policy":{},"CreateDate":"2026-09-26T19:25:29.714Z","UpdateDate":"2026-09-26T19:25:29.714Z"}"#,
        )
        .unwrap();
        assert!(serde_json::to_string(&info).unwrap().contains("CreateDate"));
    }
}
