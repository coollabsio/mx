//! S3 access layer. Submodules are split by feature area to limit merge conflicts; everything
//! public is re-exported here so callers can keep using `crate::s3::<item>`.
//!
//! | module          | contents                                              | owner |
//! |-----------------|-------------------------------------------------------|-------|
//! | `client`        | client construction, TLS/resolve, interceptors         | A     |
//! | `list`          | listing, versions, rewind, incomplete uploads          | C     |
//! | `stat`          | HeadBucket/HeadObject                                 | C     |
//! | `delete`        | object/prefix/version deletion                        | C     |
//! | `objects`       | Put/Get/Copy options and single-object transfer        | B/E   |
//! | `multipart`     | streaming multipart upload engine                      | B/E   |
//! | `bucket`        | mb/rb, versioning, CORS, encryption, policy, tags, presign, ping | F |
//! | `lifecycle`     | ILM rules                                              | F     |
//! | `lock`          | object lock: retention, legal hold                     | G     |
//! | `notify`        | bucket notifications                                   | G     |
//! | `admin`         | MinIO admin API (quota, tiers, ...)                    | H     |
//! | `admin_info`    | admin ServerInfo (ping -a/--node)                      | K     |
//! | `replication`   | bucket replication                                     | H     |
//! | `io_ext`        | if-none-match client, rewind lookup, find metadata/tags | E     |

pub mod admin;
pub mod admin_info;
pub mod bucket;
pub mod client;
pub mod delete;
pub mod error;
pub mod io_ext;
pub mod lifecycle;
pub mod list;
pub mod lock;
pub mod multipart;
pub mod notify;
pub mod objects;
pub mod replication;
pub mod stat;

pub use bucket::*;
pub use client::{build_client, force_path_style};
pub use delete::*;
pub use error::{S3ResultExt, s3_error, s3_object_error};
pub use io_ext::*;
pub use lifecycle::*;
pub use list::*;
pub use multipart::*;
pub use objects::*;
pub use stat::*;

pub use crate::flags::{ChecksumAlgo, Sse};

use std::time::SystemTime;

/// Formats an SDK timestamp the way `mx` has always printed it (RFC3339, `Z`).
pub(crate) fn debug_timestamp<T: std::fmt::Debug>(value: &T) -> String {
    format!("{value:?}")
}

/// Converts an SDK timestamp to `SystemTime` (None if out of range).
pub fn to_system_time(value: &aws_sdk_s3::primitives::DateTime) -> Option<SystemTime> {
    SystemTime::try_from(*value).ok()
}

/// Converts a `SystemTime` to an SDK timestamp.
pub fn from_system_time(value: SystemTime) -> aws_sdk_s3::primitives::DateTime {
    aws_sdk_s3::primitives::DateTime::from(value)
}

/// Returns the S3 error code (e.g. `NoSuchKey`) of an SDK error, if any.
pub fn error_code<E, R>(error: &aws_sdk_s3::error::SdkError<E, R>) -> Option<&str>
where
    E: aws_sdk_s3::error::ProvideErrorMetadata,
{
    use aws_sdk_s3::error::ProvideErrorMetadata;
    error
        .as_service_error()
        .and_then(ProvideErrorMetadata::code)
}
