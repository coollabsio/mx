//! mc-style errors.
//!
//! mc reports every failure as `MESSAGE CAUSE`: the message is the command's context
//! (`Unable to list folder.`), the cause is mc's typed error (``Bucket `b` does not exist.``)
//! or the server's error message. In `mx` the message is the outermost anyhow context and the
//! cause is the rest of the chain; [`McError`] carries mc's cause text plus the Go-marshaled
//! error value that mc prints as `cause.error` with `--json`.
//!
//! - Raise a cause: `Err(McError::object_missing())?` / `bail!(McError::path_not_found(p))`.
//! - Attach mc's message: `.context("Unable to stat `x`.")` (reported like mc `fatalIf`, JSON
//!   `type: "fatal"`) or `.context(nonfatal("Unable to list folder."))` for commands where mc
//!   uses `errorIf` + exit status 1 (JSON `type: "error"`, no punctuation added).
//! - S3 SDK errors: map with [`crate::s3::error::S3ResultExt::s3`] in the s3 layer.

use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::Value;
use std::fmt;

/// Go-marshaled error value: a JSON object whose fields keep their (Go struct) order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Detail(pub Vec<(String, Value)>);

impl Detail {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }
}

impl Serialize for Detail {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

/// `detail![("Bucket", bucket), ...]`: an ordered [`Detail`].
#[macro_export]
macro_rules! detail {
    ($(($key:expr, $value:expr)),* $(,)?) => {
        $crate::error::Detail(vec![$(($key.to_string(), serde_json::json!($value))),*])
    };
}

/// mc typed error / minio-go `ErrorResponse`: `message` is mc's cause text, `detail` is what
/// Go's `json.Marshal` prints for the error value (`{}` for errors without exported fields).
#[derive(Debug, Clone, PartialEq)]
pub struct McError {
    pub message: String,
    pub detail: Detail,
    /// S3 error code (`NoSuchKey`, ...) when the error came from the server.
    pub code: Option<String>,
}

impl McError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: Detail::default(),
            code: None,
        }
    }

    pub fn with_detail(message: impl Into<String>, detail: Detail) -> Self {
        Self {
            detail,
            ..Self::new(message)
        }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// mc `BucketDoesNotExist`.
    pub fn bucket_not_found(bucket: &str) -> Self {
        Self::with_detail(
            format!("Bucket `{bucket}` does not exist."),
            crate::detail![("Bucket", bucket)],
        )
        .with_code("NoSuchBucket")
    }

    /// mc `BucketInvalid`.
    pub fn bucket_invalid(bucket: &str) -> Self {
        Self::with_detail(
            format!("Bucket name `{bucket}` not valid."),
            crate::detail![("Bucket", bucket)],
        )
        .with_code("InvalidBucketName")
    }

    /// mc `BucketNameEmpty`.
    pub fn bucket_name_empty() -> Self {
        Self::new("Bucket name cannot be empty.")
    }

    /// mc `ObjectNameEmpty`.
    pub fn object_name_empty() -> Self {
        Self::new("Object name cannot be empty.")
    }

    /// mc `ObjectMissing`.
    pub fn object_missing() -> Self {
        Self::new("Object does not exist").with_code("NoSuchKey")
    }

    /// mc `ObjectMissing` with a rewind time (`Object did not exist at `RFC1123``).
    pub fn object_missing_at(time: &str) -> Self {
        Self::new(format!("Object did not exist at `{time}`")).with_code("NoSuchKey")
    }

    /// mc `ObjectIsDeleteMarker`.
    pub fn object_is_delete_marker() -> Self {
        Self::new("Object is marked as deleted")
    }

    /// mc `PathNotFound` (local paths and unknown aliases, which mc treats as local paths).
    pub fn path_not_found(path: &str) -> Self {
        Self::with_detail(
            format!("Requested path `{path}` not found"),
            crate::detail![("Path", path)],
        )
    }

    /// mc `PathInsufficientPermission`.
    pub fn path_insufficient_permission(path: &str) -> Self {
        Self::with_detail(
            format!("Insufficient permissions to access this path `{path}`"),
            crate::detail![("Path", path)],
        )
    }

    /// mc `PathIsNotRegular`.
    pub fn path_not_regular(path: &str) -> Self {
        Self::with_detail(
            format!("Requested path `{path}` is not a regular file."),
            crate::detail![("Path", path)],
        )
    }

    /// mc `errInvalidArgument`.
    pub fn invalid_argument() -> Self {
        Self::new(
            "Invalid arguments provided, please refer `mc <command> -h` for relevant documentation.",
        )
    }

    /// mc `errInvalidAliasedURL`.
    pub fn invalid_aliased_url(url: &str) -> Self {
        Self::new(format!(
            "Use `mc alias set mycloud {url} ...` to add an alias. Use the alias for S3 operations."
        ))
    }
}

impl fmt::Display for McError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for McError {}

/// Context message for failures mc reports with `errorIf` and then exits with status 1
/// (JSON `type: "error"`, message and cause joined with a space).
#[derive(Debug, Clone)]
pub struct NonFatal(pub String);

impl fmt::Display for NonFatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `.context(nonfatal("Unable to list folder."))`, see [`NonFatal`].
pub fn nonfatal(message: impl Into<String>) -> NonFatal {
    NonFatal(message.into())
}

/// The first [`McError`] in an anyhow chain.
pub fn mc_error(err: &anyhow::Error) -> Option<&McError> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<McError>())
}

/// S3 error code of an anyhow error mapped by the s3 layer (`NoSuchKey`, ...).
pub fn error_code(err: &anyhow::Error) -> Option<&str> {
    mc_error(err).and_then(|e| e.code.as_deref())
}

/// True when `err` is a missing bucket or object (`NoSuchBucket` / `NoSuchKey` / 404).
pub fn is_not_found(err: &anyhow::Error) -> bool {
    matches!(
        error_code(err),
        Some("NoSuchBucket" | "NoSuchKey" | "NoSuchVersion" | "NotFound")
    )
}

/// Absolute form of a local path the way mc prints it (`filepath.Abs`, no symlink resolution).
pub fn abs_path(path: &str) -> String {
    let path = std::path::Path::new(path);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    // Lexical clean like Go `filepath.Clean`.
    let mut parts: Vec<&str> = Vec::new();
    let text = joined.to_string_lossy().into_owned();
    for part in text.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

/// mc `PathNotFound` for a local path (made absolute like mc).
pub fn local_not_found(path: &str) -> McError {
    McError::path_not_found(&abs_path(path))
}

/// Maps a local I/O error on `path` to mc's typed error where mc has one.
pub fn io_error(err: &std::io::Error, path: &str) -> McError {
    match err.kind() {
        std::io::ErrorKind::NotFound => local_not_found(path),
        std::io::ErrorKind::PermissionDenied => {
            McError::path_insufficient_permission(&abs_path(path))
        }
        _ => McError::new(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use serde_json::json;

    #[test]
    fn typed_errors_use_mc_text_and_detail() {
        let err = McError::bucket_not_found("b");
        assert_eq!(err.to_string(), "Bucket `b` does not exist.");
        assert_eq!(
            serde_json::to_value(&err.detail).unwrap(),
            json!({"Bucket": "b"})
        );
        assert_eq!(
            McError::object_missing().to_string(),
            "Object does not exist"
        );
        assert!(McError::object_missing().detail.is_empty());
        assert_eq!(
            McError::path_not_found("/x").to_string(),
            "Requested path `/x` not found"
        );
    }

    #[test]
    fn finds_mc_error_and_code_in_chain() {
        let err = Err::<(), _>(McError::object_missing())
            .context("Unable to stat")
            .unwrap_err();
        assert_eq!(error_code(&err), Some("NoSuchKey"));
        assert!(is_not_found(&err));
        assert!(!is_not_found(&anyhow::anyhow!("x")));
    }

    #[test]
    fn abs_path_cleans_lexically() {
        assert_eq!(abs_path("/a/./b/../c"), "/a/c");
        assert!(abs_path("rel").ends_with("/rel"));
    }
}
