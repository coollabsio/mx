//! S3 Select (`SelectObjectContent`) for `mx sql`: mc's serialization option parsing
//! (`parseKVArgs`, `parseSerializationOpts`), the input/output serialization mc derives from
//! the options and the object name (`selectObjectInputOpts` / `selectObjectOutputOpts`), and
//! the record stream.

use crate::error::McError;
use anyhow::Result;
use aws_sdk_s3::Client;
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::primitives::event_stream::EventReceiver;
use aws_sdk_s3::types::error::SelectObjectContentEventStreamError;
use aws_sdk_s3::types::{
    CompressionType, CsvInput, CsvOutput, ExpressionType, FileHeaderInfo, InputSerialization,
    JsonInput, JsonOutput, JsonType, OutputSerialization, ParquetInput, QuoteFields,
    SelectObjectContentEventStream,
};
use aws_smithy_types::event_stream::{Message, RawMessage};
use std::collections::HashMap;

/// Lower-cased option name -> value.
pub type OptMap = HashMap<String, String>;

pub const CSV_INPUT_KEYS: &[&str] = &[
    "FieldDelimiter",
    "QuoteChar",
    "QuoteEscChar",
    "Comments",
    "FileHeader",
    "QuotedRecordDelimiter",
    "RecordDelimiter",
];
pub const CSV_OUTPUT_KEYS: &[&str] = &[
    "FieldDelimiter",
    "QuoteChar",
    "QuoteEscChar",
    "RecordDelimiter",
    "QuoteFields",
];
pub const JSON_INPUT_KEYS: &[&str] = &["Type"];
pub const JSON_OUTPUT_KEYS: &[&str] = &["RecordDelimiter"];

/// mc's abbreviations (a Go map, so mc lists them in random order; this order is fixed).
pub const CSV_INPUT_ABBR: &[(&str, &str)] = &[
    ("cc", "Comments"),
    ("fh", "FileHeader"),
    ("qrd", "QuotedRecordDelimiter"),
    ("rd", "RecordDelimiter"),
    ("fd", "FieldDelimiter"),
    ("qc", "QuoteChar"),
    ("qec", "QuoteEscChar"),
];
pub const CSV_OUTPUT_ABBR: &[(&str, &str)] = &[
    ("qf", "QuoteFields"),
    ("rd", "RecordDelimiter"),
    ("fd", "FieldDelimiter"),
    ("qc", "QuoteChar"),
    ("qec", "QuoteEscChar"),
];
pub const JSON_OUTPUT_ABBR: &[(&str, &str)] = &[("rd", "RecordDelimiter")];

/// mc `parseKVArgs`: `k=v,k=v` where values may contain commas that are not followed by a new
/// key; keys are lower-cased; `\n`, `\t`, `\r` escapes are expanded.
pub fn parse_kv_args(input: &str) -> Result<OptMap> {
    let is = input.as_bytes();
    let mut map = OptMap::new();
    let mut index = 0;
    while index < is.len() {
        let Some(i) = input[index..].find('=') else {
            return Err(McError::new("Arguments should be of the form key=value,... ").into());
        };
        let key = &input[index..index + i];
        let s = index + i + 1;
        let mut e: Option<usize> = input[s..].find(',');
        while let Some(offset) = e {
            if offset + s >= is.len() {
                break;
            }
            if is[s + offset] != b',' {
                if is[s + offset - 1] == b',' {
                    e = Some(offset - 1);
                }
                break;
            }
            e = Some(offset + 1);
        }
        let end = e.map_or(is.len(), |offset| (s + offset).min(is.len()));
        let value = &input[s..end];
        index = end + 1;
        let lower = key.to_lowercase();
        if map.contains_key(&lower) {
            return Err(
                McError::new(format!("More than one key=value found for {}", key.trim())).into(),
            );
        }
        map.insert(
            lower,
            value
                .replace("\\n", "\n")
                .replace("\\t", "\t")
                .replace("\\r", "\r"),
        );
    }
    Ok(map)
}

/// mc `fmtString`: `Long(abbr) ,Long(abbr) ` or, without abbreviations, `Key Key `.
fn fmt_keys(abbr: &[(&str, &str)], keys: &[&str]) -> String {
    if abbr.is_empty() {
        return keys.iter().map(|key| format!("{key} ")).collect();
    }
    abbr.iter()
        .map(|(short, long)| format!("{long}({short}) "))
        .collect::<Vec<_>>()
        .join(",")
}

/// mc `parseSerializationOpts`: parses `input`, expands abbreviations and checks the keys.
pub fn parse_serialization_opts(
    input: &str,
    keys: &[&str],
    abbr: &[(&str, &str)],
) -> Result<OptMap> {
    let mut out = OptMap::new();
    for (key, value) in parse_kv_args(input)? {
        let name = abbr
            .iter()
            .find(|(short, _)| *short == key)
            .map_or(key, |(_, long)| long.to_lowercase());
        out.insert(name, value);
    }
    if out
        .keys()
        .any(|key| !keys.iter().any(|valid| valid.eq_ignore_ascii_case(key)))
    {
        return Err(McError::new(format!(
            "Options should be key-value pairs in the form key=value,... where valid key(s) are {}",
            fmt_keys(abbr, keys)
        ))
        .into());
    }
    Ok(out)
}

/// mc `SelectObjectOpts`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SelectOpts {
    pub csv_input: Option<OptMap>,
    pub json_input: Option<OptMap>,
    pub csv_output: Option<OptMap>,
    pub json_output: Option<OptMap>,
    /// `--compression` as given (empty: derived from the object name).
    pub compression: String,
}

impl SelectOpts {
    /// Explicit `--csv-input` / `--json-input` options.
    pub fn has_input(&self) -> bool {
        self.csv_input.is_some() || self.json_input.is_some()
    }
}

/// Go `filepath.Ext`.
fn ext(name: &str) -> &str {
    let base = &name[name.rfind('/').map_or(0, |i| i + 1)..];
    base.rfind('.').map_or("", |i| &base[i..])
}

/// mc `mimedb.TypeByExtension` (empty when unknown).
pub fn type_by_extension(name: &str) -> String {
    let ext = ext(name);
    if ext.is_empty() {
        return String::new();
    }
    mime_guess::from_ext(&ext[1..])
        .first()
        .map(|mime| mime.essence_str().to_string())
        .unwrap_or_default()
}

/// mc `selectCompressionType`.
fn compression_type(opts: &SelectOpts, object: &str) -> String {
    if !opts.compression.is_empty() {
        return opts.compression.clone();
    }
    if ext(object).contains("parquet") || object.contains(".parquet") {
        return "NONE".into();
    }
    let content_type = type_by_extension(object);
    if content_type.contains("gzip") {
        "GZIP".into()
    } else if content_type.contains("bzip") {
        "BZIP2".into()
    } else {
        "NONE".into()
    }
}

fn trim_compression_exts(name: &str) -> &str {
    let name = name.strip_suffix(".gz").unwrap_or(name);
    let name = name.strip_suffix(".bz").unwrap_or(name);
    name.strip_suffix(".bz2").unwrap_or(name)
}

/// mc `selectObjectInputOpts`.
pub fn input_serialization(opts: &SelectOpts, object: &str) -> InputSerialization {
    let mut input = InputSerialization::builder();
    let (mut csv, mut json, mut parquet) = (None, None, false);
    if let Some(map) = &opts.json_input {
        let mut j = JsonInput::builder();
        if let Some(kind) = map.get("type").filter(|t| !t.is_empty()) {
            j = j.r#type(JsonType::from(kind.as_str()));
        }
        json = Some(j.build());
    }
    if let Some(map) = &opts.csv_input {
        let get = |key: &str| map.get(key).cloned();
        let mut c = CsvInput::builder()
            .record_delimiter(get("recorddelimiter").unwrap_or_else(|| "\n".into()));
        if let Some(value) = get("fielddelimiter") {
            c = c.field_delimiter(value);
        }
        if let Some(value) = get("quotechar") {
            c = c.quote_character(value);
        }
        if let Some(value) = get("quoteescchar") {
            c = c.quote_escape_character(value);
        }
        if let Some(value) = get("fileheader") {
            c = c.file_header_info(FileHeaderInfo::from(value.as_str()));
        }
        // mc reads `commentchar`, which `Comments(cc)` never sets.
        if let Some(value) = get("commentchar") {
            c = c.comments(value);
        }
        csv = Some(c.build());
    }
    if csv.is_none() && json.is_none() {
        let ext = ext(trim_compression_exts(object));
        if ext.contains("csv") {
            csv = Some(
                CsvInput::builder()
                    .record_delimiter("\n")
                    .field_delimiter(",")
                    .file_header_info(FileHeaderInfo::Use)
                    .build(),
            );
        }
        if ext.contains("parquet") || object.contains(".parquet") {
            parquet = true;
        }
        if ext.contains("json") {
            json = Some(JsonInput::builder().r#type(JsonType::Lines).build());
        }
    }
    if parquet {
        input = input.parquet(ParquetInput::builder().build());
    }
    input
        .set_csv(csv)
        .set_json(json)
        .compression_type(CompressionType::from(
            compression_type(opts, object).as_str(),
        ))
        .build()
}

/// mc `selectObjectOutputOpts`.
pub fn output_serialization(opts: &SelectOpts, input: &InputSerialization) -> OutputSerialization {
    let mut json = opts.json_output.as_ref().map(|map| {
        JsonOutput::builder()
            .record_delimiter(map.get("recorddelimiter").cloned().unwrap_or("\n".into()))
            .build()
    });
    let mut csv = opts.csv_output.as_ref().map(|map| {
        let get = |key: &str| map.get(key).cloned();
        let mut c = CsvOutput::builder()
            .record_delimiter(get("recorddelimiter").unwrap_or_else(|| "\n".into()))
            .field_delimiter(get("fielddelimiter").unwrap_or_else(|| ",".into()));
        if let Some(value) = get("quotechar") {
            c = c.quote_character(value);
        }
        if let Some(value) = get("quoteescchar") {
            c = c.quote_escape_character(value);
        }
        if let Some(value) = get("quotefields") {
            c = c.quote_fields(QuoteFields::from(value.as_str()));
        }
        c.build()
    });
    if csv.is_none() && json.is_none() {
        if input.json.is_some() {
            json = Some(JsonOutput::builder().record_delimiter("\n").build());
        } else {
            csv = Some(
                CsvOutput::builder()
                    .record_delimiter("\n")
                    .field_delimiter(",")
                    .build(),
            );
        }
    }
    OutputSerialization::builder()
        .set_csv(csv)
        .set_json(json)
        .build()
}

/// Records of a running select.
pub struct SelectStream {
    events: EventReceiver<SelectObjectContentEventStream, SelectObjectContentEventStreamError>,
}

impl SelectStream {
    /// Next chunk of records; `None` at the end of the stream. Error events become minio-go's
    /// `CODE:"MESSAGE"` errors.
    pub async fn next(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            let event = self.events.recv().await.map_err(|err| event_error(&err))?;
            match event {
                None | Some(SelectObjectContentEventStream::End(_)) => return Ok(None),
                Some(SelectObjectContentEventStream::Records(records)) => {
                    if let Some(payload) = records.payload {
                        return Ok(Some(payload.into_inner()));
                    }
                }
                Some(_) => {}
            }
        }
    }
}

/// minio-go's error for a failed record stream: error messages (`:message-type: error`,
/// which the SDK does not model) become `CODE:"MESSAGE"`.
fn event_error(err: &SdkError<SelectObjectContentEventStreamError, RawMessage>) -> McError {
    let header = |message: &Message, name: &str| {
        message
            .headers()
            .iter()
            .find(|h| h.name().as_str() == name)
            .and_then(|h| h.value().as_string().ok())
            .map(|value| value.as_str().to_string())
    };
    if let SdkError::ResponseError(context) = err
        && let RawMessage::Decoded(message) = context.raw()
        && header(message, ":message-type").as_deref() == Some("error")
    {
        return McError::new(format!(
            "{}:\"{}\"",
            header(message, ":error-code").unwrap_or_default(),
            header(message, ":error-message").unwrap_or_default()
        ));
    }
    match err.code() {
        Some(code) => McError::new(format!("{code}:\"{}\"", err.message().unwrap_or_default())),
        None => McError::new(err.to_string()),
    }
}

/// Starts `SelectObjectContent` (mc `S3Client.Select`); `sse_c` is the SSE-C customer key.
pub async fn select(
    client: &Client,
    bucket: &str,
    key: &str,
    expression: &str,
    sse_c: Option<[u8; 32]>,
    opts: &SelectOpts,
) -> Result<SelectStream> {
    use super::error::S3ResultExt;
    let input = input_serialization(opts, key);
    let output = output_serialization(opts, &input);
    let mut request = client
        .select_object_content()
        .bucket(bucket)
        .key(key)
        .expression(expression)
        .expression_type(ExpressionType::Sql)
        .input_serialization(input)
        .output_serialization(output);
    if let Some(customer_key) = sse_c {
        let (algorithm, encoded, md5) = super::objects::sse_c_headers(&customer_key);
        request = request
            .sse_customer_algorithm(algorithm)
            .sse_customer_key(encoded)
            .sse_customer_key_md5(md5);
    }
    // minio-go decodes select errors without the object name.
    let output = request.send().await.s3(bucket, "")?;
    Ok(SelectStream {
        events: output.payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> OptMap {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn kv_args_follow_mc() {
        assert_eq!(
            parse_kv_args("rd=\\n,FH=USE,fd=;").unwrap(),
            map(&[("rd", "\n"), ("fh", "USE"), ("fd", ";")])
        );
        // A comma value: `fd=,` followed by another key.
        assert_eq!(
            parse_kv_args("fd=,,rd=x").unwrap(),
            map(&[("fd", ","), ("rd", "x")])
        );
        assert_eq!(parse_kv_args("fd=").unwrap(), map(&[("fd", "")]));
        assert_eq!(parse_kv_args("").unwrap(), OptMap::new());
        assert_eq!(
            parse_kv_args("rd").unwrap_err().to_string(),
            "Arguments should be of the form key=value,... "
        );
        assert_eq!(
            parse_kv_args("rd=a,RD=b").unwrap_err().to_string(),
            "More than one key=value found for RD"
        );
    }

    #[test]
    fn serialization_opts_expand_and_check_keys() {
        assert_eq!(
            parse_serialization_opts("rd=\\n,fh=USE", CSV_INPUT_KEYS, CSV_INPUT_ABBR).unwrap(),
            map(&[("recorddelimiter", "\n"), ("fileheader", "USE")])
        );
        assert_eq!(
            parse_serialization_opts("fd=1", JSON_OUTPUT_KEYS, JSON_OUTPUT_ABBR)
                .unwrap_err()
                .to_string(),
            "Options should be key-value pairs in the form key=value,... where valid key(s) are RecordDelimiter(rd) "
        );
        assert_eq!(
            parse_serialization_opts("t=lines", JSON_INPUT_KEYS, &[])
                .unwrap_err()
                .to_string(),
            "Options should be key-value pairs in the form key=value,... where valid key(s) are Type "
        );
        assert!(
            parse_serialization_opts("type=lines", JSON_INPUT_KEYS, &[])
                .unwrap()
                .contains_key("type")
        );
    }

    #[test]
    fn input_defaults_come_from_the_object_name() {
        let opts = SelectOpts::default();
        let csv = input_serialization(&opts, "dir/data.csv.gz");
        let c = csv.csv.as_ref().unwrap();
        assert_eq!(c.record_delimiter.as_deref(), Some("\n"));
        assert_eq!(c.field_delimiter.as_deref(), Some(","));
        assert_eq!(c.file_header_info, Some(FileHeaderInfo::Use));
        assert_eq!(csv.compression_type, Some(CompressionType::Gzip));
        let json = input_serialization(&opts, "a.json");
        assert_eq!(json.json.unwrap().r#type, Some(JsonType::Lines));
        assert_eq!(json.compression_type, Some(CompressionType::None));
        let parquet = input_serialization(&opts, "x.parquet");
        assert!(parquet.parquet.is_some() && parquet.csv.is_none());
        let none = input_serialization(&opts, "a.txt");
        assert!(none.csv.is_none() && none.json.is_none());
        assert_eq!(
            input_serialization(&opts, "a.csv.bz2").compression_type,
            Some(CompressionType::Bzip2)
        );
    }

    #[test]
    fn explicit_input_and_output_options() {
        let opts = SelectOpts {
            csv_input: Some(map(&[("fielddelimiter", ";"), ("fileheader", "IGNORE")])),
            csv_output: Some(map(&[("quotefields", "ALWAYS")])),
            compression: "GZIP".into(),
            ..Default::default()
        };
        let input = input_serialization(&opts, "a.json");
        assert!(input.json.is_none());
        let c = input.csv.as_ref().unwrap();
        assert_eq!(c.field_delimiter.as_deref(), Some(";"));
        assert_eq!(c.quote_character, None);
        assert_eq!(c.file_header_info, Some(FileHeaderInfo::Ignore));
        assert_eq!(input.compression_type, Some(CompressionType::Gzip));
        let output = output_serialization(&opts, &input);
        let o = output.csv.unwrap();
        assert_eq!(o.quote_fields, Some(QuoteFields::Always));
        assert_eq!(o.field_delimiter.as_deref(), Some(","));
        assert!(output.json.is_none());

        // Default output follows the input: JSON input -> JSON lines.
        let input = input_serialization(&SelectOpts::default(), "a.json");
        let output = output_serialization(&SelectOpts::default(), &input);
        assert_eq!(output.json.unwrap().record_delimiter.as_deref(), Some("\n"));
        assert!(output.csv.is_none());

        // `--json` with `--csv-output`: both.
        let opts = SelectOpts {
            csv_output: Some(OptMap::new()),
            json_output: Some(map(&[("recorddelimiter", "\n\n")])),
            ..Default::default()
        };
        let output = output_serialization(&opts, &input);
        assert!(output.csv.is_some());
        assert_eq!(
            output.json.unwrap().record_delimiter.as_deref(),
            Some("\n\n")
        );
    }

    #[test]
    fn mime_types_like_mimedb() {
        assert_eq!(type_by_extension("a/b.csv"), "text/csv");
        assert_eq!(type_by_extension("b.json"), "application/json");
        assert!(type_by_extension("b.gz").contains("gzip"));
        assert_eq!(type_by_extension("noext"), "");
        assert_eq!(ext("a.b/c"), "");
    }
}
